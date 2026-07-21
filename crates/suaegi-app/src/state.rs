use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use futures::StreamExt;
use iced::advanced::clipboard;
use iced::widget::pane_grid;
use suaegi_core::domain::{
    PersistedPane, PersistedState, Repo, RepoId, SessionState, Settings, Worktree, WorktreeId,
    SCHEMA_VERSION,
};
use suaegi_git::compare::{CompareOutcome, FileDiff};

use crate::diff_panel::{panel_state_for, patch_state_for, DiffState};
use suaegi_git::worktree::{BranchDeletion, CreatedWorktree, RemoveOutcome, WorktreeEntry};
use suaegi_term::agent::AgentKind;
use suaegi_term::grid::TerminalSnapshot;
use suaegi_term::input_types::{CopyTargets, WriteOutcome};
use suaegi_term::presence::AgentPresence;

use crate::agent_status::contract::{
    hook_outcome, reduce, BadgeInput, BadgeState, HookEvent, HookOutcome, HookState, Hydration,
    HydrationStep, PaneKey, SpawnNonce, LAYOUT_SAVE_DEBOUNCE,
};
use crate::layout::{leaves_in_order, to_configuration, to_persisted, LeafOutcome};
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

/// 비교가 **결과를 내지 못한** 이유. 분류된 실패(`NoMergeBase` 등)는 여기가
/// 아니라 `CompareOutcome`이 나른다 — 여기 오는 것은 진짜 오류뿐이다.
///
/// **`String` 하나로 두지 않는 이유**: 출력이 러너 상한을 넘은 경우는 오류가
/// 아니라 패널이 그려야 할 상태이고, 그리려면 `limit`이 필요하다. `String`에
/// 담으면 UI가 문구를 파싱해야 하는데 그건 다시 문자열 매칭이다. `Message`가
/// `Clone`이어야 해서 `GitError`(비-`Clone`)를 그대로 나를 수는 없으므로,
/// 필요한 만큼만 담은 `Clone` 타입을 둔다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffFailure {
    TooLarge { limit: usize },
    Failed(String),
}

/// worktree 목록의 **출처**. `Result<Vec<_>, String>`이 아니라 이 타입인 이유는
/// **삭제 판정에 증거를 요구하기 위해서다.**
///
/// `apply_worktree_listing`은 목록에서 사라진 worktree의 세션을 거둔다 — 그것이
/// 밖에서 지워진 worktree를 정리하는 유일한 경로다. 그런데 **저하된 조회(git
/// 실패, 타임아웃)와 성공한 빈 목록은 `Vec`만 봐서는 구별할 수 없다.** 저하를
/// 권위로 오인하면 실패한 스캔 한 번이 살아 있는 세션을 전부 죽인다. 레이아웃
/// 복원이 붙은 지금은 폭발 반경이 더 크다 — **복원된 레이아웃 전체가 지워진다.**
///
/// 그래서 정리는 [`WorktreeListing::Authoritative`]에서만 일어나고, 그 사실이
/// 주석이 아니라 **타입으로** 강제된다.
///
/// `Degraded`가 메시지를 나르는 것은 플랜의 `Degraded`(무인자)에서 벗어난
/// 점이다: 그러지 않으면 사이드바의 오류 배너가 실패 이유를 잃는다. 담고 있는
/// 것이 **entry가 아니라는 것**이 이 타입의 요점이므로 그 요점은 그대로다.
#[derive(Debug, Clone)]
pub enum WorktreeListing {
    Authoritative(Vec<WorktreeEntry>),
    Degraded(String),
}

/// worktree 하나의 생성 메타데이터. `persisted_snapshot`이 매 저장마다
/// `created_at_unix_ms: 0`으로 합성하던 자리표시자를 대신한다(follow-ups #15).
///
/// **`created_with_agent`는 항상 `None`이다.** 채울 소스가 아직 없다 —
/// `WorktreeSelected`가 `AgentKind::Custom, None`을 하드코딩하고 에이전트 선택
/// UI는 범위 밖이다. **가짜로 채우지 않는다**: 틀린 값이 디스크에 굳으면 나중에
/// 진짜 값이 생겼을 때 어느 것이 진짜인지 구별할 수 없다.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorktreeMeta {
    pub created_with_agent: Option<String>,
    pub created_at_unix_ms: u64,
}

#[derive(Debug, Clone)]
pub enum Message {
    RepoProbed {
        request: OpId,
        requested_path: PathBuf,
        result: Result<(Repo, Option<String>), String>,
    },
    /// **`result`가 `Result`가 아닌 이유는 [`WorktreeListing`] 참고** — 저하된
    /// 조회로 세션을 거두면 실패한 스캔 한 번이 복원된 레이아웃을 통째로 지운다.
    WorktreesListed {
        request: OpId,
        repo_id: RepoId,
        result: WorktreeListing,
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
    /// **요청은 `OpId`를 나르지 않는다.** 뷰가 발급할 수 없기 때문이다 —
    /// `next_op()`은 `&mut self`인데 뷰는 `&self`만 쥔다. `update`가 발급하고
    /// `OpId`는 응답(`DiffLoaded`)에 실려 돌아온다. 저장소의 기존 요청/응답 쌍
    /// (`CreateWorktreeSubmitted` → `WorktreeCreated`)과 같은 모양이다.
    DiffRequested {
        worktree: WorktreeId,
    },
    FileDiffRequested {
        worktree: WorktreeId,
        path: String,
    },
    /// 패널을 닫았다. 진행 중인 compare를 취소한다 — **취소는 오류가 아니다**,
    /// 배너를 띄우지 않고 조용히 끝난다.
    DiffCancelled {
        worktree: WorktreeId,
    },
    /// `branch_compare`의 완료. **실패의 분류는 `Err`가 아니라 `CompareOutcome`이
    /// 나른다** — base ref 오타나 공통 조상 없음은 오류가 아니라 보여줄 상태다.
    /// `Err`에 남는 것은 진짜 오류뿐이다. `CompareOutcome::Cancelled`는 조용히 버린다.
    DiffLoaded {
        worktree: WorktreeId,
        op: OpId,
        result: Result<CompareOutcome, DiffFailure>,
    },
    /// 파일 하나의 patch. `FileDiff`가 바이너리·과대·렌더 불가를 함께 나른다 —
    /// 그 셋을 빈 patch로 뭉개면 UI가 "변경 없음"으로 그린다.
    FileDiffLoaded {
        worktree: WorktreeId,
        path: String,
        op: OpId,
        result: Result<FileDiff, String>,
    },
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

    // ---- Plan 5 Task 5: 하이드레이션 게이트 · 레이아웃 복원 ----
    /// 부팅이 끝날 때까지 `persist()`를 막는다. **기본값은 열려 있다** —
    /// `AppState::default()`는 부팅을 거치지 않는 경로(테스트, `AppState::new`)이고
    /// 거기서 닫아두면 저장이 영원히 막힌다. 닫힌 게이트를 세우는 것은
    /// [`AppState::boot`]뿐이다.
    hydration: Hydration,
    /// 디스크에서 읽었지만 아직 복원을 시작하지 않은 pane 트리.
    /// [`AppState::begin_layout_restore`]가 꺼내 쓰고 비운다 — `from_load`를
    /// "디스크 → 상태" 순수 변환으로 두고, 세션을 띄우는 부작용은 `boot()`이
    /// 명시적으로 부르게 하기 위해서다.
    pending_restore_tree: Option<PersistedPane>,
    /// 진행 중인 레이아웃 복원. `Some`인 동안 세션이 시작돼도 **pane을 열지
    /// 않는다** — `pane_grid::State`는 빈 채로 만들 수 없어서 트리를 한 번에
    /// 지어야 하고, 그러려면 모든 잎의 종단 결과가 모여야 한다.
    restore: Option<LayoutRestore>,
    /// worktree별 생성 메타데이터(Task 6). `persisted_snapshot`이 자리표시자
    /// 대신 읽는다.
    worktree_meta: HashMap<WorktreeId, WorktreeMeta>,
    // ---- Plan 5 Task 3: 에이전트 상태 배지 ----
    /// worktree(= `PaneKey`)별 배지 장부. `reduce`의 입력 중 훅에서 오는 절반을
    /// 여기 모으고, 나머지 절반(presence)은 `session_store`에서 읽는다.
    badges: HashMap<WorktreeId, PaneBadge>,
    /// 훅 서버의 포트·토큰. `None`이면 서버가 안 떴다는 뜻이고, 그때는 **배지 없이
    /// 계속 간다** — 훅은 편의 기능이지 세션의 전제가 아니다.
    hook_endpoint: Option<(u16, String)>,
    /// 훅 스크립트의 설치 경로. 부팅 시 한 번 설치하고 worktree 설정이 이걸 가리킨다.
    hook_script: Option<PathBuf>,
    /// 레이아웃 저장 디바운스의 세대. 리사이즈 메시지마다 올리고, 타이머가
    /// 터졌을 때 값이 그대로면 그때 저장한다.
    ///
    /// **`pane_grid::ResizeEvent`에는 드래그 단계 표시가 없다** — `split`과
    /// `ratio`만 나르고 `on_resize`는 드래그 **중에도** 계속 발화한다. "드래그가
    /// 끝났다"는 이벤트는 iced에 존재하지 않으므로 디바운스가 유일한 수단이다.
    layout_generation: u64,

    // ---- Plan 5 Task 4: diff 패널 ----
    /// **필드 하나로 뭉쳐 둔다.** 목록 상태·선택된 파일·patch·staleness 가드·
    /// 취소 손잡이를 `AppState`에 흩뿌리면 이 구조체를 동시에 고치는 다른
    /// 작업과 충돌 면적이 그만큼 넓어진다. 안쪽 불변식도 한곳에 모인다.
    diff: DiffState,
}

/// pane 하나의 배지 장부.
///
/// **`expected` nonce가 이 구조체의 존재 이유다.** `PaneKey`는 worktree에서 파생돼
/// 세션이 교체돼도 같으므로, 옛 Claude 프로세스의 늦은 훅이 새 세션의 배지를 덮을
/// 수 있다(훅이 async라 더 그렇다). 우리가 스폰마다 발급한 nonce와 다른 이벤트는
/// 버린다 — **스폰 시점에 이미 아는 값이라 "첫 이벤트를 믿는" 창이 없다.**
#[derive(Debug, Clone)]
struct PaneBadge {
    expected: SpawnNonce,
    /// 마지막으로 관측한 훅 상태와 그 시각. `None` = 훅을 하나도 못 봤다.
    hook: Option<(HookState, Instant)>,
    /// `NoAgent` streak가 확정되기 전에 유지할 값.
    previous: BadgeState,
    no_agent_streak: u8,
}

impl PaneBadge {
    fn new(expected: SpawnNonce) -> Self {
        Self {
            expected,
            hook: None,
            previous: BadgeState::Unknown,
            no_agent_streak: 0,
        }
    }
}

/// 진행 중인 복원의 장부. 잎마다 종단 결과가 하나씩 모이고, `pending`이 비면
/// 트리를 짓는다.
struct LayoutRestore {
    tree: PersistedPane,
    /// 아직 종단 결과가 오지 않은 잎 → 그 잎을 위해 발급한 세션 id.
    pending: HashMap<WorktreeId, SessionId>,
    /// 이미 결정된 잎. `Started`만 pane이 된다.
    outcomes: HashMap<WorktreeId, LeafOutcome>,
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
            // 부팅을 거치지 않는 경로는 하이드레이션할 것이 없다 — 열어둔다.
            badges: HashMap::new(),
            // 서버 없이도 앱은 완전히 동작한다 — 배지만 `Unknown`에 머문다.
            hook_endpoint: None,
            hook_script: None,
            hydration: Hydration::opened(),
            pending_restore_tree: None,
            restore: None,
            worktree_meta: HashMap::new(),
            layout_generation: 0,
            diff: DiffState::default(),
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
        state.install_hooks();

        let repo_ids: Vec<RepoId> = state.repos.iter().map(|repo| repo.id.clone()).collect();
        // **여기서 게이트를 닫는다.** 이 줄 이후 `persist()`는 부팅이 끝날
        // 때까지 아무것도 쓰지 않는다 — 부분 복원된 상태가 디스크의 멀쩡한
        // 파일을 덮어쓰는 것이 이 게이트가 막는 유일한 사고다.
        state.hydration = Hydration::new(repo_ids.clone());

        let refresh_tasks: Vec<iced::Task<Message>> = repo_ids
            .into_iter()
            .map(|repo_id| state.refresh_worktrees(repo_id))
            .collect();

        // **재조회를 기다리지 않는다.** 복원은 디스크에 저장된 worktree 목록
        // (`from_load`가 씨딩한)으로 시작한다. git 재조회가 도착하면 권위 있는
        // 목록으로 정정되고, 그때 사라진 worktree의 세션은 정리된다.
        let restore_task = state.begin_layout_restore();

        let saved_task = iced::Task::stream(boot.results.map(Message::Saved));

        let mut tasks = refresh_tasks;
        tasks.push(restore_task);
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
        // **부팅 시 실제로 읽는다.** 여기까지가 이 필드의 오랜 공백이었다 —
        // `persisted_snapshot`이 쓰기만 하고 아무도 읽지 않았다.
        state.selected_worktree = load.state.session.active_worktree_id;
        state.pending_restore_tree = load.state.session.panes;
        // 디스크에 저장된 worktree 목록을 그대로 신뢰하지 않고 화면에 먼저
        // 보여주기 위한 최선의 추정치로만 쓴다 — `boot()`이 곧바로 git 재조회를
        // 발급해 정정한다(위 문서 참고). `latest_list_op`는 일부러 세우지 않는다:
        // 재조회가 발급하는 첫 `OpId`가 무엇이든 이 씨딩보다 새것으로 취급돼야
        // 하고, `apply_worktree_listing`은 `latest_list_op`에 없는 repo의 응답을
        // 무조건 받아들이므로 그냥 두면 된다.
        let mut worktrees_by_repo: HashMap<RepoId, Vec<WorktreeEntry>> = HashMap::new();
        for worktree in load.state.worktrees {
            // Task 6: 생성 메타데이터를 씨딩한다. 이게 없으면 다음 저장이
            // 자리표시자(`created_at_unix_ms: 0`)로 덮어써서, 앱을 한 번 열었다
            // 닫는 것만으로 모든 worktree의 생성 시각이 영구히 사라진다.
            state.worktree_meta.insert(
                worktree.id.clone(),
                WorktreeMeta {
                    created_with_agent: worktree.created_with_agent,
                    created_at_unix_ms: worktree.created_at_unix_ms,
                },
            );
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
                entries.iter().map(move |entry| {
                    let id = worktree_id_for(&entry.path);
                    // Task 6: 자리표시자가 아니라 실제 메타데이터를 읽는다.
                    // 아직 모르는 worktree(밖에서 만들어진 것)는 기본값이다 —
                    // **거짓말을 쓰지 않는다**는 점에서 자리표시자와 같지만,
                    // 아는 것을 매 저장마다 지워버리지는 않는다.
                    let meta = self.worktree_meta.get(&id).cloned().unwrap_or_default();
                    Worktree {
                        id,
                        repo_id: repo_id.clone(),
                        path: entry.path.clone(),
                        branch: entry.branch.clone().unwrap_or_default(),
                        display_name: entry
                            .branch
                            .clone()
                            .unwrap_or_else(|| "worktree".to_string()),
                        created_with_agent: meta.created_with_agent,
                        created_at_unix_ms: meta.created_at_unix_ms,
                    }
                })
            })
            .collect();
        PersistedState {
            schema_version: SCHEMA_VERSION,
            repos: self.repos.clone(),
            worktrees,
            session: SessionState {
                active_worktree_id: self.selected_worktree.clone(),
                panes: self.persisted_layout(),
            },
            settings: Settings {
                workspace_root: self.workspace_root.clone(),
            },
        }
    }

    /// 지금 화면의 pane 트리를 저장 가능한 모양으로. 세션이 하나도 없으면
    /// `None`이다.
    fn persisted_layout(&self) -> Option<PersistedPane> {
        let panes = self.panes.as_ref()?;
        to_persisted(panes.layout(), panes, &self.session_worktrees)
    }

    /// 영속화 대상 상태(repo/worktree/선택/레이아웃)가 바뀌었을 때 부른다.
    ///
    /// **하이드레이션 게이트가 닫혀 있으면 아무것도 쓰지 않는다.** 부팅 중간
    /// 단계가 실패했을 때 부분 복원된 상태가 디스크의 멀쩡한 파일을 덮어쓰는
    /// 것을 막는다 — Orca가 정확히 이걸로 사용자 탭을 날렸다(이슈 #1158).
    /// 게이트가 닫힌 동안의 사용자 편집은 **거부하지 않는다**: 메모리에 남아
    /// 있다가 게이트가 열리는 순간 [`AppState::note_hydration`]이 한 번에 저장한다.
    ///
    /// 배선이 안 된 상태(`persistence == None`, 테스트 기본값)에서도 조용히
    /// 아무것도 하지 않는다.
    fn persist(&self) {
        if !self.hydration.is_open() {
            return;
        }
        if let Some(handle) = &self.persistence {
            handle.save(self.persisted_snapshot());
        }
    }

    /// 부팅 진행 한 단계를 반영하고, **그것이 게이트를 여는 단계였다면 곧바로
    /// 한 번 저장한다.** 게이트가 닫힌 동안 쌓인 편집이 디스크에 닿는 유일한
    /// 지점이다 — 여기서 저장하지 않으면 부팅 중 바뀐 것이 다음 사용자 조작
    /// 때까지 떠 있다가 크래시 한 번에 사라진다.
    fn note_hydration(&mut self, step: HydrationStep) {
        let was_open = self.hydration.is_open();
        self.hydration.apply(&step);
        if !was_open && self.hydration.is_open() {
            self.persist();
        }
    }

    // ---- Plan 5 Task 5: 레이아웃 복원 ----

    /// 디스크에서 읽은 트리로 복원을 시작한다. 잎마다 세션을 하나씩 띄우고,
    /// **모든 잎의 종단 결과가 모일 때까지 pane을 하나도 열지 않는다** —
    /// `pane_grid::State`는 빈 채로 만들 수 없어서 트리는 한 번에 지어야 한다.
    ///
    /// 복원할 것이 없으면(첫 실행, 또는 살아남은 잎이 하나도 없으면) 두 단계를
    /// 곧바로 완료 처리한다. **저하된 완료도 완료다** — 여기서 멈추면 게이트가
    /// 영원히 닫혀 사용자가 아무것도 저장할 수 없다.
    fn begin_layout_restore(&mut self) -> iced::Task<Message> {
        let Some(tree) = self.pending_restore_tree.take() else {
            self.finish_restore(None);
            return iced::Task::none();
        };

        let mut restore = LayoutRestore {
            pending: HashMap::new(),
            outcomes: HashMap::new(),
            tree,
        };
        let mut tasks = Vec::new();

        // `leaves_in_order`가 중복을 이미 접어준다 — 같은 worktree로 세션을 두 번
        // 띄우면 PTY가 하나 새고 그중 하나는 어떤 pane도 가리키지 않는다.
        for worktree_id in leaves_in_order(&restore.tree) {
            match self.start_session_for(&worktree_id) {
                Some((session_id, task)) => {
                    restore.pending.insert(worktree_id, session_id);
                    tasks.push(task);
                }
                // worktree가 사라졌다(밖에서 지워졌거나 디스크 목록이 낡았다).
                // 재시도하지 않는다 — 사용자가 다시 열면 된다.
                //
                // **이 기록 자체는 방어적이다**: `to_configuration`이 결과가 아예
                // 없는 잎도 실패와 같이 다루므로 이 줄을 지워도 트리 모양은
                // 같다(mutation으로 확인 — 아무 테스트도 죽지 않는다). 그래도
                // 남기는 것은 결과 맵을 완전하게 만들어, 나중에 잎별 사유를
                // 보여줄 때 "없음"과 "사라짐"을 되짚지 않아도 되게 하기 위해서다.
                None => {
                    restore
                        .outcomes
                        .insert(worktree_id, LeafOutcome::WorktreeGone);
                }
            }
        }

        if restore.pending.is_empty() {
            // 잎이 전부 사라진 트리. 기다릴 것이 없으니 곧바로 짓는다(빈
            // 워크벤치가 될 것이다) — 안 그러면 게이트가 영영 닫혀 있다.
            self.finish_restore(Some(restore));
            return iced::Task::none();
        }

        self.restore = Some(restore);
        iced::Task::batch(tasks)
    }

    /// 잎 하나의 종단 결과를 장부에 적고, 마지막 하나였으면 트리를 짓는다.
    /// **결과가 하나도 유실되면 안 된다** — 하나라도 `pending`에 남으면 게이트가
    /// 열리지 않는다.
    fn note_restore_outcome(&mut self, worktree_id: &WorktreeId, outcome: LeafOutcome) {
        let Some(restore) = &mut self.restore else {
            return;
        };
        if restore.pending.remove(worktree_id).is_none() {
            // 이 잎은 복원 대상이 아니거나 이미 결정됐다.
            //
            // **도달 가능한 상태에서는 이 가드가 관측되지 않는다**(mutation으로
            // 확인): 두 번째 결과가 오려면 `pending`이 이미 비어야 하는데, 그
            // 시점엔 `finish_restore`가 `self.restore`를 `None`으로 만든 뒤라
            // 위의 `let Some(restore)`에서 먼저 빠져나간다. 도달 불가능한 입력을
            // 테스트로 고정하지 않고 가드만 남긴다.
            return;
        }
        restore.outcomes.insert(worktree_id.clone(), outcome);
        if restore.pending.is_empty() {
            let restore = self.restore.take();
            self.finish_restore(restore);
        }
    }

    /// 모인 결과로 pane 트리를 실체화하고 게이트의 남은 두 단계를 닫는다.
    fn finish_restore(&mut self, restore: Option<LayoutRestore>) {
        self.restore = None;
        let config = restore.and_then(|restore| {
            to_configuration(&restore.tree, &restore.outcomes, &mut HashSet::new())
        });

        if let Some(config) = config {
            let panes = pane_grid::State::with_configuration(config);
            // 복원된 `active_worktree_id`의 pane에 포커스를 준다. 그 세션이
            // 살아나지 못했으면 아무 pane이나 — 포커스가 없는 워크벤치는
            // 키 입력이 갈 곳이 없다.
            let active_session = self
                .selected_worktree
                .as_ref()
                .and_then(|id| self.worktree_sessions.get(id))
                .copied();
            self.focused_pane = active_session
                .and_then(|session| {
                    panes
                        .iter()
                        .find(|(_, id)| **id == session)
                        .map(|(pane, _)| *pane)
                })
                .or_else(|| panes.panes.keys().next().copied());
            self.panes = Some(panes);
        }

        self.note_hydration(HydrationStep::SessionsResolved);
        self.note_hydration(HydrationStep::LayoutBuilt);
    }

    /// worktree 하나에 세션을 띄운다. 살아 있는 worktree가 아니면 `None`.
    ///
    /// `WorktreeSelected`와 복원이 **같은 경로를 쓰게 하려고** 뽑았다 — 갈라두면
    /// 한쪽만 `session_titles`나 `pending_session_starts`를 채우고 다른 쪽이
    /// 조용히 새기 시작한다.
    fn start_session_for(&mut self, id: &WorktreeId) -> Option<(SessionId, iced::Task<Message>)> {
        let (repo_id, entry) = self.find_worktree(id)?;
        let session_id = self.session_store.next_id();
        let title = entry
            .branch
            .clone()
            .unwrap_or_else(|| "(detached)".to_string());
        self.session_titles.insert(session_id, title);
        self.pending_session_starts.insert(id.clone(), session_id);

        let created = self.worktree_meta.get(id).cloned().unwrap_or_default();
        let worktree = Worktree {
            id: id.clone(),
            repo_id,
            path: entry.path.clone(),
            branch: entry.branch.clone().unwrap_or_default(),
            display_name: entry
                .branch
                .clone()
                .unwrap_or_else(|| "worktree".to_string()),
            created_with_agent: created.created_with_agent,
            created_at_unix_ms: created.created_at_unix_ms,
        };
        // **스폰마다 새 nonce를 발급하고 배지를 리셋한다.** 같은 worktree의 옛
        // 세션이 남긴 훅이 새 세션의 배지를 덮지 못하게 하는 지점이 여기다 —
        // 이 값을 env로 심고, 되돌아온 이벤트의 nonce가 다르면 버린다.
        let nonce = SpawnNonce::next();
        self.badges.insert(id.clone(), PaneBadge::new(nonce));

        let env = match &self.hook_endpoint {
            Some((port, token)) => {
                crate::agent_status::inject::spawn_env(&PaneKey(id.clone()), nonce, *port, token)
            }
            // 서버가 안 떴다 — 배지 없이 계속 간다. 훅 스크립트는 env가 비면
            // 조용히 아무것도 하지 않는다.
            None => Vec::new(),
        };

        // Custom + 커맨드 없음 = 로그인 셸. 에이전트 실행 커맨드 선택 UI는
        // 범위 밖(§2 스펙 항목 3).
        let task = self
            .session_store
            .start(session_id, &worktree, AgentKind::Custom, None, env);
        Some((session_id, task))
    }

    /// 레이아웃이 바뀐 뒤 저장을 예약한다. **리사이즈 전용이다** — 다른 트리거
    /// (pane 열기/닫기, 포커스 변경, 복원 완료)는 한 번만 발화하므로 곧바로
    /// 저장한다.
    ///
    /// 리사이즈만 디바운스하는 이유는 **`pane_grid::ResizeEvent`에 드래그 단계
    /// 표시가 없기 때문이다**: `split`과 `ratio`만 나르고 `on_resize`는 드래그
    /// 중에도 계속 발화한다. "드래그가 끝났다"는 이벤트는 iced에 없으므로 찾지
    /// 말고, 메시지가 멎고 [`LAYOUT_SAVE_DEBOUNCE`]가 지나면 저장한다.
    fn schedule_layout_save(&mut self) -> iced::Task<Message> {
        self.layout_generation += 1;
        let generation = self.layout_generation;
        iced::Task::future(async move {
            tokio::time::sleep(LAYOUT_SAVE_DEBOUNCE).await;
            Message::LayoutPersistDue { generation }
        })
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
    /// **`Vec`이 아니라 [`WorktreeListing`]을 받는 것이 핵심이다.** 정리는
    /// `Authoritative`에서만 일어난다 — 저하된 조회(git 실패)를 권위로 오인하면
    /// 실패한 스캔 한 번이 살아 있는 세션을 전부 죽이고, 레이아웃 복원이 붙은
    /// 지금은 **복원된 레이아웃 전체가 지워진다.**
    pub fn apply_worktree_listing(&mut self, repo: RepoId, op: OpId, listing: WorktreeListing) {
        let WorktreeListing::Authoritative(entries) = listing else {
            // 저하된 조회는 증거가 아니다. 목록도 갈지 않는다 — 화면에 있던
            // 최선의 추정치가 "아무것도 모른다"보다 낫다.
            return;
        };
        if let Some(latest) = self.latest_list_op.get(&repo) {
            if op.0 < latest.0 {
                return;
            }
        }
        let still_present: HashSet<WorktreeId> =
            entries.iter().map(|e| worktree_id_for(&e.path)).collect();
        let vanished: Vec<WorktreeId> = self
            .worktrees_by_repo
            .get(&repo)
            .into_iter()
            .flatten()
            .map(|e| worktree_id_for(&e.path))
            .filter(|id| !still_present.contains(id))
            .collect();
        let vanished_sessions: Vec<SessionId> = vanished
            .iter()
            .filter_map(|id| self.worktree_sessions.get(id).copied())
            .collect();
        self.worktrees_by_repo.insert(repo, entries);
        for id in &vanished {
            // 사라진 worktree의 메타데이터를 남겨두면 맵이 앱 수명 내내 자란다.
            self.worktree_meta.remove(id);
        }
        for session_id in vanished_sessions {
            self.close_session(session_id);
        }
    }

    /// 테스트가 권위 있는 목록을 넣는 지름길. **프로덕션에는 없다** — 실제
    /// 경로는 `WorktreesListed`가 나른 [`WorktreeListing`]을 그대로 넘겨야
    /// `Degraded`를 권위로 오인할 수 없다는 타입 보장이 유지된다.
    #[cfg(test)]
    pub(crate) fn apply_authoritative_listing(
        &mut self,
        repo: RepoId,
        op: OpId,
        entries: Vec<WorktreeEntry>,
    ) {
        self.apply_worktree_listing(repo, op, WorktreeListing::Authoritative(entries));
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

    /// 훅 스크립트를 설치하고, 이미 떠 있는 서버의 포트·토큰을 받아 둔다.
    ///
    /// **어느 단계가 실패해도 부팅은 계속된다.** 훅은 배지를 위한 편의 기능이지
    /// 세션의 전제가 아니다 — 스크립트를 못 쓰면 배지가 `Unknown`에 머물 뿐,
    /// 터미널은 평소대로 돈다. 그래서 `Result`를 위로 던지지 않는다.
    fn install_hooks(&mut self) {
        let path = crate::agent_status::inject::hook_script_path();
        match crate::agent_status::inject::install_hook_script(&path) {
            Ok(()) => self.hook_script = Some(path),
            Err(e) => eprintln!("suaegi: could not install the hook script: {e} (badges will stay Unknown)"),
        }
    }

    /// worktree 하나에 훅 설정을 쓴다. 실패해도 조용히 넘어간다(위와 같은 이유).
    ///
    /// **`.git/info/exclude`는 건드리지 않는다.** 그 파일은 worktree가 아니라
    /// 공용 git 디렉터리에 살아서, 여기서 쓰면 사용자의 저장소 **전체**에 영구적인
    /// 무시 규칙이 생기고 `git worktree remove` 뒤에도 남는다. 우리가 만든
    /// `.claude/`를 diff에서 빼는 일은 diff 패널이 자기 수집 단계에서 한다.
    fn inject_into_worktree(&self, worktree_path: &Path) {
        let Some(script) = &self.hook_script else {
            return;
        };
        if let Err(e) = crate::agent_status::inject::write_worktree_settings(worktree_path, script) {
            eprintln!("suaegi: could not write hook settings into the worktree: {e}");
        }
    }

    /// 훅 서버가 뜬 뒤 그 좌표를 심는다. `run()`이 `boot()`보다 **먼저** 서버를
    /// 띄우므로(세션 스폰이 포트를 알아야 한다) 별도 진입점으로 둔다.
    pub fn attach_hook_server(&mut self, port: u16, token: String) {
        self.hook_endpoint = Some((port, token));
    }

    /// worktree 하나의 배지. 훅(장부)과 폴링(세션 스토어)을 `reduce`로 합성한다 —
    /// **배지는 저장되지 않고 매번 파생된다.** 나이 기반 규칙이 있으므로 저장하면
    /// 시간이 흘러도 갱신되지 않는 값이 생긴다.
    pub(crate) fn worktree_badge(&self, worktree_id: &WorktreeId) -> BadgeState {
        let presence = self.worktree_presence(worktree_id);
        match self.badges.get(worktree_id) {
            Some(badge) => reduce(&BadgeInput {
                presence,
                hook: badge.hook,
                previous: badge.previous,
                no_agent_streak: badge.no_agent_streak,
                now: Instant::now(),
            }),
            // 장부가 없다 = 이 worktree로 세션을 띄운 적이 없다. 훅이 하나도 없는
            // 것과 같은 입력이다.
            None => reduce(&BadgeInput {
                presence,
                hook: None,
                previous: BadgeState::Unknown,
                no_agent_streak: 0,
                now: Instant::now(),
            }),
        }
    }

    /// 훅 이벤트 하나를 장부에 반영한다.
    ///
    /// **nonce가 다르면 버린다.** 세션이 교체된 뒤 도착한 옛 프로세스의 훅이
    /// 새 세션의 배지를 덮는 것을 막는 유일한 방어다.
    fn apply_hook(&mut self, event: &HookEvent) {
        let worktree_id = &event.pane_key.0;
        let Some(badge) = self.badges.get_mut(worktree_id) else {
            // 우리가 스폰한 적 없는 pane의 이벤트다.
            return;
        };
        if event.spawn_nonce != badge.expected {
            // 옛 세대의 늦은 훅. **조용히 버린다** — 오류가 아니다.
            return;
        }
        match hook_outcome(event) {
            HookOutcome::Ignore => {}
            HookOutcome::Reset => badge.hook = None,
            HookOutcome::Set(state) => badge.hook = Some((state, Instant::now())),
        }
    }

    /// presence 관측 하나를 배지 장부에 반영한다. `reduce`가 `NoAgent` streak를
    /// 읽으므로 그 카운터는 폴링이 도는 곳에서 유지돼야 한다.
    fn note_presence_for_badge(&mut self, id: SessionId, presence: AgentPresence) {
        let Some(worktree_id) = self.session_worktrees.get(&id).cloned() else {
            return;
        };
        let Some(badge) = self.badges.get_mut(&worktree_id) else {
            return;
        };
        match presence {
            AgentPresence::NoAgent => {
                badge.no_agent_streak = badge.no_agent_streak.saturating_add(1);
            }
            // **`Agent`와 `Exited`를 보면 0으로 리셋한다.** `Unknown`은 관측 실패지
            // "에이전트가 없다"가 아니므로 streak를 건드리지 않는다.
            AgentPresence::Agent(_) | AgentPresence::Exited { .. } => {
                badge.no_agent_streak = 0;
            }
            AgentPresence::Unknown => {}
        }
        // streak가 임계 미만일 때 `reduce`가 들 값을 갱신한다. **확정되지 않은
        // `NoAgent`에서는 갱신하지 않는다** — 그러면 유지하려던 값이 자기 자신으로
        // 덮여 "유지"가 의미를 잃는다.
        if !matches!(presence, AgentPresence::NoAgent) {
            badge.previous = reduce(&BadgeInput {
                presence,
                hook: badge.hook,
                previous: badge.previous,
                no_agent_streak: badge.no_agent_streak,
                now: Instant::now(),
            });
        }
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

    pub(crate) fn diff(&self) -> &DiffState {
        &self.diff
    }

    /// 사이드바의 토글. 이미 그 worktree를 보고 있으면 **닫는다**(토글이니까).
    ///
    /// base ref는 repo에 등록된 기본 브랜치 하나로 고정한다 — 선택 UI는 범위
    /// 밖이다. `WorktreeSelected`가 쓰는 것과 같은 규칙이다.
    fn request_diff(&mut self, worktree: WorktreeId) -> iced::Task<Message> {
        if self.diff.is_open() && self.diff.worktree() == Some(&worktree) {
            self.diff.close();
            return iced::Task::none();
        }
        let Some((repo_id, entry)) = self.find_worktree(&worktree) else {
            return iced::Task::none();
        };
        let Some(repo) = self.repo_by_id(&repo_id) else {
            return iced::Task::none();
        };
        let base_ref = repo
            .worktree_base_ref
            .clone()
            .unwrap_or_else(|| "main".to_string());

        let op = self.next_op();
        let cancel = self.diff.begin_compare(worktree.clone(), op);
        crate::git_tasks::compare_worktree(op, worktree, entry.path, base_ref, cancel)
    }

    /// **`ChangeStatus`가 있어야 patch를 요청할 수 있다** — 스니핑할 리비전이
    /// 상태에 달렸고(`Deleted`는 working tree에 없다), `Other(c)`는 git을
    /// 부르지 않고 끝내야 한다. 목록에 없는 파일이면 조용히 무시한다.
    fn request_file_diff(&mut self, worktree: WorktreeId, path: String) -> iced::Task<Message> {
        let Some(status) = self.diff.status_of(&path) else {
            return iced::Task::none();
        };
        let Some((repo_id, entry)) = self.find_worktree(&worktree) else {
            return iced::Task::none();
        };
        let Some(repo) = self.repo_by_id(&repo_id) else {
            return iced::Task::none();
        };
        let base_ref = repo
            .worktree_base_ref
            .clone()
            .unwrap_or_else(|| "main".to_string());

        let op = self.next_op();
        self.diff.begin_patch(path.clone(), op);
        crate::git_tasks::file_patch(op, worktree, entry.path, base_ref, path, status)
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
            // **복원 중이라면 이 잎의 결과를 되물러야 한다.** 복원이 끝나기
            // 전에 worktree가 밖에서 지워지면(권위 있는 재조회가 그걸 본다)
            // 이미 `Started`로 적힌 잎이 죽은 세션을 가리키게 되고, 그대로
            // 트리를 지으면 빈 터미널 pane이 하나 남는다.
            if let Some(restore) = &mut self.restore {
                if restore.outcomes.get(&worktree_id) == Some(&LeafOutcome::Started(id)) {
                    restore
                        .outcomes
                        .insert(worktree_id, LeafOutcome::WorktreeGone);
                }
            }
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

        // **포커스된 pane이 곧 활성 worktree다.** 이 줄이 없으면 플랜이 요구한
        // "포커스 변경 시 저장" 트리거가 아무것도 바꾸지 않는 공회전이 된다 —
        // 저장되는 값 중 포커스에 따라 달라지는 것이 하나도 없기 때문이다.
        // 부팅 시 `active_worktree_id`로 어느 pane에 포커스를 줄지 정하는 것과
        // 짝을 이룬다.
        if let Some(worktree_id) = next.and_then(|id| self.session_worktrees.get(&id)).cloned() {
            if self.selected_worktree.as_ref() != Some(&worktree_id) {
                self.selected_worktree = Some(worktree_id);
                self.persist();
            }
        }

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
            } => {
                // **성공이든 저하든 이 repo는 하이드레이션에서 빠진다.** 저하된
                // 완료도 완료다 — 실패한 조회 하나가 게이트를 영원히 닫아두면
                // 사용자가 아무것도 저장할 수 없다. 낡은 응답은 빼지 않는다:
                // 지금 요청의 `OpId`와 대조해야, 아직 진행 중인 조회가 있는데도
                // 게이트가 일찍 열리는 일이 없다.
                let is_current = self.latest_list_op.get(&repo_id) == Some(&request);
                let degraded = match &result {
                    WorktreeListing::Degraded(err) => Some(err.clone()),
                    WorktreeListing::Authoritative(_) => None,
                };
                // **저하 여부와 무관하게 넘긴다.** 정리할지 말지는
                // `apply_worktree_listing`이 타입으로 판단한다 — 여기서 미리
                // 걸러내면 그 타입 보장이 아무도 지나지 않는 죽은 코드가 되고,
                // 다른 호출자가 생겼을 때 조용히 무너진다.
                self.apply_worktree_listing(repo_id.clone(), request, result);
                match degraded {
                    Some(err) => self.last_error = Some(err),
                    None => {
                        self.last_error = None;
                        self.persist();
                    }
                }
                if is_current {
                    self.note_hydration(HydrationStep::ReposListed(repo_id));
                }
                iced::Task::none()
            }
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
                // **Task 6: 생성 시점이 메타데이터의 유일한 진짜 출처다.**
                // 여기서 `Ok(_created)`를 통째로 버리면 그 시각은 영영 없다 —
                // `persisted_snapshot`이 매 저장마다 0을 합성하게 된다.
                Ok(created) => {
                    self.last_error = None;
                    self.worktree_name_draft.remove(&repo_id);
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    // **주입은 worktree 생성 직후다.** 사용자가 그 안에서
                    // `claude`를 어떻게 띄우든(맨손, `--resume`, 별칭) 설정이
                    // 적용되게 하는 유일한 지점이다 — 우리는 `claude`를 직접
                    // 실행하지 않으므로 `--settings`를 넘길 argv가 없다.
                    self.inject_into_worktree(&created.path);
                    self.worktree_meta.insert(
                        worktree_id_for(&created.path),
                        WorktreeMeta {
                            // **가짜로 채우지 않는다.** 에이전트 선택 UI가 범위
                            // 밖이라 채울 소스가 없다 — 틀린 값이 디스크에 굳으면
                            // 나중에 진짜 값이 생겼을 때 구별할 수 없다.
                            created_with_agent: None,
                            created_at_unix_ms: now_ms,
                        },
                    );
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
                // 복원과 **같은 경로**를 쓴다 — 갈라두면 한쪽만 장부를 채운다.
                match self.start_session_for(&id) {
                    Some((_session_id, task)) => task,
                    None => iced::Task::none(),
                }
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
                // 복원 중인 잎인가. **복원 중에는 pane을 열지 않는다** — 트리는
                // 모든 잎이 결정된 뒤 한 번에 짓는다.
                let restoring = self
                    .restore
                    .as_ref()
                    .is_some_and(|r| r.pending.contains_key(&worktree_id));
                match result {
                    Ok(started) => {
                        self.last_error = None;
                        let Some(session) = started.take() else {
                            // 이미 다른 곳에서 소비됐다 — 정상 경로에서는 밟지
                            // 않지만(봉투는 한 번만 만들어진다), 방어적으로
                            // 무시한다.
                            self.session_titles.remove(&id);
                            // **그래도 잎은 결정해야 한다** — 여기서 빠져나가면
                            // 그 잎이 `pending`에 남아 게이트가 영원히 닫힌다.
                            self.note_restore_outcome(&worktree_id, LeafOutcome::Failed);
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
                                self.session_worktrees.insert(id, worktree_id.clone());
                                if restoring {
                                    self.note_restore_outcome(
                                        &worktree_id,
                                        LeafOutcome::Started(id),
                                    );
                                } else {
                                    self.open_pane_for_session(id);
                                    self.persist();
                                }
                            }
                            Err(_) => {
                                // worktree가 그새 삭제됐다 — 세션은 이미 reaper로
                                // 갔다(`accept_started`). 타이틀만 정리한다.
                                self.session_titles.remove(&id);
                                self.note_restore_outcome(
                                    &worktree_id,
                                    LeafOutcome::WorktreeGone,
                                );
                            }
                        }
                    }
                    Err(err) => {
                        self.session_titles.remove(&id);
                        self.last_error = Some(err);
                        self.note_restore_outcome(&worktree_id, LeafOutcome::Failed);
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
                // 드롭은 트리를 바꾼다. 플랜의 트리거 목록에는 없지만 "pane
                // 열기/닫기"와 같은 종류의 변경이고, 저장하지 않으면 사용자가
                // 옮겨놓은 배치가 재시작에 사라진다.
                self.persist();
                iced::Task::none()
            }
            Message::PaneDragged(_) => iced::Task::none(),
            Message::PaneResized(pane_grid::ResizeEvent { split, ratio }) => {
                if let Some(panes) = &mut self.panes {
                    panes.resize(split, ratio);
                }
                // **곧바로 저장하지 않는다** — 드래그 한 번에 이 메시지가 수십 번
                // 온다. 디바운스의 이유는 `schedule_layout_save` 참고.
                self.schedule_layout_save()
            }
            Message::PaneCloseRequested(pane) => {
                // pane 자체를 지우는 것도 `close_session`이 한다 — 대화형/비대화형
                // 경로가 갈라져서 pane이 새던 것이 이 수렴의 이유다.
                if let Some(&session_id) = self.panes.as_ref().and_then(|panes| panes.get(pane)) {
                    self.close_session(session_id);
                    self.persist();
                }
                iced::Task::none()
            }

            Message::PresenceReady {
                id,
                generation,
                presence,
            } => {
                self.session_store.apply_presence(id, generation, presence);
                self.note_presence_for_badge(id, presence);
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
            Message::HookArrived(event) => {
                self.apply_hook(&event);
                iced::Task::none()
            }
            // **아무도 이 메시지를 발행하지 않는다 — 의도된 것이다.**
            //
            // 0.6은 배지 재계산 틱을 "presence 폴링과 같은 티어"로 두라고 했는데,
            // `presence_poll::subscription`이 **이미 그 티어의 전역 타이머**다
            // (`iced::time::every(tier)`는 세션이 하나도 없어도 750ms/2s로 돈다).
            // 그 틱마다 `update`가 돌고 iced가 다시 그리며, 배지는 그릴 때마다
            // `worktree_badge`가 `Instant::now()`로 새로 파생하므로 나이 기반
            // 규칙(`HOOK_STALE_AFTER`)이 자연히 반영된다.
            //
            // 따라서 별도 타이머를 붙이면 **같은 주기의 중복 타이머**가 될 뿐이다.
            // 변형을 지우지 않고 남겨두는 것은 그것이 0.6의 계약이기 때문이고,
            // 지금 처리가 no-op인 이유를 여기 적어 다음 사람이 "배선이 빠졌다"고
            // 오해하지 않게 한다.
            // ---- Plan 5 Task 4: diff 패널 ----
            Message::DiffRequested { worktree } => self.request_diff(worktree),
            Message::DiffCancelled { .. } => {
                // 취소는 조용하다. 배너도, 오류도, `last_error`도 없다.
                self.diff.close();
                iced::Task::none()
            }
            Message::DiffLoaded {
                worktree,
                op,
                result,
            } => {
                if !self.diff.accept_compare(&worktree, op) {
                    return iced::Task::none();
                }
                // **`None`은 취소다 — 상태를 건드리지 않는다.** 여기서
                // "`Ready`가 아니면 실패"로 쓰면 패널을 닫을 때마다 배너가 뜬다.
                if let Some(panel) = panel_state_for(result) {
                    self.diff.apply_compare(panel);
                }
                iced::Task::none()
            }
            Message::FileDiffRequested { worktree, path } => self.request_file_diff(worktree, path),
            Message::FileDiffLoaded {
                worktree,
                path,
                op,
                result,
            } => {
                let _ = &path; // 상관관계는 `op`가 이미 함의한다(`accept_patch` 참고)
                if !self.diff.accept_patch(&worktree, op) {
                    return iced::Task::none();
                }
                self.diff.apply_patch(patch_state_for(result));
                iced::Task::none()
            }
            Message::HydrationStep(step) => {
                self.note_hydration(step);
                iced::Task::none()
            }
            Message::LayoutPersistDue { generation } => {
                // **최신 세대만 저장한다.** 드래그 중에 걸린 앞선 타이머들이
                // 여기서 전부 걸러진다. 세대가 그대로라는 것은 이 타이머가 걸린
                // 뒤로 리사이즈가 하나도 없었다는 뜻이다 = 드래그가 멎었다.
                if generation == self.layout_generation {
                    self.persist();
                }
                iced::Task::none()
            }
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
    use suaegi_core::domain::PersistedAxis;
    use crate::agent_status::contract::{HookEventName, NO_AGENT_CONFIRMATIONS};

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
        state.apply_authoritative_listing(repo.clone(), OpId(2), vec![entry("new")]);
        // 앞서 발급된 목록이 뒤늦게 도착
        state.apply_authoritative_listing(repo.clone(), OpId(1), vec![entry("old")]);
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
        state.apply_authoritative_listing(repo_id, OpId(1), vec![e]);

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
        state.apply_authoritative_listing(
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
        state.apply_authoritative_listing(
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
        state.apply_authoritative_listing(repo_id, OpId(2), vec![entry_at("/tmp/wt-b", "b")]);

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
        state.apply_authoritative_listing(repo_id.clone(), OpId(1), vec![entry_at("/tmp/r/wt", "wt")]);

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
        state.apply_authoritative_listing(repo_id.clone(), OpId(1), vec![entry_at("/tmp/r/wt", "wt")]);

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
        state.apply_authoritative_listing(repo_id.clone(), OpId(1), vec![e.clone()]);

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
        state.apply_authoritative_listing(repo_id, OpId(2), Vec::new());

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

    // ================= Plan 5 Task 5: 레이아웃 복원 =================

    fn wt(path: &str) -> WorktreeId {
        WorktreeId(path.to_string())
    }

    fn leaf(path: &str) -> PersistedPane {
        PersistedPane::Leaf(wt(path))
    }

    fn split(axis: PersistedAxis, ratio: f32, a: PersistedPane, b: PersistedPane) -> PersistedPane {
        PersistedPane::Split {
            axis,
            ratio,
            a: Box::new(a),
            b: Box::new(b),
        }
    }

    /// 디스크에서 트리를 읽은 직후의 상태. `listed`에 있는 worktree만 살아 있다 —
    /// 나머지 잎은 곧바로 `WorktreeGone`으로 결정된다.
    ///
    /// **게이트를 닫아둔다**(`Hydration::new([])`): 복원이 끝나야 열리는지가
    /// 이 테스트들의 관심사 절반이다.
    fn restoring_state(tree: PersistedPane, listed: &[&str]) -> AppState {
        let mut state = AppState::default();
        let repo_id = RepoId("/tmp/restore-repo".into());
        state.note_list_issued(repo_id.clone(), OpId(1));
        state.apply_authoritative_listing(
            repo_id,
            OpId(1),
            listed
                .iter()
                .map(|p| entry_at(p, p.rsplit('/').next().unwrap_or("wt")))
                .collect(),
        );
        state.pending_restore_tree = Some(tree);
        state.hydration = Hydration::new([]);
        let _ = state.begin_layout_restore();
        state
    }

    /// 잎 하나의 시작 결과를 배달한다. 진짜 `SessionStarted` 디스패치를 태운다 —
    /// 장부를 손으로 채우면 `update()`의 배선이 죽어도 통과한다.
    fn deliver_start(state: &mut AppState, worktree: &str, started: bool) {
        let worktree_id = wt(worktree);
        let id = *state
            .restore
            .as_ref()
            .expect("a restore must be in progress")
            .pending
            .get(&worktree_id)
            .unwrap_or_else(|| panic!("{worktree} must be a pending restore leaf"));
        let result = match started {
            true => Ok(StartedSession::new(SessionStore::spawn_throwaway_for_test())),
            false => Err("pty spawn failed".to_string()),
        };
        let _ = state.update(Message::SessionStarted {
            id,
            worktree_id,
            result,
        });
    }

    /// 복원된 pane 트리를 비교 가능한 문자열로. 세션 id는 실행마다 달라지므로
    /// **worktree 이름으로** 찍는다 — 그래야 단언이 읽히고 안정적이다.
    fn restored_shape(state: &AppState) -> String {
        fn walk(
            node: &pane_grid::Node,
            panes: &pane_grid::State<SessionId>,
            session_worktrees: &HashMap<SessionId, WorktreeId>,
        ) -> String {
            match node {
                pane_grid::Node::Pane(pane) => panes
                    .get(*pane)
                    .and_then(|id| session_worktrees.get(id))
                    .map(|w| w.0.rsplit('/').next().unwrap_or("?").to_string())
                    .unwrap_or_else(|| "?".to_string()),
                pane_grid::Node::Split {
                    axis, ratio, a, b, ..
                } => format!(
                    "({}{:.3} {} {})",
                    match axis {
                        pane_grid::Axis::Horizontal => "H",
                        pane_grid::Axis::Vertical => "V",
                    },
                    ratio,
                    walk(a, panes, session_worktrees),
                    walk(b, panes, session_worktrees)
                ),
            }
        }
        match state.panes() {
            None => "-".to_string(),
            Some(panes) => walk(panes.layout(), panes, &state.session_worktrees),
        }
    }

    /// **대조군이자 왕복 증명**: 저장한 트리가 그대로 다시 선다.
    #[test]
    fn a_fully_successful_restore_rebuilds_the_saved_tree() {
        let tree = split(
            PersistedAxis::Vertical,
            0.25,
            leaf("/tmp/wt-a"),
            split(
                PersistedAxis::Horizontal,
                0.75,
                leaf("/tmp/wt-b"),
                leaf("/tmp/wt-c"),
            ),
        );
        let mut state = restoring_state(tree, &["/tmp/wt-a", "/tmp/wt-b", "/tmp/wt-c"]);

        assert!(
            state.panes().is_none(),
            "no pane may exist until every leaf has reported — pane_grid cannot be \
             built empty and grown, so a half-built tree would be visible to the user"
        );
        assert!(
            !state.hydration.is_open(),
            "the gate must stay closed while the restore is in flight"
        );

        deliver_start(&mut state, "/tmp/wt-a", true);
        deliver_start(&mut state, "/tmp/wt-b", true);
        assert!(
            state.panes().is_none(),
            "two of three is still not all of them"
        );
        deliver_start(&mut state, "/tmp/wt-c", true);

        assert_eq!(
            restored_shape(&state),
            "(V0.250 wt-a (H0.750 wt-b wt-c))",
            "control: with every leaf started the tree must come back exactly as saved"
        );
        assert!(
            state.hydration.is_open(),
            "finishing the restore must open the gate — otherwise the user can never save"
        );
    }

    /// 배리어를 **앱 배선을 통해** 확인한다. 순수 함수 테스트가 규칙을 고정하고,
    /// 이 테스트는 `SessionStarted`의 실패가 실제로 그 규칙에 도달하는지를 본다.
    #[test]
    fn a_leaf_that_fails_to_start_collapses_into_its_sibling() {
        let tree = split(
            PersistedAxis::Vertical,
            0.25,
            leaf("/tmp/wt-a"),
            leaf("/tmp/wt-b"),
        );
        let mut state = restoring_state(tree, &["/tmp/wt-a", "/tmp/wt-b"]);

        deliver_start(&mut state, "/tmp/wt-a", false);
        deliver_start(&mut state, "/tmp/wt-b", true);

        assert_eq!(
            restored_shape(&state),
            "wt-b",
            "the surviving sibling takes the split's place — partial restore is allowed"
        );
        assert!(state.hydration.is_open());
    }

    /// 플랜이 명시적으로 요구한 경우 하나: **직계 자식 둘 다 실패.**
    #[test]
    fn a_restore_in_which_both_children_fail_leaves_an_empty_workbench() {
        let tree = split(
            PersistedAxis::Vertical,
            0.25,
            leaf("/tmp/wt-a"),
            leaf("/tmp/wt-b"),
        );
        let mut state = restoring_state(tree, &["/tmp/wt-a", "/tmp/wt-b"]);

        deliver_start(&mut state, "/tmp/wt-a", false);
        deliver_start(&mut state, "/tmp/wt-b", false);

        assert_eq!(restored_shape(&state), "-", "no survivor means no workbench");
        assert!(
            state.hydration.is_open(),
            "a restore in which EVERYTHING failed is still a completed restore — if the \
             gate stayed shut here the user could never save anything again"
        );
    }

    /// 두 번째로 요구된 경우: **중첩 서브트리가 통째로 비는 경우.**
    #[test]
    fn a_nested_subtree_that_empties_promotes_its_uncle_through_the_app() {
        let tree = split(
            PersistedAxis::Vertical,
            0.25,
            leaf("/tmp/wt-keep"),
            split(
                PersistedAxis::Horizontal,
                0.75,
                leaf("/tmp/wt-x"),
                leaf("/tmp/wt-y"),
            ),
        );
        let mut state = restoring_state(tree, &["/tmp/wt-keep", "/tmp/wt-x", "/tmp/wt-y"]);

        deliver_start(&mut state, "/tmp/wt-keep", true);
        deliver_start(&mut state, "/tmp/wt-x", false);
        deliver_start(&mut state, "/tmp/wt-y", false);

        assert_eq!(
            restored_shape(&state),
            "wt-keep",
            "the entire right subtree vanished; the left leaf must become the root, not \
             the root of a split with an empty side"
        );
    }

    /// 디스크의 목록에 없는 잎은 세션을 아예 띄우지 않는다 — `WorktreeGone`이다.
    /// 그리고 **그 잎만으로 이뤄진 트리는 기다릴 것이 없으므로 곧바로 끝나야
    /// 한다**: 여기서 게이트가 안 열리면 부팅이 영원히 멈춘다.
    #[test]
    fn leaves_whose_worktree_is_gone_never_start_a_session() {
        let tree = split(
            PersistedAxis::Vertical,
            0.25,
            leaf("/tmp/wt-gone"),
            leaf("/tmp/wt-also-gone"),
        );
        let state = restoring_state(tree, &[]);

        assert!(state.restore.is_none(), "there is nothing to wait for");
        assert!(
            state.pending_session_starts.is_empty(),
            "a worktree that is not in the listing must not have a session spawned for it"
        );
        assert_eq!(restored_shape(&state), "-");
        assert!(
            state.hydration.is_open(),
            "a restore with nothing to restore completes immediately"
        );
    }

    /// 복원할 트리가 아예 없는 첫 실행. 게이트는 즉시 열려야 한다.
    #[test]
    fn a_first_run_with_no_saved_layout_opens_the_gate_immediately() {
        let mut state = AppState::default();
        state.hydration = Hydration::new([]);
        assert!(!state.hydration.is_open(), "precondition: closed");

        let _ = state.begin_layout_restore();

        assert!(
            state.hydration.is_open(),
            "with no layout to restore both remaining steps are trivially done"
        );
        assert!(state.panes().is_none());
    }

    /// 중복 잎이 **세션을 두 번 띄우지 않는다.** 순수 함수 테스트는 트리 모양을
    /// 고정하지만, 세션이 두 개 뜨면 PTY 하나가 어떤 pane도 가리키지 않은 채 샌다.
    #[test]
    fn a_duplicated_leaf_starts_exactly_one_session() {
        let tree = split(
            PersistedAxis::Vertical,
            0.25,
            leaf("/tmp/wt-dup"),
            leaf("/tmp/wt-dup"),
        );
        let mut state = restoring_state(tree, &["/tmp/wt-dup"]);

        assert_eq!(
            state.pending_session_starts.len(),
            1,
            "one worktree means one session, however many leaves name it — two sessions \
             would leak a PTY and make PaneKey ambiguous for hook routing"
        );

        deliver_start(&mut state, "/tmp/wt-dup", true);
        assert_eq!(
            restored_shape(&state),
            "wt-dup",
            "the duplicate folds away, leaving a single pane"
        );
        assert_eq!(state.panes().expect("one pane").len(), 1);
    }

    /// 복원이 끝나기 **전에** worktree가 밖에서 사라지면, 이미 `Started`로 적힌
    /// 잎이 죽은 세션을 가리키게 된다. 그대로 트리를 지으면 빈 터미널 pane이
    /// 하나 남는다.
    #[test]
    fn a_session_that_dies_mid_restore_does_not_become_a_pane() {
        let tree = split(
            PersistedAxis::Vertical,
            0.25,
            leaf("/tmp/wt-a"),
            leaf("/tmp/wt-b"),
        );
        let mut state = restoring_state(tree, &["/tmp/wt-a", "/tmp/wt-b"]);

        deliver_start(&mut state, "/tmp/wt-a", true);
        // 권위 있는 재조회가 wt-a가 사라졌다고 알린다 — 아직 wt-b는 미결이다.
        let repo_id = RepoId("/tmp/restore-repo".into());
        state.note_list_issued(repo_id.clone(), OpId(2));
        state.apply_authoritative_listing(repo_id, OpId(2), vec![entry_at("/tmp/wt-b", "wt-b")]);

        deliver_start(&mut state, "/tmp/wt-b", true);

        assert_eq!(
            restored_shape(&state),
            "wt-b",
            "wt-a's session was reaped before the tree was built, so its leaf must have \
             been retracted — otherwise the pane points at a dead session and renders \
             an empty terminal forever"
        );
    }

    // ---- 하이드레이션 게이트: 저장을 막는가 ----

    fn wired_state(file: &Path) -> AppState {
        let boot = crate::persistence_thread::PersistenceHandle::spawn(file.to_path_buf());
        AppState {
            persistence: Some(boot.handle),
            ..AppState::default()
        }
    }

    /// 상태를 계속 쓰면서 디스크를 들여다본다. 핸들을 놓았다가(= flush) 새로
    /// 붙인다.
    ///
    /// **flush 없이 파일을 읽으면 안 된다.** 저장 워커는 다른 스레드이고 `save`는
    /// 논블로킹이라, 그냥 읽으면 "아직 안 쓴 것"과 "안 쓰기로 한 것"이 구별되지
    /// 않는다 — 즉 저장을 막는 코드를 통째로 지워도 단언이 통과한다.
    /// (mutation으로 실제 확인했다: 게이트를 지운 뮤턴트가 그렇게 살아남았다.)
    fn flush_and_peek(state: &mut AppState, file: &Path) -> PersistedState {
        state.persistence.take(); // drop → 워커가 밀린 저장을 flush하고 join
        let boot = crate::persistence_thread::PersistenceHandle::spawn(file.to_path_buf());
        let loaded = boot.load.state.clone();
        state.persistence = Some(boot.handle);
        loaded
    }

    /// 저장이 실제로 디스크에 닿았는지 보는 유일한 방법: 핸들을 놓아 워커가
    /// `Disconnected`를 보고 밀린 저장을 flush하게 한 뒤 다시 읽는다.
    fn flush_and_reload(mut state: AppState, file: &Path) -> PersistedState {
        state.persistence.take();
        drop(state);
        crate::persistence_thread::PersistenceHandle::spawn(file.to_path_buf())
            .load
            .state
    }

    /// 레이아웃 복원은 이미 끝났고 **repo 조회 하나만** 남은 게이트. 그래야
    /// "이 단계가 게이트를 연다"는 단언이 그 단계에 대한 것이 된다 — 다른
    /// 단계가 같이 미결이면 무엇을 고쳐도 게이트는 닫힌 채라 아무것도 못 잡는다.
    fn gate_waiting_only_on(repo: RepoId) -> Hydration {
        let mut hydration = Hydration::new([repo]);
        hydration.apply(&HydrationStep::SessionsResolved);
        hydration.apply(&HydrationStep::LayoutBuilt);
        hydration
    }

    fn some_repo(name: &str) -> Repo {
        Repo {
            id: RepoId(format!("/tmp/{name}")),
            path: PathBuf::from(format!("/tmp/{name}")),
            display_name: name.to_string(),
            worktree_base_ref: None,
        }
    }

    /// **Orca가 사용자 탭을 날린 바로 그 사고**(이슈 #1158): 부팅 중간 단계가
    /// 실패해 상태가 부분적으로만 채워졌는데 저장이 풀려 있으면, 그 반쪽짜리
    /// 상태가 디스크의 멀쩡한 파일을 덮는다.
    #[test]
    fn a_partially_hydrated_state_cannot_overwrite_the_good_file_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("data.json");

        // 사용자의 진짜 상태: repo 둘.
        let mut seed = wired_state(&file);
        seed.hydration = Hydration::opened();
        seed.upsert_repo(some_repo("repo-one"));
        seed.upsert_repo(some_repo("repo-two"));
        seed.persist();
        assert_eq!(
            flush_and_reload(seed, &file).repos.len(),
            2,
            "precondition: the good file has both repos"
        );

        // 부팅이 반만 진행된 상태 — repo 하나만 복원됐고 게이트는 닫혀 있다.
        let mut booting = wired_state(&file);
        booting.hydration = gate_waiting_only_on(RepoId("/tmp/repo-two".into()));
        booting.upsert_repo(some_repo("repo-one"));
        booting.persist();
        // 사용자가 게이트가 닫힌 동안 뭔가를 한다. **거부되지 않는다** —
        // 메모리에 남는다.
        let _ = booting.update(Message::WorktreeSelected(wt("/tmp/edited-while-closed")));

        let on_disk = flush_and_peek(&mut booting, &file);
        assert_eq!(
            on_disk.repos.len(),
            2,
            "the half-hydrated state must NOT have reached disk — this is exactly the \
             data loss the gate exists to prevent"
        );

        // **대조군 둘을 겸한다.** (1) 저장 자체는 멀쩡하게 배선돼 있다 —
        // 위의 "안 써졌다"가 "이 상태는 절대 저장 못 한다"로 설명되면 안 된다.
        // (2) 게이트가 닫힌 동안의 편집은 버려지지 않고 열릴 때 함께 나간다.
        booting.note_hydration(HydrationStep::ReposListed(RepoId("/tmp/repo-two".into())));
        let after = flush_and_reload(booting, &file);
        assert_eq!(
            after.repos.len(),
            1,
            "control: once the gate opens the very same state DOES reach disk, so the \
             block above was the gate and nothing else"
        );
        assert_eq!(
            after.session.active_worktree_id,
            Some(wt("/tmp/edited-while-closed")),
            "control: an edit made while the gate was closed is kept in memory and \
             saved when it opens — the gate defers writes, it does not reject edits"
        );
    }

    /// **저하된 완료도 완료다.** repo 조회가 실패해도 그 repo는 대기에서 빠져야
    /// 한다 — 아니면 게이트가 영원히 닫혀 사용자가 아무것도 저장할 수 없다.
    #[test]
    fn a_failed_repo_listing_still_resolves_its_hydration_step() {
        let repo_id = RepoId("/tmp/degraded-repo".into());
        let mut state = AppState::default();
        state.hydration = gate_waiting_only_on(repo_id.clone());
        state.note_list_issued(repo_id.clone(), OpId(7));
        assert!(!state.hydration.is_open(), "precondition: waiting on the repo");

        let _ = state.update(Message::WorktreesListed {
            request: OpId(7),
            repo_id: repo_id.clone(),
            result: WorktreeListing::Degraded("git exploded".to_string()),
        });

        assert!(
            state.hydration.is_open(),
            "a repo whose listing FAILED must still leave the pending set — a gate that \
             waits forever means the user can never save anything again"
        );
        assert_eq!(
            state.last_error(),
            Some("git exploded"),
            "control: the failure is still reported to the user, not silently swallowed"
        );
    }

    /// 낡은 응답이 카운터를 깎으면 아직 진행 중인 조회가 있는데도 게이트가
    /// 일찍 열린다 — 그러면 부분 복원 상태가 저장될 수 있다.
    #[test]
    fn a_stale_listing_response_does_not_resolve_the_hydration_step() {
        let repo_id = RepoId("/tmp/racy-repo".into());
        let mut state = AppState::default();
        state.hydration = gate_waiting_only_on(repo_id.clone());
        // 조회를 두 번 냈다. 지금 유효한 것은 OpId(2)다.
        state.note_list_issued(repo_id.clone(), OpId(1));
        state.note_list_issued(repo_id.clone(), OpId(2));

        let _ = state.update(Message::WorktreesListed {
            request: OpId(1),
            repo_id: repo_id.clone(),
            result: WorktreeListing::Authoritative(vec![entry_at("/tmp/stale", "stale")]),
        });
        assert!(
            !state.hydration.is_open(),
            "a response to a superseded request must not resolve the step — the current \
             request is still in flight"
        );

        // 대조군: 현재 요청의 응답은 연다.
        let _ = state.update(Message::WorktreesListed {
            request: OpId(2),
            repo_id,
            result: WorktreeListing::Authoritative(vec![entry_at("/tmp/fresh", "fresh")]),
        });
        assert!(
            state.hydration.is_open(),
            "control: the response to the CURRENT request does resolve it"
        );
    }

    // ---- 삭제 판정에 증거를 요구한다 ----

    /// **실패한 스캔 한 번이 복원된 레이아웃 전체를 지우면 안 된다.**
    #[test]
    fn a_degraded_listing_never_closes_a_session_or_removes_a_pane() {
        let (mut state, id, worktree_id, _pane) = state_with_one_open_session();
        let repo_id = RepoId("/tmp/r2".into());

        state.note_list_issued(repo_id.clone(), OpId(2));
        let _ = state.update(Message::WorktreesListed {
            request: OpId(2),
            repo_id: repo_id.clone(),
            result: WorktreeListing::Degraded("fatal: not a git repository".to_string()),
        });

        assert!(
            state.session_store().is_running(id),
            "a degraded listing is not evidence of deletion — the session must survive"
        );
        assert!(
            state.panes().is_some(),
            "and its pane with it; one failed scan must not wipe the restored layout"
        );
        assert!(state.worktree_sessions.contains_key(&worktree_id));
        assert_eq!(
            state.worktree_names(&repo_id),
            vec!["accepted"],
            "the last known good listing must also survive — a degraded scan replaces \
             nothing"
        );

        // **대조군**: 권위 있는 빈 목록은 실제로 지운다. 이게 없으면 위의 단언들이
        // "정리가 아예 배선되지 않았다"로도 설명된다.
        state.note_list_issued(repo_id.clone(), OpId(3));
        let _ = state.update(Message::WorktreesListed {
            request: OpId(3),
            repo_id,
            result: WorktreeListing::Authoritative(Vec::new()),
        });
        assert!(
            !state.session_store().is_running(id),
            "control: an AUTHORITATIVE empty listing IS evidence, and must reap the session"
        );
        assert!(
            state.panes().is_none(),
            "control: and take its pane with it"
        );
    }

    /// 위 테스트는 `update` 경로를 본다. 이건 **함수 자체의 계약**을 본다 —
    /// 정리 여부를 호출부가 판단하면 다른 호출자가 생겼을 때 조용히 무너진다.
    #[test]
    fn apply_worktree_listing_ignores_a_degraded_listing_outright() {
        let mut state = AppState::default();
        let repo_id = RepoId("/tmp/evidence".into());
        state.note_list_issued(repo_id.clone(), OpId(1));
        state.apply_authoritative_listing(
            repo_id.clone(),
            OpId(1),
            vec![entry_at("/tmp/wt-live", "live")],
        );

        state.note_list_issued(repo_id.clone(), OpId(2));
        state.apply_worktree_listing(
            repo_id.clone(),
            OpId(2),
            WorktreeListing::Degraded("git blew up".to_string()),
        );
        assert_eq!(
            state.worktree_names(&repo_id),
            vec!["live"],
            "a degraded listing must not replace the last known good one"
        );

        // 대조군: 권위 있는 빈 목록은 실제로 갈아치운다.
        state.note_list_issued(repo_id.clone(), OpId(3));
        state.apply_worktree_listing(
            repo_id.clone(),
            OpId(3),
            WorktreeListing::Authoritative(Vec::new()),
        );
        assert!(
            state.worktree_names(&repo_id).is_empty(),
            "control: an authoritative empty listing IS evidence and does replace it"
        );
    }

    // ---- 저장 트리거와 디바운스 ----

    /// 리사이즈는 드래그 한 번에 수십 번 온다. **최신 세대만 저장한다.**
    #[test]
    fn only_the_newest_debounce_generation_actually_saves() {
        let dir = tempfile::tempdir().unwrap();

        // 낡은 세대의 타이머가 터진다 → 저장하지 않는다.
        let stale_file = dir.path().join("stale.json");
        let mut state = wired_state(&stale_file);
        state.upsert_repo(some_repo("resized"));
        let _ = state.schedule_layout_save(); // generation 1
        let _ = state.schedule_layout_save(); // generation 2 — 드래그가 계속됐다
        let _ = state.update(Message::LayoutPersistDue { generation: 1 });
        assert_eq!(
            flush_and_reload(state, &stale_file).repos.len(),
            0,
            "a timer from before the latest resize must not save — otherwise every frame \
             of a drag writes to disk"
        );

        // 대조군: 최신 세대의 타이머는 저장한다.
        let fresh_file = dir.path().join("fresh.json");
        let mut state = wired_state(&fresh_file);
        state.upsert_repo(some_repo("resized"));
        let _ = state.schedule_layout_save();
        let _ = state.schedule_layout_save();
        let _ = state.update(Message::LayoutPersistDue { generation: 2 });
        assert_eq!(
            flush_and_reload(state, &fresh_file).repos.len(),
            1,
            "control: the newest generation's timer DOES save — the drag has stopped"
        );
    }

    /// 리사이즈가 **곧바로** 저장하지 않는다는 것. 디바운스가 없으면 드래그
    /// 한 번에 디스크 쓰기가 수십 번 난다.
    #[test]
    fn a_resize_defers_its_save_instead_of_writing_immediately() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("data.json");
        // 분할이 존재하려면 pane이 둘이어야 한다.
        let (mut state, _id, _wt, _pane) = state_with_two_open_sessions_wired(&file);
        state.upsert_repo(some_repo("resized"));

        let split = *state
            .panes()
            .expect("two sessions are open")
            .layout()
            .splits()
            .next()
            .expect("two panes means one split");
        let _ = state.update(Message::PaneResized(pane_grid::ResizeEvent {
            split,
            ratio: 0.3,
        }));

        assert_eq!(
            state.layout_generation, 1,
            "the resize must bump the debounce generation — that bump is what invalidates \
             any timer already in flight"
        );
        let on_disk = flush_and_peek(&mut state, &file);
        assert_eq!(
            on_disk.repos.len(),
            0,
            "the resize itself must not have written — the save is deferred to the timer"
        );

        // 대조군: 타이머가 터지면 실제로 써진다.
        let _ = state.update(Message::LayoutPersistDue { generation: 1 });
        assert_eq!(
            flush_and_reload(state, &file).repos.len(),
            1,
            "control: when the debounce fires, the same state does reach disk"
        );
    }

    /// pane을 여는 것은 **곧바로** 저장한다(디바운스 대상이 아니다).
    #[test]
    fn opening_a_pane_persists_the_layout_right_away() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("data.json");
        let mut state = wired_state(&file);
        let repo_id = RepoId("/tmp/r-open".into());
        state.note_list_issued(repo_id.clone(), OpId(1));
        state.apply_authoritative_listing(
            repo_id,
            OpId(1),
            vec![entry_at("/tmp/wt-open", "wt-open")],
        );

        let worktree_id = wt("/tmp/wt-open");
        let id = state.session_store.next_id();
        state.pending_session_starts.insert(worktree_id.clone(), id);
        let _ = state.update(Message::SessionStarted {
            id,
            worktree_id: worktree_id.clone(),
            result: Ok(StartedSession::new(SessionStore::spawn_throwaway_for_test())),
        });

        let saved = flush_and_reload(state, &file);
        assert_eq!(
            saved.session.panes,
            Some(PersistedPane::Leaf(worktree_id)),
            "the pane that just opened must be on disk without waiting for any timer"
        );
    }

    /// pane을 닫는 것도 레이아웃 변경이다. 저장하지 않으면 닫은 pane이 재시작에
    /// 되살아난다.
    #[test]
    fn closing_a_pane_persists_the_shrunken_layout() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("data.json");
        let (mut state, _id_a, _wt_a, pane) = state_with_two_open_sessions_wired(&file);

        state.persist();
        let before = flush_and_peek(&mut state, &file)
            .session
            .panes
            .expect("two panes are saved as a split");
        assert!(
            matches!(before, PersistedPane::Split { .. }),
            "precondition: the saved layout starts as a split, got {before:?}"
        );

        let _ = state.update(Message::PaneCloseRequested(pane));

        assert_eq!(
            flush_and_reload(state, &file).session.panes,
            Some(leaf("/tmp/wt-b")),
            "closing a pane must reach disk immediately — otherwise the pane the user \
             just closed comes back on the next launch"
        );
    }

    /// 드래그로 재배치한 것도 저장돼야 한다.
    #[test]
    fn reordering_panes_by_drag_persists_the_new_arrangement() {
        use iced::widget::pane_grid::{Edge, Region, Target};

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("data.json");
        let (mut state, _id_a, _wt_a, pane_a) = state_with_two_open_sessions_wired(&file);

        state.persist();
        let before = flush_and_peek(&mut state, &file).session.panes;

        let pane_b = *state
            .panes()
            .expect("two panes")
            .panes
            .keys()
            .nth(1)
            .expect("a second pane");
        let _ = state.update(Message::PaneDragged(pane_grid::DragEvent::Dropped {
            pane: pane_a,
            target: Target::Pane(pane_b, Region::Edge(Edge::Right)),
        }));

        let after = flush_and_reload(state, &file).session.panes;
        assert_ne!(
            after, before,
            "dropping a pane in a new position rearranges the tree, and that \
             rearrangement must reach disk — otherwise the drag is silently undone on \
             the next launch"
        );
        assert_eq!(
            after,
            Some(split(
                PersistedAxis::Vertical,
                0.5,
                leaf("/tmp/wt-b"),
                leaf("/tmp/wt-a")
            )),
            "and it must be the arrangement the user actually dropped"
        );
    }

    /// 저장된 레이아웃이 **분할과 비율까지** 왕복하는지. 앞의 pane 하나짜리
    /// 테스트만으로는 분할 직렬화가 통째로 죽어도 통과한다.
    #[test]
    fn a_split_layout_round_trips_through_disk_with_its_ratio() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("data.json");
        let (mut state, _id, _wt, _pane) = state_with_two_open_sessions_wired(&file);

        let split = *state
            .panes()
            .expect("two panes")
            .layout()
            .splits()
            .next()
            .expect("a split exists");
        let _ = state.update(Message::PaneResized(pane_grid::ResizeEvent {
            split,
            ratio: 0.2503,
        }));
        let _ = state.update(Message::LayoutPersistDue { generation: 1 });

        let saved = flush_and_reload(state, &file)
            .session
            .panes
            .expect("a split layout must be persisted");
        assert_eq!(
            saved,
            PersistedPane::Split {
                axis: PersistedAxis::Horizontal,
                ratio: 0.25,
                a: Box::new(leaf("/tmp/wt-a")),
                b: Box::new(leaf("/tmp/wt-b")),
            },
            "the split, its axis, its children AND the quantized ratio must all survive"
        );
    }

    /// `state_with_two_open_sessions`의 영속화 배선 버전.
    fn state_with_two_open_sessions_wired(
        file: &Path,
    ) -> (AppState, SessionId, WorktreeId, pane_grid::Pane) {
        let (mut state, _repo, [(id_a, wt_a), _b]) = state_with_two_open_sessions();
        let boot = crate::persistence_thread::PersistenceHandle::spawn(file.to_path_buf());
        state.persistence = Some(boot.handle);
        let pane = *state
            .panes()
            .expect("two panes")
            .panes
            .keys()
            .next()
            .expect("at least one");
        (state, id_a, wt_a, pane)
    }

    /// `active_worktree_id`는 오랫동안 **쓰기만 하고 아무도 읽지 않았다.**
    #[test]
    fn the_active_worktree_is_read_back_at_boot() {
        let mut disk = PersistedState::default();
        disk.session.active_worktree_id = Some(wt("/tmp/was-active"));
        let state = AppState::from_load(LoadDiagnostics {
            state: disk,
            origin: LoadOrigin::Fresh,
            save_blocked: false,
        });
        assert_eq!(
            state.selected_worktree(),
            Some(&wt("/tmp/was-active")),
            "the field is written on every save; booting must actually read it back"
        );
    }

    /// **디스크 → `from_load` → `begin_layout_restore`의 이음매.** 위의 복원
    /// 테스트들은 `pending_restore_tree`를 직접 세우므로, 저장된 트리를 부팅이
    /// 실제로 **읽어오는지**는 검사하지 못한다 — 그 한 줄이 죽어도 전부 통과한다
    /// (mutation으로 확인했다).
    #[test]
    fn a_saved_layout_is_read_off_disk_and_starts_its_sessions() {
        let mut disk = PersistedState::default();
        disk.repos = vec![some_repo("booted")];
        disk.worktrees = vec![
            Worktree {
                id: wt("/tmp/wt-a"),
                repo_id: RepoId("/tmp/booted".into()),
                path: PathBuf::from("/tmp/wt-a"),
                branch: "a".into(),
                display_name: "a".into(),
                created_with_agent: None,
                created_at_unix_ms: 1,
            },
            Worktree {
                id: wt("/tmp/wt-b"),
                repo_id: RepoId("/tmp/booted".into()),
                path: PathBuf::from("/tmp/wt-b"),
                branch: "b".into(),
                display_name: "b".into(),
                created_with_agent: None,
                created_at_unix_ms: 2,
            },
        ];
        disk.session.panes = Some(split(
            PersistedAxis::Horizontal,
            0.4,
            leaf("/tmp/wt-a"),
            leaf("/tmp/wt-b"),
        ));

        let mut state = AppState::from_load(LoadDiagnostics {
            state: disk,
            origin: LoadOrigin::Fresh,
            save_blocked: false,
        });
        state.hydration = Hydration::new([]);
        let _ = state.begin_layout_restore();

        let mut pending: Vec<WorktreeId> = state
            .restore
            .as_ref()
            .expect("the saved tree must have started a restore")
            .pending
            .keys()
            .cloned()
            .collect();
        pending.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            pending,
            vec![wt("/tmp/wt-a"), wt("/tmp/wt-b")],
            "booting must read the saved tree off disk and start a session for each leaf \
             — if the layout field is never read, there is nothing to restore and the \
             workbench comes up empty every time"
        );

        deliver_start(&mut state, "/tmp/wt-a", true);
        deliver_start(&mut state, "/tmp/wt-b", true);
        assert_eq!(
            restored_shape(&state),
            "(H0.400 wt-a wt-b)",
            "and the tree that comes back must be the one that was on disk"
        );
    }

    /// 복원된 활성 worktree의 pane이 포커스를 받는다.
    #[test]
    fn the_restored_active_worktree_gets_the_focus() {
        let tree = split(
            PersistedAxis::Vertical,
            0.25,
            leaf("/tmp/wt-a"),
            leaf("/tmp/wt-b"),
        );
        let mut state = restoring_state(tree, &["/tmp/wt-a", "/tmp/wt-b"]);
        state.selected_worktree = Some(wt("/tmp/wt-b"));

        deliver_start(&mut state, "/tmp/wt-a", true);
        deliver_start(&mut state, "/tmp/wt-b", true);

        let focused = state
            .focused_session()
            .and_then(|id| state.session_worktrees.get(&id))
            .cloned();
        assert_eq!(
            focused,
            Some(wt("/tmp/wt-b")),
            "focus must land on the worktree that was active when the app closed, not \
             simply on the first pane"
        );
    }

    /// 포커스 변경이 활성 worktree를 따라가는지. 이것이 없으면 플랜의 "포커스
    /// 변경 시 저장" 트리거가 아무것도 바꾸지 않는 공회전이 된다.
    #[test]
    fn focusing_a_pane_makes_its_worktree_the_active_one() {
        let (mut state, _repo, [(_id_a, wt_a), (id_b, wt_b)]) = state_with_two_open_sessions();
        state.selected_worktree = Some(wt_a.clone());

        let pane_b = *state
            .panes()
            .expect("two panes")
            .iter()
            .find(|(_, id)| **id == id_b)
            .map(|(pane, _)| pane)
            .expect("session b has a pane");
        let _ = state.update(Message::PaneClicked(pane_b));

        assert_eq!(
            state.selected_worktree(),
            Some(&wt_b),
            "the focused pane IS the active worktree — otherwise nothing that gets saved \
             ever changes on focus and the save trigger is a no-op"
        );
    }

    // ================= Plan 5 Task 3: 배지 =================

    fn hook(worktree: &str, nonce: u64, name: HookEventName) -> HookEvent {
        HookEvent {
            pane_key: PaneKey(wt(worktree)),
            spawn_nonce: SpawnNonce(nonce),
            claude_session_id: "sid".into(),
            event: name,
            tool_name: None,
            agent_id: None,
            background_tasks_empty: Some(true),
        }
    }

    /// 배지 장부가 있는 상태. 세션까지 띄우지 않고 장부만 세운다 — 배지 규칙은
    /// 세션 객체와 무관하다.
    fn state_with_badge(worktree: &str, nonce: u64) -> AppState {
        let mut state = AppState::default();
        state
            .badges
            .insert(wt(worktree), PaneBadge::new(SpawnNonce(nonce)));
        state
    }

    /// **세션 교체 창을 막는 유일한 방어.** `PaneKey`는 worktree에서 파생돼 세션이
    /// 바뀌어도 같으므로, 옛 프로세스의 늦은 훅(async라 더 늦다)이 새 세션의 배지를
    /// 덮을 수 있다.
    #[test]
    fn a_hook_carrying_a_stale_nonce_is_dropped() {
        let mut state = state_with_badge("/tmp/wt-a", 5);
        // 현 세대의 이벤트가 배지를 waiting으로 만든다.
        let _ = state.update(Message::HookArrived(hook(
            "/tmp/wt-a",
            5,
            HookEventName::PermissionRequest,
        )));
        assert_eq!(
            state.badges[&wt("/tmp/wt-a")].hook.map(|(s, _)| s),
            Some(HookState::Waiting),
            "precondition: the current generation's hook was accepted"
        );

        // 옛 세대(nonce 4)의 늦은 `Stop`이 도착한다.
        let _ = state.update(Message::HookArrived(hook(
            "/tmp/wt-a",
            4,
            HookEventName::Stop,
        )));
        assert_eq!(
            state.badges[&wt("/tmp/wt-a")].hook.map(|(s, _)| s),
            Some(HookState::Waiting),
            "a hook from a previous spawn must not overwrite the live badge — the old \
             process's Stop would otherwise mark the NEW session finished"
        );

        // 대조군: 같은 `Stop`이 현 세대로 오면 반영된다. 이게 없으면 위 단언이
        // "Stop이 아예 처리되지 않는다"로도 설명된다.
        let _ = state.update(Message::HookArrived(hook(
            "/tmp/wt-a",
            5,
            HookEventName::Stop,
        )));
        assert_eq!(
            state.badges[&wt("/tmp/wt-a")].hook.map(|(s, _)| s),
            Some(HookState::Done),
            "control: the same event at the current nonce IS applied"
        );
    }

    #[test]
    fn a_hook_for_a_pane_we_never_spawned_is_dropped() {
        let mut state = state_with_badge("/tmp/wt-a", 1);
        let _ = state.update(Message::HookArrived(hook(
            "/tmp/other",
            1,
            HookEventName::PermissionRequest,
        )));
        assert!(
            state.badges[&wt("/tmp/wt-a")].hook.is_none(),
            "an event for an unknown pane must not touch any other pane's badge"
        );
        assert!(!state.badges.contains_key(&wt("/tmp/other")));
    }

    /// `SessionStart`는 장부를 **지운다**. 옛 세션의 `Working`을 물려받으면 아무
    /// 일도 안 하는 pane이 도는 스피너로 보인다.
    #[test]
    fn session_start_clears_a_stale_hook_state() {
        let mut state = state_with_badge("/tmp/wt-a", 1);
        let _ = state.update(Message::HookArrived(hook(
            "/tmp/wt-a",
            1,
            HookEventName::PreToolUse,
        )));
        assert!(state.badges[&wt("/tmp/wt-a")].hook.is_some(), "precondition");

        let _ = state.update(Message::HookArrived(hook(
            "/tmp/wt-a",
            1,
            HookEventName::SessionStart,
        )));
        assert!(
            state.badges[&wt("/tmp/wt-a")].hook.is_none(),
            "a fresh session starts from Unknown, not from whatever the last one was doing"
        );
    }

    /// 배지가 **`reduce`를 통해 파생되는지** — 장부만 채우고 끝나면 화면에는
    /// 아무것도 안 나타난다.
    #[test]
    fn the_badge_surfaces_through_reduce() {
        let (mut state, id, worktree_id, _pane) = state_with_one_open_session();
        state
            .badges
            .insert(worktree_id.clone(), PaneBadge::new(SpawnNonce(1)));

        // 훅이 없고 presence도 모르면 `Unknown`이다 — `Done`을 합성하지 않는다.
        assert_eq!(state.worktree_badge(&worktree_id), BadgeState::Unknown);

        let _ = state.update(Message::PresenceReady {
            id,
            generation: 1,
            presence: AgentPresence::Agent(suaegi_term::agent::AgentKind::Claude),
        });
        let _ = state.update(Message::HookArrived(HookEvent {
            pane_key: PaneKey(worktree_id.clone()),
            spawn_nonce: SpawnNonce(1),
            claude_session_id: "s".into(),
            event: HookEventName::PermissionRequest,
            tool_name: Some("Bash".into()),
            agent_id: None,
            background_tasks_empty: None,
        }));

        assert_eq!(
            state.worktree_badge(&worktree_id),
            BadgeState::Waiting,
            "a PermissionRequest with the agent present must surface as Waiting — this is \
             the state the whole plan exists to show"
        );
    }

    /// `NoAgent` streak는 폴링이 도는 곳에서 유지돼야 한다. 안 그러면 `reduce`의
    /// 확정 규칙이 영원히 발동하지 않는다.
    #[test]
    fn repeated_no_agent_polls_eventually_confirm_done() {
        let (mut state, id, worktree_id, _pane) = state_with_one_open_session();
        state
            .badges
            .insert(worktree_id.clone(), PaneBadge::new(SpawnNonce(1)));

        for _ in 0..(NO_AGENT_CONFIRMATIONS - 1) {
            let _ = state.update(Message::PresenceReady {
                id,
                generation: 1,
                presence: AgentPresence::NoAgent,
            });
        }
        assert_ne!(
            state.worktree_badge(&worktree_id),
            BadgeState::Done,
            "below the confirmation threshold the badge must not flip — the shell briefly \
             holds the foreground while exec'ing and this would flicker"
        );

        let _ = state.update(Message::PresenceReady {
            id,
            generation: 1,
            presence: AgentPresence::NoAgent,
        });
        assert_eq!(
            state.worktree_badge(&worktree_id),
            BadgeState::Done,
            "control: once confirmed, the badge settles on Done"
        );
    }

    #[test]
    fn seeing_the_agent_again_resets_the_no_agent_streak() {
        let (mut state, id, worktree_id, _pane) = state_with_one_open_session();
        state
            .badges
            .insert(worktree_id.clone(), PaneBadge::new(SpawnNonce(1)));

        for _ in 0..(NO_AGENT_CONFIRMATIONS - 1) {
            let _ = state.update(Message::PresenceReady {
                id,
                generation: 1,
                presence: AgentPresence::NoAgent,
            });
        }
        let _ = state.update(Message::PresenceReady {
            id,
            generation: 1,
            presence: AgentPresence::Agent(suaegi_term::agent::AgentKind::Claude),
        });
        assert_eq!(
            state.badges[&worktree_id].no_agent_streak, 0,
            "seeing the agent must reset the streak, or a few scattered NoAgent polls \
             across an entire session would eventually add up to a false Done"
        );
    }

    /// 스폰마다 **새 nonce**가 발급되는지. 같은 값을 재사용하면 옛 세션의 훅이
    /// 그대로 통과한다.
    #[test]
    fn each_spawn_registers_a_fresh_nonce_for_its_pane() {
        let mut state = AppState::default();
        let repo_id = RepoId("/tmp/r-badge".into());
        state.note_list_issued(repo_id.clone(), OpId(1));
        state.apply_authoritative_listing(
            repo_id,
            OpId(1),
            vec![entry_at("/nonexistent-suaegi-badge-test", "b")],
        );
        let worktree_id = wt("/nonexistent-suaegi-badge-test");

        let _ = state.update(Message::WorktreeSelected(worktree_id.clone()));
        let first = state.badges[&worktree_id].expected;

        // 세션을 닫고 같은 worktree를 다시 연다.
        state.pending_session_starts.remove(&worktree_id);
        let _ = state.update(Message::WorktreeSelected(worktree_id.clone()));
        let second = state.badges[&worktree_id].expected;

        assert!(
            second > first,
            "a re-spawn must get a NEW nonce ({first:?} -> {second:?}) — reusing it lets the \
             previous process's late hooks drive the new session's badge"
        );
    }

    // ---- Task 6: worktree 메타데이터 ----

    /// 자리표시자(`created_at_unix_ms: 0`)를 매 저장마다 합성하면, 앱을 한 번
    /// 열었다 닫는 것만으로 모든 생성 시각이 영구히 사라진다.
    #[test]
    fn worktree_creation_metadata_survives_a_load_and_a_save() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("data.json");

        let mut disk = PersistedState::default();
        disk.repos = vec![some_repo("meta-repo")];
        disk.worktrees = vec![Worktree {
            id: wt("/tmp/wt-meta"),
            repo_id: RepoId("/tmp/meta-repo".into()),
            path: PathBuf::from("/tmp/wt-meta"),
            branch: "meta".into(),
            display_name: "meta".into(),
            created_with_agent: None,
            created_at_unix_ms: 1_700_000_000_000,
        }];

        let mut state = AppState::from_load(LoadDiagnostics {
            state: disk,
            origin: LoadOrigin::Fresh,
            save_blocked: false,
        });
        let boot = crate::persistence_thread::PersistenceHandle::spawn(file.clone());
        state.persistence = Some(boot.handle);
        state.persist();

        let saved = flush_and_reload(state, &file);
        assert_eq!(
            saved.worktrees.len(),
            1,
            "precondition: the worktree is still listed"
        );
        assert_eq!(
            saved.worktrees[0].created_at_unix_ms, 1_700_000_000_000,
            "the creation timestamp read from disk must be written back, not replaced by \
             the placeholder 0 that the old snapshot synthesized every single save"
        );
    }

    /// 생성 시점이 메타데이터의 유일한 진짜 출처다 — `Ok(_created)`를 버리면
    /// 그 시각은 영영 없다.
    #[test]
    fn creating_a_worktree_records_a_real_timestamp_but_no_agent() {
        let mut state = AppState::default();
        let repo_id = RepoId("/tmp/creator".into());
        state.upsert_repo(some_repo("creator"));

        let before = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let _ = state.update(Message::WorktreeCreated {
            request: OpId(1),
            repo_id,
            result: Ok(CreatedWorktree {
                path: PathBuf::from("/tmp/wt-new"),
                branch: "new".into(),
                display_name: "new".into(),
            }),
        });

        let meta = state
            .worktree_meta
            .get(&wt("/tmp/wt-new"))
            .expect("creation must record metadata — it is the only truthful source");
        assert!(
            meta.created_at_unix_ms >= before,
            "the timestamp must be the real creation time, not the placeholder 0"
        );
        assert_eq!(
            meta.created_with_agent, None,
            "there is no truthful source for the agent yet (no agent-selection UI), and \
             faking it would bake a wrong value into the file forever"
        );
    }

    #[test]
    fn metadata_for_a_vanished_worktree_is_dropped() {
        let mut state = AppState::default();
        let repo_id = RepoId("/tmp/pruner".into());
        state.note_list_issued(repo_id.clone(), OpId(1));
        state.apply_authoritative_listing(
            repo_id.clone(),
            OpId(1),
            vec![entry_at("/tmp/wt-doomed", "doomed")],
        );
        state.worktree_meta.insert(
            wt("/tmp/wt-doomed"),
            WorktreeMeta {
                created_with_agent: None,
                created_at_unix_ms: 42,
            },
        );

        state.note_list_issued(repo_id.clone(), OpId(2));
        state.apply_authoritative_listing(repo_id, OpId(2), Vec::new());

        assert!(
            !state.worktree_meta.contains_key(&wt("/tmp/wt-doomed")),
            "metadata for a worktree that no longer exists must not accumulate forever"
        );
    }

    #[test]
    fn a_worktree_that_still_appears_in_the_next_listing_keeps_its_session() {
        let (mut state, id, _worktree_id, _pane) = state_with_one_open_session();
        let repo_id = RepoId("/tmp/r2".into());

        state.note_list_issued(repo_id.clone(), OpId(2));
        state.apply_authoritative_listing(
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
