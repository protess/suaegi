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
use suaegi_term::input_types::CopyRequest;
use suaegi_term::presence::{AgentPresence, PresenceMonitor, ProcessProbe, PsProbe};
use suaegi_term::pty::PtySpawn;
use suaegi_term::session::{SessionSpec, TerminalSession};

use crate::background;
use crate::reaper::Reaper;
use crate::state::Message;

/// 세션을 스폰할 때 쓰는 **부트스트랩 기본값**이다. 실제 크기가 아니다.
///
/// 스폰 시점에는 레이아웃이 존재하지 않으므로 진짜 크기를 알아낼 방법이 없다 —
/// 위젯이 첫 레이아웃에서 발행하는 `TermCommand::Resize`가 이 값을 실제 pane
/// 크기로 고친다(`State::last_emitted`가 `None`에서 시작하므로 첫 유효
/// 레이아웃은 **반드시** 발행된다). 그러니 이 상수를 "고칠" 필요가 없고,
/// 여기서 크기를 추측하려 들면 안 된다.
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

// ---------------------------------------------------------------------------
// 리사이즈 합치기
// ---------------------------------------------------------------------------

/// 한 번의 `submit`이 내리는 결정.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeDecision {
    /// 지금 워커로 보낸다.
    Dispatch { rows: u16, cols: u16, seq: u64 },
    /// 이미 워커가 돌고 있다 — 최신 것으로 대기열을 덮어썼다. 워커가 끝나면
    /// `completed`가 이걸 꺼내 보낸다.
    Coalesce,
    /// 이미 본 것보다 낡았다. 버린다.
    Discard,
}

/// 세션 하나의 리사이즈 합치기 상태.
///
/// **왜 필요한가.** `TerminalSession::resize`는 블로킹이다(resize_lock + pty +
/// grid). 분할선을 끄는 동안 위젯은 셀 경계를 넘을 때마다 리사이즈를 발행하므로,
/// 하나씩 순서대로 실행하면 워커 큐가 사용자의 드래그보다 뒤처지고 마지막
/// 크기에 도달하기까지 쓸모없는 중간 크기를 전부 PTY에 적용한다. **세션당 최신
/// `seq` 하나만** 실행한다.
///
/// **`seq`가 왜 전역 단조 카운터에서 오는가**는 `terminal::state`의 `RESIZE_SEQ`
/// 문서에 있다 — 요약하면 위젯 상태에 두면 `Tree::diff`가 조용히 리셋해 이
/// 가드가 이후의 모든 리사이즈를 영구히 버린다.
#[derive(Debug, Default)]
pub struct ResizeCoalescer {
    /// 지금까지 본 가장 큰 `seq`. 순서가 뒤집혀 도착한 낡은 리사이즈를 버리는
    /// 기준이다.
    last_seq: u64,
    /// 워커가 실행 중인 `seq`.
    in_flight: Option<u64>,
    /// 워커가 도는 동안 도착한 **가장 최신** 리사이즈. 하나만 들고 있는 것이
    /// 곧 합치기다.
    pending: Option<(u16, u16, u64)>,
}

impl ResizeCoalescer {
    /// 새 리사이즈 요청.
    ///
    /// **`seq`가 엄격히 커야 받아들인다.** `>=`가 아니라 `>`인 이유: 같은 `seq`가
    /// 두 번 오는 것은 중복 전달이지 새 요청이 아니다. 그리고 순서가 뒤집혀
    /// 도착한 낡은 요청을 받아들이면 사용자가 이미 지나온 크기로 PTY를
    /// 되돌린다 — 화면과 셸이 어긋난 채 남는다.
    pub fn submit(&mut self, rows: u16, cols: u16, seq: u64) -> ResizeDecision {
        if seq <= self.last_seq {
            return ResizeDecision::Discard;
        }
        self.last_seq = seq;

        if self.in_flight.is_some() {
            // 대기열은 **덮어쓴다**. 중간 크기를 큐에 쌓아 순서대로 적용하는
            // 것이야말로 이 타입이 막으려는 것이다.
            self.pending = Some((rows, cols, seq));
            return ResizeDecision::Coalesce;
        }

        self.in_flight = Some(seq);
        ResizeDecision::Dispatch { rows, cols, seq }
    }

    /// 워커가 `seq`를 끝냈다. 대기 중인 것이 있으면 그것을 돌려준다(호출자가
    /// 워커로 보낸다).
    ///
    /// **끝난 `seq`가 진행 중이던 것과 다르면 아무것도 하지 않는다.** 남의 완료
    /// 알림이 이쪽 가드를 풀면 리사이즈 두 개가 동시에 돌고, 그중 나중에 끝난
    /// 쪽이 최종 크기가 되어 순서 보장이 깨진다.
    pub fn completed(&mut self, seq: u64) -> Option<(u16, u16, u64)> {
        if self.in_flight != Some(seq) {
            return None;
        }
        self.in_flight = None;

        let (rows, cols, next) = self.pending.take()?;
        self.in_flight = Some(next);
        Some((rows, cols, next))
    }
}

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
    /// 리사이즈는 블로킹이라 워커로 나가고, 세션당 최신 `seq` 하나만 실행한다.
    pub resize: ResizeCoalescer,
    /// 선택 추출도 워커로 나간다(`selection_to_string()`이 선택 범위 전체를
    /// 훑는다). **세션당 직렬**이라 in-flight 가드를 둔다.
    pub extract_in_flight: bool,
    /// 추출이 도는 동안 도착한 복사 요청. 최신 하나만 남긴다 — 낡은 요청은
    /// epoch 가드가 어차피 거절하므로 쌓아둘 값이 없다.
    pub extract_pending: Option<CopyRequest>,
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
            resize: ResizeCoalescer::default(),
            extract_in_flight: false,
            extract_pending: None,
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
        mode: alacritty_terminal::term::TermMode::empty(),
        selection: None,
    }
}

pub struct SessionStore {
    slots: HashMap<SessionId, SessionSlot>,
    reaper: Reaper,
    /// `close()`/`accept_started`의 거절 경로로 넘어간 세션이 **실제로 어느
    /// 스레드에서** 떨어졌는지. 오직 Reaper의 콜백만 채운다 — 그 콜백이
    /// 실행되는 스레드가 곧 소멸자가 실행된 스레드라는 증거다.
    ///
    /// 테스트 관측용일 뿐이라(`reaper_drop_thread_for_test`) 프로덕션에서는
    /// 절대 채우지 않는다 — `track_reaped_at`이 꺼져 있으면(기본값)
    /// `retire`가 여기 아무것도 넣지 않는다. 앱 수명 내내 세션이 종료될
    /// 때마다 엔트리가 하나씩 쌓이기만 하고 지워지지 않아, 채우면 프로덕션이
    /// 무기한 자라는 맵을 하나 더 들고 있게 된다.
    reaped_at: Arc<Mutex<HashMap<SessionId, std::thread::ThreadId>>>,
    track_reaped_at: bool,
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
            track_reaped_at: false,
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
    /// - 이 결과가 지금 진행 중인 가드의 주인이면(`in_flight == Some(generation)`)
    ///   **가드부터 먼저 푼다** — 캐시가 이 결과보다 이미 앞서 있는지(stale)와
    ///   무관하게. 정상 경로에서 자기 결과가 캐시보다 낡아 도착할 일은 없지만,
    ///   그런 상황이 생겨도(예: generation 계산 버그, 재전송) "값을 버린다"가
    ///   "가드를 영영 못 푼다"로 번지면 안 된다 — 후자는 그 세션의 스냅샷이
    ///   다시는 갱신되지 않는 영구 프리즈다. 가드 해제는 되돌릴 수 없는 값
    ///   반영보다 훨씬 싼 보험이라 항상 먼저 처리한다.
    /// - 캐시보다 오래된 generation이면 **값은 버린다**(캐시 자체는 안 건드린다)
    /// - 값을 캐시에 반영한 뒤, 자기 요청의 결과가 아니었으면 여기서 끝낸다
    /// - 자기 요청이었고 `session.generation()`이 이미 더 나아가 있으면
    ///   다음 요청을 낸다 — 그러지 않으면 스냅샷이 도는 동안 도착한 출력이
    ///   영영 화면에 반영되지 않는다(구독은 그 generation을 이미 알렸으므로
    ///   다시 알리지 않는다). 이 재요청은 곧바로 스레드를 스폰하지 않고
    ///   `POLL_INTERVAL`만큼 늦춘다 — 바쁜 세션에서 스냅샷 완료마다 곧장
    ///   다음 스냅샷(~190KB 할당 + 전용 OS 스레드, `background.rs`는 스레드
    ///   풀이 없다)이 나가면 초당 수백 번씩 돌며 PTY 리더 스레드와 같은
    ///   `FairMutex`를 다툰다 — 알림 경로(`workbench::feed_stream`)와 같은
    ///   주기로 페이싱해 바쁜 세션도 ~16ms 주기에 안착시킨다. 가드는 여기서
    ///   곧바로 세운다 — 무거운 작업(스레드 스폰 + snapshot())만 늦춰야, 그
    ///   사이 도착하는 `SessionDirty`가 가드 없는 틈을 타 중복 요청을 내지
    ///   못한다.
    pub fn apply_snapshot(
        &mut self,
        id: SessionId,
        generation: u64,
        snapshot: TerminalSnapshot,
    ) -> Option<Task<Message>> {
        let slot = self.slots.get_mut(&id)?;

        let is_own_request = slot.snapshot_in_flight == Some(generation);
        if is_own_request {
            slot.snapshot_in_flight = None;
        }

        if generation < slot.snapshot_generation {
            return None; // 캐시보다 오래된 결과 — 값은 버린다(가드는 이미 풀었다)
        }
        slot.snapshot = snapshot;
        slot.snapshot_generation = generation;

        if !is_own_request {
            // 이 결과의 값은 캐시에 반영했지만(위에서 이미 최신임을 확인했다),
            // 이 요청이 지금 진행 중인 가드의 주인은 아니다 — 가드는 그대로 둔다.
            return None;
        }

        let current_generation = slot.session.generation();
        if current_generation <= generation {
            return None;
        }

        slot.snapshot_in_flight = Some(current_generation);
        let session = Arc::clone(&slot.session);
        let task = Task::future(async move {
            tokio::time::sleep(crate::workbench::POLL_INTERVAL).await;
        })
        .then(move |()| {
            let session = Arc::clone(&session);
            background::blocking(move |mut sender| {
                let snapshot = session.snapshot();
                let _ = sender.try_send(Message::SnapshotReady {
                    id,
                    generation: current_generation,
                    snapshot,
                });
            })
        });
        Some(task)
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

    /// 슬롯이 소유한(persist된) 모니터로 프로브를 한 번 돈다.
    /// `request_presence_with`의 블로킹 스레드 클로저가 이 함수를 부른다.
    /// `&mut self`를 받지 않는 이유는 `request_presence_with`가 이걸 블로킹
    /// 스레드로 옮겨 부르므로 `self`를 그 스레드로 가져갈 수 없어서다
    /// (`Arc<TerminalSession>`/`Arc<Mutex<..>>`만 넘긴다).
    ///
    /// 락을 `expect`로 풀지 않는다: 이 호출은 백그라운드 스레드에서 도는데
    /// (`request_presence_with`), 거기서 패닉하면 그 스레드는 그냥 죽고
    /// `PresenceReady`가 영영 보내지지 않는다 — `presence_in_flight` 가드가
    /// 영구히 묶여 그 세션의 존재 배지가 다시는 갱신되지 않는다(재시도
    /// 경로가 없다). 중독된 락이라도 안의 `PresenceMonitor`는 그저 캐시일
    /// 뿐이라(다음 성공한 프로브가 덮어쓴다) 잠긴 값을 그대로 회수해 계속
    /// 쓰는 쪽이 "다시는 갱신 안 됨"보다 훨씬 낫다.
    fn probe_with(
        session: &TerminalSession,
        monitor: &Mutex<PresenceMonitor>,
        probe: &dyn ProcessProbe,
    ) -> AgentPresence {
        let mut guard = monitor
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
        let track_reaped_at = self.track_reaped_at;
        self.reaper.retire_with_callback(session, move || {
            retired_count.fetch_add(1, Ordering::SeqCst);
            if track_reaped_at {
                if let Some(id) = id {
                    reaped_at
                        .lock()
                        .expect("reaped_at mutex poisoned")
                        .insert(id, std::thread::current().id());
                }
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

    /// 세션 핸들. `Arc`를 클론해 돌려주는 이유는 호출부가 세션을 부르면서 동시에
    /// `&mut SessionStore`가 필요하기 때문이다(예: 마우스 결과로 스냅샷 재요청).
    pub fn session(&self, id: SessionId) -> Option<Arc<TerminalSession>> {
        self.slots.get(&id).map(|slot| Arc::clone(&slot.session))
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

    /// 커스텀 위젯이 그릴 캐시된 스냅샷. 슬롯이 없으면 빈 스냅샷을 돌려준다 —
    /// 뷰는 `Option`을 다룰 자리가 아니고, 없는 세션은 빈 화면이 맞다.
    pub fn snapshot(&self, id: SessionId) -> &TerminalSnapshot {
        static EMPTY: std::sync::OnceLock<TerminalSnapshot> = std::sync::OnceLock::new();
        match self.slots.get(&id) {
            Some(slot) => &slot.snapshot,
            None => EMPTY.get_or_init(blank_snapshot),
        }
    }

    // ---- 리사이즈: 워커 + 세션당 최신 seq만 (Task 0.8 스레딩 정책 표) ----

    /// 위젯이 발행한 리사이즈를 합치기에 넣고, 실행하기로 결정됐으면 워커
    /// 태스크를 돌려준다. 결정을 함께 돌려주는 이유는 테스트가 "버려졌다"와
    /// "합쳐졌다"를 구별해야 하기 때문이다 — `Task`는 들여다볼 수 없다.
    pub fn request_resize(
        &mut self,
        id: SessionId,
        rows: u16,
        cols: u16,
        seq: u64,
    ) -> (ResizeDecision, Task<Message>) {
        let Some(slot) = self.slots.get_mut(&id) else {
            return (ResizeDecision::Discard, Task::none());
        };
        let decision = slot.resize.submit(rows, cols, seq);
        let task = match decision {
            ResizeDecision::Dispatch { rows, cols, seq } => {
                resize_task(Arc::clone(&slot.session), id, rows, cols, seq)
            }
            ResizeDecision::Coalesce | ResizeDecision::Discard => Task::none(),
        };
        (decision, task)
    }

    /// 워커가 하나를 끝냈다. 합치기에 대기 중이던 것이 있으면 이어서 보낸다.
    pub fn resize_completed(&mut self, id: SessionId, seq: u64) -> Task<Message> {
        let Some(slot) = self.slots.get_mut(&id) else {
            return Task::none();
        };
        match slot.resize.completed(seq) {
            Some((rows, cols, next)) => {
                resize_task(Arc::clone(&slot.session), id, rows, cols, next)
            }
            None => Task::none(),
        }
    }

    // ---- 선택 추출: 워커(세션당 직렬) ----

    /// 복사 요청을 워커로 보낸다. 이미 추출이 돌고 있으면 대기열에 **최신
    /// 하나만** 남기고 `false`를 돌려준다.
    pub fn request_extraction(
        &mut self,
        id: SessionId,
        request: CopyRequest,
    ) -> (bool, Task<Message>) {
        let Some(slot) = self.slots.get_mut(&id) else {
            return (false, Task::none());
        };
        if slot.extract_in_flight {
            slot.extract_pending = Some(request);
            return (false, Task::none());
        }
        slot.extract_in_flight = true;
        (true, extract_task(Arc::clone(&slot.session), id, request))
    }

    /// 추출 하나가 끝났다. 대기 중이던 요청이 있으면 이어서 보낸다.
    pub fn extraction_completed(&mut self, id: SessionId) -> Task<Message> {
        let Some(slot) = self.slots.get_mut(&id) else {
            return Task::none();
        };
        slot.extract_in_flight = false;
        match slot.extract_pending.take() {
            Some(request) => {
                slot.extract_in_flight = true;
                extract_task(Arc::clone(&slot.session), id, request)
            }
            None => Task::none(),
        }
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

/// 블로킹 리사이즈를 전용 스레드에서. **실패해도 완료 메시지를 반드시 보낸다** —
/// 안 보내면 합치기의 `in_flight`가 영영 풀리지 않아 그 세션은 다시는
/// 리사이즈되지 않는다(`apply_snapshot`이 가드를 값 반영보다 **먼저** 푸는 것과
/// 같은 이유다).
fn resize_task(
    session: Arc<TerminalSession>,
    id: SessionId,
    rows: u16,
    cols: u16,
    seq: u64,
) -> Task<Message> {
    background::blocking(move |mut sender| {
        let result = session.resize(rows, cols).map_err(|e| e.to_string());
        let _ = sender.try_send(Message::ResizeApplied { id, seq, result });
    })
}

/// 선택 추출을 전용 스레드에서. `extract_selection`이 epoch를 락 안에서 비교해
/// 불일치면 `None`을 돌려준다 — 그 `None`은 **조용한 취소**이지 오류가 아니다.
fn extract_task(
    session: Arc<TerminalSession>,
    id: SessionId,
    request: CopyRequest,
) -> Task<Message> {
    background::blocking(move |mut sender| {
        let text = session.extract_selection(request.epoch);
        let _ = sender.try_send(Message::SelectionExtracted {
            id,
            targets: request.to,
            text,
        });
    })
}

// ---- 테스트 전용 헬퍼. `#[cfg(test)]`가 아니라 `#[doc(hidden)]`인 이유:
// `tests/session_store_test.rs`는 별도 크레이트로 컴파일되어 라이브러리
// 크레이트의 `cfg(test)`를 보지 못한다. 공개 API 표면을 넓히지 않으려면
// 문서에서만 숨긴다. ----
impl SessionStore {
    #[doc(hidden)]
    pub fn for_test() -> Self {
        let mut store = Self::new();
        // 프로덕션은 `reaped_at`을 채우지 않는다(항목 6: 무기한 자라는 맵) —
        // 이 필드로 관측하는 테스트만 `for_test()`를 거치므로 여기서 켠다.
        store.track_reaped_at = true;
        store
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
    /// 건드릴 수 없다. `scroll_display(Scroll::Delta(0))`은 화면을 실제로 옮기지 않으면서도
    /// (delta 0) 호출마다 generation을 1씩 올린다(session.rs) — 이를 빌려
    /// "스냅샷이 도는 동안 출력이 더 들어왔다"는 상황을 재현한다.
    #[doc(hidden)]
    pub fn bump_generation_for_test(&mut self, id: SessionId, times: u32) {
        if let Some(slot) = self.slots.get(&id) {
            for _ in 0..times {
                slot.session
                    .scroll_display(alacritty_terminal::grid::Scroll::Delta(0));
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

#[cfg(test)]
mod resize_coalescer_tests {
    use super::*;

    /// 워커가 도는 동안 아무것도 안 오면 그대로 끝난다.
    #[test]
    fn a_single_resize_dispatches_and_completes() {
        let mut c = ResizeCoalescer::default();
        assert_eq!(
            c.submit(25, 100, 1),
            ResizeDecision::Dispatch {
                rows: 25,
                cols: 100,
                seq: 1
            }
        );
        assert_eq!(c.completed(1), None, "nothing was queued behind it");
    }

    /// **이 가드가 존재하는 이유.** 워커 두 개의 완료 순서가 뒤집히거나 메시지가
    /// 재전송되면 낡은 `seq`가 새 것 뒤에 도착한다. 그걸 받아들이면 사용자가
    /// 이미 지나온 크기로 PTY를 되돌려, 화면과 셸이 어긋난 채 남는다.
    #[test]
    fn an_older_seq_arriving_after_a_newer_one_is_discarded() {
        let mut c = ResizeCoalescer::default();

        assert_eq!(
            c.submit(25, 100, 5),
            ResizeDecision::Dispatch {
                rows: 25,
                cols: 100,
                seq: 5
            }
        );
        assert_eq!(c.completed(5), None);

        // 뒤늦게 도착한 낡은 것.
        assert_eq!(
            c.submit(10, 40, 3),
            ResizeDecision::Discard,
            "seq 3 is older than the seq 5 already applied"
        );
        // 같은 seq의 중복 전달도 새 요청이 아니다.
        assert_eq!(
            c.submit(25, 100, 5),
            ResizeDecision::Discard,
            "a duplicate delivery of seq 5 is not a new request"
        );

        // **대조군**: 더 새로운 것은 반드시 적용된다. 이게 없으면 위의 두
        // `Discard`가 "가드가 옳다"가 아니라 "이 타입이 아무것도 안 한다"로도
        // 설명된다.
        assert_eq!(
            c.submit(30, 120, 6),
            ResizeDecision::Dispatch {
                rows: 30,
                cols: 120,
                seq: 6
            },
            "control: a newer seq must still be applied"
        );
    }

    /// 드래그하는 동안 셀 경계를 넘을 때마다 리사이즈가 나온다. 워커가 하나
    /// 도는 사이 도착한 것들은 **마지막 하나로 뭉쳐야** 한다 — 순서대로 다
    /// 실행하면 워커 큐가 드래그보다 뒤처지고 중간 크기를 전부 PTY에 적용한다.
    #[test]
    fn resizes_arriving_during_a_dispatch_collapse_to_the_latest() {
        let mut c = ResizeCoalescer::default();

        assert!(matches!(
            c.submit(25, 100, 1),
            ResizeDecision::Dispatch { .. }
        ));
        assert_eq!(c.submit(26, 104, 2), ResizeDecision::Coalesce);
        assert_eq!(c.submit(27, 108, 3), ResizeDecision::Coalesce);
        assert_eq!(c.submit(28, 112, 4), ResizeDecision::Coalesce);

        assert_eq!(
            c.completed(1),
            Some((28, 112, 4)),
            "only the newest of the three queued resizes may run"
        );
        assert_eq!(
            c.completed(4),
            None,
            "and after it, the queue is empty — the middle sizes never ran"
        );
    }

    /// 합치기가 끝난 뒤에도 가드가 정상 상태로 돌아와야 한다. 여기가 새면
    /// `in_flight`가 영영 안 풀려 그 세션은 다시는 리사이즈되지 않는다.
    #[test]
    fn the_coalescer_returns_to_idle_and_keeps_working() {
        let mut c = ResizeCoalescer::default();

        assert!(matches!(
            c.submit(25, 100, 1),
            ResizeDecision::Dispatch { .. }
        ));
        assert_eq!(c.submit(26, 104, 2), ResizeDecision::Coalesce);
        assert_eq!(c.completed(1), Some((26, 104, 2)));
        assert_eq!(c.completed(2), None);

        // 유휴로 돌아왔으니 다음 것은 다시 곧바로 나가야 한다(Coalesce가 아니라).
        assert_eq!(
            c.submit(27, 108, 3),
            ResizeDecision::Dispatch {
                rows: 27,
                cols: 108,
                seq: 3
            },
            "after the queue drains the next resize must dispatch immediately"
        );
    }

    /// **대기열에서 꺼낸 것도 in-flight다.** `completed`가 꺼내 돌려준 리사이즈는
    /// 호출자가 곧바로 워커로 보내므로, 그 시점부터 다시 워커가 돌고 있다.
    /// 여기서 가드를 다시 세우지 않으면 코얼레서는 유휴라고 착각하고, 직후에
    /// 도착한 리사이즈를 곧바로 dispatch해 **블로킹 리사이즈 두 개가 동시에
    /// 돈다** — 나중에 끝난 쪽이 최종 크기가 되어 순서 보장이 깨진다.
    ///
    /// (이 테스트는 mutation이 살아남아서 추가됐다: `completed`에서
    /// `in_flight = Some(next)`를 지워도 기존 테스트가 전부 통과했다.)
    #[test]
    fn a_drained_resize_is_itself_in_flight() {
        let mut c = ResizeCoalescer::default();

        assert!(matches!(
            c.submit(25, 100, 1),
            ResizeDecision::Dispatch { .. }
        ));
        assert_eq!(c.submit(26, 104, 2), ResizeDecision::Coalesce);
        assert_eq!(
            c.completed(1),
            Some((26, 104, 2)),
            "precondition: seq 2 is now the one running"
        );

        assert_eq!(
            c.submit(27, 108, 3),
            ResizeDecision::Coalesce,
            "seq 2 is still running, so seq 3 must queue behind it — dispatching \\
             here would run two blocking resizes at once"
        );
        // 그리고 그 2가 끝나야 비로소 3이 나간다.
        assert_eq!(c.completed(2), Some((27, 108, 3)));
    }

    /// 남의 완료 알림이 이쪽 가드를 풀면 리사이즈 두 개가 동시에 돌고, 나중에
    /// 끝난 쪽이 최종 크기가 되어 순서 보장이 깨진다.
    #[test]
    fn a_completion_for_a_different_seq_does_not_release_the_guard() {
        let mut c = ResizeCoalescer::default();

        assert!(matches!(
            c.submit(25, 100, 7),
            ResizeDecision::Dispatch { .. }
        ));
        assert_eq!(
            c.completed(6),
            None,
            "a stale completion must not drain the queue"
        );

        // 가드가 아직 살아 있으므로 새 요청은 합쳐져야 한다.
        assert_eq!(
            c.submit(26, 104, 8),
            ResizeDecision::Coalesce,
            "the guard must still be held — a foreign completion released it if this dispatches"
        );
        // 그리고 진짜 완료가 오면 그때 대기열이 풀린다.
        assert_eq!(c.completed(7), Some((26, 104, 8)));
    }
}
