//! PTY-compatible child process that bridges a local Suaegi terminal pane to
//! an Orca server's encrypted `terminal.*` RPC stream.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use suaegi_core::domain::RuntimeEnvironmentSetting;
use suaegi_term::pty::PtySpawn;

const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
pub type RemoteProgram<'a> = (&'a str, &'a [String], &'a [(String, String)]);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BridgeConfig {
    environment: RuntimeEnvironmentSetting,
    worktree: String,
    command: Option<String>,
    #[serde(default)]
    env: Vec<(String, String)>,
    rows: u16,
    cols: u16,
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    if value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"_+-./:@%=".contains(&byte))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn command_line(program: &str, args: &[String]) -> String {
    std::iter::once(program)
        .chain(args.iter().map(String::as_str))
        .map(shell_quote)
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn spawn(
    environment: RuntimeEnvironmentSetting,
    worktree: &str,
    program: Option<RemoteProgram<'_>>,
    rows: u16,
    cols: u16,
) -> Result<PtySpawn, String> {
    let (command, env) = program.map_or((None, Vec::new()), |(program, args, env)| {
        (Some(command_line(program, args)), env.to_vec())
    });
    let config = BridgeConfig {
        environment,
        worktree: worktree.to_string(),
        command,
        env,
        rows,
        cols,
    };
    let file = tempfile::Builder::new()
        .prefix("suaegi-runtime-terminal-")
        .suffix(".json")
        .tempfile()
        .map_err(|error| format!("Could not create remote terminal configuration: {error}"))?;
    serde_json::to_writer(file.as_file(), &config)
        .map_err(|error| format!("Could not encode remote terminal configuration: {error}"))?;
    let config_path = file
        .into_temp_path()
        .keep()
        .map_err(|error| format!("Could not retain remote terminal configuration: {error}"))?;
    let executable = std::env::current_exe()
        .map_err(|error| format!("Could not locate the Suaegi executable: {error}"))?;
    Ok(PtySpawn {
        program: executable.to_string_lossy().into_owned(),
        args: vec![
            "--runtime-terminal-bridge".to_string(),
            "--config".to_string(),
            config_path.to_string_lossy().into_owned(),
        ],
        cwd: None,
        env: Vec::new(),
        env_remove: Vec::new(),
        rows,
        cols,
    })
}

pub fn config_path_from_args(
    mut args: impl Iterator<Item = std::ffi::OsString>,
) -> Result<PathBuf, String> {
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--config")) {
        return Err("expected --config after --runtime-terminal-bridge".to_string());
    }
    args.next()
        .map(PathBuf::from)
        .ok_or_else(|| "missing remote terminal bridge configuration".to_string())
}

pub fn run(config_path: &Path) -> Result<(), String> {
    let metadata = fs::metadata(config_path)
        .map_err(|error| format!("Could not inspect remote terminal configuration: {error}"))?;
    if !metadata.is_file() || metadata.len() > MAX_CONFIG_BYTES {
        return Err("Remote terminal configuration is invalid or too large.".to_string());
    }
    let bytes = fs::read(config_path)
        .map_err(|error| format!("Could not read remote terminal configuration: {error}"))?;
    fs::remove_file(config_path)
        .map_err(|error| format!("Could not remove remote terminal configuration: {error}"))?;
    let config: BridgeConfig = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Remote terminal configuration is invalid: {error}"))?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("Could not start remote terminal runtime: {error}"))?;
    runtime.block_on(async move {
        let result = crate::remote_runtime::request(
            config.environment.clone(),
            "terminal.create",
            serde_json::json!({
                "worktree": config.worktree,
                "command": config.command,
                "env": config.env.into_iter().collect::<std::collections::BTreeMap<_, _>>(),
                "background": false,
                "viewport": {"rows": config.rows, "cols": config.cols}
            }),
            std::time::Duration::from_secs(30),
        )
        .await?;
        let terminal = result
            .pointer("/terminal/handle")
            .or_else(|| result.get("handle"))
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "Remote runtime did not return a terminal handle.".to_string())?
            .to_string();

        let (sender, receiver) =
            tokio::sync::mpsc::channel::<crate::remote_runtime::TerminalStreamInput>(64);
        let input_open = Arc::new(AtomicBool::new(true));
        let input_open_reader = Arc::clone(&input_open);
        let input_sender = sender.clone();
        std::thread::Builder::new()
            .name("suaegi-runtime-terminal-input".to_string())
            .spawn(move || {
                let mut stdin = std::io::stdin();
                let mut buffer = [0_u8; 8192];
                loop {
                    match stdin.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => {
                            if input_sender
                                .blocking_send(crate::remote_runtime::TerminalStreamInput::Data(
                                    buffer[..read].to_vec(),
                                ))
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                }
                input_open_reader.store(false, Ordering::Release);
            })
            .map_err(|error| format!("Could not start remote terminal input: {error}"))?;

        #[cfg(unix)]
        {
            let resize_sender = sender.clone();
            let resize_open = Arc::clone(&input_open);
            let initial_size = (config.rows, config.cols);
            std::thread::Builder::new()
                .name("suaegi-runtime-terminal-resize".to_string())
                .spawn(move || {
                    let mut previous = initial_size;
                    while resize_open.load(Ordering::Acquire) {
                        let mut size = libc::winsize {
                            ws_row: 0,
                            ws_col: 0,
                            ws_xpixel: 0,
                            ws_ypixel: 0,
                        };
                        // SAFETY: `size` points to a valid writable winsize and
                        // STDIN is the bridge's PTY slave.
                        let read =
                            unsafe { libc::ioctl(libc::STDIN_FILENO, libc::TIOCGWINSZ, &mut size) };
                        let current = (size.ws_row, size.ws_col);
                        if read == 0 && current.0 > 0 && current.1 > 0 && current != previous {
                            previous = current;
                            if resize_sender
                                .blocking_send(crate::remote_runtime::TerminalStreamInput::Resize {
                                    rows: current.0,
                                    cols: current.1,
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                        std::thread::sleep(std::time::Duration::from_millis(150));
                    }
                })
                .map_err(|error| format!("Could not start remote terminal resize: {error}"))?;
        }
        drop(sender);
        crate::remote_runtime::stream_terminal(
            config.environment,
            terminal,
            config.rows,
            config.cols,
            receiver,
        )
        .await
    })
}

#[cfg(test)]
mod tests {
    use super::{command_line, shell_quote};

    #[test]
    fn quotes_remote_commands_without_shell_injection() {
        assert_eq!(shell_quote("plain/path"), "plain/path");
        assert_eq!(shell_quote("it's here"), "'it'\"'\"'s here'");
        assert_eq!(
            command_line("agent command", &["--prompt".into(), "hello world".into()]),
            "'agent command' --prompt 'hello world'"
        );
    }
}
