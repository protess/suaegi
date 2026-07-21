mod platform; // suaegi-term의 tests/platform/mod.rs 복사

use std::sync::{Arc, Mutex};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use suaegi_app::reaper::Reaper;
use suaegi_app::session_store::{blank_snapshot, SessionStore};
use suaegi_core::domain::WorktreeId;
use suaegi_term::agent::AgentKind;
use suaegi_term::presence::AgentPresence;
use suaegi_term::pty::PtySpawn;
use suaegi_term::session::{SessionSpec, TerminalSession};

fn wait_until<F: FnMut() -> bool>(t: Duration, mut f: F) -> bool {
    let deadline = Instant::now() + t;
    while Instant::now() < deadline {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

/// `accept_started`의 거절 경로를 테스트하려면 스토어 밖에서 스폰된, 슬롯에
/// 등록되지 않은 세션이 필요하다.
fn start_throwaway_session(command: (String, Vec<String>)) -> TerminalSession {
    TerminalSession::start(SessionSpec {
        pty: PtySpawn {
            program: command.0,
            args: command.1,
            cwd: None,
            env: Vec::new(),
            rows: 24,
            cols: 80,
        },
        scrollback: 200,
    })
    .expect("throwaway test session must start")
}

/// Drop이 **실제로 어느 스레드에서 실행됐는지** 기록하는 센티널.
/// "reaper 스레드 id를 돌려주는" 헬퍼로는 소멸자가 거기서 돌았다는 증거가 안 된다.
struct DropSentinel(Arc<Mutex<Option<ThreadId>>>);
impl Drop for DropSentinel {
    fn drop(&mut self) {
        *self.0.lock().unwrap() = Some(std::thread::current().id());
    }
}

#[test]
fn a_stale_snapshot_result_never_overwrites_a_newer_one() {
    let mut store = SessionStore::for_test();
    let id = store.start_for_test(platform::echo("hello"));
    assert!(wait_until(Duration::from_secs(10), || {
        store.pump_for_test(id);
        store.row_text(id, 0).contains("hello")
    }));
    let newest = store.row_text(id, 0);
    let current = store.snapshot_generation(id);
    let _ = store.apply_snapshot(id, current.saturating_sub(1), blank_snapshot());
    assert_eq!(
        store.row_text(id, 0),
        newest,
        "stale result must be discarded"
    );
}

#[test]
fn only_one_snapshot_request_is_in_flight_per_session() {
    let mut store = SessionStore::for_test();
    let id = store.start_for_test(platform::echo("x"));
    let (first, _) = store.request_snapshot(id, 1);
    let (second, _) = store.request_snapshot(id, 2);
    assert!(first);
    assert!(
        !second,
        "a second request must be suppressed while one is in flight"
    );
}

#[test]
fn the_guard_clears_on_its_own_result_and_not_on_someone_elses() {
    // 가드를 안 풀면 그 세션은 영영 스냅샷을 못 뜨고 화면이 굳는다.
    // 반대로 아무 결과에나 풀면 동시 스냅샷이 생겨 리더와 락을 두고 경합한다.
    let mut store = SessionStore::for_test();
    let id = store.start_for_test(platform::echo("x"));

    let (issued, _) = store.request_snapshot(id, 5);
    assert!(issued);

    // 이 요청과 무관한(오래된) 결과가 도착해도 가드는 그대로여야 한다
    assert!(store.apply_snapshot(id, 1, blank_snapshot()).is_none());
    assert!(
        !store.request_snapshot(id, 6).0,
        "a foreign result must not release the guard"
    );

    // 자기 결과가 도착하면 풀린다
    let _ = store.apply_snapshot(id, 5, blank_snapshot());
    assert!(
        store.request_snapshot(id, 6).0,
        "the matching result must release the guard"
    );
}

#[test]
fn output_arriving_during_a_snapshot_is_not_lost() {
    // 스냅샷이 도는 동안 generation이 올라가면 구독은 그 세대를 이미 알린 뒤라
    // 다시 알리지 않는다. 완료 시점에 다시 요청하지 않으면 그 출력은 영영
    // 화면에 안 나온다 — 터미널이 조용히 멈춘 것처럼 보인다.
    let mut store = SessionStore::for_test();
    let id = store.start_for_test(platform::echo("first"));

    let (issued, _task) = store.request_snapshot(id, 1);
    assert!(issued);
    store.bump_generation_for_test(id, 9); // 스냅샷이 도는 동안 출력이 더 들어왔다
    let follow_up = store.apply_snapshot(id, 1, blank_snapshot());
    assert!(
        follow_up.is_some(),
        "completion must schedule another snapshot when the session moved on"
    );
    // pr4 항목 4: 재요청은 실제 스레드 스폰(무거운 작업)만 POLL_INTERVAL만큼
    // 늦춘다 — in-flight 가드는 여기서 이미 (동기적으로) 세워져 있어야 한다.
    // 그러지 않으면 그 지연 창 동안 도착하는 별도의 `request_snapshot` 호출이
    // 가드 없는 틈을 타 중복 요청을 낸다. `_task`를 실제로 실행하지 않아도
    // (이 크레이트의 다른 테스트들도 `Task<Message>`를 끝까지 돌리지 않는다)
    // 가드가 이미 세워졌는지는 여기서 확인할 수 있다.
    let (issued_during_delay, _task) = store.request_snapshot(id, 999);
    assert!(
        !issued_during_delay,
        "the guard for the paced re-issue must be held immediately, not only once the delay elapses"
    );
}

#[test]
fn closing_through_the_store_drops_the_session_off_the_calling_thread() {
    // **반드시 SessionStore::close()를 거친다.** Reaper를 직접 부르면
    // close()를 "그 자리에서 drop"으로 되돌리는 mutation을 잡지 못한다.
    let mut store = SessionStore::for_test();
    let id = store.start_for_test(platform::sleep_seconds(30));
    let impostor = store.clone_arc_for_test(id); // 구독이 든 클론을 흉내
    let caller = std::thread::current().id();

    store.close(id);
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        store.reaper_drop_thread_for_test(id).is_none(),
        "reaper must wait while another clone is alive"
    );

    drop(impostor);
    assert!(wait_until(Duration::from_secs(10), || store
        .reaper_drop_thread_for_test(id)
        .is_some()));
    assert_ne!(
        store.reaper_drop_thread_for_test(id).unwrap(),
        caller,
        "the session must not be destroyed on the calling thread"
    );
}

#[test]
fn a_stuck_session_does_not_block_reaping_of_later_ones() {
    // head-of-line blocking 방지: 앞선 세션의 클론이 오래 살아 있어도
    // 뒤에 은퇴한 세션은 제때 정리돼야 한다.
    let reaper = Reaper::spawn();
    let stuck_where = Arc::new(Mutex::new(None));
    let later_where = Arc::new(Mutex::new(None));
    let stuck = Arc::new(DropSentinel(stuck_where.clone()));
    let stuck_clone = stuck.clone(); // 일부러 계속 살려둔다
    let later = Arc::new(DropSentinel(later_where.clone()));

    reaper.retire_for_test(stuck);
    reaper.retire_for_test(later); // 뒤에 은퇴

    assert!(
        wait_until(Duration::from_secs(10), || later_where
            .lock()
            .unwrap()
            .is_some()),
        "a later session must be reaped even while an earlier one is pinned"
    );
    assert!(stuck_where.lock().unwrap().is_none());
    drop(stuck_clone);
}

#[test]
fn a_session_that_exits_reports_its_code_and_stops_running() {
    let mut store = SessionStore::for_test();
    let id = store.start_for_test(platform::exit_with(3));
    assert!(wait_until(Duration::from_secs(10), || store.exit_code(id) == Some(3)));
    assert!(!store.is_running(id));
}

#[test]
fn a_late_start_for_a_deleted_worktree_is_retired_not_orphaned() {
    let mut store = SessionStore::for_test();
    let id = store.next_id();
    let gone = WorktreeId("/tmp/deleted".into());
    let session = start_throwaway_session(platform::sleep_seconds(30));
    // 호출자가 "그 worktree는 이제 없다"고 알려준다
    assert!(store.accept_started(id, gone, session, false).is_err());
    assert_eq!(store.slot_count(), 0, "no orphan slot");
    // 세션은 reaper로 갔어야 한다 — 아니면 PTY와 스레드가 샌다
    assert!(wait_until(Duration::from_secs(10), || store
        .reaper_retired_count()
        == 1));
}

#[test]
fn a_stale_presence_result_does_not_overwrite_a_newer_one() {
    let mut store = SessionStore::for_test();
    let id = store.start_for_test(platform::sleep_seconds(30));
    store.apply_presence(id, 2, AgentPresence::Agent(AgentKind::Claude));
    store.apply_presence(id, 1, AgentPresence::NoAgent); // 늦게 도착한 옛 결과
    assert!(
        matches!(store.presence(id), AgentPresence::Agent(_)),
        "an older presence result must be discarded"
    );
}
