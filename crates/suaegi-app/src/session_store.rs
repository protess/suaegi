//! 세션 생명주기 + 스냅샷 캐시.
//!
//! **스냅샷은 UI 스레드에서 뜨지 않는다.** `snapshot()`은 뷰포트 전체(80×50 ≈
//! 190KB)를 새로 할당하고 PTY 리더 스레드와 같은 `FairMutex`를 두고 경합한다
//! — `request_snapshot`이 `Arc<TerminalSession>`을 블로킹 스레드로 옮기고,
//! 결과는 메시지로 돌아온다(`apply_snapshot`).
//!
//! **세션의 마지막 drop도 UI 스레드에서 일어나지 않는다.** `Drop for
//! TerminalSession`은 최대 2초를 먹을 수 있다(`session.rs`). `close()`는 슬롯을
//! 꺼내 `Arc`를 [`Reaper`]로 넘긴다 — 구독·프레즌스 폴링이 든 다른 클론이 아직
//! 살아 있으면 Reaper가 그 클론들이 모두 사라질 때까지 기다렸다가 자신의
//! 스레드에서 떨어뜨린다.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use iced::Task;
use suaegi_core::domain::{Worktree, WorktreeId};
use suaegi_term::agent::{build_spawn, AgentKind};
use suaegi_term::grid::TerminalSnapshot;
use suaegi_term::presence::{AgentPresence, PresenceMonitor, ProcessProbe, PsProbe};
use suaegi_term::pty::PtySpawn;
use suaegi_term::session::{SessionSpec, TerminalSession};

use crate::background;
use crate::reaper::Reaper;
use crate::state::Message;

/// 렌더링 뷰포트 고정 크기. 이 플랜의 터미널 렌더링은 읽기 전용 단색
/// 모노스페이스 텍스트로 범위가 좁혀져 있다(Plan 4가 리사이즈 가능한 커스텀
/// 위젯을 맡는다) — 그래서 지금은 세션마다 고정 크기로 스폰한다.
const DEFAULT_ROWS: u16 = 50;
const DEFAULT_COLS: u16 = 80;
/// 스크롤백 상한(줄 수). 세션당 메모리 상한과 사용성 사이의 절충값 — 정확한
/// 조정은 이 태스크 범위 밖이다.
const SCROLLBACK_LINES: usize = 5_000;
/// `start_for_test`/`accept_started`가 쓰는 테스트/기본 스크롤백. 실제
/// 스크롤백을 검증하는 테스트가 아니므로 작게 잡아 메모리를 아낀다.
const TEST_SCROLLBACK_LINES: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(pub u64);

/// `TerminalSession`은 `Clone`이 아니라서 `Message`(iced 위젯이 `Message:
/// Clone`을 요구한다)에 직접 담을 수 없다. 봉투로 감싸 **한 번만** 꺼내 쓴다 —
/// 두 번째 `take()`는 `None`이다(이미 다른 곳에서 처리됐다는 뜻).
#[derive(Clone)]
pub struct StartedSession(Arc<Mutex<Option<TerminalSession>>>);

impl StartedSession {
    /// `pub(crate)`인 이유: `state.rs`(Task 6)가 `SessionStarted`의 성공 경로를
    /// 테스트할 때 실제 `Task` 파이프라인 없이 직접 봉투를 만들어야 한다 —
    /// `tests/`의 별도 크레이트 경계를 넘지 않으므로 `doc(hidden) pub`보다
    /// `pub(crate)`가 정확한 가시성이다.
    pub(crate) fn new(session: TerminalSession) -> Self {
        Self(Arc::new(Mutex::new(Some(session))))
    }

    pub fn take(&self) -> Option<TerminalSession> {
        self.0
            .lock()
            .expect("started session mutex poisoned")
            .take()
    }
}

impl std::fmt::Debug for StartedSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StartedSession(..)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartRejected {
    /// `accept_started`가 도착했을 때 그 worktree가 이미 삭제되어 있었다.
    /// 세션 스토어는 어떤 worktree가 살아 있는지 모른다 — 호출자가 판단해
    /// `worktree_still_exists`로 알려준다.
    WorktreeGone,
}

pub struct SessionSlot {
    pub id: SessionId,
    pub worktree_id: WorktreeId,
    pub session: Arc<TerminalSession>,
    pub snapshot: TerminalSnapshot,
    pub snapshot_generation: u64,
    /// 진행 중인 스냅샷 요청이 **어느 generation을 뜨는 중인지**. bool로는
    /// 엉뚱한 결과가 남의 가드를 풀어 동시 스냅샷이 생긴다.
    pub snapshot_in_flight: Option<u64>,
    pub presence: AgentPresence,
    pub presence_in_flight: bool,
    /// 프레즌스 값에 붙는 별도의 단조 시퀀스. `AgentPresence` 자체는 generation을
    /// 담지 않으므로 staleness 비교를 위해 슬롯에 따로 보관한다.
    presence_generation: u64,
    /// 세션마다 하나 — 틱마다 새로 만들면 pgid 캐시가 죽어 매번 ps를 띄운다.
    pub monitor: Arc<Mutex<PresenceMonitor>>,
}

impl SessionSlot {
    fn new(id: SessionId, worktree_id: WorktreeId, session: TerminalSession) -> Self {
        Self {
            id,
            worktree_id,
            session: Arc::new(session),
            snapshot: blank_snapshot(),
            snapshot_generation: 0,
            snapshot_in_flight: None,
            presence: AgentPresence::Unknown,
            presence_in_flight: false,
            presence_generation: 0,
            monitor: Arc::new(Mutex::new(PresenceMonitor::default())),
        }
    }
}

/// 아직 스냅샷을 뜨지 않은 슬롯의 초기 캐시값. 빈 80x0 그리드 — 실제 크기는
/// 첫 스냅샷이 도착하면 갱신된다.
pub fn blank_snapshot() -> TerminalSnapshot {
    TerminalSnapshot {
        rows: Vec::new(),
        size: suaegi_term::grid::GridSize { rows: 0, cols: 0 },
        cursor: None,
        display_offset: 0,
        history_size: 0,
    }
}

pub struct SessionStore {
    slots: HashMap<SessionId, SessionSlot>,
    reaper: Reaper,
    /// `close()`/`accept_started`의 거절 경로로 넘어간 세션이 **실제로 어느
    /// 스레드에서** 떨어졌는지. 오직 Reaper의 콜백만 채운다 — 그 콜백이
    /// 실행되는 스레드가 곧 소멸자가 실행된 스레드라는 증거다.
    reaped_at: Arc<Mutex<HashMap<SessionId, std::thread::ThreadId>>>,
    retired_count: Arc<AtomicU64>,
    next_id: u64,
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionStore {
    pub fn new() -> Self {
        Self {
            slots: HashMap::new(),
            reaper: Reaper::spawn(),
            reaped_at: Arc::new(Mutex::new(HashMap::new())),
            retired_count: Arc::new(AtomicU64::new(0)),
            next_id: 0,
        }
    }

    /// 호출자가 미리 발급받을 `SessionId`. `start`는 이 id를 그대로 받아
    /// 결과 메시지에 실어 보낸다 — 동시에 시작한 세션들의 완료 순서가
    /// 뒤바뀌어도 어느 요청의 결과인지 잃지 않는다.
    pub fn next_id(&mut self) -> SessionId {
        let id = SessionId(self.next_id);
        self.next_id += 1;
        id
    }

    /// 블로킹 스레드에서 `TerminalSession::start`(fork/exec)를 수행하고 결과를
    /// 메시지로 돌려준다. 실패도 맥락(`id`, `worktree_id`)을 나른다.
    pub fn start(
        &mut self,
        id: SessionId,
        worktree: &Worktree,
        agent: AgentKind,
        prompt: Option<String>,
    ) -> Task<Message> {
        let worktree_id = worktree.id.clone();
        let cwd = worktree.path.clone();
        let spawn = build_spawn(
            agent,
            None,
            prompt.as_deref(),
            cwd,
            DEFAULT_ROWS,
            DEFAULT_COLS,
        );
        background::blocking(move |mut sender| {
            let spec = SessionSpec {
                pty: spawn,
                scrollback: SCROLLBACK_LINES,
            };
            let result = TerminalSession::start(spec)
                .map(StartedSession::new)
                .map_err(|e| e.to_string());
            let _ = sender.try_send(Message::SessionStarted {
                id,
                worktree_id: worktree_id.clone(),
                result,
            });
        })
    }

    /// 시작 결과가 늦게 도착했을 때 슬롯을 만들지 결정한다. 세션 스토어는 어떤
    /// worktree가 살아 있는지 모른다 — 호출자가 `worktree_still_exists`로
    /// 알려준다. 거절되면 세션은 곧장 reaper로 간다(고아 세션이 남으면 PTY와
    /// 스레드가 새어나간다).
    pub fn accept_started(
        &mut self,
        id: SessionId,
        worktree_id: WorktreeId,
        session: TerminalSession,
        worktree_still_exists: bool,
    ) -> Result<(), StartRejected> {
        if !worktree_still_exists {
            self.retire(Arc::new(session), Some(id));
            return Err(StartRejected::WorktreeGone);
        }
        self.slots
            .insert(id, SessionSlot::new(id, worktree_id, session));
        Ok(())
    }

    /// 스냅샷은 UI 스레드에서 뜨지 않는다. 이미 진행 중이면 `false`를 반환하고
    /// 띄우지 않는다.
    pub fn request_snapshot(&mut self, id: SessionId, generation: u64) -> (bool, Task<Message>) {
        let Some(slot) = self.slots.get_mut(&id) else {
            return (false, Task::none());
        };
        if slot.snapshot_in_flight.is_some() {
            return (false, Task::none());
        }
        slot.snapshot_in_flight = Some(generation);
        let session = Arc::clone(&slot.session);
        let task = background::blocking(move |mut sender| {
            let snapshot = session.snapshot();
            let _ = sender.try_send(Message::SnapshotReady {
                id,
                generation,
                snapshot,
            });
        });
        (true, task)
    }

    /// 도착한 결과를 반영한다:
    /// - 캐시보다 오래된 generation이면 **버린다**(캐시도 가드도 건드리지 않는다)
    /// - 캐시에 반영한 뒤, 가드는 **자기 요청의 결과일 때만** 푼다
    ///   (`in_flight == Some(generation)`)
    /// - 푼 직후 `session.generation()`이 이미 더 나아가 있으면 곧바로 다음
    ///   요청을 낸다 — 그러지 않으면 스냅샷이 도는 동안 도착한 출력이 영영
    ///   화면에 반영되지 않는다(구독은 그 generation을 이미 알렸으므로 다시
    ///   알리지 않는다).
    pub fn apply_snapshot(
        &mut self,
        id: SessionId,
        generation: u64,
        snapshot: TerminalSnapshot,
    ) -> Option<Task<Message>> {
        let slot = self.slots.get_mut(&id)?;

        if generation < slot.snapshot_generation {
            return None; // 캐시보다 오래된 결과 — 버린다
        }
        slot.snapshot = snapshot;
        slot.snapshot_generation = generation;

        if slot.snapshot_in_flight != Some(generation) {
            // 이 결과의 값은 캐시에 반영했지만(위에서 이미 최신임을 확인했다),
            // 이 요청이 지금 진행 중인 가드의 주인은 아니다 — 가드는 그대로 둔다.
            return None;
        }
        slot.snapshot_in_flight = None;

        let current_generation = slot.session.generation();
        if current_generation > generation {
            let (_, task) = self.request_snapshot(id, current_generation);
            Some(task)
        } else {
            None
        }
    }

    pub fn request_presence(&mut self, id: SessionId, generation: u64) -> (bool, Task<Message>) {
        self.request_presence_with(id, generation, Arc::new(PsProbe))
    }

    /// `request_presence`가 실제로 쓰는 디스패치 경로 — 프로브만 주입 가능하게
    /// 갈라낸 것. 프로덕션은 `request_presence`를 통해 항상 [`PsProbe`]로
    /// 부르고, 테스트(`presence_poll`의 캐시 회귀 테스트)는 이 함수를 직접
    /// 불러 카운팅 프로브를 꽂는다. 예전엔 프로덕션 클로저가 `&PsProbe`를
    /// 하드코딩해서 대체할 수 없었고, 테스트는 `probe_with`만 동기적으로
    /// 우회 호출하는 별도 헬퍼를 썼다 — 그래서 `request_presence`가 슬롯의
    /// 모니터 대신 새 모니터를 매 틱 만드는 회귀(캐시가 매번 죽는 버그)가
    /// 나도 그 테스트는 계속 통과했다. 지금은 프로덕션과 테스트가 이 함수
    /// 하나(가드 설정 → 백그라운드 스레드 → `probe_with`)를 공유하므로 그
    /// 회귀가 실제로 테스트를 깬다. `Send + Sync + 'static`인 이유: 이 호출이
    /// 백그라운드 스레드로 넘어가기 때문이다.
    pub fn request_presence_with(
        &mut self,
        id: SessionId,
        generation: u64,
        probe: Arc<dyn ProcessProbe + Send + Sync>,
    ) -> (bool, Task<Message>) {
        let Some(slot) = self.slots.get_mut(&id) else {
            return (false, Task::none());
        };
        if slot.presence_in_flight {
            return (false, Task::none());
        }
        slot.presence_in_flight = true;
        let session = Arc::clone(&slot.session);
        let monitor = Arc::clone(&slot.monitor);
        let task = background::blocking(move |mut sender| {
            let presence = Self::probe_with(&session, &monitor, probe.as_ref());
            let _ = sender.try_send(Message::PresenceReady {
                id,
                generation,
                presence,
            });
        });
        (true, task)
    }

    /// 슬롯이 소유한(persist된) 모니터로 프로브를 한 번 돈다. `request_presence`의
    /// 블로킹 스레드 클로저와 [`Self::probe_now_for_test`]가 이 함수 하나를
    /// 공유한다 — "호출마다 모니터를 새로 만드는" 회귀가 프로덕션 경로든 테스트
    /// 경로든 똑같이 드러나야 하기 때문이다. `&mut self`를 받지 않는 이유는
    /// `request_presence`가 이걸 블로킹 스레드로 옮겨 부르므로 `self`를 그
    /// 스레드로 가져갈 수 없어서다(`Arc<TerminalSession>`/`Arc<Mutex<..>>`만
    /// 넘긴다).
    fn probe_with(
        session: &TerminalSession,
        monitor: &Mutex<PresenceMonitor>,
        probe: &dyn ProcessProbe,
    ) -> AgentPresence {
        let mut guard = monitor.lock().expect("presence monitor mutex poisoned");
        guard.probe(session, probe)
    }

    /// 캐시보다 오래된 결과는 값을 버린다. 프레즌스 요청은 한 번에 하나만
    /// 진행되므로(`presence_in_flight` 가드) 도착한 결과는 항상 그 유일한
    /// 진행 중 요청의 답이다 — 그래서 가드는 staleness와 무관하게 항상 푼다.
    pub fn apply_presence(&mut self, id: SessionId, generation: u64, presence: AgentPresence) {
        let Some(slot) = self.slots.get_mut(&id) else {
            return;
        };
        slot.presence_in_flight = false;
        if generation >= slot.presence_generation {
            slot.presence = presence;
            slot.presence_generation = generation;
        }
    }

    /// 슬롯을 꺼내 Arc를 reaper에 넘긴다. 반드시 이 경로를 거쳐야 한다 —
    /// 슬롯을 그 자리에서 drop하면 마지막 clone일 경우 `Drop for
    /// TerminalSession`이 이 호출 스레드(보통 UI 스레드)에서 최대 2초 실행된다.
    pub fn close(&mut self, id: SessionId) {
        if let Some(slot) = self.slots.remove(&id) {
            self.retire(slot.session, Some(id));
        }
    }

    fn retire(&self, session: Arc<TerminalSession>, id: Option<SessionId>) {
        let retired_count = Arc::clone(&self.retired_count);
        let reaped_at = Arc::clone(&self.reaped_at);
        self.reaper.retire_with_callback(session, move || {
            retired_count.fetch_add(1, Ordering::SeqCst);
            if let Some(id) = id {
                reaped_at
                    .lock()
                    .expect("reaped_at mutex poisoned")
                    .insert(id, std::thread::current().id());
            }
        });
    }

    pub fn exit_code(&self, id: SessionId) -> Option<i32> {
        self.slots
            .get(&id)
            .and_then(|slot| slot.session.exit_code())
    }

    pub fn is_running(&self, id: SessionId) -> bool {
        self.slots
            .get(&id)
            .map(|slot| slot.session.is_running())
            .unwrap_or(false)
    }

    pub fn presence(&self, id: SessionId) -> AgentPresence {
        self.slots
            .get(&id)
            .map(|slot| slot.presence)
            .unwrap_or(AgentPresence::Unknown)
    }

    /// 워크벤치 구독(Task 6)이 세션마다 하나씩 붙이는 피드를 만들기 위한
    /// 열거. `Arc`를 클론해 돌려준다 — 구독이 이 `Arc`를 들고 있는 동안
    /// `SessionStore`가 슬롯을 지워도(`close`) 세션 자체는 reaper로 넘어갈
    /// 뿐, 구독이 들고 있는 클론이 매달려 있는 한 즉시 죽지 않는다.
    pub fn sessions(&self) -> impl Iterator<Item = (SessionId, Arc<TerminalSession>)> + '_ {
        self.slots
            .values()
            .map(|slot| (slot.id, Arc::clone(&slot.session)))
    }

    /// 캐시된 스냅샷을 화면에 그대로 그릴 수 있는 줄 단위 텍스트로. 스냅샷이
    /// 아직 한 번도 안 왔으면(`blank_snapshot`) 빈 문자열이다.
    pub fn snapshot_text(&self, id: SessionId) -> String {
        let Some(slot) = self.slots.get(&id) else {
            return String::new();
        };
        (0..slot.snapshot.rows.len())
            .map(|row| slot.snapshot.row_text(row))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

// ---- 테스트 전용 헬퍼. `#[cfg(test)]`가 아니라 `#[doc(hidden)]`인 이유:
// `tests/session_store_test.rs`는 별도 크레이트로 컴파일되어 라이브러리
// 크레이트의 `cfg(test)`를 보지 못한다. 공개 API 표면을 넓히지 않으려면
// 문서에서만 숨긴다. ----
impl SessionStore {
    #[doc(hidden)]
    pub fn for_test() -> Self {
        Self::new()
    }

    /// `platform::echo(...)` 등이 돌려주는 `(program, args)`로 즉시 세션을
    /// 시작하고 슬롯에 넣는다 — `start()`의 비동기 Task 파이프라인을 거치지
    /// 않는다(플레인 `#[test]`엔 iced 실행기가 없다).
    #[doc(hidden)]
    pub fn start_for_test(&mut self, command: (String, Vec<String>)) -> SessionId {
        let id = self.next_id();
        let spec = SessionSpec {
            pty: PtySpawn {
                program: command.0,
                args: command.1,
                cwd: None,
                env: Vec::new(),
                rows: DEFAULT_ROWS,
                cols: DEFAULT_COLS,
            },
            scrollback: TEST_SCROLLBACK_LINES,
        };
        let session = TerminalSession::start(spec).expect("test session must start");
        self.slots.insert(
            id,
            SessionSlot::new(id, WorktreeId("test-worktree".to_string()), session),
        );
        id
    }

    /// `request_snapshot`/`apply_snapshot`의 비동기 가드를 거치지 않고 캐시를
    /// 강제로 최신화한다 — 폴링 루프로 화면 갱신을 기다리는 테스트용.
    #[doc(hidden)]
    pub fn pump_for_test(&mut self, id: SessionId) {
        if let Some(slot) = self.slots.get_mut(&id) {
            slot.snapshot = slot.session.snapshot();
            slot.snapshot_generation = slot.session.generation();
        }
    }

    #[doc(hidden)]
    pub fn row_text(&self, id: SessionId, row: usize) -> String {
        self.slots
            .get(&id)
            .map(|slot| slot.snapshot.row_text(row))
            .unwrap_or_default()
    }

    #[doc(hidden)]
    pub fn snapshot_generation(&self, id: SessionId) -> u64 {
        self.slots
            .get(&id)
            .map(|slot| slot.snapshot_generation)
            .unwrap_or(0)
    }

    /// 구독·프레즌스 폴링이 세션의 `Arc`를 붙들고 있는 상황을 흉내낸다.
    #[doc(hidden)]
    pub fn clone_arc_for_test(&self, id: SessionId) -> Arc<TerminalSession> {
        Arc::clone(&self.slots.get(&id).expect("session exists").session)
    }

    /// `TerminalSession`의 실제 generation 카운터는 private이라 밖에서 직접
    /// 건드릴 수 없다. `scroll_display(0)`은 화면을 실제로 옮기지 않으면서도
    /// (delta 0) 호출마다 generation을 1씩 올린다(session.rs) — 이를 빌려
    /// "스냅샷이 도는 동안 출력이 더 들어왔다"는 상황을 재현한다.
    #[doc(hidden)]
    pub fn bump_generation_for_test(&mut self, id: SessionId, times: u32) {
        if let Some(slot) = self.slots.get(&id) {
            for _ in 0..times {
                slot.session.scroll_display(0);
            }
        }
    }

    #[doc(hidden)]
    pub fn reaper_drop_thread_for_test(&self, id: SessionId) -> Option<std::thread::ThreadId> {
        self.reaped_at
            .lock()
            .expect("reaped_at mutex poisoned")
            .get(&id)
            .copied()
    }

    #[doc(hidden)]
    pub fn reaper_retired_count(&self) -> u64 {
        self.retired_count.load(Ordering::SeqCst)
    }

    #[doc(hidden)]
    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }


    /// 어디에도 슬롯으로 등록되지 않은, 진짜로 살아있는 `TerminalSession`
    /// 하나. `workbench.rs`의 구독 동일성 테스트와 `state.rs`의
    /// `SessionStarted` 배선 테스트가 손으로 봉투를 만들 때 이 세션이
    /// 필요하다 — 둘 다 `tests/`가 아니라 이 크레이트 안의 `#[cfg(test)]`라
    /// `tests/platform/mod.rs`를 `mod`로 끌어올 수 없다.
    #[doc(hidden)]
    pub fn spawn_throwaway_for_test() -> TerminalSession {
        #[cfg(unix)]
        let (program, args) = ("sleep".to_string(), vec!["5".to_string()]);
        #[cfg(windows)]
        let (program, args) = (
            "cmd".to_string(),
            vec!["/C".to_string(), "ping -n 6 127.0.0.1 > nul".to_string()],
        );
        TerminalSession::start(SessionSpec {
            pty: PtySpawn {
                program,
                args,
                cwd: None,
                env: Vec::new(),
                rows: DEFAULT_ROWS,
                cols: DEFAULT_COLS,
            },
            scrollback: TEST_SCROLLBACK_LINES,
        })
        .expect("throwaway test session must start")
    }
}
