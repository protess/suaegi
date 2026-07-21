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

/// `probe()`는 running을 관찰한 *뒤에* exit_code를 다시 읽어야 한다. 먼저 읽은
/// exit_code(None)를 재사용해 -1로 단정하면, 그 두 읽기 사이에 리더가 실제
/// 코드를 발행해버리는 좁은 창에서 진짜 코드(7)를 -1로 잘못 보고한다.
///
/// 이 테스트는 흔한 경로(수렴 후 probe)만 검증한다: 미리 `exit_code() ==
/// Some(7)`을 기다리므로 `probe()`가 이미 끝난 상태를 관찰해 빠르게 반환하고,
/// 좁은 레이스 창의 코드는 지나가지 않는다. 그 창은 공개 API로는 결정적으로
/// 재현할 수 없다 — running을 먼저 읽고 exit_code를 나중에 읽는 문서화된
/// 읽기 순서 자체가 그 창에서의 정확성 근거이며, 이 테스트가 아니라 그
/// 순서 논증이 레이스 분기를 보증한다.
#[test]
fn exited_session_reports_the_real_exit_code_not_a_placeholder() {
    let session = TerminalSession::start(spec(platform::exit_with(7))).unwrap();
    assert!(wait_until(Duration::from_secs(10), || session.exit_code() == Some(7)));
    let mut monitor = PresenceMonitor::default();
    let probe = FakeProbe(HashMap::new());
    assert_eq!(
        monitor.probe(&session, &probe),
        AgentPresence::Exited { code: 7 }
    );
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
