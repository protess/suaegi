//! 에이전트 존재 폴링의 티어링. `iced::time::every`가 `Duration` 자체로
//! 키가 잡히므로([`tier`]가 반환하는 값이 바뀌면) 런타임이 알아서 타이머를
//! 교체한다 — 우리가 직접 구독을 파괴/재생성할 필요가 없다.
//!
//! **모니터는 세션 슬롯이 소유한다**(`session_store.rs`, Task 5). 이 모듈은
//! 그 슬롯을 틱마다 다시 만들지 않고 그대로 재사용한다 — 그래야 foreground
//! pgid 캐시가 폴링 주기를 넘어 살아남아 매 틱 `ps`를 새로 띄우지 않는다.

use iced::{Subscription, Task};

use crate::session_store::SessionId;
use crate::state::{AppState, Message};

/// 세션 중 하나라도 에이전트가 foreground에 떠 있으면 이 주기로 돈다 —
/// 에이전트가 일하는 동안은 상태 변화(작업 종료 등)를 빨리 잡아내고 싶다.
pub const ACTIVE_TIER: std::time::Duration = std::time::Duration::from_millis(750);
/// 에이전트가 하나도 안 보이면(세션이 없거나, 전부 `NoAgent`/`Unknown`/
/// `Exited`) 이 주기로 늦춘다 — 지켜볼 게 없는 동안 `ps`를 자주 띄울
/// 이유가 없다.
pub const IDLE_TIER: std::time::Duration = std::time::Duration::from_secs(2);

/// 지금 어느 티어로 폴링해야 하는지. 세션 하나라도 `AgentPresence::Agent`면
/// [`ACTIVE_TIER`], 아니면(세션이 없는 경우 포함) [`IDLE_TIER`].
pub fn tier(state: &AppState) -> std::time::Duration {
    let any_agent_present = state.session_store().sessions().any(|(id, _)| {
        matches!(
            state.session_store().presence(id),
            suaegi_term::presence::AgentPresence::Agent(_)
        )
    });
    if any_agent_present {
        ACTIVE_TIER
    } else {
        IDLE_TIER
    }
}

/// `every`는 `Duration` 자체로 키가 잡히므로, [`tier`]가 매번 다른 값을
/// 돌려주면 런타임이 알아서 타이머를 바꿔 끼운다 — 여기서 수동으로 갈아
/// 끼울 필요가 없다.
pub fn subscription(state: &AppState) -> Subscription<Message> {
    iced::time::every(tier(state)).map(|_instant| Message::PresenceTick)
}

/// 틱 하나를 실제로 처리한다: in-flight가 아닌 세션마다 프로브를 하나씩
/// 디스패치한다(`SessionStore::request_presence`가 in-flight 가드를 쥐고
/// 있으므로 여기서는 그냥 전 세션에 대해 불러보고 실제로 발급된 것만 센다).
/// `AppState::update`의 `Message::PresenceTick` 핸들러와 이 파일의 테스트가
/// **이 함수 하나**를 공유한다 — 테스트가 플래그를 손으로 세우지 않고 실제
/// 디스패치 사이클을 돈다.
pub(crate) fn dispatch_tick(state: &mut AppState) -> (Vec<SessionId>, Task<Message>) {
    let ids: Vec<SessionId> = state.session_store().sessions().map(|(id, _)| id).collect();
    let mut dispatched = Vec::new();
    let mut tasks = Vec::new();
    for id in ids {
        let seq = state.next_presence_seq();
        let (issued, task) = state.session_store_mut().request_presence(id, seq);
        if issued {
            dispatched.push(id);
            tasks.push(task);
        }
    }
    (dispatched, Task::batch(tasks))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use suaegi_core::domain::WorktreeId;
    use suaegi_term::presence::AgentPresence;

    use crate::session_store::SessionStore;

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

    /// 실제로 살아있는(throwaway) 세션을 `n`개 스토어에 붙여 넣는다. Task 5의
    /// `spawn_throwaway_for_test` + `accept_started`를 그대로 쓴다 — 별도
    /// 스텁을 새로 만들면 프로덕션이 실제로 쓰는 `SessionSlot` 생성 경로를
    /// 우회하게 된다.
    fn state_with_sessions(n: usize) -> AppState {
        let mut state = AppState::default();
        for i in 0..n {
            let id = state.session_store_mut().next_id();
            let worktree_id = WorktreeId(format!("/tmp/presence-poll-test-{i}"));
            let session = SessionStore::spawn_throwaway_for_test();
            state
                .session_store_mut()
                .accept_started(id, worktree_id, session, true)
                .expect("accept_started must succeed for a live worktree");
        }
        state
    }

    fn state_with_one_session() -> (AppState, SessionId) {
        let state = state_with_sessions(1);
        let id = state.session_store().sessions().next().unwrap().0;
        (state, id)
    }

    /// 테스트 전용: `Message::PresenceReady`가 도착했을 때의 반영을 흉내낸다
    /// (`AppState::update`가 실제로 하는 일과 같다). 시퀀스 값은
    /// `next_presence_seq`로 계속 증가하는 값을 새로 발급한다 —
    /// `apply_presence`의 in-flight 가드는 bool이라 값과 무관하게 항상
    /// 풀리므로, 여기서 발급하는 시퀀스가 디스패치 때 쓰인 것과 같을 필요는
    /// 없다(있어야 하는 건 "가드가 실제로 풀린다"는 사실뿐이다).
    fn apply_presence_result(state: &mut AppState, id: SessionId, presence: AgentPresence) {
        let seq = state.next_presence_seq();
        state.session_store_mut().apply_presence(id, seq, presence);
    }

    #[test]
    fn active_sessions_poll_faster_than_idle_ones() {
        let (idle_state, _idle_id) = state_with_one_session();
        let (mut active_state, active_id) = state_with_one_session();
        active_state.session_store_mut().apply_presence(
            active_id,
            1,
            AgentPresence::Agent("claude"),
        );

        assert!(tier(&active_state) < tier(&idle_state));
    }

    #[test]
    fn no_sessions_means_the_slow_tier() {
        assert_eq!(tier(&AppState::default()), IDLE_TIER);
    }

    #[test]
    fn a_tick_while_a_probe_is_in_flight_does_not_dispatch_a_second_one() {
        // 손으로 플래그를 세우지 않는다 — 실제 틱 경로를 두 번 돌린다.
        let (mut state, _id) = state_with_one_session();
        let (dispatched_first, _task_first) = dispatch_tick(&mut state);
        let (dispatched_second, _task_second) = dispatch_tick(&mut state);
        assert_eq!(dispatched_first.len(), 1);
        assert!(
            dispatched_second.is_empty(),
            "no second probe while one is in flight"
        );
    }

    #[test]
    fn the_guard_clears_when_the_result_arrives_so_the_next_tick_dispatches() {
        let (mut state, _id) = state_with_one_session();
        let (first, _task) = dispatch_tick(&mut state);
        assert_eq!(first.len(), 1);
        apply_presence_result(&mut state, first[0], AgentPresence::NoAgent);
        let (second, _task) = dispatch_tick(&mut state);
        assert_eq!(second.len(), 1, "the guard must clear on result");
    }

    /// 이 검증은 foreground pgid가 관측되는 unix에서만 의미가 있다. Windows
    /// 에서는 존재 감지가 항상 `Unknown`이라 호출 횟수가 0으로 남는다.
    ///
    /// **반드시 `request_presence_with`(프로덕션이 실제로 부르는 경로)를
    /// 거친다.** 예전엔 `SessionStore::probe_now_for_test`로 `probe_with`만
    /// 우회 호출했는데, 그 헬퍼는 `request_presence`의 가드 설정·백그라운드
    /// 스레드 디스패치를 건너뛴다 — `Arc::clone(&slot.monitor)`를 틱마다
    /// `PresenceMonitor::default()`로 바꾸는 회귀("모니터를 매번 새로
    /// 만든다")가 나도 이 테스트는 계속 통과했다. 지금은 `dispatch_tick`이
    /// 부르는 것과 같은 함수를 직접 호출하므로 그 회귀가 실제로 이 테스트를
    /// 깬다.
    #[cfg(unix)]
    #[test]
    fn the_monitor_cache_survives_across_ticks() {
        // 틱마다 새 모니터를 만들면 "에이전트임"이 캐시되지 않아 ps가 매번
        // 뜬다. 필드 포인터 비교로는 그 mutation을 못 잡으므로 호출 횟수로
        // 본다.
        struct CountingProbe(Arc<AtomicUsize>);
        impl suaegi_term::presence::ProcessProbe for CountingProbe {
            fn command_line(&self, _pid: i32) -> Option<String> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Some("claude".to_string())
            }
        }

        let calls = Arc::new(AtomicUsize::new(0));
        let (mut state, id) = state_with_one_session();
        let session = state.session_store().sessions().next().unwrap().1;
        // foreground pgid는 PTY가 자식을 실제로 관측한 뒤에야 채워진다 —
        // 그전에 프로브하면 항상 `Unknown`이라 `after_first == 0`이 되어
        // 그 아래 비교가 공허해진다.
        assert!(wait_until(Duration::from_secs(10), || session
            .foreground_pgid()
            .is_some()));

        let probe: Arc<dyn suaegi_term::presence::ProcessProbe + Send + Sync> =
            Arc::new(CountingProbe(calls.clone()));

        let (issued_first, _task) =
            state
                .session_store_mut()
                .request_presence_with(id, 1, probe.clone());
        assert!(issued_first, "the first tick must actually dispatch");
        assert!(
            wait_until(Duration::from_secs(10), || calls.load(Ordering::SeqCst) > 0),
            "the background probe must run and call the injected probe at least once"
        );
        let after_first = calls.load(Ordering::SeqCst);

        // 첫 결과가 실제로 도착한 걸로 치고 가드를 푼다 — 안 그러면 두 번째
        // `request_presence_with`가 in-flight 가드에 막혀 아예 디스패치되지
        // 않고, 그 경우 `issued_second`가 이미 이 테스트의 실패로 드러난다.
        state
            .session_store_mut()
            .apply_presence(id, 1, AgentPresence::Agent("claude"));

        let (issued_second, _task) =
            state
                .session_store_mut()
                .request_presence_with(id, 2, probe.clone());
        assert!(issued_second, "the second tick must also dispatch");
        // 캐시가 살아 있으면 두 번째 라운드은 pgid가 캐시에 남아 있는 동안
        // `probe.command_line`을 다시 부르지 않는다 — 콜 카운트가 그대로여야
        // 한다. 충분히 기다렸는데도 카운트가 그대로면 캐시가 재사용된 것,
        // 늘었으면 매 틱 새 모니터가 만들어진(회귀) 것이다.
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            after_first,
            "a cached agent pgid must not re-probe on the next tick"
        );
    }
}
