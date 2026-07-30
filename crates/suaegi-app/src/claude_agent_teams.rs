//! Claude Code Agent Teams native-pane compatibility.
//!
//! Claude's pane backend talks to `tmux`. Orca puts a private `tmux` shim at
//! the front of PATH and translates the bounded command subset into native
//! terminal operations. Suaegi mirrors that contract here. Team topology is
//! kept in a private, short-lived file because each shim invocation is a
//! separate CLI process; credentials and the leader environment are never
//! written to it.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};

const LEADER_PANE: &str = "%1";
const MAX_STATE_BYTES: u64 = 1024 * 1024;
const LOCK_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TeamPane {
    fake_pane_id: String,
    handle: String,
    index: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    split_from_pane: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    split_direction: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MainVertical {
    main_pane: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_column_pane: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentTeam {
    team_id: String,
    token_hash: String,
    leader_pane: String,
    leader_handle: String,
    session_name: String,
    window_index: String,
    tmux_value: String,
    panes: BTreeMap<String, TeamPane>,
    pane_order: Vec<String>,
    next_pane_number: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    main_vertical: Option<MainVertical>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previously_focused_pane: Option<String>,
}

#[derive(Debug, Default)]
struct ParsedArgs {
    flags: BTreeSet<String>,
    values: BTreeMap<String, Vec<String>>,
    positional: Vec<String>,
}

impl ParsedArgs {
    fn value(&self, flag: &str) -> Option<&str> {
        self.values
            .get(flag)
            .and_then(|values| values.last())
            .map(String::as_str)
    }
}

struct LockGuard(PathBuf);

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub fn run_claude(args: &[String]) -> Result<i32, String> {
    if cfg!(target_os = "windows") {
        return Err("Claude Agent Teams native panes are not supported on Windows.".into());
    }
    let pane_key = std::env::var("ORCA_PANE_KEY")
        .or_else(|_| std::env::var("SUAEGI_PANE_KEY"))
        .map_err(|_| "suaegi claude-teams must be run inside a Suaegi terminal.".to_string())?;
    if pane_key.trim().is_empty() {
        return Err("suaegi claude-teams must be run inside a Suaegi terminal.".into());
    }
    let leader_handle = std::env::var("ORCA_TERMINAL_HANDLE")
        .or_else(|_| std::env::var("SUAEGI_TERMINAL_HANDLE"))
        .map_err(|_| "The current Suaegi terminal handle is unavailable.".to_string())?;
    rpc("terminal.show", json!({"terminal": leader_handle}))?;

    let team_id = format!("team-{}", random_url_token(16)?);
    let token = random_url_token(32)?;
    let shim_dir = ensure_shim_dir()?;
    let executable = std::env::current_exe()
        .map_err(|error| format!("Could not locate the Suaegi CLI: {error}"))?;
    let tmux_value = format!("/tmp/suaegi-claude-agent-teams/{team_id},0,1");
    let leader = TeamPane {
        fake_pane_id: LEADER_PANE.into(),
        handle: leader_handle.clone(),
        index: 0,
        split_from_pane: None,
        split_direction: None,
    };
    let team = AgentTeam {
        team_id: team_id.clone(),
        token_hash: sha256_hex(&token),
        leader_pane: LEADER_PANE.into(),
        leader_handle,
        session_name: "suaegi".into(),
        window_index: "0".into(),
        tmux_value: tmux_value.clone(),
        panes: BTreeMap::from([(LEADER_PANE.into(), leader)]),
        pane_order: vec![LEADER_PANE.into()],
        next_pane_number: 2,
        main_vertical: None,
        previously_focused_pane: None,
    };
    write_team(&team)?;

    let inherited_path = std::env::var_os("PATH").unwrap_or_default();
    let mut path = std::ffi::OsString::from(shim_dir.as_os_str());
    path.push(":");
    path.push(inherited_path);
    let mut forwarded = args.to_vec();
    if !has_teammate_mode(&forwarded) {
        forwarded.splice(0..0, ["--teammate-mode".into(), "auto".into()]);
    }
    let claude_binary =
        std::env::var("SUAEGI_AGENT_TEAMS_CLAUDE_BIN").unwrap_or_else(|_| "claude".into());
    let status = std::process::Command::new(claude_binary)
        .args(&forwarded)
        .env("CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS", "1")
        .env("PATH", path)
        .env("TMUX", tmux_value)
        .env("TMUX_PANE", LEADER_PANE)
        .env("ORCA_AGENT_TEAMS_TEAM_ID", &team_id)
        .env("ORCA_AGENT_TEAMS_TOKEN", &token)
        .env("ORCA_AGENT_TEAMS_LEADER_PANE", LEADER_PANE)
        .env("ORCA_AGENT_TEAMS_SHIM_DIR", &shim_dir)
        .env("ORCA_AGENT_TEAMS_SHIM_BIN", executable)
        .env_remove("ELECTRON_RUN_AS_NODE")
        .status()
        .map_err(|error| format!("Could not start Claude Agent Teams: {error}"));
    let _ = fs::remove_file(team_path(&team_id));
    status.map(|status| status.code().unwrap_or(1))
}

pub fn run_tmux(argv: &[String]) -> Result<i32, String> {
    let team_id = required_env("ORCA_AGENT_TEAMS_TEAM_ID")?;
    let token = required_env("ORCA_AGENT_TEAMS_TOKEN")?;
    let env_pane = required_env("TMUX_PANE")?;
    let (command, args) = split_tmux_command(argv)?;
    let response = with_team(&team_id, &token, |team| {
        if !team.panes.contains_key(&env_pane) {
            return Err(format!("unknown pane: {env_pane}"));
        }
        dispatch(team, &command, &args, &env_pane)
    });
    match response {
        Ok(stdout) => {
            print!("{stdout}");
            Ok(0)
        }
        Err(error) => {
            eprintln!("tmux: {error}");
            Ok(1)
        }
    }
}

fn dispatch(
    team: &mut AgentTeam,
    command: &str,
    args: &[String],
    env_pane: &str,
) -> Result<String, String> {
    match command {
        "-V" | "-v" => Ok("tmux 3.4\n".into()),
        "show-options" | "show-option" | "show" => show_options(args),
        "display-message" | "display" | "displayp" => display_message(team, args, env_pane),
        "split-window" | "splitw" => split_window(team, args, env_pane),
        "respawn-pane" | "respawnp" => respawn_pane(team, args, env_pane),
        "select-layout" => select_layout(team, args, env_pane),
        "resize-pane" | "resizep" => Ok(String::new()),
        "list-panes" | "lsp" => list_panes(team, args, env_pane),
        "send-keys" | "send" => send_keys(team, args, env_pane),
        "capture-pane" | "capturep" => capture_pane(team, args, env_pane),
        "select-pane" | "selectp" => select_pane(team, args, env_pane),
        "kill-pane" | "killp" => kill_pane(team, args, env_pane),
        "last-pane" => last_pane(team, args),
        "set-option" | "set" | "set-window-option" | "setw" | "set-hook" | "refresh-client"
        | "attach-session" | "detach-client" | "source-file" | "wait-for" | "has-session"
        | "has" => Ok(String::new()),
        _ => Err(format!("unsupported command: {command}")),
    }
}

fn show_options(args: &[String]) -> Result<String, String> {
    let parsed = parse_tmux_args(args, &["-t"], &["-g", "-q", "-s", "-v", "-w"]);
    let option = parsed
        .positional
        .last()
        .map(String::as_str)
        .unwrap_or_default();
    if option != "extended-keys" {
        return Err(format!("unsupported option: {option}"));
    }
    Ok(if parsed.flags.contains("-v") {
        "on\n"
    } else {
        "extended-keys on\n"
    }
    .into())
}

fn display_message(team: &AgentTeam, args: &[String], env_pane: &str) -> Result<String, String> {
    let parsed = parse_tmux_args(args, &["-F", "-t"], &["-p"]);
    let target = parsed.value("-t").unwrap_or(env_pane);
    let pane = if is_window_target(team, target) {
        resolve_pane(team, env_pane)?
    } else {
        resolve_pane(team, target)?
    };
    let format = if parsed.positional.is_empty() {
        parsed.value("-F")
    } else {
        None
    };
    let owned;
    let format = if let Some(format) = format {
        format
    } else {
        owned = parsed.positional.join(" ");
        &owned
    };
    Ok(format!("{}\n", render_format(format, team, pane, "")))
}

fn split_window(team: &mut AgentTeam, args: &[String], env_pane: &str) -> Result<String, String> {
    let parsed = parse_tmux_args(
        args,
        &["-c", "-F", "-l", "-t"],
        &["-P", "-b", "-d", "-f", "-h", "-v"],
    );
    let target = resolve_pane(team, parsed.value("-t").unwrap_or(env_pane))?.clone();
    let fake_pane_id = format!("%{}", team.next_pane_number);
    team.next_pane_number += 1;
    let (origin, direction) = split_target(team, &target, parsed.flags.contains("-h"));
    let origin_handle = origin.handle.clone();
    let origin_pane = origin.fake_pane_id.clone();
    let command = parsed.positional.join(" ");
    let result = rpc(
        "terminal.split",
        json!({
            "terminal": origin_handle,
            "direction": direction,
            "command": (!command.is_empty()).then_some(command),
            "focus": false,
            "env": child_env(&fake_pane_id),
            "envToDelete": ["TERM_PROGRAM", "ORCA_ATTRIBUTION_SHIM_DIR"],
        }),
    )?;
    let handle = result
        .pointer("/terminal/handle")
        .and_then(Value::as_str)
        .ok_or_else(|| "Suaegi returned no split terminal handle.".to_string())?
        .to_string();
    let pane = TeamPane {
        fake_pane_id: fake_pane_id.clone(),
        handle,
        index: team.pane_order.len(),
        split_from_pane: Some(origin_pane.clone()),
        split_direction: Some(direction.to_string()),
    };
    team.panes.insert(fake_pane_id.clone(), pane.clone());
    team.pane_order.push(fake_pane_id.clone());
    update_main_vertical(team, &fake_pane_id, &origin_pane, direction);
    if !parsed.flags.contains("-P") {
        return Ok(String::new());
    }
    Ok(format!(
        "{}\n",
        render_format(
            parsed.value("-F").unwrap_or_default(),
            team,
            &pane,
            &fake_pane_id
        )
    ))
}

fn respawn_pane(team: &mut AgentTeam, args: &[String], env_pane: &str) -> Result<String, String> {
    let parsed = parse_tmux_args(args, &["-c", "-e", "-t"], &["-k"]);
    let target_id = parsed.value("-t").unwrap_or(env_pane).to_string();
    let pane = resolve_pane(team, &target_id)?.clone();
    if pane.fake_pane_id == team.leader_pane {
        return Err("refusing to respawn leader pane".into());
    }
    let command = parsed.positional.join(" ");
    if command.is_empty() {
        return Ok(String::new());
    }
    let origin = pane
        .split_from_pane
        .as_ref()
        .and_then(|id| team.panes.get(id))
        .or_else(|| team.panes.get(&team.leader_pane))
        .ok_or_else(|| "leader pane is missing".to_string())?
        .clone();
    let result = rpc(
        "terminal.split",
        json!({
            "terminal": origin.handle,
            "direction": pane.split_direction.as_deref().unwrap_or("horizontal"),
            "command": command,
            "focus": false,
            "env": child_env(&pane.fake_pane_id),
            "envToDelete": ["TERM_PROGRAM", "ORCA_ATTRIBUTION_SHIM_DIR"],
        }),
    )?;
    let replacement = result
        .pointer("/terminal/handle")
        .and_then(Value::as_str)
        .ok_or_else(|| "Suaegi returned no replacement terminal handle.".to_string())?
        .to_string();
    if let Err(error) = rpc("terminal.close", json!({"terminal": pane.handle})) {
        let _ = rpc("terminal.close", json!({"terminal": replacement}));
        return Err(error);
    }
    if let Some(registered) = team.panes.get_mut(&target_id) {
        registered.handle = replacement;
    }
    Ok(String::new())
}

fn select_layout(team: &mut AgentTeam, args: &[String], env_pane: &str) -> Result<String, String> {
    let parsed = parse_tmux_args(args, &["-t"], &[]);
    let layout = parsed
        .positional
        .first()
        .map(String::as_str)
        .unwrap_or_default();
    if layout == "main-vertical" {
        let target = parsed.value("-t").unwrap_or(env_pane);
        let target_pane = (!is_window_target(team, target))
            .then(|| resolve_pane(team, target))
            .transpose()?;
        let previous = team
            .main_vertical
            .as_ref()
            .and_then(|layout| layout.last_column_pane.clone());
        team.main_vertical = Some(MainVertical {
            main_pane: team.leader_pane.clone(),
            last_column_pane: previous.or_else(|| {
                target_pane
                    .filter(|pane| pane.fake_pane_id != team.leader_pane)
                    .map(|pane| pane.fake_pane_id.clone())
            }),
        });
    } else if !layout.is_empty() {
        team.main_vertical = None;
    }
    Ok(String::new())
}

fn list_panes(team: &AgentTeam, args: &[String], env_pane: &str) -> Result<String, String> {
    let parsed = parse_tmux_args(args, &["-F", "-t"], &[]);
    let target = parsed.value("-t").unwrap_or(env_pane);
    if !is_window_target(team, target) {
        resolve_pane(team, target)?;
    }
    let format = parsed.value("-F").unwrap_or_default();
    let mut lines = Vec::new();
    for id in &team.pane_order {
        let pane = team
            .panes
            .get(id)
            .ok_or_else(|| format!("unknown pane: {id}"))?;
        lines.push(render_format(format, team, pane, &pane.fake_pane_id));
    }
    Ok(format!("{}\n", lines.join("\n")))
}

fn send_keys(team: &AgentTeam, args: &[String], env_pane: &str) -> Result<String, String> {
    let parsed = parse_tmux_args(args, &["-t"], &["-l"]);
    let pane = resolve_pane(team, parsed.value("-t").unwrap_or(env_pane))?;
    let text = send_keys_text(&parsed.positional, parsed.flags.contains("-l"));
    if !text.is_empty() {
        rpc(
            "terminal.send",
            json!({"terminal": pane.handle, "text": text}),
        )?;
    }
    Ok(String::new())
}

fn capture_pane(team: &AgentTeam, args: &[String], env_pane: &str) -> Result<String, String> {
    let parsed = parse_tmux_args(args, &["-E", "-S", "-t"], &["-J", "-N", "-p"]);
    let pane = resolve_pane(team, parsed.value("-t").unwrap_or(env_pane))?;
    let result = rpc(
        "terminal.read",
        json!({"terminal": pane.handle, "limit": 1_000}),
    )?;
    let output = result
        .pointer("/terminal/output")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Ok(if parsed.flags.contains("-p") {
        format!("{output}\n")
    } else {
        String::new()
    })
}

fn select_pane(team: &mut AgentTeam, args: &[String], env_pane: &str) -> Result<String, String> {
    let parsed = parse_tmux_args(args, &["-P", "-T", "-t"], &[]);
    if parsed.value("-P").is_some() || parsed.value("-T").is_some() {
        return Ok(String::new());
    }
    let pane = resolve_pane(team, parsed.value("-t").unwrap_or(env_pane))?.clone();
    team.previously_focused_pane = Some(env_pane.to_string());
    rpc("terminal.focus", json!({"terminal": pane.handle}))?;
    Ok(String::new())
}

fn kill_pane(team: &mut AgentTeam, args: &[String], env_pane: &str) -> Result<String, String> {
    let parsed = parse_tmux_args(args, &["-t"], &[]);
    let pane = resolve_pane(team, parsed.value("-t").unwrap_or(env_pane))?.clone();
    if pane.fake_pane_id == team.leader_pane {
        return Err("refusing to kill leader pane".into());
    }
    rpc("terminal.close", json!({"terminal": pane.handle}))?;
    team.panes.remove(&pane.fake_pane_id);
    team.pane_order.retain(|id| id != &pane.fake_pane_id);
    if team
        .main_vertical
        .as_ref()
        .and_then(|layout| layout.last_column_pane.as_ref())
        == Some(&pane.fake_pane_id)
    {
        let replacement = team
            .pane_order
            .iter()
            .rev()
            .find(|id| *id != &team.leader_pane)
            .cloned();
        if let Some(layout) = &mut team.main_vertical {
            layout.last_column_pane = replacement;
        }
    }
    Ok(String::new())
}

fn last_pane(team: &AgentTeam, args: &[String]) -> Result<String, String> {
    let _ = parse_tmux_args(args, &["-t"], &[]);
    if let Some(pane) = team
        .previously_focused_pane
        .as_ref()
        .and_then(|id| team.panes.get(id))
    {
        rpc("terminal.focus", json!({"terminal": pane.handle}))?;
    }
    Ok(String::new())
}

fn split_target<'a>(
    team: &'a AgentTeam,
    target: &'a TeamPane,
    horizontal: bool,
) -> (&'a TeamPane, &'static str) {
    if horizontal {
        if let Some(last) = team
            .main_vertical
            .as_ref()
            .and_then(|layout| layout.last_column_pane.as_ref())
            .and_then(|id| team.panes.get(id))
        {
            return (last, "horizontal");
        }
    }
    (target, if horizontal { "vertical" } else { "horizontal" })
}

fn update_main_vertical(team: &mut AgentTeam, fake_pane_id: &str, origin: &str, direction: &str) {
    if let Some(layout) = &mut team.main_vertical {
        layout.last_column_pane = Some(fake_pane_id.to_string());
    } else if direction == "vertical" && origin == team.leader_pane {
        team.main_vertical = Some(MainVertical {
            main_pane: team.leader_pane.clone(),
            last_column_pane: Some(fake_pane_id.to_string()),
        });
    }
}

fn child_env(fake_pane_id: &str) -> BTreeMap<String, String> {
    let mut env = std::env::vars()
        .filter(|(name, value)| {
            name.len() <= 256
                && value.len() <= 16 * 1024
                && !value.contains('\0')
                && !matches!(
                    name.as_str(),
                    "ORCA_TERMINAL_HANDLE"
                        | "SUAEGI_TERMINAL_HANDLE"
                        | "ORCA_PANE_KEY"
                        | "SUAEGI_PANE_KEY"
                        | "TERM_PROGRAM"
                        | "ORCA_ATTRIBUTION_SHIM_DIR"
                )
        })
        .collect::<BTreeMap<_, _>>();
    env.insert("TMUX_PANE".into(), fake_pane_id.into());
    env.insert("ORCA_AGENT_TEAMS_LEADER_PANE".into(), LEADER_PANE.into());
    env
}

fn split_tmux_command(argv: &[String]) -> Result<(String, Vec<String>), String> {
    let value_flags = ["-L", "-S", "-f"];
    let bool_flags = ["-V", "-v"];
    let mut index = 0;
    while index < argv.len() {
        let arg = &argv[index];
        if arg == "--" {
            break;
        }
        if !arg.starts_with('-') || arg == "-" {
            return Ok((arg.to_ascii_lowercase(), argv[index + 1..].to_vec()));
        }
        if bool_flags.contains(&arg.as_str()) {
            return Ok((arg.clone(), Vec::new()));
        }
        if value_flags.contains(&arg.as_str()) {
            index += 1;
        }
        index += 1;
    }
    Err("tmux shim requires a command".into())
}

fn parse_tmux_args(args: &[String], value_flags: &[&str], bool_flags: &[&str]) -> ParsedArgs {
    let value_flags = value_flags.iter().copied().collect::<BTreeSet<_>>();
    let bool_flags = bool_flags.iter().copied().collect::<BTreeSet<_>>();
    let mut parsed = ParsedArgs::default();
    let mut index = 0;
    let mut past_terminator = false;
    while index < args.len() {
        let arg = &args[index];
        if past_terminator {
            parsed.positional.push(arg.clone());
            index += 1;
            continue;
        }
        if arg == "--" {
            past_terminator = true;
            index += 1;
            continue;
        }
        if !arg.starts_with('-') || arg == "-" || arg.starts_with("--") {
            parsed.positional.push(arg.clone());
            index += 1;
            continue;
        }
        let cluster = &arg[1..];
        let characters = cluster.char_indices().collect::<Vec<_>>();
        let mut cursor = 0;
        let mut recognized = false;
        while cursor < characters.len() {
            let (_, character) = characters[cursor];
            let flag = format!("-{character}");
            if bool_flags.contains(flag.as_str()) {
                parsed.flags.insert(flag);
                recognized = true;
                cursor += 1;
                continue;
            }
            if value_flags.contains(flag.as_str()) {
                let start = characters
                    .get(cursor + 1)
                    .map(|(offset, _)| *offset)
                    .unwrap_or(cluster.len());
                let remainder = &cluster[start..];
                let value = if remainder.is_empty() {
                    index += 1;
                    args.get(index).cloned().unwrap_or_default()
                } else {
                    remainder.to_string()
                };
                parsed.values.entry(flag).or_default().push(value);
                recognized = true;
                cursor = characters.len();
                continue;
            }
            recognized = false;
            break;
        }
        if !recognized {
            parsed.positional.push(arg.clone());
        }
        index += 1;
    }
    parsed
}

fn send_keys_text(tokens: &[String], literal: bool) -> String {
    if literal {
        return tokens.join(" ");
    }
    let mut result = String::new();
    let mut pending_space = false;
    for token in tokens {
        if let Some(special) = special_key(token) {
            result.push_str(special);
            pending_space = false;
            continue;
        }
        if pending_space {
            result.push(' ');
        }
        result.push_str(token);
        pending_space = true;
    }
    result
}

fn special_key(token: &str) -> Option<&'static str> {
    match token.to_ascii_lowercase().as_str() {
        "enter" | "c-m" | "kpenter" => Some("\r"),
        "tab" | "c-i" => Some("\t"),
        "space" => Some(" "),
        "bspace" | "backspace" => Some("\x7f"),
        "escape" | "esc" | "c-[" => Some("\x1b"),
        "c-c" => Some("\x03"),
        "c-d" => Some("\x04"),
        "c-z" => Some("\x1a"),
        "c-l" => Some("\x0c"),
        _ => None,
    }
}

fn render_format(format: &str, team: &AgentTeam, pane: &TeamPane, fallback: &str) -> String {
    if format.is_empty() {
        return fallback.to_string();
    }
    let context = [
        ("session_name", team.session_name.as_str()),
        ("session_id", "$0"),
        ("window_id", "@0"),
        ("window_index", team.window_index.as_str()),
        ("window_name", "agent-teams"),
        ("window_active", "1"),
        ("window_flags", "*"),
        ("pane_id", pane.fake_pane_id.as_str()),
        (
            "pane_active",
            if pane.fake_pane_id == team.leader_pane {
                "1"
            } else {
                "0"
            },
        ),
    ];
    let mut rendered = format.to_string();
    for (key, value) in context {
        rendered = rendered.replace(&format!("#{{{key}}}"), value);
    }
    for key in [
        "pane_index",
        "pane_title",
        "pane_width",
        "pane_height",
        "pane_left",
        "pane_top",
        "window_width",
        "window_height",
    ] {
        let value = if key == "pane_index" {
            pane.index.to_string()
        } else {
            String::new()
        };
        rendered = rendered.replace(&format!("#{{{key}}}"), &value);
    }
    while let Some(start) = rendered.find("#{") {
        let Some(relative_end) = rendered[start + 2..].find('}') else {
            break;
        };
        rendered.replace_range(start..=start + 2 + relative_end, "");
    }
    let rendered = rendered.trim();
    if rendered.is_empty() {
        fallback.to_string()
    } else {
        rendered.to_string()
    }
}

fn is_window_target(team: &AgentTeam, target: &str) -> bool {
    target.contains(':') || target == team.session_name || target.starts_with('@')
}

fn resolve_pane<'a>(team: &'a AgentTeam, target: &str) -> Result<&'a TeamPane, String> {
    team.panes
        .get(target)
        .ok_or_else(|| format!("unknown pane: {target}"))
}

fn rpc(method: &str, params: Value) -> Result<Value, String> {
    crate::local_rpc::call(method, params)?.ok_or_else(|| "Suaegi is not running.".to_string())
}

fn with_team(
    team_id: &str,
    token: &str,
    action: impl FnOnce(&mut AgentTeam) -> Result<String, String>,
) -> Result<String, String> {
    validate_component(team_id)?;
    let _lock = lock_team(team_id)?;
    let path = team_path(team_id);
    let metadata = path
        .metadata()
        .map_err(|_| "stale or unauthorized agent team".to_string())?;
    if metadata.len() > MAX_STATE_BYTES {
        return Err("agent team state is too large".into());
    }
    let bytes = fs::read(&path).map_err(|_| "stale or unauthorized agent team".to_string())?;
    let mut team: AgentTeam = serde_json::from_slice(&bytes)
        .map_err(|_| "stale or unauthorized agent team".to_string())?;
    if !constant_time_equal(&team.token_hash, &sha256_hex(token)) || team.team_id != team_id {
        return Err("stale or unauthorized agent team".into());
    }
    let result = action(&mut team)?;
    write_team(&team)?;
    Ok(result)
}

fn lock_team(team_id: &str) -> Result<LockGuard, String> {
    let lock = team_path(team_id).with_extension("lock");
    let started = Instant::now();
    loop {
        match OpenOptions::new().write(true).create_new(true).open(&lock) {
            Ok(mut file) => {
                let _ = writeln!(file, "{}", std::process::id());
                return Ok(LockGuard(lock));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if !lock_owner_alive(&lock) {
                    let _ = fs::remove_file(&lock);
                    continue;
                }
                if started.elapsed() < LOCK_TIMEOUT {
                    std::thread::sleep(Duration::from_millis(20));
                    continue;
                }
                return Err("agent team is busy".into());
            }
            Err(error) => return Err(format!("Could not lock agent team state: {error}")),
        }
    }
}

fn write_team(team: &AgentTeam) -> Result<(), String> {
    let directory = teams_dir();
    fs::create_dir_all(&directory)
        .map_err(|error| format!("Could not create agent team state directory: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("Could not secure agent team state directory: {error}"))?;
    }
    let path = team_path(&team.team_id);
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let bytes = serde_json::to_vec(team)
        .map_err(|error| format!("Could not encode agent team state: {error}"))?;
    if bytes.len() as u64 > MAX_STATE_BYTES {
        return Err("agent team state is too large".into());
    }
    fs::write(&temporary, bytes)
        .map_err(|error| format!("Could not write agent team state: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("Could not secure agent team state: {error}"))?;
    }
    fs::rename(&temporary, &path)
        .map_err(|error| format!("Could not publish agent team state: {error}"))
}

fn ensure_shim_dir() -> Result<PathBuf, String> {
    let root = dirs::home_dir()
        .ok_or_else(|| "The home directory could not be found.".to_string())?
        .join(".suaegi")
        .join("claude-agent-teams-bin");
    fs::create_dir_all(&root)
        .map_err(|error| format!("Could not create the Claude team shim directory: {error}"))?;
    let shim = root.join("tmux");
    let content = "#!/usr/bin/env sh\nset -eu\nexec \"${ORCA_AGENT_TEAMS_SHIM_BIN:-suaegi}\" agent-teams-tmux \"$@\"\n";
    if fs::read_to_string(&shim).ok().as_deref() != Some(content) {
        let temporary = shim.with_extension(format!("{}.tmp", std::process::id()));
        fs::write(&temporary, content)
            .map_err(|error| format!("Could not write the Claude team shim: {error}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&temporary, fs::Permissions::from_mode(0o755))
                .map_err(|error| format!("Could not secure the Claude team shim: {error}"))?;
        }
        fs::rename(&temporary, &shim)
            .map_err(|error| format!("Could not publish the Claude team shim: {error}"))?;
    }
    Ok(root)
}

fn teams_dir() -> PathBuf {
    crate::persistence_thread::default_data_file()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("agent-teams")
}

fn team_path(team_id: &str) -> PathBuf {
    teams_dir().join(format!("{team_id}.json"))
}

fn required_env(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("Missing {name}"))
}

fn validate_component(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 256
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("Invalid agent team id.".into());
    }
    Ok(())
}

fn random_url_token(bytes: usize) -> Result<String, String> {
    let mut value = vec![0_u8; bytes];
    getrandom::fill(&mut value)
        .map_err(|error| format!("Could not create an agent team token: {error}"))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value))
}

fn sha256_hex(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn lock_owner_alive(lock: &Path) -> bool {
    let pid = fs::read_to_string(lock)
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok());
    pid.is_some_and(process_is_alive)
}

#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    // Signal zero only probes process existence/permission.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(not(unix))]
fn process_is_alive(_pid: u32) -> bool {
    true
}

fn has_teammate_mode(args: &[String]) -> bool {
    args.iter()
        .any(|argument| argument == "--teammate-mode" || argument.starts_with("--teammate-mode="))
}

fn constant_time_equal(left: &str, right: &str) -> bool {
    left.len() == right.len()
        && left
            .bytes()
            .zip(right.bytes())
            .fold(0_u8, |difference, (left, right)| {
                difference | (left ^ right)
            })
            == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_global_flags_and_clustered_tmux_flags() {
        assert_eq!(
            split_tmux_command(&["-L".into(), "orca".into(), "list-panes".into()]).unwrap(),
            ("list-panes".into(), Vec::new())
        );
        let parsed = parse_tmux_args(
            &["-Pdh".into(), "-F#{pane_id}".into(), "cat".into()],
            &["-F", "-t"],
            &["-P", "-d", "-h"],
        );
        assert!(parsed.flags.contains("-P"));
        assert!(parsed.flags.contains("-d"));
        assert!(parsed.flags.contains("-h"));
        assert_eq!(parsed.value("-F"), Some("#{pane_id}"));
        assert_eq!(parsed.positional, ["cat"]);
    }

    #[test]
    fn send_keys_matches_orca_special_key_contract() {
        assert_eq!(
            send_keys_text(&["hello".into(), "Enter".into(), "world".into()], false),
            "hello\rworld"
        );
        assert_eq!(
            send_keys_text(&["hello".into(), "world".into()], true),
            "hello world"
        );
    }

    #[test]
    fn format_context_has_stable_fake_tmux_identifiers() {
        let pane = TeamPane {
            fake_pane_id: "%2".into(),
            handle: "term_2".into(),
            index: 1,
            split_from_pane: None,
            split_direction: None,
        };
        let team = AgentTeam {
            team_id: "team-test".into(),
            token_hash: sha256_hex("secret"),
            leader_pane: "%1".into(),
            leader_handle: "term_1".into(),
            session_name: "suaegi".into(),
            window_index: "0".into(),
            tmux_value: "value".into(),
            panes: BTreeMap::new(),
            pane_order: Vec::new(),
            next_pane_number: 3,
            main_vertical: None,
            previously_focused_pane: None,
        };
        assert_eq!(
            render_format(
                "#{session_name}:#{window_index}.#{pane_index}:#{pane_id}",
                &team,
                &pane,
                ""
            ),
            "suaegi:0.1:%2"
        );
    }

    #[test]
    fn direct_mode_is_added_only_once() {
        assert!(!has_teammate_mode(&["--resume".into(), "id".into()]));
        assert!(has_teammate_mode(&["--teammate-mode=auto".into()]));
        assert!(constant_time_equal("token", "token"));
        assert!(!constant_time_equal("token", "other"));
    }
}
