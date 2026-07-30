//! OpenSSH-backed target import and connection checks.
//!
//! Suaegi deliberately delegates authentication, agent/keychain integration,
//! ProxyCommand, ProxyJump, and host-key policy to the system OpenSSH client.

use std::collections::HashSet;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use suaegi_core::domain::SshHostSetting;
use wait_timeout::ChildExt;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(12);
const PROJECT_SETUP_TIMEOUT: Duration = Duration::from_secs(10 * 60);
type RemoteProgram<'a> = (&'a str, &'a [String], &'a [(String, String)]);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshConnectionCheck {
    pub success: bool,
    pub message: String,
}

fn run_command(program: &Path, args: &[String], timeout: Duration) -> Result<String, String> {
    run_command_with_input(program, args, timeout, None, 1024 * 1024)
}

fn run_command_with_input(
    program: &Path,
    args: &[String],
    timeout: Duration,
    input: Option<&[u8]>,
    max_output_bytes: u64,
) -> Result<String, String> {
    let mut stdout_file = tempfile::tempfile().map_err(|error| error.to_string())?;
    let mut stderr_file = tempfile::tempfile().map_err(|error| error.to_string())?;
    let mut child = Command::new(program)
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::from(
            stdout_file.try_clone().map_err(|error| error.to_string())?,
        ))
        .stderr(Stdio::from(
            stderr_file.try_clone().map_err(|error| error.to_string())?,
        ))
        .spawn()
        .map_err(|error| format!("Could not start OpenSSH: {error}"))?;
    if let Some(input) = input {
        child
            .stdin
            .take()
            .ok_or_else(|| "Could not open SSH command input.".to_string())?
            .write_all(input)
            .map_err(|error| format!("Could not write SSH command input: {error}"))?;
    }
    let status = match child
        .wait_timeout(timeout)
        .map_err(|error| error.to_string())?
    {
        Some(status) => status,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return Err("SSH command timed out.".to_string());
        }
    };
    stdout_file.rewind().map_err(|error| error.to_string())?;
    stderr_file.rewind().map_err(|error| error.to_string())?;
    let stdout_size = stdout_file
        .metadata()
        .map_err(|error| error.to_string())?
        .len();
    let stderr_size = stderr_file
        .metadata()
        .map_err(|error| error.to_string())?
        .len();
    if stdout_size > max_output_bytes || stderr_size > 1024 * 1024 {
        return Err("SSH command output exceeded its safety limit.".to_string());
    }
    let mut stdout = String::new();
    let mut stderr = String::new();
    stdout_file
        .read_to_string(&mut stdout)
        .map_err(|error| error.to_string())?;
    stderr_file
        .read_to_string(&mut stderr)
        .map_err(|error| error.to_string())?;
    if status.success() {
        Ok(stdout.trim().to_string())
    } else {
        let message = stderr
            .lines()
            .filter(|line| !line.trim().is_empty())
            .take(4)
            .collect::<Vec<_>>()
            .join("\n");
        Err(if message.is_empty() {
            format!("OpenSSH exited with {status}.")
        } else {
            message
        })
    }
}

fn ssh_binary() -> PathBuf {
    PathBuf::from("/usr/bin/ssh")
}

fn shell_quote(value: &str) -> Result<String, String> {
    if value.contains('\0') {
        return Err("Paths and URLs cannot contain NUL bytes.".to_string());
    }
    Ok(format!("'{}'", value.replace('\'', "'\"'\"'")))
}

fn remote_command(
    target: &SshHostSetting,
    command: String,
    timeout: Duration,
) -> Result<String, String> {
    let mut args = command_args(target, true);
    args.push(command);
    run_command(&ssh_binary(), &args, timeout)
}

fn clone_name(url: &str) -> Result<String, String> {
    let trimmed = url.trim().trim_end_matches('/');
    let tail = trimmed
        .rsplit(['/', ':'])
        .next()
        .unwrap_or_default()
        .strip_suffix(".git")
        .unwrap_or_else(|| trimmed.rsplit(['/', ':']).next().unwrap_or_default())
        .trim();
    if tail.is_empty()
        || matches!(tail, "." | "..")
        || tail.starts_with('-')
        || tail.contains(['/', '\\', '\0'])
    {
        return Err("Could not derive a safe repository name from the clone URL.".to_string());
    }
    Ok(tail.to_string())
}

fn remote_clone_path(destination: &str, url: &str) -> Result<String, String> {
    let destination = destination.trim();
    if !destination.starts_with('/') {
        return Err("Clone destination must be an absolute remote path.".to_string());
    }
    let name = clone_name(url)?;
    if destination == "/" {
        Ok(format!("/{name}"))
    } else {
        Ok(format!("{}/{name}", destination.trim_end_matches('/')))
    }
}

pub async fn validate_project_folder(
    target: SshHostSetting,
    path: String,
    kind: String,
) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let path = path.trim();
        if !path.starts_with('/') {
            return Err("Project path must be an absolute remote path.".to_string());
        }
        if !matches!(kind.as_str(), "git" | "folder") {
            return Err("Project kind must be git or folder.".to_string());
        }
        let quoted = shell_quote(path)?;
        let command = if kind == "git" {
            format!(
                "test -d {quoted} && git -C {quoted} rev-parse --is-inside-work-tree >/dev/null"
            )
        } else {
            format!("test -d {quoted}")
        };
        remote_command(&target, command, COMMAND_TIMEOUT)?;
        Ok(path.to_string())
    })
    .await
    .map_err(|error| format!("SSH project validation task failed: {error}"))?
}

pub async fn clone_project(
    target: SshHostSetting,
    url: String,
    destination: String,
) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let url = url.trim();
        if url.is_empty() {
            return Err("Clone URL is required.".to_string());
        }
        let clone_path = remote_clone_path(&destination, url)?;
        let destination = destination.trim();
        let destination_q = shell_quote(destination)?;
        let clone_path_q = shell_quote(&clone_path)?;
        let url_q = shell_quote(url)?;
        // Claim nothing and delete nothing: if the target already exists the
        // command fails without touching it, matching Orca's conservative
        // clone-target ownership rule.
        let command = format!(
            "test -d {destination_q} && test ! -e {clone_path_q} && \
             git clone -- {url_q} {clone_path_q}"
        );
        remote_command(&target, command, PROJECT_SETUP_TIMEOUT)?;
        Ok(clone_path)
    })
    .await
    .map_err(|error| format!("SSH clone task failed: {error}"))?
}

fn concrete_aliases(contents: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut aliases = Vec::new();
    for raw in contents.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(keyword) = parts.next() else {
            continue;
        };
        if !keyword.eq_ignore_ascii_case("host") {
            continue;
        }
        for alias in parts {
            if alias.starts_with('!')
                || alias.contains('*')
                || alias.contains('?')
                || alias.starts_with('#')
            {
                continue;
            }
            if seen.insert(alias.to_string()) {
                aliases.push(alias.to_string());
            }
        }
    }
    aliases
}

fn value<'a>(config: &'a [(String, String)], key: &str) -> Option<&'a str> {
    config
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.as_str())
}

fn resolve_alias(alias: &str) -> Result<SshHostSetting, String> {
    let args = vec!["-G".to_string(), "--".to_string(), alias.to_string()];
    let output = run_command(&ssh_binary(), &args, COMMAND_TIMEOUT)?;
    let config = output
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once(char::is_whitespace)?;
            Some((key.to_string(), value.trim().to_string()))
        })
        .collect::<Vec<_>>();
    let hostname = value(&config, "hostname").unwrap_or(alias).to_string();
    let user = value(&config, "user").unwrap_or_default().to_string();
    let port = value(&config, "port")
        .and_then(|port| port.parse::<u16>().ok())
        .unwrap_or(22);
    let identity_file = value(&config, "identityfile")
        .filter(|path| *path != "none")
        .unwrap_or_default()
        .to_string();
    let proxy_command = value(&config, "proxycommand")
        .filter(|command| *command != "none")
        .unwrap_or_default()
        .to_string();
    let jump_host = value(&config, "proxyjump")
        .filter(|host| *host != "none")
        .unwrap_or_default()
        .to_string();
    Ok(SshHostSetting {
        id: format!("ssh-config-{alias}"),
        label: alias.to_string(),
        config_host: alias.to_string(),
        hostname,
        user,
        port,
        identity_file,
        proxy_command,
        jump_host,
        system_ssh_connection_reuse: true,
        relay_grace_period_seconds: 0,
        source: "ssh-config".to_string(),
    })
}

pub async fn import_config() -> Result<Vec<SshHostSetting>, String> {
    tokio::task::spawn_blocking(|| {
        let path = dirs::home_dir()
            .ok_or_else(|| "Could not locate the home directory.".to_string())?
            .join(".ssh/config");
        let contents = std::fs::read_to_string(&path)
            .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
        let mut targets = Vec::new();
        let mut errors = Vec::new();
        for alias in concrete_aliases(&contents) {
            match resolve_alias(&alias) {
                Ok(target) => targets.push(target),
                Err(error) => errors.push(format!("{alias}: {error}")),
            }
        }
        if targets.is_empty() && !errors.is_empty() {
            Err(errors.join("\n"))
        } else {
            Ok(targets)
        }
    })
    .await
    .map_err(|error| format!("SSH import task failed: {error}"))?
}

pub fn command_args(target: &SshHostSetting, batch_mode: bool) -> Vec<String> {
    let mut args = Vec::new();
    if batch_mode {
        args.extend([
            "-o".to_string(),
            "BatchMode=yes".to_string(),
            "-o".to_string(),
            "ConnectTimeout=8".to_string(),
        ]);
    }
    if target.port != 22 {
        args.extend(["-p".to_string(), target.port.to_string()]);
    }
    if !target.config_host.trim().is_empty()
        && !target.hostname.trim().is_empty()
        && target.config_host.trim() != target.hostname.trim()
    {
        args.extend([
            "-o".to_string(),
            format!("HostName={}", target.hostname.trim()),
        ]);
    }
    if !target.identity_file.trim().is_empty() {
        args.extend(["-i".to_string(), target.identity_file.trim().to_string()]);
    }
    if !target.proxy_command.trim().is_empty() {
        args.extend([
            "-o".to_string(),
            format!("ProxyCommand={}", target.proxy_command.trim()),
        ]);
    }
    if !target.jump_host.trim().is_empty() {
        args.extend(["-J".to_string(), target.jump_host.trim().to_string()]);
    }
    if target.system_ssh_connection_reuse {
        args.extend([
            "-o".to_string(),
            "ControlMaster=auto".to_string(),
            "-o".to_string(),
            "ControlPersist=60".to_string(),
        ]);
    }
    let host = if target.config_host.trim().is_empty() {
        target.hostname.trim()
    } else {
        target.config_host.trim()
    };
    let destination = if target.user.trim().is_empty() {
        host.to_string()
    } else {
        format!("{}@{host}", target.user.trim())
    };
    args.push("--".to_string());
    args.push(destination);
    args
}

pub fn interactive_project_command(
    target: &SshHostSetting,
    project_path: &str,
) -> Result<String, String> {
    let project_path = project_path.trim();
    if !project_path.starts_with('/') {
        return Err("Project path must be an absolute remote path.".to_string());
    }
    let mut args = command_args(target, false);
    let option_end = args
        .iter()
        .position(|arg| arg == "--")
        .ok_or_else(|| "Could not construct SSH arguments.".to_string())?;
    args.insert(option_end, "-t".to_string());
    let path_q = shell_quote(project_path)?;
    args.push(format!("cd -- {path_q} && exec \"${{SHELL:-/bin/sh}}\" -l"));
    let mut command = String::from("ssh");
    for arg in args {
        command.push(' ');
        command.push_str(&shell_quote(&arg)?);
    }
    command.push('\r');
    Ok(command)
}

fn recipe_batch_args(target: &crate::ephemeral_vm::RecipeSshTarget) -> Vec<String> {
    let mut args = vec![
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        "ConnectTimeout=8".to_string(),
    ];
    if target.port != 22 {
        args.extend(["-p".to_string(), target.port.to_string()]);
    }
    if target
        .config_host
        .as_deref()
        .map(str::trim)
        .is_some_and(|alias| !alias.is_empty() && alias != target.host.trim())
    {
        args.extend(["-o".to_string(), format!("HostName={}", target.host.trim())]);
    }
    if let Some(value) = target
        .identity_file
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        args.extend(["-i".to_string(), value.to_string()]);
    }
    if let Some(value) = target
        .identity_agent
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        args.extend(["-o".to_string(), format!("IdentityAgent={value}")]);
    }
    if let Some(value) = target.identities_only {
        args.extend([
            "-o".to_string(),
            format!("IdentitiesOnly={}", if value { "yes" } else { "no" }),
        ]);
    }
    if let Some(value) = target
        .proxy_command
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        args.extend(["-o".to_string(), format!("ProxyCommand={value}")]);
    }
    if let Some(value) = target
        .jump_host
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        args.extend(["-J".to_string(), value.to_string()]);
    }
    let destination_host = target
        .config_host
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| target.host.trim());
    args.push("--".to_string());
    args.push(if target.username.is_empty() {
        destination_host.to_string()
    } else {
        format!("{}@{destination_host}", target.username)
    });
    args
}

pub async fn run_recipe_remote_command(
    target: crate::ephemeral_vm::RecipeSshTarget,
    project_root: String,
    command: String,
    input: Option<Vec<u8>>,
    max_output_bytes: u64,
) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        if !project_root.starts_with('/') {
            return Err("SSH project root must be an absolute POSIX path.".to_string());
        }
        let mut args = recipe_batch_args(&target);
        args.push(format!(
            "cd -- {} && {command}",
            shell_quote(&project_root)?
        ));
        run_command_with_input(
            &ssh_binary(),
            &args,
            PROJECT_SETUP_TIMEOUT,
            input.as_deref(),
            max_output_bytes,
        )
    })
    .await
    .map_err(|error| format!("SSH command task failed: {error}"))?
}

pub fn recipe_terminal_spawn(
    target: &crate::ephemeral_vm::RecipeSshTarget,
    project_path: &str,
    remote_program: Option<RemoteProgram<'_>>,
    rows: u16,
    cols: u16,
) -> Result<suaegi_term::pty::PtySpawn, String> {
    if !project_path.starts_with('/') {
        return Err("Recipe project path must be an absolute POSIX path for SSH.".to_string());
    }
    let mut args = Vec::new();
    if target.port != 22 {
        args.extend(["-p".to_string(), target.port.to_string()]);
    }
    if let Some(config_host) = target
        .config_host
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if config_host != target.host.trim() {
            args.extend(["-o".to_string(), format!("HostName={}", target.host.trim())]);
        }
    }
    if let Some(identity_file) = target
        .identity_file
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        args.extend(["-i".to_string(), identity_file.to_string()]);
    }
    if let Some(identity_agent) = target
        .identity_agent
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        args.extend(["-o".to_string(), format!("IdentityAgent={identity_agent}")]);
    }
    if let Some(identities_only) = target.identities_only {
        args.extend([
            "-o".to_string(),
            format!(
                "IdentitiesOnly={}",
                if identities_only { "yes" } else { "no" }
            ),
        ]);
    }
    if let Some(proxy_command) = target
        .proxy_command
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        args.extend(["-o".to_string(), format!("ProxyCommand={proxy_command}")]);
    }
    if let Some(jump_host) = target
        .jump_host
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        args.extend(["-J".to_string(), jump_host.to_string()]);
    }
    if let Some(forwards) = &target.port_forwards {
        for forward in forwards {
            args.extend([
                "-L".to_string(),
                format!(
                    "{}:{}:{}",
                    forward.local_port, forward.remote_host, forward.remote_port
                ),
            ]);
        }
    }
    args.push("-t".to_string());
    let destination_host = target
        .config_host
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| target.host.trim());
    let destination = if target.username.is_empty() {
        destination_host.to_string()
    } else {
        format!("{}@{destination_host}", target.username)
    };
    args.push("--".to_string());
    args.push(destination);
    let path = shell_quote(project_path)?;
    let remote_command = if let Some((program, program_args, environment)) = remote_program {
        let mut command = format!("cd -- {path} && exec env");
        for (name, value) in environment {
            if !valid_environment_name(name) || value.contains('\0') {
                continue;
            }
            command.push(' ');
            command.push_str(name);
            command.push('=');
            command.push_str(&shell_quote(value)?);
        }
        command.push(' ');
        command.push_str(&shell_quote(program)?);
        for argument in program_args {
            command.push(' ');
            command.push_str(&shell_quote(argument)?);
        }
        command
    } else {
        format!("cd -- {path} && exec \"${{SHELL:-/bin/sh}}\" -l")
    };
    args.push(remote_command);
    Ok(suaegi_term::pty::PtySpawn {
        program: ssh_binary().to_string_lossy().into_owned(),
        args,
        cwd: None,
        env: Vec::new(),
        env_remove: Vec::new(),
        rows,
        cols,
    })
}

fn valid_environment_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte == b'_' || byte.is_ascii_alphabetic())
        && bytes.all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
}

pub async fn test_connection(target: SshHostSetting) -> SshConnectionCheck {
    tokio::task::spawn_blocking(move || {
        if target.hostname.trim().is_empty() && target.config_host.trim().is_empty() {
            return SshConnectionCheck {
                success: false,
                message: "Host or SSH config alias is required.".to_string(),
            };
        }
        let mut args = command_args(&target, true);
        args.push("true".to_string());
        match run_command(&ssh_binary(), &args, COMMAND_TIMEOUT) {
            Ok(_) => SshConnectionCheck {
                success: true,
                message: "Connection successful.".to_string(),
            },
            Err(error) => SshConnectionCheck {
                success: false,
                message: error,
            },
        }
    })
    .await
    .unwrap_or_else(|error| SshConnectionCheck {
        success: false,
        message: format!("SSH test task failed: {error}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> SshHostSetting {
        SshHostSetting {
            id: "id".into(),
            label: "Build".into(),
            config_host: String::new(),
            hostname: "build.example.com".into(),
            user: "deploy".into(),
            port: 2222,
            identity_file: "/tmp/key".into(),
            proxy_command: String::new(),
            jump_host: "bastion.example.com".into(),
            system_ssh_connection_reuse: false,
            relay_grace_period_seconds: 0,
            source: "manual".into(),
        }
    }

    #[test]
    fn parser_keeps_only_concrete_host_aliases() {
        assert_eq!(
            concrete_aliases(
                "Host *\n  User james\nHost build staging\nHost !blocked *.internal\nHost build\n"
            ),
            vec!["build", "staging"]
        );
    }

    #[test]
    fn command_args_preserve_advanced_connection_fields_without_a_shell() {
        let args = command_args(&target(), true);
        assert!(args.windows(2).any(|pair| pair == ["-p", "2222"]));
        assert!(args.windows(2).any(|pair| pair == ["-i", "/tmp/key"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["-J", "bastion.example.com"]));
        assert_eq!(
            args.last().map(String::as_str),
            Some("deploy@build.example.com")
        );
    }

    #[test]
    fn shell_quote_and_clone_paths_do_not_allow_argument_injection() {
        assert_eq!(
            shell_quote("/srv/O'Brien/repo").unwrap(),
            "'/srv/O'\"'\"'Brien/repo'"
        );
        assert_eq!(
            remote_clone_path("/srv/projects", "git@github.com:stablyai/orca.git").unwrap(),
            "/srv/projects/orca"
        );
        assert!(remote_clone_path("relative", "https://example.com/a.git").is_err());
        assert!(remote_clone_path("/srv", "https://example.com/--upload-pack.git").is_err());
        let command = interactive_project_command(&target(), "/srv/O'Brien/repo").unwrap();
        assert!(command.starts_with("ssh "));
        assert!(command.contains("'-t'"));
        assert!(command.contains("cd --"));
        assert!(command.ends_with('\r'));
    }

    #[test]
    fn recipe_terminal_spawn_keeps_ssh_options_as_argv_and_remote_values_quoted() {
        let target = crate::ephemeral_vm::RecipeSshTarget {
            label: "VM".into(),
            config_host: None,
            host: "vm.example.com".into(),
            port: 2222,
            username: "runner".into(),
            identity_file: Some("/tmp/key".into()),
            identity_agent: None,
            identities_only: Some(true),
            proxy_command: None,
            jump_host: None,
            relay_grace_period_seconds: None,
            port_forwards: None,
        };
        let args = vec!["--prompt".into(), "O'Brien".into()];
        let env = vec![("SAFE_NAME".into(), "a b".into())];
        let spawn = recipe_terminal_spawn(
            &target,
            "/workspace/O'Brien",
            Some(("agent", &args, &env)),
            40,
            100,
        )
        .unwrap();
        assert_eq!(spawn.program, "/usr/bin/ssh");
        assert!(spawn.args.windows(2).any(|pair| pair == ["-p", "2222"]));
        assert_eq!(spawn.args[spawn.args.len() - 2], "runner@vm.example.com");
        let command = spawn.args.last().unwrap();
        assert!(command.contains("cd -- '/workspace/O'\"'\"'Brien'"));
        assert!(command.contains("SAFE_NAME='a b'"));
        assert!(command.contains("'O'\"'\"'Brien'"));
    }
}
