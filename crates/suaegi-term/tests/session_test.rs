mod platform;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use suaegi_term::grid::TitleChange;
use suaegi_term::pty::PtySpawn;
use suaegi_term::session::{SessionSpec, TerminalSession};

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
        scrollback: 500,
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

fn snapshot_contains(session: &TerminalSession, needle: &str) -> bool {
    let snap = session.snapshot();
    (0..snap.size.rows).any(|r| snap.row_text(r).contains(needle))
}

#[test]
fn child_output_reaches_the_grid() {
    let session = TerminalSession::start(spec(platform::echo("from-session"))).unwrap();
    assert!(wait_until(Duration::from_secs(10), || snapshot_contains(
        &session,
        "from-session"
    )));
}

#[test]
fn generation_increases_when_output_arrives() {
    let session = TerminalSession::start(spec(platform::echo("gen"))).unwrap();
    assert!(wait_until(Duration::from_secs(10), || session.generation() > 0));
}

#[test]
fn exit_code_is_reported_after_child_finishes() {
    let session = TerminalSession::start(spec(platform::exit_with(7))).unwrap();
    assert!(wait_until(Duration::from_secs(10), || session.exit_code()
        == Some(7)));
    assert!(!session.is_running());
}

#[test]
fn write_is_echoed_into_the_grid() {
    let session = TerminalSession::start(spec(platform::echo_stdin())).unwrap();
    assert!(session.write(b"typed-text\n".to_vec()), "queued write");
    assert!(wait_until(Duration::from_secs(10), || snapshot_contains(
        &session,
        "typed-text"
    )));
}

#[test]
fn resize_updates_both_pty_and_grid() {
    let session = TerminalSession::start(spec(platform::echo_stdin())).unwrap();
    session.resize(30, 100).unwrap();
    let snap = session.snapshot();
    assert_eq!(snap.size.rows, 30);
    assert_eq!(snap.size.cols, 100);
}

#[test]
fn zero_size_resize_is_ignored() {
    let session = TerminalSession::start(spec(platform::echo_stdin())).unwrap();
    session.resize(0, 0).unwrap();
    assert_eq!(
        session.snapshot().size.rows,
        24,
        "degenerate size must not reach the grid"
    );
}

#[test]
fn title_escape_is_surfaced() {
    let script = "printf '\\033]0;session-title\\007'";
    let session = TerminalSession::start(spec(platform::shell_command(script))).unwrap();
    let mut seen = Vec::new();
    assert!(
        wait_until(Duration::from_secs(10), || {
            seen.extend(session.take_title_changes());
            seen.contains(&TitleChange::Set("session-title".to_string()))
        }),
        "title change was not surfaced"
    );
}

#[test]
fn dropping_the_session_does_not_block() {
    // Drop이 자식을 죽이지 않으면 이 프로세스는 60초간 살아남는다
    let session = TerminalSession::start(spec(platform::sleep_seconds(60))).unwrap();
    let start = Instant::now();
    drop(session);
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "drop must not block on a long-running child"
    );
}

/// unix에서는 그룹 SIGKILL로 자손까지 확실히 죽는다는 보장이 있다.
/// (Windows는 자손 종료 수단이 없어 이 보장을 하지 않는다 — Global Constraints 참고)
#[cfg(unix)]
#[test]
fn dropping_the_session_kills_the_process_group() {
    let session = TerminalSession::start(spec(platform::sleep_seconds(60))).unwrap();
    assert!(wait_until(Duration::from_secs(10), || session
        .foreground_pgid()
        .is_some()));
    let pgid = session.foreground_pgid().unwrap();
    drop(session);
    assert!(
        wait_until(Duration::from_secs(5), || {
            // kill(pgid, 0)이 실패하면(ESRCH) 그룹이 사라진 것
            (unsafe { libc::killpg(pgid as libc::pid_t, 0) }) != 0
        }),
        "process group survived session drop"
    );
}

/// 가장 높은 `REPLY-<n>` 마커의 `n`을 뷰포트에서 찾는다. 마커가 없으면 0.
#[cfg(unix)]
fn max_reply_index(session: &TerminalSession) -> u64 {
    let snap = session.snapshot();
    (0..snap.size.rows)
        .filter_map(|r| snap.row_text(r).strip_prefix("REPLY-").map(str::to_string))
        .filter_map(|n| n.trim().parse::<u64>().ok())
        .max()
        .unwrap_or(0)
}

/// 두 큐 설계의 핵심 보장: UI 쓰기 큐가 포화돼도 리더는 장치 응답을
/// 블로킹 없이 넘길 수 있어야 하고, 그래야 PTY 출력 소비도 멈추지 않는다.
/// 자식이 `\033[c`(DA1) 질의를 반복해서 보내고 그 응답이 돌아올 때까지
/// 기다렸다가만 다음 마커를 찍게 만든다 — 이러면 마커 진행 자체가 "리더가
/// 응답을 큐에 실제로 올려보냈다"는 증거가 된다. `tick`만 반복 출력하는
/// 자식으로는 리더가 응답 큐를 한 번도 건드리지 않아 큐 설계와 무관하게
/// 통과해버린다(실측: 두 큐를 하나의 공유 바운드 채널로 합쳐도 이전 버전은
/// 계속 통과했다) — 그래서 반드시 응답 왕복이 있어야 한다.
///
/// `write()`가 실제로 `false`를 반환하는지 먼저 확인해 포화가 실제로
/// 일어났음을 검증하고, 그 뒤로도 관찰 구간 내내 큐를 계속 채워 넣으면서
/// (한 번 포화됐다가 라이터가 서서히 비워내면 낡은 설계도 운 좋게 통과할 수
/// 있다) 마커가 계속 증가하는지 본다.
#[cfg(unix)]
#[test]
fn saturated_write_queue_does_not_stall_the_reader() {
    let script = "stty -icanon min 1 time 0 -echo; i=0; \
                   while true; do \
                     printf '\\033[c'; \
                     dd bs=1 count=5 >/dev/null 2>&1; \
                     i=$((i+1)); \
                     printf 'REPLY-%d\\n' \"$i\"; \
                   done";
    let session = TerminalSession::start(spec(platform::shell_command(script))).unwrap();

    // 리더가 실제로 질의/응답 왕복을 수행하는 상태에서 시작한다 — 그렇지
    // 않으면 이 테스트는 공허하게 통과한다.
    assert!(
        wait_until(Duration::from_secs(10), || max_reply_index(&session) >= 1),
        "child never completed a device-query/reply round trip before \
         saturation attempt; this test would be vacuous"
    );

    // 큐를 포화시킨다. try_send는 논블로킹이므로 이 루프 자체는 빠르게 끝난다;
    // 실제로 막히는 건 백그라운드 라이터 스레드의 블로킹 pty write()다.
    let mut saturated = false;
    for i in 0..4000u32 {
        let payload = format!("payload-{i}\n").into_bytes();
        if !session.write(payload) {
            saturated = true;
            break;
        }
    }
    assert!(
        saturated,
        "write queue never reported full — this child may be draining stdin, \
         which would make the rest of this test vacuous"
    );

    // 관찰 구간 내내 큐를 항상 가득 찬 상태로 유지해야 한다 — 한 번만
    // 채우고 라이터가 서서히 비우게 두면, 큐 하나만 쓰는 설계에서도 응답이
    // (뒤에서긴 하지만) 결국 차례가 와 통과해버릴 수 있다. 별도 스레드가
    // sleep 없이 계속 `write()`를 재시도해 빈 슬롯이 나는 즉시 다시 채운다.
    // 리더는 이 배경 스레드와 완전히 독립적으로, 계속 응답을 넘기고 PTY를
    // 읽어야 한다 — 두 큐를 분리한 이유 그 자체다.
    let replies_before = max_reply_index(&session);
    let stop = AtomicBool::new(false);
    let progressed = std::thread::scope(|scope| {
        scope.spawn(|| {
            let mut i = 0u32;
            while !stop.load(Ordering::Relaxed) {
                session.write(format!("flood-{i}\n").into_bytes());
                i = i.wrapping_add(1);
            }
        });

        let progressed = wait_until(Duration::from_secs(5), || {
            max_reply_index(&session) > replies_before
        });
        stop.store(true, Ordering::Relaxed);
        progressed
    });
    assert!(
        progressed,
        "child made no further device-reply round trips while the UI write \
         queue stayed continuously saturated — the reader appears stalled \
         trying to hand off a reply"
    );
    assert!(
        session.is_running(),
        "session should still be running — the writer is contending for \
         pty bandwidth, not dead"
    );
}

/// 장치 질의(DA1) 응답이 PTY로 되돌아가야 한다. 응답에는 개행이 없으므로
/// **먼저 라인 디시플린을 비정규 모드로 바꿔야** 한다 — 그러지 않으면 `dd`든
/// `read`든 개행이 올 때까지 커널에서 블로킹된다.
#[cfg(unix)]
#[test]
fn device_query_is_answered_back_to_the_pty() {
    let script = "stty -icanon min 1 time 0 -echo; printf '\\033[c'; \
                  dd bs=1 count=3 >/dev/null 2>&1; printf 'ANSWERED'";
    let session = TerminalSession::start(spec(platform::shell_command(script))).unwrap();
    assert!(
        wait_until(Duration::from_secs(10), || snapshot_contains(
            &session,
            "ANSWERED"
        )),
        "device query reply never reached the child"
    );
}
