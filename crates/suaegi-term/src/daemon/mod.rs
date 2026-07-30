//! Detached PTY holder protocol and runtime.
//!
//! The daemon owns PTYs while the GUI keeps terminal emulation locally. Output
//! is retained as a bounded raw-byte tail and replayed when a client attaches.

#[cfg(unix)]
mod client;
#[cfg(not(unix))]
#[path = "client_unsupported.rs"]
mod client;
mod protocol;
#[cfg(unix)]
mod server;

pub use client::{
    configure, configured, default_runtime_dir, kill_all_sessions, kill_session, list_sessions,
    restart, session_foreground_pgid, DaemonClientSession, DaemonConfiguration, DaemonReader,
};
pub use protocol::{SessionInfo, SpawnSpec};

use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error("daemon is not configured")]
    NotConfigured,
    #[error("daemon protocol is unsupported on this platform")]
    UnsupportedPlatform,
    #[error("daemon I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("daemon protocol: {0}")]
    Protocol(String),
    #[error("daemon rejected request: {0}")]
    Rejected(String),
    #[error("failed to start daemon: {0}")]
    Start(String),
}

/// Runs the PTY holder until its listener stops.
///
/// The application binary calls this for its private `--pty-daemon` mode.
pub fn run(runtime_dir: &Path) -> Result<(), DaemonError> {
    #[cfg(unix)]
    {
        server::run(runtime_dir)
    }
    #[cfg(not(unix))]
    {
        let _ = runtime_dir;
        Err(DaemonError::UnsupportedPlatform)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::io::Read;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::pty::PtySpawn;
    use crate::session::{SessionSpec, TerminalSession};

    #[test]
    fn daemon_round_trips_pty_io_and_lists_the_live_session() {
        let runtime_dir = tempfile::tempdir().unwrap().keep();
        let server_dir = runtime_dir.clone();
        std::thread::spawn(move || {
            server::run(&server_dir).unwrap();
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        while (!runtime_dir.join("pty-v1.sock").exists()
            || !runtime_dir.join("pty-v1.token").exists())
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(runtime_dir.join("pty-v1.sock").exists());

        configure(DaemonConfiguration {
            executable: "/bin/false".into(),
            runtime_dir,
        })
        .unwrap();
        let spec = SpawnSpec {
            program: "/bin/sh".to_string(),
            args: vec![
                "-c".to_string(),
                "printf 'ready\\n'; IFS= read -r line; printf 'got:%s\\n' \"$line\"".to_string(),
            ],
            cwd: None,
            env: Vec::new(),
            rows: 24,
            cols: 80,
        };
        let (session, mut reader, is_new) =
            DaemonClientSession::create_or_attach("daemon-test".to_string(), spec.clone()).unwrap();
        assert!(is_new);
        let mut first_output = [0_u8; 1024];
        let first_read = reader.read(&mut first_output).unwrap();
        assert!(String::from_utf8_lossy(&first_output[..first_read]).contains("ready"));
        session.disconnect();
        drop(reader);

        let (session, mut reader, is_new) =
            DaemonClientSession::create_or_attach("daemon-test".to_string(), spec.clone()).unwrap();
        assert!(!is_new);
        let replay_read = reader.read(&mut first_output).unwrap();
        assert!(
            String::from_utf8_lossy(&first_output[..replay_read]).contains("ready"),
            "reattach did not replay the retained output"
        );
        session.write(b"hello\n").unwrap();

        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut output = Vec::new();
            let mut chunk = [0_u8; 1024];
            while let Ok(read) = reader.read(&mut chunk) {
                if read == 0 {
                    break;
                }
                output.extend_from_slice(&chunk[..read]);
                if output.windows(9).any(|window| window == b"got:hello") {
                    break;
                }
            }
            let _ = sender.send(output);
        });
        let output = receiver.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(
            String::from_utf8_lossy(&output).contains("got:hello"),
            "output was {:?}",
            String::from_utf8_lossy(&output)
        );

        let sessions = list_sessions().unwrap();
        let info = sessions
            .iter()
            .find(|info| info.session_id == "daemon-test")
            .unwrap();
        assert_eq!((info.rows, info.cols), (24, 80));

        session.kill().unwrap();
        let (replacement, _, is_new) =
            DaemonClientSession::create_or_attach("daemon-test".to_string(), spec).unwrap();
        assert!(is_new, "an explicitly closed session id must be reusable");
        replacement.kill().unwrap();

        let query_spec = SessionSpec {
            pty: PtySpawn {
                program: "/bin/sh".to_string(),
                args: vec![
                    "-c".to_string(),
                    "stty raw -echo; printf '\\033[c'; dd bs=1 count=5 2>/dev/null; \
                     printf 'first\\r\\n'; dd bs=1 count=5 2>/dev/null; printf 'duplicate\\r\\n'"
                        .to_string(),
                ],
                cwd: None,
                env: Vec::new(),
                env_remove: Vec::new(),
                rows: 24,
                cols: 80,
            },
            scrollback: 200,
        };
        let first =
            TerminalSession::start_persistent(query_spec.clone(), "query-replay-test".to_string())
                .unwrap();
        assert!(!first.was_reattached());
        let deadline = Instant::now() + Duration::from_secs(3);
        while !snapshot_text(&first).contains("first") && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(snapshot_text(&first).contains("first"));
        drop(first);

        let attached =
            TerminalSession::start_persistent(query_spec, "query-replay-test".to_string()).unwrap();
        assert!(attached.was_reattached());
        std::thread::sleep(Duration::from_millis(200));
        let replayed = snapshot_text(&attached);
        assert!(replayed.contains("first"));
        assert!(
            !replayed.contains("duplicate"),
            "historical terminal queries must not be answered again"
        );
        attached.kill().unwrap();
    }

    fn snapshot_text(session: &TerminalSession) -> String {
        let snapshot = session.snapshot();
        (0..snapshot.rows.len())
            .map(|row| snapshot.row_text(row))
            .collect::<Vec<_>>()
            .join("\n")
    }
}
