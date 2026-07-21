use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use futures::StreamExt;
use iced::advanced::clipboard;
use iced::widget::pane_grid;
use suaegi_core::domain::{
    PersistedState, Repo, RepoId, SessionState, Settings, Worktree, WorktreeId, SCHEMA_VERSION,
};
use suaegi_git::worktree::{BranchDeletion, CreatedWorktree, RemoveOutcome, WorktreeEntry};
use suaegi_term::agent::AgentKind;
use suaegi_term::grid::TerminalSnapshot;
use suaegi_term::input_types::{CopyTargets, WriteOutcome};
use suaegi_term::presence::AgentPresence;

use crate::agent_status::contract::{HookEvent, HydrationStep};
use crate::persistence_thread::{
    LoadDiagnostics, LoadOrigin, PersistenceHandle, SaveReport, SaveStatus,
};
use crate::session_store::{SessionId, SessionStore, StartedSession};
use crate::terminal::contract::TermCommand;

/// 포커스 전환이 내야 할 `FOCUS_IN_OUT` 리포트를 **순서대로**.
///
/// **순서가 계약이다**: 이전 세션에 focus-out을 먼저, 그다음 새 세션에 focus-in.
/// 뒤집으면 두 세션이 동시에 자기가 포커스를 쥐고 있다고 믿는 창이 생기고, 그
/// 창에서 셸이 그린 것(예: 포커스에 따라 커서 모양을 바꾸는 TUI)이 어긋난다.
///
/// **순수 함수로 뽑은 이유**는 이것이 헤드리스로 확인할 수 있는 유일한 형태이기
/// 때문이다: `report_focus`가 실제로 바이트를 내는 것은 셸이 `FOCUS_IN_OUT`을
/// 켰을 때뿐이라(평범한 셸은 켜지 않는다) 바이트를 관찰해 순서를 볼 수 없다.
/// 순서 결정을 값으로 만들면 그 결정만은 정확히 검사할 수 있다.
fn focus_reports(previous: Option<SessionId>, next: Option<SessionId>) -> Vec<(SessionId, bool)> {
    // 같은 pane을 다시 눌렀다. 리포트를 또 내면 셸이 focus-in을 두 번 받는다.
    if previous == next {
        return Vec::new();
    }
    let mut reports = Vec::new();
    if let Some(previous) = previous {
        reports.push((previous, false));
    }
    if let Some(next) = next {
        reports.push((next, true));
    }
    reports
}

/// 추출된 선택 텍스트를 **요청된 클립보드에만** 쓴다.
///
/// 기본값은 호출부가 정한다: 명시적 복사는 양쪽(`CopyTargets::EXPLICIT`),
/// 드래그 완료는 primary에만(`DRAG_COMPLETE`) — X11/Wayland의 중클릭 붙여넣기
/// 관례다. Primary는 macOS/Windows에서 no-op이므로 양쪽에 쓰는 것이 안전하다.
fn clipboard_writes(targets: CopyTargets, text: String) -> iced::Task<Message> {
    iced::Task::batch(clipboard_kinds(targets).into_iter().map(|kind| match kind {
        clipboard::Kind::Standard => iced::clipboard::write(text.clone()),
        clipboard::Kind::Primary => iced::clipboard::write_primary(text.clone()),
    }))
}

/// 어느 클립보드에 쓸 것인가. **`Task`는 들여다볼 수 없으므로** 결정을 값으로
/// 뽑아야 검사할 수 있다 — 그리고 이건 검사할 값이 있는 결정이다: 드래그 완료가
/// standard까지 쓰면 사용자가 복사한 적 없는 텍스트가 시스템 클립보드를 덮어쓴다.
fn clipboard_kinds(targets: CopyTargets) -> Vec<clipboard::Kind> {
    let mut kinds = Vec::new();
    if targets.standard {
        kinds.push(clipboard::Kind::Standard);
    }
    if targets.primary {
        kinds.push(clipboard::Kind::Primary);
    }
    kinds
}

/// 비동기 작업 하나를 식별한다. 결과가 순서를 바꿔 도착해도 대상을 잃지 않게 한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OpId(pub u64);

#[derive(Debug, Clone)]
pub enum Message {
    RepoProbed {
        request: OpId,
        requested_path: PathBuf,
        result: Result<(Repo, Option<String>), String>,
    },
    WorktreesListed {
        request: OpId,
        repo_id: RepoId,
        result: Result<Vec<WorktreeEntry>, String>,
    },
    WorktreeCreated {
        request: OpId,
        repo_id: RepoId,
        result: Result<CreatedWorktree, String>,
    },
    WorktreeRemoved {
        request: OpId,
        repo_id: RepoId,
        worktree_id: WorktreeId,
        result: Result<RemoveOutcome, String>,
    },

    // ---- Task 4: sidebar interactions ----
    RepoPathInputChanged(String),
    AddRepoSubmitted,
    WorktreeNameInputChanged {
        repo_id: RepoId,
        value: String,
    },
    CreateWorktreeSubmitted {
        repo_id: RepoId,
    },
    RemoveWorktreeRequested {
        repo_id: RepoId,
        worktree_id: WorktreeId,
        worktree_path: PathBuf,
        branch: Option<String>,
    },
    /// UI 선택 표시만 한다. worktree 선택으로 세션을 시작하는 것은 Task 5의 몫이다.
    WorktreeSelected(WorktreeId),
    /// 영속화 스레드(Task 2)의 저장 결과. `AppState::boot`이 `PersistenceHandle`을
    /// 스폰하며 `results` 스트림을 `Task::stream(...)`으로 여기로 연결한다.
    Saved(SaveReport),

    // ---- Task 5: session_store.rs의 비동기 결과. `AppState`가 `SessionStore`를
    // 들고 이 메시지들을 실제로 처리하는 배선은 Task 6/7(워크벤치 UI)의 몫이다
    // — 지금은 `Message`가 컴파일되도록 변형만 미리 만들어 둔다(Task 1의
    // "뒤 태스크가 참조할 공용 타입은 여기서 미리 만든다" 원칙과 대칭이다). ----
    /// `SessionStore::start`의 완료. 실패도 `id`/`worktree_id` 맥락을 나른다.
    SessionStarted {
        id: SessionId,
        worktree_id: WorktreeId,
        result: Result<StartedSession, String>,
    },
    /// `SessionStore::request_snapshot`의 완료.
    SnapshotReady {
        id: SessionId,
        generation: u64,
        snapshot: TerminalSnapshot,
    },
    /// `SessionStore::request_presence`의 완료.
    PresenceReady {
        id: SessionId,
        generation: u64,
        presence: AgentPresence,
    },
    /// `presence_poll::subscription`의 티어링된 타이머 틱. 그 자체로는 화면을
    /// 갱신하지 않는다 — in-flight가 아닌 세션마다 `request_presence`를 내는
    /// 트리거일 뿐이다.
    PresenceTick,

    // ---- Task 6: workbench.rs의 pane_grid + 세션 구독 ----
    /// 세션별 구독(`workbench::subscription`)이 `generation()` 변화를 감지했다는
    /// 알림. 그 자체로는 화면을 갱신하지 않는다 — 캐시된 스냅샷을 다시 뜨라는
    /// 요청을 `SessionStore::request_snapshot`에 넘길 뿐이다.
    SessionDirty {
        id: SessionId,
        generation: u64,
    },
    /// pane_grid가 클릭된 pane을 알린다. 포커스 갱신용.
    PaneClicked(pane_grid::Pane),
    /// pane_grid 드래그 앤 드롭 상호작용. `Dropped`만 레이아웃을 바꾼다.
    PaneDragged(pane_grid::DragEvent),
    /// pane_grid 분할선 리사이즈.
    PaneResized(pane_grid::ResizeEvent),
    /// 타이틀바 닫기 버튼. 마지막 pane이면 pane_grid 자체를 비운다(pane_grid는
    /// 마지막 pane을 `close()`로 지울 수 없다 — 형제가 없기 때문).
    PaneCloseRequested(pane_grid::Pane),

    // ---- Plan 4 Task 7: 터미널 위젯 배선 ----
    /// 터미널 위젯이 발행한 커맨드. 위젯은 세션을 **절대 만지지 않는다** —
    /// 여기가 세션에 닿는 유일한 지점이고, 그 경계가 위젯 테스트 가능성의
    /// 근거다. 실행 스레드는 Task 0.8의 정책 표를 따른다.
    Terminal {
        id: SessionId,
        command: TermCommand,
    },
    /// 리사이즈 워커의 완료. 합치기의 in-flight 가드를 풀고, 대기 중이던
    /// 최신 리사이즈가 있으면 이어서 보낸다. **실패해도 반드시 온다** —
    /// 안 오면 그 세션은 다시는 리사이즈되지 않는다.
    ResizeApplied {
        id: SessionId,
        seq: u64,
        result: Result<(), String>,
    },
    /// 선택 추출 워커의 완료. `text: None`은 **조용한 취소**다(epoch 불일치
    /// 또는 선택 없음) — 오류를 띄우지 않는다. `Some`이면 요청된
    /// Standard/Primary에 **정확히 그것만** 쓴다.
    SelectionExtracted {
        id: SessionId,
        targets: CopyTargets,
        text: Option<String>,
    },

    // ---- Plan 5 Task 0.6: 훅·diff·복원 ----
    /// 훅 서버가 정규화해 보낸 이벤트. **`OpId`를 갖지 않는다** — 요청에 대한
    /// 응답이 아니라 푸시이고, 상관관계 키는 `PaneKey`뿐이다. `BadgeChanged`가
    /// 없는 것도 같은 이유다: 배지는 `agent_status::contract::reduce`에서
    /// 파생되지 전달되지 않는다.
    HookArrived(HookEvent),
    /// 배지 재계산 틱. `PresenceTick`과 같은 티어에 둔다 — 나이 기반 규칙
    /// (`HOOK_STALE_AFTER`)은 새 이벤트가 없어도 상태를 바꾸므로 무언가가
    /// 주기적으로 `reduce`를 다시 불러야 한다.
    BadgeTick,
    DiffRequested {
        worktree: WorktreeId,
        op: OpId,
    },
    FileDiffRequested {
        worktree: WorktreeId,
        path: String,
        op: OpId,
    },
    /// 패널을 닫았다. 진행 중인 compare를 취소한다 — **취소는 오류가 아니다**,
    /// 배너를 띄우지 않고 조용히 끝난다.
    DiffCancelled {
        worktree: WorktreeId,
    },
    // TODO(Plan 5): `DiffLoaded`/`FileDiffLoaded`는 여기 없다. 페이로드 타입
    // (`CompareOutcome`, `FileDiff`)이 `suaegi-git`의 Task 1 산출물인데 Task 1은
    // Task 0 뒤에 시작하도록 되어 있어, Task 0 시점에는 참조할 타입이 존재하지
    // 않는다. 추측으로 만들지 않는다 — Task 1이 타입을 확정하면서 이 두 변형을
    // 함께 추가한다. (`FileDiff`는 플랜 어디에도 정의가 없다는 점도 같이 해결해야
    // 한다. 지금 `suaegi_git::compare::file_diff`는 patch `String`을 돌려준다.)
    /// 하이드레이션 게이트의 진행. 셋이 모두 도착해야 `persist()`가 풀린다.
    HydrationStep(HydrationStep),
    /// 레이아웃 저장 디바운스 타이머의 만료. **최신 세대만 저장한다** — 리사이즈
    /// 메시지마다 세대를 올리므로 앞선 타이머는 여기서 걸러진다.
    LayoutPersistDue {
        generation: u64,
    },
}

pub struct AppState {
    /// repo별로 마지막에 발급한 목록 요청의 OpId. 그보다 오래된 응답은 버린다.
    latest_list_op: HashMap<RepoId, OpId>,
    worktrees_by_repo: HashMap<RepoId, Vec<WorktreeEntry>>,

    /// 등록된 repo 목록. `HashMap` 순서가 아니라 등록 순서를 보존해 사이드바
    /// 그룹 순서가 프레임마다 흔들리지 않게 한다.
    repos: Vec<Repo>,
    repo_path_input: String,
    /// repo별 "새 worktree 이름" 입력창의 임시 값.
    worktree_name_draft: HashMap<RepoId, String>,
    selected_worktree: Option<WorktreeId>,
    /// 가장 최근 git 작업(등록/목록/생성/삭제) 실패 메시지. 다음 실패가 오면
    /// 덮어쓴다 — worktree마다 개별 배지를 다는 건 Task 7 이후 범위.
    last_error: Option<String>,
    next_op_id: u64,
    workspace_root: PathBuf,

    /// 사이드바 상태 표시줄이 읽는 영속화 진단 정보. `AppState::boot`이
    /// `PersistenceHandle::spawn`의 `LoadDiagnostics`로 채운다. 기본값
    /// (`Fresh`/`None`)은 플레인 `AppState::default()`(테스트 전반에서 쓰는)가
    /// 헛경고를 내지 않기 위한 안전한 값이다.
    load_origin: LoadOrigin,
    last_save_status: Option<SaveStatus>,
    /// `None`이면 저장이 배선되지 않은 상태(테스트, 또는 미래에 실패한 부팅) —
    /// `persist()`는 조용히 아무것도 하지 않는다. 실 앱 경로에서는 `boot()`이
    /// 항상 `Some`을 채운다.
    persistence: Option<PersistenceHandle>,

    // ---- Task 6: 세션 생명주기 + 워크벤치 배선 ----
    session_store: SessionStore,
    /// `None`이면 열린 세션이 없다는 뜻 — `pane_grid::State::new`는 첫 pane 없이
    /// 만들 수 없으므로(항상 최소 하나) 첫 세션이 열릴 때 비로소 생성한다.
    panes: Option<pane_grid::State<SessionId>>,
    focused_pane: Option<pane_grid::Pane>,
    /// worktree당 세션 하나. 이미 열린 worktree를 다시 선택하면 새 세션을 또
    /// 띄우지 않고 기존 pane에 포커스만 옮긴다.
    worktree_sessions: HashMap<WorktreeId, SessionId>,
    /// `worktree_sessions`의 역방향 조회 — pane을 닫을 때 어느 worktree의
    /// 자리가 비었는지 알아야 한다.
    session_worktrees: HashMap<SessionId, WorktreeId>,
    /// 세션 시작을 요청했지만 아직 `SessionStarted`가 도착하지 않은 worktree.
    /// 없으면 같은 worktree를 두 번 빠르게 클릭했을 때 세션이 두 개 뜬다.
    pending_session_starts: HashMap<WorktreeId, SessionId>,
    /// 제거 요청을 보냈지만 `WorktreeRemoved` 응답이 아직 안 온 worktree.
    /// `RemoveWorktreeRequested`가 세션을 닫는 건 그 시점에 `worktree_sessions`에
    /// 이미 올라온 세션뿐이다 — 시작 요청이 in flight인 채로(`pending_session_starts`)
    /// 제거가 시작되면, git 삭제가 끝나 `worktrees_by_repo`가 갱신되기 전까지는
    /// `worktree_still_exists`가 여전히 `true`를 돌려줘 그 사이 도착하는
    /// `SessionStarted`가 산 슬롯으로 받아들여지고, 그 세션은 아무도 닫지 않아
    /// PTY와 스레드가 샌다. 이 집합이 그 창을 막는다: `worktree_still_exists`는
    /// 여기 있는 worktree를 항상 "없다"고 답한다.
    pending_worktree_removals: HashSet<WorktreeId>,
    /// pane 타이틀바에 쓰는 표시용 이름. 세션 시작을 요청한 시점에 미리
    /// 채워둔다 — `SessionStarted`가 도착하기 전에도(또는 실패해도) 어떤
    /// worktree를 위한 시도였는지 사용자에게 보여줄 수 있다.
    session_titles: HashMap<SessionId, String>,

    // ---- Task 7: 존재 폴링 ----
    /// `SessionStore::request_presence`에 넘길, 계속 증가하는 시퀀스. 프레즌스
    /// 요청은 세션당 한 번에 하나만 진행되므로(`presence_in_flight`는 bool)
    /// 이 값 자체가 요청을 식별하지는 않지만, `apply_presence`의 staleness
    /// 비교(`generation >= slot.presence_generation`)가 항상 최신 값을
    /// 받아들이도록 단조 증가를 보장한다.
    next_presence_seq: u64,

    // ---- Plan 4 Task 7: 터미널 입력 유실 피드백 ----
    /// 가장 최근에 **입력을 유실한** 세션. `WriteOutcome::Dropped`(쓰기 큐 상한
    /// 256에 못 넣었다 = 사용자가 친 것이 사라졌다)에서만 세운다.
    /// **`Suppressed`는 여기 오지 않는다** — 모드상 보낼 바이트가 없었을 뿐
    /// 유실이 아니고, 그걸 경고로 띄우면 정상 동작이 오류로 보인다.
    last_input_loss: Option<SessionId>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            latest_list_op: HashMap::new(),
            worktrees_by_repo: HashMap::new(),
            repos: Vec::new(),
            repo_path_input: String::new(),
            worktree_name_draft: HashMap::new(),
            selected_worktree: None,
            last_error: None,
            next_op_id: 0,
            // `suaegi_core`의 기본 workspace root 계산을 재사용한다 (홈 디렉터리
            // 아래 `suaegi-workspaces`) — 여기서 `dirs`에 직접 의존하지 않는다.
            workspace_root: PersistedState::default().settings.workspace_root,
            load_origin: LoadOrigin::Fresh,
            last_save_status: None,
            persistence: None,
            session_store: SessionStore::new(),
            panes: None,
            focused_pane: None,
            worktree_sessions: HashMap::new(),
            session_worktrees: HashMap::new(),
            pending_session_starts: HashMap::new(),
            pending_worktree_removals: HashSet::new(),
            session_titles: HashMap::new(),
            next_presence_seq: 0,
            last_input_loss: None,
        }
    }
}

/// git이 돌려주는 `WorktreeEntry`에는 안정적인 id가 없다. `RepoId`가 정규화된
/// 절대 경로 문자열이듯, worktree 경로도 이미 canonical absolute path다
/// (`add_worktree`가 canonicalize한 parent 아래 만든다) — 같은 규칙을 따른다.
/// 사이드바(선택/삭제 버튼)와 워크벤치(worktree → 세션 매핑)가 같은 규칙을
/// 공유해야 하므로 여기 한 곳에 둔다.
pub(crate) fn worktree_id_for(path: &Path) -> WorktreeId {
    WorktreeId(path.to_string_lossy().into_owned())
}

impl AppState {
    /// 부팅 경로. `PersistenceHandle::spawn`이 창이 뜨기 전에 동기로 1회 로드를
    /// 끝내고(`docs` Global Constraints — UI를 막지 않는다), 그 결과로 초기
    /// `AppState`를 만든다. 반환하는 `Task`는 두 가지를 한다: (1) 복원된 repo마다
    /// 최신 worktree 목록을 git에서 다시 받아온다(디스크에 저장된 목록은 앱이
    /// 닫힌 사이 바뀌었을 수 있는 스냅샷일 뿐이라 git이 항상 최종 권위다),
    /// (2) 저장 결과 채널(`results`)을 `Message::Saved`로 흘려보내 상태
    /// 표시줄이 실제로 반응하게 한다 — 이 배선이 없으면 `Message::Saved`는
    /// 영영 도착하지 않는 메시지로 남는다.
    pub fn boot() -> (AppState, iced::Task<Message>) {
        let boot = PersistenceHandle::spawn(crate::persistence_thread::default_data_file());
        let mut state = AppState::from_load(boot.load);
        state.persistence = Some(boot.handle);

        let refresh_tasks: Vec<iced::Task<Message>> = state
            .repos
            .iter()
            .map(|repo| repo.id.clone())
            .collect::<Vec<_>>()
            .into_iter()
            .map(|repo_id| state.refresh_worktrees(repo_id))
            .collect();

        let saved_task = iced::Task::stream(boot.results.map(Message::Saved));

        let mut tasks = refresh_tasks;
        tasks.push(saved_task);
        (state, iced::Task::batch(tasks))
    }

    /// `PersistenceHandle::spawn`이 돌려주는 `LoadDiagnostics`로 초기 상태를
    /// 채운다. `state.rs`/`sidebar.rs` 테스트가 실제 부팅 경로(손으로 필드를
    /// 세우는 `AppState::fresh()` 등의 테스트 헬퍼가 아니라)를 태워
    /// `LoadOrigin`이 상태 표시줄까지 실제로 흘러가는지 검증할 때도 이 함수를
    /// 그대로 쓴다.
    pub(crate) fn from_load(load: LoadDiagnostics) -> AppState {
        let mut state = AppState::default();
        state.repos = load.state.repos;
        state.workspace_root = load.state.settings.workspace_root;
        state.load_origin = load.origin;
        // 디스크에 저장된 worktree 목록을 그대로 신뢰하지 않고 화면에 먼저
        // 보여주기 위한 최선의 추정치로만 쓴다 — `boot()`이 곧바로 git 재조회를
        // 발급해 정정한다(위 문서 참고). `latest_list_op`는 일부러 세우지 않는다:
        // 재조회가 발급하는 첫 `OpId`가 무엇이든 이 씨딩보다 새것으로 취급돼야
        // 하고, `apply_worktree_listing`은 `latest_list_op`에 없는 repo의 응답을
        // 무조건 받아들이므로 그냥 두면 된다.
        let mut worktrees_by_repo: HashMap<RepoId, Vec<WorktreeEntry>> = HashMap::new();
        for worktree in load.state.worktrees {
            worktrees_by_repo
                .entry(worktree.repo_id.clone())
                .or_default()
                .push(WorktreeEntry {
                    path: worktree.path,
                    branch: Some(worktree.branch),
                    head: None,
                    is_main: false,
                });
        }
        state.worktrees_by_repo = worktrees_by_repo;
        state
    }

    /// 지금 화면에 있는 repo/worktree/선택 상태를 `PersistedState`로 스냅샷
    /// 뜬다. worktree 쪽은 git 목록(`WorktreeEntry`)에서 도메인 `Worktree`를
    /// 새로 합성한다 — 생성 시각/생성 에이전트 같은 메타데이터는 이 씨딩
    /// 시점에 알 수 없으므로 기본값을 쓴다(세션 레이아웃 복원은 Plan 5).
    fn persisted_snapshot(&self) -> PersistedState {
        let worktrees = self
            .worktrees_by_repo
            .iter()
            .flat_map(|(repo_id, entries)| {
                entries.iter().map(move |entry| Worktree {
                    id: worktree_id_for(&entry.path),
                    repo_id: repo_id.clone(),
                    path: entry.path.clone(),
                    branch: entry.branch.clone().unwrap_or_default(),
                    display_name: entry
                        .branch
                        .clone()
                        .unwrap_or_else(|| "worktree".to_string()),
                    created_with_agent: None,
                    created_at_unix_ms: 0,
                })
            })
            .collect();
        PersistedState {
            schema_version: SCHEMA_VERSION,
            repos: self.repos.clone(),
            worktrees,
            session: SessionState {
                active_worktree_id: self.selected_worktree.clone(),
            },
            settings: Settings {
                workspace_root: self.workspace_root.clone(),
            },
        }
    }

    /// 영속화 대상 상태(repo/worktree/선택)가 바뀌었을 때 부른다. 배선이 안 된
    /// 상태(`persistence == None`, 테스트 기본값)에서는 조용히 아무것도 하지
    /// 않는다.
    fn persist(&self) {
        if let Some(handle) = &self.persistence {
            handle.save(self.persisted_snapshot());
        }
    }

    /// 목록 요청을 발급한 시점에 호출한다. 이후 그보다 오래된 `OpId`로 도착하는
    /// 응답은 `apply_worktree_listing`이 버린다.
    pub fn note_list_issued(&mut self, repo: RepoId, op: OpId) {
        self.latest_list_op.insert(repo, op);
    }

    /// `op`가 해당 repo에 대해 마지막으로 발급된 목록 요청보다 오래됐으면 버린다.
    /// 생성/삭제 직후 재조회한 최신 목록이, 그 전에 발급됐던 목록의 뒤늦은 응답에
    /// 덮어써지는 것을 막는다.
    ///
    /// 이 앱을 거치지 않고 밖에서(다른 터미널, 다른 도구) worktree가
    /// 지워졌을 수도 있다 — `RemoveWorktreeRequested` 경로를 타지 않았으므로
    /// 그 세션은 아무도 닫지 않는다. 새 목록에서 사라진 worktree를 여기서
    /// 찾아 세션을 닫는다(Reaper로) — 그러지 않으면 PTY/스레드/pane/구독이
    /// 그 세션의 `Arc`를 계속 붙들고 영원히 산다.
    pub fn apply_worktree_listing(&mut self, repo: RepoId, op: OpId, entries: Vec<WorktreeEntry>) {
        if let Some(latest) = self.latest_list_op.get(&repo) {
            if op.0 < latest.0 {
                return;
            }
        }
        let still_present: HashSet<WorktreeId> =
            entries.iter().map(|e| worktree_id_for(&e.path)).collect();
        let vanished_sessions: Vec<SessionId> = self
            .worktrees_by_repo
            .get(&repo)
            .into_iter()
            .flatten()
            .map(|e| worktree_id_for(&e.path))
            .filter(|id| !still_present.contains(id))
            .filter_map(|id| self.worktree_sessions.get(&id).copied())
            .collect();
        self.worktrees_by_repo.insert(repo, entries);
        for session_id in vanished_sessions {
            self.close_session(session_id);
        }
    }

    pub fn worktree_names(&self, repo: &RepoId) -> Vec<String> {
        self.worktrees_by_repo
            .get(repo)
            .map(|entries| entries.iter().filter_map(|e| e.branch.clone()).collect())
            .unwrap_or_default()
    }

    // ---- Task 4: accessors the sidebar view (and its pure helpers) read ----

    pub(crate) fn repos(&self) -> &[Repo] {
        &self.repos
    }

    pub(crate) fn worktrees_for(&self, repo: &RepoId) -> &[WorktreeEntry] {
        self.worktrees_by_repo
            .get(repo)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub(crate) fn repo_path_input(&self) -> &str {
        &self.repo_path_input
    }

    pub(crate) fn worktree_name_draft(&self, repo: &RepoId) -> &str {
        self.worktree_name_draft
            .get(repo)
            .map(String::as_str)
            .unwrap_or("")
    }

    pub(crate) fn selected_worktree(&self) -> Option<&WorktreeId> {
        self.selected_worktree.as_ref()
    }

    // ---- Task 6: accessors the workbench view (and its subscription) read ----

    pub(crate) fn panes(&self) -> Option<&pane_grid::State<SessionId>> {
        self.panes.as_ref()
    }

    pub(crate) fn session_store(&self) -> &SessionStore {
        &self.session_store
    }

    // ---- Task 7: accessors presence_poll (and its tests) read/mutate ----

    pub(crate) fn session_store_mut(&mut self) -> &mut SessionStore {
        &mut self.session_store
    }

    /// `request_presence`에 넘길 다음 시퀀스 값. 호출마다 증가한다.
    pub(crate) fn next_presence_seq(&mut self) -> u64 {
        self.next_presence_seq += 1;
        self.next_presence_seq
    }

    /// worktree 하나에 세션이 열려 있으면 그 세션의 존재 판정을, 아니면
    /// `Unknown`을 돌려준다(세션이 없으면 판정할 게 없다 — `NoAgent`로
    /// 단정하면 "에이전트가 없다"와 "아직 아무것도 모른다"를 혼동한다).
    /// 사이드바가 worktree 행의 존재 배지를 그릴 때 읽는다.
    pub(crate) fn worktree_presence(&self, worktree_id: &WorktreeId) -> AgentPresence {
        self.worktree_sessions
            .get(worktree_id)
            .map(|&id| self.session_store.presence(id))
            .unwrap_or(AgentPresence::Unknown)
    }

    pub(crate) fn session_title(&self, id: SessionId) -> &str {
        self.session_titles
            .get(&id)
            .map(String::as_str)
            .unwrap_or("session")
    }

    pub(crate) fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub(crate) fn load_origin(&self) -> LoadOrigin {
        self.load_origin
    }

    pub(crate) fn last_save_status(&self) -> Option<&SaveStatus> {
        self.last_save_status.as_ref()
    }

    /// 존재하면 갱신, 없으면 등록 순서 끝에 추가한다 (등록 순서를 보존한다).
    pub(crate) fn upsert_repo(&mut self, repo: Repo) {
        if let Some(existing) = self.repos.iter_mut().find(|r| r.id == repo.id) {
            *existing = repo;
        } else {
            self.repos.push(repo);
        }
    }

    pub(crate) fn repo_by_id(&self, id: &RepoId) -> Option<&Repo> {
        self.repos.iter().find(|r| &r.id == id)
    }

    fn next_op(&mut self) -> OpId {
        self.next_op_id += 1;
        OpId(self.next_op_id)
    }

    /// 목록 재조회를 발급하고 staleness 가드에 기록한다. repo가 이미 사라졌으면
    /// (드물지만 삭제와 경합) 조용히 아무 것도 하지 않는다.
    fn refresh_worktrees(&mut self, repo_id: RepoId) -> iced::Task<Message> {
        let Some(repo) = self.repo_by_id(&repo_id).cloned() else {
            return iced::Task::none();
        };
        let op = self.next_op();
        self.note_list_issued(repo_id, op);
        crate::git_tasks::list_worktrees(op, repo)
    }

    /// `worktree_id`에 해당하는 (repo, entry) 쌍을 찾는다. `SessionStore::start`가
    /// 요구하는 `Worktree` 도메인 값을 만들려면 어느 repo 소속인지가 필요하지만
    /// `WorktreeEntry` 자체는 그걸 모른다(git이 그렇게 준다) — 그래서
    /// `worktrees_by_repo`를 repo별로 순회해 경로로 역매칭한다.
    fn find_worktree(&self, id: &WorktreeId) -> Option<(RepoId, WorktreeEntry)> {
        self.worktrees_by_repo
            .iter()
            .find_map(|(repo_id, entries)| {
                entries
                    .iter()
                    .find(|entry| worktree_id_for(&entry.path) == *id)
                    .map(|entry| (repo_id.clone(), entry.clone()))
            })
    }

    /// `accept_started`가 늦게 도착한 시작 결과를 받아들일지 판단하는 데 쓴다.
    /// 세션 스토어는 어떤 worktree가 살아 있는지 모르므로(`session_store.rs`
    /// 문서 참고) 호출자인 여기서 판단해 넘겨준다.
    ///
    /// `pending_worktree_removals`를 먼저 본다: 제거가 진행 중인 동안은
    /// `worktrees_by_repo`가 아직 예전 값을 들고 있을 수 있다(git 삭제가 끝나고
    /// 목록을 다시 받아올 때까지 갱신되지 않는다) — 그 lag를 `worktrees_by_repo`
    /// 만으로 판단하면 제거 중인 worktree로 걸어들어오는 `SessionStarted`가
    /// "아직 있다"고 오판되어 산 슬롯으로 받아들여지고, 그 세션은 아무도 닫지
    /// 않아 PTY와 스레드가 샌다.
    fn worktree_still_exists(&self, id: &WorktreeId) -> bool {
        if self.pending_worktree_removals.contains(id) {
            return false;
        }
        self.worktrees_by_repo
            .values()
            .any(|entries| entries.iter().any(|e| worktree_id_for(&e.path) == *id))
    }

    /// `WorktreeRemoved`의 성공 경로에서, 재조회 응답(`WorktreesListed`)이
    /// 도착하기 전에도 곧바로 목록에서 지운다. 재조회에만 맡기면 "git 삭제는
    /// 끝났지만 목록은 아직 갱신 전"인 창이 남아 `worktree_still_exists`가 그
    /// 창 동안은 여전히 `pending_worktree_removals`에만 의존하게 된다 — 이중
    /// 방어로 그 창을 최대한 좁힌다.
    fn remove_worktree_entry(&mut self, repo_id: &RepoId, worktree_id: &WorktreeId) {
        if let Some(entries) = self.worktrees_by_repo.get_mut(repo_id) {
            entries.retain(|entry| worktree_id_for(&entry.path) != *worktree_id);
        }
    }

    /// 첫 세션이면 `pane_grid::State`를 새로 만든다(pane_grid는 pane 없이
    /// 존재할 수 없다). 이후로는 포커스된 pane(없으면 아무 pane)을 수평
    /// 분할한다.
    fn open_pane_for_session(&mut self, id: SessionId) {
        match &mut self.panes {
            None => {
                let (state, pane) = pane_grid::State::new(id);
                self.panes = Some(state);
                self.focused_pane = Some(pane);
            }
            Some(state) => {
                let target = self
                    .focused_pane
                    .filter(|p| state.get(*p).is_some())
                    .or_else(|| state.panes.keys().next().copied());
                if let Some(target) = target {
                    if let Some((new_pane, _)) =
                        state.split(pane_grid::Axis::Horizontal, target, id)
                    {
                        self.focused_pane = Some(new_pane);
                    }
                }
            }
        }
    }

    /// 세션을 스토어에서 닫고(Reaper로 은퇴) 그 세션에 딸린 상태를 **전부**
    /// 정리한다 — worktree ↔ 세션 매핑, 제목, 유실 경고, 그리고 그 세션을 가리키던
    /// pane까지.
    ///
    /// **pane 정리가 여기 있는 이유.** 원래는 호출자 몫이었고 대화형 경로
    /// (`PaneCloseRequested`)만 그걸 지켰다. 비대화형 경로(사라진 worktree 청소,
    /// `WorktreeRemoved` 성공)는 pane을 남겼고, 그 pane은 죽은 `SessionId`를
    /// 가리킨 채 영원히 빈 터미널로 남아 포커스까지 가져갔다. Plan 4가 그 pane을
    /// 죽은 `text()`에서 **포커스 가능한 위젯**으로 바꾸면서 증상이 커졌다.
    /// "호출자가 알아서 한다"는 계약은 네 곳 중 두 곳이 어겼으니 지켜지지 않는
    /// 계약이다 — 세션 소멸과 pane 소멸을 한 함수로 묶어 어길 수 없게 한다.
    fn close_session(&mut self, id: SessionId) {
        self.session_store.close(id);
        if let Some(worktree_id) = self.session_worktrees.remove(&id) {
            self.worktree_sessions.remove(&worktree_id);
        }
        self.session_titles.remove(&id);
        if self.last_input_loss == Some(id) {
            // 사라진 세션의 유실 경고를 남겨두면 지울 방법이 없다.
            self.last_input_loss = None;
        }
        self.close_panes_for_session(id);
    }

    /// `id`를 가리키던 pane을 pane_grid에서 지운다.
    ///
    /// **마지막 pane은 `close()`로 지울 수 없다** — pane_grid는 pane이 0개인
    /// 상태로 존재할 수 없어서 형제가 없는 pane에 대해 `close()`가 `None`을
    /// 돌려준다. 그래서 그 경우만 워크벤치 전체를 빈 상태(`panes = None`)로
    /// 되돌린다.
    fn close_panes_for_session(&mut self, id: SessionId) {
        let Some(panes) = &mut self.panes else {
            return;
        };
        let doomed: Vec<pane_grid::Pane> = panes
            .iter()
            .filter(|(_, session)| **session == id)
            .map(|(pane, _)| *pane)
            .collect();
        for pane in doomed {
            if panes.len() <= 1 {
                self.panes = None;
                self.focused_pane = None;
                return;
            }
            if let Some((_, sibling)) = panes.close(pane) {
                // 포커스가 방금 사라진 pane에 있었을 때만 옮긴다. 다른 pane에
                // 있었다면 그 포커스는 그대로 유효하다.
                if self.focused_pane == Some(pane) {
                    self.focused_pane = Some(sibling);
                }
            }
        }
    }

    // ---- Plan 4 Task 7: 터미널 위젯 → 세션 배선 ----

    /// 세션 없이 pane 레이아웃만 갖춘 상태. `workbench::view`의 pane_grid 설정
    /// (`spacing`/`on_resize` leeway/`TitleBar`)을 **실제 pane_grid에 이벤트를
    /// 흘려** 확인하려면 pane이 둘 이상 필요한데, 그 확인은 세션과 무관하다 —
    /// `session_store().snapshot(id)`는 모르는 id에 빈 스냅샷을 돌려주므로 PTY를
    /// 하나도 띄우지 않고 뷰를 만들 수 있다.
    #[cfg(test)]
    pub(crate) fn with_panes_for_test(panes: pane_grid::State<SessionId>) -> Self {
        let mut state = Self::default();
        state.set_panes_for_test(panes);
        state
    }

    /// 이미 세션이 들어 있는 상태에 pane 레이아웃만 얹는다. `with_panes_for_test`는
    /// **새 상태를 만들어 돌려주므로** 먼저 채워둔 세션 스토어를 버린다 — 세션이
    /// 필요한 테스트는 반드시 이쪽을 쓴다.
    #[cfg(test)]
    pub(crate) fn set_panes_for_test(&mut self, panes: pane_grid::State<SessionId>) {
        self.panes = Some(panes);
    }

    /// 지금 포커스된 pane의 세션.
    pub(crate) fn focused_session(&self) -> Option<SessionId> {
        let pane = self.focused_pane?;
        self.panes.as_ref()?.get(pane).copied()
    }

    /// 입력을 유실한 세션이 있으면 그것. 사이드바/타이틀바가 읽는다.
    pub(crate) fn last_input_loss(&self) -> Option<SessionId> {
        self.last_input_loss
    }

    /// 쓰기 결과를 상태로 옮긴다. **세 결과를 구별하는 것이 요점이다** —
    /// `bool`이었다면 "모드상 보낼 것 없음"과 "큐가 차서 유실"이 같은 값으로
    /// 뭉개져 유실이 조용히 지나간다.
    fn note_write(&mut self, id: SessionId, outcome: WriteOutcome) {
        match outcome {
            WriteOutcome::Queued => {}
            // 유실이 아니다. 피드백을 내지 않는다.
            WriteOutcome::Suppressed => {}
            WriteOutcome::Dropped => self.last_input_loss = Some(id),
        }
    }

    /// pane 포커스 전환. **`FOCUS_IN_OUT` 바이트의 권위는 여기다** — 위젯의
    /// `Focusable`은 `Shell`도 메시지 채널도 받지 못해 바이트를 낼 수 없다
    /// (`iced_core/src/widget/operation/focusable.rs:7-16`).
    fn focus_pane(&mut self, pane: pane_grid::Pane) -> iced::Task<Message> {
        let previous = self.focused_session();
        let next = self.panes.as_ref().and_then(|p| p.get(pane)).copied();
        self.focused_pane = Some(pane);

        for (id, focused) in focus_reports(previous, next) {
            if let Some(session) = self.session_store.session(id) {
                let outcome = session.report_focus(focused);
                self.note_write(id, outcome);
            }
        }

        match next {
            // `operation::focus`는 매칭되지 않는 focusable을 전부 unfocus시키므로
            // 상호배타가 공짜다(`focusable.rs:45-47`).
            Some(id) => iced::widget::operation::focus(crate::terminal::widget_id_for(id)),
            None => iced::Task::none(),
        }
    }

    /// 위젯이 발행한 커맨드를 세션에 적용한다. 실행 스레드는 Task 0.8의 정책
    /// 표를 따른다: `Key`/`Paste`/`Mouse`/`Scroll`은 UI 스레드에서 곧바로(그리드가
    /// 짧은 term 락으로 인코딩 후 `try_send`), `Resize`와 선택 추출은 워커로.
    fn dispatch_term_command(
        &mut self,
        id: SessionId,
        command: TermCommand,
    ) -> iced::Task<Message> {
        // 닫히는 중인 세션의 커맨드는 조용히 버린다 — 위젯이 그리는 프레임과
        // 세션이 사라지는 시점 사이에 항상 창이 있다.
        let Some(session) = self.session_store.session(id) else {
            return iced::Task::none();
        };

        match command {
            TermCommand::Key(input) => {
                let outcome = session.send_key(&input);
                self.note_write(id, outcome);
                iced::Task::none()
            }
            TermCommand::Paste(text) => {
                let outcome = session.send_paste(&text);
                self.note_write(id, outcome);
                iced::Task::none()
            }
            // 워커로 보내면 순서가 뒤집혀 스크롤이 튄다. 짧은 락이라 직접 한다.
            TermCommand::Scroll(scroll) => {
                session.scroll_display(scroll);
                iced::Task::none()
            }
            TermCommand::Resize { rows, cols, seq } => {
                self.session_store.request_resize(id, rows, cols, seq).1
            }
            TermCommand::Mouse(intent) => match session.send_mouse(&intent) {
                Err(error) => {
                    // **억제와 다르게 취급한다.** 조용히 버리면 상태기계 버그가
                    // 정상 억제로 위장된다(위젯의 held 전이 표가 깨졌다는 뜻이다).
                    eprintln!("terminal mouse intent rejected (session {}): {error}", id.0);
                    debug_assert!(
                        false,
                        "MouseEncodeError must not occur on well-formed input: {error}"
                    );
                    iced::Task::none()
                }
                Ok(result) => {
                    self.note_write(id, result.write);
                    // **다시 그리라고만 하면 옛 스냅샷을 옛 선택으로 다시 그린다.**
                    // 선택 변경을 화면에 반영하려면 새 스냅샷을 찍어야 한다 —
                    // `send_mouse`가 redraw일 때 generation을 이미 올려둔다.
                    let redraw = if result.redraw {
                        let generation = session.generation();
                        self.session_store.request_snapshot(id, generation).1
                    } else {
                        iced::Task::none()
                    };
                    let copy = match result.copy {
                        Some(request) => self.session_store.request_extraction(id, request).1,
                        None => iced::Task::none(),
                    };
                    iced::Task::batch([redraw, copy])
                }
            },
            TermCommand::CopySelection { to } => {
                // `request_copy`가 락 안에서 현재 epoch를 읽는다. 선택이 없거나
                // 드래그가 아직 진행 중이면 `None` — **조용한 취소**다.
                match session.request_copy(to) {
                    Some(request) => self.session_store.request_extraction(id, request).1,
                    None => iced::Task::none(),
                }
            }
        }
    }

    pub fn update(&mut self, message: Message) -> iced::Task<Message> {
        match message {
            Message::RepoPathInputChanged(value) => {
                self.repo_path_input = value;
                iced::Task::none()
            }
            Message::AddRepoSubmitted => {
                let path = self.repo_path_input.trim().to_string();
                if path.is_empty() {
                    return iced::Task::none();
                }
                self.repo_path_input.clear();
                let op = self.next_op();
                crate::git_tasks::add_repo(op, PathBuf::from(path))
            }
            Message::RepoProbed { result, .. } => match result {
                Ok((mut repo, head_branch)) => {
                    self.last_error = None;
                    if repo.worktree_base_ref.is_none() {
                        repo.worktree_base_ref = head_branch;
                    }
                    let repo_id = repo.id.clone();
                    self.upsert_repo(repo);
                    self.persist();
                    self.refresh_worktrees(repo_id)
                }
                Err(err) => {
                    self.last_error = Some(err);
                    iced::Task::none()
                }
            },
            Message::WorktreesListed {
                request,
                repo_id,
                result,
            } => match result {
                Ok(entries) => {
                    self.last_error = None;
                    self.apply_worktree_listing(repo_id, request, entries);
                    self.persist();
                    iced::Task::none()
                }
                Err(err) => {
                    self.last_error = Some(err);
                    iced::Task::none()
                }
            },
            Message::WorktreeNameInputChanged { repo_id, value } => {
                self.worktree_name_draft.insert(repo_id, value);
                iced::Task::none()
            }
            Message::CreateWorktreeSubmitted { repo_id } => {
                let Some(repo) = self.repo_by_id(&repo_id).cloned() else {
                    return iced::Task::none();
                };
                let name = self.worktree_name_draft(&repo_id).trim().to_string();
                if name.is_empty() {
                    return iced::Task::none();
                }
                // repo 등록 시 감지한 HEAD 브랜치를 기본 base ref로 쓴다. probe가
                // 실패했거나 HEAD를 못 읽었으면 "main"으로 최선을 다해 추정한다 —
                // 정확한 기본 브랜치 선택 UI는 이 태스크 범위 밖이다.
                let base_ref = repo
                    .worktree_base_ref
                    .clone()
                    .unwrap_or_else(|| "main".to_string());
                let op = self.next_op();
                crate::git_tasks::create_worktree(
                    op,
                    repo,
                    name,
                    base_ref,
                    self.workspace_root.clone(),
                )
            }
            Message::WorktreeCreated {
                repo_id, result, ..
            } => match result {
                Ok(_created) => {
                    self.last_error = None;
                    self.worktree_name_draft.remove(&repo_id);
                    self.refresh_worktrees(repo_id)
                }
                Err(err) => {
                    self.last_error = Some(err);
                    iced::Task::none()
                }
            },
            Message::RemoveWorktreeRequested {
                repo_id,
                worktree_id,
                worktree_path,
                branch,
            } => {
                let Some(repo) = self.repo_by_id(&repo_id).cloned() else {
                    return iced::Task::none();
                };
                if self.selected_worktree.as_ref() == Some(&worktree_id) {
                    self.selected_worktree = None;
                }
                // 세션을 여기서 곧바로 닫으면 안 된다 — git이 삭제를 실제로
                // 허용할지 아직 모른다. non-forced `git worktree remove`는
                // dirty한 worktree에서 흔히 실패하고(에이전트가 파일을 바꾸는
                // 게 이 앱의 존재 이유이니 그게 오히려 정상 상태다), 그때
                // worktree는 살아남는다. 여기서 세션을 먼저 닫으면 삭제가
                // 실패해도 방금 돌던(어쩌면 작업 중이던) 세션은 이미 reaper로
                // 갔고 pane은 빈 화면으로 남는다 — `close_session`은
                // `WorktreeRemoved`의 성공 경로로 미룬다. 아래
                // `pending_worktree_removals` 가드가 그 사이 새 세션이 끼어드는
                // 걸 막아주므로 순서를 미뤄도 안전하다.
                self.pending_worktree_removals.insert(worktree_id.clone());
                let op = self.next_op();
                crate::git_tasks::remove_worktree(
                    op,
                    repo,
                    worktree_id,
                    worktree_path,
                    false,
                    branch,
                )
            }
            Message::WorktreeRemoved {
                repo_id,
                worktree_id,
                result,
                ..
            } => {
                self.pending_worktree_removals.remove(&worktree_id);
                match result {
                    Ok(outcome) => {
                        // worktree 체크아웃 자체는 지워졌지만 브랜치 삭제가
                        // 거부됐을 수 있다(예: 아직 병합되지 않은 커밋이
                        // 있어 `git branch -d`가 안전하게 거절한 경우) — 이
                        // 경우도 "성공"으로 조용히 넘기면 사용자가 브랜치가
                        // 남아 있다는 걸 알 방법이 없다.
                        self.last_error = match outcome.branch_deletion {
                            BranchDeletion::Failed(msg) => Some(format!(
                                "worktree removed, but branch deletion failed: {msg}"
                            )),
                            BranchDeletion::Deleted | BranchDeletion::NotRequested => None,
                        };
                        // git이 worktree 삭제를 실제로 허용했다 — 이제야 세션을 닫는다
                        // (`RemoveWorktreeRequested`의 문서 참고). 그 세션을
                        // 가리키던 pane도 `close_session`이 같이 지운다 — 남겨두면
                        // 죽은 id를 가리키는 빈 터미널이 포커스를 먹는다.
                        if let Some(&session_id) = self.worktree_sessions.get(&worktree_id) {
                            self.close_session(session_id);
                        }
                        // 재조회 응답을 기다리지 않고 곧바로 지운다 — 그 사이
                        // 도착하는 `worktree_still_exists` 판단이 새 목록이
                        // 반영되기 전 낡은 목록으로 "아직 있다"고 답하지 않게 한다.
                        self.remove_worktree_entry(&repo_id, &worktree_id);
                        self.persist();
                        self.refresh_worktrees(repo_id)
                    }
                    Err(err) => {
                        self.last_error = Some(err);
                        iced::Task::none()
                    }
                }
            }
            Message::WorktreeSelected(id) => {
                self.selected_worktree = Some(id.clone());
                self.persist();
                if let Some(&session_id) = self.worktree_sessions.get(&id) {
                    // 이미 열려 있다 — 새 세션을 띄우지 않고 그 pane에 포커스만
                    // 옮긴다. pane_grid는 pane → 값 매핑만 들고 있으므로 여기서
                    // 직접 훑어야 한다(양방향 인덱스가 없다).
                    if let Some(panes) = &self.panes {
                        if let Some((pane, _)) = panes.iter().find(|(_, sid)| **sid == session_id) {
                            self.focused_pane = Some(*pane);
                        }
                    }
                    return iced::Task::none();
                }
                if self.pending_session_starts.contains_key(&id) {
                    // 시작 요청이 이미 나가 있다 — 빠른 재클릭으로 세션이
                    // 두 개 뜨는 걸 막는다.
                    return iced::Task::none();
                }
                let Some((repo_id, entry)) = self.find_worktree(&id) else {
                    return iced::Task::none();
                };
                let session_id = self.session_store.next_id();
                let title = entry
                    .branch
                    .clone()
                    .unwrap_or_else(|| "(detached)".to_string());
                self.session_titles.insert(session_id, title);
                self.pending_session_starts.insert(id.clone(), session_id);

                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                let worktree = Worktree {
                    id: id.clone(),
                    repo_id,
                    path: entry.path.clone(),
                    branch: entry.branch.clone().unwrap_or_default(),
                    display_name: entry.branch.unwrap_or_else(|| "worktree".to_string()),
                    created_with_agent: None,
                    created_at_unix_ms: now_ms,
                };
                // Custom + 커맨드 없음 = 로그인 셸. 에이전트 실행 커맨드 선택
                // UI는 이 태스크 범위 밖(§2 스펙 항목 3) — 여기서는 세션 →
                // 스냅샷 → 구독 → 화면 사슬을 증명하는 게 목적이다.
                self.session_store
                    .start(session_id, &worktree, AgentKind::Custom, None)
            }
            Message::Saved(report) => {
                self.last_save_status = Some(report.status);
                iced::Task::none()
            }

            // ---- Task 5의 비동기 결과를 실제로 반영한다 ----
            Message::SessionStarted {
                id,
                worktree_id,
                result,
            } => {
                self.pending_session_starts.remove(&worktree_id);
                match result {
                    Ok(started) => {
                        self.last_error = None;
                        let Some(session) = started.take() else {
                            // 이미 다른 곳에서 소비됐다 — 정상 경로에서는 밟지
                            // 않지만(봉투는 한 번만 만들어진다), 방어적으로
                            // 무시한다.
                            self.session_titles.remove(&id);
                            return iced::Task::none();
                        };
                        let still_exists = self.worktree_still_exists(&worktree_id);
                        match self.session_store.accept_started(
                            id,
                            worktree_id.clone(),
                            session,
                            still_exists,
                        ) {
                            Ok(()) => {
                                self.worktree_sessions.insert(worktree_id.clone(), id);
                                self.session_worktrees.insert(id, worktree_id);
                                self.open_pane_for_session(id);
                            }
                            Err(_) => {
                                // worktree가 그새 삭제됐다 — 세션은 이미 reaper로
                                // 갔다(`accept_started`). 타이틀만 정리한다.
                                self.session_titles.remove(&id);
                            }
                        }
                    }
                    Err(err) => {
                        self.session_titles.remove(&id);
                        self.last_error = Some(err);
                    }
                }
                iced::Task::none()
            }
            Message::SessionDirty { id, generation } => {
                let (_, task) = self.session_store.request_snapshot(id, generation);
                task
            }
            Message::SnapshotReady {
                id,
                generation,
                snapshot,
            } => self
                .session_store
                .apply_snapshot(id, generation, snapshot)
                .unwrap_or_else(iced::Task::none),
            Message::PaneClicked(pane) => self.focus_pane(pane),

            Message::Terminal { id, command } => self.dispatch_term_command(id, command),

            Message::ResizeApplied { id, seq, result } => {
                if let Err(e) = result {
                    // 리사이즈 실패는 입력 유실이 아니다 — 경고 UI를 띄우지
                    // 않는다. (`resize`는 rows/cols가 0이면 아무것도 안 하고 Ok다.)
                    eprintln!("terminal resize failed (session {}): {e}", id.0);
                }
                self.session_store.resize_completed(id, seq)
            }

            Message::SelectionExtracted { id, targets, text } => {
                let next = self.session_store.extraction_completed(id);
                // `None`은 조용한 취소다(epoch 불일치 또는 선택 없음). 오류가
                // 아니므로 아무것도 띄우지 않는다.
                let write = match text {
                    Some(text) => clipboard_writes(targets, text),
                    None => iced::Task::none(),
                };
                iced::Task::batch([next, write])
            }
            Message::PaneDragged(pane_grid::DragEvent::Dropped { pane, target }) => {
                if let Some(panes) = &mut self.panes {
                    panes.drop(pane, target);
                }
                iced::Task::none()
            }
            Message::PaneDragged(_) => iced::Task::none(),
            Message::PaneResized(pane_grid::ResizeEvent { split, ratio }) => {
                if let Some(panes) = &mut self.panes {
                    panes.resize(split, ratio);
                }
                iced::Task::none()
            }
            Message::PaneCloseRequested(pane) => {
                // pane 자체를 지우는 것도 `close_session`이 한다 — 대화형/비대화형
                // 경로가 갈라져서 pane이 새던 것이 이 수렴의 이유다.
                if let Some(&session_id) = self.panes.as_ref().and_then(|panes| panes.get(pane)) {
                    self.close_session(session_id);
                }
                iced::Task::none()
            }

            Message::PresenceReady {
                id,
                generation,
                presence,
            } => {
                self.session_store.apply_presence(id, generation, presence);
                iced::Task::none()
            }
            Message::PresenceTick => {
                let (_dispatched, task) = crate::presence_poll::dispatch_tick(self);
                task
            }

            // ---- Plan 5: 변형만 먼저 만들어 두고 처리는 소유 태스크가 채운다.
            // **`_ =>` 와일드카드를 쓰지 않는다** — 그러면 다음에 변형을 더할 때
            // 컴파일러가 배선 누락을 잡아주지 못한다. 여기 이름을 늘어놓는 비용이
            // 그 안전망의 값이다. ----
            Message::HookArrived(_) => iced::Task::none(), // Task 3
            Message::BadgeTick => iced::Task::none(),      // Task 3
            Message::DiffRequested { .. }
            | Message::FileDiffRequested { .. }
            | Message::DiffCancelled { .. } => iced::Task::none(), // Task 4
            Message::HydrationStep(_) => iced::Task::none(), // Task 5
            Message::LayoutPersistDue { .. } => iced::Task::none(), // Task 5
        }
    }
}

#[cfg(test)]
impl AppState {
    pub fn fresh() -> Self {
        Self {
            load_origin: LoadOrigin::Fresh,
            ..Self::default()
        }
    }

    pub fn recovered(slot: usize) -> Self {
        Self {
            load_origin: LoadOrigin::Recovered { slot },
            ..Self::default()
        }
    }

    pub fn recovery_failed() -> Self {
        Self {
            load_origin: LoadOrigin::RecoveryFailed,
            ..Self::default()
        }
    }

    pub fn with_save_error(message: &str) -> Self {
        Self {
            last_save_status: Some(SaveStatus::Failed(message.to_string())),
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn entry(name: &str) -> WorktreeEntry {
        WorktreeEntry {
            path: PathBuf::from(format!("/tmp/{name}")),
            branch: Some(name.to_string()),
            head: None,
            is_main: false,
        }
    }

    #[test]
    fn an_out_of_order_worktree_listing_is_discarded() {
        let mut state = AppState::default();
        let repo = RepoId("/tmp/r".into());
        state.note_list_issued(repo.clone(), OpId(2));
        state.apply_worktree_listing(repo.clone(), OpId(2), vec![entry("new")]);
        // 앞서 발급된 목록이 뒤늦게 도착
        state.apply_worktree_listing(repo.clone(), OpId(1), vec![entry("old")]);
        assert_eq!(
            state.worktree_names(&repo),
            vec!["new"],
            "a stale listing must not win"
        );
    }

    fn entry_at(path: &str, branch: &str) -> WorktreeEntry {
        WorktreeEntry {
            path: PathBuf::from(path),
            branch: Some(branch.to_string()),
            head: None,
            is_main: false,
        }
    }

    #[test]
    fn selecting_an_unopened_worktree_records_a_pending_session_start() {
        let mut state = AppState::default();
        let repo_id = RepoId("/tmp/r".into());
        // 실제 존재하지 않는 경로를 써서, 이 테스트가 트리거하는 진짜 백그라운드
        // 스폰(로그인 셸)이 즉시 실패하게 한다 — `SessionStarted`가 여기 도착할
        // 때까지 기다리지 않으므로(플레인 `#[test]`엔 iced executor가 없다),
        // 성공 경로를 밟으면 아무도 받지 않는 채널로 진짜 `TerminalSession`이
        // 흘러들어가 이 테스트 스레드에서 drop되며 최대 2초를 먹을 위험이 있다.
        let e = entry_at("/nonexistent-suaegi-test-dir-xyz", "feature");
        let worktree_id = worktree_id_for(&e.path);
        state.note_list_issued(repo_id.clone(), OpId(1));
        state.apply_worktree_listing(repo_id, OpId(1), vec![e]);

        let _ = state.update(Message::WorktreeSelected(worktree_id.clone()));

        assert_eq!(state.selected_worktree(), Some(&worktree_id));
        assert!(
            state.pending_session_starts.contains_key(&worktree_id),
            "a start must be pending until SessionStarted arrives"
        );
        assert!(
            !state
                .session_title(*state.pending_session_starts.get(&worktree_id).unwrap())
                .is_empty(),
            "the pane title is captured up front, not after the session actually starts"
        );
        assert!(
            state.panes().is_none(),
            "no pane exists until a session actually starts"
        );

        // 같은 worktree를 다시 선택해도(빠른 재클릭) 두 번째 시작 요청을 내면
        // 안 된다 — pending 상태 그대로다.
        let _ = state.update(Message::WorktreeSelected(worktree_id.clone()));
        assert_eq!(state.pending_session_starts.len(), 1);
    }

    /// `SessionStarted`(성공)부터 시작해 세션이 하나 열려 있는 상태를 만든다.
    /// 진짜 `TerminalSession`(reaper가 정상 경로로 정리하는)을 쓴다 —
    /// `state.session_store`가 소유하게 되므로 `close()`를 거치지 않는 한
    /// 이 테스트 스레드를 블로킹할 일이 없다(`SessionStore`의 위험 지점 문서
    /// 참고).
    fn state_with_one_open_session() -> (AppState, SessionId, WorktreeId, pane_grid::Pane) {
        let mut state = AppState::default();
        let worktree_id = WorktreeId("/tmp/accepted".into());
        let repo_id = RepoId("/tmp/r2".into());
        state.note_list_issued(repo_id.clone(), OpId(1));
        state.apply_worktree_listing(
            repo_id,
            OpId(1),
            vec![entry_at("/tmp/accepted", "accepted")],
        );

        let id = state.session_store.next_id();
        state.pending_session_starts.insert(worktree_id.clone(), id);
        state.session_titles.insert(id, "accepted".to_string());

        let session = SessionStore::spawn_throwaway_for_test();
        let _ = state.update(Message::SessionStarted {
            id,
            worktree_id: worktree_id.clone(),
            result: Ok(StartedSession::new(session)),
        });

        let pane = *state
            .panes()
            .expect("the first session must open a pane")
            .panes
            .keys()
            .next()
            .expect("pane_grid::State always has at least one pane");
        (state, id, worktree_id, pane)
    }

    #[test]
    fn accepting_a_started_session_registers_it_and_opens_a_pane() {
        let (state, id, worktree_id, _pane) = state_with_one_open_session();

        assert!(
            !state.pending_session_starts.contains_key(&worktree_id),
            "the pending marker must clear once SessionStarted lands"
        );
        assert_eq!(state.worktree_sessions.get(&worktree_id), Some(&id));
        assert_eq!(state.session_worktrees.get(&id), Some(&worktree_id));
        assert!(state.panes().is_some());
        assert!(state.session_store().is_running(id));
    }

    #[test]
    fn closing_the_only_pane_closes_its_session_and_clears_the_workbench() {
        let (mut state, id, worktree_id, pane) = state_with_one_open_session();

        let _ = state.update(Message::PaneCloseRequested(pane));

        assert!(
            state.panes().is_none(),
            "pane_grid cannot close its last pane — the workbench itself must reset instead"
        );
        assert!(
            !state.session_store().is_running(id),
            "the underlying session must actually be closed, not merely detached from the pane"
        );
        assert!(!state.worktree_sessions.contains_key(&worktree_id));
    }

    /// 세션 둘, pane 둘. 비대화형 종료가 **형제 pane과 포커스를 어떻게 남기는지**
    /// 보려면 마지막 pane이 아닌 pane을 닫아봐야 한다 — 마지막 pane 경로는
    /// `panes = None`으로 빠져나가 아무것도 증명하지 못한다.
    fn state_with_two_open_sessions() -> (AppState, RepoId, [(SessionId, WorktreeId); 2]) {
        let repo_id = RepoId("/tmp/two".into());
        let mut state = AppState::default();
        state.note_list_issued(repo_id.clone(), OpId(1));
        state.apply_worktree_listing(
            repo_id.clone(),
            OpId(1),
            vec![entry_at("/tmp/wt-a", "a"), entry_at("/tmp/wt-b", "b")],
        );

        let mut opened = Vec::new();
        for path in ["/tmp/wt-a", "/tmp/wt-b"] {
            let worktree_id = WorktreeId(path.to_string());
            let id = state.session_store.next_id();
            state.pending_session_starts.insert(worktree_id.clone(), id);
            let _ = state.update(Message::SessionStarted {
                id,
                worktree_id: worktree_id.clone(),
                result: Ok(StartedSession::new(SessionStore::spawn_throwaway_for_test())),
            });
            opened.push((id, worktree_id));
        }
        assert_eq!(
            state.panes().expect("two sessions must open panes").len(),
            2,
            "precondition: each session got its own pane"
        );
        let opened: [(SessionId, WorktreeId); 2] = opened.try_into().expect("exactly two");
        (state, repo_id, opened)
    }

    /// 앱 밖에서(다른 터미널에서) worktree가 지워지면 목록 갱신이 그 세션을
    /// 거둔다. **그때 pane도 같이 가야 한다** — 남으면 죽은 `SessionId`를 가리키는
    /// 빈 터미널이 되고, Plan 4 이후로는 그게 포커스까지 가져간다.
    #[test]
    fn a_vanished_worktree_takes_its_pane_with_it() {
        let (mut state, repo_id, [(id_a, _wt_a), (id_b, _wt_b)]) = state_with_two_open_sessions();

        // /tmp/wt-a가 새 목록에서 사라졌다.
        state.note_list_issued(repo_id.clone(), OpId(2));
        state.apply_worktree_listing(repo_id, OpId(2), vec![entry_at("/tmp/wt-b", "b")]);

        let panes = state.panes().expect("the surviving session keeps its pane");
        assert_eq!(panes.len(), 1, "the vanished session's pane must be gone");
        let survivors: Vec<SessionId> = panes.iter().map(|(_, id)| *id).collect();
        assert_eq!(
            survivors,
            vec![id_b],
            "and the pane that remains must be the one that still has a live session"
        );
        assert!(!state.session_store().is_running(id_a));
        assert_eq!(
            state.focused_session(),
            Some(id_b),
            "focus must land on a session that exists, not on a dead id"
        );
    }

    /// `WorktreeRemoved` 성공 경로도 같은 계약을 진다. 이쪽은 앱이 직접 지운
    /// 경우라 `apply_worktree_listing`을 기다리지 않고 즉시 정리한다.
    #[test]
    fn a_removed_worktree_takes_its_pane_with_it() {
        let (mut state, repo_id, [(id_a, wt_a), (id_b, _wt_b)]) = state_with_two_open_sessions();

        let _ = state.update(Message::WorktreeRemoved {
            request: OpId(9),
            repo_id,
            worktree_id: wt_a,
            result: Ok(RemoveOutcome {
                branch_deletion: BranchDeletion::NotRequested,
            }),
        });

        let panes = state.panes().expect("the surviving session keeps its pane");
        assert_eq!(panes.len(), 1, "the removed session's pane must be gone");
        assert_eq!(
            panes.iter().map(|(_, id)| *id).collect::<Vec<_>>(),
            vec![id_b]
        );
        assert!(!state.session_store().is_running(id_a));
        assert_eq!(state.focused_session(), Some(id_b));
    }

    /// 마지막 pane을 비대화형으로 닫는 경로. pane_grid는 pane 0개로 존재할 수
    /// 없으므로 `close()`가 아니라 워크벤치 전체 리셋으로 빠져야 한다 —
    /// `PaneCloseRequested`만 알던 규칙이 이제 모든 경로에 적용된다.
    #[test]
    fn removing_the_last_worktree_resets_the_workbench() {
        let (mut state, id, worktree_id, _pane) = state_with_one_open_session();
        assert!(state.panes().is_some(), "precondition");

        let _ = state.update(Message::WorktreeRemoved {
            request: OpId(9),
            repo_id: RepoId("/tmp/r2".into()),
            worktree_id,
            result: Ok(RemoveOutcome {
                branch_deletion: BranchDeletion::NotRequested,
            }),
        });

        assert!(
            state.panes().is_none(),
            "the last pane cannot be closed — the workbench must reset"
        );
        assert_eq!(
            state.focused_session(),
            None,
            "focus must not survive the pane it pointed at"
        );
        assert!(!state.session_store().is_running(id));
    }

    // ---- Plan 4 Task 7: 터미널 위젯 배선 ----

    /// 포커스 리포트의 **순서**가 계약이다. 바이트 자체는 헤드리스로 볼 수 없다
    /// (`report_focus`는 셸이 `FOCUS_IN_OUT`을 켰을 때만 바이트를 내고 평범한
    /// 셸은 켜지 않는다) — 순서 결정을 값으로 뽑아 그것을 검사한다.
    #[test]
    fn focus_out_precedes_focus_in() {
        let a = SessionId(1);
        let b = SessionId(2);

        assert_eq!(
            focus_reports(Some(a), Some(b)),
            vec![(a, false), (b, true)],
            "the OLD session must be told it lost focus BEFORE the new one is \
             told it gained focus"
        );
    }

    #[test]
    fn focus_reports_cover_the_edges() {
        let a = SessionId(1);
        let b = SessionId(2);

        assert_eq!(
            focus_reports(None, Some(a)),
            vec![(a, true)],
            "the first focus has no predecessor to notify"
        );
        assert_eq!(
            focus_reports(Some(a), None),
            vec![(a, false)],
            "losing focus with no successor still notifies the old session"
        );
        assert_eq!(
            focus_reports(Some(a), Some(a)),
            vec![],
            "re-clicking the focused pane must not re-send focus-in"
        );
        assert_eq!(focus_reports(None, None), vec![]);
        // 대조군: 위의 빈 결과들이 "이 함수가 늘 비어 있다"가 아님을 고정한다.
        assert_eq!(focus_reports(Some(a), Some(b)), vec![(a, false), (b, true)]);
    }

    /// `WriteOutcome`이 셋인 이유 전체가 여기 걸려 있다. `bool`이었다면 "모드상
    /// 보낼 것 없음"과 "큐가 차서 유실"이 같은 값으로 뭉개져 유실이 조용히
    /// 지나간다.
    #[test]
    fn only_a_dropped_write_surfaces_as_input_loss() {
        let id = SessionId(3);

        let mut state = AppState::default();
        state.note_write(id, WriteOutcome::Queued);
        assert_eq!(
            state.last_input_loss(),
            None,
            "a queued write is not a loss"
        );

        let mut state = AppState::default();
        state.note_write(id, WriteOutcome::Suppressed);
        assert_eq!(
            state.last_input_loss(),
            None,
            "Suppressed means the mode had nothing to send — not a loss, and \
             surfacing it would report normal operation as an error"
        );

        // 대조군: 실제 유실은 반드시 보여야 한다. 이게 없으면 위의 두 단언이
        // "이 함수가 아무것도 안 한다"로도 설명된다.
        let mut state = AppState::default();
        state.note_write(id, WriteOutcome::Dropped);
        assert_eq!(
            state.last_input_loss(),
            Some(id),
            "control: a dropped write IS lost user input and must surface"
        );
    }

    #[test]
    fn closing_a_session_clears_its_input_loss_warning() {
        let (mut state, id, _worktree_id, pane) = state_with_one_open_session();
        state.note_write(id, WriteOutcome::Dropped);
        assert_eq!(state.last_input_loss(), Some(id), "precondition");

        let _ = state.update(Message::PaneCloseRequested(pane));

        assert_eq!(
            state.last_input_loss(),
            None,
            "a warning about a session that no longer exists can never be dismissed"
        );
    }

    /// 위젯이 그리는 프레임과 세션이 사라지는 시점 사이에는 항상 창이 있다.
    /// 그 창에 도착한 커맨드로 패닉하면 안 된다.
    #[test]
    fn a_command_for_an_unknown_session_is_dropped_silently() {
        let mut state = AppState::default();
        let _ = state.update(Message::Terminal {
            id: SessionId(999),
            command: TermCommand::Resize {
                rows: 25,
                cols: 100,
                seq: 1,
            },
        });
        assert_eq!(state.last_input_loss(), None);
    }

    /// 앱 배선까지 포함한 seq 가드. 코얼레서 단위 테스트가 규칙을 고정하고,
    /// 이 테스트는 **`Message::Terminal`에서 거기까지 실제로 이어져 있는지**를
    /// 본다 — 둘 중 하나만으로는 배선이 끊겨도 통과한다.
    #[test]
    fn the_resize_seq_guard_is_wired_through_the_message_path() {
        use crate::session_store::ResizeDecision;

        let (mut state, id, _worktree_id, _pane) = state_with_one_open_session();

        // **첫 리사이즈는 반드시 메시지로 넣는다.** 여기서 `request_resize`를
        // 직접 부르면 `Message::Terminal`의 `TermCommand::Resize` 팔이 통째로
        // 죽어도 테스트가 통과한다 — 실제로 그랬다.
        let _ = state.update(Message::Terminal {
            id,
            command: TermCommand::Resize {
                rows: 30,
                cols: 120,
                seq: 10,
            },
        });

        // 워커가 seq 10을 끝냈다고 알린다. **이 완료가 받아들여진다는 것 자체가
        // 배선의 증거다** — 코얼레서는 `in_flight == Some(10)`일 때만 가드를
        // 푼다(`ResizeCoalescer::completed`). 팔이 죽어 있었다면 seq 10은
        // in-flight가 된 적이 없어 이 완료는 아무 일도 하지 않는다.
        let _ = state.update(Message::ResizeApplied {
            id,
            seq: 10,
            result: Ok(()),
        });

        // 뒤늦게 도착한 낡은 seq는 버려진다. 팔이 죽어 있었다면 코얼레서의
        // `last_seq`가 아직 0이라 seq 4가 여기서 `Dispatch`된다.
        assert_eq!(
            state.session_store.request_resize(id, 10, 40, 4).0,
            ResizeDecision::Discard,
            "a resize older than the one that already went through the message path \
             must be discarded — if this dispatches, Message::Terminal never reached \
             the coalescer"
        );

        // 대조군 둘을 겸한다. (1) 가드가 모든 것을 버리는 게 아니라 더 새로운
        // seq는 통과시킨다. (2) `Coalesce`가 아니라 `Dispatch`라는 것은 위의
        // `ResizeApplied`가 in-flight 가드를 실제로 풀었다는 뜻이고, 그건 seq
        // 10이 메시지 경로를 타고 in-flight가 됐을 때만 성립한다.
        let fresh = state.session_store.request_resize(id, 31, 124, 11).0;
        assert!(
            matches!(fresh, ResizeDecision::Dispatch { seq: 11, .. }),
            "control: a newer resize must still dispatch (and Dispatch rather than \
             Coalesce proves seq 10 held the in-flight guard); got {fresh:?}"
        );
    }

    /// 드래그 완료가 standard까지 쓰면 사용자가 복사한 적 없는 텍스트가 시스템
    /// 클립보드를 덮어쓴다. 명시적 복사(단축키)만 양쪽에 쓴다.
    #[test]
    fn each_copy_target_writes_exactly_where_it_was_asked_to() {
        use clipboard::Kind;

        assert_eq!(
            clipboard_kinds(CopyTargets::EXPLICIT),
            vec![Kind::Standard, Kind::Primary],
            "an explicit copy goes to both"
        );
        assert_eq!(
            clipboard_kinds(CopyTargets::DRAG_COMPLETE),
            vec![Kind::Primary],
            "a finished drag goes to primary ONLY — X11/Wayland middle-click \
             convention, and it must not clobber the system clipboard"
        );
        assert_eq!(
            clipboard_kinds(CopyTargets {
                standard: true,
                primary: false
            }),
            vec![Kind::Standard]
        );
        assert_eq!(
            clipboard_kinds(CopyTargets {
                standard: false,
                primary: false
            }),
            vec![],
            "asking for nothing writes nothing"
        );
    }

    /// 추출은 세션당 **직렬**이다(`selection_to_string()`이 선택 범위 전체를
    /// 훑는다). 도는 동안 온 요청은 최신 하나로 대기했다가 완료 후에 나간다.
    #[test]
    fn selection_extraction_is_serialized_per_session() {
        use suaegi_term::input_types::CopyRequest;

        let (mut state, id, _worktree_id, _pane) = state_with_one_open_session();
        let store = &mut state.session_store;

        let first = CopyRequest {
            epoch: 1,
            to: CopyTargets::EXPLICIT,
        };
        assert!(
            store.request_extraction(id, first).0,
            "the first extraction must dispatch"
        );

        let second = CopyRequest {
            epoch: 2,
            to: CopyTargets::DRAG_COMPLETE,
        };
        assert!(
            !store.request_extraction(id, second).0,
            "a second extraction must NOT run concurrently with the first"
        );

        assert_eq!(
            store.extraction_state(id),
            Some((true, Some(second))),
            "precondition: one running, one queued"
        );

        // 완료하면 대기하던 것이 나간다 — 버려지지 않는다. 사용자가 누른 복사가
        // "마침 다른 추출이 돌고 있었다"는 이유로 사라지면 안 된다.
        let _ = state.update(Message::SelectionExtracted {
            id,
            targets: CopyTargets::EXPLICIT,
            text: None,
        });

        // **대기열이 비었는지를 직접 본다.** `request_extraction`의 `bool`을
        // 프록시로 쓰면 안 된다 — "대기하던 것이 나갔다"와 "완료 처리가 아예
        // 안 돼서 영원히 막혔다"가 둘 다 `false`라 구별되지 않는다(mutation으로
        // 확인: `extraction_completed`를 `Task::none()`으로 바꿔도 통과했다).
        assert_eq!(
            state.session_store.extraction_state(id),
            Some((true, None)),
            "the queued request must have been dispatched (pending drained) and now be \
             in flight — a still-Some pending means the completion was dropped and this \
             session can never extract again"
        );

        // 대기하던 요청이 지금 in-flight이므로 새 요청은 다시 대기한다.
        let third = CopyRequest {
            epoch: 3,
            to: CopyTargets::EXPLICIT,
        };
        assert!(
            !state.session_store.request_extraction(id, third).0,
            "the queued extraction is now running, so the next one queues behind it"
        );
    }

    /// `text: None`은 **조용한 취소**다(epoch 불일치 또는 선택 없음). 오류 배너를
    /// 띄우거나 입력 유실로 보고하면 정상 동작이 고장으로 보인다.
    #[test]
    fn an_empty_extraction_result_is_a_silent_cancellation() {
        let (mut state, id, _worktree_id, _pane) = state_with_one_open_session();

        let _ = state.update(Message::SelectionExtracted {
            id,
            targets: CopyTargets::EXPLICIT,
            text: None,
        });

        assert_eq!(state.last_error(), None, "a cancelled copy is not an error");
        assert_eq!(
            state.last_input_loss(),
            None,
            "and it is not lost input either"
        );
    }

    fn snapshot_with_text(line: &str) -> TerminalSnapshot {
        use alacritty_terminal::term::cell::Flags;
        use alacritty_terminal::vte::ansi::{Color, NamedColor};
        use suaegi_term::grid::{GridSize, SnapshotCell};

        let cells: Vec<SnapshotCell> = line
            .chars()
            .map(|c| SnapshotCell {
                c,
                combining: Vec::new(),
                fg: Color::Named(NamedColor::Foreground),
                bg: Color::Named(NamedColor::Background),
                flags: Flags::empty(),
            })
            .collect();
        TerminalSnapshot {
            size: GridSize {
                rows: 1,
                cols: cells.len(),
            },
            rows: vec![cells],
            cursor: None,
            display_offset: 0,
            history_size: 0,
            mode: alacritty_terminal::term::TermMode::empty(),
            selection: None,
        }
    }

    #[test]
    fn session_dirty_requests_a_snapshot_and_a_stale_reply_cannot_clobber_a_fresher_one() {
        let (mut state, id, _worktree_id, _pane) = state_with_one_open_session();
        assert_eq!(state.session_store().snapshot_text(id), "");

        // 실제 request_snapshot 파이프라인을 태워 in-flight 가드를 세운다.
        let _ = state.update(Message::SessionDirty { id, generation: 5 });
        let _ = state.update(Message::SnapshotReady {
            id,
            generation: 5,
            snapshot: snapshot_with_text("hello"),
        });
        assert_eq!(state.session_store().snapshot_text(id), "hello");

        // 더 오래된 generation의 결과가 뒤늦게 도착해도 캐시를 덮으면 안 된다.
        let _ = state.update(Message::SnapshotReady {
            id,
            generation: 1,
            snapshot: snapshot_with_text("stale"),
        });
        assert_eq!(
            state.session_store().snapshot_text(id),
            "hello",
            "a stale snapshot result must not overwrite a newer one"
        );
    }

    #[test]
    fn presence_ready_updates_the_session_and_is_visible_through_worktree_presence() {
        let (mut state, id, worktree_id, _pane) = state_with_one_open_session();
        assert!(matches!(
            state.worktree_presence(&worktree_id),
            AgentPresence::Unknown
        ));

        let _ = state.update(Message::PresenceReady {
            id,
            generation: 1,
            presence: AgentPresence::Agent(suaegi_term::agent::AgentKind::Claude),
        });

        assert!(matches!(
            state.worktree_presence(&worktree_id),
            AgentPresence::Agent(suaegi_term::agent::AgentKind::Claude)
        ));
    }

    #[test]
    fn a_worktree_with_no_session_reports_unknown_presence() {
        let state = AppState::default();
        assert!(matches!(
            state.worktree_presence(&WorktreeId("/tmp/no-session".into())),
            AgentPresence::Unknown
        ));
    }

    #[test]
    fn a_successful_git_op_clears_a_stale_error_banner() {
        // last_error가 실패에서만 세워지고 성공에서 지워지지 않으면, 사용자가
        // 재시도에 성공한 뒤에도 사이드바에 옛 에러 배너가 계속 떠 있다.
        let mut state = AppState::default();
        let _ = state.update(Message::RepoProbed {
            request: OpId(1),
            requested_path: PathBuf::from("/tmp/bad"),
            result: Err("not a git repo".to_string()),
        });
        assert_eq!(state.last_error(), Some("not a git repo"));

        let repo = Repo {
            id: RepoId("/tmp/good".into()),
            path: PathBuf::from("/tmp/good"),
            display_name: "good".into(),
            worktree_base_ref: None,
        };
        let _ = state.update(Message::RepoProbed {
            request: OpId(2),
            requested_path: PathBuf::from("/tmp/good"),
            result: Ok((repo, Some("main".to_string()))),
        });

        assert_eq!(
            state.last_error(),
            None,
            "a success after a failure must clear the stale error banner"
        );
    }

    // ---- Task 8, Step 1: worktree 생성/삭제 실패가 UI 상태에 남는지. 손으로
    // `last_error`를 세우지 않고, 실제 `update()` 디스패치를 통해 검증한다 ----

    #[test]
    fn a_failed_worktree_creation_is_visible_as_an_error() {
        let mut state = AppState::default();
        let _ = state.update(Message::WorktreeCreated {
            request: OpId(1),
            repo_id: RepoId("/tmp/r".into()),
            result: Err("branch already exists".to_string()),
        });
        assert_eq!(state.last_error(), Some("branch already exists"));
    }

    #[test]
    fn a_failed_worktree_removal_is_visible_as_an_error() {
        let mut state = AppState::default();
        let _ = state.update(Message::WorktreeRemoved {
            request: OpId(1),
            repo_id: RepoId("/tmp/r".into()),
            worktree_id: WorktreeId("/tmp/r/wt".into()),
            result: Err("worktree has uncommitted changes".to_string()),
        });
        assert_eq!(state.last_error(), Some("worktree has uncommitted changes"));
    }

    // ---- pr4 적대적 리뷰 항목 1: worktree 자체는 지워졌지만(Ok) 브랜치가
    // 아직 병합되지 않아 `git branch -d`가 안전하게 거절했을 수 있다
    // (`BranchDeletion::Failed`). 이걸 `Ok(_)`로 뭉개면 사용자는 브랜치가
    // 남아 있다는 걸 알 방법이 없다 ----

    #[test]
    fn a_refused_branch_deletion_is_visible_as_an_error_even_though_the_worktree_removal_succeeded()
    {
        let mut state = AppState::default();
        let repo_id = RepoId("/tmp/r".into());
        let worktree_id = WorktreeId("/tmp/r/wt".into());
        state.note_list_issued(repo_id.clone(), OpId(1));
        state.apply_worktree_listing(repo_id.clone(), OpId(1), vec![entry_at("/tmp/r/wt", "wt")]);

        let _ = state.update(Message::WorktreeRemoved {
            request: OpId(2),
            repo_id: repo_id.clone(),
            worktree_id: worktree_id.clone(),
            result: Ok(RemoveOutcome {
                branch_deletion: BranchDeletion::Failed("not fully merged".to_string()),
            }),
        });

        assert!(
            state
                .last_error()
                .is_some_and(|e| e.contains("not fully merged")),
            "a refused branch delete must surface, got {:?}",
            state.last_error()
        );
        assert!(
            !state
                .worktrees_for(&repo_id)
                .iter()
                .any(|w| worktree_id_for(&w.path) == worktree_id),
            "the worktree checkout itself was still removed and must drop from the list"
        );
    }

    #[test]
    fn a_successful_branch_deletion_clears_a_stale_error() {
        let mut state = AppState {
            last_error: Some("stale error from a previous op".to_string()),
            ..AppState::default()
        };
        let repo_id = RepoId("/tmp/r".into());
        let worktree_id = WorktreeId("/tmp/r/wt".into());
        state.note_list_issued(repo_id.clone(), OpId(1));
        state.apply_worktree_listing(repo_id.clone(), OpId(1), vec![entry_at("/tmp/r/wt", "wt")]);

        let _ = state.update(Message::WorktreeRemoved {
            request: OpId(2),
            repo_id,
            worktree_id,
            result: Ok(RemoveOutcome {
                branch_deletion: BranchDeletion::Deleted,
            }),
        });

        assert_eq!(state.last_error(), None);
    }

    // ---- 최종 리뷰 항목 2: 제거 요청이 git 결과를 기다리지 않고 세션을
    // 먼저 닫으면, non-forced 삭제가 흔하게 실패하는(dirty worktree) 상황에서
    // worktree는 살아남았는데 그 위에서 돌던 세션(어쩌면 작업 중이던
    // 에이전트)은 이미 reaper로 가버린다 — pane은 빈 화면으로 남는다.
    // `close_session`은 `WorktreeRemoved`의 성공 경로로 미뤄야 한다 ----

    #[test]
    fn a_failed_worktree_removal_leaves_the_session_alive_and_its_pane_rendering() {
        let (mut state, id, worktree_id, pane) = state_with_one_open_session();
        // `state_with_one_open_session`은 목록만 채우고 repo는 등록하지
        // 않는다 — `RemoveWorktreeRequested`는 `repo_by_id`를 요구하므로
        // 여기서 등록해야 핸들러가 실제로 진행된다.
        let repo_id = RepoId("/tmp/r2".into());
        state.upsert_repo(Repo {
            id: repo_id.clone(),
            path: PathBuf::from("/tmp/r2"),
            display_name: "r2".to_string(),
            worktree_base_ref: None,
        });

        let _ = state.update(Message::RemoveWorktreeRequested {
            repo_id: repo_id.clone(),
            worktree_id: worktree_id.clone(),
            worktree_path: PathBuf::from("/tmp/accepted"),
            branch: Some("accepted".to_string()),
        });

        // 제거 요청을 보낸 직후(git 응답은 아직 안 옴) — 세션은 여전히 살아
        // 있어야 하고 pane도 여전히 그걸 가리켜야 한다.
        assert!(
            state.session_store().is_running(id),
            "the session must still be running while the removal request is in flight"
        );
        assert_eq!(
            state.panes().and_then(|panes| panes.get(pane)),
            Some(&id),
            "the pane must still show the live session while removal is pending"
        );

        let _ = state.update(Message::WorktreeRemoved {
            request: OpId(99),
            repo_id,
            worktree_id: worktree_id.clone(),
            result: Err("worktree has uncommitted changes".to_string()),
        });

        assert!(
            state.session_store().is_running(id),
            "a failed removal must not have closed the still-live session"
        );
        assert_eq!(
            state.panes().and_then(|panes| panes.get(pane)),
            Some(&id),
            "the pane must still render the session's content after a failed removal"
        );
        assert_eq!(
            state.worktree_sessions.get(&worktree_id),
            Some(&id),
            "the worktree -> session mapping must survive a failed removal"
        );
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

    // ---- Task 6 리뷰에서 넘어온 수정: 세션 시작이 진행 중인 worktree를
    // 제거하면(제거가 끝나기 전에 SessionStarted가 도착하면) 그 세션은
    // reaper로 가야 한다 — 산 슬롯으로 받아들여지면 아무도 닫지 않는 PTY와
    // 스레드가 샌다 ----

    #[test]
    fn a_session_started_while_its_worktree_removal_is_in_flight_is_retired_not_leaked() {
        let mut state = AppState::default();
        let repo_id = RepoId("/tmp/race-repo".into());
        state.upsert_repo(Repo {
            id: repo_id.clone(),
            path: PathBuf::from("/tmp/race-repo"),
            display_name: "race-repo".to_string(),
            worktree_base_ref: None,
        });
        let e = entry_at("/tmp/race-repo/wt", "feature");
        let worktree_id = worktree_id_for(&e.path);
        state.note_list_issued(repo_id.clone(), OpId(1));
        state.apply_worktree_listing(repo_id.clone(), OpId(1), vec![e.clone()]);

        // WorktreeSelected로 세션 시작을 건다 — 아직 SessionStarted는 안 왔다.
        let _ = state.update(Message::WorktreeSelected(worktree_id.clone()));
        let session_id = *state
            .pending_session_starts
            .get(&worktree_id)
            .expect("a start must be pending");

        // 같은 worktree를 곧바로 지운다. 세션은 아직 `pending_session_starts`에만
        // 있고 `worktree_sessions`엔 없으므로, 이 핸들러가 세션을 직접 닫는
        // 기존 경로(`worktree_sessions.get`)는 아무것도 못 잡는다.
        let _ = state.update(Message::RemoveWorktreeRequested {
            repo_id: repo_id.clone(),
            worktree_id: worktree_id.clone(),
            worktree_path: e.path.clone(),
            branch: e.branch.clone(),
        });

        // git 삭제는 실제로 돌지 않았다(테스트 스레드엔 iced executor가 없다) —
        // `worktrees_by_repo`는 아직 그대로다. 이 상태에서도 새는지가 이 버그의
        // 핵심이었다: 목록만 보고 판단하면 여기서 "아직 있다"고 잘못 답한다.
        assert!(
            state
                .worktrees_for(&repo_id)
                .iter()
                .any(|w| worktree_id_for(&w.path) == worktree_id),
            "the git removal has not completed in this test, so the stale listing must still show the entry"
        );

        // 이제야 SessionStarted가 도착한다.
        let session = SessionStore::spawn_throwaway_for_test();
        let _ = state.update(Message::SessionStarted {
            id: session_id,
            worktree_id: worktree_id.clone(),
            result: Ok(StartedSession::new(session)),
        });

        assert!(
            !state.worktree_sessions.contains_key(&worktree_id),
            "a session racing an in-flight removal must not be accepted into a live slot"
        );
        assert!(
            wait_until(Duration::from_secs(10), || state
                .session_store()
                .reaper_retired_count()
                == 1),
            "the session must have been retired to the reaper instead of leaking"
        );
    }

    // ---- Task 8: persist()가 실제로 배선됐는지. `PersistenceHandle`을 손으로
    // 만든 임시 파일에 꽂아 넣고, git 성공 메시지를 실제로 디스패치한 뒤
    // 디스크에서 다시 읽어 확인한다 — `update()`의 핸들러가 `self.persist()`
    // 호출을 잃으면(mutation) 이 테스트가 잡는다. ----

    #[test]
    fn a_successful_repo_probe_persists_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("data.json");
        let boot = crate::persistence_thread::PersistenceHandle::spawn(file.clone());
        let mut state = AppState {
            persistence: Some(boot.handle),
            ..AppState::default()
        };

        let repo = Repo {
            id: RepoId("/tmp/persisted-repo".into()),
            path: PathBuf::from("/tmp/persisted-repo"),
            display_name: "persisted-repo".to_string(),
            worktree_base_ref: None,
        };
        let _ = state.update(Message::RepoProbed {
            request: OpId(1),
            requested_path: PathBuf::from("/tmp/persisted-repo"),
            result: Ok((repo, None)),
        });

        // 핸들을 놓아 워커가 Disconnected를 보게 하고 밀린 저장을 flush한다.
        state.persistence.take();

        let reloaded = crate::persistence_thread::PersistenceHandle::spawn(file);
        assert_eq!(
            reloaded.load.state.repos.len(),
            1,
            "the repo added via a real update() dispatch must have reached disk"
        );
        assert_eq!(reloaded.load.state.repos[0].display_name, "persisted-repo");
    }

    // ---- pr4 적대적 리뷰 항목 2: worktree가 이 앱을 거치지 않고 밖에서
    // 지워지면(다른 터미널의 `git worktree remove`, 파일 관리자로 디렉토리
    // 삭제 등) `RemoveWorktreeRequested`/`WorktreeRemoved` 경로를 전혀 타지
    // 않는다. 다음 재조회(`apply_worktree_listing`)가 그 worktree를 빼고
    // 도착했을 때 세션을 닫지 않으면 PTY/스레드/reaper 클론이 영원히 산다 ----

    #[test]
    fn a_worktree_that_vanished_externally_has_its_session_closed_on_the_next_listing() {
        let (mut state, id, worktree_id, _pane) = state_with_one_open_session();
        let repo_id = RepoId("/tmp/r2".into());

        assert!(
            state.session_store().is_running(id),
            "sanity: the session must be alive before the worktree disappears"
        );

        // 다음 목록 응답엔 그 worktree가 없다 — 밖에서 지워졌다는 뜻이다.
        state.note_list_issued(repo_id.clone(), OpId(2));
        state.apply_worktree_listing(repo_id, OpId(2), Vec::new());

        assert!(
            !state.session_store().is_running(id),
            "a session for a worktree that vanished externally must be closed, not leaked"
        );
        assert!(
            !state.worktree_sessions.contains_key(&worktree_id),
            "the worktree -> session mapping must be cleared along with the session"
        );
        assert!(
            wait_until(Duration::from_secs(10), || state
                .session_store()
                .reaper_retired_count()
                == 1),
            "the session must actually reach the reaper, not just be dropped from bookkeeping"
        );
    }

    #[test]
    fn a_worktree_that_still_appears_in_the_next_listing_keeps_its_session() {
        let (mut state, id, _worktree_id, _pane) = state_with_one_open_session();
        let repo_id = RepoId("/tmp/r2".into());

        state.note_list_issued(repo_id.clone(), OpId(2));
        state.apply_worktree_listing(
            repo_id,
            OpId(2),
            vec![entry_at("/tmp/accepted", "accepted")],
        );

        assert!(
            state.session_store().is_running(id),
            "a worktree that is still listed must not have its session torn down"
        );
    }
}
