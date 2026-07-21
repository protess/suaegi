mod platform;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use suaegi_term::agent::AgentKind;
use suaegi_term::presence::{AgentPresence, PresenceMonitor, ProcessProbe};
use suaegi_term::pty::PtySpawn;
use suaegi_term::session::{SessionSpec, TerminalSession};

struct FakeProbe(HashMap<i32, String>);

impl ProcessProbe for FakeProbe {
    fn command_line(&self, pid: i32) -> Option<String> {
        self.0.get(&pid).cloned()
    }
}

struct CountingProbe {
    line: String,
    calls: std::cell::Cell<usize>,
}

impl ProcessProbe for CountingProbe {
    fn command_line(&self, _pid: i32) -> Option<String> {
        self.calls.set(self.calls.get() + 1);
        Some(self.line.clone())
    }
}

fn spec(cmd: (String, Vec<String>)) -> SessionSpec {
    SessionSpec {
        pty: PtySpawn {
            program: cmd.0,
            args: cmd.1,
            cwd: None,
            env: Vec::new(),
            rows: 24,
            cols: 80,
        },
        scrollback: 100,
    }
}

fn wait_until<F: FnMut() -> bool>(timeout: Duration, mut cond: F) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
fn exited_session_reports_exited() {
    let session = TerminalSession::start(spec(platform::exit_with(0))).unwrap();
    assert!(wait_until(Duration::from_secs(10), || !session.is_running()));
    let mut monitor = PresenceMonitor::default();
    let probe = FakeProbe(HashMap::new());
    assert!(matches!(
        monitor.probe(&session, &probe),
        AgentPresence::Exited { .. }
    ));
}

#[cfg(not(unix))]
#[test]
fn non_unix_reports_unknown_while_running() {
    let session = TerminalSession::start(spec(platform::sleep_seconds(30))).unwrap();
    let mut monitor = PresenceMonitor::default();
    let probe = FakeProbe(HashMap::new());
    assert_eq!(monitor.probe(&session, &probe), AgentPresence::Unknown);
}

#[cfg(unix)]
mod unix_only {
    use super::*;

    fn running_session() -> (TerminalSession, i32) {
        let session = TerminalSession::start(spec(platform::sleep_seconds(30))).unwrap();
        assert!(wait_until(Duration::from_secs(10), || session
            .foreground_pgid()
            .is_some()));
        let pgid = session.foreground_pgid().unwrap();
        (session, pgid)
    }

    #[test]
    fn non_agent_foreground_reports_no_agent() {
        let (session, pgid) = running_session();
        let probe = FakeProbe(HashMap::from([(pgid, "/bin/zsh".to_string())]));
        let mut monitor = PresenceMonitor::default();
        assert_eq!(monitor.probe(&session, &probe), AgentPresence::NoAgent);
    }

    #[test]
    fn agent_foreground_reports_the_agent() {
        let (session, pgid) = running_session();
        let probe = FakeProbe(HashMap::from([(pgid, "claude --resume".to_string())]));
        let mut monitor = PresenceMonitor::default();
        assert_eq!(
            monitor.probe(&session, &probe),
            AgentPresence::Agent(AgentKind::Claude)
        );
    }

    #[test]
    fn unresolvable_process_reports_unknown_not_no_agent() {
        // "모른다"와 "에이전트가 아니다"는 다른 상태다
        let (session, _) = running_session();
        let probe = FakeProbe(HashMap::new());
        let mut monitor = PresenceMonitor::default();
        assert_eq!(monitor.probe(&session, &probe), AgentPresence::Unknown);
    }

    #[test]
    fn repeat_probe_of_the_same_agent_pgid_is_cached() {
        let (session, _) = running_session();
        let probe = CountingProbe {
            line: "claude".to_string(),
            calls: std::cell::Cell::new(0),
        };
        let mut monitor = PresenceMonitor::default();
        monitor.probe(&session, &probe);
        monitor.probe(&session, &probe);
        assert_eq!(probe.calls.get(), 1, "same agent pgid must not re-probe");
    }

    #[test]
    fn non_agent_result_is_not_cached() {
        // 셸이 나중에 에이전트를 exec하는 전환을 놓치면 안 된다
        let (session, _) = running_session();
        let probe = CountingProbe {
            line: "/bin/zsh".to_string(),
            calls: std::cell::Cell::new(0),
        };
        let mut monitor = PresenceMonitor::default();
        monitor.probe(&session, &probe);
        monitor.probe(&session, &probe);
        assert_eq!(probe.calls.get(), 2, "non-agent result must be re-probed");
    }

    #[test]
    fn ps_probe_resolves_our_own_process() {
        use suaegi_term::presence::PsProbe;
        let probe = PsProbe;
        let me = std::process::id() as i32;
        let line = probe.command_line(me).expect("ps should resolve our pid");
        assert!(!line.trim().is_empty());
    }
}
