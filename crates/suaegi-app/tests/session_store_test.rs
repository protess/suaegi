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

// ---- pr4 적대적 리뷰 항목 5: in-flight 가드에 복구 경로가 없었다. 정상
// 경로에서는 자기 결과가 캐시보다 낡아서 도착할 일이 없지만, 그런 상황이
// 생기면(레이스, 버그) "값을 버린다" 이른 반환이 가드 해제보다 먼저 있어
// 가드가 영영 안 풀렸다 — 그 세션의 스냅샷은 그 뒤로 다시는 갱신되지 않는다
// (요청이 계속 `issued=false`로 막힌다). `pump_for_test`로 가드를 거치지
// 않고 캐시를 인위적으로 앞서게 만들어, 그 뒤 도착하는 "자기 결과"가
// staleness 검사에 걸리는 상황을 재현한다 ----

#[test]
fn an_own_result_that_arrives_stale_still_releases_its_guard() {
    let mut store = SessionStore::for_test();
    let id = store.start_for_test(platform::echo("x"));

    let (issued, _) = store.request_snapshot(id, 5);
    assert!(issued);

    // 가드(5)를 거치지 않고 캐시를 그보다 앞선 generation으로 밀어둔다 —
    // 정상 경로에서는 안 일어나지만, 방어 대상 시나리오(자기 결과가
    // 캐시보다 낡아 도착)를 인위적으로 만든다.
    store.bump_generation_for_test(id, 20);
    store.pump_for_test(id);
    assert!(
        store.snapshot_generation(id) > 5,
        "the cache must now be ahead of the in-flight guard's generation"
    );

    // 이제야 가드(5)의 "자기 결과"가 도착한다 — 캐시 입장에선 낡았다.
    let follow_up = store.apply_snapshot(id, 5, blank_snapshot());
    assert!(
        follow_up.is_none(),
        "a stale result must not schedule a follow-up snapshot"
    );

    // 값은 버려졌더라도 가드는 풀렸어야 한다 — 안 그러면 이 세션은 다시는
    // 스냅샷을 못 뜬다.
    assert!(
        store.request_snapshot(id, 999).0,
        "the guard for generation 5 must release even though its own result arrived stale"
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

// ---- pr4 적대적 리뷰 항목 6: `reaped_at`은 테스트 관측(`reaper_drop_thread_
// for_test`) 전용인데, 프로덕션 경로(`SessionStore::new()`)에서도 세션이
// 닫힐 때마다 계속 채워지고 있었다 — 앱 수명 내내 지워지지 않는 맵. 오직
// `SessionStore::for_test()`만 이 필드를 채우게 해서, 프로덕션 인스턴스는
// 세션을 아무리 많이 열고 닫아도 이 맵이 비어 있어야 한다 ----

#[test]
fn production_stores_never_populate_reaped_at() {
    // `for_test()`가 아니라 프로덕션이 실제로 부르는 `SessionStore::new()`를
    // 그대로 쓴다. `start_for_test`/`reaper_drop_thread_for_test`는 시작/관측
    // 편의를 위한 테스트 헬퍼일 뿐 — 어느 생성자로 만들었든 슬롯 조작 자체는
    // 동작한다(그래서 이 조합으로 프로덕션 생성자를 검증할 수 있다).
    let mut store = SessionStore::new();
    let id = store.start_for_test(platform::sleep_seconds(30));

    store.close(id);
    assert!(
        wait_until(Duration::from_secs(10), || store.reaper_retired_count()
            == 1),
        "the session must still actually reach the reaper"
    );
    // retired_count로 이미 reap이 끝났다고 확인했으니, 더 기다려도 안 채워질
    // reaped_at 엔트리를 기다리지 않는다 — 지금 바로 None이어야 한다.
    assert!(
        store.reaper_drop_thread_for_test(id).is_none(),
        "a production SessionStore must never populate reaped_at"
    );
}
