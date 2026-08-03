//! Standalone `suaegi` command line entry point.
//!
//! The desktop installer links `~/.local/bin/suaegi` to the application
//! executable, so this module must run before Iced is initialized. Commands
//! which only inspect Git or persisted desktop state intentionally work while
//! the GUI is closed as well.

use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use suaegi_core::domain::{PersistedState, Repo, RuntimeEnvironmentSetting, Worktree, WorktreeId};
use suaegi_core::persistence::Store;

const HELP: &str = "\
Suaegi desktop automation CLI

Usage:
  suaegi status [--json]
  suaegi open
  suaegi serve [--port <port>] [--pairing-address <host>] [--no-pairing] [--project-root <path>] [--recipe-json] [--json]
  suaegi claude-teams [claude args...]
  suaegi project list [--json]
  suaegi project setups [--project <id>] [--host <host-id>] [--json]
  suaegi project setup-existing-folder --project <id> --host <host-id> --path <path>
  suaegi project setup-clone --project <id> --host <host-id> --url <url> --destination <path>
  suaegi project setup-create --project <id> --host <host-id> [metadata flags] [--json]
  suaegi project setup-update --setup <setup-id> [metadata flags] [--json]
  suaegi project setup-delete --setup <setup-id> [--json]
  suaegi repo list [--json]
  suaegi repo show <name|path> [--json]
  suaegi repo add <path> [--json]
  suaegi repo set-base-ref --repo <selector> --ref <ref> [--json]
  suaegi repo search-refs --repo <selector> --query <text> [--limit <n>] [--json]
  suaegi environment add --name <name> --pairing-code <code> [--json]
  suaegi environment list [--json]
  suaegi environment show --environment <selector> [--json]
  suaegi environment rm --environment <selector> [--json]
  suaegi agent hooks status|on|off [--json]
  suaegi linear issue [<id>] [--current] [--comments] [--children] [--depth <n>] [--attachments] [--relations] [--activity] [--full] [--json]
  suaegi linear search <query> [--limit <n>] [--json]
  suaegi linear list [--limit <n>] [--json]
  suaegi linear list-issues [filter options] [--limit <n>] [--json]
  suaegi linear team list [--json]
  suaegi linear team members --team <key|id> [--json]
  suaegi linear team states --team <key|id> [--json]
  suaegi linear team labels --team <key|id> [--json]
  suaegi linear project list [--query <text>] [--limit <n>] [--json]
  suaegi linear create --team <key|id> --title <title> [issue fields] [--json]
  suaegi linear save-issue [<id>] [--current] [issue fields] [--json]
  suaegi linear status set [<id>] [--current] --to <state> [--json]
  suaegi linear assignee set [<id>] [--current] [--me | --to-id <user>] [--json]
  suaegi linear assignee clear [<id>] [--current] [--json]
  suaegi linear priority set [<id>] [--current] --to <priority> [--json]
  suaegi linear priority clear [<id>] [--current] [--json]
  suaegi linear estimate set [<id>] [--current] --to <number> [--json]
  suaegi linear estimate clear [<id>] [--current] [--json]
  suaegi linear due-date set [<id>] [--current] --to <yyyy-mm-dd> [--json]
  suaegi linear due-date clear [<id>] [--current] [--json]
  suaegi linear label add [<id>] [--current] --label <label>... [--json]
  suaegi linear label remove [<id>] [--current] --label <label>... [--json]
  suaegi linear label set [<id>] [--current] --label <label>... [--json]
  suaegi linear comment add [<id>] (--body <text>|--body-file <path|->) [--json]
  suaegi linear attach [<id>] --url <url> [--title <title>] [--json]
  suaegi linear relation add|remove [<id>] --related <issue> --type <relationship>
  suaegi diagnostics memory [--json]
  suaegi computer capabilities [--json]
  suaegi computer list-apps [--json]
  suaegi computer permissions [--id <accessibility|screenshots>] [--json]
  suaegi computer list-windows --app <name|bundle|pid:N> [--json]
  suaegi computer get-app-state --app <app> [--window-index <n>] [--json]
  suaegi computer click --app <app> (--element-index <n>|--x <x> --y <y>) [--json]
  suaegi computer perform-secondary-action --app <app> --element-index <n> --action <name>
  suaegi computer scroll --app <app> --direction <direction> [target] [--json]
  suaegi computer drag --app <app> [element or coordinate targets] [--json]
  suaegi computer type-text --app <app> [--text <text>|--text-stdin] [--json]
  suaegi computer press-key --app <app> --key <key> [--json]
  suaegi computer hotkey --app <app> --key <combo> [--json]
  suaegi computer paste-text --app <app> [--text <text>|--text-stdin] [--json]
  suaegi computer set-value --app <app> --element-index <n> [--value <text>|--value-stdin]
  suaegi orchestration run-create --objective <text> [--from <handle>] [--json]
  suaegi orchestration run-use --id <run-id> [--from <handle>] [--json]
  suaegi orchestration run-current [--from <handle>] [--json]
  suaegi orchestration run-list [--json]
  suaegi orchestration run-show --id <run-id> [--json]
  suaegi orchestration send --subject <text> [message fields] [--json]
  suaegi orchestration check [--wait] [--timeout-ms <n>] [--json]
  suaegi orchestration reply --id <message-id> --body <text> [--json]
  suaegi orchestration inbox [--limit <n>] [--terminal <handle>] [--json]
  suaegi orchestration task-create --spec <text> [--deps <json>] [--json]
  suaegi orchestration task-list [--status <status>] [--ready] [--json]
  suaegi orchestration task-update --id <task-id> --status <status> [--json]
  suaegi orchestration dispatch --task <task-id> --to <terminal> [--inject] [--json]
  suaegi orchestration dispatch-show --task <task-id> [--preamble] [--json]
  suaegi orchestration ask [--question <text>|--resume <message-id>] [--json]
  suaegi orchestration coordinator-start [--json]
  suaegi orchestration coordinator-stop [--json]
  suaegi orchestration gate-create --task <task-id> --question <text> [--json]
  suaegi orchestration gate-resolve --id <gate-id> --resolution <text> [--json]
  suaegi orchestration gate-list [--task <task-id>] [--status <status>] [--json]
  suaegi orchestration reset [--all|--tasks|--messages] [--json]
  suaegi orchestration worker-start --task <task-id> [--worktree <current|selector|new-child|new-top-level>] (--agent <agent>|--terminal <handle>) [--name <name>] [--repo <selector>] [--base-branch <ref>] [--setup <run|skip|inherit>] [--timeout-ms <n>]
  suaegi orchestration worker-show --dispatch <dispatch-id> [--json]
  suaegi orchestration worker-read --dispatch <dispatch-id> [--json]
  suaegi orchestration worker-stop --dispatch <dispatch-id> [--json]
  suaegi orchestration worker-abandon --dispatch <dispatch-id> [--json]
  suaegi vm recipe doctor <recipe-id> [--repo-path <path>] [--provision|--connect] [--json]
  suaegi worktree list [--repo <name|path>] [--json]
  suaegi worktree current [--json]
  suaegi worktree show <name|path> [--json]
  suaegi worktree create --name <name> [--repo <name|path>] [--base-branch <ref>]
  suaegi worktree set --worktree <selector> [metadata flags] [--json]
  suaegi worktree ps [--limit <n>] [--json]
  suaegi worktree rm --worktree <name|path> [--force]
  suaegi emulator devices [--json]
  suaegi emulator list [--json]
  suaegi emulator attach [device] [--json]
  suaegi emulator tap <x> <y> [--device <id>] [--json]
  suaegi emulator type <text> [--device <id>] [--json]
  suaegi emulator gesture <points-json> [--device <id>] [--json]
  suaegi emulator button <name> [--device <id>] [--json]
  suaegi emulator rotate <orientation> [--device <id>] [--json]
  suaegi emulator exec --command <command> [--device <id>] [--json]
  suaegi emulator kill|shutdown [--device <id>] [--json]
  suaegi emulator install <apk> [--reinstall] [--device <id>] [--json]
  suaegi emulator launch <package> [--activity <name>] [--device <id>] [--json]
  suaegi emulator permissions <grant|revoke|reset> [package] [permission]
  suaegi emulator ax|logcat [--device <id>] [--json]
  suaegi terminal list [--json]
  suaegi terminal show [--terminal <handle>] [--json]
  suaegi terminal read [--terminal <handle>] [--json]
  suaegi terminal send [--terminal <handle>] [--text <text>] [--enter|--interrupt]
  suaegi terminal wait [--terminal <handle>] --for exit|tui-idle [--timeout-ms <ms>]
  suaegi terminal stop --worktree <selector> [--json]
  suaegi terminal create [--worktree <selector>] [--command <text>] [--json]
  suaegi terminal switch|focus [--terminal <handle>] [--json]
  suaegi terminal rename [--terminal <handle>] [--title <text>] [--json]
  suaegi terminal split [--terminal <handle>] [--direction horizontal|vertical]
  suaegi terminal close --terminal <handle> [--json]
  suaegi goto --url <url> [--json]
  suaegi back|forward|reload [--json]
  suaegi snapshot [--json]
  suaegi screenshot [--format <png|jpeg>] [--json]
  suaegi full-screenshot [--format <png|jpeg>] [--json]
  suaegi pdf [--json]
  suaegi click|focus|hover|clear|select-all --element <ref> [--json]
  suaegi check|uncheck --element <ref> [--json]
  suaegi dblclick|scrollintoview --element <ref> [--json]
  suaegi fill|select --element <ref> --value <text> [--json]
  suaegi type --input <text> [--json]
  suaegi inserttext --text <text> [--json]
  suaegi drag --from <ref> --to <ref> [--json]
  suaegi upload --element <ref> --files <path,...> [--json]
  suaegi download --selector <ref> --path <path> [--json]
  suaegi get --what <property> [--element <ref>] [--json]
  suaegi is --what <state> --element <ref> [--json]
  suaegi find --locator <kind> --value <value> --action <action> [--text <text>]
  suaegi highlight --selector <ref> [--json]
  suaegi scroll --direction <direction> [--amount <pixels>] [--json]
  suaegi keypress --key <key> [--json]
  suaegi eval --expression <javascript> [--json]
  suaegi dialog accept [--text <text>] [--json]
  suaegi dialog dismiss [--json]
  suaegi wait [--selector <css|ref>|--text <text>|--url <url>|--load <state>|--fn <js>]
  suaegi cookie get|set|delete [options] [--json]
  suaegi storage local|session get|set|clear [options] [--json]
  suaegi tab list|current|show|switch|create|close [options] [--json]
  suaegi tab profile list|create|delete|set|show|use-default|clone [options]
  suaegi mouse move|down|up|wheel [options] [--json]
  suaegi viewport --width <w> --height <h> [--scale <n>] [--mobile] [--json]
  suaegi geolocation --latitude <n> --longitude <n> [--accuracy <n>]
  suaegi intercept enable|disable|list [--patterns <glob,...>] [--json]
  suaegi set device|offline|headers|credentials|media [options] [--json]
  suaegi clipboard read|write [--text <text>] [--json]
  suaegi capture start|stop [--json]
  suaegi console|network [--limit <n>] [--json]
  suaegi exec --command <agent-browser-command> [--json]
  suaegi file open|diff <path> [--worktree <selector>] [--json]
  suaegi file open-changed [--mode edit|diff|both] [--worktree <selector>] [--json]
  suaegi automations list|show|create|edit|remove|run|runs [options] [--json]
  suaegi skills list [--json]
  suaegi skills get <topic> [--full] [--json]
  suaegi ui open <browser|emulator|automations|activity|tasks|board|settings>
  suaegi settings open [section] [--json]
  suaegi agent-context [--json]
  suaegi help

Run `suaegi help` to display this message.
";

pub fn should_handle(argv0: &OsStr, args: &[OsString]) -> bool {
    let invoked_as_cli = Path::new(argv0)
        .file_name()
        .is_some_and(|name| name == OsStr::new("suaegi"));
    invoked_as_cli
        || args.first().is_some_and(|arg| {
            matches!(
                arg.to_str(),
                Some(
                    "help"
                        | "--help"
                        | "-h"
                        | "status"
                        | "open"
                        | "serve"
                        | "claude-teams"
                        | "agent-teams-tmux"
                        | "project"
                        | "repo"
                        | "environment"
                        | "agent"
                        | "linear"
                        | "diagnostics"
                        | "computer"
                        | "orchestration"
                        | "vm"
                        | "worktree"
                        | "emulator"
                        | "terminal"
                        | "goto"
                        | "back"
                        | "forward"
                        | "reload"
                        | "snapshot"
                        | "screenshot"
                        | "full-screenshot"
                        | "pdf"
                        | "click"
                        | "fill"
                        | "type"
                        | "select"
                        | "scroll"
                        | "eval"
                        | "check"
                        | "uncheck"
                        | "focus"
                        | "clear"
                        | "select-all"
                        | "keypress"
                        | "hover"
                        | "dblclick"
                        | "scrollintoview"
                        | "inserttext"
                        | "drag"
                        | "upload"
                        | "download"
                        | "get"
                        | "is"
                        | "find"
                        | "highlight"
                        | "wait"
                        | "cookie"
                        | "storage"
                        | "tab"
                        | "mouse"
                        | "viewport"
                        | "geolocation"
                        | "intercept"
                        | "set"
                        | "clipboard"
                        | "capture"
                        | "console"
                        | "network"
                        | "dialog"
                        | "exec"
                        | "file"
                        | "automations"
                        | "skills"
                        | "ui"
                        | "settings"
                        | "agent-context"
                )
            )
        })
}

pub fn run(args: Vec<OsString>) -> Result<i32, String> {
    let args = utf8_args(args)?;
    if args
        .first()
        .is_some_and(|argument| argument == "claude-teams")
    {
        return crate::claude_agent_teams::run_claude(&args[1..]);
    }
    if args
        .first()
        .is_some_and(|argument| argument == "agent-teams-tmux")
    {
        return crate::claude_agent_teams::run_tmux(&args[1..]);
    }
    let json_output = args.iter().any(|arg| arg == "--json");
    let positional_owned = positional_args(&args);
    let positional = positional_owned
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();

    match positional.as_slice() {
        [] | ["help"] => {
            print!("{HELP}");
            Ok(0)
        }
        ["status"] => {
            if let Some(status) = crate::local_rpc::call("status", Value::Null)? {
                let plain = format!(
                    "Suaegi is running · {} repositories · {} worktrees · {}",
                    status["repositories"].as_u64().unwrap_or(0),
                    status["worktrees"].as_u64().unwrap_or(0),
                    status["surface"].as_str().unwrap_or("workbench"),
                );
                output(status, json_output, plain);
                return Ok(0);
            }
            let data_file = crate::persistence_thread::default_data_file();
            let state = load_state();
            output(
                json!({
                    "app": "Suaegi",
                    "running": desktop_is_running(),
                    "dataFile": data_file,
                    "repositories": state.repos.len(),
                    "worktrees": state.worktrees.len(),
                    "schemaVersion": state.schema_version,
                }),
                json_output,
                format!(
                    "Suaegi {} · {} repositories · {} saved worktrees",
                    if desktop_is_running() {
                        "is running"
                    } else {
                        "is not running"
                    },
                    state.repos.len(),
                    state.worktrees.len()
                ),
            );
            Ok(0)
        }
        ["serve"] => crate::runtime_server::run(&args, json_output),
        ["open"] => {
            open_desktop()?;
            output(json!({"opened": true}), json_output, "Opened Suaegi".into());
            Ok(0)
        }
        ["project", command] => project_command(command, &args, json_output),
        ["repo", "list"] => {
            if let Some(repositories) = crate::local_rpc::call("repo.list", Value::Null)? {
                let plain = repositories["repositories"]
                    .as_array()
                    .map(|rows| {
                        rows.iter()
                            .map(|repo| {
                                format!(
                                    "{}\t{}",
                                    repo["name"].as_str().unwrap_or("Repository"),
                                    repo["path"].as_str().unwrap_or_default()
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_else(|| "No repositories".into());
                output(repositories, json_output, plain);
                return Ok(0);
            }
            let state = load_state();
            let rows = state.repos.iter().map(repo_json).collect::<Vec<_>>();
            let plain = if state.repos.is_empty() {
                "No repositories".into()
            } else {
                state
                    .repos
                    .iter()
                    .map(|repo| format!("{}\t{}", repo.display_name, repo.path.display()))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            output(json!({"repositories": rows}), json_output, plain);
            Ok(0)
        }
        ["repo", "show", selector] => {
            let state = load_state();
            let repo = select_repo(&state, selector)?;
            output(repo_json(repo), json_output, pretty_repo(repo));
            Ok(0)
        }
        ["repo", "show"] => {
            let selector = option_value(&args, "--repo")?
                .ok_or_else(|| "repo show requires --repo <selector>".to_string())?;
            let state = load_state();
            let repo = select_repo(&state, selector.trim_start_matches("id:"))?;
            output(repo_json(repo), json_output, pretty_repo(repo));
            Ok(0)
        }
        ["repo", "add", path] => add_repo(path, json_output),
        ["repo", "add"] => {
            let path = option_value(&args, "--path")?
                .ok_or_else(|| "repo add requires --path <path>".to_string())?;
            add_repo(&path, json_output)
        }
        ["repo", "set-base-ref"] => set_repo_base_ref(&args, json_output),
        ["repo", "search-refs"] => search_repo_refs(&args, json_output),
        ["environment", command] => environment_command(command, &args, json_output),
        ["agent", "hooks", command] => agent_hooks_command(command, json_output),
        ["linear", "issue", rest @ ..] => linear_issue_command(rest, &args, json_output),
        ["linear", "search", rest @ ..] => linear_search_command(rest, &args, json_output),
        ["linear", "list"] => linear_list_command(&args, json_output),
        ["linear", "list-issues"] => linear_list_issues_command(&args, json_output),
        ["linear", "team", command] => linear_team_command(command, &args, json_output),
        ["linear", "project", "list"] => linear_project_list_command(&args, json_output),
        ["linear", "create"] => linear_create_command(&args, json_output),
        ["linear", "save-issue", rest @ ..] => linear_save_issue_command(rest, &args, json_output),
        ["linear", group, command, rest @ ..]
            if matches!(
                (*group, *command),
                ("status", "set")
                    | ("assignee", "set" | "clear")
                    | ("priority", "set" | "clear")
                    | ("estimate", "set" | "clear")
                    | ("due-date", "set" | "clear")
                    | ("label", "add" | "remove" | "set")
            ) =>
        {
            linear_update_command(group, command, rest, &args, json_output)
        }
        ["linear", "comment", "add", rest @ ..] => {
            linear_comment_add_command(rest, &args, json_output)
        }
        ["linear", "attach", rest @ ..] => linear_attach_command(rest, &args, json_output),
        ["linear", "relation", command, rest @ ..] => {
            linear_relation_command(command, rest, &args, json_output)
        }
        ["diagnostics", "memory"] => diagnostics_memory(json_output),
        ["computer", command] => {
            let result = crate::computer::run(command, &args)?;
            let plain = format_computer_result(command, &result);
            output(result, json_output, plain);
            Ok(0)
        }
        ["orchestration", command] => {
            let result = crate::orchestration::run(command, &args)?;
            output(
                result,
                json_output,
                format!("Orchestration {command} completed."),
            );
            Ok(0)
        }
        ["vm", "recipe", "doctor", recipe_id] => vm_recipe_doctor(recipe_id, &args, json_output),
        ["vm", "recipe", "doctor"] => {
            let recipe_id = option_value(&args, "--recipe-id")?
                .ok_or_else(|| "vm recipe doctor requires a recipe id".to_string())?;
            vm_recipe_doctor(&recipe_id, &args, json_output)
        }
        ["worktree", "list"] => list_worktrees(&args, json_output),
        ["worktree", "current"] => current_worktree(json_output),
        ["worktree", "show", selector] => show_worktree(selector, json_output),
        ["worktree", "show"] => {
            let selector = option_value(&args, "--worktree")?
                .ok_or_else(|| "worktree show requires --worktree <selector>".to_string())?;
            show_worktree_live(&selector, json_output)
        }
        ["worktree", "create"] => create_worktree(&args, json_output),
        ["worktree", "set"] => set_worktree(&args, json_output),
        ["worktree", "ps"] => worktree_ps(&args, json_output),
        ["worktree", "rm" | "remove" | "delete"] => remove_worktree(&args, json_output),
        ["emulator", command, rest @ ..] => emulator_command(command, rest, &args, json_output),
        ["terminal", command] => terminal_command(command, &args, json_output),
        ["goto"] => browser_goto(&args, json_output),
        ["back" | "forward" | "reload"] => {
            let action = positional[0];
            let result = local_rpc_required(&format!("browser.{action}"), Value::Null)?;
            output(result, json_output, format!("Browser {action}"));
            Ok(0)
        }
        ["snapshot"] => browser_interact_command("snapshot", &args, json_output),
        ["screenshot"] => browser_capture("screenshot", &args, json_output),
        ["full-screenshot"] => browser_capture("full-screenshot", &args, json_output),
        ["pdf"] => browser_capture("pdf", &args, json_output),
        [command @ ("click" | "fill" | "type" | "select" | "scroll" | "eval" | "check"
        | "uncheck" | "focus" | "clear" | "select-all" | "keypress" | "hover"
        | "dblclick" | "scrollintoview" | "inserttext" | "drag" | "get" | "is"
        | "find" | "highlight")] => browser_interact_command(command, &args, json_output),
        ["upload"] => browser_upload(&args, json_output),
        ["download"] => browser_download(&args, json_output),
        ["dialog", command @ ("accept" | "dismiss")] => {
            browser_dialog_command(command, &args, json_output)
        }
        ["wait"] => browser_wait(&args, json_output),
        ["cookie", command] => browser_cookie_command(command, &args, json_output),
        ["storage", kind, command] => browser_storage_command(kind, command, &args, json_output),
        ["tab", command] => browser_tab_command(command, &args, json_output),
        ["tab", "profile", command] => browser_profile_command(command, &args, json_output),
        ["mouse", command] => {
            browser_advanced_command(&format!("mouse-{command}"), &args, json_output)
        }
        ["viewport"] => browser_advanced_command("viewport", &args, json_output),
        ["geolocation"] => browser_advanced_command("geolocation", &args, json_output),
        ["intercept", command @ ("enable" | "disable" | "list")] => {
            browser_advanced_command(&format!("intercept-{command}"), &args, json_output)
        }
        ["set", command @ ("device" | "offline" | "headers" | "credentials" | "media" | "preferences")] =>
        {
            let command = if *command == "preferences" {
                "media"
            } else {
                command
            };
            browser_advanced_command(&format!("set-{command}"), &args, json_output)
        }
        ["clipboard", command @ ("read" | "write")] => {
            browser_advanced_command(&format!("clipboard-{command}"), &args, json_output)
        }
        ["capture", command @ ("start" | "stop")] => {
            browser_advanced_command(&format!("capture-{command}"), &args, json_output)
        }
        [command @ ("console" | "network")] => {
            browser_advanced_command(command, &args, json_output)
        }
        ["exec"] => browser_exec(&args, json_output),
        ["file", command @ ("open" | "diff"), rest @ ..] => {
            file_command(command, rest, &args, json_output)
        }
        ["file", "open-changed"] => file_open_changed(&args, json_output),
        ["automations", command, rest @ ..] => {
            automations_command(command, rest, &args, json_output)
        }
        ["skills", "list"] => skills_list(json_output),
        ["skills", "get" | "show", topic] => skills_get(topic, &args, json_output),
        ["skills", "get" | "show"] => {
            let topic = option_value(&args, "--topic")?
                .ok_or_else(|| "skills get requires a topic".to_string())?;
            skills_get(&topic, &args, json_output)
        }
        ["ui", "open", destination] => {
            let result = local_rpc_required("navigate", json!({"destination": destination}))?;
            output(result, json_output, format!("Opened {destination}"));
            Ok(0)
        }
        ["settings", "open"] => open_settings("general", json_output),
        ["settings", "open", section] => open_settings(section, json_output),
        ["agent-context"] => {
            let schema = agent_context_schema();
            if json_output {
                output(schema, true, String::new());
            } else {
                println!(
                    "{} commands (schema v1).\nRun `suaegi agent-context --json` for the full machine-readable command schema.",
                    schema["commandCount"].as_u64().unwrap_or(0)
                );
            }
            Ok(0)
        }
        _ => Err(format!(
            "Unknown or incomplete command: {}\n\n{HELP}",
            args.join(" ")
        )),
    }
}

fn positional_args(args: &[String]) -> Vec<String> {
    const VALUE_FLAGS: &[&str] = &[
        "--repo",
        "--project",
        "--host",
        "--setup",
        "--setup-id",
        "--destination",
        "--kind",
        "--method",
        "--worktree-base-path",
        "--git-username",
        "--worktree",
        "--device",
        "--emulator",
        "--activity",
        "--command",
        "--lines",
        "--name",
        "--base-branch",
        "--agent",
        "--prompt",
        "--terminal",
        "--text",
        "--for",
        "--timeout-ms",
        "--title",
        "--limit",
        "--depth",
        "--cursor",
        "--url",
        "--path",
        "--ref",
        "--query",
        "--display-name",
        "--comment",
        "--workspace-status",
        "--parent-worktree",
        "--issue",
        "--linear-issue",
        "--id",
        "--provider",
        "--trigger",
        "--schedule",
        "--time",
        "--day",
        "--timezone",
        "--workspace",
        "--team",
        "--filter",
        "--cycle",
        "--order-by",
        "--release",
        "--delegate",
        "--created-at",
        "--updated-at",
        "--body",
        "--description",
        "--body-file",
        "--state",
        "--assignee",
        "--priority",
        "--estimate",
        "--due-date",
        "--project",
        "--parent",
        "--parent-id",
        "--write-id",
        "--reply-to",
        "--to-id",
        "--related",
        "--type",
        "--element",
        "--value",
        "--input",
        "--direction",
        "--amount",
        "--expression",
        "--key",
        "--from",
        "--to",
        "--what",
        "--selector",
        "--locator",
        "--action",
        "--timeout",
        "--load",
        "--fn",
        "--domain",
        "--sameSite",
        "--expires",
        "--name",
        "--state",
        "--format",
        "--page",
        "--index",
        "--profile",
        "--label",
        "--scope",
        "--x",
        "--y",
        "--dx",
        "--dy",
        "--button",
        "--width",
        "--height",
        "--scale",
        "--patterns",
        "--files",
        "--latitude",
        "--longitude",
        "--accuracy",
        "--headers",
        "--user",
        "--pass",
        "--color-scheme",
        "--reduced-motion",
        "--environment",
        "--pairing-code",
        "--pairing-address",
        "--port",
        "--mode",
        "--topic",
        "--recipe-id",
        "--repo-path",
        "--app",
        "--window-id",
        "--window-index",
        "--element-index",
        "--from-element-index",
        "--to-element-index",
        "--from-x",
        "--from-y",
        "--to-x",
        "--to-y",
        "--click-count",
        "--mouse-button",
        "--pages",
        "--objective",
        "--run",
        "--subject",
        "--body",
        "--priority",
        "--thread-id",
        "--payload",
        "--task-id",
        "--dispatch-id",
        "--outcome",
        "--files-modified",
        "--report-path",
        "--phase",
        "--ack",
        "--types",
        "--spec",
        "--task-title",
        "--deps",
        "--parent",
        "--result",
        "--task",
        "--retry-of",
        "--question",
        "--resume",
        "--options",
        "--resolution",
        "--dispatch",
        "--source",
        "--on",
        "--status",
    ];
    let mut values = Vec::new();
    let mut index = 0;
    while index < args.len() {
        if args[index].starts_with("--") {
            index += if VALUE_FLAGS.contains(&args[index].as_str()) {
                2
            } else {
                1
            };
        } else {
            values.push(args[index].clone());
            index += 1;
        }
    }
    values
}

fn utf8_args(args: Vec<OsString>) -> Result<Vec<String>, String> {
    args.into_iter()
        .map(|arg| {
            arg.into_string()
                .map_err(|_| "CLI arguments must be valid UTF-8".to_string())
        })
        .collect()
}

fn data_store() -> Store {
    Store::new(crate::persistence_thread::default_data_file())
}

fn load_state() -> PersistedState {
    data_store().load().state
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("Could not start the command runtime: {error}"))
}

fn select_repo<'a>(state: &'a PersistedState, selector: &str) -> Result<&'a Repo, String> {
    let canonical = Path::new(selector).canonicalize().ok();
    state
        .repos
        .iter()
        .find(|repo| {
            repo.id.0 == selector
                || repo.display_name == selector
                || repo.path == Path::new(selector)
                || canonical.as_ref().is_some_and(|path| path == &repo.path)
        })
        .ok_or_else(|| format!("Repository not found: {selector}"))
}

fn repo_json(repo: &Repo) -> Value {
    json!({
        "id": repo.id.0,
        "name": repo.display_name,
        "path": repo.path,
        "baseRef": repo.worktree_base_ref,
    })
}

fn pretty_repo(repo: &Repo) -> String {
    format!(
        "{}\n  path: {}\n  base ref: {}",
        repo.display_name,
        repo.path.display(),
        repo.worktree_base_ref.as_deref().unwrap_or("auto")
    )
}

fn environment_json(environment: &RuntimeEnvironmentSetting) -> Value {
    json!({
        "id": environment.id,
        "name": environment.name,
        "endpoint": environment.endpoint,
        "credentialsConfigured": environment.credentials_configured,
        "createdAtUnixMs": environment.created_at_unix_ms,
    })
}

fn select_environment<'a>(
    state: &'a PersistedState,
    selector: &str,
) -> Result<&'a RuntimeEnvironmentSetting, String> {
    state
        .settings
        .ui
        .runtime_environments
        .iter()
        .find(|environment| {
            environment.id == selector || environment.name.eq_ignore_ascii_case(selector)
        })
        .ok_or_else(|| format!("Environment not found: {selector}"))
}

fn pretty_environment(environment: &RuntimeEnvironmentSetting) -> String {
    format!(
        "{} ({})\n  endpoint: {}\n  credentials: {}",
        environment.name,
        environment.id,
        environment.endpoint,
        if environment.credentials_configured {
            "configured"
        } else {
            "missing"
        }
    )
}

fn linear_client() -> Result<suaegi_tracker::LinearClient, String> {
    let resolved = suaegi_secrets::load(&crate::tracker_tasks::secret_request());
    let token = resolved.secret.ok_or_else(|| {
        "Linear is not connected. Add a Linear API key in Suaegi or set LINEAR_API_KEY.".to_string()
    })?;
    Ok(suaegi_tracker::LinearClient::with_transport(
        std::sync::Arc::new(suaegi_http::ReqwestTransport::new()),
        Some(token),
    ))
}

fn linear_issue_command(rest: &[&str], args: &[String], json_output: bool) -> Result<i32, String> {
    let issue = option_value(args, "--id")?
        .or_else(|| rest.first().map(|value| (*value).to_string()))
        .or_else(|| {
            args.iter()
                .any(|argument| argument == "--current")
                .then(linear_current_issue)
                .flatten()
        })
        .ok_or_else(|| "linear issue requires an id or --current.".to_string())?;
    let issue = normalize_linear_issue_input(&issue);
    let client = linear_client()?;
    let full = args.iter().any(|argument| argument == "--full");
    let children = full || args.iter().any(|argument| argument == "--children");
    if args.iter().any(|argument| argument == "--depth") && !children {
        return Err("--depth requires --children or --full.".into());
    }
    let depth = option_value(args, "--depth")?
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| "--depth must be an integer from 0 to 5.".to_string())
        })
        .transpose()?
        .unwrap_or(2);
    if depth > 5 {
        return Err("--depth must be at most 5.".into());
    }
    let options = suaegi_tracker::LinearIssueContextOptions {
        comments: full || args.iter().any(|argument| argument == "--comments"),
        children,
        attachments: full || args.iter().any(|argument| argument == "--attachments"),
        relations: full || args.iter().any(|argument| argument == "--relations"),
        activity: full || args.iter().any(|argument| argument == "--activity"),
        depth,
    };
    let value = match runtime()?.block_on(client.get_issue_context(&issue, options)) {
        suaegi_tracker::Lookup::Found(value) => value,
        suaegi_tracker::Lookup::NotFound => {
            return Err("Linear issue was not found.".into());
        }
        suaegi_tracker::Lookup::Unavailable(error) => {
            return Err(format_linear_unavailable(&error));
        }
    };
    let issue_value = &value["issue"];
    let text = format!(
        "{}  {}  {}  {}",
        issue_value["identifier"].as_str().unwrap_or("Unknown"),
        issue_value["state"]["name"].as_str().unwrap_or("Unknown"),
        issue_value["assignee"]["displayName"]
            .as_str()
            .unwrap_or("Unassigned"),
        issue_value["title"].as_str().unwrap_or_default(),
    );
    output(value, json_output, text);
    Ok(0)
}

fn linear_search_command(rest: &[&str], args: &[String], json_output: bool) -> Result<i32, String> {
    let query = option_value(args, "--query")?
        .or_else(|| (!rest.is_empty()).then(|| rest.join(" ")))
        .filter(|query| !query.trim().is_empty())
        .ok_or_else(|| "linear search requires a query.".to_string())?;
    let limit = positive_limit(args, 50, 250)?;
    let client = linear_client()?;
    let issues = match runtime()?.block_on(client.search_issues(&query)) {
        suaegi_tracker::Lookup::Found(issues) => issues,
        suaegi_tracker::Lookup::NotFound => Vec::new(),
        suaegi_tracker::Lookup::Unavailable(error) => {
            return Err(format_linear_unavailable(&error));
        }
    };
    linear_issue_list_output(issues, limit, json_output)
}

fn linear_list_command(args: &[String], json_output: bool) -> Result<i32, String> {
    let limit = positive_limit(args, 50, 250)?;
    let filter = option_value(args, "--filter")?.unwrap_or_else(|| "assigned".into());
    if !matches!(
        filter.as_str(),
        "assigned" | "created" | "all" | "completed" | "open"
    ) {
        return Err("--filter must be assigned, created, all, completed, or open.".into());
    }
    let client = linear_client()?;
    let viewer_id = if matches!(filter.as_str(), "assigned" | "created") {
        match runtime()?.block_on(client.viewer_id()) {
            suaegi_tracker::Lookup::Found(id) => Some(id),
            suaegi_tracker::Lookup::NotFound => None,
            suaegi_tracker::Lookup::Unavailable(error) => {
                return Err(format_linear_unavailable(&error));
            }
        }
    } else {
        None
    };
    let team_id = option_value(args, "--team")?
        .map(|team| linear_resolve_team(&client, &team).map(|team| team.id))
        .transpose()?;
    let page = match runtime()?.block_on(client.list_issues(None)) {
        suaegi_tracker::Lookup::Found(page) => page,
        suaegi_tracker::Lookup::NotFound => suaegi_tracker::IssuePage {
            issues: Vec::new(),
            has_more: false,
        },
        suaegi_tracker::Lookup::Unavailable(error) => {
            return Err(format_linear_unavailable(&error));
        }
    };
    let issues = page
        .issues
        .into_iter()
        .filter(|issue| {
            team_id
                .as_deref()
                .is_none_or(|team| issue.team_id.as_deref() == Some(team))
        })
        .filter(|issue| match filter.as_str() {
            "assigned" => issue.assignee_id.as_deref() == viewer_id.as_deref(),
            "created" => issue.creator_id.as_deref() == viewer_id.as_deref(),
            "completed" => issue.state_type.as_deref() == Some("completed"),
            "open" => !matches!(issue.state_type.as_deref(), Some("completed" | "canceled")),
            _ => true,
        })
        .collect::<Vec<_>>();
    let has_more = page.has_more || issues.len() > limit;
    let issues = issues.into_iter().take(limit).collect::<Vec<_>>();
    let values = issues
        .iter()
        .map(|issue| linear_issue_json(issue, None))
        .collect::<Vec<_>>();
    let plain = format_linear_issue_rows(&issues);
    output(
        json!({"issues": values, "hasMore": has_more}),
        json_output,
        plain,
    );
    Ok(0)
}

fn linear_list_issues_command(args: &[String], json_output: bool) -> Result<i32, String> {
    for flag in [
        "--cycle",
        "--cursor",
        "--order-by",
        "--release",
        "--delegate",
        "--created-at",
        "--updated-at",
        "--include-archived",
    ] {
        if args.iter().any(|argument| argument == flag) {
            return Err(format!(
                "linear list-issues {flag} is not available with the current single-workspace provider."
            ));
        }
    }
    let limit = positive_limit(args, 50, 250)?;
    let client = linear_client()?;
    let query = option_value(args, "--query")?;
    let (issues, provider_has_more) = if let Some(query) = query {
        match runtime()?.block_on(client.search_issues(&query)) {
            suaegi_tracker::Lookup::Found(issues) => (issues, false),
            suaegi_tracker::Lookup::NotFound => (Vec::new(), false),
            suaegi_tracker::Lookup::Unavailable(error) => {
                return Err(format_linear_unavailable(&error));
            }
        }
    } else {
        match runtime()?.block_on(client.list_issues(None)) {
            suaegi_tracker::Lookup::Found(page) => (page.issues, page.has_more),
            suaegi_tracker::Lookup::NotFound => (Vec::new(), false),
            suaegi_tracker::Lookup::Unavailable(error) => {
                return Err(format_linear_unavailable(&error));
            }
        }
    };
    let team_id = option_value(args, "--team")?
        .map(|team| linear_resolve_team(&client, &team).map(|team| team.id))
        .transpose()?;
    let project_id = option_value(args, "--project")?
        .map(|project| linear_resolve_project_id(&client, &project))
        .transpose()?;
    let parent_id = option_value(args, "--parent-id")?
        .map(|parent| {
            if parent == "null" {
                Ok(None)
            } else {
                linear_get_issue(&client, &parent).map(|issue| Some(issue.id))
            }
        })
        .transpose()?;
    let priority = option_value(args, "--priority")?
        .map(|value| linear_priority(&value))
        .transpose()?;
    let state = option_value(args, "--state")?;
    let labels = option_values(args, "--label")?;
    let assignee = option_value(args, "--assignee")?;
    let viewer_id = if assignee.as_deref() == Some("me") {
        match runtime()?.block_on(client.viewer_id()) {
            suaegi_tracker::Lookup::Found(id) => Some(id),
            suaegi_tracker::Lookup::NotFound => None,
            suaegi_tracker::Lookup::Unavailable(error) => {
                return Err(format_linear_unavailable(&error));
            }
        }
    } else {
        None
    };
    let filtered = issues
        .into_iter()
        .filter(|issue| {
            team_id
                .as_deref()
                .is_none_or(|team| issue.team_id.as_deref() == Some(team))
        })
        .filter(|issue| {
            project_id
                .as_deref()
                .is_none_or(|project| issue.project_id.as_deref() == Some(project))
        })
        .filter(|issue| {
            parent_id
                .as_ref()
                .is_none_or(|parent| issue.parent_id.as_deref() == parent.as_deref())
        })
        .filter(|issue| priority.is_none_or(|priority| issue.priority == Some(priority)))
        .filter(|issue| {
            state.as_deref().is_none_or(|state| {
                issue.state_id.as_deref() == Some(state)
                    || issue
                        .state
                        .as_deref()
                        .is_some_and(|name| name.eq_ignore_ascii_case(state))
                    || issue.state_type.as_deref() == Some(state)
            })
        })
        .filter(|issue| {
            labels.iter().all(|label| {
                issue.label_ids.iter().any(|id| id == label)
                    || issue
                        .label_names
                        .iter()
                        .any(|name| name.eq_ignore_ascii_case(label))
            })
        })
        .filter(|issue| match assignee.as_deref() {
            None => true,
            Some("null") => issue.assignee_id.is_none(),
            Some("me") => issue.assignee_id.as_deref() == viewer_id.as_deref(),
            Some(value) => {
                issue.assignee_id.as_deref() == Some(value)
                    || issue
                        .assignee
                        .as_deref()
                        .is_some_and(|name| name.eq_ignore_ascii_case(value))
            }
        })
        .collect::<Vec<_>>();
    let has_more = provider_has_more || filtered.len() > limit;
    linear_issue_list_output_with_more(filtered, limit, has_more, json_output)
}

fn linear_team_command(command: &str, args: &[String], json_output: bool) -> Result<i32, String> {
    let client = linear_client()?;
    match command {
        "list" => {
            let teams = match runtime()?.block_on(client.list_teams()) {
                suaegi_tracker::Lookup::Found(teams) => teams,
                suaegi_tracker::Lookup::NotFound => Vec::new(),
                suaegi_tracker::Lookup::Unavailable(error) => {
                    return Err(format_linear_unavailable(&error));
                }
            };
            let rows = teams
                .iter()
                .map(|team| json!({"id": team.id, "key": team.key, "name": team.name}))
                .collect::<Vec<_>>();
            let plain = teams
                .iter()
                .map(|team| format!("{}\t{}\t{}", team.key, team.name, team.id))
                .collect::<Vec<_>>()
                .join("\n");
            output(json!({"teams": rows}), json_output, plain);
        }
        "members" => {
            let team = required_option(args, "--team", "linear team members")?;
            let team = linear_resolve_team(&client, &team)?;
            let members = match runtime()?.block_on(client.list_team_members(&team.id)) {
                suaegi_tracker::Lookup::Found(members) => members,
                suaegi_tracker::Lookup::NotFound => Vec::new(),
                suaegi_tracker::Lookup::Unavailable(error) => {
                    return Err(format_linear_unavailable(&error));
                }
            };
            let rows = members
                .iter()
                .map(|member| json!({"id": member.id, "name": member.name, "email": member.email}))
                .collect::<Vec<_>>();
            let plain = members
                .iter()
                .map(|member| {
                    format!(
                        "{}\t{}\t{}",
                        member.name,
                        member.email.as_deref().unwrap_or(""),
                        member.id
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            output(
                json!({"team": {"id": team.id, "key": team.key, "name": team.name}, "members": rows}),
                json_output,
                plain,
            );
        }
        "states" => {
            let team = required_option(args, "--team", "linear team states")?;
            let team = linear_resolve_team(&client, &team)?;
            let states = linear_team_states(&client, &team.id)?;
            let rows = states
                .iter()
                .map(|state| json!({"id": state.id, "name": state.name, "type": state.state_type}))
                .collect::<Vec<_>>();
            let plain = states
                .iter()
                .map(|state| {
                    format!(
                        "{}\t{}\t{}",
                        state.name,
                        state.state_type.as_deref().unwrap_or(""),
                        state.id
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            output(
                json!({"team": {"id": team.id, "key": team.key, "name": team.name}, "states": rows}),
                json_output,
                plain,
            );
        }
        "labels" => {
            let team = required_option(args, "--team", "linear team labels")?;
            let team = linear_resolve_team(&client, &team)?;
            let labels = linear_team_labels(&client, &team.id)?;
            let rows = labels
                .iter()
                .map(|label| json!({"id": label.id, "name": label.name, "color": label.color}))
                .collect::<Vec<_>>();
            let plain = labels
                .iter()
                .map(|label| format!("{}\t{}", label.name, label.id))
                .collect::<Vec<_>>()
                .join("\n");
            output(
                json!({"team": {"id": team.id, "key": team.key, "name": team.name}, "labels": rows}),
                json_output,
                plain,
            );
        }
        _ => return Err(format!("Unknown Linear team command: {command}")),
    }
    Ok(0)
}

fn linear_project_list_command(args: &[String], json_output: bool) -> Result<i32, String> {
    let limit = positive_limit(args, 50, 250)?;
    let query = option_value(args, "--query")?
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    let client = linear_client()?;
    let projects = match runtime()?.block_on(client.list_projects()) {
        suaegi_tracker::Lookup::Found(projects) => projects,
        suaegi_tracker::Lookup::NotFound => Vec::new(),
        suaegi_tracker::Lookup::Unavailable(error) => {
            return Err(format_linear_unavailable(&error));
        }
    };
    let matching = projects
        .into_iter()
        .filter(|project| query.is_empty() || project.name.to_lowercase().contains(&query))
        .collect::<Vec<_>>();
    let has_more = matching.len() > limit;
    let matching = matching.into_iter().take(limit).collect::<Vec<_>>();
    let rows = matching
        .iter()
        .map(|project| {
            json!({
                "id": project.id,
                "name": project.name,
                "state": project.state,
                "url": project.url,
            })
        })
        .collect::<Vec<_>>();
    let plain = matching
        .iter()
        .map(|project| format!("{}\t{}", project.name, project.id))
        .collect::<Vec<_>>()
        .join("\n");
    output(
        json!({"projects": rows, "hasMore": has_more}),
        json_output,
        plain,
    );
    Ok(0)
}

fn linear_resolve_team(
    client: &suaegi_tracker::LinearClient,
    selector: &str,
) -> Result<suaegi_tracker::LinearTeam, String> {
    let teams = match runtime()?.block_on(client.list_teams()) {
        suaegi_tracker::Lookup::Found(teams) => teams,
        suaegi_tracker::Lookup::NotFound => Vec::new(),
        suaegi_tracker::Lookup::Unavailable(error) => {
            return Err(format_linear_unavailable(&error));
        }
    };
    resolve_named_linear_record(
        teams,
        selector,
        |team| &team.id,
        |team| [&team.key, &team.name],
        "team",
    )
}

fn linear_team_states(
    client: &suaegi_tracker::LinearClient,
    team_id: &str,
) -> Result<Vec<suaegi_tracker::LinearState>, String> {
    match runtime()?.block_on(client.list_team_states(team_id)) {
        suaegi_tracker::Lookup::Found(states) => Ok(states),
        suaegi_tracker::Lookup::NotFound => Ok(Vec::new()),
        suaegi_tracker::Lookup::Unavailable(error) => Err(format_linear_unavailable(&error)),
    }
}

fn linear_team_labels(
    client: &suaegi_tracker::LinearClient,
    team_id: &str,
) -> Result<Vec<suaegi_tracker::LinearLabel>, String> {
    match runtime()?.block_on(client.list_team_labels(team_id)) {
        suaegi_tracker::Lookup::Found(labels) => Ok(labels),
        suaegi_tracker::Lookup::NotFound => Ok(Vec::new()),
        suaegi_tracker::Lookup::Unavailable(error) => Err(format_linear_unavailable(&error)),
    }
}

fn resolve_named_linear_record<T, I, N>(
    values: Vec<T>,
    selector: &str,
    id: I,
    names: N,
    kind: &str,
) -> Result<T, String>
where
    I: Fn(&T) -> &str,
    N: Fn(&T) -> [&str; 2],
{
    let mut exact = values
        .into_iter()
        .filter(|value| {
            id(value) == selector
                || names(value)
                    .into_iter()
                    .any(|name| name.eq_ignore_ascii_case(selector))
        })
        .collect::<Vec<_>>();
    match exact.len() {
        1 => Ok(exact.remove(0)),
        0 => Err(format!("Linear {kind} was not found: {selector}")),
        _ => Err(format!("Linear {kind} is ambiguous: {selector}")),
    }
}

fn linear_create_command(args: &[String], json_output: bool) -> Result<i32, String> {
    let team_selector = required_option(args, "--team", "linear create")?;
    let title = required_option(args, "--title", "linear create")?;
    if title.trim().is_empty() {
        return Err("--title cannot be empty.".into());
    }
    let write_id = linear_write_id(args)?;
    let client = linear_client()?;
    let team = linear_resolve_team(&client, &team_selector)?;
    let mut fields = linear_issue_fields(&client, &team.id, args, true)?;
    fields.insert("teamId".into(), Value::String(team.id));
    fields.insert("title".into(), Value::String(title));
    let outcome = runtime()?.block_on(client.create_issue_fields(&write_id, Value::Object(fields)));
    linear_issue_write_output(outcome, json_output)
}

fn linear_save_issue_command(
    rest: &[&str],
    args: &[String],
    json_output: bool,
) -> Result<i32, String> {
    let requested = linear_optional_issue_selector(rest, args)?;
    if requested.is_none() {
        return linear_create_command(args, json_output);
    }
    let client = linear_client()?;
    let issue = linear_get_issue(&client, &requested.unwrap())?;
    let team_id = issue
        .team_id
        .as_deref()
        .ok_or_else(|| "Linear issue did not include a team id.".to_string())?;
    let fields = linear_issue_fields(&client, team_id, args, false)?;
    if fields.is_empty() {
        return Err("linear save-issue requires at least one issue field to update.".into());
    }
    let outcome = runtime()?.block_on(client.update_issue_fields(&issue.id, Value::Object(fields)));
    linear_issue_write_output(outcome, json_output)
}

fn linear_update_command(
    group: &str,
    command: &str,
    rest: &[&str],
    args: &[String],
    json_output: bool,
) -> Result<i32, String> {
    validate_linear_update_args(group, command, args)?;
    let client = linear_client()?;
    let selector =
        linear_required_issue_selector(rest, args, &format!("linear {group} {command}"))?;
    let issue = linear_get_issue(&client, &selector)?;
    let mut fields = serde_json::Map::new();
    match (group, command) {
        ("status", "set") => {
            let value = required_option(args, "--to", "linear status set")?;
            let team_id = issue
                .team_id
                .as_deref()
                .ok_or_else(|| "Linear issue did not include a team id.".to_string())?;
            fields.insert(
                "stateId".into(),
                Value::String(linear_resolve_state_id(&client, team_id, &value)?),
            );
        }
        ("assignee", "set") => {
            let me = args.iter().any(|arg| arg == "--me");
            let to_id = option_value(args, "--to-id")?;
            if me == to_id.is_some() {
                return Err("Use exactly one of --me or --to-id for linear assignee set.".into());
            }
            let assignee = if me {
                match runtime()?.block_on(client.viewer_id()) {
                    suaegi_tracker::Lookup::Found(id) => id,
                    suaegi_tracker::Lookup::NotFound => {
                        return Err("Linear viewer was not found.".into());
                    }
                    suaegi_tracker::Lookup::Unavailable(error) => {
                        return Err(format_linear_unavailable(&error));
                    }
                }
            } else {
                to_id.unwrap()
            };
            fields.insert("assigneeId".into(), Value::String(assignee));
        }
        ("assignee", "clear") => {
            fields.insert("assigneeId".into(), Value::Null);
        }
        ("priority", "set") => {
            let value = required_option(args, "--to", "linear priority set")?;
            fields.insert(
                "priority".into(),
                Value::Number(linear_priority(&value)?.into()),
            );
        }
        ("priority", "clear") => {
            fields.insert("priority".into(), Value::Number(0.into()));
        }
        ("estimate", "set") => {
            let value = required_option(args, "--to", "linear estimate set")?;
            fields.insert("estimate".into(), linear_number(&value, "--to")?);
        }
        ("estimate", "clear") => {
            fields.insert("estimate".into(), Value::Null);
        }
        ("due-date", "set") => {
            let value = required_option(args, "--to", "linear due-date set")?;
            validate_linear_due_date(&value)?;
            fields.insert("dueDate".into(), Value::String(value));
        }
        ("due-date", "clear") => {
            fields.insert("dueDate".into(), Value::Null);
        }
        ("label", "add" | "remove" | "set") => {
            let team_id = issue
                .team_id
                .as_deref()
                .ok_or_else(|| "Linear issue did not include a team id.".to_string())?;
            let requested = linear_label_ids(&client, team_id, args)?;
            let label_ids = match command {
                "set" => requested,
                "add" => {
                    let mut values = issue.label_ids.clone();
                    for id in requested {
                        if !values.contains(&id) {
                            values.push(id);
                        }
                    }
                    values
                }
                "remove" => issue
                    .label_ids
                    .iter()
                    .filter(|id| !requested.contains(id))
                    .cloned()
                    .collect(),
                _ => unreachable!(),
            };
            fields.insert(
                "labelIds".into(),
                Value::Array(label_ids.into_iter().map(Value::String).collect()),
            );
        }
        _ => return Err(format!("Unknown Linear update command: {group} {command}")),
    }
    let outcome = runtime()?.block_on(client.update_issue_fields(&issue.id, Value::Object(fields)));
    linear_issue_write_output(outcome, json_output)
}

fn linear_comment_add_command(
    rest: &[&str],
    args: &[String],
    json_output: bool,
) -> Result<i32, String> {
    let reply_to = option_value(args, "--reply-to")?;
    let body = linear_body(args, true)?
        .ok_or_else(|| "linear comment add requires --body or --body-file.".to_string())?;
    let write_id = linear_write_id(args)?;
    let client = linear_client()?;
    let selector = linear_required_issue_selector(rest, args, "linear comment add")?;
    let issue = linear_get_issue(&client, &selector)?;
    match runtime()?.block_on(client.add_comment_with_parent(
        &write_id,
        &issue.id,
        &body,
        reply_to.as_deref(),
    )) {
        suaegi_tracker::WriteOutcome::Written(comment) => {
            output(
                json!({
                    "written": true,
                    "comment": {
                        "id": comment.id,
                        "url": comment.url,
                        "issueIdentifier": comment.issue_identifier,
                    }
                }),
                json_output,
                "Linear comment added.".into(),
            );
            Ok(0)
        }
        other => linear_non_issue_write_outcome(other, json_output, "comment"),
    }
}

fn linear_attach_command(rest: &[&str], args: &[String], json_output: bool) -> Result<i32, String> {
    let url = required_option(args, "--url", "linear attach")?;
    let parsed = url::Url::parse(&url).map_err(|_| "--url must be an absolute URL.".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("--url must use http or https.".into());
    }
    let title = option_value(args, "--title")?.unwrap_or_else(|| "Attached link".into());
    let write_id = linear_write_id(args)?;
    let client = linear_client()?;
    let selector = linear_required_issue_selector(rest, args, "linear attach")?;
    let issue = linear_get_issue(&client, &selector)?;
    match runtime()?.block_on(client.attach_link(&write_id, &issue.id, &title, &url)) {
        suaegi_tracker::WriteOutcome::Written(attachment) => {
            output(
                json!({
                    "written": true,
                    "attachment": {
                        "id": attachment.id,
                        "title": attachment.title,
                        "url": attachment.url,
                        "issueIdentifier": attachment.issue_identifier,
                    }
                }),
                json_output,
                "Linear link attached.".into(),
            );
            Ok(0)
        }
        other => linear_non_issue_write_outcome(other, json_output, "attachment"),
    }
}

fn linear_relation_command(
    command: &str,
    rest: &[&str],
    args: &[String],
    json_output: bool,
) -> Result<i32, String> {
    if !matches!(command, "add" | "remove" | "rm") {
        return Err(format!("Unknown Linear relation command: {command}"));
    }
    let related_selector = required_option(args, "--related", "linear relation")?;
    let requested_type = required_option(args, "--type", "linear relation")?;
    let relationship = match requested_type.as_str() {
        "blocks" => "blocks",
        "blocked-by" => "blockedBy",
        "related" => "relatedTo",
        "duplicate-of" => "duplicateOf",
        _ => return Err("--type must be blocks, blocked-by, related, or duplicate-of.".into()),
    };
    let selector = linear_required_issue_selector(rest, args, "linear relation")?;
    let client = linear_client()?;
    let issue = linear_get_issue(&client, &selector)?;
    let related = linear_get_issue(&client, &related_selector)?;
    if issue.id == related.id {
        return Err("A Linear issue cannot be related to itself.".into());
    }
    let page = match runtime()?.block_on(client.get_issue_relations(&issue.id)) {
        suaegi_tracker::Lookup::Found(page) => page,
        suaegi_tracker::Lookup::NotFound => suaegi_tracker::LinearRelationPage {
            relations: Vec::new(),
            has_more: false,
        },
        suaegi_tracker::Lookup::Unavailable(error) => {
            return Err(format_linear_unavailable(&error));
        }
    };
    if page.has_more {
        return Err(
            "Cannot safely modify this issue: more than 250 relations must be checked.".into(),
        );
    }
    let existing = page.relations.into_iter().find(|relation| {
        relation.related_issue_id == related.id && relation.relationship == relationship
    });
    let operation = if command == "add" { "add" } else { "remove" };
    if (operation == "add" && existing.is_some()) || (operation == "remove" && existing.is_none()) {
        output(
            json!({
                "operation": operation,
                "alreadySet": true,
                "issue": linear_issue_json(&issue, None),
                "relatedIssue": linear_issue_json(&related, None),
                "relation": existing.as_ref().map(linear_relation_json),
            }),
            json_output,
            format!("Linear relation already reflects {operation}."),
        );
        return Ok(0);
    }
    if operation == "remove" {
        let relation = existing.expect("checked above");
        return match runtime()?.block_on(client.delete_issue_relation(&relation.id)) {
            suaegi_tracker::WriteOutcome::Written(()) => {
                output(
                    json!({
                        "operation": "remove",
                        "alreadySet": false,
                        "issue": linear_issue_json(&issue, None),
                        "relatedIssue": linear_issue_json(&related, None),
                        "relation": linear_relation_json(&relation),
                    }),
                    json_output,
                    "Linear relation removed.".into(),
                );
                Ok(0)
            }
            other => linear_non_issue_write_outcome(other, json_output, "relation"),
        };
    }
    let (from, to, provider_type) = match relationship {
        "blockedBy" => (&related.id, &issue.id, "blocks"),
        "relatedTo" => (&issue.id, &related.id, "related"),
        "duplicateOf" => (&issue.id, &related.id, "duplicate"),
        _ => (&issue.id, &related.id, "blocks"),
    };
    match runtime()?.block_on(client.create_issue_relation(from, to, provider_type)) {
        suaegi_tracker::WriteOutcome::Written(relation) => {
            output(
                json!({
                    "operation": "add",
                    "alreadySet": false,
                    "issue": linear_issue_json(&issue, None),
                    "relatedIssue": linear_issue_json(&related, None),
                    "relation": linear_relation_json(&relation),
                }),
                json_output,
                "Linear relation added.".into(),
            );
            Ok(0)
        }
        other => linear_non_issue_write_outcome(other, json_output, "relation"),
    }
}

fn linear_relation_json(relation: &suaegi_tracker::LinearRelation) -> Value {
    json!({
        "id": relation.id,
        "type": relation.relation_type,
        "direction": relation.direction,
        "relationship": relation.relationship,
        "relatedIssue": {
            "id": relation.related_issue_id,
            "identifier": relation.related_identifier,
            "title": relation.related_title,
            "url": relation.related_url,
        }
    })
}

fn validate_linear_update_args(group: &str, command: &str, args: &[String]) -> Result<(), String> {
    match (group, command) {
        ("status", "set") => {
            required_option(args, "--to", "linear status set")?;
        }
        ("assignee", "set") => {
            let me = args.iter().any(|arg| arg == "--me");
            let to_id = option_value(args, "--to-id")?;
            if me == to_id.is_some() {
                return Err("Use exactly one of --me or --to-id for linear assignee set.".into());
            }
        }
        ("priority", "set") => {
            linear_priority(&required_option(args, "--to", "linear priority set")?)?;
        }
        ("estimate", "set") => {
            linear_number(
                &required_option(args, "--to", "linear estimate set")?,
                "--to",
            )?;
        }
        ("due-date", "set") => {
            validate_linear_due_date(&required_option(args, "--to", "linear due-date set")?)?;
        }
        ("label", "add" | "remove" | "set") if option_values(args, "--label")?.is_empty() => {
            return Err("At least one --label is required.".into());
        }
        ("assignee" | "priority" | "estimate" | "due-date", "clear")
        | ("label", "add" | "remove" | "set") => {}
        _ => return Err(format!("Unknown Linear update command: {group} {command}")),
    }
    Ok(())
}

fn linear_issue_fields(
    client: &suaegi_tracker::LinearClient,
    team_id: &str,
    args: &[String],
    creating: bool,
) -> Result<serde_json::Map<String, Value>, String> {
    let mut fields = serde_json::Map::new();
    if !creating {
        if let Some(title) = option_value(args, "--title")? {
            if title.trim().is_empty() {
                return Err("--title cannot be empty.".into());
            }
            fields.insert("title".into(), Value::String(title));
        }
    }
    if let Some(body) = linear_body(args, false)? {
        fields.insert("description".into(), Value::String(body));
    }
    if let Some(state) = option_value(args, "--state")? {
        fields.insert(
            "stateId".into(),
            Value::String(linear_resolve_state_id(client, team_id, &state)?),
        );
    }
    if let Some(assignee) = option_value(args, "--assignee")? {
        let value = if assignee == "null" {
            Value::Null
        } else if assignee.eq_ignore_ascii_case("me") {
            match runtime()?.block_on(client.viewer_id()) {
                suaegi_tracker::Lookup::Found(id) => Value::String(id),
                suaegi_tracker::Lookup::NotFound => {
                    return Err("Linear viewer was not found.".into());
                }
                suaegi_tracker::Lookup::Unavailable(error) => {
                    return Err(format_linear_unavailable(&error));
                }
            }
        } else {
            Value::String(linear_resolve_member_id(client, team_id, &assignee)?)
        };
        fields.insert("assigneeId".into(), value);
    }
    if let Some(priority) = option_value(args, "--priority")? {
        fields.insert(
            "priority".into(),
            Value::Number(linear_priority(&priority)?.into()),
        );
    }
    if let Some(estimate) = option_value(args, "--estimate")? {
        fields.insert(
            "estimate".into(),
            if estimate == "null" {
                Value::Null
            } else {
                linear_number(&estimate, "--estimate")?
            },
        );
    }
    if let Some(due_date) = option_value(args, "--due-date")? {
        let value = if due_date == "null" {
            Value::Null
        } else {
            validate_linear_due_date(&due_date)?;
            Value::String(due_date)
        };
        fields.insert("dueDate".into(), value);
    }
    let labels = option_values(args, "--label")?;
    if !labels.is_empty() {
        let ids = linear_label_ids(client, team_id, args)?;
        fields.insert(
            "labelIds".into(),
            Value::Array(ids.into_iter().map(Value::String).collect()),
        );
    }
    if let Some(project) = option_value(args, "--project")? {
        fields.insert(
            "projectId".into(),
            if project == "null" {
                Value::Null
            } else {
                Value::String(linear_resolve_project_id(client, &project)?)
            },
        );
    }
    let parent = option_value(args, "--parent-id")?.or(option_value(args, "--parent")?);
    if let Some(parent) = parent {
        fields.insert(
            "parentId".into(),
            if parent == "null" {
                Value::Null
            } else {
                Value::String(linear_get_issue(client, &parent)?.id)
            },
        );
    } else if args.iter().any(|arg| arg == "--parent-current") {
        let current = linear_current_issue()
            .ok_or_else(|| "No Linear issue is linked to the current worktree.".to_string())?;
        fields.insert(
            "parentId".into(),
            Value::String(linear_get_issue(client, &current)?.id),
        );
    }
    Ok(fields)
}

fn linear_optional_issue_selector(
    rest: &[&str],
    args: &[String],
) -> Result<Option<String>, String> {
    let positional = rest.first().map(|value| (*value).to_string());
    let explicit = option_value(args, "--id")?;
    let current = args.iter().any(|argument| argument == "--current");
    if positional.is_some() && explicit.is_some() {
        return Err("Provide the Linear issue either positionally or with --id, not both.".into());
    }
    if current && (positional.is_some() || explicit.is_some()) {
        return Err("Use either an issue id or --current, not both.".into());
    }
    Ok(positional
        .or(explicit)
        .or_else(|| current.then(linear_current_issue).flatten())
        .map(|value| normalize_linear_issue_input(&value)))
}

fn linear_required_issue_selector(
    rest: &[&str],
    args: &[String],
    command: &str,
) -> Result<String, String> {
    linear_optional_issue_selector(rest, args)?
        .ok_or_else(|| format!("{command} requires an issue id or --current."))
}

fn linear_get_issue(
    client: &suaegi_tracker::LinearClient,
    selector: &str,
) -> Result<suaegi_tracker::Issue, String> {
    let selector = normalize_linear_issue_input(selector);
    match runtime()?.block_on(client.get_issue(&selector)) {
        suaegi_tracker::Lookup::Found(issue) => Ok(issue),
        suaegi_tracker::Lookup::NotFound => Err(format!("Linear issue was not found: {selector}")),
        suaegi_tracker::Lookup::Unavailable(error) => Err(format_linear_unavailable(&error)),
    }
}

fn linear_resolve_state_id(
    client: &suaegi_tracker::LinearClient,
    team_id: &str,
    selector: &str,
) -> Result<String, String> {
    let matches = linear_team_states(client, team_id)?
        .into_iter()
        .filter(|state| state.id == selector || state.name.eq_ignore_ascii_case(selector))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [state] => Ok(state.id.clone()),
        [] => Err(format!("Linear workflow state was not found: {selector}")),
        _ => Err(format!("Linear workflow state is ambiguous: {selector}")),
    }
}

fn linear_resolve_member_id(
    client: &suaegi_tracker::LinearClient,
    team_id: &str,
    selector: &str,
) -> Result<String, String> {
    let members = match runtime()?.block_on(client.list_team_members(team_id)) {
        suaegi_tracker::Lookup::Found(members) => members,
        suaegi_tracker::Lookup::NotFound => Vec::new(),
        suaegi_tracker::Lookup::Unavailable(error) => {
            return Err(format_linear_unavailable(&error));
        }
    };
    let matches = members
        .into_iter()
        .filter(|member| {
            member.id == selector
                || member.name.eq_ignore_ascii_case(selector)
                || member
                    .email
                    .as_deref()
                    .is_some_and(|email| email.eq_ignore_ascii_case(selector))
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [member] => Ok(member.id.clone()),
        [] => Err(format!("Linear member was not found: {selector}")),
        _ => Err(format!("Linear member is ambiguous: {selector}")),
    }
}

fn linear_resolve_project_id(
    client: &suaegi_tracker::LinearClient,
    selector: &str,
) -> Result<String, String> {
    let projects = match runtime()?.block_on(client.list_projects()) {
        suaegi_tracker::Lookup::Found(projects) => projects,
        suaegi_tracker::Lookup::NotFound => Vec::new(),
        suaegi_tracker::Lookup::Unavailable(error) => {
            return Err(format_linear_unavailable(&error));
        }
    };
    let matches = projects
        .into_iter()
        .filter(|project| project.id == selector || project.name.eq_ignore_ascii_case(selector))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [project] => Ok(project.id.clone()),
        [] => Err(format!("Linear project was not found: {selector}")),
        _ => Err(format!("Linear project is ambiguous: {selector}")),
    }
}

fn linear_label_ids(
    client: &suaegi_tracker::LinearClient,
    team_id: &str,
    args: &[String],
) -> Result<Vec<String>, String> {
    let selectors = option_values(args, "--label")?
        .into_iter()
        .flat_map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    if selectors.is_empty() {
        return Err("At least one --label is required.".into());
    }
    let labels = linear_team_labels(client, team_id)?;
    selectors
        .into_iter()
        .map(|selector| {
            let matches = labels
                .iter()
                .filter(|label| label.id == selector || label.name.eq_ignore_ascii_case(&selector))
                .collect::<Vec<_>>();
            match matches.as_slice() {
                [label] => Ok(label.id.clone()),
                [] => Err(format!("Linear label was not found: {selector}")),
                _ => Err(format!("Linear label is ambiguous: {selector}")),
            }
        })
        .collect()
}

fn linear_priority(value: &str) -> Result<i64, String> {
    match value.to_ascii_lowercase().as_str() {
        "none" | "0" => Ok(0),
        "urgent" | "1" => Ok(1),
        "high" | "2" => Ok(2),
        "medium" | "3" => Ok(3),
        "low" | "4" => Ok(4),
        _ => Err("Linear priority must be none, low, medium, high, or urgent.".into()),
    }
}

fn linear_number(value: &str, flag: &str) -> Result<Value, String> {
    let number = value
        .parse::<f64>()
        .ok()
        .filter(|number| number.is_finite() && *number >= 0.0)
        .ok_or_else(|| format!("{flag} must be a non-negative number."))?;
    serde_json::Number::from_f64(number)
        .map(Value::Number)
        .ok_or_else(|| format!("{flag} must be a finite number."))
}

fn validate_linear_due_date(value: &str) -> Result<(), String> {
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map(|_| ())
        .map_err(|_| "--due-date/--to must use yyyy-mm-dd.".to_string())
}

fn linear_body(args: &[String], required: bool) -> Result<Option<String>, String> {
    let inline = option_value(args, "--body")?.or(option_value(args, "--description")?);
    let file = option_value(args, "--body-file")?;
    if inline.is_some() && file.is_some() {
        return Err("Use either --body/--description or --body-file, not both.".into());
    }
    let body = match (inline, file) {
        (Some(body), None) => Some(body),
        (None, Some(path)) if path == "-" => {
            let mut body = String::new();
            std::io::stdin()
                .read_to_string(&mut body)
                .map_err(|error| format!("Could not read Linear body from stdin: {error}"))?;
            Some(body)
        }
        (None, Some(path)) => {
            let path = resolve_cli_path(&path)?;
            Some(
                std::fs::read_to_string(&path)
                    .map_err(|error| format!("Could not read {path}: {error}"))?,
            )
        }
        (None, None) => None,
        (Some(_), Some(_)) => unreachable!(),
    };
    if required && body.as_deref().is_none_or(|body| body.trim().is_empty()) {
        return Err("A non-empty --body or --body-file is required.".into());
    }
    Ok(body)
}

fn linear_write_id(args: &[String]) -> Result<suaegi_tracker::WriteId, String> {
    let value = match option_value(args, "--write-id")? {
        Some(value) => value,
        None => {
            let mut bytes = [0_u8; 16];
            getrandom::fill(&mut bytes)
                .map_err(|error| format!("Could not create a Linear write id: {error}"))?;
            bytes[6] = (bytes[6] & 0x0f) | 0x40;
            bytes[8] = (bytes[8] & 0x3f) | 0x80;
            format!(
                "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
                bytes[0],
                bytes[1],
                bytes[2],
                bytes[3],
                bytes[4],
                bytes[5],
                bytes[6],
                bytes[7],
                bytes[8],
                bytes[9],
                bytes[10],
                bytes[11],
                bytes[12],
                bytes[13],
                bytes[14],
                bytes[15],
            )
        }
    };
    suaegi_tracker::WriteId::parse(&value).map_err(|_| "--write-id must be a UUID.".to_string())
}

fn linear_issue_write_output(
    outcome: suaegi_tracker::WriteOutcome<suaegi_tracker::Issue>,
    json_output: bool,
) -> Result<i32, String> {
    match outcome {
        suaegi_tracker::WriteOutcome::Written(issue) => {
            let plain = format!("Linear issue saved: {}", issue.identifier);
            output(
                json!({"written": true, "issue": linear_issue_json(&issue, None)}),
                json_output,
                plain,
            );
            Ok(0)
        }
        suaegi_tracker::WriteOutcome::Duplicate(id) => {
            output(
                json!({"written": false, "duplicate": true, "writeId": id}),
                json_output,
                "Linear write was already applied.".into(),
            );
            Ok(0)
        }
        suaegi_tracker::WriteOutcome::Rejected(error) => Err(format!(
            "Linear rejected the write. {}",
            format_linear_unavailable(&error)
        )),
        suaegi_tracker::WriteOutcome::Unconfirmed => {
            Err("Linear write outcome is unconfirmed. Retry with the same --write-id.".into())
        }
        suaegi_tracker::WriteOutcome::Unavailable(error) => Err(format_linear_unavailable(&error)),
    }
}

fn linear_non_issue_write_outcome<T>(
    outcome: suaegi_tracker::WriteOutcome<T>,
    json_output: bool,
    kind: &str,
) -> Result<i32, String> {
    match outcome {
        suaegi_tracker::WriteOutcome::Duplicate(id) => {
            output(
                json!({"written": false, "duplicate": true, "writeId": id}),
                json_output,
                format!("Linear {kind} write was already applied."),
            );
            Ok(0)
        }
        suaegi_tracker::WriteOutcome::Rejected(error) => Err(format!(
            "Linear rejected the write. {}",
            format_linear_unavailable(&error)
        )),
        suaegi_tracker::WriteOutcome::Unconfirmed => {
            Err("Linear write outcome is unconfirmed. Retry with the same --write-id.".into())
        }
        suaegi_tracker::WriteOutcome::Unavailable(error) => Err(format_linear_unavailable(&error)),
        suaegi_tracker::WriteOutcome::Written(_) => {
            Err(format!("Linear {kind} result could not be formatted."))
        }
    }
}

fn linear_issue_list_output(
    issues: Vec<suaegi_tracker::Issue>,
    limit: usize,
    json_output: bool,
) -> Result<i32, String> {
    let has_more = issues.len() > limit;
    linear_issue_list_output_with_more(issues, limit, has_more, json_output)
}

fn linear_issue_list_output_with_more(
    issues: Vec<suaegi_tracker::Issue>,
    limit: usize,
    has_more: bool,
    json_output: bool,
) -> Result<i32, String> {
    let issues = issues.into_iter().take(limit).collect::<Vec<_>>();
    let values = issues
        .iter()
        .map(|issue| linear_issue_json(issue, None))
        .collect::<Vec<_>>();
    let plain = format_linear_issue_rows(&issues);
    output(
        json!({"issues": values, "hasMore": has_more}),
        json_output,
        plain,
    );
    Ok(0)
}

fn positive_limit(args: &[String], default: usize, maximum: usize) -> Result<usize, String> {
    option_value(args, "--limit")?
        .map(|value| {
            value
                .parse::<usize>()
                .ok()
                .filter(|value| *value > 0)
                .map(|value| value.min(maximum))
                .ok_or_else(|| "--limit must be a positive integer.".to_string())
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn linear_current_issue() -> Option<String> {
    let cwd = std::env::current_dir().ok()?.canonicalize().ok()?;
    load_state()
        .worktrees
        .into_iter()
        .filter(|worktree| cwd.starts_with(&worktree.path))
        .max_by_key(|worktree| worktree.path.as_os_str().len())
        .and_then(|worktree| worktree.linked_linear_issue)
}

fn normalize_linear_issue_input(value: &str) -> String {
    if let Ok(url) = url::Url::parse(value) {
        let segments = url
            .path_segments()
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        if let Some(index) = segments.iter().position(|segment| *segment == "issue") {
            if let Some(identifier) = segments.get(index + 1) {
                return (*identifier).to_string();
            }
        }
    }
    value.to_string()
}

fn linear_issue_json(issue: &suaegi_tracker::Issue, comments: Option<Vec<Value>>) -> Value {
    let mut value = json!({
        "id": issue.id,
        "identifier": issue.identifier,
        "title": issue.title,
        "description": issue.description,
        "url": issue.url,
        "state": {
            "id": issue.state_id,
            "name": issue.state,
            "type": issue.state_type,
        },
        "assignee": {
            "id": issue.assignee_id,
            "name": issue.assignee,
        },
        "creatorId": issue.creator_id,
        "team": {
            "id": issue.team_id,
            "key": issue.team_key,
            "name": issue.team_name,
        },
        "priority": issue.priority,
        "estimate": issue.estimate,
        "dueDate": issue.due_date,
        "labels": issue.label_ids.iter().zip(issue.label_names.iter()).map(|(id, name)| {
            json!({"id": id, "name": name})
        }).collect::<Vec<_>>(),
        "project": {
            "id": issue.project_id,
            "name": issue.project_name,
        },
        "parent": {
            "id": issue.parent_id,
            "identifier": issue.parent_identifier,
        },
    });
    if let Some(comments) = comments {
        value["comments"] = Value::Array(comments);
    }
    value
}

fn format_linear_issue(issue: &suaegi_tracker::Issue) -> String {
    format!(
        "{}  {}  {}  {}",
        issue.identifier,
        issue.state.as_deref().unwrap_or("Unknown"),
        issue.assignee.as_deref().unwrap_or("Unassigned"),
        issue.title,
    )
}

fn format_linear_issue_rows(issues: &[suaegi_tracker::Issue]) -> String {
    if issues.is_empty() {
        "No Linear issues found.".into()
    } else {
        issues
            .iter()
            .map(format_linear_issue)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn format_linear_unavailable(error: &suaegi_tracker::Classified) -> String {
    error
        .user_message
        .clone()
        .unwrap_or_else(|| match error.kind {
            suaegi_tracker::TrackerUnavailable::NotAuthenticated => {
                "Linear authentication failed. Reconnect Linear in Suaegi.".into()
            }
            suaegi_tracker::TrackerUnavailable::RateLimited => {
                "Linear is rate limiting requests. Retry later.".into()
            }
            suaegi_tracker::TrackerUnavailable::Forbidden => {
                "The Linear account cannot access this resource.".into()
            }
            suaegi_tracker::TrackerUnavailable::Network => {
                "Linear is unavailable because of a network error.".into()
            }
            suaegi_tracker::TrackerUnavailable::Internal => {
                "Linear reported an internal error.".into()
            }
            suaegi_tracker::TrackerUnavailable::InvalidInput => {
                "Linear rejected the request input.".into()
            }
            suaegi_tracker::TrackerUnavailable::Unknown => {
                "Linear returned an unexpected response.".into()
            }
        })
}

fn format_computer_result(command: &str, result: &Value) -> String {
    match command {
        "capabilities" => format!(
            "Computer Use provider: {} ({})",
            result["provider"].as_str().unwrap_or("unavailable"),
            if result["available"].as_bool().unwrap_or(false) {
                "available"
            } else {
                "unavailable"
            }
        ),
        "list-apps" => result["apps"]
            .as_array()
            .map(|apps| {
                apps.iter()
                    .map(|app| {
                        format!(
                            "{}\t{}\t{}",
                            app["name"].as_str().unwrap_or("App"),
                            app["bundleIdentifier"].as_str().unwrap_or(""),
                            app["pid"].as_i64().unwrap_or_default()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default(),
        "list-windows" => result["windows"]
            .as_array()
            .map(|windows| {
                windows
                    .iter()
                    .map(|window| {
                        format!(
                            "{}\t{}",
                            window["index"].as_u64().unwrap_or_default(),
                            window["title"].as_str().unwrap_or("Window")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default(),
        "permissions" => "Opened macOS Computer Use permissions.".into(),
        _ => format!("Computer {command} completed."),
    }
}

fn project_command(command: &str, args: &[String], json_output: bool) -> Result<i32, String> {
    match command {
        "list" => {
            let result = local_rpc_required("project.list", Value::Null)?;
            let plain = result["projects"]
                .as_array()
                .map(|projects| {
                    projects
                        .iter()
                        .map(|project| {
                            format!(
                                "{}  {}",
                                project["id"].as_str().unwrap_or_default(),
                                project["displayName"].as_str().unwrap_or_default()
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "No projects found.".into());
            output(result, json_output, plain);
            Ok(0)
        }
        "setups" => {
            let mut result = local_rpc_required("projectHostSetup.list", Value::Null)?;
            let project = option_value(args, "--project")?;
            let host = option_value(args, "--host")?;
            if let Some(setups) = result["setups"].as_array_mut() {
                setups.retain(|setup| {
                    project
                        .as_ref()
                        .is_none_or(|project| setup["projectId"].as_str() == Some(project.as_str()))
                        && host
                            .as_ref()
                            .is_none_or(|host| setup["hostId"].as_str() == Some(host.as_str()))
                });
            }
            let plain = result["setups"]
                .as_array()
                .map(|setups| {
                    setups
                        .iter()
                        .map(|setup| {
                            format!(
                                "{}  project:{}  host:{}  {}  {}",
                                setup["id"].as_str().unwrap_or_default(),
                                setup["projectId"].as_str().unwrap_or_default(),
                                setup["hostId"].as_str().unwrap_or_default(),
                                setup["setupState"].as_str().unwrap_or_default(),
                                setup["path"].as_str().unwrap_or_default(),
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "No project host setups found.".into());
            output(result, json_output, plain);
            Ok(0)
        }
        "setup-existing-folder" => {
            let project = required_option(args, "--project", command)?;
            let host = required_option(args, "--host", command)?;
            let path = resolve_cli_path(&required_option(args, "--path", command)?)?;
            let kind = project_kind(args)?.unwrap_or_else(|| "git".into());
            let mut params = json!({
                "projectId": project,
                "hostId": host,
                "path": path,
                "kind": kind,
            });
            if let Some(name) = option_value(args, "--display-name")? {
                params["displayName"] = Value::String(name);
            }
            let result = local_rpc_required("projectHostSetup.setupExistingFolder", params)?;
            output(
                result.clone(),
                json_output,
                format_project_setup_result(&result, false),
            );
            Ok(0)
        }
        "setup-clone" => project_setup_clone(args, json_output),
        "setup-create" => {
            let project = required_option(args, "--project", command)?;
            let host = required_option(args, "--host", command)?;
            let mut params = serde_json::Map::new();
            params.insert("projectId".into(), Value::String(project));
            params.insert("hostId".into(), Value::String(host));
            project_setup_optional_fields(args, &mut params, false)?;
            let result = local_rpc_required("projectHostSetup.create", Value::Object(params))?;
            output(
                result.clone(),
                json_output,
                format_project_setup_result(&result, false),
            );
            Ok(0)
        }
        "setup-update" => {
            let setup = required_option(args, "--setup", command)?;
            let mut updates = serde_json::Map::new();
            project_setup_optional_fields(args, &mut updates, true)?;
            if updates.is_empty() {
                return Err("project setup-update requires at least one metadata flag.".into());
            }
            let result = local_rpc_required(
                "projectHostSetup.update",
                json!({"setupId": setup, "updates": updates}),
            )?;
            output(
                result.clone(),
                json_output,
                format_project_setup_result(&result, false),
            );
            Ok(0)
        }
        "setup-delete" => {
            let setup = required_option(args, "--setup", command)?;
            let result = local_rpc_required("projectHostSetup.delete", json!({"setupId": setup}))?;
            output(
                result.clone(),
                json_output,
                format!(
                    "deleted: {setup}\n{}",
                    format_project_setup_result(&result, false)
                ),
            );
            Ok(0)
        }
        _ => Err(format!("Unknown project command: {command}")),
    }
}

fn project_setup_optional_fields(
    args: &[String],
    params: &mut serde_json::Map<String, Value>,
    allow_legacy_method: bool,
) -> Result<(), String> {
    for (flag, field) in [
        ("--setup-id", "setupId"),
        ("--display-name", "displayName"),
        ("--worktree-base-path", "worktreeBasePath"),
        ("--git-username", "gitUsername"),
    ] {
        if let Some(value) = option_value(args, flag)? {
            params.insert(field.into(), Value::String(value));
        }
    }
    if let Some(path) = option_value(args, "--path")? {
        params.insert("path".into(), Value::String(resolve_cli_path(&path)?));
    }
    if let Some(kind) = project_kind(args)? {
        params.insert("kind".into(), Value::String(kind));
    }
    if let Some(state) = option_value(args, "--state")? {
        if !matches!(
            state.as_str(),
            "ready" | "not-set-up" | "setting-up" | "error" | "unsupported"
        ) {
            return Err(
                "--state must be ready, not-set-up, setting-up, error, or unsupported.".into(),
            );
        }
        params.insert("setupState".into(), Value::String(state));
    }
    if let Some(method) = option_value(args, "--method")? {
        let valid = matches!(
            method.as_str(),
            "imported-existing-folder" | "cloned" | "provisioned"
        ) || (allow_legacy_method && method == "legacy-repo");
        if !valid {
            return Err(if allow_legacy_method {
                "--method must be legacy-repo, imported-existing-folder, cloned, or provisioned."
                    .into()
            } else {
                "--method must be imported-existing-folder, cloned, or provisioned.".into()
            });
        }
        params.insert("setupMethod".into(), Value::String(method));
    }
    Ok(())
}

fn project_kind(args: &[String]) -> Result<Option<String>, String> {
    let kind = option_value(args, "--kind")?;
    if kind
        .as_deref()
        .is_some_and(|kind| !matches!(kind, "git" | "folder"))
    {
        return Err("--kind must be git or folder.".into());
    }
    Ok(kind)
}

fn project_setup_clone(args: &[String], json_output: bool) -> Result<i32, String> {
    let project = required_option(args, "--project", "project setup-clone")?;
    let host = required_option(args, "--host", "project setup-clone")?;
    if host != "local" {
        return Err(
            "CLI cloning currently requires --host local; SSH clones are available in project settings."
                .into(),
        );
    }
    let url = required_option(args, "--url", "project setup-clone")?;
    let destination = resolve_cli_path(&required_option(
        args,
        "--destination",
        "project setup-clone",
    )?)?;
    let destination = PathBuf::from(destination);
    std::fs::create_dir_all(&destination)
        .map_err(|error| format!("Could not create {}: {error}", destination.display()))?;
    let name = clone_directory_name(&url)
        .ok_or_else(|| "Could not derive a checkout name from --url.".to_string())?;
    let checkout = destination.join(name);
    if checkout.exists() {
        return Err(format!(
            "Clone destination already exists: {}",
            checkout.display()
        ));
    }
    let clone = std::process::Command::new("git")
        .arg("clone")
        .arg("--")
        .arg(&url)
        .arg(&checkout)
        .output()
        .map_err(|error| format!("Could not start git clone: {error}"))?;
    if !clone.status.success() {
        let detail = String::from_utf8_lossy(&clone.stderr);
        let detail = detail.trim().chars().take(2_000).collect::<String>();
        return Err(if detail.is_empty() {
            format!("git clone failed with {}", clone.status)
        } else {
            format!("git clone failed: {detail}")
        });
    }
    let mut params = json!({
        "projectId": project,
        "hostId": host,
        "path": checkout,
        "kind": "git",
        "setupMethod": "cloned",
    });
    if let Some(name) = option_value(args, "--display-name")? {
        params["displayName"] = Value::String(name);
    }
    let result = local_rpc_required("projectHostSetup.setupExistingFolder", params)?;
    output(
        result.clone(),
        json_output,
        format_project_setup_result(&result, false),
    );
    Ok(0)
}

fn clone_directory_name(url: &str) -> Option<String> {
    let value = url.trim().trim_end_matches('/');
    let candidate = if let Ok(url) = url::Url::parse(value) {
        url.path_segments()?
            .rfind(|segment| !segment.is_empty())?
            .to_string()
    } else {
        value
            .rsplit_once('/')
            .map(|(_, name)| name)
            .or_else(|| value.rsplit_once(':').map(|(_, name)| name))
            .unwrap_or(value)
            .to_string()
    }
    .trim_end_matches(".git")
    .to_string();
    (!candidate.is_empty()
        && candidate != "."
        && candidate != ".."
        && !candidate.contains(['/', '\\', '\0']))
    .then_some(candidate)
}

fn resolve_cli_path(raw: &str) -> Result<String, String> {
    let path = if raw == "~" {
        dirs::home_dir().ok_or_else(|| "The home directory is unavailable.".to_string())?
    } else if let Some(relative) = raw.strip_prefix("~/") {
        dirs::home_dir()
            .ok_or_else(|| "The home directory is unavailable.".to_string())?
            .join(relative)
    } else {
        let path = PathBuf::from(raw);
        if path.is_absolute() {
            path
        } else {
            std::env::current_dir()
                .map_err(|error| format!("Could not resolve the current directory: {error}"))?
                .join(path)
        }
    };
    Ok(path.to_string_lossy().into_owned())
}

fn format_project_setup_result(result: &Value, deleted: bool) -> String {
    let result = &result["result"];
    let setup = &result["setup"];
    let project = &result["project"];
    let mut rows = Vec::new();
    if deleted {
        rows.push(format!(
            "deleted: {}",
            setup["id"].as_str().unwrap_or_default()
        ));
    }
    rows.extend([
        format!("projectId: {}", project["id"].as_str().unwrap_or_default()),
        format!(
            "project: {}",
            project["displayName"].as_str().unwrap_or_default()
        ),
        format!("setupId: {}", setup["id"].as_str().unwrap_or_default()),
        format!("hostId: {}", setup["hostId"].as_str().unwrap_or_default()),
        format!("path: {}", setup["path"].as_str().unwrap_or_default()),
        format!(
            "state: {}",
            setup["setupState"].as_str().unwrap_or_default()
        ),
        format!(
            "method: {}",
            setup["setupMethod"].as_str().unwrap_or_default()
        ),
    ]);
    rows.join("\n")
}

fn environment_command(command: &str, args: &[String], json_output: bool) -> Result<i32, String> {
    match command {
        "list" => {
            if let Some(result) =
                crate::local_rpc::call("environment.list", serde_json::Value::Null)?
            {
                let plain = result["environments"]
                    .as_array()
                    .map(|environments| {
                        environments
                            .iter()
                            .map(|environment| {
                                format!(
                                    "{}\t{}",
                                    environment["name"].as_str().unwrap_or("Environment"),
                                    environment["endpoint"].as_str().unwrap_or_default()
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .filter(|rows| !rows.is_empty())
                    .unwrap_or_else(|| "No saved environments".into());
                output(result, json_output, plain);
                return Ok(0);
            }
            let state = load_state();
            let environments = &state.settings.ui.runtime_environments;
            let plain = if environments.is_empty() {
                "No saved environments".into()
            } else {
                environments
                    .iter()
                    .map(|environment| format!("{}\t{}", environment.name, environment.endpoint))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            output(
                json!({"environments": environments.iter().map(environment_json).collect::<Vec<_>>() }),
                json_output,
                plain,
            );
            Ok(0)
        }
        "show" => {
            let selector = option_value(args, "--environment")?
                .ok_or_else(|| "environment show requires --environment <selector>".to_string())?;
            if let Some(result) =
                crate::local_rpc::call("environment.show", json!({"environment": selector}))?
            {
                let environment = &result["environment"];
                let plain = format!(
                    "{} ({})\n  endpoint: {}\n  credentials: {}",
                    environment["name"].as_str().unwrap_or("Environment"),
                    environment["id"].as_str().unwrap_or_default(),
                    environment["endpoint"].as_str().unwrap_or_default(),
                    if environment["credentialsConfigured"]
                        .as_bool()
                        .unwrap_or(false)
                    {
                        "configured"
                    } else {
                        "missing"
                    }
                );
                output(result, json_output, plain);
                return Ok(0);
            }
            let state = load_state();
            let environment = select_environment(&state, &selector)?;
            output(
                json!({"environment": environment_json(environment)}),
                json_output,
                pretty_environment(environment),
            );
            Ok(0)
        }
        "add" => {
            let name = option_value(args, "--name")?
                .ok_or_else(|| "environment add requires --name <name>".to_string())?;
            let pairing_code = option_value(args, "--pairing-code")?
                .ok_or_else(|| "environment add requires --pairing-code <code>".to_string())?;
            if let Some(result) = crate::local_rpc::call(
                "environment.add",
                json!({"name": name, "pairingCode": pairing_code}),
            )? {
                let environment = &result["environment"];
                let plain = format!(
                    "Saved environment {} ({}).",
                    environment["name"].as_str().unwrap_or("Environment"),
                    environment["id"].as_str().unwrap_or_default()
                );
                output(result, json_output, plain);
                return Ok(0);
            }
            let mut store = data_store();
            let mut state = store.load().state;
            if state
                .settings
                .ui
                .runtime_environments
                .iter()
                .any(|environment| environment.name.eq_ignore_ascii_case(name.trim()))
            {
                return Err(format!(
                    "An environment named \"{}\" already exists.",
                    name.trim()
                ));
            }
            let environment =
                runtime()?.block_on(crate::remote_runtime::save_environment(name, pairing_code))?;
            state
                .settings
                .ui
                .runtime_environments
                .push(environment.clone());
            store
                .save(&state)
                .map_err(|error| format!("Could not save environment: {error}"))?;
            output(
                json!({"environment": environment_json(&environment)}),
                json_output,
                format!(
                    "Saved environment {} ({}).",
                    environment.name, environment.id
                ),
            );
            Ok(0)
        }
        "rm" | "remove" | "delete" => {
            let selector = option_value(args, "--environment")?
                .ok_or_else(|| "environment rm requires --environment <selector>".to_string())?;
            if let Some(result) =
                crate::local_rpc::call("environment.remove", json!({"environment": selector}))?
            {
                let removed = &result["removed"];
                let plain = format!(
                    "Removed environment {} ({}).",
                    removed["name"].as_str().unwrap_or("Environment"),
                    removed["id"].as_str().unwrap_or_default()
                );
                output(result, json_output, plain);
                return Ok(0);
            }
            let mut store = data_store();
            let mut state = store.load().state;
            let environment = select_environment(&state, &selector)?.clone();
            runtime()?.block_on(crate::remote_runtime::remove_environment(
                environment.id.clone(),
            ))?;
            state
                .settings
                .ui
                .runtime_environments
                .retain(|candidate| candidate.id != environment.id);
            if state.settings.ui.active_runtime_environment_id.as_deref()
                == Some(environment.id.as_str())
            {
                state.settings.ui.active_runtime_environment_id = None;
            }
            store
                .save(&state)
                .map_err(|error| format!("Could not save environment removal: {error}"))?;
            output(
                json!({"removed": environment_json(&environment)}),
                json_output,
                format!(
                    "Removed environment {} ({}).",
                    environment.name, environment.id
                ),
            );
            Ok(0)
        }
        _ => Err(format!("Unknown environment command: {command}")),
    }
}

fn agent_hook_status(enabled: bool, applied_by: &str) -> Value {
    let script = crate::agent_status::inject::hook_script_path();
    let installed = script.is_file();
    json!({
        "enabled": enabled,
        "settingsPath": crate::persistence_thread::default_data_file(),
        "appliedBy": applied_by,
        "statuses": [{
            "agent": "claude",
            "state": if installed { "installed" } else { "not-installed" },
            "path": script,
        }],
    })
}

fn format_agent_hook_status(result: &Value) -> String {
    let status = result["statuses"]
        .as_array()
        .and_then(|statuses| statuses.first())
        .map(|status| {
            format!(
                "{}: {}",
                status["agent"].as_str().unwrap_or("claude"),
                status["state"].as_str().unwrap_or("unknown")
            )
        })
        .unwrap_or_default();
    format!(
        "agentStatusHooksEnabled: {}\nappliedBy: {}\nsettingsPath: {}\n{}",
        result["enabled"].as_bool().unwrap_or(false),
        result["appliedBy"].as_str().unwrap_or("offline"),
        result["settingsPath"].as_str().unwrap_or_default(),
        status
    )
}

fn agent_hooks_command(command: &str, json_output: bool) -> Result<i32, String> {
    if !matches!(command, "status" | "on" | "off") {
        return Err(format!("Unknown agent hooks command: {command}"));
    }
    if let Some(result) = crate::local_rpc::call("agent.hooks", json!({"action": command}))? {
        let plain = format_agent_hook_status(&result);
        output(result, json_output, plain);
        return Ok(0);
    }

    let mut store = data_store();
    let mut state = store.load().state;
    if command != "status" {
        let enabled = command == "on";
        let script = crate::agent_status::inject::hook_script_path();
        if enabled {
            crate::agent_status::inject::install_hook_script(&script)
                .map_err(|error| format!("Could not install agent hook: {error}"))?;
        } else if script.exists() {
            std::fs::remove_file(&script)
                .map_err(|error| format!("Could not remove agent hook: {error}"))?;
        }
        state.settings.ui.agent_status_hooks_enabled = enabled;
        store
            .save(&state)
            .map_err(|error| format!("Could not save agent hook setting: {error}"))?;
    }
    let result = agent_hook_status(state.settings.ui.agent_status_hooks_enabled, "offline");
    let plain = format_agent_hook_status(&result);
    output(result, json_output, plain);
    Ok(0)
}

fn add_repo(path: &str, json_output: bool) -> Result<i32, String> {
    if let Some(result) = crate::local_rpc::call("repo.add", json!({"path": path}))? {
        let plain = format!(
            "{}\n  path: {}\n  base ref: {}",
            result["name"].as_str().unwrap_or("repository"),
            result["path"].as_str().unwrap_or(path),
            result["baseRef"].as_str().unwrap_or("auto"),
        );
        output(result, json_output, plain);
        return Ok(0);
    }
    let mut store = data_store();
    let mut state = store.load().state;
    let repo = crate::git_tasks::build_repo_now(PathBuf::from(path))?;
    let repo = runtime()?
        .block_on(crate::git_tasks::probe_repo_now(repo))?
        .0;
    if let Some(existing) = state.repos.iter_mut().find(|item| item.id == repo.id) {
        *existing = repo.clone();
    } else {
        state.repos.push(repo.clone());
    }
    store
        .save(&state)
        .map_err(|error| format!("Could not save repository: {error}"))?;
    output(repo_json(&repo), json_output, pretty_repo(&repo));
    Ok(0)
}

fn set_repo_base_ref(args: &[String], json_output: bool) -> Result<i32, String> {
    let repo = option_value(args, "--repo")?
        .ok_or_else(|| "repo set-base-ref requires --repo <selector>".to_string())?;
    let reference = option_value(args, "--ref")?
        .ok_or_else(|| "repo set-base-ref requires --ref <ref>".to_string())?;
    let result = local_rpc_required("repo.set_base_ref", json!({"repo": repo, "ref": reference}))?;
    output(
        result,
        json_output,
        format!("Set {repo} base ref to {reference}"),
    );
    Ok(0)
}

fn search_repo_refs(args: &[String], json_output: bool) -> Result<i32, String> {
    let selector = option_value(args, "--repo")?
        .ok_or_else(|| "repo search-refs requires --repo <selector>".to_string())?;
    let query = option_value(args, "--query")?
        .ok_or_else(|| "repo search-refs requires --query <text>".to_string())?;
    let limit = option_value(args, "--limit")?
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| "--limit must be a positive integer".to_string())
        })
        .transpose()?
        .unwrap_or(20)
        .clamp(1, 1_000);
    let state = load_state();
    let repo = select_repo(&state, selector.trim_start_matches("id:"))?;
    let git_output = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo.path)
        .args([
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ])
        .output()
        .map_err(|error| format!("Could not search Git refs: {error}"))?;
    if !git_output.status.success() {
        return Err(String::from_utf8_lossy(&git_output.stderr)
            .trim()
            .to_string());
    }
    let needle = query.to_lowercase();
    let mut refs = String::from_utf8_lossy(&git_output.stdout)
        .lines()
        .filter(|reference| reference.to_lowercase().contains(&needle))
        .take(limit)
        .map(str::to_string)
        .collect::<Vec<_>>();
    refs.sort();
    let plain = if refs.is_empty() {
        "No matching refs".into()
    } else {
        refs.join("\n")
    };
    output(
        json!({"repoId": repo.id.0, "query": query, "refs": refs}),
        json_output,
        plain,
    );
    Ok(0)
}

pub(crate) fn option_value(args: &[String], name: &str) -> Result<Option<String>, String> {
    let Some(index) = args.iter().position(|arg| arg == name) else {
        return Ok(None);
    };
    args.get(index + 1)
        .filter(|value| !value.starts_with("--"))
        .cloned()
        .ok_or_else(|| format!("{name} requires a value"))
        .map(Some)
}

fn option_values(args: &[String], name: &str) -> Result<Vec<String>, String> {
    let mut values = Vec::new();
    for (index, argument) in args.iter().enumerate() {
        if argument == name {
            values.push(
                args.get(index + 1)
                    .filter(|value| !value.starts_with("--"))
                    .cloned()
                    .ok_or_else(|| format!("{name} requires a value"))?,
            );
        }
    }
    Ok(values)
}

fn show_worktree_live(selector: &str, json_output: bool) -> Result<i32, String> {
    let result = local_rpc_required("worktree.show", json!({"worktree": selector}))?;
    let worktree = result.get("worktree").cloned().unwrap_or(Value::Null);
    let plain = format!(
        "{}\n  branch: {}\n  status: {}\n  presence: {}",
        worktree["path"].as_str().unwrap_or(selector),
        worktree["branch"].as_str().unwrap_or("(detached)"),
        worktree["workspaceStatus"]
            .as_str()
            .unwrap_or("in_progress"),
        worktree["presence"].as_str().unwrap_or("unknown"),
    );
    output(result, json_output, plain);
    Ok(0)
}

fn set_worktree(args: &[String], json_output: bool) -> Result<i32, String> {
    let selector = option_value(args, "--worktree")?.unwrap_or_else(|| "active".to_string());
    let mut params = serde_json::Map::new();
    params.insert("worktree".into(), Value::String(selector.clone()));
    for (flag, field) in [
        ("--display-name", "displayName"),
        ("--comment", "comment"),
        ("--workspace-status", "workspaceStatus"),
        ("--parent-worktree", "parentWorktree"),
    ] {
        if let Some(value) = option_value(args, flag)? {
            params.insert(field.into(), Value::String(value));
        }
    }
    if args.iter().any(|arg| arg == "--no-parent") {
        params.insert("noParent".into(), Value::Bool(true));
    }
    if let Some(value) = option_value(args, "--issue")? {
        let value = if value == "null" {
            Value::Null
        } else {
            Value::from(
                value
                    .parse::<u64>()
                    .map_err(|_| "--issue must be a positive integer or null".to_string())?,
            )
        };
        params.insert("linkedIssue".into(), value);
    }
    if let Some(value) = option_value(args, "--linear-issue")? {
        params.insert(
            "linearIssue".into(),
            if value == "null" {
                Value::Null
            } else {
                Value::String(value)
            },
        );
    }
    if params.len() == 1 {
        return Err("worktree set requires at least one metadata flag.".into());
    }
    let result = local_rpc_required("worktree.set", Value::Object(params))?;
    output(result, json_output, format!("Updated {selector}"));
    Ok(0)
}

fn worktree_ps(args: &[String], json_output: bool) -> Result<i32, String> {
    let limit = option_value(args, "--limit")?
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| "--limit must be a positive integer".to_string())
        })
        .transpose()?
        .unwrap_or(100)
        .clamp(1, 10_000);
    let result = local_rpc_required("worktree.ps", json!({"limit": limit}))?;
    let plain = result["worktrees"]
        .as_array()
        .map(|worktrees| {
            worktrees
                .iter()
                .map(|worktree| {
                    format!(
                        "{}\t{}\t{}\t{}",
                        worktree["displayName"].as_str().unwrap_or("workspace"),
                        worktree["workspaceStatus"]
                            .as_str()
                            .unwrap_or("in_progress"),
                        worktree["presence"].as_str().unwrap_or("unknown"),
                        worktree["comment"].as_str().unwrap_or_default(),
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_else(|| "No worktrees".into());
    output(result, json_output, plain);
    Ok(0)
}

fn list_worktrees(args: &[String], json_output: bool) -> Result<i32, String> {
    let state = load_state();
    let selected = option_value(args, "--repo")?;
    let repos = if let Some(selector) = selected {
        vec![select_repo(&state, &selector)?.clone()]
    } else {
        state.repos.clone()
    };
    let runtime = runtime()?;
    let mut rows = Vec::new();
    let mut plain = Vec::new();
    for repo in repos {
        let entries = runtime.block_on(crate::git_tasks::list_worktrees_now(repo.clone()))?;
        for entry in entries {
            rows.push(json!({
                "repoId": repo.id.0,
                "repo": repo.display_name,
                "path": entry.path,
                "branch": entry.branch,
                "head": entry.head,
                "isMain": entry.is_main,
            }));
            plain.push(format!(
                "{}\t{}\t{}{}",
                repo.display_name,
                entry.branch.as_deref().unwrap_or("(detached)"),
                entry.path.display(),
                if entry.is_main { "\t(main)" } else { "" }
            ));
        }
    }
    output(
        json!({"worktrees": rows}),
        json_output,
        if plain.is_empty() {
            "No worktrees".into()
        } else {
            plain.join("\n")
        },
    );
    Ok(0)
}

fn current_worktree(json_output: bool) -> Result<i32, String> {
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    let state = load_state();
    let runtime = runtime()?;
    for repo in state.repos {
        let entries = runtime.block_on(crate::git_tasks::list_worktrees_now(repo.clone()))?;
        if let Some(entry) = entries
            .into_iter()
            .filter(|entry| cwd.starts_with(&entry.path))
            .max_by_key(|entry| entry.path.components().count())
        {
            let value = json!({
                "repoId": repo.id.0,
                "repo": repo.display_name,
                "path": entry.path,
                "branch": entry.branch,
                "head": entry.head,
                "isMain": entry.is_main,
            });
            output(value, json_output, entry.path.display().to_string());
            return Ok(0);
        }
    }
    Err(format!(
        "{} is not inside a registered Suaegi worktree",
        cwd.display()
    ))
}

fn show_worktree(selector: &str, json_output: bool) -> Result<i32, String> {
    let state = load_state();
    let runtime = runtime()?;
    for repo in state.repos {
        let entries = runtime.block_on(crate::git_tasks::list_worktrees_now(repo.clone()))?;
        if let Some(entry) = entries.into_iter().find(|entry| {
            entry.path == Path::new(selector)
                || entry
                    .path
                    .file_name()
                    .is_some_and(|name| name == OsStr::new(selector))
                || entry.branch.as_deref() == Some(selector)
        }) {
            let plain = format!(
                "{}\n  repository: {}\n  branch: {}",
                entry.path.display(),
                repo.display_name,
                entry.branch.as_deref().unwrap_or("(detached)")
            );
            output(
                json!({
                    "repoId": repo.id.0,
                    "repo": repo.display_name,
                    "path": entry.path,
                    "branch": entry.branch,
                    "head": entry.head,
                    "isMain": entry.is_main,
                }),
                json_output,
                plain,
            );
            return Ok(0);
        }
    }
    Err(format!("Worktree not found: {selector}"))
}

fn create_worktree(args: &[String], json_output: bool) -> Result<i32, String> {
    let name = option_value(args, "--name")?
        .ok_or_else(|| "worktree create requires --name <name>".to_string())?;
    let repo_selector = option_value(args, "--repo")?;
    let base_branch = option_value(args, "--base-branch")?;
    let agent = option_value(args, "--agent")?;
    let mut rpc_params = serde_json::Map::new();
    rpc_params.insert("name".into(), Value::String(name.clone()));
    if let Some(repo) = repo_selector.as_ref() {
        rpc_params.insert("repo".into(), Value::String(repo.clone()));
    }
    if let Some(base) = base_branch.as_ref() {
        rpc_params.insert("baseBranch".into(), Value::String(base.clone()));
    }
    if let Some(agent) = agent.as_ref() {
        rpc_params.insert("agent".into(), Value::String(agent.clone()));
    }
    if let Some(result) = crate::local_rpc::call("worktree.create", Value::Object(rpc_params))? {
        output(
            result.clone(),
            json_output,
            format!(
                "Created {} at {}",
                result["branch"].as_str().unwrap_or(&name),
                result["path"].as_str().unwrap_or_default(),
            ),
        );
        return Ok(0);
    }
    let mut store = data_store();
    let mut state = store.load().state;
    let runtime = runtime()?;
    let repo = match repo_selector {
        Some(selector) => select_repo(&state, &selector)?.clone(),
        None => infer_repo_for_cwd(&state, &runtime)?,
    };
    let base = base_branch
        .or_else(|| repo.worktree_base_ref.clone())
        .unwrap_or_else(|| current_git_branch(&repo.path).unwrap_or_else(|| "HEAD".into()));
    let configured_root = state
        .settings
        .ui
        .repo_worktree_base_paths
        .get(&repo.id.0)
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty());
    let workspace_root = configured_root.map_or_else(
        || state.settings.workspace_root.clone(),
        |value| {
            let path = PathBuf::from(value);
            if path.is_absolute() {
                path
            } else {
                repo.path.join(path)
            }
        },
    );
    let nest = configured_root
        .map(|_| false)
        .unwrap_or(state.settings.ui.nest_workspaces);
    let created = runtime.block_on(crate::git_tasks::create_worktree_with_layout_now(
        repo.clone(),
        name,
        base,
        workspace_root,
        nest,
        None,
        state.settings.ui.refresh_local_base_ref,
    ))?;
    let persisted = Worktree {
        id: WorktreeId(created.path.to_string_lossy().into_owned()),
        repo_id: repo.id.clone(),
        path: created.path.clone(),
        branch: created.branch.clone(),
        display_name: created.display_name.clone(),
        created_with_agent: agent,
        created_at_unix_ms: unix_ms(),
        linked_github_pr: None,
        linked_linear_issue: None,
        linked_linear_issue_workspace_id: None,
        linked_linear_issue_organization_url_key: None,
        linked_jira_issue: None,
        linked_jira_site: None,
    };
    state.worktrees.retain(|item| item.id != persisted.id);
    state.worktrees.push(persisted);
    store
        .save(&state)
        .map_err(|error| format!("Could not save the worktree: {error}"))?;
    output(
        json!({
            "repoId": repo.id.0,
            "path": created.path,
            "branch": created.branch,
            "displayName": created.display_name,
        }),
        json_output,
        format!("Created {} at {}", created.branch, created.path.display()),
    );
    Ok(0)
}

fn remove_worktree(args: &[String], json_output: bool) -> Result<i32, String> {
    let selector = option_value(args, "--worktree")?
        .ok_or_else(|| "worktree rm requires --worktree <name|path>".to_string())?;
    let force = args.iter().any(|arg| arg == "--force");
    if let Some(result) = crate::local_rpc::call(
        "worktree.remove",
        json!({"worktree": &selector, "force": force}),
    )? {
        output(
            result.clone(),
            json_output,
            format!(
                "Removed {}",
                result["removed"].as_str().unwrap_or(&selector)
            ),
        );
        return Ok(0);
    }
    let mut store = data_store();
    let mut state = store.load().state;
    let runtime = runtime()?;
    let (repo, entry) = find_worktree(&state, &runtime, &selector)?;
    if entry.is_main {
        return Err("The primary repository checkout cannot be removed.".into());
    }
    let linked_paths = state
        .settings
        .ui
        .repo_symlink_paths
        .get(&repo.id.0)
        .cloned()
        .unwrap_or_default();
    runtime.block_on(crate::git_tasks::remove_worktree_now(
        repo,
        entry.path.clone(),
        force,
        entry.branch.clone(),
        linked_paths,
    ))?;
    state
        .worktrees
        .retain(|worktree| worktree.path != entry.path);
    store
        .save(&state)
        .map_err(|error| format!("Could not save worktree removal: {error}"))?;
    output(
        json!({"removed": entry.path, "branch": entry.branch}),
        json_output,
        format!("Removed {}", entry.path.display()),
    );
    Ok(0)
}

fn infer_repo_for_cwd(
    state: &PersistedState,
    runtime: &tokio::runtime::Runtime,
) -> Result<Repo, String> {
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    for repo in &state.repos {
        let entries = runtime.block_on(crate::git_tasks::list_worktrees_now(repo.clone()))?;
        if entries.iter().any(|entry| cwd.starts_with(&entry.path)) {
            return Ok(repo.clone());
        }
    }
    Err("Use --repo because the current directory is not in a registered worktree.".into())
}

fn find_worktree(
    state: &PersistedState,
    runtime: &tokio::runtime::Runtime,
    selector: &str,
) -> Result<(Repo, suaegi_git::worktree::WorktreeEntry), String> {
    for repo in &state.repos {
        let entries = runtime.block_on(crate::git_tasks::list_worktrees_now(repo.clone()))?;
        if let Some(entry) = entries.into_iter().find(|entry| {
            entry.path == Path::new(selector)
                || entry
                    .path
                    .file_name()
                    .is_some_and(|name| name == OsStr::new(selector))
                || entry.branch.as_deref() == Some(selector)
        }) {
            return Ok((repo.clone(), entry));
        }
    }
    Err(format!("Worktree not found: {selector}"))
}

fn current_git_branch(path: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["-C", path.to_str()?, "branch", "--show-current"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|branch| !branch.is_empty())
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn emulator_command(
    command: &str,
    positional: &[&str],
    args: &[String],
    json_output: bool,
) -> Result<i32, String> {
    let state = load_state();
    let sdk = state.settings.ui.android_sdk_path;
    let runtime = runtime()?;
    let availability = runtime.block_on(crate::emulator::inspect(sdk.clone()));
    if matches!(command, "devices" | "list") {
        let devices = availability
            .devices
            .iter()
            .map(emulator_device_json)
            .collect::<Vec<_>>();
        let plain = if availability.devices.is_empty() {
            availability.message
        } else {
            availability
                .devices
                .iter()
                .map(|device| {
                    format!(
                        "{}\t{}\t{}\t{}",
                        emulator_platform_name(&device.platform),
                        device.id,
                        device.state,
                        device.name
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        output(json!({"devices": devices}), json_output, plain);
        return Ok(0);
    }

    let selector = option_value(args, "--device")?
        .or_else(|| option_value(args, "--emulator").ok().flatten())
        .or_else(|| {
            (command == "attach")
                .then(|| positional.first().map(|value| (*value).to_string()))
                .flatten()
        });
    let device = select_emulator_device(&availability.devices, selector.as_deref())?.clone();
    let success = |result: Value, plain: String| {
        output(
            json!({"ok": true, "device": emulator_device_json(&device), "result": result}),
            json_output,
            plain,
        );
        Ok(0)
    };

    match (command, positional) {
        ("attach", _) => {
            runtime.block_on(crate::emulator::launch(device.clone(), sdk))?;
            success(
                json!({"attached": true}),
                format!("Attached {}", device.name),
            )
        }
        ("tap", [x, y]) => {
            let x = parse_coordinate(x)?;
            let y = parse_coordinate(y)?;
            runtime.block_on(crate::emulator::tap(device.clone(), x, y, sdk))?;
            success(json!({"x": x, "y": y}), "Tap sent".into())
        }
        ("type", [text]) => {
            runtime.block_on(crate::emulator::type_text(
                device.clone(),
                (*text).to_string(),
                sdk,
            ))?;
            success(json!({"typed": text}), "Text sent".into())
        }
        ("gesture", [points]) => {
            let points = parse_gesture_points(points)?;
            runtime.block_on(crate::emulator::gesture(
                device.clone(),
                points.clone(),
                sdk,
            ))?;
            success(json!({"points": points}), "Gesture sent".into())
        }
        ("button", [name]) => {
            runtime.block_on(crate::emulator::button(
                device.clone(),
                (*name).to_string(),
                sdk,
            ))?;
            success(json!({"button": name}), "Button sent".into())
        }
        ("rotate", [orientation]) => {
            runtime.block_on(crate::emulator::rotate(
                device.clone(),
                (*orientation).to_string(),
                sdk,
            ))?;
            success(json!({"orientation": orientation}), "Rotation sent".into())
        }
        ("exec", []) => {
            let command = option_value(args, "--command")?
                .ok_or_else(|| "emulator exec requires --command".to_string())?;
            let result =
                runtime.block_on(crate::emulator::raw_exec(device.clone(), command, sdk))?;
            success(json!({"output": result}), result)
        }
        ("kill", []) => {
            runtime.block_on(crate::emulator::stop_helper(device.clone(), sdk))?;
            success(json!({"stopped": true}), "Emulator helper stopped".into())
        }
        ("shutdown", []) => {
            runtime.block_on(crate::emulator::shutdown(device.clone(), sdk))?;
            success(json!({"shutdown": true}), "Emulator shut down".into())
        }
        ("install", [path]) => {
            runtime.block_on(crate::emulator::install_android(
                device.clone(),
                PathBuf::from(path),
                args.iter().any(|arg| arg == "--reinstall"),
                sdk,
            ))?;
            success(json!({"installed": path}), "APK installed".into())
        }
        ("launch", [package]) => {
            let activity = option_value(args, "--activity")?;
            runtime.block_on(crate::emulator::launch_android_app(
                device.clone(),
                (*package).to_string(),
                activity.clone(),
                sdk,
            ))?;
            success(
                json!({"package": package, "activity": activity}),
                "Android app launched".into(),
            )
        }
        ("permissions", [operation]) if *operation == "reset" => {
            runtime.block_on(crate::emulator::android_permission(
                device.clone(),
                (*operation).to_string(),
                String::new(),
                None,
                sdk,
            ))?;
            success(json!({"operation": operation}), "Permissions reset".into())
        }
        ("permissions", [operation, package, permission]) => {
            runtime.block_on(crate::emulator::android_permission(
                device.clone(),
                (*operation).to_string(),
                (*package).to_string(),
                Some((*permission).to_string()),
                sdk,
            ))?;
            success(
                json!({"operation": operation, "package": package, "permission": permission}),
                "Permission updated".into(),
            )
        }
        ("ax", []) => {
            let tree =
                runtime.block_on(crate::emulator::accessibility_tree(device.clone(), sdk))?;
            success(json!({"tree": tree}), tree)
        }
        ("logcat", []) => {
            let lines = option_value(args, "--lines")?
                .map(|value| {
                    value
                        .parse::<usize>()
                        .map_err(|_| "--lines must be a positive integer".to_string())
                })
                .transpose()?;
            let logs = runtime.block_on(crate::emulator::logcat(device.clone(), lines, sdk))?;
            success(json!({"logs": logs}), logs)
        }
        _ => Err(format!(
            "Unknown or incomplete emulator command: {command} {}",
            positional.join(" ")
        )),
    }
}

fn select_emulator_device<'a>(
    devices: &'a [crate::emulator::EmulatorDevice],
    selector: Option<&str>,
) -> Result<&'a crate::emulator::EmulatorDevice, String> {
    match selector {
        Some(selector) => devices
            .iter()
            .find(|device| device.id == selector || device.name == selector)
            .ok_or_else(|| format!("Emulator device not found: {selector}")),
        None => crate::emulator::pick_default(devices)
            .ok_or_else(|| "No emulator devices are available.".to_string()),
    }
}

fn emulator_platform_name(platform: &crate::emulator::EmulatorPlatform) -> &'static str {
    match platform {
        crate::emulator::EmulatorPlatform::Ios => "ios",
        crate::emulator::EmulatorPlatform::Android => "android",
    }
}

fn emulator_device_json(device: &crate::emulator::EmulatorDevice) -> Value {
    json!({
        "platform": emulator_platform_name(&device.platform),
        "id": device.id,
        "name": device.name,
        "state": device.state,
        "runtime": device.runtime,
        "available": device.available,
    })
}

fn parse_coordinate(value: &str) -> Result<f32, String> {
    value
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
        .ok_or_else(|| "Coordinates must be numbers from 0 to 1.".to_string())
}

fn parse_gesture_points(value: &str) -> Result<Vec<(f32, f32)>, String> {
    let value: Value = serde_json::from_str(value)
        .map_err(|_| "Gesture points must be valid JSON.".to_string())?;
    let points = value
        .as_array()
        .ok_or_else(|| "Gesture points must be a JSON array.".to_string())?;
    points
        .iter()
        .map(|point| {
            if let Some(values) = point.as_array() {
                let x = values.first().and_then(Value::as_f64).unwrap_or(f64::NAN) as f32;
                let y = values.get(1).and_then(Value::as_f64).unwrap_or(f64::NAN) as f32;
                return Ok((
                    parse_coordinate(&x.to_string())?,
                    parse_coordinate(&y.to_string())?,
                ));
            }
            let x = point.get("x").and_then(Value::as_f64).unwrap_or(f64::NAN) as f32;
            let y = point.get("y").and_then(Value::as_f64).unwrap_or(f64::NAN) as f32;
            Ok((
                parse_coordinate(&x.to_string())?,
                parse_coordinate(&y.to_string())?,
            ))
        })
        .collect()
}

fn terminal_command(command: &str, args: &[String], json_output: bool) -> Result<i32, String> {
    let live_request = match command {
        "list" => Some((
            "terminal.list",
            json!({
                "worktree": option_value(args, "--worktree")?,
                "limit": option_value(args, "--limit")?.and_then(|value| value.parse::<u64>().ok()),
            }),
        )),
        "show" => Some((
            "terminal.show",
            json!({"terminal": option_value(args, "--terminal")?}),
        )),
        "read" => Some((
            "terminal.read",
            json!({
                "terminal": option_value(args, "--terminal")?,
                "cursor": option_value(args, "--cursor")?.map(|value| value.parse::<u64>())
                    .transpose().map_err(|_| "--cursor must be a non-negative integer.")?,
                "limit": option_value(args, "--limit")?.map(|value| value.parse::<u64>())
                    .transpose().map_err(|_| "--limit must be a positive integer.")?,
            }),
        )),
        "send" => Some((
            "terminal.send",
            json!({
                "terminal": option_value(args, "--terminal")?,
                "text": option_value(args, "--text")?,
                "enter": args.iter().any(|arg| arg == "--enter"),
                "interrupt": args.iter().any(|arg| arg == "--interrupt"),
            }),
        )),
        "wait" => Some((
            "terminal.wait",
            json!({
                "terminal": option_value(args, "--terminal")?,
                "for": required_option(args, "--for", "terminal wait")?,
                "timeoutMs": option_value(args, "--timeout-ms")?
                    .map(|value| value.parse::<u64>())
                    .transpose().map_err(|_| "--timeout-ms must be a positive integer.")?,
            }),
        )),
        "stop" => Some((
            "terminal.stop",
            json!({"worktree": required_option(args, "--worktree", "terminal stop")?}),
        )),
        "create" => Some((
            "terminal.create",
            json!({
                "worktree": option_value(args, "--worktree")?,
                "command": option_value(args, "--command")?,
                "title": option_value(args, "--title")?,
                "focus": args.iter().any(|arg| arg == "--focus"),
            }),
        )),
        "switch" | "focus" => Some((
            "terminal.focus",
            json!({"terminal": option_value(args, "--terminal")?}),
        )),
        "close" => Some((
            if args.iter().any(|arg| arg == "--tab") {
                "terminal.closeTab"
            } else {
                "terminal.close"
            },
            json!({"terminal": option_value(args, "--terminal")?}),
        )),
        "rename" => Some((
            "terminal.rename",
            json!({
                "terminal": option_value(args, "--terminal")?,
                "title": option_value(args, "--title")?,
            }),
        )),
        "split" => Some((
            "terminal.split",
            json!({
                "terminal": option_value(args, "--terminal")?,
                "direction": option_value(args, "--direction")?,
                "command": option_value(args, "--command")?,
            }),
        )),
        _ => None,
    };
    if let Some((method, params)) = live_request {
        if let Some(result) = crate::local_rpc::call(method, params)? {
            let plain = match command {
                "list" => result["terminals"]
                    .as_array()
                    .map(|terminals| {
                        terminals
                            .iter()
                            .map(|terminal| {
                                format!(
                                    "{}\t{}\t{}",
                                    terminal["handle"].as_str().unwrap_or_default(),
                                    terminal["title"].as_str().unwrap_or_default(),
                                    if terminal["running"].as_bool() == Some(true) {
                                        "running"
                                    } else {
                                        "exited"
                                    }
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| "No live terminals".into()),
                "read" => result["terminal"]["output"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                _ => format!("Terminal {command} completed"),
            };
            output(result, json_output, plain);
            return Ok(0);
        }
        if matches!(command, "switch" | "focus" | "rename" | "split") {
            return Err(format!(
                "terminal {command} requires the Suaegi desktop app to be running."
            ));
        }
    }
    match command {
        "list" => {
            let mut sessions =
                suaegi_term::daemon::list_sessions().map_err(|error| error.to_string())?;
            if let Some(worktree) = option_value(args, "--worktree")? {
                let path = resolve_worktree_path(&worktree)?;
                let id = daemon_worktree_id(&path);
                sessions.retain(|session| session.session_id == id);
            }
            let values = sessions
                .iter()
                .map(terminal_session_json)
                .collect::<Vec<_>>();
            let plain = if sessions.is_empty() {
                "No live terminals".into()
            } else {
                sessions
                    .iter()
                    .map(|session| {
                        format!(
                            "{}\t{}x{}\t{}",
                            session.session_id,
                            session.cols,
                            session.rows,
                            if session.running { "running" } else { "exited" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            output(json!({"terminals": values}), json_output, plain);
            Ok(0)
        }
        "show" => {
            let id = resolve_terminal_id(args)?;
            let session = daemon_session_info(&id)?;
            output(
                terminal_session_json(&session),
                json_output,
                format!(
                    "{}\n  size: {}×{}\n  status: {}",
                    session.session_id,
                    session.cols,
                    session.rows,
                    if session.running { "running" } else { "exited" }
                ),
            );
            Ok(0)
        }
        "read" => {
            let id = resolve_terminal_id(args)?;
            let info = daemon_session_info(&id)?;
            let output_text = read_daemon_output(&id, option_value(args, "--limit")?)?;
            output(
                json!({
                    "terminal": id,
                    "output": output_text,
                    "nextCursor": info.next_sequence,
                    "running": info.running,
                    "exitCode": info.exit_code,
                }),
                json_output,
                output_text,
            );
            Ok(0)
        }
        "send" => {
            let id = resolve_terminal_id(args)?;
            let (session, reader, is_new) = attach_daemon_session(&id)?;
            drop(reader);
            if is_new {
                let _ = session.kill();
                return Err(format!("Terminal no longer exists: {id}"));
            }
            let mut bytes = Vec::new();
            if args.iter().any(|arg| arg == "--interrupt") {
                bytes.push(0x03);
            }
            if let Some(text) = option_value(args, "--text")? {
                bytes.extend_from_slice(text.as_bytes());
            }
            if args.iter().any(|arg| arg == "--enter") {
                bytes.push(b'\r');
            }
            if bytes.is_empty() {
                return Err("terminal send requires --text, --enter, or --interrupt".into());
            }
            session.write(&bytes).map_err(|error| error.to_string())?;
            session.disconnect();
            output(
                json!({"terminal": id, "bytesWritten": bytes.len()}),
                json_output,
                format!("Sent {} bytes", bytes.len()),
            );
            Ok(0)
        }
        "wait" => {
            let id = resolve_terminal_id(args)?;
            let condition = option_value(args, "--for")?
                .ok_or_else(|| "terminal wait requires --for exit".to_string())?;
            if condition != "exit" {
                return Err("This standalone CLI currently supports --for exit.".into());
            }
            let timeout = option_value(args, "--timeout-ms")?
                .map(|value| {
                    value
                        .parse::<u64>()
                        .map_err(|_| "--timeout-ms must be an integer".to_string())
                })
                .transpose()?
                .unwrap_or(30_000)
                .clamp(1, 3_600_000);
            let started = std::time::Instant::now();
            loop {
                let session = suaegi_term::daemon::list_sessions()
                    .map_err(|error| error.to_string())?
                    .into_iter()
                    .find(|session| session.session_id == id);
                if session.as_ref().is_none_or(|session| !session.running) {
                    let code = session.and_then(|session| session.exit_code);
                    output(
                        json!({"terminal": id, "condition": "exit", "exitCode": code}),
                        json_output,
                        format!(
                            "Terminal exited{}",
                            code.map_or(String::new(), |c| format!(" ({c})"))
                        ),
                    );
                    return Ok(0);
                }
                if started.elapsed() >= std::time::Duration::from_millis(timeout) {
                    return Err(format!("Timed out waiting for terminal {id} to exit."));
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
        "stop" => {
            let worktree = option_value(args, "--worktree")?
                .ok_or_else(|| "terminal stop requires --worktree".to_string())?;
            let id = daemon_worktree_id(&resolve_worktree_path(&worktree)?);
            suaegi_term::daemon::kill_session(&id).map_err(|error| error.to_string())?;
            output(
                json!({"terminal": id, "stopped": true}),
                json_output,
                "Terminal stopped".into(),
            );
            Ok(0)
        }
        "create" => {
            let worktree = match option_value(args, "--worktree")? {
                Some(selector) => resolve_worktree_path(&selector)?,
                None => resolve_worktree_path("current")?,
            };
            let command = option_value(args, "--command")?;
            let id = format!("cli:{}", unix_ms());
            let (program, spawn_args) = match command {
                Some(command) => ("/bin/zsh".to_string(), vec!["-lc".into(), command]),
                None => ("/bin/zsh".to_string(), vec!["-l".into()]),
            };
            let spec = suaegi_term::daemon::SpawnSpec {
                program,
                args: spawn_args,
                cwd: Some(worktree),
                env: std::env::vars().collect(),
                rows: 50,
                cols: 80,
            };
            let (session, reader, _) =
                suaegi_term::daemon::DaemonClientSession::create_or_attach(id.clone(), spec)
                    .map_err(|error| error.to_string())?;
            drop(reader);
            session.disconnect();
            output(
                json!({"terminal": id, "created": true}),
                json_output,
                format!("Created terminal {id}"),
            );
            Ok(0)
        }
        "close" => {
            let id = option_value(args, "--terminal")?
                .ok_or_else(|| "terminal close requires --terminal".to_string())?;
            suaegi_term::daemon::kill_session(&id).map_err(|error| error.to_string())?;
            output(
                json!({"terminal": id, "closed": true}),
                json_output,
                "Terminal closed".into(),
            );
            Ok(0)
        }
        _ => Err(format!("Unknown terminal command: {command}")),
    }
}

fn terminal_session_json(session: &suaegi_term::daemon::SessionInfo) -> Value {
    json!({
        "handle": session.session_id,
        "running": session.running,
        "exitCode": session.exit_code,
        "rows": session.rows,
        "cols": session.cols,
        "nextCursor": session.next_sequence,
    })
}

pub(crate) fn daemon_session_info(id: &str) -> Result<suaegi_term::daemon::SessionInfo, String> {
    suaegi_term::daemon::list_sessions()
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|session| session.session_id == id)
        .ok_or_else(|| format!("Terminal not found: {id}"))
}

fn resolve_terminal_id(args: &[String]) -> Result<String, String> {
    if let Some(id) = option_value(args, "--terminal")? {
        daemon_session_info(&id)?;
        return Ok(id);
    }
    Ok(daemon_worktree_id(&resolve_worktree_path("current")?))
}

fn daemon_worktree_id(path: &Path) -> String {
    format!("worktree:{}", path.to_string_lossy())
}

fn resolve_worktree_path(selector: &str) -> Result<PathBuf, String> {
    let state = load_state();
    let runtime = runtime()?;
    if selector == "current" || selector == "active" {
        let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
        for repo in &state.repos {
            let entries = runtime.block_on(crate::git_tasks::list_worktrees_now(repo.clone()))?;
            if let Some(entry) = entries
                .into_iter()
                .filter(|entry| cwd.starts_with(&entry.path))
                .max_by_key(|entry| entry.path.components().count())
            {
                return Ok(entry.path);
            }
        }
        return Err("The current directory is not in a registered worktree.".into());
    }
    find_worktree(&state, &runtime, selector).map(|(_, entry)| entry.path)
}

pub(crate) fn attach_daemon_session(
    id: &str,
) -> Result<
    (
        std::sync::Arc<suaegi_term::daemon::DaemonClientSession>,
        suaegi_term::daemon::DaemonReader,
        bool,
    ),
    String,
> {
    let cwd = id.strip_prefix("worktree:").map(PathBuf::from);
    let spec = suaegi_term::daemon::SpawnSpec {
        program: "/bin/zsh".into(),
        args: vec!["-l".into()],
        cwd,
        env: std::env::vars().collect(),
        rows: 50,
        cols: 80,
    };
    suaegi_term::daemon::DaemonClientSession::create_or_attach(id.to_string(), spec)
        .map_err(|error| error.to_string())
}

pub(crate) fn read_daemon_output(id: &str, limit: Option<String>) -> Result<String, String> {
    let max_lines = limit
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| "--limit must be an integer".to_string())
        })
        .transpose()?
        .unwrap_or(1_000)
        .clamp(1, 100_000);
    let (session, mut reader, is_new) = attach_daemon_session(id)?;
    if is_new {
        let _ = session.kill();
        return Err(format!("Terminal no longer exists: {id}"));
    }
    let (sender, receiver) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        let mut collected = Vec::new();
        let mut buffer = [0_u8; 8192];
        while collected.len() < 16 * 1024 * 1024 {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    collected.extend_from_slice(&buffer[..read]);
                    let _ = sender.send(collected.clone());
                }
            }
        }
    });
    let mut bytes = Vec::new();
    while let Ok(next) = receiver.recv_timeout(std::time::Duration::from_millis(120)) {
        bytes = next;
    }
    session.disconnect();
    let _ = thread.join();
    let text = String::from_utf8_lossy(&bytes);
    let lines = text.lines().collect::<Vec<_>>();
    Ok(lines[lines.len().saturating_sub(max_lines)..].join("\n"))
}

fn local_rpc_required(method: &str, params: Value) -> Result<Value, String> {
    crate::local_rpc::call(method, params)?
        .ok_or_else(|| "Suaegi is not running. Start it with `suaegi open` and retry.".to_string())
}

fn browser_exec(args: &[String], json_output: bool) -> Result<i32, String> {
    let command = required_option(args, "--command", "exec")?;
    let mut parsed = split_command_line(&command)?;
    if parsed.is_empty() {
        return Err("exec --command cannot be empty.".into());
    }
    if parsed.first().is_some_and(|command| command == "exec") {
        return Err("Nested browser exec commands are not allowed.".into());
    }
    if json_output && !parsed.iter().any(|argument| argument == "--json") {
        parsed.push("--json".into());
    }
    run(parsed.into_iter().map(OsString::from).collect())
}

fn split_command_line(input: &str) -> Result<Vec<String>, String> {
    let mut values = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut started = false;
    for character in input.chars() {
        if escaped {
            current.push(character);
            escaped = false;
            started = true;
            continue;
        }
        if character == '\\' && quote != Some('\'') {
            escaped = true;
            started = true;
            continue;
        }
        if let Some(active) = quote {
            if character == active {
                quote = None;
            } else {
                current.push(character);
            }
            started = true;
            continue;
        }
        if matches!(character, '\'' | '"') {
            quote = Some(character);
            started = true;
        } else if character.is_whitespace() {
            if started {
                values.push(std::mem::take(&mut current));
                started = false;
            }
        } else {
            current.push(character);
            started = true;
        }
    }
    if escaped {
        return Err("Browser exec command ends with an incomplete escape.".into());
    }
    if quote.is_some() {
        return Err("Browser exec command has an unclosed quote.".into());
    }
    if started {
        values.push(current);
    }
    Ok(values)
}

fn browser_goto(args: &[String], json_output: bool) -> Result<i32, String> {
    let url =
        option_value(args, "--url")?.ok_or_else(|| "goto requires --url <url>".to_string())?;
    let result = local_rpc_required("browser.goto", json!({"url": url}))?;
    output(result, json_output, format!("Opened {url}"));
    Ok(0)
}

fn browser_interact_command(
    command: &str,
    args: &[String],
    json_output: bool,
) -> Result<i32, String> {
    let mut params = serde_json::Map::new();
    match command {
        "snapshot" => {}
        "type" => {
            let input = required_option(args, "--input", "type")?;
            params.insert("value".into(), Value::String(input));
        }
        "inserttext" => {
            let text = required_option(args, "--text", "inserttext")?;
            params.insert("value".into(), Value::String(text));
        }
        "scroll" => {
            let direction = option_value(args, "--direction")?.unwrap_or_else(|| "down".into());
            if !matches!(direction.as_str(), "up" | "down" | "left" | "right") {
                return Err("--direction must be up, down, left, or right.".into());
            }
            let amount = option_value(args, "--amount")?
                .map(|value| {
                    value
                        .parse::<f64>()
                        .ok()
                        .filter(|value| value.is_finite() && *value >= 0.0)
                        .ok_or_else(|| "--amount must be a non-negative number.".to_string())
                })
                .transpose()?
                .unwrap_or(700.0);
            params.insert("direction".into(), Value::String(direction));
            params.insert(
                "amount".into(),
                serde_json::Number::from_f64(amount)
                    .map(Value::Number)
                    .ok_or_else(|| "--amount is outside the supported range.".to_string())?,
            );
        }
        "eval" => {
            let expression = required_option(args, "--expression", "eval")?;
            params.insert("expression".into(), Value::String(expression));
        }
        "keypress" => {
            let key = required_option(args, "--key", "keypress")?;
            params.insert("key".into(), Value::String(key));
        }
        "drag" => {
            params.insert(
                "from".into(),
                Value::String(required_option(args, "--from", "drag")?),
            );
            params.insert(
                "to".into(),
                Value::String(required_option(args, "--to", "drag")?),
            );
        }
        "get" => {
            params.insert(
                "what".into(),
                Value::String(required_option(args, "--what", "get")?),
            );
            if let Some(element) = option_value(args, "--element")? {
                params.insert("element".into(), Value::String(element));
            }
        }
        "is" => {
            params.insert(
                "what".into(),
                Value::String(required_option(args, "--what", "is")?),
            );
            params.insert(
                "element".into(),
                Value::String(required_option(args, "--element", "is")?),
            );
        }
        "find" => {
            for (flag, key) in [
                ("--locator", "locator"),
                ("--value", "value"),
                ("--action", "action"),
            ] {
                params.insert(
                    key.into(),
                    Value::String(required_option(args, flag, "find")?),
                );
            }
            if let Some(text) = option_value(args, "--text")? {
                params.insert("text".into(), Value::String(text));
            }
        }
        "highlight" => {
            params.insert(
                "element".into(),
                Value::String(required_option(args, "--selector", "highlight")?),
            );
        }
        _ => {
            let element = required_option(args, "--element", command)?;
            params.insert("element".into(), Value::String(element));
            if matches!(command, "fill" | "select") {
                let value = required_option(args, "--value", command)?;
                params.insert("value".into(), Value::String(value));
            }
        }
    }
    let rpc_command = match command {
        "scrollintoview" => "scroll-into-view",
        "inserttext" => "insert-text",
        other => other,
    };
    let result = local_rpc_required(&format!("browser.{rpc_command}"), Value::Object(params))?;
    let plain = if command == "snapshot" {
        let header = format!(
            "{}\n{}",
            result["title"].as_str().unwrap_or("Browser"),
            result["url"].as_str().unwrap_or_default()
        );
        let elements = result["elements"]
            .as_array()
            .map(|elements| {
                elements
                    .iter()
                    .map(|element| {
                        format!(
                            "{}\t{}\t{}",
                            element["ref"].as_str().unwrap_or_default(),
                            element["tag"].as_str().unwrap_or_default(),
                            element["label"].as_str().unwrap_or_default()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        format!("{header}\n{elements}")
    } else {
        format!("Browser {command} completed")
    };
    output(result, json_output, plain);
    Ok(0)
}

fn browser_upload(args: &[String], json_output: bool) -> Result<i32, String> {
    let element = required_option(args, "--element", "upload")?;
    let files = required_option(args, "--files", "upload")?
        .split(',')
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(resolve_cli_path)
        .collect::<Result<Vec<_>, _>>()?;
    if files.is_empty() {
        return Err("upload requires at least one file path.".into());
    }
    let result = local_rpc_required(
        "browser.upload",
        json!({"element": element, "files": files}),
    )?;
    output(
        result.clone(),
        json_output,
        format!(
            "Uploaded {} file(s)",
            result["uploaded"].as_u64().unwrap_or_default()
        ),
    );
    Ok(0)
}

fn browser_dialog_command(
    command: &str,
    args: &[String],
    json_output: bool,
) -> Result<i32, String> {
    let mut params = serde_json::Map::new();
    if command == "accept" {
        if let Some(text) = option_value(args, "--text")? {
            params.insert("text".into(), Value::String(text));
        }
    } else if args.iter().any(|argument| argument == "--text") {
        return Err("dialog dismiss does not accept --text.".into());
    }
    let result = local_rpc_required(&format!("browser.dialog-{command}"), Value::Object(params))?;
    output(
        result,
        json_output,
        format!("Handled browser dialog {command}"),
    );
    Ok(0)
}

fn browser_download(args: &[String], json_output: bool) -> Result<i32, String> {
    let element = required_option(args, "--selector", "download")?;
    let path = resolve_cli_path(&required_option(args, "--path", "download")?)?;
    let result = local_rpc_required(
        "browser.download",
        json!({"element": element, "path": path}),
    )?;
    output(result, json_output, format!("Downloaded to {path}"));
    Ok(0)
}

fn browser_capture(command: &str, args: &[String], json_output: bool) -> Result<i32, String> {
    let params = if matches!(command, "screenshot" | "full-screenshot") {
        let format = option_value(args, "--format")?.unwrap_or_else(|| "png".into());
        if !matches!(format.as_str(), "png" | "jpeg") {
            return Err("--format must be png or jpeg.".into());
        }
        json!({"format": format})
    } else {
        Value::Null
    };
    let result = local_rpc_required(&format!("browser.{command}"), params)?;
    let bytes = result["data"]
        .as_str()
        .map(|data| data.len().saturating_mul(3) / 4)
        .unwrap_or_default();
    output(
        result,
        json_output,
        format!("Browser {command} captured ({bytes} bytes)"),
    );
    Ok(0)
}

fn browser_wait(args: &[String], json_output: bool) -> Result<i32, String> {
    let selector = option_value(args, "--selector")?;
    let text = option_value(args, "--text")?;
    let url = option_value(args, "--url")?;
    let load = option_value(args, "--load")?;
    let function = option_value(args, "--fn")?;
    let state = option_value(args, "--state")?.unwrap_or_else(|| "visible".into());
    let timeout = option_value(args, "--timeout")?
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| "--timeout must be a positive integer in milliseconds.".to_string())
        })
        .transpose()?
        .unwrap_or(30_000)
        .clamp(1, 3_600_000);
    let conditions = [
        selector.is_some(),
        text.is_some(),
        url.is_some(),
        load.is_some(),
        function.is_some(),
    ]
    .into_iter()
    .filter(|present| *present)
    .count();
    if conditions > 1 {
        return Err("wait accepts only one of --selector, --text, --url, --load, or --fn.".into());
    }
    if conditions == 0 {
        std::thread::sleep(std::time::Duration::from_millis(timeout));
        output(
            json!({"waited": true, "timeout": timeout}),
            json_output,
            format!("Waited {timeout} ms"),
        );
        return Ok(0);
    }

    let expression = if let Some(selector) = selector.as_ref() {
        let selector_json = serde_json::to_string(selector).map_err(|error| error.to_string())?;
        let state_json = serde_json::to_string(&state).map_err(|error| error.to_string())?;
        format!(
            r#"(() => {{
const selector = {selector_json};
const state = {state_json};
const match = /^@e(\d+)$/.exec(selector);
const el = match ? window.__suaegiElements?.[Number(match[1])-1] : document.querySelector(selector);
if (state === "hidden" || state === "detached") return !el || !el.isConnected;
if (!el || !el.isConnected) return false;
if (state === "attached") return true;
const style = getComputedStyle(el), rect = el.getBoundingClientRect();
return style.visibility !== "hidden" && style.display !== "none" && rect.width > 0 && rect.height > 0;
}})()"#
        )
    } else if let Some(text) = text.as_ref() {
        format!(
            "(document.body?.innerText || '').includes({})",
            serde_json::to_string(text).map_err(|error| error.to_string())?
        )
    } else if let Some(url) = url.as_ref() {
        format!(
            "location.href.includes({})",
            serde_json::to_string(url).map_err(|error| error.to_string())?
        )
    } else if let Some(load) = load.as_ref() {
        let expected = match load.as_str() {
            "load" | "complete" | "networkidle" => "complete",
            "domcontentloaded" | "interactive" => "interactive",
            other => return Err(format!("Unsupported load state: {other}")),
        };
        if expected == "interactive" {
            "document.readyState === 'interactive' || document.readyState === 'complete'".into()
        } else {
            "document.readyState === 'complete'".into()
        }
    } else {
        format!(
            "Boolean((0,eval)({}))",
            serde_json::to_string(function.as_deref().unwrap_or_default())
                .map_err(|error| error.to_string())?
        )
    };
    let started = std::time::Instant::now();
    loop {
        let result = local_rpc_required("browser.eval", json!({"expression": expression}))?;
        if result.as_bool() == Some(true) {
            output(
                json!({"waited": true, "elapsedMs": started.elapsed().as_millis()}),
                json_output,
                "Browser wait condition satisfied".into(),
            );
            return Ok(0);
        }
        if started.elapsed() >= std::time::Duration::from_millis(timeout) {
            return Err(format!(
                "Timed out after {timeout} ms waiting for the browser condition."
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

fn browser_cookie_command(
    command: &str,
    args: &[String],
    json_output: bool,
) -> Result<i32, String> {
    let mut params = serde_json::Map::new();
    match command {
        "get" => {
            if let Some(url) = option_value(args, "--url")? {
                params.insert("url".into(), Value::String(url));
            }
        }
        "set" => {
            for (flag, key) in [("--name", "name"), ("--value", "value")] {
                params.insert(
                    key.into(),
                    Value::String(required_option(args, flag, "cookie set")?),
                );
            }
            for (flag, key) in [
                ("--domain", "domain"),
                ("--path", "path"),
                ("--sameSite", "sameSite"),
            ] {
                if let Some(value) = option_value(args, flag)? {
                    params.insert(key.into(), Value::String(value));
                }
            }
            for (flag, key) in [("--secure", "secure"), ("--httpOnly", "httpOnly")] {
                if args.iter().any(|arg| arg == flag) {
                    params.insert(key.into(), Value::Bool(true));
                }
            }
            if let Some(expires) = option_value(args, "--expires")? {
                let value = expires
                    .parse::<i64>()
                    .map_err(|_| "--expires must be a Unix timestamp.".to_string())?;
                params.insert("expires".into(), Value::Number(value.into()));
            }
        }
        "delete" => {
            params.insert(
                "name".into(),
                Value::String(required_option(args, "--name", "cookie delete")?),
            );
            for (flag, key) in [("--domain", "domain"), ("--url", "url")] {
                if let Some(value) = option_value(args, flag)? {
                    params.insert(key.into(), Value::String(value));
                }
            }
        }
        _ => return Err(format!("Unknown cookie command: {command}")),
    }
    let result = local_rpc_required(&format!("browser.cookie.{command}"), Value::Object(params))?;
    let plain = if command == "get" {
        result["cookies"]
            .as_array()
            .map(|cookies| {
                cookies
                    .iter()
                    .map(|cookie| {
                        format!(
                            "{}={} ({})",
                            cookie["name"].as_str().unwrap_or_default(),
                            cookie["value"].as_str().unwrap_or_default(),
                            cookie["domain"].as_str().unwrap_or_default()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "No cookies".into())
    } else {
        format!("Cookie {command} completed")
    };
    output(result, json_output, plain);
    Ok(0)
}

fn browser_storage_command(
    kind: &str,
    command: &str,
    args: &[String],
    json_output: bool,
) -> Result<i32, String> {
    if !matches!(kind, "local" | "session") {
        return Err("--storage kind must be local or session.".into());
    }
    if !matches!(command, "get" | "set" | "clear") {
        return Err(format!("Unknown storage command: {command}"));
    }
    let mut params = serde_json::Map::new();
    if matches!(command, "get" | "set") {
        params.insert(
            "key".into(),
            Value::String(required_option(args, "--key", "storage")?),
        );
    }
    if command == "set" {
        params.insert(
            "value".into(),
            Value::String(required_option(args, "--value", "storage")?),
        );
    }
    let result = local_rpc_required(
        &format!("browser.storage-{kind}-{command}"),
        Value::Object(params),
    )?;
    output(
        result,
        json_output,
        format!("{kind}Storage {command} completed"),
    );
    Ok(0)
}

fn browser_tab_command(command: &str, args: &[String], json_output: bool) -> Result<i32, String> {
    let mut params = serde_json::Map::new();
    match command {
        "list" | "current" => {}
        "show" => {
            params.insert(
                "page".into(),
                Value::String(required_option(args, "--page", "tab show")?),
            );
        }
        "create" => {
            if let Some(url) = option_value(args, "--url")? {
                params.insert("url".into(), Value::String(url));
            }
            if let Some(profile) = option_value(args, "--profile")? {
                params.insert("profile".into(), Value::String(profile));
            }
        }
        "switch" => {
            let page = option_value(args, "--page")?;
            let index = option_value(args, "--index")?;
            match (page, index) {
                (Some(page), None) => {
                    params.insert("page".into(), Value::String(page));
                }
                (None, Some(index)) => {
                    let index = index
                        .parse::<u64>()
                        .map_err(|_| "--index must be a non-negative integer.".to_string())?;
                    params.insert("index".into(), Value::Number(index.into()));
                }
                _ => return Err("tab switch requires exactly one of --page or --index.".into()),
            }
        }
        "close" => {
            if let Some(page) = option_value(args, "--page")? {
                params.insert("page".into(), Value::String(page));
            }
            if let Some(index) = option_value(args, "--index")? {
                let index = index
                    .parse::<u64>()
                    .map_err(|_| "--index must be a non-negative integer.".to_string())?;
                params.insert("index".into(), Value::Number(index.into()));
            }
        }
        _ => return Err(format!("Unknown tab command: {command}")),
    }
    let result = local_rpc_required(&format!("browser.tab.{command}"), Value::Object(params))?;
    let plain = if command == "list" {
        result["tabs"]
            .as_array()
            .map(|tabs| {
                tabs.iter()
                    .map(|tab| {
                        format!(
                            "{}[{}] {}  {} — {}",
                            if tab["active"].as_bool() == Some(true) {
                                "* "
                            } else {
                                "  "
                            },
                            tab["index"].as_u64().unwrap_or_default(),
                            tab["browserPageId"].as_str().unwrap_or_default(),
                            tab["title"].as_str().unwrap_or_default(),
                            tab["url"].as_str().unwrap_or_default(),
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "No browser tabs open.".into())
    } else {
        format!("Browser tab {command} completed")
    };
    output(result, json_output, plain);
    Ok(0)
}

fn browser_profile_command(
    command: &str,
    args: &[String],
    json_output: bool,
) -> Result<i32, String> {
    let mut params = serde_json::Map::new();
    match command {
        "list" => {}
        "create" => {
            params.insert(
                "label".into(),
                Value::String(required_option(args, "--label", "tab profile create")?),
            );
            if let Some(scope) = option_value(args, "--scope")? {
                if !matches!(scope.as_str(), "isolated" | "imported") {
                    return Err("--scope must be isolated or imported.".into());
                }
                params.insert("scope".into(), Value::String(scope));
            }
        }
        "delete" => {
            params.insert(
                "profile".into(),
                Value::String(required_option(args, "--profile", "tab profile delete")?),
            );
        }
        "set" | "clone" => {
            params.insert(
                "profile".into(),
                Value::String(required_option(
                    args,
                    "--profile",
                    &format!("tab profile {command}"),
                )?),
            );
            if let Some(page) = option_value(args, "--page")? {
                params.insert("page".into(), Value::String(page));
            }
        }
        "show" | "use-default" => {
            if let Some(page) = option_value(args, "--page")? {
                params.insert("page".into(), Value::String(page));
            }
        }
        _ => return Err(format!("Unknown browser profile command: {command}")),
    }
    let rpc = match command {
        "use-default" => "browser.profile.use-default".to_string(),
        other => format!("browser.profile.{other}"),
    };
    let result = local_rpc_required(&rpc, Value::Object(params))?;
    let plain = if command == "list" {
        result["profiles"]
            .as_array()
            .map(|profiles| {
                profiles
                    .iter()
                    .map(|profile| {
                        format!(
                            "{}\t{}\t{}",
                            profile["id"].as_str().unwrap_or_default(),
                            profile["label"].as_str().unwrap_or_default(),
                            profile["scope"].as_str().unwrap_or_default()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default()
    } else {
        format!("Browser profile {command} completed")
    };
    output(result, json_output, plain);
    Ok(0)
}

fn browser_advanced_command(
    command: &str,
    args: &[String],
    json_output: bool,
) -> Result<i32, String> {
    let number = |flag: &str, required: bool| -> Result<Option<f64>, String> {
        let value = option_value(args, flag)?;
        if required && value.is_none() {
            return Err(format!("{command} requires {flag} <number>"));
        }
        value
            .map(|value| {
                value
                    .parse::<f64>()
                    .ok()
                    .filter(|value| value.is_finite())
                    .ok_or_else(|| format!("{flag} must be a finite number."))
            })
            .transpose()
    };
    let mut params = serde_json::Map::new();
    match command {
        "viewport" => {
            for (key, flag) in [("width", "--width"), ("height", "--height")] {
                let value = number(flag, true)?.unwrap_or_default();
                if value <= 0.0 {
                    return Err(format!("{flag} must be a positive number."));
                }
                params.insert(
                    key.into(),
                    Value::Number(
                        serde_json::Number::from_f64(value)
                            .ok_or_else(|| format!("Invalid {flag} value."))?,
                    ),
                );
            }
            if let Some(scale) = number("--scale", false)? {
                if scale <= 0.0 {
                    return Err("--scale must be a positive number.".into());
                }
                params.insert(
                    "deviceScaleFactor".into(),
                    Value::Number(
                        serde_json::Number::from_f64(scale)
                            .ok_or_else(|| "Invalid --scale value.".to_string())?,
                    ),
                );
            }
            params.insert(
                "mobile".into(),
                Value::Bool(args.iter().any(|arg| arg == "--mobile")),
            );
        }
        "mouse-move" => {
            for (key, value) in [("x", number("--x", true)?), ("y", number("--y", true)?)] {
                params.insert(
                    key.into(),
                    Value::Number(
                        serde_json::Number::from_f64(value.unwrap_or_default())
                            .ok_or_else(|| format!("Invalid --{key} value."))?,
                    ),
                );
            }
        }
        "mouse-down" | "mouse-up" => {
            if let Some(button) = option_value(args, "--button")? {
                if !matches!(button.as_str(), "left" | "right" | "middle") {
                    return Err("--button must be left, right, or middle.".into());
                }
                params.insert("button".into(), Value::String(button));
            }
            for (key, value) in [("x", number("--x", false)?), ("y", number("--y", false)?)] {
                if let Some(value) = value {
                    params.insert(
                        key.into(),
                        Value::Number(
                            serde_json::Number::from_f64(value)
                                .ok_or_else(|| format!("Invalid --{key} value."))?,
                        ),
                    );
                }
            }
        }
        "mouse-wheel" => {
            for (key, value) in [
                ("dx", number("--dx", false)?),
                ("dy", number("--dy", true)?),
            ] {
                if let Some(value) = value {
                    params.insert(
                        key.into(),
                        Value::Number(
                            serde_json::Number::from_f64(value)
                                .ok_or_else(|| format!("Invalid --{key} value."))?,
                        ),
                    );
                }
            }
        }
        "geolocation" => {
            for (key, flag, required) in [
                ("latitude", "--latitude", true),
                ("longitude", "--longitude", true),
                ("accuracy", "--accuracy", false),
            ] {
                if let Some(value) = number(flag, required)? {
                    params.insert(
                        key.into(),
                        Value::Number(
                            serde_json::Number::from_f64(value)
                                .ok_or_else(|| format!("Invalid {flag} value."))?,
                        ),
                    );
                }
            }
        }
        "intercept-enable" => {
            let patterns = option_value(args, "--patterns")?
                .map(|patterns| {
                    patterns
                        .split(',')
                        .map(str::trim)
                        .filter(|pattern| !pattern.is_empty())
                        .map(|pattern| Value::String(pattern.to_string()))
                        .collect::<Vec<_>>()
                })
                .filter(|patterns| !patterns.is_empty())
                .unwrap_or_else(|| vec![Value::String("*".into())]);
            params.insert("patterns".into(), Value::Array(patterns));
        }
        "intercept-disable" | "intercept-list" => {}
        "set-device" => {
            params.insert(
                "name".into(),
                Value::String(required_option(args, "--name", "set device")?),
            );
        }
        "set-offline" => {
            let state = option_value(args, "--state")?.unwrap_or_else(|| "on".into());
            let offline = match state.as_str() {
                "on" | "true" => true,
                "off" | "false" => false,
                _ => return Err("--state must be on or off.".into()),
            };
            params.insert("state".into(), Value::Bool(offline));
        }
        "set-headers" => {
            let raw = required_option(args, "--headers", "set headers")?;
            let headers: Value = serde_json::from_str(&raw)
                .map_err(|error| format!("--headers must be a JSON object: {error}"))?;
            if !headers.is_object() {
                return Err("--headers must be a JSON object.".into());
            }
            params.insert("headers".into(), headers);
        }
        "set-credentials" => {
            params.insert(
                "user".into(),
                Value::String(required_option(args, "--user", "set credentials")?),
            );
            params.insert(
                "pass".into(),
                Value::String(required_option(args, "--pass", "set credentials")?),
            );
        }
        "set-media" => {
            if let Some(value) = option_value(args, "--color-scheme")? {
                if !matches!(value.as_str(), "dark" | "light" | "no-preference") {
                    return Err("--color-scheme must be dark, light, or no-preference.".into());
                }
                params.insert("colorScheme".into(), Value::String(value));
            }
            if let Some(value) = option_value(args, "--reduced-motion")? {
                if !matches!(value.as_str(), "reduce" | "no-preference") {
                    return Err("--reduced-motion must be reduce or no-preference.".into());
                }
                params.insert("reducedMotion".into(), Value::String(value));
            }
        }
        "clipboard-read" | "capture-start" | "capture-stop" => {}
        "clipboard-write" => {
            params.insert(
                "value".into(),
                Value::String(required_option(args, "--text", "clipboard write")?),
            );
        }
        "console" | "network" => {
            if let Some(limit) = option_value(args, "--limit")? {
                let limit = limit
                    .parse::<u64>()
                    .map_err(|_| "--limit must be a positive integer.".to_string())?;
                params.insert("limit".into(), Value::Number(limit.into()));
            }
        }
        _ => return Err(format!("Unknown browser command: {command}")),
    }
    let result = local_rpc_required(&format!("browser.{command}"), Value::Object(params))?;
    output(result, json_output, format!("Browser {command} completed"));
    Ok(0)
}

fn file_command(
    command: &str,
    positional: &[&str],
    args: &[String],
    json_output: bool,
) -> Result<i32, String> {
    let path = option_value(args, "--path")?
        .or_else(|| positional.first().map(|value| (*value).to_string()))
        .ok_or_else(|| format!("file {command} requires --path <relative-path>"))?;
    let worktree = option_value(args, "--worktree")?.unwrap_or_else(|| "active".into());
    let result = local_rpc_required(
        &format!("file.{command}"),
        json!({
            "worktree": worktree,
            "path": path,
            "staged": args.iter().any(|argument| argument == "--staged"),
        }),
    )?;
    output(
        result,
        json_output,
        format!("{command} {path} in {worktree}"),
    );
    Ok(0)
}

fn file_open_changed(args: &[String], json_output: bool) -> Result<i32, String> {
    let mode = option_value(args, "--mode")?.unwrap_or_else(|| "diff".into());
    if !matches!(mode.as_str(), "edit" | "diff" | "both") {
        return Err("Invalid --mode. Use edit, diff, or both.".into());
    }
    let worktree = option_value(args, "--worktree")?.unwrap_or_else(|| "active".into());
    let result = local_rpc_required(
        "file.openChanged",
        json!({"worktree": worktree, "mode": mode}),
    )?;
    let total = result["totalChanged"].as_u64().unwrap_or(0);
    let opened = result["opened"].as_array().map_or(0, Vec::len);
    let skipped = result["skipped"].as_array().map_or(0, Vec::len);
    let plain = if total == 0 {
        "No changed files.".to_string()
    } else if skipped == 0 {
        format!("Opened {opened} changed file targets.")
    } else {
        format!("Opened {opened} changed file targets. Skipped {skipped}.")
    };
    output(result, json_output, plain);
    Ok(0)
}

fn automations_command(
    command: &str,
    rest: &[&str],
    args: &[String],
    json_output: bool,
) -> Result<i32, String> {
    match command {
        "list" => {
            let result = local_rpc_required("automation.list", Value::Null)?;
            let plain = format_automations(&result);
            output(result, json_output, plain);
        }
        "show" => {
            let id = automation_id(rest, args)?;
            let result = local_rpc_required("automation.show", json!({"id": id}))?;
            let automation = &result["automation"];
            output(
                result.clone(),
                json_output,
                format!(
                    "{}\n  {}\n  {} · {} · {}",
                    automation["name"].as_str().unwrap_or(&id),
                    automation["prompt"].as_str().unwrap_or_default(),
                    automation["schedule"].as_str().unwrap_or_default(),
                    automation["timezone"].as_str().unwrap_or_default(),
                    automation["provider"].as_str().unwrap_or_default(),
                ),
            );
        }
        "create" => {
            let name = required_option(args, "--name", "automations create")?;
            let prompt = required_option(args, "--prompt", "automations create")?;
            let provider = required_option(args, "--provider", "automations create")?;
            let schedule = automation_schedule(args, true)?
                .ok_or_else(|| "automations create requires --trigger <schedule>".to_string())?;
            let worktree = option_value(args, "--workspace")?
                .or(option_value(args, "--worktree")?)
                .unwrap_or_else(|| "active".into());
            let timezone = option_value(args, "--timezone")?.unwrap_or_else(|| "Asia/Seoul".into());
            let enabled = automation_enabled_flag(args)?.unwrap_or(true);
            let result = local_rpc_required(
                "automation.create",
                json!({
                    "name": name,
                    "prompt": prompt,
                    "provider": provider,
                    "schedule": schedule,
                    "worktree": worktree,
                    "timezone": timezone,
                    "enabled": enabled,
                }),
            )?;
            let id = result["automation"]["id"].as_str().unwrap_or("automation");
            output(result.clone(), json_output, format!("Created {id}"));
        }
        "edit" => {
            let id = automation_id(rest, args)?;
            let mut params = serde_json::Map::new();
            params.insert("id".into(), Value::String(id.clone()));
            for (flag, field) in [
                ("--name", "name"),
                ("--prompt", "prompt"),
                ("--provider", "provider"),
                ("--timezone", "timezone"),
                ("--workspace", "worktree"),
            ] {
                if let Some(value) = option_value(args, flag)? {
                    params.insert(field.into(), Value::String(value));
                }
            }
            if let Some(schedule) = automation_schedule(args, false)? {
                params.insert("schedule".into(), Value::String(schedule));
            }
            if let Some(enabled) = automation_enabled_flag(args)? {
                params.insert("enabled".into(), Value::Bool(enabled));
            }
            if params.len() == 1 {
                return Err("automations edit requires at least one change flag.".into());
            }
            let result = local_rpc_required("automation.edit", Value::Object(params))?;
            output(result, json_output, format!("Updated {id}"));
        }
        "remove" => {
            let id = automation_id(rest, args)?;
            let result = local_rpc_required("automation.remove", json!({"id": id}))?;
            output(result, json_output, format!("Removed {id}"));
        }
        "run" => {
            let id = automation_id(rest, args)?;
            let result = local_rpc_required("automation.run", json!({"id": id}))?;
            output(result, json_output, format!("Dispatched {id}"));
        }
        "runs" => {
            let id = option_value(args, "--id")?;
            let result = local_rpc_required("automation.runs", json!({"id": id}))?;
            let plain = result["runs"]
                .as_array()
                .map(|runs| {
                    runs.iter()
                        .map(|run| {
                            format!(
                                "{}\t{}\t{}",
                                run["automation_id"].as_str().unwrap_or_default(),
                                run["status"].as_str().unwrap_or("unknown"),
                                run["started_at_unix_ms"].as_i64().unwrap_or_default(),
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_else(|| "No automation runs".into());
            output(result, json_output, plain);
        }
        _ => return Err(format!("Unknown automations command: {command}")),
    }
    Ok(0)
}

fn automation_id(rest: &[&str], args: &[String]) -> Result<String, String> {
    rest.first()
        .map(|value| (*value).to_string())
        .or(option_value(args, "--id")?)
        .ok_or_else(|| "Automation id is required.".to_string())
}

fn required_option(args: &[String], flag: &str, command: &str) -> Result<String, String> {
    option_value(args, flag)?.ok_or_else(|| format!("{command} requires {flag} <value>"))
}

fn automation_enabled_flag(args: &[String]) -> Result<Option<bool>, String> {
    let enabled = args.iter().any(|arg| arg == "--enabled");
    let disabled = args.iter().any(|arg| arg == "--disabled");
    match (enabled, disabled) {
        (true, true) => Err("Use either --enabled or --disabled, not both.".into()),
        (true, false) => Ok(Some(true)),
        (false, true) => Ok(Some(false)),
        (false, false) => Ok(None),
    }
}

fn automation_schedule(args: &[String], required: bool) -> Result<Option<String>, String> {
    let trigger = option_value(args, "--trigger")?;
    let schedule = option_value(args, "--schedule")?;
    if trigger.is_some() && schedule.is_some() {
        return Err("Use either --trigger or --schedule, not both.".into());
    }
    let Some(raw) = trigger.or(schedule) else {
        if required {
            return Err("Missing required --trigger.".into());
        }
        return Ok(None);
    };
    let time = option_value(args, "--time")?.unwrap_or_else(|| "09:00".into());
    let (hour, minute) = parse_automation_time(&time)?;
    let day = option_value(args, "--day")?
        .map(|value| {
            value
                .parse::<u8>()
                .ok()
                .filter(|day| *day <= 6)
                .ok_or_else(|| "--day must be an integer from 0 to 6".to_string())
        })
        .transpose()?
        .unwrap_or(1);
    let normalized = match raw.as_str() {
        "hourly" => {
            if args.iter().any(|arg| arg == "--time") {
                return Err("--time cannot be used with the hourly trigger.".into());
            }
            "0 * * * *".to_string()
        }
        "daily" => format!("{minute} {hour} * * *"),
        "weekdays" => format!("{minute} {hour} * * 1-5"),
        "weekly" => format!("{minute} {hour} * * {day}"),
        _ => {
            if args.iter().any(|arg| arg == "--time" || arg == "--day") {
                return Err("--time and --day can only be used with preset triggers.".into());
            }
            raw
        }
    };
    Ok(Some(normalized))
}

fn parse_automation_time(value: &str) -> Result<(u8, u8), String> {
    let Some((hour, minute)) = value.split_once(':') else {
        return Err("--time must use HH:MM format".into());
    };
    let hour = hour
        .parse::<u8>()
        .ok()
        .filter(|hour| *hour <= 23)
        .ok_or_else(|| "--time must be a valid 24-hour time".to_string())?;
    let minute = minute
        .parse::<u8>()
        .ok()
        .filter(|minute| *minute <= 59)
        .ok_or_else(|| "--time must be a valid 24-hour time".to_string())?;
    Ok((hour, minute))
}

fn format_automations(result: &Value) -> String {
    result["automations"]
        .as_array()
        .map(|automations| {
            automations
                .iter()
                .map(|automation| {
                    format!(
                        "{}\t{}\t{}\t{}",
                        automation["id"].as_str().unwrap_or_default(),
                        if automation["enabled"].as_bool().unwrap_or(false) {
                            "enabled"
                        } else {
                            "disabled"
                        },
                        automation["schedule"].as_str().unwrap_or_default(),
                        automation["name"].as_str().unwrap_or_default(),
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|plain| !plain.is_empty())
        .unwrap_or_else(|| "No automations".into())
}

struct BundledSkillGuide {
    name: &'static str,
    markdown: &'static str,
}

const BUNDLED_SKILL_GUIDES: &[BundledSkillGuide] = &[
    BundledSkillGuide {
        name: "computer-use",
        markdown: include_str!("../assets/orca/skills/computer-use.md"),
    },
    BundledSkillGuide {
        name: "linear-tickets",
        markdown: include_str!("../assets/orca/skills/linear-tickets.md"),
    },
    BundledSkillGuide {
        name: "orca-cli",
        markdown: include_str!("../assets/orca/skills/orca-cli.md"),
    },
    BundledSkillGuide {
        name: "orca-emulator",
        markdown: include_str!("../assets/orca/skills/orca-emulator.md"),
    },
    BundledSkillGuide {
        name: "orca-emulator-android",
        markdown: include_str!("../assets/orca/skills/orca-emulator-android.md"),
    },
    BundledSkillGuide {
        name: "orca-linear",
        markdown: include_str!("../assets/orca/skills/orca-linear.md"),
    },
    BundledSkillGuide {
        name: "orca-per-workspace-env",
        markdown: include_str!("../assets/orca/skills/orca-per-workspace-env.md"),
    },
    BundledSkillGuide {
        name: "orchestration",
        markdown: include_str!("../assets/orca/skills/orchestration.md"),
    },
];

fn skill_description(markdown: &str) -> String {
    let mut description = Vec::new();
    let mut collecting = false;
    for line in markdown.lines() {
        if matches!(line, "description: >-" | "description: >") {
            collecting = true;
            continue;
        }
        if collecting {
            if let Some(value) = line.strip_prefix("  ") {
                description.push(value.trim());
            } else {
                break;
            }
        }
    }
    description.join(" ")
}

fn bundled_skill(topic: &str) -> Option<&'static BundledSkillGuide> {
    BUNDLED_SKILL_GUIDES.iter().find(|guide| {
        guide.name == topic || (topic == "linear-tickets" && guide.name == "orca-linear")
    })
}

fn rebrand_skill_text(value: &str) -> String {
    value.replace("Orca", "Suaegi").replace("orca ", "suaegi ")
}

fn skills_list(json_output: bool) -> Result<i32, String> {
    let topics = BUNDLED_SKILL_GUIDES
        .iter()
        .map(|guide| {
            json!({
                "name": guide.name,
                "description": rebrand_skill_text(&skill_description(guide.markdown)),
            })
        })
        .collect::<Vec<_>>();
    let plain = topics
        .iter()
        .map(|topic| {
            format!(
                "{}: {}",
                topic["name"].as_str().unwrap_or_default(),
                topic["description"].as_str().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    output(json!({"topics": topics}), json_output, plain);
    Ok(0)
}

fn skills_get(topic: &str, args: &[String], json_output: bool) -> Result<i32, String> {
    let guide = bundled_skill(topic).ok_or_else(|| {
        format!(
            "Unknown skill topic \"{topic}\". Available topics: {}",
            BUNDLED_SKILL_GUIDES
                .iter()
                .map(|guide| guide.name)
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;
    // The current Orca release ships no extra reference appendices, so
    // `--full` is intentionally byte-identical while preserving its wire flag.
    let full = args.iter().any(|arg| arg == "--full");
    let markdown = rebrand_skill_text(guide.markdown);
    if json_output {
        output(
            json!({"name": guide.name, "full": full, "markdown": markdown}),
            true,
            String::new(),
        );
    } else {
        print!(
            "{}{}",
            markdown,
            if markdown.ends_with('\n') { "" } else { "\n" }
        );
    }
    Ok(0)
}

fn diagnostics_memory(json_output: bool) -> Result<i32, String> {
    if let Some(result) = crate::local_rpc::call("diagnostics.memory", Value::Null)? {
        let snapshot: crate::memory::MemorySnapshot = serde_json::from_value(result.clone())
            .map_err(|error| format!("Invalid memory snapshot from Suaegi: {error}"))?;
        output(result, json_output, crate::memory::format(&snapshot));
        return Ok(0);
    }
    let snapshot = crate::memory::collect(&load_state(), None);
    let plain = crate::memory::format(&snapshot);
    output(
        serde_json::to_value(snapshot)
            .map_err(|error| format!("Could not encode memory snapshot: {error}"))?,
        json_output,
        plain,
    );
    Ok(0)
}

fn vm_recipe_doctor(recipe_id: &str, args: &[String], json_output: bool) -> Result<i32, String> {
    let repo_path = option_value(args, "--repo-path")?
        .map(PathBuf::from)
        .unwrap_or(
            std::env::current_dir().map_err(|error| format!("Could not read cwd: {error}"))?,
        );
    let mut checks = Vec::new();
    let yaml_path = repo_path.join("orca.yaml");
    if !yaml_path.is_file() {
        checks.push(json!({
            "id": "orca_yaml.exists",
            "status": "fail",
            "message": format!("No orca.yaml found at {}", yaml_path.display()),
            "remediation": "Add environmentRecipes to the repo orca.yaml."
        }));
        return finish_vm_doctor(recipe_id, &repo_path, checks, false, json_output);
    }
    checks.push(json!({
        "id": "orca_yaml.exists",
        "status": "pass",
        "message": "orca.yaml exists."
    }));
    let recipes = match crate::repo_hooks::load_environment_recipes(&repo_path) {
        Ok(recipes) => {
            checks.push(json!({
                "id": "orca_yaml.parse",
                "status": "pass",
                "message": "orca.yaml parsed successfully."
            }));
            recipes
        }
        Err(error) => {
            checks.push(json!({
                "id": "orca_yaml.parse",
                "status": "fail",
                "message": error,
                "remediation": "Fix the YAML syntax and environmentRecipes schema."
            }));
            return finish_vm_doctor(recipe_id, &repo_path, checks, false, json_output);
        }
    };
    let Some(recipe) = recipes.into_iter().find(|recipe| recipe.id == recipe_id) else {
        checks.push(json!({
            "id": "recipe.exists",
            "status": "fail",
            "message": format!("Environment recipe \"{recipe_id}\" was not found."),
            "remediation": "Use an id from orca.yaml environmentRecipes."
        }));
        return finish_vm_doctor(recipe_id, &repo_path, checks, false, json_output);
    };
    checks.push(json!({
        "id": "recipe.exists",
        "status": "pass",
        "message": format!("Recipe {} ({}) is valid.", recipe.id, recipe.name)
    }));
    checks.push(json!({
        "id": "recipe.lifecycle",
        "status": if recipe.suspend.is_some() == recipe.resume.is_some() { "pass" } else { "warn" },
        "message": if recipe.suspend.is_some() == recipe.resume.is_some() {
            "Suspend and resume lifecycle commands are paired."
        } else {
            "Only one of suspend/resume is configured."
        }
    }));

    let provision = args
        .iter()
        .any(|arg| matches!(arg.as_str(), "--provision" | "--connect"));
    if provision {
        let temp = tempfile::tempdir()
            .map_err(|error| format!("Could not create VM doctor workspace: {error}"))?;
        let store = temp.path().join("runtime.json");
        let result = runtime()?.block_on(crate::ephemeral_vm::provision(
            store.clone(),
            recipe.clone(),
            repo_path.clone(),
            crate::ephemeral_vm::RecipeContext::default(),
            None,
        ));
        match result {
            Ok(record) => {
                checks.push(json!({
                    "id": "recipe.provision",
                    "status": "pass",
                    "message": "Recipe ran successfully and produced a valid VM recipe result."
                }));
                let cleanup = runtime()?.block_on(crate::ephemeral_vm::cleanup(
                    store,
                    record.id,
                    repo_path.clone(),
                ));
                match cleanup {
                    Ok(record)
                        if record.cleanup_status
                            == crate::ephemeral_vm::CleanupStatus::Succeeded =>
                    {
                        checks.push(json!({
                            "id": "recipe.destroy.run",
                            "status": "pass",
                            "message": "Destroy action ran successfully after provisioning."
                        }));
                    }
                    Ok(_) => checks.push(json!({
                        "id": "recipe.destroy.run",
                        "status": "warn",
                        "message": "Destroy was skipped because destroy is disabled or missing.",
                        "remediation": "Destroy any provider resources created by the doctor run manually."
                    })),
                    Err(_) => {
                        checks.push(json!({
                            "id": "recipe.destroy.run",
                            "status": "fail",
                            "message": "Destroy action failed after provisioning.",
                            "remediation": "Inspect the recipe destroy command and clean up provider resources manually."
                        }));
                    }
                }
            }
            Err(_) => checks.push(json!({
                "id": "recipe.provision",
                "status": "fail",
                "message": "Recipe provisioning failed.",
                "remediation": "Run the create command manually and inspect its redacted output."
            })),
        }
    }
    let ok = checks
        .iter()
        .all(|check| check["status"].as_str() != Some("fail"));
    finish_vm_doctor(recipe_id, &repo_path, checks, ok, json_output)
}

fn finish_vm_doctor(
    recipe_id: &str,
    repo_path: &Path,
    checks: Vec<Value>,
    ok: bool,
    json_output: bool,
) -> Result<i32, String> {
    let plain = checks
        .iter()
        .map(|check| {
            format!(
                "{} {}: {}",
                if check["status"] == "pass" {
                    "✓"
                } else if check["status"] == "warn" {
                    "!"
                } else {
                    "×"
                },
                check["id"].as_str().unwrap_or("check"),
                check["message"].as_str().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    output(
        json!({
            "recipeId": recipe_id,
            "repoPath": repo_path,
            "ok": ok,
            "checks": checks
        }),
        json_output,
        plain,
    );
    Ok(if ok { 0 } else { 1 })
}

fn open_settings(section: &str, json_output: bool) -> Result<i32, String> {
    let destination = format!("settings:{section}");
    let result = local_rpc_required("navigate", json!({"destination": destination}))?;
    output(result, json_output, format!("Opened settings: {section}"));
    Ok(0)
}

fn agent_context_schema() -> Value {
    let mut commands = Vec::new();
    for usage in HELP
        .lines()
        .filter_map(|line| line.trim().strip_prefix("suaegi "))
    {
        let tokens = usage.split_whitespace().collect::<Vec<_>>();
        let mut paths = vec![Vec::<String>::new()];
        for token in &tokens {
            if token.starts_with('[') || token.starts_with('<') || token.starts_with("--") {
                break;
            }
            let choices = token
                .trim_matches(|character| matches!(character, '[' | ']' | '<' | '>'))
                .split('|')
                .collect::<Vec<_>>();
            let mut expanded = Vec::with_capacity(paths.len() * choices.len());
            for path in &paths {
                for choice in &choices {
                    let mut next = path.clone();
                    next.push((*choice).to_string());
                    expanded.push(next);
                }
            }
            paths = expanded;
        }
        let mut flags = tokens
            .iter()
            .filter_map(|token| {
                let token = token.trim_matches(|character: char| {
                    matches!(character, '[' | ']' | '<' | '>' | ',')
                });
                token
                    .strip_prefix("--")
                    .map(|flag| flag.trim_end_matches(']').to_string())
            })
            .collect::<Vec<_>>();
        flags.extend(["help".to_string(), "json".to_string()]);
        flags.sort();
        flags.dedup();
        for path in paths.into_iter().filter(|path| !path.is_empty()) {
            commands.push(json!({
                "command": path.join(" "),
                "path": path,
                "aliases": [],
                "argumentMode": "parsed",
                "summary": "",
                "usage": format!("suaegi {usage}"),
                "flags": flags,
                "positionalArgs": [],
                "examples": [],
                "notes": [],
            }));
        }
    }
    commands.sort_by(|left, right| left["command"].as_str().cmp(&right["command"].as_str()));
    commands.dedup_by(|left, right| left["command"] == right["command"]);
    json!({
        "schemaVersion": 1,
        "commandCount": commands.len(),
        "commands": commands,
    })
}

fn desktop_is_running() -> bool {
    std::process::Command::new("pgrep")
        .args(["-af", "/Contents/MacOS/suaegi-app"])
        .output()
        .is_ok_and(|output| {
            output.status.success()
                && String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .any(|line| !line.contains("--pty-daemon"))
        })
}

fn open_desktop() -> Result<(), String> {
    let status = std::process::Command::new("open")
        .args(["-a", "Suaegi"])
        .status()
        .map_err(|error| format!("Could not launch Suaegi: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("macOS could not launch Suaegi".into())
    }
}

fn output(value: Value, json_output: bool, plain: String) {
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("JSON value is serializable")
        );
    } else {
        println!("{plain}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use suaegi_core::domain::RepoId;

    #[test]
    fn only_cli_invocations_are_intercepted() {
        assert!(should_handle(
            OsStr::new("/Users/test/.local/bin/suaegi"),
            &[]
        ));
        assert!(should_handle(
            OsStr::new("/tmp/suaegi-app"),
            &[OsString::from("status")]
        ));
        assert!(should_handle(
            OsStr::new("/tmp/suaegi-app"),
            &[OsString::from("claude-teams")]
        ));
        assert!(!should_handle(
            OsStr::new("/Applications/Suaegi.app/Contents/MacOS/Suaegi"),
            &[]
        ));
        assert!(!should_handle(
            OsStr::new("/Applications/Suaegi.app/Contents/MacOS/Suaegi"),
            &[OsString::from("-psn_0_123")]
        ));
    }

    #[test]
    fn option_values_are_strict() {
        assert_eq!(
            option_value(
                &[
                    "worktree".into(),
                    "list".into(),
                    "--repo".into(),
                    "app".into()
                ],
                "--repo"
            )
            .unwrap()
            .as_deref(),
            Some("app")
        );
        assert!(option_value(&["--repo".into()], "--repo").is_err());
    }

    #[test]
    fn linear_issue_deep_context_flags_match_orca_and_depth_is_not_positional() {
        assert!(HELP.contains(
            "linear issue [<id>] [--current] [--comments] [--children] [--depth <n>] [--attachments] [--relations] [--activity] [--full]"
        ));
        let args = vec![
            "linear".into(),
            "issue".into(),
            "ENG-123".into(),
            "--children".into(),
            "--depth".into(),
            "3".into(),
            "--activity".into(),
        ];
        assert_eq!(positional_args(&args), vec!["linear", "issue", "ENG-123"]);
    }

    #[test]
    fn repo_selection_accepts_name_id_and_path() {
        let state = PersistedState {
            repos: vec![Repo {
                id: RepoId("/tmp/example".into()),
                path: PathBuf::from("/tmp/example"),
                display_name: "example".into(),
                worktree_base_ref: None,
            }],
            ..PersistedState::default()
        };
        assert_eq!(
            select_repo(&state, "example").unwrap().id,
            RepoId("/tmp/example".into())
        );
        assert_eq!(
            select_repo(&state, "/tmp/example").unwrap().display_name,
            "example"
        );
    }

    #[test]
    fn environment_selection_accepts_id_and_case_insensitive_name_without_secrets() {
        let environment = RuntimeEnvironmentSetting {
            id: "runtime-123".into(),
            name: "Work Laptop".into(),
            endpoint: "wss://runtime.example.test".into(),
            credentials_configured: true,
            created_at_unix_ms: 42,
        };
        let mut state = PersistedState::default();
        state
            .settings
            .ui
            .runtime_environments
            .push(environment.clone());

        assert_eq!(
            select_environment(&state, "runtime-123").unwrap(),
            &environment
        );
        assert_eq!(
            select_environment(&state, "work laptop").unwrap(),
            &environment
        );
        let encoded = environment_json(&environment).to_string();
        assert!(!encoded.contains("token"));
        assert!(!encoded.contains("publicKey"));
    }

    #[test]
    fn agent_context_is_generated_from_the_live_help_surface() {
        let schema = agent_context_schema();
        let commands = schema["commands"].as_array().unwrap();
        for expected in [
            "agent-context",
            "agent hooks off",
            "computer capabilities",
            "environment add",
            "exec",
            "file open-changed",
            "linear create",
            "linear issue",
            "linear save-issue",
            "linear team list",
            "project setup-create",
            "project setup-delete",
            "project setups",
            "skills get",
            "skills list",
            "tab profile delete",
            "terminal show",
        ] {
            assert!(
                commands
                    .iter()
                    .any(|command| command["command"] == expected),
                "missing {expected}"
            );
        }
        assert_eq!(
            schema["commandCount"].as_u64().unwrap() as usize,
            commands.len()
        );
    }

    #[test]
    fn bundled_skill_guides_match_the_reference_topics_and_have_descriptions() {
        assert_eq!(BUNDLED_SKILL_GUIDES.len(), 8);
        let names = BUNDLED_SKILL_GUIDES
            .iter()
            .map(|guide| guide.name)
            .collect::<Vec<_>>();
        assert!(names.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(BUNDLED_SKILL_GUIDES.iter().all(|guide| {
            !skill_description(guide.markdown).is_empty()
                && guide.markdown.starts_with("---\nname:")
        }));
    }

    #[test]
    fn file_open_changed_rejects_unknown_modes_before_contacting_the_app() {
        let error = file_open_changed(
            &[
                "file".into(),
                "open-changed".into(),
                "--mode".into(),
                "sideways".into(),
            ],
            true,
        )
        .unwrap_err();
        assert_eq!(error, "Invalid --mode. Use edit, diff, or both.");
    }

    #[test]
    fn automation_presets_match_orca_cron_semantics() {
        assert_eq!(
            automation_schedule(
                &[
                    "automations".into(),
                    "create".into(),
                    "--trigger".into(),
                    "weekdays".into(),
                    "--time".into(),
                    "09:30".into(),
                ],
                true,
            )
            .unwrap()
            .as_deref(),
            Some("30 9 * * 1-5")
        );
        assert!(automation_schedule(
            &[
                "--trigger".into(),
                "hourly".into(),
                "--time".into(),
                "09:30".into(),
            ],
            true,
        )
        .is_err());
    }

    #[test]
    fn project_clone_names_are_derived_without_accepting_path_components() {
        assert_eq!(
            clone_directory_name("https://github.com/stablyai/orca.git").as_deref(),
            Some("orca")
        );
        assert_eq!(
            clone_directory_name("git@github.com:stablyai/orca.git").as_deref(),
            Some("orca")
        );
        assert_eq!(clone_directory_name("https://example.test/"), None);
        assert_eq!(clone_directory_name(".."), None);
    }

    #[test]
    fn project_metadata_enums_are_validated_before_rpc() {
        assert!(project_kind(&["--kind".into(), "archive".into()]).is_err());
        let mut fields = serde_json::Map::new();
        assert!(project_setup_optional_fields(
            &["--state".into(), "unknown".into()],
            &mut fields,
            false,
        )
        .is_err());
    }

    #[test]
    fn linear_issue_urls_are_normalized_without_changing_identifiers() {
        assert_eq!(normalize_linear_issue_input("ENG-123"), "ENG-123");
        assert_eq!(
            normalize_linear_issue_input("https://linear.app/acme/issue/ENG-123/title"),
            "ENG-123"
        );
    }

    #[test]
    fn linear_write_inputs_are_strict_and_idempotency_ids_are_valid() {
        assert_eq!(linear_priority("urgent").unwrap(), 1);
        assert_eq!(linear_priority("low").unwrap(), 4);
        assert!(linear_priority("critical").is_err());
        assert!(validate_linear_due_date("2026-07-30").is_ok());
        assert!(validate_linear_due_date("07/30/2026").is_err());
        let id = linear_write_id(&[]).unwrap();
        assert_eq!(id.as_str().len(), 36);
        assert!(suaegi_tracker::WriteId::parse(id.as_str()).is_ok());
        assert!(linear_write_id(&["--write-id".into(), "not-a-uuid".into()]).is_err());
    }

    #[test]
    fn repeated_linear_labels_are_preserved() {
        let args = vec![
            "--label".into(),
            "Bug".into(),
            "--label".into(),
            "Backend".into(),
        ];
        assert_eq!(
            option_values(&args, "--label").unwrap(),
            vec!["Bug".to_string(), "Backend".to_string()]
        );
        assert!(option_values(&["--label".into()], "--label").is_err());
    }

    #[test]
    fn browser_exec_parser_preserves_quoted_agent_browser_arguments() {
        assert_eq!(
            split_command_line(r#"fill --element e2 --value "hello world" --json"#).unwrap(),
            vec![
                "fill".to_string(),
                "--element".to_string(),
                "e2".to_string(),
                "--value".to_string(),
                "hello world".to_string(),
                "--json".to_string(),
            ]
        );
        assert_eq!(
            split_command_line(r#"eval --expression 'document.title'"#).unwrap(),
            vec![
                "eval".to_string(),
                "--expression".to_string(),
                "document.title".to_string(),
            ]
        );
        assert!(split_command_line(r#"fill --value "unterminated"#).is_err());
    }
}
