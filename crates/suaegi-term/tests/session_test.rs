mod platform;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use alacritty_terminal::index::Side;
use suaegi_term::grid::TitleChange;
use suaegi_term::input_types::{
    ClickKind, KeyInput, KeyLocation, Mods, MouseAction, MouseIntent, NamedKey, TermKey,
    ViewportHit, WriteOutcome,
};
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
    assert!(wait_until(Duration::from_secs(10), || session.exit_code() == Some(7)));
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

/// resize()는 &self로 동시 호출될 수 있다(Sync). pty.resize와 grid.resize가
/// 한 락으로 직렬화되지 않으면 두 호출이 인터리브돼(PTY=A, PTY=B, grid=B,
/// grid=A) pty와 grid가 서로 다른 크기로 영구히 어긋날 수 있다. 여러 스레드가
/// 서로 다른 크기로 동시에 resize를 반복해도, 각 라운드가 끝난 뒤에는 항상
/// pty와 grid가 같은 크기를 보고해야 한다.
#[test]
fn concurrent_resizes_never_leave_the_pty_and_grid_disagreeing() {
    let session = TerminalSession::start(spec(platform::echo_stdin())).unwrap();
    let candidates: [(u16, u16); 4] = [(24, 80), (30, 100), (50, 132), (20, 60)];

    for round in 0..100u32 {
        std::thread::scope(|scope| {
            for &(rows, cols) in &candidates {
                let session = &session;
                scope.spawn(move || {
                    session.resize(rows, cols).unwrap();
                });
            }
        });

        let grid_size = session.snapshot().size;
        let pty_size = session.pty_size().unwrap();
        assert_eq!(
            (grid_size.rows as u16, grid_size.cols as u16),
            pty_size,
            "pty and grid disagree after concurrent resize round {round}"
        );
    }
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
/// 중요: 마커는 **DA1 응답 자체를 관찰했을 때만** 찍는다. 이전 버전은
/// `dd bs=1 count=5`로 "아무 5바이트"만 세었는데, 플러드 중에는 그 5바이트가
/// 플러드 페이로드(`flood-N\n`) 자체로도 채워질 수 있어 마커 진행이 "리더가
/// 계속 읽는다"만 증명하고 "응답이 실제로 큐에 올라가 전달됐다"는 증명하지
/// 못했다 — 응답을 조용히 드롭하는 회귀도 이 테스트를 통과시켰을 것이다.
/// DA1 응답은 ESC(`\x1b`)로 시작하고 플러드 페이로드는 ESC를 포함하지
/// 않으므로, 자식이 청크 단위로 읽으며 ESC가 나타날 때까지 기다렸다가만
/// 마커를 찍게 하면 마커 진행이 곧 "특정 응답이 도착했다"는 증거가 된다.
///
/// `write()`가 실제로 `false`를 반환하는지 먼저 확인해 포화가 실제로
/// 일어났음을 검증하고, 그 뒤로도 관찰 구간 내내 큐를 계속 채워 넣으면서
/// (한 번 포화됐다가 라이터가 서서히 비워내면 낡은 설계도 운 좋게 통과할 수
/// 있다) 마커가 계속 증가하는지 본다.
///
/// 압력 레짐: 이 테스트가 실제로 포화시키는 건 UI 쓰기 큐(용량 256)뿐이다.
/// 자식은 한 번에 질의 하나만 내보내고 그 응답을 볼 때까지 다음 질의를
/// 보내지 않으므로 응답 큐(용량 4096)에는 항상 많아야 한두 개만 쌓인다 —
/// 즉 여기서 관찰하는 마커 정체는 문서화된 "응답 큐 포화 시 드롭" 정책이
/// 아니라 진짜 회귀만을 의미한다.
#[cfg(unix)]
#[test]
fn saturated_write_queue_does_not_stall_the_reader() {
    let script = "stty -icanon min 1 time 0 -echo; \
                   esc=$(printf '\\033'); i=0; \
                   while true; do \
                     printf '\\033[c'; \
                     while true; do \
                       chunk=$(dd bs=64 count=1 2>/dev/null); \
                       case \"$chunk\" in *\"$esc\"*) break ;; esac; \
                     done; \
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

/// 자식이 죽으면 리더가 exit_code/running을 발행하고 자기 reply_tx 사본을
/// drop한다. 세션(과 그 UI 송신자)을 계속 들고 있어도 라이터 스레드가 그
/// 신호를 보고 곧 스스로 끝나야 한다 — 그러지 않으면 끝난 세션마다 20ms
/// 주기로 깨어나는 라이터 스레드가 계속 남는다.
#[test]
fn writer_thread_exits_after_child_death_even_while_session_is_kept_alive() {
    let session = TerminalSession::start(spec(platform::exit_with(0))).unwrap();
    assert!(wait_until(Duration::from_secs(10), || !session.is_running()));
    assert!(
        wait_until(Duration::from_secs(2), || session
            .writer_thread_is_finished()),
        "writer thread should exit shortly after the child dies, even while \
         the session is still alive"
    );
}

/// 프로세스의 RSS(KB). `/proc` 없는 macOS도 지원해야 하므로 `ps`를 쓴다.
#[cfg(unix)]
fn process_rss_kb() -> u64 {
    let pid = std::process::id().to_string();
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid])
        .output()
        .expect("ps should run");
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0)
}

/// 이 테스트가 방지하는 정확한 실패 경로: 자식이 stdin을 전혀 읽지 않으면서
/// 장치 질의(DA1)를 계속 쏟아낸다. 커널 PTY 입력 버퍼가 차면 라이터의 블로킹
/// `pty.write`가 파킹되고, 그 뒤로도 리더는 계속 응답을 만들어 큐에 넣으려
/// 한다. 언바운드 큐였다면 여기서 메모리가 자식의 출력 속도로 무한히 자란다.
/// 바운드 큐 + `try_send` 드롭으로 이를 막았다는 것을,
/// (a) 리더가 여전히 진행 중이고(응답 큐 상태와 무관하게 PTY 출력을 계속
///     소비해 generation이 계속 오른다), (b) 세션 프로세스의 RSS가 관찰 구간
///     동안 유의미하게 자라지 않는다는 두 가지로 확인한다.
#[cfg(unix)]
#[test]
fn flooding_unread_device_queries_does_not_grow_memory_unbounded() {
    // `-icanon`이 없으면 자식이 정규 모드에 머물러, tty 입력 큐가 차면 커널이
    // 응답 바이트를 그냥 버린다 — 라이터가 절대 파킹되지 않고, 이 테스트가
    // 지키려는 실패 경로(파킹된 라이터 뒤로 큐가 무한히 쌓임)가 아예 발동하지
    // 않는다. 비정규 모드로 바꿔야 커널이 응답을 실제로 입력 큐에 채운다.
    let script = "stty -icanon min 1 time 0 -echo; while true; do printf '\\033[c'; done";
    let session = TerminalSession::start(spec(platform::shell_command(script))).unwrap();

    // 리더가 최소 한 번은 PTY 출력을 소비했는지 확인한다 — 그렇지 않으면
    // 아래 관찰이 공허하게 통과한다.
    assert!(
        wait_until(Duration::from_secs(10), || session.generation() > 0),
        "reader never observed any output from the flooding child"
    );

    let rss_before = process_rss_kb();
    let generation_before = session.generation();

    // 커널 tty 입력 버퍼가 채워지고 라이터가 블로킹 write에 파킹될 시간을 준다.
    std::thread::sleep(Duration::from_secs(3));

    let rss_after = process_rss_kb();
    let generation_after = session.generation();

    assert!(
        generation_after > generation_before,
        "reader appears stalled while flooded with unread device queries"
    );
    assert!(
        session.is_running(),
        "session should still be alive while flooded"
    );
    let grew_kb = rss_after.saturating_sub(rss_before);
    // 바운드 큐는 실측상 0.1-0.2MB(100-200KB) 안에서 머문다. 언바운드 큐로
    // 되돌리면 3초에 ~23MB 자란다(실측). 10MB는 둘 사이에 넉넉한 여유를 둔다.
    assert!(
        grew_kb < 10_000,
        "RSS grew by {grew_kb}KB while flooding unread device queries \
         (before={rss_before}KB, after={rss_after}KB) — the reply queue \
         appears unbounded"
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
            &session, "ANSWERED"
        )),
        "device query reply never reached the child"
    );
}

// ---------------------------------------------------------------------------
// 입력 래퍼 — grid가 인코딩하고 session이 큐에 넣는다
// ---------------------------------------------------------------------------

fn wheel(lines: i32) -> MouseIntent {
    MouseIntent {
        action: MouseAction::Wheel { lines },
        hit: ViewportHit {
            row: 0,
            col: 0,
            side: Side::Left,
        },
        held: None,
        mods: Mods::default(),
        click: ClickKind::Single,
        force_local: false,
    }
}

/// 인코딩부터 PTY 도달까지 전 경로. 단위 테스트는 바이트 모양까지만 보므로
/// "큐에 들어갔다"가 "실제로 자식에게 갔다"인지는 여기서만 확인된다.
#[test]
fn send_paste_reaches_the_child_process() {
    let session = TerminalSession::start(spec(platform::echo_stdin())).unwrap();
    assert_eq!(session.send_paste("suaegi-paste\n"), WriteOutcome::Queued);
    assert!(
        wait_until(Duration::from_secs(10), || snapshot_contains(
            &session,
            "suaegi-paste"
        )),
        "the pasted text never reached the child"
    );
}

/// `Suppressed`와 `Queued`는 서로 다른 결과다 — `bool`이었다면 둘 다 실패로
/// 뭉개져 앱이 "모드상 보낼 것 없음"에도 유실 피드백을 냈을 것이다.
#[test]
fn suppression_and_queueing_are_distinguishable_outcomes() {
    let session = TerminalSession::start(spec(platform::echo_stdin())).unwrap();

    // FOCUS_IN_OUT이 꺼져 있으므로 포커스 리포트는 보낼 것이 없다.
    assert_eq!(session.report_focus(true), WriteOutcome::Suppressed);
    // 매핑 없는 키도 마찬가지다.
    let unknown = KeyInput {
        key: TermKey::Unknown,
        physical_latin: None,
        location: KeyLocation::Standard,
        mods: Mods::default(),
        text: None,
        repeat: false,
    };
    assert_eq!(session.send_key(&unknown), WriteOutcome::Suppressed);
    // 빈 붙여넣기는 인코더가 **빈 바이트열**을 준다(None이 아니다) — 앞의 두
    // 경우와 다른 코드 경로다.
    assert_eq!(session.send_paste(""), WriteOutcome::Suppressed);

    // 대조군: 실제로 보낼 것이 있으면 Queued다.
    let enter = KeyInput {
        key: TermKey::Named(NamedKey::Enter),
        physical_latin: None,
        location: KeyLocation::Standard,
        mods: Mods::default(),
        text: None,
        repeat: false,
    };
    assert_eq!(session.send_key(&enter), WriteOutcome::Queued);
}

/// `redraw`면 generation을 올려야 한다. 올리지 않으면 스냅샷 스케줄링이
/// (generation으로 돈다) 새 스냅샷을 찍지 않아 **옛 화면을 다시 그린다.**
#[test]
fn send_mouse_bumps_the_generation_when_it_asks_for_a_redraw() {
    let script = "for i in 1 2 3 4 5 6 7 8 9 0; do printf 'row%s\\n' \"$i\"; done; sleep 5";
    let session = TerminalSession::start(spec(platform::shell_command(script))).unwrap();
    assert!(wait_until(Duration::from_secs(10), || snapshot_contains(
        &session, "row0"
    )));

    let before = session.generation();
    let result = session.send_mouse(&wheel(2)).expect("wheel routes");
    assert!(result.redraw, "a local scroll changes what is on screen");
    assert_eq!(result.write, WriteOutcome::Suppressed, "nothing goes to the pty");
    assert!(
        session.generation() > before,
        "a redraw without a generation bump repaints the stale snapshot"
    );

    // 대조군: 아무것도 하지 않는 intent는 generation을 건드리지 않는다.
    let quiet = session.generation();
    let ignored = session.send_mouse(&wheel(0)).expect("a zero wheel routes");
    assert!(!ignored.redraw);
    assert_eq!(
        session.generation(),
        quiet,
        "an ignored intent must not schedule a snapshot"
    );
}
