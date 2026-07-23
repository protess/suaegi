use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use futures::StreamExt;
use iced::advanced::clipboard;
use iced::widget::pane_grid;
use suaegi_core::domain::{
    JiraConnectionConfig, PersistedPane, PersistedState, Repo, RepoId, SessionState, Settings,
    Worktree, WorktreeId, SCHEMA_VERSION,
};
use suaegi_git::compare::{CompareOutcome, FileDiff};

use crate::diff_panel::{panel_state_for, patch_state_for, DiffState};
use crate::forge_ui::{GithubFetch, GithubStatus, MergeResultDisplay, PrDetails};
use crate::pr_panel::PrPanelState;
use suaegi_forge::{
    CreateReviewInput, CreationEligibility, MergeMethod, MergeOptions, Review, ReviewLookup,
};
use suaegi_secrets::Secret;
use suaegi_tracker::{
    IssuePage, JiraAuthType, JiraConnection, JiraIssue, JiraPage, JiraViewer, LinearWorkspace,
    LinkedJiraIssue, LinkedLinearIssue, Lookup,
};
use suaegi_git::worktree::{BranchDeletion, CreatedWorktree, RemoveOutcome, WorktreeEntry};
use suaegi_term::agent::{agent_def_by_id, PromptInjection};
use suaegi_term::grid::TerminalSnapshot;
use suaegi_term::input_types::{CopyTargets, WriteOutcome};
use suaegi_term::presence::AgentPresence;

use crate::agent_status::server::HookServer;
use crate::agent_status::contract::{
    hook_outcome, reduce, BadgeInput, BadgeState, HookEvent, HookOutcome, HookState, Hydration,
    HydrationStep, PaneKey, SpawnNonce, LAYOUT_SAVE_DEBOUNCE, RESTORE_WATCHDOG,
};
use crate::layout::{leaves_in_order, to_configuration, to_persisted, without_leaf, LeafOutcome};
use crate::persistence_thread::{
    LoadDiagnostics, LoadOrigin, PersistenceHandle, SaveReport, SaveStatus,
};
use crate::prompt_inject::{GateAction, GateObservation, PromptGate};
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
/// **`created_with_agent`**(6c): 생성 시 사이드바 피커가 고른 에이전트 id.
/// `None`이면 로그인 셸(기본, 오늘의 동작). 이 값이 디스크에 굳어 복원 후에도
/// 세션이 같은 에이전트로 뜬다 — `start_session_for`가 레지스트리로 다시 검증해
/// 표에 없는 값이면 로그인 셸로 안전하게 떨어뜨린다.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorktreeMeta {
    pub created_with_agent: Option<String>,
    pub created_at_unix_ms: u64,
    /// Plan 7a: 이 worktree 브랜치에 연결된 GitHub PR 번호. `created_at_unix_ms`와
    /// **똑같은 이유로** 여기 산다 — `persisted_snapshot`이 매 저장마다 도메인
    /// `Worktree`를 새로 합성하므로, 이 값을 meta에 씨딩해 두지 않으면 앱을 한 번
    /// 열었다 닫는 것만으로 연결된 PR이 영구히 사라진다(위 `created_at_unix_ms` 주석 참고).
    pub linked_github_pr: Option<u64>,
    /// N1 §1.3: 이 worktree에 링크된 Linear 이슈 식별자(예: `ENG-123`) + 워크스페이스 좌표.
    /// **`linked_github_pr`과 똑같은 데이터-손실 계약이다** — `persisted_snapshot`이 매 저장마다
    /// `Worktree`를 새로 합성하므로 여기 씨딩·재주입하지 않으면 한 번 저장에 링크가 사라진다
    /// (forge #14 클래스). 좌표(workspace/url_key)는 딥링크·재연결용이라 식별자와 함께 산다.
    pub linked_linear_issue: Option<String>,
    pub linked_linear_issue_workspace_id: Option<String>,
    pub linked_linear_issue_organization_url_key: Option<String>,
    /// N2 §2: 이 worktree에 링크된 Jira 이슈 키(예: `PROJ-123`) + 사이트. **`linked_linear_issue`와
    /// 똑같은 데이터-손실 계약이다** — `persisted_snapshot`이 매 저장마다 `Worktree`를 새로 합성하므로
    /// 여기 씨딩·재주입하지 않으면 한 번 저장에 링크가 사라진다(forge #14 클래스). 사이트는 딥링크·
    /// 다중-사이트 구분용이라 키와 함께 산다.
    pub linked_jira_issue: Option<String>,
    pub linked_jira_site: Option<String>,
}

/// 사이드바 에이전트 피커의 한 항목. `None` = 로그인 셸(기본, 오늘의 동작).
/// `pick_list`가 요구하는 `ToString + PartialEq + Clone`을 만족한다(id가 `'static`
/// 이라 `Copy`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentChoice(pub Option<&'static str>);

impl AgentChoice {
    /// 피커의 기본이자 항상 존재하는 항목 — 에이전트를 안 고른 상태.
    pub const LOGIN_SHELL: AgentChoice = AgentChoice(None);

    /// 드롭다운에 보일 이름. 로그인 셸은 명시적으로 이름을 준다. 등록된 id는
    /// 표의 `display_name`을, (있을 리 없지만) 미등록 id는 id 자체를 보여준다.
    pub fn label(self) -> &'static str {
        match self.0 {
            None => "Login shell",
            Some(id) => agent_def_by_id(id).map(|d| d.display_name).unwrap_or(id),
        }
    }
}

impl std::fmt::Display for AgentChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// 설치된 에이전트 id 목록 → 피커 옵션. 항상 맨 앞에 로그인 셸(기본)을 둔다.
/// 순수 함수라 설치 감지(PATH 스캔)와 분리해 직접 테스트한다.
pub(crate) fn agent_choices(installed: &[&'static str]) -> Vec<AgentChoice> {
    let mut choices = vec![AgentChoice::LOGIN_SHELL];
    choices.extend(installed.iter().map(|id| AgentChoice(Some(id))));
    choices
}

/// 현재 런타임에서 PATH로 설치가 확인되는 33행의 id들. `boot`이 한 번만 부른다.
fn detect_installed_agents() -> Vec<&'static str> {
    use suaegi_term::agent::{agent_defs, current_runtime, detect_installed, PathProbe};
    let probe = PathProbe;
    let runtime = current_runtime();
    agent_defs()
        .iter()
        .filter(|def| detect_installed(def, &probe, runtime))
        .map(|def| def.id)
        .collect()
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
        /// 생성을 시작할 때 고른 에이전트 id(`None`=로그인 셸). 성공 시
        /// [`WorktreeMeta::created_with_agent`]로 굳어 세션 시작 때 그 에이전트를 띄운다.
        created_with_agent: Option<String>,
        /// 생성 제출 시점의 초기 프롬프트(빈 문자열은 `None`). create op와 함께
        /// 실려 와, 성공 시 `pending_prompts`(메모리, 비영속)에 담겼다가 그
        /// worktree의 **첫** 세션 시작 때 한 번 소비된다.
        initial_prompt: Option<String>,
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
    /// 사이드바 에이전트 피커의 선택 변경. `CreateWorktreeSubmitted`가 이 드래프트를
    /// 읽어 새 worktree의 시작 에이전트로 굳힌다.
    WorktreeAgentSelected {
        repo_id: RepoId,
        choice: AgentChoice,
    },
    /// 사이드바 초기-프롬프트 입력창의 변경. `CreateWorktreeSubmitted`가 이 드래프트를
    /// 스냅샷해 새 worktree의 **일회성** 시작 프롬프트로 실어 보낸다 — 영속화하지
    /// 않는다(복원된 세션은 낡은 프롬프트를 다시 주입하면 안 된다).
    WorktreePromptInputChanged {
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
    /// 복원 워치독의 만료. 결과가 영영 오지 않는 잎이 하이드레이션 게이트를
    /// 영구히 닫아두는 것을 막는다.
    RestoreWatchdog {
        generation: u64,
    },

    // ---- Plan 7a-1: GitHub PR 상태 · 생성 (forge_tasks 경유, gh shell-out) ----
    /// PR 상태 재조회 트리거. worktree 행의 새로고침 버튼이 낸다 — **명시적 수동
    /// 새로고침**이라 캐시가 있어도 다시 조회한다(§3.7: 배경 폴링 없음, 새로고침은 재조회).
    GithubRefreshRequested {
        worktree: WorktreeId,
    },
    /// `forge_tasks::fetch_status`의 완료. **결과는 gh shell-out에서 오므로 UI 스레드
    /// 밖에서 만들어진다.** `op`로 staleness를 거른다(수동 새로고침이 on-activate 조회를
    /// 앞지를 수 있다). `Found`면 `linked_github_pr`을 굳힌다.
    GithubStatusFetched {
        worktree: WorktreeId,
        op: OpId,
        fetch: GithubFetch,
        eligibility: CreationEligibility,
    },
    /// Create-PR 다이얼로그 열기(자격이 있을 때만 어포던스가 뜬다). base 기본값과
    /// 제목 초안을 채운다.
    CreatePrOpened {
        worktree: WorktreeId,
    },
    CreatePrTitleChanged(String),
    CreatePrBodyChanged(String),
    CreatePrBaseChanged(String),
    CreatePrDraftToggled(bool),
    /// 다이얼로그 제출 → gh `pr create`(UI 스레드 밖).
    CreatePrSubmitted,
    CreatePrCancelled,
    /// `forge_tasks::create_pr`의 완료. 성공하면 `linked_github_pr`을 굳히고 상태를
    /// 새로고침한다; 실패는 **분류된 문구**로 다이얼로그에 표시한다(raw stderr 아님).
    CreatePrCreated {
        worktree: WorktreeId,
        op: OpId,
        result: Result<Review, String>,
    },

    // ---- Plan 7b: PR 패널 (머지가능성·리뷰·코멘트 읽기 + 확인 게이트 머지) ----
    /// worktree의 PR 패널을 연다(사이드바의 `PR` 버튼이 낸다). 헤더는 7a 리뷰에서
    /// 씨딩하고, 세부(머지가능성·리뷰·코멘트)를 `forge_tasks`로 조회한다.
    PrPanelOpened {
        worktree: WorktreeId,
    },
    PrPanelClosed,
    /// 패널의 세부를 다시 조회한다(수동 새로고침).
    PrPanelRefreshRequested,
    /// `forge_tasks::fetch_pr_details`의 완료. `op`로 staleness를 거른다.
    PrDetailsFetched {
        worktree: WorktreeId,
        op: OpId,
        details: PrDetails,
    },
    /// Merge 버튼 — **확인 단계를 열 뿐 머지하지 않는다**(§4.6). 머지가능성이
    /// `Mergeable`일 때만 확인 단계가 열린다.
    MergeRequested,
    /// 확인 단계에서 방식(merge/squash/rebase)을 고른다.
    MergeMethodSelected(MergeMethod),
    MergeDeleteBranchToggled(bool),
    /// **파괴적 확정** — 확인 단계가 열려 있을 때만 `merge_pr`을 발급한다(UI 스레드 밖).
    MergeConfirmed,
    MergeCancelled,
    /// `forge_tasks::merge_pr`의 완료. Merged면 7a 상태·패널 세부를 재조회한다;
    /// Rejected/Unavailable은 **구별된** 표시로 남긴다(성공으로 안 읽힌다).
    MergeCompleted {
        worktree: WorktreeId,
        op: OpId,
        display: MergeResultDisplay,
    },

    // ---- N1: Linear 트래커 UI ----
    /// 마스킹된 API 키 입력의 변경. 값은 `LinearState::api_key_input`(평문 버퍼)에만 잠깐 산다.
    LinearApiKeyChanged(String),
    /// 연결 제출 — 입력 버퍼를 `Secret`로 감싸 `test_connection`을 UI 스레드 밖에서 발급한다.
    LinearConnectSubmitted,
    /// `tracker_tasks::connect`의 완료. Found면 워크스페이스를 굳히고 이슈 조회를 잇는다;
    /// Unavailable은 **분류된 문구**를 남긴다(raw 에러/키 아님).
    LinearConnected {
        op: OpId,
        result: Lookup<LinearWorkspace>,
    },
    /// 이슈 목록 수동 새로고침.
    LinearIssuesRefreshRequested,
    /// `tracker_tasks::list_issues`의 완료. raw `Lookup`을 그대로 담고, 표시 매핑은
    /// `tracker_ui::issue_list`가 한다 — **`Unavailable`을 빈 목록으로 접지 않는다**.
    LinearIssuesFetched {
        op: OpId,
        result: Lookup<IssuePage>,
    },
    /// 이슈 행의 "link this worktree" — **선택된** worktree를 이 이슈에 링크한다. 링크는
    /// `WorktreeMeta`에 굳고 즉시 persist된다(저장을 거쳐도 남는다 — forge #14 데이터-손실 가드).
    LinearIssueLinked {
        worktree: WorktreeId,
        issue: suaegi_tracker::Issue,
    },

    // ---- N2: Jira 트래커 UI ----
    /// 연결 폼 입력들의 변경. site/email/토큰은 각기 다른 필드에 산다(토큰만 평문 버퍼, 즉시 소거).
    JiraSiteUrlChanged(String),
    JiraEmailChanged(String),
    /// 마스킹된 토큰 입력의 변경. 값은 `JiraState::token_input`(평문 버퍼)에만 잠깐 산다.
    JiraTokenChanged(String),
    /// Cloud/Server 토글. `JiraAuthType`을 정한다(REST 버전·인증 헤더·바디 포맷이 갈린다).
    JiraCloudToggled(bool),
    /// 연결 제출 — 입력들로 `JiraConnection`을 조립하고 토큰을 `Secret`로 감싸 `test_connection`을
    /// UI 스레드 밖에서 발급한다.
    JiraConnectSubmitted,
    /// `tracker_tasks::jira_connect`의 완료. Found면 계정을 굳히고 이슈 조회를 잇는다; Unavailable은
    /// **분류된 문구**를 남긴다(raw 에러/토큰 아님).
    JiraConnected {
        op: OpId,
        result: Lookup<JiraViewer>,
    },
    /// 이슈 목록 수동 새로고침.
    JiraIssuesRefreshRequested,
    /// `tracker_tasks::jira_list_issues`의 완료. raw `Lookup`을 그대로 담고, 표시 매핑은
    /// `tracker_ui::jira_issue_list`가 한다 — **`Unavailable`을 빈 목록으로 접지 않는다**.
    JiraIssuesFetched {
        op: OpId,
        result: Lookup<JiraPage<JiraIssue>>,
    },
    /// 이슈 행의 "link this worktree" — **선택된** worktree를 이 Jira 이슈에 링크한다. 링크는
    /// `WorktreeMeta`에 굳고 즉시 persist된다(저장을 거쳐도 남는다 — forge #14 데이터-손실 가드).
    JiraIssueLinked {
        worktree: WorktreeId,
        issue: JiraIssue,
    },
}

/// 열려 있는 Create-PR 다이얼로그의 편집 상태. 한 번에 하나만(선택된 worktree에
/// 대해). 필드 초안은 이름 드래프트와 같은 수명 — 제출/취소/성공이 지운다.
#[derive(Debug, Clone)]
pub(crate) struct CreatePrDraft {
    pub worktree: WorktreeId,
    pub title: String,
    pub body: String,
    pub base: String,
    pub draft: bool,
    /// gh `pr create`가 진행 중 — 버튼을 잠그고 중복 제출을 막는다.
    pub submitting: bool,
    /// 마지막 제출 실패의 **분류된** 문구(raw stderr 아님).
    pub error: Option<String>,
}

/// N1(Linear) 연결 + 이슈 목록의 UI 상태. **API 키는 여기서 `Secret`로만 다룬다** —
/// `api_key_input`만 잠깐 평문(text_input이 `&str`을 요구)이고, 커스텀 `Debug`가 그마저
/// 리댁션한다. 토큰/워크스페이스/이슈는 메모리 전용이고 **`persisted_snapshot`에 절대 안 들어간다**
/// (평문 JSON 금지 — 키는 `suaegi-secrets` 키체인으로만 간다).
#[derive(Default)]
pub(crate) struct LinearState {
    /// 연결 다이얼로그의 마스킹된 키 입력 버퍼. 제출 즉시 비운다(평문을 오래 들고 있지 않는다).
    pub api_key_input: String,
    /// 인증된 토큰(연결 성공 또는 부팅 시 키체인/env 로드). 이슈 조회가 이걸 clone해 쓴다.
    pub token: Option<Secret>,
    /// 연결 확인된 워크스페이스(org 이름 표시 + 링크 좌표). 성공한 `test_connection`이 채운다.
    pub workspace: Option<LinearWorkspace>,
    /// 연결 시도 진행 중 — 버튼을 잠그고 중복 제출을 막는다.
    pub connecting: bool,
    /// 마지막 연결 실패의 **분류된** 문구(raw 에러/키 아님).
    pub connect_error: Option<String>,
    /// 마지막 이슈 목록 조회 결과(raw `Lookup`). 표시 매핑은 `tracker_ui::issue_list`가 한다 —
    /// **`Unavailable`을 여기서 빈 목록으로 접지 않는다**(crux는 매핑에서 지킨다).
    pub issues: Option<Lookup<IssuePage>>,
    /// 이슈 조회 진행 중.
    pub issues_loading: bool,
}

/// 커스텀 `Debug`: `api_key_input`을 절대 찍지 않는다(text_input 버퍼가 평문이므로 파생 Debug는
/// 키를 흘린다). `Secret`은 이미 리댁션하지만 입력 버퍼는 타입이 `String`이라 여기서 막는다.
impl std::fmt::Debug for LinearState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinearState")
            .field("api_key_input", &"<redacted>")
            .field("authenticated", &self.token.is_some())
            .field("workspace", &self.workspace)
            .field("connecting", &self.connecting)
            .field("connect_error", &self.connect_error)
            .field("issues_loading", &self.issues_loading)
            .finish()
    }
}

/// N2(Jira) 연결 + 이슈 목록의 UI 상태. Linear보다 **연결 입력이 많다**(Jira는 API 키 하나로
/// 안 되고 site/email/token/Cloud-Server가 필요). **토큰은 여기서 `Secret`로만 다룬다** —
/// `token_input`만 잠깐 평문(text_input이 `&str`을 요구)이고, 커스텀 `Debug`가 그마저 리댁션한다.
/// 토큰/계정/이슈는 메모리 전용이고 **`persisted_snapshot`의 토큰에는 절대 안 들어간다**(토큰은
/// `suaegi-secrets` 키체인으로만). 단, non-secret 연결 설정([`JiraConnection`])은 `Settings`에
/// 굳어 부팅 재연결에 쓴다(토큰 없이 site/email/auth_type만).
pub(crate) struct JiraState {
    /// 사이트 URL 입력 버퍼(예: `https://acme.atlassian.net`). 제출 시 정규화된다.
    pub site_url_input: String,
    /// 로그인 이메일 입력 버퍼(Cloud/Server-Basic). Server PAT면 비워도 된다(→ Bearer).
    pub email_input: String,
    /// 마스킹된 토큰/PAT 입력 버퍼. 제출 즉시 비운다(평문을 오래 들고 있지 않는다).
    pub token_input: String,
    /// Cloud/Server 토글 상태. true=Cloud(`/rest/api/3`, ADF), false=Server(`/rest/api/2`, plain).
    pub is_cloud: bool,
    /// 인증된 토큰(연결 성공 또는 부팅 시 키체인/env 로드). 이슈 조회가 이걸 clone해 쓴다.
    pub token: Option<Secret>,
    /// 활성 연결 설정(site/email/auth_type). 성공한 연결이 채우고 **`Settings`에 굳어** 부팅
    /// 재연결에 쓴다. 이슈 조회가 클라이언트를 재조립할 때도 이걸 clone한다. **토큰은 여기 없다.**
    pub connection: Option<JiraConnection>,
    /// 연결 확인된 계정(`/myself`). 성공한 `test_connection`이 채운다(연결 표시).
    pub viewer: Option<JiraViewer>,
    /// 연결 시도 진행 중 — 버튼을 잠그고 중복 제출을 막는다.
    pub connecting: bool,
    /// 마지막 연결 실패의 **분류된** 문구(raw 에러/토큰 아님).
    pub connect_error: Option<String>,
    /// 마지막 이슈 목록 조회 결과(raw `Lookup`). 표시 매핑(Unavailable≠no issues)은
    /// `tracker_ui::jira_issue_list`가 한다 — 여기서 빈 목록으로 접지 않는다.
    pub issues: Option<Lookup<JiraPage<JiraIssue>>>,
    /// 이슈 조회 진행 중.
    pub issues_loading: bool,
}

/// **기본은 Cloud**(가장 흔한 배포). 나머지는 빈/None(미연결). `Default` 파생 대신 손으로 써서
/// `is_cloud: true`를 못박는다.
impl Default for JiraState {
    fn default() -> Self {
        Self {
            site_url_input: String::new(),
            email_input: String::new(),
            token_input: String::new(),
            is_cloud: true,
            token: None,
            connection: None,
            viewer: None,
            connecting: false,
            connect_error: None,
            issues: None,
            issues_loading: false,
        }
    }
}

/// 커스텀 `Debug`: `token_input`(text_input 평문 버퍼)을 절대 찍지 않는다. `Secret`은 이미
/// 리댁션하지만 입력 버퍼는 타입이 `String`이라 여기서 막는다(`LinearState`와 같은 규율).
impl std::fmt::Debug for JiraState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JiraState")
            .field("site_url_input", &self.site_url_input)
            .field("email_input", &self.email_input)
            .field("token_input", &"<redacted>")
            .field("is_cloud", &self.is_cloud)
            .field("authenticated", &self.token.is_some())
            .field("connection", &self.connection)
            .field("viewer", &self.viewer)
            .field("connecting", &self.connecting)
            .field("connect_error", &self.connect_error)
            .field("issues_loading", &self.issues_loading)
            .finish()
    }
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
    /// repo별 에이전트 피커의 선택. 엔트리가 없으면 로그인 셸(기본) — 피커를
    /// 무시한 사용자는 오늘의 동작을 그대로 받는다. `AgentChoice::LOGIN_SHELL`을
    /// 고르면 엔트리를 지운다(= 없음과 같다).
    worktree_agent_draft: HashMap<RepoId, AgentChoice>,
    /// repo별 "초기 프롬프트" 입력창의 임시 값. 비었으면 주입 없음(기본). 이름
    /// 드래프트와 같은 수명 — `CreateWorktreeSubmitted`가 스냅샷하고 성공 시 지운다.
    worktree_prompt_draft: HashMap<RepoId, String>,
    /// PATH에서 설치가 확인된 에이전트 id(피커에 나열). **부팅 때 한 번** 스캔한다
    /// (`detect_installed`는 에이전트마다 PATH를 훑으므로 프레임마다 하면 비싸다).
    /// 앱 실행 중 새로 설치한 에이전트는 재부팅 전까지 안 보인다 — v1 한계.
    installed_agents: Vec<&'static str>,
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

    // ---- Plan 6b-B: 초기 프롬프트 주입 ----
    /// worktree별 **일회성** 초기 프롬프트. `WorktreeCreated`가 담고, 그 worktree의
    /// **첫** `start_session_for`가 `remove`로 소비한다 — 그래서 세션을 닫았다
    /// 다시 열거나 재시작 후 복원해도 다시 주입되지 않는다. **영속화하지 않는다.**
    pending_prompts: HashMap<WorktreeId, String>,
    /// stdin-after-start 세션의 주입 대기 프롬프트. `start_session_for`가 시작을
    /// **요청**할 때 담고, `SessionStarted`가 세션이 **실제로 살아난** 뒤 게이트로
    /// 옮긴다(`prompt_gates`). 시작이 실패/거절되면 여기서 지워 새지 않게 한다.
    /// 왜 게이트를 바로 안 무장하고 여기 잠깐 두는가: 세션 슬롯은 `accept_started`
    /// 전엔 없으므로, 게이트를 그 전에 무장하면 poll이 세션을 못 찾는다.
    pending_injections: HashMap<SessionId, String>,
    /// 무장된 주입 게이트. stdin-after-start 세션이 composer 준비(BRACKETED_PASTE +
    /// 조용한 창)에 이르면 프롬프트를 한 번 써넣고 사라진다. `PresenceTick`이 폴링한다.
    prompt_gates: HashMap<SessionId, PromptGate>,

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
    /// **디스크에서 읽은 원본 레이아웃. 저하된 복원이 이것을 덮지 못하게 한다.**
    ///
    /// 세션 시작 실패는 **일시적**이다(PTY 스폰 실패, 부팅 중 자원 경합). 그런데
    /// 실패한 잎은 살아 있는 트리에서 접혀 사라지고, 게이트가 열리는 순간 그
    /// 접힌 모양이 디스크에 저장된다 — **한 번의 스폰 실패가 멀쩡한 pane을
    /// 영구히 지운다.** 분할의 양쪽이 다 실패하면 저장된 레이아웃 전체가 빈
    /// 값으로 덮인다.
    ///
    /// 그래서 `Some`인 동안 `persisted_snapshot`은 **살아 있는 트리가 아니라
    /// 이것을** 쓴다. 두 가지만 이 보존을 끝낸다:
    /// - **사용자가 레이아웃을 편집하면** `None`이 된다(그때부터 화면이 진실이다).
    /// - **권위 있는 목록이 worktree의 소멸을 확인하면** 그 잎만 지운다.
    ///
    /// 게이트를 저하된 완료에서도 여는 것은 옳지만, **저하된 재구성을 저장하는
    /// 것은 옳지 않다** — 이 필드가 그 둘을 가른다.
    preserved_layout: Option<PersistedPane>,
    // ---- Plan 5 Task 3: 에이전트 상태 배지 ----
    /// worktree(= `PaneKey`)별 배지 장부. `reduce`의 입력 중 훅에서 오는 절반을
    /// 여기 모으고, 나머지 절반(presence)은 `session_store`에서 읽는다.
    badges: HashMap<WorktreeId, PaneBadge>,
    /// 훅 서버의 포트·토큰. `None`이면 서버가 안 떴다는 뜻이고, 그때는 **배지 없이
    /// 계속 간다** — 훅은 편의 기능이지 세션의 전제가 아니다.
    hook_endpoint: Option<(u16, String)>,
    /// 서버 핸들. **앱이 소유해야 한다** — 떨구면 포트가 닫히고, 버린 이벤트
    /// 카운터를 읽을 곳도 여기뿐이다.
    hook_server: Option<HookServer>,
    /// 마지막으로 반영한 `dropped()` 값. 늘어났으면 배지를 무효화한다.
    hook_drops_seen: u64,
    /// 훅 스크립트의 설치 경로. 부팅 시 한 번 설치하고 worktree 설정이 이걸 가리킨다.
    hook_script: Option<PathBuf>,
    /// 복원 시도의 세대. 워치독 대조에 쓴다.
    restore_generation: u64,
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

    // ---- Plan 7a-1: GitHub PR 상태 · 생성 ----
    /// worktree별 마지막 PR 상태 조회 결과 캐시. **엔트리 없음 = 아직 조회 안 함**
    /// (표시자를 숨긴다). 활성화 시 1회 + 수동 새로고침으로만 채워지고, `PresenceTick`은
    /// 절대 건드리지 않는다(§3.7: 배경 폴링 없음).
    github_status: HashMap<WorktreeId, GithubStatus>,
    /// worktree별 마지막에 발급한 PR 조회의 OpId. 그보다 오래된 응답은 버린다 —
    /// 수동 새로고침이 on-activate 조회를 앞질러도 낡은 결과가 새 것을 덮지 않게 한다
    /// (`latest_list_op`와 같은 규율).
    latest_forge_op: HashMap<WorktreeId, OpId>,
    /// 열려 있는 Create-PR 다이얼로그(없으면 닫힘). 한 번에 하나.
    create_pr: Option<CreatePrDraft>,

    // ---- N1: Linear 트래커 UI ----
    /// Linear 연결 + 이슈 목록 상태. API 키는 여기서 `Secret`로만 다루고 키체인으로만 저장된다.
    linear: LinearState,
    /// 마지막에 발급한 Linear 네트워크 op(연결·이슈 조회 공용). 그보다 오래된 응답은 버린다 —
    /// 재연결이 진행 중인 이슈 조회를 앞질러도 낡은 결과가 새 것을 덮지 않게 한다.
    latest_linear_op: Option<OpId>,

    // ---- N2: Jira 트래커 UI ----
    /// Jira 연결 + 이슈 목록 상태. 토큰은 여기서 `Secret`로만 다루고 키체인(account=site)으로만
    /// 저장된다. non-secret 연결 설정은 `Settings`에 굳어 부팅 재연결에 쓴다.
    jira: JiraState,
    /// 마지막에 발급한 Jira 네트워크 op(연결·이슈 조회 공용). Linear의 `latest_linear_op`와 같은
    /// 규율 — 낡은 응답이 새 것을 덮지 않게 한다.
    latest_jira_op: Option<OpId>,

    // ---- Plan 7b: PR 패널 ----
    /// 열려 있는 PR 패널 상태(닫혀 있으면 `worktree`가 `None`). diff 패널과 같이
    /// 필드 하나로 든다 — 머지가능성·리뷰·코멘트 + 확인 게이트 머지가 여기 산다.
    pr_panel: PrPanelState,
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
    /// **이 pane이 훅을 하나라도 받았는가.** 로그인 셸(`AgentKind::Custom`) 안에서
    /// 사용자가 claude를 띄우면, 그 세션은 `StatusSource::OscTitle`이면서 **동시에**
    /// 훅을 낸다 — 훅과 OSC-title이 같은 `hook` 슬롯을 공유하므로 `hook.is_some()`
    /// 만으로는 출처를 못 가른다. 훅을 한 번이라도 본 pane은 훅이 권위이고, 그 뒤로는
    /// 타이틀이 배지를 건드리지 못하게 한다(안 그러면 claude의 `✳ …` idle 타이틀이
    /// permission 대기 `Waiting`을 `Done`으로 덮어 MVP가 검증한 주황 배지를 회귀시킨다).
    /// **스폰마다 [`PaneBadge::new`]로 리셋되므로 세션 교체를 넘어 새지 않는다.**
    received_hook: bool,
    /// `NoAgent` streak가 확정되기 전에 유지할 값.
    previous: BadgeState,
    no_agent_streak: u8,
}

impl PaneBadge {
    fn new(expected: SpawnNonce) -> Self {
        Self {
            expected,
            hook: None,
            received_hook: false,
            previous: BadgeState::Unknown,
            no_agent_streak: 0,
        }
    }
}

/// 진행 중인 복원의 장부. 잎마다 종단 결과가 하나씩 모이고, `pending`이 비면
/// 트리를 짓는다.
struct LayoutRestore {
    /// 이 복원 시도의 세대. 워치독이 **자기가 걸린 그 복원**만 끝내도록 한다 —
    /// 늦게 도착한 워치독이 그 사이 시작된 새 복원을 잘라버리면 안 된다.
    generation: u64,
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
            // 기본은 빈 목록 = 피커에 로그인 셸만. 실제 앱 경로(`boot`)만 PATH를
            // 스캔한다 — 손으로 세우는 테스트 상태는 스캔 비용을 치르지 않는다.
            worktree_agent_draft: HashMap::new(),
            worktree_prompt_draft: HashMap::new(),
            installed_agents: Vec::new(),
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
            pending_prompts: HashMap::new(),
            pending_injections: HashMap::new(),
            prompt_gates: HashMap::new(),
            next_presence_seq: 0,
            last_input_loss: None,
            // 부팅을 거치지 않는 경로는 하이드레이션할 것이 없다 — 열어둔다.
            badges: HashMap::new(),
            // 서버 없이도 앱은 완전히 동작한다 — 배지만 `Unknown`에 머문다.
            hook_endpoint: None,
            hook_server: None,
            hook_drops_seen: 0,
            hook_script: None,
            preserved_layout: None,
            restore_generation: 0,
            hydration: Hydration::opened(),
            pending_restore_tree: None,
            restore: None,
            worktree_meta: HashMap::new(),
            layout_generation: 0,
            diff: DiffState::default(),
            github_status: HashMap::new(),
            latest_forge_op: HashMap::new(),
            create_pr: None,
            linear: LinearState::default(),
            latest_linear_op: None,
            jira: JiraState::default(),
            latest_jira_op: None,
            pr_panel: PrPanelState::default(),
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
    /// `hook_server`는 **`begin_layout_restore()`보다 먼저** 자리를 잡아야 한다.
    /// 복원이 시작하는 세션도 `start_session_for`를 지나며 그 자리에서 `SUAEGI_*`
    /// env를 심기 때문이다 — 나중에 붙이면 재시작 직후의 모든 pane이 훅 없이
    /// 떠서 배지가 영원히 `Unknown`이다. 그래서 `run()`이 붙여주는 것이 아니라
    /// **인자로 받는다**: 순서를 호출부의 규율이 아니라 타입으로 강제한다.
    pub fn boot(hook_server: Option<HookServer>) -> (AppState, iced::Task<Message>) {
        let boot = PersistenceHandle::spawn(crate::persistence_thread::default_data_file());
        let mut state = AppState::from_load(boot.load);
        state.persistence = Some(boot.handle);
        // **여기서 딱 한 번** 설치된 에이전트를 감지한다(위 필드 주석 참고).
        state.installed_agents = detect_installed_agents();
        state.install_hooks();
        state.attach_hook_server(hook_server);

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

        // N1: 저장된 Linear 키(키체인 우선, env fallback)가 있으면 메모리 토큰으로 올리고
        // 재연결(verify + 워크스페이스/이슈 조회)을 발급한다 — "연결"이 앱 재시작을 넘어
        // 지속되게 한다. 키가 없으면 조용히 미연결로 시작한다(키는 절대 로그/JSON에 안 남는다).
        let resolved = suaegi_secrets::load(&crate::tracker_tasks::secret_request());
        if let Some(token) = resolved.secret {
            state.linear.token = Some(token.clone());
            state.linear.connecting = true;
            let op = state.next_op();
            state.latest_linear_op = Some(op);
            tasks.push(crate::tracker_tasks::connect(op, token));
        }

        // N2: 저장된 Jira 연결(`from_load`가 settings에서 올린 것)이 있으면 키체인 토큰
        // (account=site)을 짚어 재연결(verify + 계정/이슈 조회)을 발급한다. 토큰이 없으면 연결을
        // 시도하지 않는다 — 연결 설정은 그대로 두어(폼이 미리 채워진 채) 다음 저장에 보존되고,
        // 사용자가 토큰만 다시 넣으면 재연결된다. 토큰은 절대 로그/JSON에 안 남는다.
        if let Some(connection) = state.jira.connection.clone() {
            let resolved =
                suaegi_secrets::load(&crate::tracker_tasks::jira_secret_request(&connection.site_url));
            if let Some(token) = resolved.secret {
                state.jira.token = Some(token.clone());
                state.jira.connecting = true;
                let op = state.next_op();
                state.latest_jira_op = Some(op);
                tasks.push(crate::tracker_tasks::jira_connect(op, connection, token));
            }
        }

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
        // N2 §2: 저장된 Jira 연결 설정(non-secret)을 메모리로 올린다. 부팅(`boot`)이 이걸로 키체인
        // 토큰을 짚어 재연결하고, `persisted_snapshot`이 다시 `Settings`로 굳혀 왕복이 닫힌다.
        // 연결 폼 입력도 미리 채워, 키체인 토큰이 없을 때 사용자가 site/email을 다시 안 쳐도 되게 한다
        // (토큰 입력은 절대 채우지 않는다 — 토큰은 디스크에 없다).
        if let Some(cfg) = load.state.settings.jira_connection {
            state.jira.site_url_input = cfg.site_url.clone();
            state.jira.email_input = cfg.email.clone();
            state.jira.is_cloud = cfg.is_cloud;
            state.jira.connection = Some(JiraConnection {
                site_url: cfg.site_url,
                email: cfg.email,
                auth_type: if cfg.is_cloud {
                    JiraAuthType::Cloud
                } else {
                    JiraAuthType::Server
                },
            });
        }
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
                    linked_github_pr: worktree.linked_github_pr,
                    // N1 §1.3: Linear 링크도 PR과 같은 경로로 씨딩한다 — 안 하면 다음 저장이
                    // 자리표시자(None)로 덮어써 링크가 영구히 사라진다(forge #14 데이터-손실 클래스).
                    linked_linear_issue: worktree.linked_linear_issue,
                    linked_linear_issue_workspace_id: worktree.linked_linear_issue_workspace_id,
                    linked_linear_issue_organization_url_key: worktree
                        .linked_linear_issue_organization_url_key,
                    // N2 §2: Jira 링크도 **정확히 같은 데이터-손실 계약**으로 씨딩한다 — 안 하면
                    // 다음 저장이 None으로 덮어써 링크가 사라진다(forge #14). persisted_snapshot의
                    // 재주입과 짝을 이룬다.
                    linked_jira_issue: worktree.linked_jira_issue,
                    linked_jira_site: worktree.linked_jira_site,
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
                        linked_github_pr: meta.linked_github_pr,
                        // N1 §1.3: Linear 링크를 meta에서 **재주입**한다. `linked_github_pr`과
                        // 똑같이 — 여기서 None으로 합성하면 한 번 저장에 링크가 사라진다.
                        linked_linear_issue: meta.linked_linear_issue.clone(),
                        linked_linear_issue_workspace_id: meta
                            .linked_linear_issue_workspace_id
                            .clone(),
                        linked_linear_issue_organization_url_key: meta
                            .linked_linear_issue_organization_url_key
                            .clone(),
                        // N2 §2: Jira 링크도 meta에서 **재주입**한다 — from_load 씨딩과 짝을 이룬다.
                        // 여기서 None으로 합성하면 한 번 저장에 링크가 사라진다(forge #14).
                        linked_jira_issue: meta.linked_jira_issue.clone(),
                        linked_jira_site: meta.linked_jira_site.clone(),
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
                // N2 §2: 활성 Jira 연결(있으면)을 non-secret 설정으로 굳힌다 — 부팅 재연결의 근거.
                // **토큰은 절대 여기 없다**(키체인 전용). auth_type을 is_cloud로 평평하게 매핑한다.
                jira_connection: self.jira.connection.as_ref().map(|c| JiraConnectionConfig {
                    site_url: c.site_url.clone(),
                    email: c.email.clone(),
                    is_cloud: c.auth_type.is_cloud(),
                }),
            },
        }
    }

    /// 지금 화면의 pane 트리를 저장 가능한 모양으로. 세션이 하나도 없으면
    /// `None`이다.
    fn persisted_layout(&self) -> Option<PersistedPane> {
        // 복원이 저하됐고 사용자가 아직 레이아웃을 건드리지 않았다면, 디스크에
        // 남길 진실은 화면이 아니라 **원본**이다(위 `preserved_layout` 참고).
        if let Some(preserved) = &self.preserved_layout {
            return Some(preserved.clone());
        }
        let panes = self.panes.as_ref()?;
        to_persisted(panes.layout(), panes, &self.session_worktrees)
    }

    /// 사용자가 레이아웃을 바꿨다 — 이제부터 화면이 진실이다.
    ///
    /// **권위 있는 소멸 정리에서는 부르지 않는다.** 그건 사용자의 편집이 아니라
    /// 외부 사실이고, 그 경우엔 보존된 트리에서 해당 잎만 지운다.
    fn note_user_layout_edit(&mut self) {
        self.preserved_layout = None;
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

        self.restore_generation += 1;
        let generation = self.restore_generation;
        let mut restore = LayoutRestore {
            generation,
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
        // **결과가 영영 오지 않는 잎에 대한 유일한 방어.**
        // `SessionStore::start`는 워커 스레드에서 `try_send`로 결과를 보내는데,
        // `TerminalSession::start`가 **패닉하면** 그 스레드가 죽고 메시지는
        // 영영 오지 않는다. 그러면 그 잎이 `pending`에 남아 `finish_restore`가
        // 돌지 않고, 하이드레이션 게이트가 **프로세스가 끝날 때까지** 닫힌 채라
        // 사용자의 모든 조작이 저장되지 않는다 — 조용히.
        //
        // "저하된 완료도 완료다"를 **도착하지 않는 결과까지** 확장한 것이다.
        tasks.push(iced::Task::future(async move {
            tokio::time::sleep(RESTORE_WATCHDOG).await;
            Message::RestoreWatchdog { generation }
        }));
        iced::Task::batch(tasks)
    }

    /// 워치독 만료. 아직 그 세대의 복원이 진행 중이면 남은 잎을 전부 실패로
    /// 확정하고 트리를 짓는다.
    fn expire_restore(&mut self, generation: u64) {
        let stale = self
            .restore
            .as_ref()
            .is_none_or(|restore| restore.generation != generation);
        if stale {
            // 이미 정상적으로 끝났거나 다른 세대의 복원이 돌고 있다.
            return;
        }
        let restore = self.restore.take();
        if let Some(mut restore) = restore {
            let stranded: Vec<WorktreeId> = restore.pending.keys().cloned().collect();
            for worktree_id in stranded {
                restore.pending.remove(&worktree_id);
                restore.outcomes.insert(worktree_id, LeafOutcome::Failed);
            }
            eprintln!(
                "suaegi: layout restore timed out; {} leaf/leaves never reported",
                restore.outcomes.len()
            );
            self.finish_restore(Some(restore));
        }
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
            // **원본을 보존한다.** 아래에서 짓는 트리는 실패한 잎이 접힌 저하된
            // 모양이고, 게이트가 곧 열리며 저장이 풀린다 — 보존하지 않으면 그
            // 저하된 모양이 디스크의 멀쩡한 레이아웃을 덮는다.
            self.preserved_layout = Some(restore.tree.clone());
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
            linked_github_pr: created.linked_github_pr,
            // Linear 링크(N1 §1.3)는 아직 이 경로로 배선되지 않는다(§4 후속) → None.
            linked_linear_issue: None,
            linked_linear_issue_workspace_id: None,
            linked_linear_issue_organization_url_key: None,
            // Jira 링크(N2 §2)도 마찬가지 → None(이슈 목록의 "link this worktree"가 나중에 굳힌다).
            linked_jira_issue: None,
            linked_jira_site: None,
        };
        // **세션을 띄울 때마다 주입한다.** 생성 시점에만 쓰면 이 기능보다 **먼저
        // 만들어진 worktree**는 설정 파일을 영영 못 받고, 파일이 지워진 경우도
        // 복구되지 않는다 — 그러면 그 pane의 배지는 영구히 `Unknown`이다.
        // 멱등하고 파일 하나짜리 쓰기이므로 매번 해도 싸다.
        self.inject_into_worktree(&entry.path);

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

        // worktree 생성 시 고른 에이전트를 그대로 띄운다(6c). 디스크에 굳은
        // `created_with_agent` 문자열을 **레지스트리로 다시 검증**해 `&'static str`
        // id로 만든다 — 표에 없는 값(제거된 에이전트, 손상된 저장본)은 `None`으로
        // 떨어져 로그인 셸이 된다(오늘의 기본과 동일). 고른 게 없으면 `None` →
        // 로그인 셸이라 기존 동작이 그대로다.
        let agent_def = worktree
            .created_with_agent
            .as_deref()
            .and_then(agent_def_by_id);
        let agent_id: Option<&'static str> = agent_def.map(|def| def.id);

        // **일회성 초기 프롬프트를 여기서 소비한다.** `pending_prompts`에서 빼므로
        // 세션을 닫았다 다시 열거나 재시작 후 복원해도 다시 주입되지 않는다 —
        // 이 값은 영속화되지 않고 오직 이 첫 시작에만 실린다.
        let prompt = self.pending_prompts.remove(id);

        // argv/flag 에이전트는 프롬프트가 스폰 시점에 argv로 들어간다
        // (`build_spawn_by_id` → `apply_prompt_injection`). stdin-after-start
        // 에이전트는 argv로는 no-op이라, 세션이 살아난 뒤 게이트로 PTY에 써넣어야
        // 한다 — 그 프롬프트를 세션이 실제로 시작될 때까지 여기 잠깐 둔다.
        if let (Some(prompt), Some(PromptInjection::StdinAfterStart)) =
            (prompt.as_ref(), agent_def.map(|def| def.prompt_injection))
        {
            self.pending_injections.insert(session_id, prompt.clone());
        }

        let task =
            self.session_store
                .start(session_id, &worktree, agent_id, prompt, env);
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
            // 세션이 없던 worktree라면 `close_session`이 지나가지 않으므로
            // 여기서도 지운다 — 안 그러면 맵이 앱 수명 내내 자란다.
            self.badges.remove(id);
            // **소멸이 확인된 잎만** 보존된 레이아웃에서 지운다. 이것이 저장된
            // 트리에서 잎을 자동으로 없앨 수 있는 **유일한** 경로다 — 시작
            // 실패는 증거가 아니다.
            if let Some(preserved) = &self.preserved_layout {
                self.preserved_layout = without_leaf(preserved, id);
            }
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

    /// 이 repo의 에이전트 피커에 나열할 옵션(로그인 셸 + 설치된 에이전트들).
    pub(crate) fn agent_picker_choices(&self) -> Vec<AgentChoice> {
        agent_choices(&self.installed_agents)
    }

    /// 이 repo의 현재 피커 선택. 엔트리가 없으면 로그인 셸(기본).
    pub(crate) fn worktree_agent_selection(&self, repo: &RepoId) -> AgentChoice {
        self.worktree_agent_draft
            .get(repo)
            .copied()
            .unwrap_or(AgentChoice::LOGIN_SHELL)
    }

    /// 이 repo의 초기-프롬프트 입력창 값. 비었으면 빈 문자열(주입 없음).
    pub(crate) fn worktree_prompt_draft(&self, repo: &RepoId) -> &str {
        self.worktree_prompt_draft
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

    // ---- Plan 7a-1: 사이드바가 읽는 PR 상태/다이얼로그 접근자 ----

    /// worktree 하나의 마지막 PR 상태 캐시(`None` = 아직 조회 안 함). 사이드바가
    /// `forge_ui::indicator_for`/`create_pr_affordance`에 넘겨 표시자·어포던스를 파생한다.
    pub(crate) fn github_status_for(&self, worktree_id: &WorktreeId) -> Option<&GithubStatus> {
        self.github_status.get(worktree_id)
    }

    /// 열려 있는 Create-PR 다이얼로그(없으면 `None`). 사이드바가 폼을 그릴지 판단한다.
    pub(crate) fn create_pr_dialog(&self) -> Option<&CreatePrDraft> {
        self.create_pr.as_ref()
    }

    /// N1: Linear 연결/이슈 상태. 사이드바가 연결 폼·이슈 목록을 그릴 때 읽는다. 표시 매핑
    /// (Unavailable≠no issues)은 `tracker_ui`가 한다 — 여기선 raw 상태만 노출한다.
    pub(crate) fn linear(&self) -> &LinearState {
        &self.linear
    }

    /// N2: Jira 연결/이슈 상태. 사이드바가 연결 폼(site/email/token/Cloud-Server)·이슈 목록을
    /// 그릴 때 읽는다. 표시 매핑(Unavailable≠no issues)은 `tracker_ui`가 한다.
    pub(crate) fn jira(&self) -> &JiraState {
        &self.jira
    }

    /// PR 패널 상태. `lib.rs`가 `pr_panel::view`에 넘겨 (열려 있으면) 패널을 그린다.
    pub(crate) fn pr_panel(&self) -> &PrPanelState {
        &self.pr_panel
    }

    /// worktree의 PR 상태 조회를 발급한다. **`force=false`면 on-activate 1회**(캐시가
    /// 이미 있으면 건너뛴다); **`force=true`면 수동 새로고침**(항상 재조회). gh 호출은
    /// `forge_tasks`가 UI 스레드 밖에서 돌린다 — 여기서는 `Checking`만 세우고 op를 건다.
    fn request_github_status(&mut self, worktree: WorktreeId, force: bool) -> iced::Task<Message> {
        // 조회가 이미 진행 중이면 중복 발급하지 않는다(강제여도 in-flight는 존중).
        if matches!(self.github_status.get(&worktree), Some(GithubStatus::Checking)) {
            return iced::Task::none();
        }
        // on-activate는 **1회**다: 캐시가 있으면(성공이든 실패든) 다시 조회하지 않는다.
        // 재조회는 명시적 새로고침(force)의 몫이다 — 배경 폴링이 아니다.
        if !force && self.github_status.contains_key(&worktree) {
            return iced::Task::none();
        }
        let Some((_repo_id, entry)) = self.find_worktree(&worktree) else {
            return iced::Task::none();
        };
        let branch = entry.branch.clone();
        let linked_pr = self
            .worktree_meta
            .get(&worktree)
            .and_then(|meta| meta.linked_github_pr);
        self.github_status
            .insert(worktree.clone(), GithubStatus::Checking);
        let op = self.next_op();
        self.latest_forge_op.insert(worktree.clone(), op);
        crate::forge_tasks::fetch_status(op, worktree, entry.path, branch, linked_pr)
    }

    /// 이 worktree 브랜치에 PR 번호를 굳힌다. `created_at_unix_ms`와 **같은 경로**로
    /// `WorktreeMeta`에 산다 — `persisted_snapshot`이 매 저장마다 `Worktree`를 새로
    /// 합성하므로, 여기 씨딩하지 않으면 한 번 저장에 링크가 사라진다.
    fn link_pr(&mut self, worktree: &WorktreeId, number: u64) {
        self.worktree_meta
            .entry(worktree.clone())
            .or_default()
            .linked_github_pr = Some(number);
    }

    /// 이 worktree에 Linear 이슈를 링크한다(N1 §1.3). `link_pr`과 **같은 경로**로 `WorktreeMeta`에
    /// 산다 — `persisted_snapshot`이 매 저장마다 `Worktree`를 새로 합성하므로, 여기 씨딩하지
    /// 않으면 한 번 저장에 링크가 사라진다(forge #14 데이터-손실 클래스). 부르는 쪽이 persist한다.
    fn link_linear_issue(&mut self, worktree: &WorktreeId, link: &LinkedLinearIssue) {
        let meta = self.worktree_meta.entry(worktree.clone()).or_default();
        meta.linked_linear_issue = Some(link.issue.clone());
        meta.linked_linear_issue_workspace_id = link.workspace_id.clone();
        meta.linked_linear_issue_organization_url_key = link.organization_url_key.clone();
    }

    /// 사이드바 worktree 행이 읽는, 링크된 Linear 이슈 식별자(예: `ENG-123`). 없으면 `None`.
    pub(crate) fn linked_linear_issue(&self, worktree: &WorktreeId) -> Option<&str> {
        self.worktree_meta
            .get(worktree)
            .and_then(|m| m.linked_linear_issue.as_deref())
    }

    /// 이 worktree에 Jira 이슈를 링크한다(N2 §2). `link_linear_issue`와 **같은 경로**로
    /// `WorktreeMeta`에 산다 — 씨딩·재주입이 없으면 한 번 저장에 링크가 사라진다(forge #14).
    /// 부르는 쪽이 persist한다.
    fn link_jira_issue(&mut self, worktree: &WorktreeId, link: &LinkedJiraIssue) {
        let meta = self.worktree_meta.entry(worktree.clone()).or_default();
        meta.linked_jira_issue = Some(link.issue.clone());
        meta.linked_jira_site = link.site.clone();
    }

    /// 사이드바 worktree 행이 읽는, 링크된 Jira 이슈 키(예: `PROJ-123`). 없으면 `None`.
    pub(crate) fn linked_jira_issue(&self, worktree: &WorktreeId) -> Option<&str> {
        self.worktree_meta
            .get(worktree)
            .and_then(|m| m.linked_jira_issue.as_deref())
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
    /// 이 worktree를 **suaegi가 만들었는가.** `workspace_root` 아래 있으면
    /// 우리 것이다.
    ///
    /// 밖에서 발견된 worktree(사용자가 직접 만든 것, 다른 도구가 만든 것)에는
    /// **쓰지 않는다** — 우리 소유가 아닌 디렉터리에 우리 파일을 남기는 일이다.
    /// 그 대가는 그런 worktree의 배지가 `Unknown`에 머무는 것이고, 그쪽이 옳다.
    fn is_suaegi_worktree(&self, path: &Path) -> bool {
        path.starts_with(&self.workspace_root)
    }

    fn inject_into_worktree(&self, worktree_path: &Path) {
        let Some(script) = &self.hook_script else {
            return;
        };
        if !self.is_suaegi_worktree(worktree_path) {
            return;
        }
        if let Err(e) = crate::agent_status::inject::write_worktree_settings(worktree_path, script) {
            eprintln!("suaegi: could not write hook settings into the worktree: {e}");
        }
    }

    /// 서버 핸들을 넘겨받는다. **앱이 서버를 소유해야 하는 이유가 둘이다**:
    /// 떨구면 포트가 닫히고, 버린 이벤트 카운터(`dropped()`)를 읽을 곳이
    /// 필요하다.
    fn attach_hook_server(&mut self, server: Option<HookServer>) {
        if let Some(server) = server {
            self.hook_endpoint = Some((server.port(), server.token().to_string()));
            self.hook_server = Some(server);
        }
    }

    /// 훅 이벤트가 버려졌는지 확인하고, 버려졌으면 **모든 pane의 훅 상태를
    /// 무효화한다.**
    ///
    /// 큐가 가득 차면 정책상 **새 이벤트가 버려진다**(drop-newest). 버려진 것이
    /// 마지막 `PermissionRequest`나 `Stop`이면 그 배지는 **영원히 틀린 채로
    /// 남는다** — 폴링은 계속 `Agent`를 보고, 잃어버린 `Waiting`은 재구성할 수
    /// 없으며, 후속 훅이 아예 없을 수도 있다. "다음 이벤트가 곧 고친다"는
    /// 항상 참이 아니다.
    ///
    /// **어느 pane의 이벤트가 버려졌는지는 알 수 없으므로**(버린 쪽은 pane을
    /// 모른다) 전부 `Unknown`으로 되돌린다. 틀린 상태를 자신 있게 보여주는 것보다
    /// "모른다"가 정직하다.
    fn note_hook_drops(&mut self) {
        let Some(server) = &self.hook_server else {
            return;
        };
        // 카운터 읽기와 판단을 나눈다 — 판단 쪽만 테스트에서 직접 구동할 수
        // 있어야 한다(채널을 실제로 넘치게 하려면 HTTP 요청 수백 개가 든다).
        let dropped = server.dropped();
        self.apply_hook_drops(dropped);
    }

    fn apply_hook_drops(&mut self, dropped: u64) {
        if dropped <= self.hook_drops_seen {
            return;
        }
        self.hook_drops_seen = dropped;
        for badge in self.badges.values_mut() {
            badge.hook = None;
        }
        self.last_error = Some(format!(
            "hook events were dropped ({dropped} total); agent badges reset to unknown"
        ));
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
        // presence를 **먼저** 읽는다(아래에서 `badges`를 가변 대여하므로).
        let presence = self.worktree_presence(worktree_id);
        let Some(badge) = self.badges.get_mut(worktree_id) else {
            // 우리가 스폰한 적 없는 pane의 이벤트다.
            return;
        };
        if event.spawn_nonce != badge.expected {
            // 옛 세대의 늦은 훅. **조용히 버린다** — 오류가 아니다.
            return;
        }
        // 이 세대의 훅이 **하나라도** 도착했다 = claude가 이 pane을 소유한다.
        // 출처가 `Ignore`(유령)든 `Reset`(SessionStart)이든, 이 pane은 훅-권위이고
        // 그 뒤로 OSC-title이 배지를 덮어선 안 된다([`note_title_status_for_badge`]).
        badge.received_hook = true;
        match hook_outcome(event) {
            HookOutcome::Ignore => {}
            HookOutcome::Reset => badge.hook = None,
            HookOutcome::Set(state) => badge.hook = Some((state, Instant::now())),
        }
        // **훅이 바뀌면 "유지할 값"도 같이 갱신한다.**
        //
        // `previous`는 확정되지 않은 `NoAgent` 구간에서 리듀서가 드는 값이다.
        // 폴링에서만 갱신하면 **폴 사이에 도착한 훅이 반영되지 않아**, 훅 직후
        // `NoAgent`가 한 번 오면 한 틱 낡은 값을 든다 — 일하는 중인 pane이
        // 잠깐 `Unknown`으로 보인다. 훅과 폴링은 같은 배지의 두 입력이므로
        // 어느 쪽이 움직이든 갱신돼야 한다.
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

    /// 모든 세션의 타이틀 변경을 드레인하고, OscTitle 세션은 최신 타이틀에서 배지
    /// 상태를 추론한다. **모든 세션을 드레인한다**(Hooks 세션도) — 안 그러면 그
    /// 세션의 `title_changes` deque가 상한까지 자란다. 상태를 먹이는 것만 OscTitle로
    /// 가른다([`title_status_update`]).
    ///
    /// [`title_status_update`]: crate::agent_status::title::title_status_update
    fn poll_title_status(&mut self) {
        // `sessions()`가 스토어를 빌리는 동안 `badges`를 못 바꾸므로 먼저 모은다.
        let mut updates: Vec<(WorktreeId, HookState)> = Vec::new();
        for (id, session) in self.session_store.sessions() {
            let changes = session.take_title_changes();
            let Some(source) = self.session_store.status_source(id) else {
                continue;
            };
            if let Some(state) =
                crate::agent_status::title::title_status_update(&changes, source)
            {
                if let Some(worktree_id) = self.session_worktrees.get(&id) {
                    updates.push((worktree_id.clone(), state));
                }
            }
        }
        for (worktree_id, state) in updates {
            self.note_title_status_for_badge(&worktree_id, state);
        }
    }

    /// 무장된 프롬프트 게이트를 한 틱 전진시킨다. composer 준비(BRACKETED_PASTE +
    /// 조용한 창)에 이른 stdin-after-start 세션에 프롬프트를 **한 번** 써넣는다.
    /// 하드 타임아웃이 지났거나 사용자가 이미 타이핑했으면(게이트가 이미 제거됨)
    /// 조용히 아무 일도 하지 않는다.
    ///
    /// **관측을 먼저 모으고(불변 차용) 게이트를 전진시킨 뒤(가변) 쓴다** — 세
    /// 단계를 섞으면 `session_store`(불변)와 `prompt_gates`(가변)를 동시에 빌려
    /// 컴파일이 안 된다.
    fn poll_prompt_injections(&mut self) {
        if self.prompt_gates.is_empty() {
            return;
        }
        let now = Instant::now();
        // 1) 살아 있는 세션마다 관측을 모은다(스냅샷을 뜨지 않는 값싼 조회).
        let observations: Vec<(SessionId, GateObservation)> = self
            .prompt_gates
            .keys()
            .copied()
            .filter_map(|id| {
                let session = self.session_store.session(id)?;
                Some((
                    id,
                    GateObservation {
                        now,
                        bracketed_paste: session.bracketed_paste_enabled(),
                        generation: session.generation(),
                    },
                ))
            })
            .collect();
        // 2) 게이트를 전진시키고, 주입할 것과 끝난 것을 가른다.
        let mut injects: Vec<(SessionId, String)> = Vec::new();
        let mut finished: Vec<SessionId> = Vec::new();
        for (id, obs) in observations {
            if let Some(gate) = self.prompt_gates.get_mut(&id) {
                match gate.poll(obs) {
                    GateAction::Inject => {
                        injects.push((id, gate.prompt().to_string()));
                        finished.push(id);
                    }
                    // **포기한 게이트도 회수한다.** 안 그러면 `has_armed_prompt_gates()`가
                    // 계속 true라 presence tier가 세션 내내 ACTIVE(750ms)에 고착된다 —
                    // 이 게이트의 근거("주입 창은 시작 직후 몇 초뿐")와 정면으로 어긋난다.
                    GateAction::GaveUp => {
                        finished.push(id);
                    }
                    GateAction::Wait => {}
                }
            }
        }
        // 주입했거나 포기한 게이트를 제거한다 — 회수가 tier를 idle로 되돌린다.
        for id in finished {
            self.prompt_gates.remove(&id);
        }
        // 3) 프롬프트를 PTY에 써넣는다(항상 bracketed paste로 감싸 raw 유출 방지).
        for (id, prompt) in injects {
            if let Some(session) = self.session_store.session(id) {
                session.inject_bracketed_paste(&prompt);
            }
        }
    }

    /// 무장된 프롬프트 게이트가 하나라도 있는가. presence 폴링 티어를 그동안
    /// [`ACTIVE_TIER`](crate::presence_poll::ACTIVE_TIER)로 올려, 조용한 창을 더
    /// 촘촘히 관측한다(주입 창은 짧고 정확도가 중요하다).
    pub(crate) fn has_armed_prompt_gates(&self) -> bool {
        !self.prompt_gates.is_empty()
    }

    /// 타이틀에서 추론한 상태를 배지 장부에 반영한다. `apply_hook`의 꼬리와 같은
    /// 규칙으로 `hook` 슬롯과 `previous`를 갱신한다 — 타이틀-파생 상태는 훅과 같은
    /// "가장 최근 상태 신호 + 시각" 슬롯을 쓴다.
    ///
    /// **훅을 본 pane은 건드리지 않는다**(`received_hook`). 로그인 셸 안에서 돌아가는
    /// claude처럼 OSC-title 세션이 훅도 낼 때, 타이틀이 정밀한 훅 상태를 덮는 것을
    /// 막는 유일한 지점이다 — precedence: 훅 > 타이틀.
    fn note_title_status_for_badge(&mut self, worktree_id: &WorktreeId, state: HookState) {
        let presence = self.worktree_presence(worktree_id);
        let Some(badge) = self.badges.get_mut(worktree_id) else {
            return;
        };
        if badge.received_hook {
            return;
        }
        badge.hook = Some((state, Instant::now()));
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
        // 무장된(또는 대기 중인) 주입은 세션과 함께 사라진다 — 닫힌 세션에
        // 프롬프트를 써넣을 수는 없고, 남겨두면 맵이 무한히 자란다.
        self.prompt_gates.remove(&id);
        self.pending_injections.remove(&id);
        if let Some(worktree_id) = self.session_worktrees.remove(&id) {
            self.worktree_sessions.remove(&worktree_id);
            // **배지 장부도 같이 간다.** 남겨두면 세션이 사라진 뒤에도 마지막
            // 훅 상태가 살아 있는데, presence는 세션이 없으므로 `Unknown`으로
            // 떨어지고 — 리듀서의 `Unknown` 팔은 훅을 그대로 신뢰한다. 마지막
            // 훅이 `Waiting`이었다면 **`Waiting`은 나이로 감쇠하지 않으므로**
            // 사이드바 행(git 목록으로 그려지므로 세션과 무관하게 살아남는다)에
            // 주황색 "사람을 기다림" 표시가 영구히 박힌다.
            // 지우면 `worktree_badge`가 `None` 가지를 타 `Unknown`이 된다 —
            // 세션이 없을 때 정직한 답이다. 맵이 무한히 자라는 것도 같이 막는다.
            self.badges.remove(&worktree_id);
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
                // **사용자가 타이핑을 시작했으면 주입을 취소한다.** 사용자가 이미
                // 뭔가 치고 있는데 프롬프트가 끼어들면 그가 친 것과 뒤섞인다 —
                // mis-injection is worse than none.
                self.prompt_gates.remove(&id);
                self.pending_injections.remove(&id);
                let outcome = session.send_key(&input);
                self.note_write(id, outcome);
                iced::Task::none()
            }
            TermCommand::Paste(text) => {
                // 사용자가 직접 붙여넣는 것도 타이핑과 같다 — 주입을 취소한다.
                self.prompt_gates.remove(&id);
                self.pending_injections.remove(&id);
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
                // **사이드바 입력에 포커스를 잡아 터미널을 unfocus한다.** 이게 없으면
                // 포커스된 터미널 pane이 그대로 남아, 여기 타이핑한 키가 활성
                // 터미널로도 새어 들어간다(iced가 포커스된 터미널을 먼저 라우팅해
                // `is_event_captured`도 못 막는다 — 실측으로 확인).
                let focus = iced::widget::operation::focus(crate::sidebar::name_input_id(&repo_id));
                self.worktree_name_draft.insert(repo_id, value);
                focus
            }
            Message::WorktreeAgentSelected { repo_id, choice } => {
                // 로그인 셸(기본)을 고르면 엔트리를 지운다 — "없음"과 같은 의미라
                // 맵이 불필요하게 자라지 않는다.
                if choice == AgentChoice::LOGIN_SHELL {
                    self.worktree_agent_draft.remove(&repo_id);
                } else {
                    self.worktree_agent_draft.insert(repo_id, choice);
                }
                iced::Task::none()
            }
            Message::WorktreePromptInputChanged { repo_id, value } => {
                // name 입력과 같은 이유로 사이드바에 포커스를 잡아 터미널을 unfocus한다.
                let focus =
                    iced::widget::operation::focus(crate::sidebar::prompt_input_id(&repo_id));
                // 빈 값이면 엔트리를 지운다(= 주입 없음, 기본). 맵을 불필요하게
                // 키우지 않는다.
                if value.is_empty() {
                    self.worktree_prompt_draft.remove(&repo_id);
                } else {
                    self.worktree_prompt_draft.insert(repo_id, value);
                }
                focus
            }
            Message::CreateWorktreeSubmitted { repo_id } => {
                let Some(repo) = self.repo_by_id(&repo_id).cloned() else {
                    return iced::Task::none();
                };
                let name = self.worktree_name_draft(&repo_id).trim().to_string();
                if name.is_empty() {
                    return iced::Task::none();
                }
                // 제출 시점의 피커 선택을 create op에 실어 보낸다(응답을 기다리는
                // 사이 피커가 바뀌어도 이 worktree엔 영향 없다).
                let selected_agent = self
                    .worktree_agent_selection(&repo_id)
                    .0
                    .map(|id| id.to_string());
                // 제출 시점의 초기 프롬프트도 함께 스냅샷한다(빈 값은 `None`) —
                // 응답을 기다리는 사이 입력창이 바뀌어도 이 worktree엔 영향 없다.
                let initial_prompt = self
                    .worktree_prompt_draft
                    .get(&repo_id)
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
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
                    selected_agent,
                    initial_prompt,
                )
            }
            Message::WorktreeCreated {
                repo_id,
                created_with_agent,
                initial_prompt,
                result,
                ..
            } => match result {
                // **Task 6: 생성 시점이 메타데이터의 유일한 진짜 출처다.**
                // 여기서 `Ok(_created)`를 통째로 버리면 그 시각은 영영 없다 —
                // `persisted_snapshot`이 매 저장마다 0을 합성하게 된다.
                Ok(created) => {
                    self.last_error = None;
                    self.worktree_name_draft.remove(&repo_id);
                    // 이 create가 소비했으니 피커 선택도 초기화(다음 worktree는 다시
                    // 로그인 셸 기본에서 시작). 성공 경로에서만 지운다 — 실패하면
                    // 사용자가 재시도할 때 고른 에이전트가 유지된다.
                    self.worktree_agent_draft.remove(&repo_id);
                    // 초기-프롬프트 입력창도 초기화(같은 이유). 스냅샷된 값은 이미
                    // `initial_prompt`에 실려 있으므로 여기서 지워도 잃지 않는다.
                    self.worktree_prompt_draft.remove(&repo_id);
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    // **주입은 worktree 생성 직후다.** 사용자가 그 안에서
                    // `claude`를 어떻게 띄우든(맨손, `--resume`, 별칭) 설정이
                    // 적용되게 하는 유일한 지점이다 — 우리는 `claude`를 직접
                    // 실행하지 않으므로 `--settings`를 넘길 argv가 없다.
                    self.inject_into_worktree(&created.path);
                    let created_id = worktree_id_for(&created.path);
                    self.worktree_meta.insert(
                        created_id.clone(),
                        WorktreeMeta {
                            // 6c: 생성 시점에 고른 에이전트를 여기 굳힌다. `None`이면
                            // (피커를 안 건드림) 로그인 셸로 뜬다 — 예전과 동일.
                            // 세션 시작(`start_session_for`)이 이 값을 레지스트리로
                            // 다시 검증해 그 에이전트를 직접 띄운다.
                            created_with_agent,
                            created_at_unix_ms: now_ms,
                            // 갓 만든 worktree엔 아직 연결된 PR이 없다. UI 후속(Create PR
                            // 다이얼로그)이 생성 성공 시 이 값을 채운다.
                            linked_github_pr: None,
                            // 갓 만든 worktree엔 링크된 Linear 이슈도 없다(이슈 목록의
                            // "link this worktree"가 나중에 굳힌다).
                            linked_linear_issue: None,
                            linked_linear_issue_workspace_id: None,
                            linked_linear_issue_organization_url_key: None,
                            // 갓 만든 worktree엔 링크된 Jira 이슈도 없다(N2 §2, 같은 이유).
                            linked_jira_issue: None,
                            linked_jira_site: None,
                        },
                    );
                    // **일회성 프롬프트를 메모리에만 담는다**(`WorktreeMeta`가 아니라).
                    // 이 worktree의 첫 세션 시작이 소비한다. 영속화되지 않으므로
                    // 재시작 후 복원된 세션은 낡은 프롬프트를 다시 주입하지 않는다.
                    if let Some(prompt) = initial_prompt {
                        self.pending_prompts.insert(created_id, prompt);
                    }
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
                // **worktree가 활성화되면 PR 상태를 1회 조회한다**(§3.7). 세션이 이미
                // 열려 있든 새로 뜨든 활성화는 선택이다 — 그래서 세션 태스크와 별개로
                // 여기서 발급하고 batch한다. `force=false`라 캐시가 있으면 건너뛴다
                // (재조회는 명시적 새로고침의 몫, `PresenceTick`이 아니다).
                let status_task = self.request_github_status(id.clone(), false);
                // 세션 쪽 태스크. 조기 return을 없애 status_task와 항상 함께 실린다.
                let session_task = if let Some(&session_id) = self.worktree_sessions.get(&id) {
                    // 이미 열려 있다 — 새 세션을 띄우지 않고 그 pane에 포커스만
                    // 옮긴다. pane_grid는 pane → 값 매핑만 들고 있으므로 여기서
                    // 직접 훑어야 한다(양방향 인덱스가 없다).
                    if let Some(panes) = &self.panes {
                        if let Some((pane, _)) = panes.iter().find(|(_, sid)| **sid == session_id) {
                            self.focused_pane = Some(*pane);
                        }
                    }
                    iced::Task::none()
                } else if self.pending_session_starts.contains_key(&id) {
                    // 시작 요청이 이미 나가 있다 — 빠른 재클릭으로 세션이
                    // 두 개 뜨는 걸 막는다.
                    iced::Task::none()
                } else {
                    // 복원과 **같은 경로**를 쓴다 — 갈라두면 한쪽만 장부를 채운다.
                    match self.start_session_for(&id) {
                        Some((_session_id, task)) => task,
                        None => iced::Task::none(),
                    }
                };
                iced::Task::batch([status_task, session_task])
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
                            self.pending_injections.remove(&id);
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
                                // **세션이 실제로 살아났다 — 이제 주입 게이트를
                                // 무장한다.** 슬롯이 이 시점에 생기므로(`accept_started`)
                                // 다음 `PresenceTick`부터 poll이 세션을 찾을 수 있다.
                                // 게이트의 하드 타임아웃 시계는 첫 poll부터 잰다.
                                if let Some(prompt) = self.pending_injections.remove(&id) {
                                    self.prompt_gates.insert(id, PromptGate::new(prompt));
                                }
                                if restoring {
                                    self.note_restore_outcome(
                                        &worktree_id,
                                        LeafOutcome::Started(id),
                                    );
                                } else {
                                    // 복원 밖에서 열린 pane = 사용자의 편집이다.
                                    self.note_user_layout_edit();
                                    self.open_pane_for_session(id);
                                    self.persist();
                                }
                            }
                            Err(_) => {
                                // worktree가 그새 삭제됐다 — 세션은 이미 reaper로
                                // 갔다(`accept_started`). 타이틀·주입 대기를 정리한다.
                                self.pending_injections.remove(&id);
                                self.session_titles.remove(&id);
                                self.note_restore_outcome(
                                    &worktree_id,
                                    LeafOutcome::WorktreeGone,
                                );
                            }
                        }
                    }
                    Err(err) => {
                        self.pending_injections.remove(&id);
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
                self.note_user_layout_edit();
                // 드롭은 트리를 바꾼다. 플랜의 트리거 목록에는 없지만 "pane
                // 열기/닫기"와 같은 종류의 변경이고, 저장하지 않으면 사용자가
                // 옮겨놓은 배치가 재시작에 사라진다.
                self.persist();
                iced::Task::none()
            }
            Message::PaneDragged(_) => iced::Task::none(),
            Message::PaneResized(pane_grid::ResizeEvent { split, ratio }) => {
                // **비유한 비율을 pane_grid에 넣지 않는다.** iced가 0 높이 분할에서
                // `0.0/0.0`을 만들 수 있고 `f32::clamp`는 NaN을 통과시킨다.
                // 저장 경로(`quantize_ratio`)가 이미 막지만, 오염된 값을 살아 있는
                // 레이아웃에 먼저 들이면 그 프레임의 계산이 전부 NaN이 된다.
                let ratio = if ratio.is_finite() {
                    ratio.clamp(0.0, 1.0)
                } else {
                    0.5
                };
                if let Some(panes) = &mut self.panes {
                    panes.resize(split, ratio);
                }
                self.note_user_layout_edit();
                // **곧바로 저장하지 않는다** — 드래그 한 번에 이 메시지가 수십 번
                // 온다. 디바운스의 이유는 `schedule_layout_save` 참고.
                self.schedule_layout_save()
            }
            Message::PaneCloseRequested(pane) => {
                // pane 자체를 지우는 것도 `close_session`이 한다 — 대화형/비대화형
                // 경로가 갈라져서 pane이 새던 것이 이 수렴의 이유다.
                if let Some(&session_id) = self.panes.as_ref().and_then(|panes| panes.get(pane)) {
                    // 사용자가 직접 닫았다 — 이제 화면이 진실이다.
                    self.note_user_layout_edit();
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
                // 이 틱이 앱에서 가장 규칙적으로 도는 지점이라 여기서 확인한다.
                self.note_hook_drops();
                // 비-Claude(OscTitle) 세션의 상태는 터미널 타이틀에서 온다. presence와
                // 같은 티어로 폴링한다 — 나이 기반 배지 규칙과 결이 맞는다.
                self.poll_title_status();
                // 무장된 stdin-after-start 주입 게이트를 같은 틱에 전진시킨다 —
                // 앱에서 가장 규칙적으로 도는 지점이다.
                self.poll_prompt_injections();
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
            Message::RestoreWatchdog { generation } => {
                self.expire_restore(generation);
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

            // ---- Plan 7a-1: GitHub PR 상태 · 생성 ----
            Message::GithubRefreshRequested { worktree } => {
                // 명시적 수동 새로고침 — 캐시가 있어도 다시 조회한다(force).
                self.request_github_status(worktree, true)
            }
            Message::GithubStatusFetched {
                worktree,
                op,
                fetch,
                eligibility,
            } => {
                // 낡은 응답은 버린다: 수동 새로고침이 on-activate 조회를 앞질렀을 수
                // 있고, 그때 먼저 떠난 조회의 결과가 새 것을 덮으면 안 된다.
                if self.latest_forge_op.get(&worktree) != Some(&op) {
                    return iced::Task::none();
                }
                // PR을 찾았으면 번호를 굳힌다(다음 조회는 번호로 — 상태가 안정적이고,
                // 저장을 한 번 거쳐도 링크가 남는다). `Unavailable`은 **아무것도
                // 지우지 않는다** — 조회 실패가 알려진 PR 링크를 날리면 안 된다.
                if let GithubFetch::Resolved(ReviewLookup::Found(review)) = &fetch {
                    let number = review.number;
                    if self.worktree_meta.get(&worktree).and_then(|m| m.linked_github_pr)
                        != Some(number)
                    {
                        self.link_pr(&worktree, number);
                        self.persist();
                    }
                }
                self.github_status
                    .insert(worktree, GithubStatus::Fetched { fetch, eligibility });
                iced::Task::none()
            }
            Message::CreatePrOpened { worktree } => {
                // 제목·base 초안을 채운다. base는 repo 기본 브랜치, 제목은 브랜치명.
                let (branch, base) = match self.find_worktree(&worktree) {
                    Some((repo_id, entry)) => {
                        let base = self
                            .repo_by_id(&repo_id)
                            .and_then(|r| r.worktree_base_ref.clone())
                            .unwrap_or_else(|| "main".to_string());
                        (entry.branch.clone().unwrap_or_default(), base)
                    }
                    None => (String::new(), "main".to_string()),
                };
                self.create_pr = Some(CreatePrDraft {
                    worktree,
                    title: branch,
                    body: String::new(),
                    base,
                    draft: false,
                    submitting: false,
                    error: None,
                });
                iced::Task::none()
            }
            Message::CreatePrTitleChanged(value) => {
                if let Some(dialog) = &mut self.create_pr {
                    dialog.title = value;
                }
                iced::Task::none()
            }
            Message::CreatePrBodyChanged(value) => {
                if let Some(dialog) = &mut self.create_pr {
                    dialog.body = value;
                }
                iced::Task::none()
            }
            Message::CreatePrBaseChanged(value) => {
                if let Some(dialog) = &mut self.create_pr {
                    dialog.base = value;
                }
                iced::Task::none()
            }
            Message::CreatePrDraftToggled(value) => {
                if let Some(dialog) = &mut self.create_pr {
                    dialog.draft = value;
                }
                iced::Task::none()
            }
            Message::CreatePrCancelled => {
                self.create_pr = None;
                iced::Task::none()
            }
            Message::CreatePrSubmitted => {
                let Some(dialog) = &self.create_pr else {
                    return iced::Task::none();
                };
                // 중복 제출 방지 — 이미 진행 중이면 무시한다.
                if dialog.submitting {
                    return iced::Task::none();
                }
                // 제목이 비면 UI에서 미리 막는다(백엔드도 거부하지만 왕복이 아깝다).
                if dialog.title.trim().is_empty() {
                    if let Some(dialog) = &mut self.create_pr {
                        dialog.error = Some("Title is required.".to_string());
                    }
                    return iced::Task::none();
                }
                let worktree = dialog.worktree.clone();
                // head 브랜치와 worktree 경로는 목록에서 확정한다.
                let Some((_repo_id, entry)) = self.find_worktree(&worktree) else {
                    if let Some(dialog) = &mut self.create_pr {
                        dialog.error = Some("This worktree is no longer available.".to_string());
                    }
                    return iced::Task::none();
                };
                let body = dialog.body.clone();
                let input = CreateReviewInput {
                    worktree_path: entry.path,
                    base: dialog.base.trim().to_string(),
                    head: entry.branch.clone(),
                    title: dialog.title.trim().to_string(),
                    // body가 비면 repo PR 템플릿을 쓴다(백엔드가 채운다).
                    use_template: body.trim().is_empty(),
                    body,
                    draft: dialog.draft,
                };
                if let Some(dialog) = &mut self.create_pr {
                    dialog.submitting = true;
                    dialog.error = None;
                }
                let op = self.next_op();
                crate::forge_tasks::create_pr(op, worktree, input)
            }
            Message::CreatePrCreated {
                worktree,
                op: _,
                result,
            } => match result {
                Ok(review) => {
                    // **생성 성공은 링크를 굳힌다**(§5 mutation (c)). 저장을 거쳐도
                    // 남도록 `WorktreeMeta`에 씨딩하고 persist한다.
                    self.link_pr(&worktree, review.number);
                    self.persist();
                    // 다이얼로그가 아직 이 worktree를 위해 열려 있으면 닫는다.
                    if self.create_pr.as_ref().map(|d| &d.worktree) == Some(&worktree) {
                        self.create_pr = None;
                    }
                    // 상태를 강제 재조회해 표시자가 새 PR(Found)로 바뀌게 한다.
                    self.request_github_status(worktree, true)
                }
                Err(err) => {
                    // **분류된 문구**를 다이얼로그에 표시한다(raw stderr 아님).
                    if let Some(dialog) = &mut self.create_pr {
                        if dialog.worktree == worktree {
                            dialog.submitting = false;
                            dialog.error = Some(err);
                        }
                    }
                    iced::Task::none()
                }
            },

            // ---- Plan 7b: PR 패널 ----
            Message::PrPanelOpened { worktree } => {
                // 헤더는 **7a 리뷰에서** 씨딩한다(중복 상태-조회 안 함). Found 리뷰가
                // 없으면(비-GitHub·조회 전·PR 없음·조회 실패) 열지 않는다 — 사이드바
                // 버튼도 Present일 때만 뜨므로 정상 경로에선 항상 리뷰가 있다.
                let review = match self.github_status.get(&worktree) {
                    Some(GithubStatus::Fetched {
                        fetch: GithubFetch::Resolved(ReviewLookup::Found(review)),
                        ..
                    }) => review.clone(),
                    _ => return iced::Task::none(),
                };
                let Some((_repo_id, entry)) = self.find_worktree(&worktree) else {
                    return iced::Task::none();
                };
                let path = entry.path;
                let number = review.number;
                let op = self.next_op();
                self.pr_panel
                    .open(worktree.clone(), number, review.title, review.state, op);
                crate::forge_tasks::fetch_pr_details(op, worktree, path, number)
            }
            Message::PrPanelClosed => {
                self.pr_panel.close();
                iced::Task::none()
            }
            Message::PrPanelRefreshRequested => {
                let Some(worktree) = self.pr_panel.worktree().cloned() else {
                    return iced::Task::none();
                };
                let Some(number) = self.pr_panel.number() else {
                    return iced::Task::none();
                };
                let Some((_repo_id, entry)) = self.find_worktree(&worktree) else {
                    return iced::Task::none();
                };
                let path = entry.path;
                let op = self.next_op();
                self.pr_panel.begin_details(op);
                crate::forge_tasks::fetch_pr_details(op, worktree, path, number)
            }
            Message::PrDetailsFetched {
                worktree,
                op,
                details,
            } => {
                // 낡은/다른 worktree의 결과는 버린다(수동 새로고침이 on-open 조회를
                // 앞질렀을 수 있다 — diff 패널과 같은 규율).
                if self.pr_panel.accept_details(&worktree, op) {
                    self.pr_panel.apply_details(details);
                }
                iced::Task::none()
            }
            Message::MergeRequested => {
                // **확인 단계를 열 뿐 머지하지 않는다.** 머지가능성이 Mergeable이
                // 아니면 아무 일도 없다(비활성 버튼의 마지막 방어선).
                self.pr_panel.request_merge();
                iced::Task::none()
            }
            Message::MergeMethodSelected(method) => {
                self.pr_panel.set_method(method);
                iced::Task::none()
            }
            Message::MergeDeleteBranchToggled(value) => {
                self.pr_panel.set_delete_branch(value);
                iced::Task::none()
            }
            Message::MergeCancelled => {
                self.pr_panel.cancel_merge();
                iced::Task::none()
            }
            Message::MergeConfirmed => {
                // **파괴적 확정.** worktree·번호·경로를 먼저 확정한 뒤 `confirm_merge`를
                // 부른다 — 그래야 확인 단계가 없을 때(`None`) `merging`을 세우지 않고
                // 조용히 끝난다. 이것이 원클릭 파괴를 막는 게이트다: 확인 단계가
                // 열려 있지 않으면 `merge_pr` 태스크를 절대 만들지 않는다.
                let Some(worktree) = self.pr_panel.worktree().cloned() else {
                    return iced::Task::none();
                };
                let Some(number) = self.pr_panel.number() else {
                    return iced::Task::none();
                };
                let Some((_repo_id, entry)) = self.find_worktree(&worktree) else {
                    return iced::Task::none();
                };
                let path = entry.path;
                let op = self.next_op();
                let Some(confirm) = self.pr_panel.confirm_merge(op) else {
                    // 확인 단계가 없다 = 아무도 확인하지 않았다 → 머지 발급 안 함.
                    return iced::Task::none();
                };
                let options = MergeOptions {
                    delete_branch: confirm.delete_branch,
                };
                crate::forge_tasks::merge_pr(op, worktree, path, number, confirm.method, options)
            }
            Message::MergeCompleted {
                worktree,
                op,
                display,
            } => {
                if !self.pr_panel.accept_merge(&worktree, op) {
                    return iced::Task::none();
                }
                let merged = matches!(display, MergeResultDisplay::Merged);
                self.pr_panel.apply_merge(display);
                if !merged {
                    // Rejected/Unavailable은 패널에 남긴다 — 사용자가 사유를 보고
                    // 재시도/수정하도록. 상태를 새로 조회하지 않는다.
                    return iced::Task::none();
                }
                // 성공 → 사이드바 7a 표시자를 강제 재조회하고(merged로 바뀐다),
                // 패널 세부도 새로고침한다.
                let refresh_status = self.request_github_status(worktree.clone(), true);
                match (self.pr_panel.number(), self.find_worktree(&worktree)) {
                    (Some(number), Some((_repo_id, entry))) => {
                        let op2 = self.next_op();
                        self.pr_panel.begin_details(op2);
                        let details =
                            crate::forge_tasks::fetch_pr_details(op2, worktree, entry.path, number);
                        iced::Task::batch([refresh_status, details])
                    }
                    _ => refresh_status,
                }
            }

            // ---- N1: Linear 트래커 UI ----
            Message::LinearApiKeyChanged(value) => {
                self.linear.api_key_input = value;
                iced::Task::none()
            }
            Message::LinearConnectSubmitted => {
                // 중복 제출 방지.
                if self.linear.connecting {
                    return iced::Task::none();
                }
                let key = self.linear.api_key_input.trim().to_string();
                if key.is_empty() {
                    self.linear.connect_error = Some("Enter a Linear API key.".to_string());
                    return iced::Task::none();
                }
                // 입력 버퍼를 즉시 비운다 — 평문 키를 UI 상태에 오래 들고 있지 않는다. 토큰은
                // `Secret`로만 남는다.
                self.linear.api_key_input.clear();
                let token = Secret::new(key);
                self.linear.token = Some(token.clone());
                self.linear.connecting = true;
                self.linear.connect_error = None;
                let op = self.next_op();
                self.latest_linear_op = Some(op);
                crate::tracker_tasks::connect(op, token)
            }
            Message::LinearConnected { op, result } => {
                // 낡은 응답은 버린다: 재연결이 앞선 시도를 앞질렀을 수 있다.
                if self.latest_linear_op != Some(op) {
                    return iced::Task::none();
                }
                self.linear.connecting = false;
                match crate::tracker_ui::connect_view(&result) {
                    crate::tracker_ui::ConnectView::Connected(ws) => {
                        self.linear.workspace = Some(ws);
                        self.linear.connect_error = None;
                        // 연결 성공 → 이슈를 한 번 가져온다(같은 토큰으로, UI 스레드 밖).
                        self.request_linear_issues()
                    }
                    crate::tracker_ui::ConnectView::Failed(msg) => {
                        // 인증 실패면 토큰을 버린다 — 무효 키를 들고 이슈를 조회하지 않는다.
                        self.linear.token = None;
                        self.linear.workspace = None;
                        self.linear.connect_error = Some(msg);
                        iced::Task::none()
                    }
                }
            }
            Message::LinearIssuesRefreshRequested => self.request_linear_issues(),
            Message::LinearIssuesFetched { op, result } => {
                if self.latest_linear_op != Some(op) {
                    return iced::Task::none();
                }
                self.linear.issues_loading = false;
                // raw `Lookup`을 그대로 담는다 — `Unavailable`을 빈 목록으로 접지 않는다.
                // 표시 매핑(Unavailable≠no issues)은 `tracker_ui::issue_list`가 한다.
                self.linear.issues = Some(result);
                iced::Task::none()
            }
            Message::LinearIssueLinked { worktree, issue } => {
                // 이슈 + 연결된 워크스페이스 좌표 → 도메인 링크 필드(순수 매핑). 워크스페이스를
                // 모르면 좌표는 None(식별자만 링크).
                let link = crate::tracker_ui::link_for(&issue, self.linear.workspace.as_ref());
                self.link_linear_issue(&worktree, &link);
                // **저장을 거쳐도 남도록** 즉시 persist한다(`WorktreeMeta` 씨딩·재주입 —
                // forge #14 데이터-손실 가드). `link_pr` 성공 경로와 같은 규율.
                self.persist();
                iced::Task::none()
            }

            // ---- N2: Jira 트래커 UI ----
            Message::JiraSiteUrlChanged(value) => {
                self.jira.site_url_input = value;
                iced::Task::none()
            }
            Message::JiraEmailChanged(value) => {
                self.jira.email_input = value;
                iced::Task::none()
            }
            Message::JiraTokenChanged(value) => {
                self.jira.token_input = value;
                iced::Task::none()
            }
            Message::JiraCloudToggled(is_cloud) => {
                self.jira.is_cloud = is_cloud;
                iced::Task::none()
            }
            Message::JiraConnectSubmitted => {
                // 중복 제출 방지.
                if self.jira.connecting {
                    return iced::Task::none();
                }
                let site_url = crate::tracker_ui::normalize_site_url(&self.jira.site_url_input);
                if site_url.is_empty() {
                    self.jira.connect_error = Some("Enter your Jira site URL.".to_string());
                    return iced::Task::none();
                }
                let token_raw = self.jira.token_input.trim().to_string();
                if token_raw.is_empty() {
                    self.jira.connect_error = Some("Enter a Jira API token.".to_string());
                    return iced::Task::none();
                }
                let auth_type = if self.jira.is_cloud {
                    JiraAuthType::Cloud
                } else {
                    JiraAuthType::Server
                };
                // Cloud/Server-Basic은 email이 필요하다(Basic base64). Server PAT만 email 없이(→ Bearer).
                let email = self.jira.email_input.trim().to_string();
                if auth_type == JiraAuthType::Cloud && email.is_empty() {
                    self.jira.connect_error =
                        Some("Cloud Jira needs the account email.".to_string());
                    return iced::Task::none();
                }
                // 토큰 입력 버퍼를 즉시 비운다 — 평문 토큰을 UI 상태에 오래 들고 있지 않는다.
                self.jira.token_input.clear();
                let connection = JiraConnection {
                    site_url,
                    email,
                    auth_type,
                };
                let token = Secret::new(token_raw);
                // 성공 시 재조립·persist에 쓰도록 지금 굳혀 둔다(Linear가 token을 미리 세우는 것과
                // 같은 규율). 실패하면 아래 `JiraConnected` 핸들러가 되돌린다.
                self.jira.connection = Some(connection.clone());
                self.jira.token = Some(token.clone());
                self.jira.connecting = true;
                self.jira.connect_error = None;
                let op = self.next_op();
                self.latest_jira_op = Some(op);
                crate::tracker_tasks::jira_connect(op, connection, token)
            }
            Message::JiraConnected { op, result } => {
                // 낡은 응답은 버린다: 재연결이 앞선 시도를 앞질렀을 수 있다.
                if self.latest_jira_op != Some(op) {
                    return iced::Task::none();
                }
                self.jira.connecting = false;
                match crate::tracker_ui::jira_connect_view(&result) {
                    crate::tracker_ui::JiraConnectView::Connected(viewer) => {
                        self.jira.viewer = Some(viewer);
                        self.jira.connect_error = None;
                        // 연결 성공 → 활성 연결 설정을 persist해 부팅 재연결이 지속되게 한다.
                        self.persist();
                        // 이슈를 한 번 가져온다(같은 연결·토큰으로, UI 스레드 밖).
                        self.request_jira_issues()
                    }
                    crate::tracker_ui::JiraConnectView::Failed(msg) => {
                        // 인증 실패면 토큰·연결을 버린다 — 무효 크리덴셜로 이슈를 조회하지 않고,
                        // 무효 연결을 persist하지 않는다(다음 저장에 굳지 않도록 connection을 지운다).
                        self.jira.token = None;
                        self.jira.connection = None;
                        self.jira.viewer = None;
                        self.jira.connect_error = Some(msg);
                        iced::Task::none()
                    }
                }
            }
            Message::JiraIssuesRefreshRequested => self.request_jira_issues(),
            Message::JiraIssuesFetched { op, result } => {
                if self.latest_jira_op != Some(op) {
                    return iced::Task::none();
                }
                self.jira.issues_loading = false;
                // raw `Lookup`을 그대로 담는다 — `Unavailable`을 빈 목록으로 접지 않는다.
                // 표시 매핑(Unavailable≠no issues)은 `tracker_ui::jira_issue_list`가 한다.
                self.jira.issues = Some(result);
                iced::Task::none()
            }
            Message::JiraIssueLinked { worktree, issue } => {
                // 이슈 + 연결된 사이트 → 도메인 링크 필드(순수 매핑). 사이트를 모르면 None(키만 링크).
                let site = self.jira.connection.as_ref().map(|c| c.site_url.clone());
                let link = crate::tracker_ui::jira_link_for(&issue, site.as_deref());
                self.link_jira_issue(&worktree, &link);
                // **저장을 거쳐도 남도록** 즉시 persist한다(forge #14 데이터-손실 가드).
                self.persist();
                iced::Task::none()
            }
        }
    }

    /// 저장된 토큰으로 이슈 목록 조회를 발급한다(연결 성공 직후 + 수동 새로고침). 토큰이 없으면
    /// (미연결) 아무것도 하지 않는다. 네트워크는 UI 스레드 밖(`Task::perform`).
    fn request_linear_issues(&mut self) -> iced::Task<Message> {
        let Some(token) = self.linear.token.clone() else {
            return iced::Task::none();
        };
        self.linear.issues_loading = true;
        let op = self.next_op();
        self.latest_linear_op = Some(op);
        crate::tracker_tasks::list_issues(op, token)
    }

    /// 저장된 연결·토큰으로 Jira 이슈 목록 조회를 발급한다(연결 성공 직후 + 수동 새로고침). 연결이나
    /// 토큰이 없으면(미연결) 아무것도 하지 않는다. 네트워크는 UI 스레드 밖(`Task::perform`).
    fn request_jira_issues(&mut self) -> iced::Task<Message> {
        let (Some(connection), Some(token)) =
            (self.jira.connection.clone(), self.jira.token.clone())
        else {
            return iced::Task::none();
        };
        self.jira.issues_loading = true;
        let op = self.next_op();
        self.latest_jira_op = Some(op);
        crate::tracker_tasks::jira_list_issues(op, connection, token)
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
    use suaegi_forge::{ChecksSummary, CreationBlockedReason, ForgeUnavailable, ReviewState};
    use crate::agent_status::contract::{HookEventName, HOOK_STALE_AFTER, NO_AGENT_CONFIRMATIONS};

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
            presence: AgentPresence::Agent("claude"),
        });

        assert!(matches!(
            state.worktree_presence(&worktree_id),
            AgentPresence::Agent("claude")
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
            created_with_agent: None,
            initial_prompt: None,
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

    // ---- 저하된 복원이 디스크의 좋은 레이아웃을 덮지 않는가 ----
    //
    // **기존 복원 테스트는 전부 이걸 못 잡는다.** 메모리상의 접힌 모양과
    // 게이트가 열리는 것만 단언하고, 영속화를 붙여 저장을 일으킨 뒤 파일을
    // 다시 읽지 않기 때문이다 — 즉 이름이 가리키는 바로 그 성질을 검사하지
    // 않는다. 아래 둘이 그 공백을 메운다.

    /// 저장된 레이아웃을 디스크에 심고 복원을 시작한다. `restoring_state`와 달리
    /// **영속화가 배선돼 있어** 게이트가 열릴 때 실제로 저장이 일어난다.
    fn restoring_state_with_disk(
        file: &Path,
        tree: PersistedPane,
        listed: &[&str],
    ) -> AppState {
        let mut disk = PersistedState::default();
        disk.session.panes = Some(tree.clone());
        {
            let mut store = suaegi_core::persistence::Store::new(file.to_path_buf());
            store.save(&disk).expect("seeding the good file must succeed");
        }

        let mut state = restoring_state(tree, listed);
        let boot = crate::persistence_thread::PersistenceHandle::spawn(file.to_path_buf());
        state.persistence = Some(boot.handle);
        state
    }

    /// **일시적인 시작 실패가 저장된 pane을 영구히 지우면 안 된다.**
    /// PTY 스폰 실패는 흔하고 되돌릴 수 있는 일이다 — 다음 실행에서 성공할 수도
    /// 있는 것을 디스크에서 없애버리면 사용자는 되찾을 방법이 없다.
    #[test]
    fn a_transient_start_failure_does_not_erase_the_saved_pane() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("data.json");
        let saved = split(
            PersistedAxis::Vertical,
            0.25,
            leaf("/tmp/wt-a"),
            leaf("/tmp/wt-b"),
        );
        let mut state =
            restoring_state_with_disk(&file, saved.clone(), &["/tmp/wt-a", "/tmp/wt-b"]);

        deliver_start(&mut state, "/tmp/wt-a", false); // 일시적 실패
        deliver_start(&mut state, "/tmp/wt-b", true);

        // 화면은 저하된다 — 그건 옳다.
        assert_eq!(
            restored_shape(&state),
            "wt-b",
            "precondition: the live layout collapses around the leaf that failed"
        );
        assert!(
            state.hydration.is_open(),
            "precondition: degraded completion still opens the gate"
        );

        // **디스크는 저하되면 안 된다.**
        assert_eq!(
            flush_and_reload(state, &file).session.panes,
            Some(saved),
            "one failed PTY spawn must NOT delete a valid pane from the saved layout — \
             the user has no way to get it back, and the failure may well be transient"
        );
    }

    /// 최악의 경우: 분할의 **양쪽이 다** 실패한다. 저하된 재구성을 저장하면
    /// 저장된 레이아웃 전체가 빈 값으로 덮인다.
    #[test]
    fn a_restore_in_which_everything_fails_does_not_wipe_the_saved_layout() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("data.json");
        let saved = split(
            PersistedAxis::Horizontal,
            0.6,
            leaf("/tmp/wt-a"),
            leaf("/tmp/wt-b"),
        );
        let mut state =
            restoring_state_with_disk(&file, saved.clone(), &["/tmp/wt-a", "/tmp/wt-b"]);

        deliver_start(&mut state, "/tmp/wt-a", false);
        deliver_start(&mut state, "/tmp/wt-b", false);

        assert_eq!(restored_shape(&state), "-", "precondition: nothing came up");
        assert!(state.hydration.is_open(), "precondition: the gate still opens");

        assert_eq!(
            flush_and_reload(state, &file).session.panes,
            Some(saved),
            "a boot where every session failed to start must leave the saved layout \
             untouched — overwriting it with nothing is unrecoverable data loss"
        );
    }

    /// **대조군: 사용자가 레이아웃을 편집하면 화면이 진실이 된다.** 보존이 영원히
    /// 붙어 있으면 사용자가 pane을 닫아도 다시 살아 돌아온다.
    #[test]
    fn once_the_user_edits_the_layout_the_live_tree_is_what_gets_saved() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("data.json");
        let saved = split(
            PersistedAxis::Vertical,
            0.25,
            leaf("/tmp/wt-a"),
            leaf("/tmp/wt-b"),
        );
        let mut state =
            restoring_state_with_disk(&file, saved.clone(), &["/tmp/wt-a", "/tmp/wt-b"]);
        deliver_start(&mut state, "/tmp/wt-a", true);
        deliver_start(&mut state, "/tmp/wt-b", true);

        // 사용자가 pane 하나를 닫는다.
        let pane = *state
            .panes()
            .expect("two panes")
            .iter()
            .find(|(_, id)| state.session_worktrees.get(id) == Some(&wt("/tmp/wt-a")))
            .map(|(pane, _)| pane)
            .expect("wt-a has a pane");
        let _ = state.update(Message::PaneCloseRequested(pane));

        assert_eq!(
            flush_and_reload(state, &file).session.panes,
            Some(leaf("/tmp/wt-b")),
            "control: a deliberate close must reach disk — preservation protects against \
             degraded restores, not against the user"
        );
    }

    /// 복원이 **끝난 뒤에** 일어난 시작 실패도 저장된 레이아웃을 건드리면 안 된다.
    ///
    /// 복원 중의 실패는 `preserved_layout`이 아직 `None`이라 구조적으로 안전하지만
    /// (보존은 모든 잎이 결정된 뒤에 세워진다), 복원이 끝난 뒤 사용자가 연
    /// worktree의 스폰이 실패하는 창은 **다르다** — 그때는 보존이 살아 있다.
    /// 규칙은 창과 무관하게 하나다: **시작 실패는 소멸의 증거가 아니다.**
    #[test]
    fn a_start_failure_after_the_restore_still_does_not_prune_the_saved_layout() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("data.json");
        let saved = split(
            PersistedAxis::Vertical,
            0.25,
            leaf("/tmp/wt-a"),
            leaf("/tmp/wt-b"),
        );
        let mut state =
            restoring_state_with_disk(&file, saved.clone(), &["/tmp/wt-a", "/tmp/wt-b"]);
        deliver_start(&mut state, "/tmp/wt-a", true);
        deliver_start(&mut state, "/tmp/wt-b", true);
        assert!(
            state.preserved_layout.is_some(),
            "precondition: the restore finished and the original is preserved"
        );

        // 복원 밖에서 세션 하나가 실패한다(사용자가 연 worktree의 스폰 실패).
        let failed_id = state.session_store.next_id();
        let _ = state.update(Message::SessionStarted {
            id: failed_id,
            worktree_id: wt("/tmp/wt-a"),
            result: Err("pty spawn failed".to_string()),
        });

        // **메모리 상태를 직접 본다.** 실패 핸들러 자체는 저장하지 않으므로
        // 디스크만 보면 손상이 아직 안 보인다 — 그래도 손상은 이미 일어났고,
        // 다음 사용자 조작이 그것을 디스크로 옮긴다.
        assert_eq!(
            state.preserved_layout,
            Some(saved.clone()),
            "the failed spawn must not have touched the preserved tree in memory"
        );

        // 그리고 실제로 다음 저장이 일어나면 원본이 그대로 나가야 한다.
        state.persist();
        assert_eq!(
            flush_and_reload(state, &file).session.panes,
            Some(saved),
            "a failed spawn is never evidence that a worktree is gone, whichever window \
             it happens in — only an authoritative listing may remove a leaf"
        );
    }

    /// **권위 있는 소멸만이 저장된 잎을 자동으로 지운다.** 시작 실패와 달리
    /// 이건 증거다.
    #[test]
    fn an_authoritative_disappearance_does_prune_the_preserved_layout() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("data.json");
        let saved = split(
            PersistedAxis::Vertical,
            0.25,
            leaf("/tmp/wt-a"),
            leaf("/tmp/wt-b"),
        );
        let mut state =
            restoring_state_with_disk(&file, saved, &["/tmp/wt-a", "/tmp/wt-b"]);
        deliver_start(&mut state, "/tmp/wt-a", true);
        deliver_start(&mut state, "/tmp/wt-b", true);

        // git이 wt-a가 사라졌다고 **권위 있게** 알린다. 헬퍼가 아니라 실제
        // 메시지를 태운다 — `persist()`는 `update`의 핸들러에 있으므로
        // 헬퍼로는 저장이 일어나지 않아 이 단언이 무엇도 검사하지 못한다.
        let repo_id = RepoId("/tmp/restore-repo".into());
        state.note_list_issued(repo_id.clone(), OpId(2));
        let _ = state.update(Message::WorktreesListed {
            request: OpId(2),
            repo_id,
            result: WorktreeListing::Authoritative(vec![entry_at("/tmp/wt-b", "wt-b")]),
        });

        assert_eq!(
            flush_and_reload(state, &file).session.panes,
            Some(leaf("/tmp/wt-b")),
            "a worktree confirmed gone by an authoritative listing IS removed from the \
             saved layout — otherwise a deleted worktree haunts the layout forever"
        );
    }

    /// **실제 경로로** 확인한다: 리사이즈 메시지가 NaN을 나르고, 그것이 저장돼
    /// 디스크에 닿은 뒤, 그 파일이 **다시 읽히는지**. 순수 함수만 검사하면
    /// 이 버그의 요점(순수 함수는 멀쩡해 보인다)을 그대로 놓친다.
    #[test]
    fn a_nan_resize_does_not_render_the_data_file_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("data.json");
        let (mut state, _id, _wt, _pane) = state_with_two_open_sessions_wired(&file);
        state.upsert_repo(some_repo("survivor"));

        let split_id = *state
            .panes()
            .expect("two panes")
            .layout()
            .splits()
            .next()
            .expect("one split");
        // iced가 높이 0인 분할 영역에서 실제로 만들어내는 값이다.
        let _ = state.update(Message::PaneResized(pane_grid::ResizeEvent {
            split: split_id,
            ratio: 0.0 / 0.0,
        }));
        let _ = state.update(Message::LayoutPersistDue { generation: 1 });

        // **살아 있는 트리도 깨끗해야 한다.** 저장 경로(`quantize_ratio`)가
        // 어차피 고치므로 디스크만 보면 이 clamp가 없어도 통과한다 — 그러면
        // 오염된 비율이 그 프레임의 레이아웃 계산에 그대로 들어간다.
        fn ratios_are_finite(node: &pane_grid::Node) -> bool {
            match node {
                pane_grid::Node::Pane(_) => true,
                pane_grid::Node::Split { ratio, a, b, .. } => {
                    ratio.is_finite() && ratios_are_finite(a) && ratios_are_finite(b)
                }
            }
        }
        assert!(
            ratios_are_finite(state.panes().expect("panes").layout()),
            "a NaN ratio must never enter the live pane_grid — every layout computation              in that frame becomes NaN"
        );

        let reloaded = flush_and_reload(state, &file);
        assert_eq!(
            reloaded.repos.len(),
            1,
            "the whole document must survive — a null ratio makes parse_trusted reject \
             the file, taking repos, worktree metadata and settings with it, and the \
             backup rotation then overwrites every slot with the corrupt copy"
        );
        assert!(
            reloaded.session.panes.is_some(),
            "and the layout itself must still be there"
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
                linked_github_pr: None,
                linked_linear_issue: None,
                linked_linear_issue_workspace_id: None,
                linked_linear_issue_organization_url_key: None,
                linked_jira_issue: None,
                linked_jira_site: None,
            },
            Worktree {
                id: wt("/tmp/wt-b"),
                repo_id: RepoId("/tmp/booted".into()),
                path: PathBuf::from("/tmp/wt-b"),
                branch: "b".into(),
                display_name: "b".into(),
                created_with_agent: None,
                created_at_unix_ms: 2,
                linked_github_pr: None,
                linked_linear_issue: None,
                linked_linear_issue_workspace_id: None,
                linked_linear_issue_organization_url_key: None,
                linked_jira_issue: None,
                linked_jira_site: None,
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

    /// **precedence: 훅 > 타이틀.** 로그인 셸(`Custom`→`OscTitle`) 안에서 claude를
    /// 띄우면 그 세션은 훅과 OSC-title을 **둘 다** 낸다. 훅이 permission 대기
    /// (`Waiting`)를 세운 뒤 claude의 `✳ Claude Code`(→`Done`) idle 타이틀이 폴에
    /// 잡혀도 배지는 **`Waiting`을 유지**해야 한다 — 안 그러면 MVP가 검증한 주황 배지가
    /// 회색(`Done`)으로 덮이고, 거부 후엔 훅이 오지 않아 회색에 고착한다.
    #[test]
    fn a_title_never_overwrites_a_pane_that_has_received_hooks() {
        let mut state = state_with_badge("/tmp/wt-a", 1);
        // 훅이 permission 대기를 세운다 → received_hook=true, hook=Waiting.
        let _ = state.update(Message::HookArrived(hook(
            "/tmp/wt-a",
            1,
            HookEventName::PermissionRequest,
        )));
        assert_eq!(
            state.worktree_badge(&wt("/tmp/wt-a")),
            BadgeState::Waiting,
            "precondition: the hook set the badge to Waiting"
        );

        // claude가 로그인 셸에서 내보내는 idle 타이틀(`✳ Claude Code`→Done)이 폴에
        // 잡힌 것과 동일하게, 타이틀-파생 Done을 배지에 반영 시도한다.
        state.note_title_status_for_badge(&wt("/tmp/wt-a"), HookState::Done);
        assert_eq!(
            state.badges[&wt("/tmp/wt-a")].hook.map(|(s, _)| s),
            Some(HookState::Waiting),
            "a hook-owned pane must ignore the title — the ✳ idle title must not clobber the \
             authoritative Waiting hook state"
        );
        assert_eq!(
            state.worktree_badge(&wt("/tmp/wt-a")),
            BadgeState::Waiting,
            "the badge stays orange (Waiting); the title path does not reach a hook-owned pane"
        );

        // 대조군: 훅을 한 번도 못 본 pane(진짜 non-hook 에이전트 — 로그인 셸의
        // codex/goose 등)은 타이틀 경로가 그대로 작동한다. 이게 없으면 위 단언이
        // "타이틀 경로가 아예 죽었다"로도 설명된다.
        let mut fresh = state_with_badge("/tmp/wt-b", 1);
        fresh.note_title_status_for_badge(&wt("/tmp/wt-b"), HookState::Done);
        assert_eq!(
            fresh.badges[&wt("/tmp/wt-b")].hook.map(|(s, _)| s),
            Some(HookState::Done),
            "control: a pane that never received a hook still takes title-derived status"
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
            presence: AgentPresence::Agent("claude"),
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
            presence: AgentPresence::Agent("claude"),
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

    /// 버려진 훅 이벤트가 **틀린 배지를 남긴다.** 버려진 것이 마지막
    /// `PermissionRequest`나 `Stop`이면 폴링은 계속 `Agent`를 보고, 잃어버린
    /// `Waiting`은 재구성할 수 없다 — "다음 이벤트가 곧 고친다"는 항상 참이 아니다.
    #[test]
    fn dropped_hook_events_invalidate_the_badges_they_may_have_belonged_to() {
        let mut state = state_with_badge("/tmp/wt-a", 1);
        state
            .badges
            .insert(wt("/tmp/wt-b"), PaneBadge::new(SpawnNonce(1)));
        for w in ["/tmp/wt-a", "/tmp/wt-b"] {
            let _ = state.update(Message::HookArrived(hook(
                w,
                1,
                HookEventName::PermissionRequest,
            )));
        }
        assert!(
            state.badges.values().all(|b| b.hook.is_some()),
            "precondition: both panes have a hook state"
        );

        // 드롭이 없으면 아무것도 무효화하지 않는다.
        state.apply_hook_drops(0);
        assert!(
            state.badges.values().all(|b| b.hook.is_some()),
            "control: with no drops the badges must be left alone"
        );

        state.apply_hook_drops(3);
        assert!(
            state.badges.values().all(|b| b.hook.is_none()),
            "once events have been dropped we no longer know these badges are right — \
             the dropping side does not know which pane it dropped, so all of them go to \
             Unknown. Confidently showing a wrong state is worse than admitting we lost it"
        );
        assert!(
            state.last_error().is_some_and(|e| e.contains("dropped")),
            "and the loss must be surfaced, not silent"
        );

        // 같은 값으로 다시 불러도 이미 반영했으므로 재무효화하지 않는다.
        let _ = state.update(Message::HookArrived(hook(
            "/tmp/wt-a",
            1,
            HookEventName::PreToolUse,
        )));
        state.apply_hook_drops(3);
        assert!(
            state.badges[&wt("/tmp/wt-a")].hook.is_some(),
            "an already-accounted-for drop count must not keep wiping fresh hook state"
        );
    }

    /// **닫힌 pane의 배지가 `Waiting`에 굳으면 안 된다.**
    ///
    /// 세션이 사라지면 presence는 `Unknown`으로 떨어지고, 리듀서의 `Unknown` 팔은
    /// 훅을 그대로 신뢰한다. 마지막 훅이 `Waiting`이었다면 **`Waiting`은 나이로
    /// 감쇠하지 않으므로** 사이드바 행(git 목록으로 그려져 세션과 무관하게
    /// 살아남는다)에 주황색 표시가 영구히 박힌다. `Exited` 행은 이걸 못 막는다 —
    /// presence가 `Exited`로 관측될 일이 아예 없기 때문이다.
    #[test]
    fn closing_a_pane_does_not_strand_its_badge_on_waiting() {
        let (mut state, _id, worktree_id, pane) = state_with_one_open_session();
        state
            .badges
            .insert(worktree_id.clone(), PaneBadge::new(SpawnNonce(1)));
        let _ = state.update(Message::HookArrived(HookEvent {
            pane_key: PaneKey(worktree_id.clone()),
            spawn_nonce: SpawnNonce(1),
            claude_session_id: "s".into(),
            event: HookEventName::PermissionRequest,
            tool_name: None,
            agent_id: None,
            background_tasks_empty: None,
        }));
        assert_eq!(
            state.worktree_badge(&worktree_id),
            BadgeState::Waiting,
            "precondition: the badge is Waiting while the session lives"
        );

        let _ = state.update(Message::PaneCloseRequested(pane));

        assert_eq!(
            state.worktree_badge(&worktree_id),
            BadgeState::Unknown,
            "with the session gone the honest answer is Unknown — leaving Waiting puts a \
             permanent orange 'needs you' marker on a worktree nobody is working in, and \
             nothing can ever clear it because Waiting does not decay with age"
        );
        assert!(
            !state.badges.contains_key(&worktree_id),
            "and the ledger entry must be gone, not merely ignored — otherwise `badges` \
             grows for the lifetime of the app"
        );
    }

    /// **세션이 없는 worktree**의 배지도 정리돼야 한다.
    ///
    /// 세션이 있으면 `close_session`이 지우므로 이 경로가 죽어 있어도 티가 나지
    /// 않는다(mutation으로 확인: 세션이 있는 픽스처에서는 뮤턴트가 살아남았다).
    /// 세션 없이 배지만 남은 경우가 이 줄이 유일하게 책임지는 상황이다.
    #[test]
    fn a_vanished_worktree_with_no_session_still_drops_its_badge_ledger() {
        let mut state = AppState::default();
        let repo_id = RepoId("/tmp/r-prune".into());
        state.note_list_issued(repo_id.clone(), OpId(1));
        state.apply_authoritative_listing(
            repo_id.clone(),
            OpId(1),
            vec![entry_at("/tmp/wt-ghost", "ghost")],
        );
        let worktree_id = wt("/tmp/wt-ghost");
        // 배지만 있고 세션은 없다 — 세션이 이미 닫힌 뒤 목록이 갱신되는 순서다.
        state
            .badges
            .insert(worktree_id.clone(), PaneBadge::new(SpawnNonce(1)));
        assert!(
            !state.worktree_sessions.contains_key(&worktree_id),
            "precondition: no session, so close_session cannot do the cleanup for us"
        );

        state.note_list_issued(repo_id.clone(), OpId(2));
        state.apply_authoritative_listing(repo_id, OpId(2), Vec::new());

        assert!(
            !state.badges.contains_key(&worktree_id),
            "the vanish path removes worktree_meta for exactly this reason; badges must \
             follow or the map grows for the lifetime of the app"
        );
    }

    // ---- 복원 워치독: 도착하지 않는 결과 ----

    /// **결과가 영영 오지 않는 잎**(세션 스폰 워커가 패닉하면 그렇게 된다)이
    /// 하이드레이션 게이트를 프로세스 수명 내내 닫아두면, 사용자의 **모든**
    /// 조작이 조용히 저장되지 않는다.
    #[test]
    fn a_leaf_whose_outcome_never_arrives_cannot_wedge_the_gate_forever() {
        let tree = split(
            PersistedAxis::Vertical,
            0.25,
            leaf("/tmp/wt-a"),
            leaf("/tmp/wt-b"),
        );
        let mut state = restoring_state(tree, &["/tmp/wt-a", "/tmp/wt-b"]);
        let generation = state
            .restore
            .as_ref()
            .expect("a restore is in flight")
            .generation;

        // wt-a만 보고한다. wt-b의 워커는 죽었다고 하자 — 아무것도 오지 않는다.
        deliver_start(&mut state, "/tmp/wt-a", true);
        assert!(
            !state.hydration.is_open(),
            "precondition: one leaf is still outstanding, so the gate is shut"
        );

        // 워치독이 만료된다(타이머 자체는 기다리지 않고 메시지를 직접 태운다).
        let _ = state.update(Message::RestoreWatchdog { generation });

        assert!(
            state.hydration.is_open(),
            "the watchdog must complete the restore so saving is possible again — a leaf \
             with no path to completion otherwise disables persistence for the entire \
             process, silently"
        );
        assert_eq!(
            restored_shape(&state),
            "wt-a",
            "and the leaves that did report must still be honoured"
        );
    }

    /// 낡은 워치독이 그 사이 시작된 **다른** 복원을 잘라버리면 안 된다.
    #[test]
    fn a_stale_watchdog_does_not_cut_short_a_later_restore() {
        let mut state = restoring_state(leaf("/tmp/wt-a"), &["/tmp/wt-a"]);
        let first = state
            .restore
            .as_ref()
            .expect("restore in flight")
            .generation;

        // 낡은 세대의 워치독이 뒤늦게 도착한다.
        let _ = state.update(Message::RestoreWatchdog {
            generation: first.wrapping_sub(1),
        });

        assert!(
            state.restore.is_some(),
            "a watchdog from an earlier restore must not terminate the current one"
        );
        assert!(!state.hydration.is_open(), "so the gate stays shut");

        // 대조군: 맞는 세대는 실제로 끝낸다.
        let _ = state.update(Message::RestoreWatchdog {
            generation: first,
        });
        assert!(state.hydration.is_open(), "control: the matching generation completes it");
    }

    // ---- presence → 배지 장부 ----

    /// **확정되지 않은 `NoAgent`는 이전 배지를 그대로 든다.** 셸이 exec하는 동안
    /// 포그라운드를 잠깐 쥐는 전이라 한 틱에 반응하면 배지가 깜빡인다.
    ///
    /// `repeated_no_agent_polls_eventually_confirm_done`은 이걸 검사하지 못한다 —
    /// 거기서는 훅이 `None`이고 `previous`가 내내 `Unknown`이라, 미만 구간이
    /// `Unknown`을 하드코딩해도 통과한다(공허하게 참이다). 실제로 **유지되는
    /// 값이 있을 때** 유지되는지를 봐야 한다.
    #[test]
    fn an_unconfirmed_no_agent_streak_holds_the_working_badge() {
        let (mut state, id, worktree_id, _pane) = state_with_one_open_session();
        state
            .badges
            .insert(worktree_id.clone(), PaneBadge::new(SpawnNonce(1)));

        let _ = state.update(Message::PresenceReady {
            id,
            generation: 1,
            presence: AgentPresence::Agent("claude"),
        });
        let _ = state.update(Message::HookArrived(hook(
            worktree_id.0.as_str(),
            1,
            HookEventName::PreToolUse,
        )));
        assert_eq!(
            state.worktree_badge(&worktree_id),
            BadgeState::Working,
            "precondition: the agent is working"
        );

        for _ in 0..(NO_AGENT_CONFIRMATIONS - 1) {
            let _ = state.update(Message::PresenceReady {
                id,
                generation: 1,
                presence: AgentPresence::NoAgent,
            });
        }

        assert_eq!(
            state.worktree_badge(&worktree_id),
            BadgeState::Working,
            "below the threshold the badge must HOLD at Working — if `previous` is never \
             updated the whole anti-flicker mechanism is dead and this reads Unknown"
        );
    }

    /// **훅이 나이로 감쇠하는 전이는 폴링만이 본다.** 훅 도착 시점에 갱신하는
    /// 것만으로는 부족하다: `Working`이 [`HOOK_STALE_AFTER`]를 넘겨 `Unknown`이
    /// 되는 것은 시간이 흐른 결과라 새 훅 이벤트가 없고, 그래서 **폴링 쪽
    /// 갱신이 없으면 `previous`가 낡은 `Working`에 머문다** — 그 뒤 `NoAgent`가
    /// 오면 오래전에 죽은 것을 "일하는 중"으로 든다.
    #[test]
    fn a_presence_poll_refreshes_the_held_badge_after_the_hook_goes_stale() {
        let (mut state, id, worktree_id, _pane) = state_with_one_open_session();
        let mut badge = PaneBadge::new(SpawnNonce(1));
        // 이미 오래된 `Working` 훅. 훅 경로는 다시 지나지 않는다.
        badge.hook = Some((
            HookState::Working,
            Instant::now() - (HOOK_STALE_AFTER + Duration::from_secs(60)),
        ));
        badge.previous = BadgeState::Working;
        state.badges.insert(worktree_id.clone(), badge);

        let _ = state.update(Message::PresenceReady {
            id,
            generation: 1,
            presence: AgentPresence::Agent("claude"),
        });
        assert_eq!(
            state.badges[&worktree_id].previous,
            BadgeState::Unknown,
            "the poll must re-evaluate the age rule and refresh what will be held"
        );

        let _ = state.update(Message::PresenceReady {
            id,
            generation: 1,
            presence: AgentPresence::NoAgent,
        });
        assert_eq!(
            state.worktree_badge(&worktree_id),
            BadgeState::Unknown,
            "so an unconfirmed NoAgent holds Unknown, not a Working badge that expired              four minutes ago"
        );
    }

    /// **`Unknown` 관측은 streak를 리셋하지 않는다.** `ps`가 간헐적으로 실패해
    /// `NoAgent`/`Unknown`이 번갈아 오면, 리셋하는 구현에서는 임계에 영영 닿지
    /// 못해 pane이 `Done`으로 정착하지 못한다(굶는다).
    #[test]
    fn interleaved_unknown_polls_do_not_starve_the_no_agent_streak() {
        let (mut state, id, worktree_id, _pane) = state_with_one_open_session();
        state
            .badges
            .insert(worktree_id.clone(), PaneBadge::new(SpawnNonce(1)));

        // **마지막 관측이 `NoAgent`여야 한다.** `Unknown`으로 끝내면 배지는
        // 정당하게 `Unknown`이고(마지막에 아는 것이 없다), 그러면 이 테스트는
        // streak가 아니라 그 사실을 검사하게 된다.
        for i in 0..NO_AGENT_CONFIRMATIONS {
            if i > 0 {
                // 관측 실패가 사이사이 끼어든다.
                let _ = state.update(Message::PresenceReady {
                    id,
                    generation: 1,
                    presence: AgentPresence::Unknown,
                });
            }
            let _ = state.update(Message::PresenceReady {
                id,
                generation: 1,
                presence: AgentPresence::NoAgent,
            });
        }

        assert_eq!(
            state.badges[&worktree_id].no_agent_streak,
            NO_AGENT_CONFIRMATIONS,
            "an Unknown observation means 'we could not tell', not 'the agent is back' — \
             resetting on it lets a flaky ps starve the streak forever"
        );
        assert_eq!(
            state.worktree_badge(&worktree_id),
            BadgeState::Done,
            "so the pane still settles on Done"
        );
    }

    // ---- 주입: 복원된 세션도 훅 env를 받는가 ----

    /// **`spawn_env()`를 따로 테스트하는 것으로는 부족하다.** 그 함수는 늘 옳았고,
    /// 버그는 호출 시점에 `hook_endpoint`가 아직 `None`이라 **아예 불리지 않은
    /// 것**이었다. 그래서 여기서는 세션에 실제로 심긴 env를 본다.
    #[test]
    fn a_restored_session_is_spawned_with_the_hook_environment() {
        let mut state = AppState::default();
        // 서버가 세션 시작 **전에** 붙어 있어야 한다 — `boot()`이 강제하는 순서다.
        state.hook_endpoint = Some((51234, "tok-abc".to_string()));

        let repo_id = RepoId("/tmp/r-env".into());
        state.note_list_issued(repo_id.clone(), OpId(1));
        state.apply_authoritative_listing(
            repo_id,
            OpId(1),
            vec![entry_at("/nonexistent-suaegi-env-test", "e")],
        );
        state.pending_restore_tree = Some(leaf("/nonexistent-suaegi-env-test"));
        state.hydration = Hydration::new([]);

        let _ = state.begin_layout_restore();

        let env = state
            .session_store()
            .last_spawn_env()
            .expect("the restore must have spawned a session")
            .to_vec();
        let get = |k: &str| {
            env.iter()
                .find(|(name, _)| name == k)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(
            get("SUAEGI_HOOK_PORT"),
            Some("51234".to_string()),
            "a session started by layout RESTORE must receive the hook port — if the \
             endpoint is attached after boot(), every pane after a restart comes up with \
             no hook environment and its badge is Unknown forever"
        );
        assert_eq!(get("SUAEGI_HOOK_TOKEN"), Some("tok-abc".to_string()));
        assert_eq!(get("SUAEGI_SPAWN_NONCE").is_some(), true);
        assert!(
            get("SUAEGI_PANE_KEY").is_some_and(|k| !k.contains('/')),
            "and the pane key must be planted already base64url-encoded"
        );
    }

    /// 6c 핵심 검증: worktree가 claude로 생성됐으면 세션 시작이 **claude를 직접**
    /// 띄운다 — program=claude, cwd=worktree, 그리고 SUAEGI_* 훅 env가 **함께** 심긴다.
    /// 이 셋이 한 번의 관측으로 성립해야 직접 실행 경로에서 훅이 발화할 근거가 된다.
    /// (실제 훅 POST가 뜨는지는 실 claude가 필요해 human-eyes 항목 — 보고서 참고.)
    #[test]
    fn a_claude_worktree_launches_claude_directly_with_the_hook_environment() {
        let mut state = AppState::default();
        // 세션 시작 전에 엔드포인트가 붙어 있어야 env가 심긴다(boot 순서와 동일).
        state.hook_endpoint = Some((51999, "tok-claude".to_string()));

        let path = "/nonexistent-suaegi-claude-launch";
        let repo_id = RepoId("/tmp/r-claude".into());
        state.note_list_issued(repo_id.clone(), OpId(1));
        state.apply_authoritative_listing(repo_id, OpId(1), vec![entry_at(path, "feat")]);
        // 이 worktree는 claude로 생성됐다 — 생성 메타에 그렇게 굳어 있다.
        state.worktree_meta.insert(
            wt(path),
            WorktreeMeta {
                created_with_agent: Some("claude".to_string()),
                created_at_unix_ms: 1,
                linked_github_pr: None,
                ..Default::default()
            },
        );

        let _ = state.update(Message::WorktreeSelected(wt(path)));

        let spawn = state
            .session_store()
            .last_spawn()
            .expect("selecting the worktree must have started a session")
            .clone();
        assert_eq!(
            spawn.program, "claude",
            "a claude worktree must launch the claude program directly, not a login shell"
        );
        assert_eq!(
            spawn.cwd.as_deref(),
            Some(std::path::Path::new(path)),
            "claude must be launched with cwd set to the worktree so it reads that \
             worktree's .claude/settings.local.json"
        );
        let get = |k: &str| {
            spawn
                .env
                .iter()
                .find(|(name, _)| name == k)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(
            get("SUAEGI_HOOK_PORT"),
            Some("51999".to_string()),
            "the direct claude launch must still carry the hook env, or 6b-A's hook \
             precedence can never fire"
        );
        assert_eq!(get("SUAEGI_HOOK_TOKEN"), Some("tok-claude".to_string()));
        assert!(get("SUAEGI_PANE_KEY").is_some());
    }

    /// 대조군: 에이전트를 안 고른 worktree는 예전 그대로 **로그인 셸**로 뜬다
    /// (program이 claude가 아니다). 위 테스트가 "라우팅이 옳다"임을 보장한다.
    #[test]
    fn a_worktree_without_a_chosen_agent_still_launches_a_login_shell() {
        let mut state = AppState::default();
        let path = "/nonexistent-suaegi-login-shell";
        let repo_id = RepoId("/tmp/r-shell".into());
        state.note_list_issued(repo_id.clone(), OpId(1));
        state.apply_authoritative_listing(repo_id, OpId(1), vec![entry_at(path, "feat")]);
        // worktree_meta 없음 = created_with_agent None = 로그인 셸.

        let _ = state.update(Message::WorktreeSelected(wt(path)));

        let spawn = state
            .session_store()
            .last_spawn()
            .expect("selecting the worktree must have started a session")
            .clone();
        assert_ne!(
            spawn.program, "claude",
            "no agent chosen must keep the login-shell default, unchanged from before 6c"
        );
        #[cfg(unix)]
        assert!(
            spawn.args.iter().any(|a| a == "-l"),
            "the default must be a login shell"
        );
    }

    // ── 6b-B: 초기 프롬프트 주입 ──────────────────────────────────────────

    /// (a) argv 에이전트(claude) + 프롬프트 → 프롬프트가 **스폰 인자**에 들어간다.
    /// 그리고 `pending_prompts`가 소비돼(one-shot) 재시작해도 다시 실리지 않는다.
    /// argv 경로는 게이트를 무장하지 **않는다**(주입이 이미 argv로 끝났다).
    #[test]
    fn an_argv_agent_launches_with_the_initial_prompt_in_spawn_args() {
        let mut state = AppState::default();
        let path = "/nonexistent-suaegi-argv-prompt";
        let repo_id = RepoId("/tmp/r-argv".into());
        state.note_list_issued(repo_id.clone(), OpId(1));
        state.apply_authoritative_listing(repo_id, OpId(1), vec![entry_at(path, "feat")]);
        state.worktree_meta.insert(
            wt(path),
            WorktreeMeta {
                created_with_agent: Some("claude".to_string()),
                created_at_unix_ms: 1,
                linked_github_pr: None,
                ..Default::default()
            },
        );
        // 이 worktree엔 일회성 프롬프트가 걸려 있다(create가 담아둔 것).
        state
            .pending_prompts
            .insert(wt(path), "fix the flaky test".to_string());

        let _ = state.update(Message::WorktreeSelected(wt(path)));

        let spawn = state
            .session_store()
            .last_spawn()
            .expect("selecting the worktree must have started a session")
            .clone();
        assert_eq!(spawn.program, "claude");
        assert!(
            spawn.args.iter().any(|a| a == "fix the flaky test"),
            "an argv agent must carry the prompt as a spawn argument, got {:?}",
            spawn.args
        );
        assert!(
            !state.pending_prompts.contains_key(&wt(path)),
            "the one-shot prompt must be consumed so a restart never re-injects it"
        );
        // argv 경로는 stdin 게이트를 쓰지 않는다.
        assert!(
            state.pending_injections.is_empty() && state.prompt_gates.is_empty(),
            "an argv agent injects at spawn — it must not arm the stdin gate"
        );
    }

    /// stdin-after-start 에이전트(aider)는 프롬프트를 argv로 받지 않는다 —
    /// bare로 뜨고 대신 주입이 **대기**한다(`pending_injections`). 세션이 실제로
    /// 살아난 뒤에야 게이트로 옮겨진다(`SessionStarted`).
    #[test]
    fn a_stdin_agent_launches_bare_and_queues_the_injection_instead_of_argv() {
        let mut state = AppState::default();
        let path = "/nonexistent-suaegi-stdin-prompt";
        let repo_id = RepoId("/tmp/r-stdin".into());
        state.note_list_issued(repo_id.clone(), OpId(1));
        state.apply_authoritative_listing(repo_id, OpId(1), vec![entry_at(path, "feat")]);
        state.worktree_meta.insert(
            wt(path),
            WorktreeMeta {
                created_with_agent: Some("aider".to_string()),
                created_at_unix_ms: 1,
                linked_github_pr: None,
                ..Default::default()
            },
        );
        state
            .pending_prompts
            .insert(wt(path), "write the tests".to_string());

        let _ = state.update(Message::WorktreeSelected(wt(path)));

        let spawn = state
            .session_store()
            .last_spawn()
            .expect("selecting the worktree must have started a session")
            .clone();
        assert_eq!(spawn.program, "aider");
        assert!(
            !spawn.args.iter().any(|a| a == "write the tests"),
            "a stdin-after-start agent must launch bare — the prompt must NOT be in argv, \
             got {:?}",
            spawn.args
        );
        assert!(
            !state.pending_prompts.contains_key(&wt(path)),
            "the one-shot prompt is consumed at start"
        );
        assert_eq!(
            state.pending_injections.values().collect::<Vec<_>>(),
            vec![&"write the tests".to_string()],
            "the prompt must be queued for post-spawn injection, keyed by the new session"
        );
    }

    /// 초기 프롬프트는 **영속화되지 않는다.** 생성 시 담긴 프롬프트는 메모리
    /// (`pending_prompts`)에만 살고, 디스크로 나가는 스냅샷에는 그 문자열이
    /// 어디에도 없어야 한다 — 복원된 세션이 낡은 프롬프트를 다시 주입하면 안 된다.
    #[test]
    fn the_initial_prompt_is_carried_in_memory_but_never_persisted() {
        let mut state = AppState::default();
        let repo_id = RepoId("/tmp/r-persist".into());
        state.upsert_repo(some_repo("r-persist"));

        let _ = state.update(Message::WorktreeCreated {
            request: OpId(1),
            repo_id,
            created_with_agent: Some("aider".to_string()),
            initial_prompt: Some("SECRET-PROMPT-TOKEN".to_string()),
            result: Ok(CreatedWorktree {
                path: PathBuf::from("/tmp/wt-persist"),
                branch: "feat".into(),
                display_name: "feat".into(),
            }),
        });

        // 메모리에는 담겼다(첫 세션 시작이 쓸 값).
        assert_eq!(
            state.pending_prompts.get(&wt("/tmp/wt-persist")),
            Some(&"SECRET-PROMPT-TOKEN".to_string()),
            "the prompt must be held in memory for the first session start"
        );
        // 그러나 디스크로 나가는 스냅샷 어디에도 그 문자열이 없어야 한다.
        let json = serde_json::to_string(&state.persisted_snapshot()).unwrap();
        assert!(
            !json.contains("SECRET-PROMPT-TOKEN"),
            "the one-shot prompt must never reach persisted state: {json}"
        );
    }

    /// 사용자가 타이핑을 시작하면 무장된 주입이 **취소된다** — 이미 치고 있는데
    /// 프롬프트가 끼어들면 입력이 뒤섞인다. 실제 취소는 `dispatch_term_command`의
    /// `Key`/`Paste` 팔에 있으므로, 살아 있는 세션(guard를 통과해야 팔에 닿는다)이
    /// 필요하다.
    #[test]
    fn user_typing_cancels_a_pending_injection() {
        use suaegi_term::input_types::{KeyInput, KeyLocation, Mods, TermKey};

        let mut state = AppState::default();
        // 살아 있는 세션을 하나 만든다(해가 없는 sleep). 그런 뒤 그 세션에 주입을
        // 무장한 상태를 손으로 세운다.
        #[cfg(unix)]
        let cmd = ("sleep".to_string(), vec!["5".to_string()]);
        #[cfg(windows)]
        let cmd = (
            "cmd".to_string(),
            vec!["/C".to_string(), "ping -n 6 127.0.0.1 > nul".to_string()],
        );
        let session_id = state
            .session_store_mut()
            .start_for_test_with_agent(cmd, Some("aider"));
        state
            .prompt_gates
            .insert(session_id, PromptGate::new("the prompt".to_string()));
        state
            .pending_injections
            .insert(session_id, "the prompt".to_string());

        // 사용자가 키를 하나 친다 → 주입 취소.
        let key = KeyInput {
            key: TermKey::Char('h'),
            physical_latin: None,
            location: KeyLocation::Standard,
            mods: Mods::default(),
            text: Some("h".to_string()),
            repeat: false,
        };
        let _ = state.dispatch_term_command(session_id, crate::terminal::contract::TermCommand::Key(key));
        assert!(
            !state.pending_injections.contains_key(&session_id)
                && !state.prompt_gates.contains_key(&session_id),
            "typing must cancel both the queued injection and any armed gate"
        );
    }

    /// 하드 타임아웃으로 **포기한** 게이트는 회수되어야 한다 — 안 그러면
    /// `has_armed_prompt_gates()`가 계속 true라 presence tier가 세션 내내
    /// ACTIVE(750ms)에 고착된다. 게이트는 clock-주입이라 실제 8초를 기다리지 않고
    /// `started`를 과거로 세워 다음 poll이 곧바로 타임아웃에 걸리게 한다.
    #[test]
    fn a_gate_that_gives_up_is_reclaimed_so_the_tier_returns_to_idle() {
        use std::time::{Duration, Instant};

        let mut state = AppState::default();
        // poll이 관측할 살아 있는 세션이 필요하다(해가 없는 sleep).
        #[cfg(unix)]
        let cmd = ("sleep".to_string(), vec!["5".to_string()]);
        #[cfg(windows)]
        let cmd = (
            "cmd".to_string(),
            vec!["/C".to_string(), "ping -n 6 127.0.0.1 > nul".to_string()],
        );
        let session_id = state
            .session_store_mut()
            .start_for_test_with_agent(cmd, Some("aider"));

        // 시계가 이미 타임아웃을 넘긴 게이트를 무장한다(과거 `started`).
        let past =
            Instant::now() - crate::prompt_inject::HARD_TIMEOUT - Duration::from_secs(1);
        state
            .prompt_gates
            .insert(session_id, PromptGate::armed_at("the prompt".to_string(), past));
        assert!(state.has_armed_prompt_gates(), "precondition: a gate is armed");
        assert_eq!(
            crate::presence_poll::tier(&state),
            crate::presence_poll::ACTIVE_TIER,
            "precondition: an armed gate bumps the tier to ACTIVE"
        );

        // 이 poll이 하드 타임아웃에 걸려 게이트를 포기·회수해야 한다.
        state.poll_prompt_injections();

        assert!(
            !state.has_armed_prompt_gates(),
            "a gave-up gate must be reclaimed — otherwise the tier stays ACTIVE forever"
        );
        assert_eq!(
            crate::presence_poll::tier(&state),
            crate::presence_poll::IDLE_TIER,
            "with the gate reclaimed and no agent present, the tier returns to idle"
        );
    }

    /// 이 기능보다 **먼저 만들어진** worktree도 설정 파일을 받아야 한다 —
    /// 생성 시점에만 쓰면 기존 worktree의 배지는 영구히 없다.
    #[test]
    fn starting_a_session_injects_settings_into_a_pre_existing_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("ws");
        let worktree = workspace.join("repo").join("wt-old");
        std::fs::create_dir_all(&worktree).unwrap();

        let mut state = AppState::default();
        state.workspace_root = workspace;
        state.hook_script = Some(dir.path().join("hook.sh"));
        let repo_id = RepoId("/tmp/r-old".into());
        state.note_list_issued(repo_id.clone(), OpId(1));
        state.apply_authoritative_listing(
            repo_id,
            OpId(1),
            vec![entry_at(worktree.to_str().unwrap(), "wt-old")],
        );

        let settings = worktree.join(".claude").join("settings.local.json");
        assert!(!settings.exists(), "precondition: it was never created");

        let _ = state.update(Message::WorktreeSelected(worktree_id_for(&worktree)));

        assert!(
            settings.exists(),
            "a worktree that predates this feature must get its settings when a session \
             starts — otherwise its badge can never work, and installing the shared \
             script at boot does not repair it"
        );
    }

    /// **밖에서 발견된 worktree에는 쓰지 않는다.** 우리 소유가 아닌 디렉터리에
    /// 우리 파일을 남기는 일이다.
    #[test]
    fn a_worktree_outside_our_workspace_is_never_written_to() {
        let dir = tempfile::tempdir().unwrap();
        let foreign = dir.path().join("someone-elses-repo");
        std::fs::create_dir_all(&foreign).unwrap();

        let mut state = AppState::default();
        state.workspace_root = dir.path().join("our-workspace");
        state.hook_script = Some(dir.path().join("hook.sh"));
        let repo_id = RepoId("/tmp/r-foreign".into());
        state.note_list_issued(repo_id.clone(), OpId(1));
        state.apply_authoritative_listing(
            repo_id,
            OpId(1),
            vec![entry_at(foreign.to_str().unwrap(), "theirs")],
        );

        let _ = state.update(Message::WorktreeSelected(worktree_id_for(&foreign)));

        assert!(
            !foreign.join(".claude").exists(),
            "we must not leave our config inside a directory we do not own — the cost is \
             that such a worktree's badge stays Unknown, and that is the right trade"
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
            // Plan 7a: 연결된 PR도 `created_at_unix_ms`와 같은 경로로 보존돼야 한다 —
            // `persisted_snapshot`이 매 저장마다 Worktree를 새로 합성하므로, 이 값이
            // `WorktreeMeta`로 씨딩·재주입되지 않으면 한 번 저장에 사라진다.
            linked_github_pr: Some(1234),
            // N1 §1.3: Linear 링크도 **정확히 같은 데이터-손실 계약**을 받는다 — 씨딩·재주입이
            // 없으면 한 번 저장에 사라진다(forge #14 클래스). 세 조각(식별자 + 좌표)을 다 심는다.
            linked_linear_issue: Some("ENG-42".into()),
            linked_linear_issue_workspace_id: Some("org-77".into()),
            linked_linear_issue_organization_url_key: Some("acme".into()),
            // N2 §2: Jira 링크도 **정확히 같은 데이터-손실 계약**을 받는다 — 씨딩·재주입 중 하나라도
            // 지우는 뮤턴트는 아래 두 단언에서 죽는다. 두 조각(키 + 사이트)을 다 심는다.
            linked_jira_issue: Some("PROJ-99".into()),
            linked_jira_site: Some("https://acme.atlassian.net".into()),
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
        assert_eq!(
            saved.worktrees[0].linked_github_pr,
            Some(1234),
            "the linked PR read from disk must be written back — if it is not seeded into \
             WorktreeMeta and re-injected, one save-reconstruction erases it (data-loss class)"
        );
        // **N1 데이터-손실 가드**: from_load 씨딩(state.rs ~933) 또는 persisted_snapshot
        // 재주입(~897) 중 하나라도 지우는 뮤턴트는 이 세 단언에서 죽는다.
        assert_eq!(
            saved.worktrees[0].linked_linear_issue.as_deref(),
            Some("ENG-42"),
            "the linked Linear issue read from disk must survive a save — seeded into \
             WorktreeMeta and re-injected, exactly like linked_github_pr (forge #14 class)"
        );
        assert_eq!(
            saved.worktrees[0].linked_linear_issue_workspace_id.as_deref(),
            Some("org-77"),
            "the Linear workspace coordinate must survive too (deep-link/reconnect)"
        );
        assert_eq!(
            saved.worktrees[0]
                .linked_linear_issue_organization_url_key
                .as_deref(),
            Some("acme"),
            "the Linear url_key coordinate must survive too"
        );
        // **N2 데이터-손실 가드**: from_load 씨딩(linked_jira_*) 또는 persisted_snapshot 재주입
        // 중 하나라도 지우는 뮤턴트는 이 두 단언에서 죽는다(forge #14 클래스, 두 경로 모두 load-bearing).
        assert_eq!(
            saved.worktrees[0].linked_jira_issue.as_deref(),
            Some("PROJ-99"),
            "the linked Jira issue read from disk must survive a save — seeded into WorktreeMeta \
             and re-injected, exactly like linked_linear_issue (forge #14 class)"
        );
        assert_eq!(
            saved.worktrees[0].linked_jira_site.as_deref(),
            Some("https://acme.atlassian.net"),
            "the Jira site coordinate must survive too (deep-link/multi-site)"
        );
    }

    /// **N1 데이터-손실 가드 (리듀서 경로)**: 이슈 목록의 "link this worktree"가 굳힌 링크는
    /// 저장을 거쳐도 남는다. `LinearIssueLinked` 핸들러의 `link_linear_issue`/`persist`를
    /// 지우는 뮤턴트는 meta 단언에서, 씨딩·재주입을 지우는 뮤턴트는 flush_and_reload 단언에서
    /// 죽는다(forge의 `a_successful_create_pr_persists_the_linked_pr_number` 미러).
    #[test]
    fn linking_a_worktree_to_a_linear_issue_survives_a_save() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("data.json");
        let (mut state, _repo_id, worktree_id) = state_with_one_listed_worktree();
        let boot = crate::persistence_thread::PersistenceHandle::spawn(file.clone());
        state.persistence = Some(boot.handle);

        // 연결된 워크스페이스를 가정하고(좌표를 채우려고) 이슈를 링크한다.
        state.linear.workspace = Some(LinearWorkspace {
            id: "org-77".into(),
            name: "Acme".into(),
            url_key: "acme".into(),
            viewer_email: "ada@acme.com".into(),
        });
        let issue = suaegi_tracker::Issue {
            id: "iss_1".into(),
            identifier: "ENG-42".into(),
            title: "Fix the bug".into(),
            description: None,
            url: None,
            state: Some("In Progress".into()),
            assignee: None,
        };
        let _ = state.update(Message::LinearIssueLinked {
            worktree: worktree_id.clone(),
            issue,
        });

        // 메모리에 굳었는가(link_linear_issue).
        assert_eq!(
            state
                .worktree_meta
                .get(&worktree_id)
                .and_then(|m| m.linked_linear_issue.as_deref()),
            Some("ENG-42"),
            "linking must fold the issue identifier into WorktreeMeta"
        );
        assert_eq!(
            state.linked_linear_issue(&worktree_id),
            Some("ENG-42"),
            "the sidebar reader must see the link"
        );

        // 저장을 거쳐도 남는가(씨딩·재주입 + 좌표).
        let saved = flush_and_reload(state, &file);
        assert_eq!(
            saved.worktrees[0].linked_linear_issue.as_deref(),
            Some("ENG-42"),
            "the link must survive a save — WorktreeMeta seeded + re-injected (forge #14 class)"
        );
        assert_eq!(
            saved.worktrees[0].linked_linear_issue_workspace_id.as_deref(),
            Some("org-77"),
            "the workspace coordinate captured from the connected workspace must survive too"
        );
    }

    /// **API 키 규율 (c)**: 키는 `suaegi-secrets`로만 가고 **평문 JSON에 절대 안 들어간다**.
    /// 인메모리 토큰/입력 버퍼를 세워도 `persisted_snapshot` 직렬화에 키가 나타나지 않고,
    /// `LinearState`의 커스텀 Debug도 입력 버퍼를 리댁션한다. 키를 Worktree/Settings 등
    /// 영속 필드에 흘리는 뮤턴트는 JSON 단언에서 죽는다.
    #[test]
    fn the_linear_api_key_never_enters_persisted_json_or_debug() {
        const KEY: &str = "lin_api_supersecret_ABC123";
        let (mut state, _repo_id, _worktree_id) = state_with_one_listed_worktree();
        // 사용자가 키를 입력하고(평문 버퍼) 연결된 상태를 흉내낸다.
        state.linear.api_key_input = KEY.to_string();
        state.linear.token = Some(Secret::new(KEY));
        state.linear.workspace = Some(LinearWorkspace {
            id: "org-1".into(),
            name: "Acme".into(),
            url_key: "acme".into(),
            viewer_email: "ada@acme.com".into(),
        });

        // 영속 스냅샷 JSON 어디에도 키가 없다(토큰은 키체인으로만 간다).
        let json = serde_json::to_string(&state.persisted_snapshot()).unwrap();
        assert!(
            !json.contains(KEY),
            "the Linear API key must never appear in the persisted JSON"
        );
        // LinearState Debug도 입력 버퍼(평문)를 리댁션한다.
        let dbg = format!("{:?}", state.linear);
        assert!(
            !dbg.contains(KEY),
            "the Linear API key must never appear in Debug output: {dbg}"
        );
    }

    // ---- N2: Jira 트래커 UI ----

    fn jira_issue(key: &str) -> JiraIssue {
        JiraIssue {
            id: format!("id_{key}"),
            key: key.to_string(),
            title: "Fix the bug".to_string(),
            description: String::new(),
            url: format!("https://acme.atlassian.net/browse/{key}"),
            project_key: Some("PROJ".to_string()),
            issue_type: Some("Task".to_string()),
            status: Some("In Progress".to_string()),
            assignee: None,
            labels: vec![],
        }
    }

    fn connected_jira_connection() -> JiraConnection {
        JiraConnection {
            site_url: "https://acme.atlassian.net".into(),
            email: "ada@acme.com".into(),
            auth_type: JiraAuthType::Cloud,
        }
    }

    /// **N2 데이터-손실 가드 (리듀서 경로)**: 이슈 목록의 "link this worktree"가 굳힌 Jira 링크는
    /// 저장을 거쳐도 남는다. `JiraIssueLinked` 핸들러의 `link_jira_issue`/`persist`를 지우는 뮤턴트는
    /// meta 단언에서, 씨딩·재주입을 지우는 뮤턴트는 flush_and_reload 단언에서 죽는다(N1 미러).
    #[test]
    fn linking_a_worktree_to_a_jira_issue_survives_a_save() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("data.json");
        let (mut state, _repo_id, worktree_id) = state_with_one_listed_worktree();
        let boot = crate::persistence_thread::PersistenceHandle::spawn(file.clone());
        state.persistence = Some(boot.handle);

        // 연결된 사이트를 가정하고(사이트 좌표를 채우려고) 이슈를 링크한다.
        state.jira.connection = Some(connected_jira_connection());
        let _ = state.update(Message::JiraIssueLinked {
            worktree: worktree_id.clone(),
            issue: jira_issue("PROJ-42"),
        });

        // 메모리에 굳었는가(link_jira_issue).
        assert_eq!(
            state
                .worktree_meta
                .get(&worktree_id)
                .and_then(|m| m.linked_jira_issue.as_deref()),
            Some("PROJ-42"),
            "linking must fold the issue key into WorktreeMeta"
        );
        assert_eq!(
            state.linked_jira_issue(&worktree_id),
            Some("PROJ-42"),
            "the sidebar reader must see the link"
        );

        // 저장을 거쳐도 남는가(씨딩·재주입 + 사이트 좌표).
        let saved = flush_and_reload(state, &file);
        assert_eq!(
            saved.worktrees[0].linked_jira_issue.as_deref(),
            Some("PROJ-42"),
            "the link must survive a save — WorktreeMeta seeded + re-injected (forge #14 class)"
        );
        assert_eq!(
            saved.worktrees[0].linked_jira_site.as_deref(),
            Some("https://acme.atlassian.net"),
            "the site coordinate captured from the connected connection must survive too"
        );
    }

    /// **토큰 규율 (c)**: Jira 토큰은 `suaegi-secrets`로만 가고 **평문 JSON에 절대 안 들어간다**.
    /// 인메모리 토큰/입력 버퍼를 세우고 연결까지 굳혀도 `persisted_snapshot` 직렬화에 토큰이
    /// 나타나지 않고(site/email 같은 non-secret 설정은 나타나도 됨), `JiraState`의 커스텀 Debug도
    /// 토큰 입력 버퍼를 리댁션한다. 토큰을 영속 필드에 흘리는 뮤턴트는 JSON 단언에서 죽는다.
    #[test]
    fn the_jira_token_never_enters_persisted_json_or_debug() {
        const TOKEN: &str = "jira_pat_supersecret_ABC123";
        let (mut state, _repo_id, _worktree_id) = state_with_one_listed_worktree();
        // 사용자가 토큰을 입력하고(평문 버퍼) 연결된 상태를 흉내낸다.
        state.jira.token_input = TOKEN.to_string();
        state.jira.token = Some(Secret::new(TOKEN));
        state.jira.connection = Some(connected_jira_connection());
        state.jira.viewer = Some(JiraViewer {
            account_id: "acc_1".into(),
            display_name: "Ada".into(),
            email: Some("ada@acme.com".into()),
        });

        // 영속 스냅샷 JSON 어디에도 토큰이 없다(토큰은 키체인으로만 간다).
        let json = serde_json::to_string(&state.persisted_snapshot()).unwrap();
        assert!(
            !json.contains(TOKEN),
            "the Jira token must never appear in the persisted JSON"
        );
        // 대조군: non-secret 연결 설정은 실제로 굳는다(부팅 재연결의 근거) — 위 단언이 "아무것도
        // 안 썼다"로 설명되면 안 된다.
        assert!(
            json.contains("acme.atlassian.net"),
            "control: the non-secret connection config must be persisted for boot reconnect"
        );
        // JiraState Debug도 토큰 입력 버퍼(평문)를 리댁션한다.
        let dbg = format!("{:?}", state.jira);
        assert!(
            !dbg.contains(TOKEN),
            "the Jira token must never appear in Debug output: {dbg}"
        );
    }

    /// 부팅 재연결의 근거: 성공한 Jira 연결은 non-secret 설정을 `Settings`에 굳히고, 그 설정은
    /// 저장→재로드를 거쳐 `from_load`가 메모리 연결로 되살린다(토큰은 키체인이 따로 짚는다).
    /// 이 왕복이 깨지면 "연결"이 재시작을 못 넘는다.
    #[test]
    fn a_jira_connection_config_survives_a_save_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("data.json");
        let (mut state, _repo_id, _worktree_id) = state_with_one_listed_worktree();
        let boot = crate::persistence_thread::PersistenceHandle::spawn(file.clone());
        state.persistence = Some(boot.handle);

        state.jira.connection = Some(JiraConnection {
            site_url: "https://acme.atlassian.net".into(),
            email: "ada@acme.com".into(),
            auth_type: JiraAuthType::Server,
        });
        state.persist();

        let saved = flush_and_reload(state, &file);
        let cfg = saved
            .settings
            .jira_connection
            .clone()
            .expect("the connection config must be persisted into Settings");
        assert_eq!(cfg.site_url, "https://acme.atlassian.net");
        assert_eq!(cfg.email, "ada@acme.com");
        assert!(!cfg.is_cloud, "Server auth_type must map to is_cloud=false");

        // from_load가 그 설정으로 메모리 연결을 되살린다(부팅이 재연결에 쓸 좌표).
        let reloaded = AppState::from_load(LoadDiagnostics {
            state: saved,
            origin: LoadOrigin::Fresh,
            save_blocked: false,
        });
        let conn = reloaded
            .jira
            .connection
            .expect("from_load must revive the connection from persisted settings");
        assert_eq!(conn.site_url, "https://acme.atlassian.net");
        assert_eq!(conn.auth_type, JiraAuthType::Server);
        // 폼 입력도 미리 채워진다(토큰 입력만 빼고 — 토큰은 디스크에 없다).
        assert_eq!(reloaded.jira.site_url_input, "https://acme.atlassian.net");
        assert!(
            reloaded.jira.token_input.is_empty(),
            "the token input must never be seeded from disk"
        );
    }

    fn review(number: u64, state: ReviewState) -> Review {
        Review {
            number,
            state,
            title: "t".to_string(),
            url: "https://example/pr".to_string(),
            checks: ChecksSummary::default(),
        }
    }

    /// listed(하지만 세션 없는) worktree 하나. PR 상태/생성 리듀서를 세션 스폰 없이
    /// 돌리려는 테스트용 — WorktreeSelected의 세션 경로는 여기서 관심사가 아니다.
    fn state_with_one_listed_worktree() -> (AppState, RepoId, WorktreeId) {
        let mut state = AppState::default();
        let repo_id = RepoId("/tmp/pr-repo".into());
        state.upsert_repo(some_repo("pr-repo"));
        let worktree_id = wt("/tmp/wt-pr");
        state.note_list_issued(repo_id.clone(), OpId(1));
        state.apply_authoritative_listing(
            repo_id.clone(),
            OpId(1),
            vec![entry_at("/tmp/wt-pr", "feature")],
        );
        (state, repo_id, worktree_id)
    }

    /// **§5 mutation (c): 생성 성공은 `linked_github_pr`을 굳히고, 그 링크는 저장을
    /// 거쳐도 남는다.** `link_pr` 호출을 지우는 뮤턴트는 meta 단언에서, 씨딩·재주입을
    /// 지우는 뮤턴트는 flush_and_reload 단언에서 죽는다.
    #[test]
    fn a_successful_create_pr_persists_the_linked_pr_number() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("data.json");
        let (mut state, _repo_id, worktree_id) = state_with_one_listed_worktree();
        let boot = crate::persistence_thread::PersistenceHandle::spawn(file.clone());
        state.persistence = Some(boot.handle);

        // 다이얼로그가 이 worktree를 위해 열려 있다고 가정하고 성공 응답을 넣는다.
        let _ = state.update(Message::CreatePrOpened {
            worktree: worktree_id.clone(),
        });
        let _ = state.update(Message::CreatePrCreated {
            worktree: worktree_id.clone(),
            op: OpId(99),
            result: Ok(review(77, ReviewState::Open)),
        });

        assert_eq!(
            state
                .worktree_meta
                .get(&worktree_id)
                .and_then(|m| m.linked_github_pr),
            Some(77),
            "a successful create must link the PR number into WorktreeMeta"
        );
        assert!(
            state.create_pr.is_none(),
            "a successful create must close the dialog"
        );

        let saved = flush_and_reload(state, &file);
        assert_eq!(
            saved.worktrees[0].linked_github_pr,
            Some(77),
            "the linked PR must survive a save — seeded into WorktreeMeta and re-injected"
        );
    }

    /// 생성 실패는 다이얼로그를 닫지 않고 **분류된 문구**를 그 자리에 남긴다;
    /// 링크는 굳히지 않는다.
    #[test]
    fn a_failed_create_pr_keeps_the_dialog_open_with_a_classified_error() {
        let (mut state, _repo_id, worktree_id) = state_with_one_listed_worktree();
        let _ = state.update(Message::CreatePrOpened {
            worktree: worktree_id.clone(),
        });
        let _ = state.update(Message::CreatePrCreated {
            worktree: worktree_id.clone(),
            op: OpId(5),
            result: Err("run gh auth login".to_string()),
        });

        let dialog = state
            .create_pr_dialog()
            .expect("a failed create must keep the dialog open so the user can retry");
        assert_eq!(dialog.error.as_deref(), Some("run gh auth login"));
        assert!(!dialog.submitting, "a failed create must unlock the submit button");
        assert_eq!(
            state
                .worktree_meta
                .get(&worktree_id)
                .and_then(|m| m.linked_github_pr),
            None,
            "a failed create must not link any PR"
        );
    }

    /// **§5 mutation (d): PR 상태는 worktree 활성화 시 조회되고, `PresenceTick`으로는
    /// 절대 조회되지 않는다.** 활성화 트리거를 `PresenceTick`으로 옮기거나 tick에
    /// 조회를 더하는 뮤턴트는 이 테스트를 깬다.
    #[test]
    fn github_status_is_fetched_on_activate_never_on_a_presence_tick() {
        let (mut state, _repo_id, worktree_id) = state_with_one_listed_worktree();

        // 틱만으로는 절대 조회하지 않는다 — 조회는 명시적 활성화의 몫이다.
        let _ = state.update(Message::PresenceTick);
        assert!(
            state.github_status_for(&worktree_id).is_none(),
            "a PresenceTick must never trigger a PR fetch (no background polling)"
        );

        // 활성화는 한 번 조회한다(Checking을 세운다). 세션 경로는 이미 세션이 없고
        // pending도 아니라 start_session_for로 가지만, 반환된 Task는 테스트에서
        // 실행되지 않으므로 여기서 관찰하는 것은 상태(Checking)뿐이다.
        let _ = state.update(Message::WorktreeSelected(worktree_id.clone()));
        assert!(
            matches!(
                state.github_status_for(&worktree_id),
                Some(GithubStatus::Checking)
            ),
            "activating a worktree must issue a PR status fetch (Checking)"
        );

        // 그 뒤의 틱은 상태를 건드리지 않는다.
        let before = state.github_status_for(&worktree_id).cloned();
        let _ = state.update(Message::PresenceTick);
        assert_eq!(
            state.github_status_for(&worktree_id).cloned(),
            before,
            "a PresenceTick must not disturb an already-fetched PR status"
        );
    }

    /// on-activate는 **1회**다: 이미 캐시가 있으면(성공이든 실패든) 재조회하지 않는다.
    /// 재조회는 명시적 새로고침(force)의 몫이다.
    #[test]
    fn activate_fetches_once_but_manual_refresh_refetches() {
        let (mut state, _repo_id, worktree_id) = state_with_one_listed_worktree();
        // 이미 조회가 끝난 캐시(예: Unavailable)를 심는다.
        state.github_status.insert(
            worktree_id.clone(),
            GithubStatus::Fetched {
                fetch: GithubFetch::Unavailable(ForgeUnavailable::Network),
                eligibility: CreationEligibility::Blocked(blocked_unavailable()),
            },
        );

        // 활성화(force=false)는 캐시가 있으니 재조회하지 않는다 — 상태 그대로.
        let _ = state.update(Message::WorktreeSelected(worktree_id.clone()));
        assert!(
            matches!(
                state.github_status_for(&worktree_id),
                Some(GithubStatus::Fetched { .. })
            ),
            "on-activate must not re-fetch when a cached result already exists"
        );

        // 수동 새로고침(force=true)은 다시 조회한다 — Checking으로 되돌린다.
        let _ = state.update(Message::GithubRefreshRequested {
            worktree: worktree_id.clone(),
        });
        assert!(
            matches!(
                state.github_status_for(&worktree_id),
                Some(GithubStatus::Checking)
            ),
            "a manual refresh must always re-fetch"
        );
    }

    fn blocked_unavailable() -> CreationBlockedReason {
        CreationBlockedReason::Unavailable(ForgeUnavailable::Network)
    }

    /// 조회 실패(`Unavailable`)가 도착해도 **이미 굳은 `linked_github_pr`을 지우지
    /// 않는다** — 일시 오류가 알려진 PR 링크를 날리면 안 된다(캐시-오염 구별).
    #[test]
    fn an_unavailable_fetch_does_not_erase_an_existing_linked_pr() {
        let (mut state, _repo_id, worktree_id) = state_with_one_listed_worktree();
        state.link_pr(&worktree_id, 1234);
        let op = OpId(7);
        state.latest_forge_op.insert(worktree_id.clone(), op);

        let _ = state.update(Message::GithubStatusFetched {
            worktree: worktree_id.clone(),
            op,
            fetch: GithubFetch::Unavailable(ForgeUnavailable::RateLimited),
            eligibility: CreationEligibility::Blocked(blocked_unavailable()),
        });

        assert_eq!(
            state
                .worktree_meta
                .get(&worktree_id)
                .and_then(|m| m.linked_github_pr),
            Some(1234),
            "a transient Unavailable must never clear a known linked PR"
        );
    }

    /// 낡은 조회 응답(op이 최신이 아님)은 버린다 — 수동 새로고침이 on-activate
    /// 조회를 앞질렀을 때 먼저 떠난 결과가 새 것을 덮으면 안 된다.
    #[test]
    fn a_stale_github_fetch_response_is_discarded() {
        let (mut state, _repo_id, worktree_id) = state_with_one_listed_worktree();
        // 최신 op은 2. 도착하는 응답은 낡은 op 1.
        state.latest_forge_op.insert(worktree_id.clone(), OpId(2));
        state
            .github_status
            .insert(worktree_id.clone(), GithubStatus::Checking);

        let _ = state.update(Message::GithubStatusFetched {
            worktree: worktree_id.clone(),
            op: OpId(1),
            fetch: GithubFetch::Resolved(ReviewLookup::None),
            eligibility: CreationEligibility::Eligible,
        });

        assert!(
            matches!(
                state.github_status_for(&worktree_id),
                Some(GithubStatus::Checking)
            ),
            "a stale fetch (older op) must not overwrite the in-flight status"
        );
    }

    // ---- Plan 7b: PR 패널 배선(실제 `update` 디스패치) ----
    //
    // 순수 확인-게이트 상태기계는 `pr_panel` 유닛 테스트가 본다. `update`가 그
    // 게이트를 **부르는 것을 빠뜨리는** 뮤턴트는 그걸로 못 잡으므로, 여기서 실제
    // 메시지를 흘려 배선 자체를 태운다(diff 패널 `wiring` 모듈과 같은 규율).

    /// PR 패널을 열 수 있도록 7a Found 리뷰를 캐시에 심는다.
    fn seed_found_pr(state: &mut AppState, worktree_id: &WorktreeId, number: u64) {
        state.github_status.insert(
            worktree_id.clone(),
            GithubStatus::Fetched {
                fetch: GithubFetch::Resolved(ReviewLookup::Found(review(number, ReviewState::Open))),
                eligibility: CreationEligibility::Blocked(CreationBlockedReason::AlreadyExists),
            },
        );
    }

    fn details_with(mergeability: suaegi_forge::MergeabilityState) -> PrDetails {
        PrDetails {
            mergeability,
            reviews: suaegi_forge::ReviewThreadLookup::Found(vec![]),
            comments: suaegi_forge::CommentLookup::Found(vec![]),
        }
    }

    /// **7b의 심장, 실제 디스패치로: 파괴적 머지는 확인 단계 없이 절대 발급되지
    /// 않는다.** `update`가 `MergeConfirmed`에서 `confirm_merge`의 `None`을 무시하고
    /// 무조건 머지를 발급하는(원클릭) 뮤턴트, 또는 `MergeRequested`를 바로 머지로
    /// 잇는 뮤턴트는 이 테스트를 깨야 한다.
    #[test]
    fn a_merge_never_fires_without_the_confirm_step_via_real_dispatch() {
        let (mut state, _repo_id, worktree_id) = state_with_one_listed_worktree();
        seed_found_pr(&mut state, &worktree_id, 42);

        let _ = state.update(Message::PrPanelOpened {
            worktree: worktree_id.clone(),
        });
        assert!(state.pr_panel().is_open(), "the panel opened for the linked PR");
        assert_eq!(state.pr_panel().number(), Some(42));

        // 머지가능성만 테스트 seam으로 세운다(on-open 조회 op를 흉내내지 않는다).
        state
            .pr_panel
            .apply_details(details_with(suaegi_forge::MergeabilityState::Mergeable));

        // 확인 단계 없이 확정 → **머지 발급 안 됨.**
        let _ = state.update(Message::MergeConfirmed);
        assert!(
            !state.pr_panel().is_merging(),
            "MergeConfirmed with no open confirm step must not start a merge"
        );

        // Merge 버튼 → 확인 단계만 연다(아직 머지 아님).
        let _ = state.update(Message::MergeRequested);
        assert!(
            state.pr_panel().confirm().is_some(),
            "the confirm step is now open"
        );
        assert!(
            !state.pr_panel().is_merging(),
            "opening the confirm step must not be a merge — the whole point of 7b"
        );

        // 확정 → 머지 in flight.
        let _ = state.update(Message::MergeConfirmed);
        assert!(
            state.pr_panel().is_merging(),
            "confirming an open confirm step must start the merge"
        );
    }

    /// 머지가능성이 `Mergeable`이 아니면 `MergeRequested`는 확인 단계를 열지 않는다 —
    /// 실제 디스패치로(비활성 버튼의 마지막 방어선을 `update`가 존중하는지).
    #[test]
    fn merge_request_is_inert_when_not_mergeable_via_real_dispatch() {
        let (mut state, _repo_id, worktree_id) = state_with_one_listed_worktree();
        seed_found_pr(&mut state, &worktree_id, 42);
        let _ = state.update(Message::PrPanelOpened {
            worktree: worktree_id.clone(),
        });
        state
            .pr_panel
            .apply_details(details_with(suaegi_forge::MergeabilityState::Blocked));

        let _ = state.update(Message::MergeRequested);
        assert!(
            state.pr_panel().confirm().is_none(),
            "Blocked must not open a confirm step"
        );
        let _ = state.update(Message::MergeConfirmed);
        assert!(!state.pr_panel().is_merging(), "and no merge may start");
    }

    /// 성공한 머지는 7a 표시자를 강제 재조회(→ Checking)하고 결과를 Merged로 남긴다.
    #[test]
    fn a_successful_merge_refreshes_the_status_and_records_merged() {
        let (mut state, _repo_id, worktree_id) = state_with_one_listed_worktree();
        seed_found_pr(&mut state, &worktree_id, 42);
        let _ = state.update(Message::PrPanelOpened {
            worktree: worktree_id.clone(),
        });
        state
            .pr_panel
            .apply_details(details_with(suaegi_forge::MergeabilityState::Mergeable));

        // 알려진 op로 in-flight 머지를 만든다(`MergeCompleted`의 staleness 가드 통과용).
        let _ = state.update(Message::MergeRequested);
        let _ = state.pr_panel.confirm_merge(OpId(500));

        let _ = state.update(Message::MergeCompleted {
            worktree: worktree_id.clone(),
            op: OpId(500),
            display: MergeResultDisplay::Merged,
        });

        assert_eq!(state.pr_panel().outcome(), Some(&MergeResultDisplay::Merged));
        assert!(!state.pr_panel().is_merging());
        assert!(
            matches!(
                state.github_status_for(&worktree_id),
                Some(GithubStatus::Checking)
            ),
            "a successful merge must force-refresh the 7a status indicator"
        );
    }

    /// 확정적 거부는 **성공으로 안 읽히고**, 7a 상태를 재조회하지도 않는다 — 사용자가
    /// 사유를 보고 고치도록 패널에 남는다. Rejected를 Merged로, 또는 재조회를 무조건
    /// 거는 뮤턴트는 이 테스트를 깬다.
    #[test]
    fn a_rejected_merge_is_distinct_from_success_and_does_not_refresh() {
        let (mut state, _repo_id, worktree_id) = state_with_one_listed_worktree();
        seed_found_pr(&mut state, &worktree_id, 42);
        let _ = state.update(Message::PrPanelOpened {
            worktree: worktree_id.clone(),
        });
        state
            .pr_panel
            .apply_details(details_with(suaegi_forge::MergeabilityState::Mergeable));
        let _ = state.update(Message::MergeRequested);
        let _ = state.pr_panel.confirm_merge(OpId(501));

        let rejected = MergeResultDisplay::Rejected("Merge conflict.".to_string());
        let _ = state.update(Message::MergeCompleted {
            worktree: worktree_id.clone(),
            op: OpId(501),
            display: rejected.clone(),
        });

        assert_eq!(state.pr_panel().outcome(), Some(&rejected));
        assert!(!state.pr_panel().is_merging());
        // 7a 상태는 그대로(Found) — 재조회하지 않는다.
        assert!(
            matches!(
                state.github_status_for(&worktree_id),
                Some(GithubStatus::Fetched { .. })
            ),
            "a rejected merge must not force a 7a status refresh"
        );
    }

    /// 생성 시점이 메타데이터의 유일한 진짜 출처다 — `Ok(_created)`를 버리면
    /// 그 시각은 영영 없다. 에이전트를 안 고르면(피커 미조작) agent는 `None`.
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
            created_with_agent: None,
            initial_prompt: None,
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
            "no agent was chosen, so the worktree stays a login shell (today's default)"
        );
    }

    /// 6c: 생성 시 고른 에이전트 id가 메타데이터로 굳어야 한다 — 그래야 세션 시작이
    /// 로그인 셸이 아니라 그 에이전트를 띄운다.
    #[test]
    fn creating_a_worktree_records_the_chosen_agent() {
        let mut state = AppState::default();
        let repo_id = RepoId("/tmp/creator".into());
        state.upsert_repo(some_repo("creator"));

        let _ = state.update(Message::WorktreeCreated {
            request: OpId(1),
            repo_id,
            created_with_agent: Some("claude".to_string()),
            initial_prompt: None,
            result: Ok(CreatedWorktree {
                path: PathBuf::from("/tmp/wt-claude"),
                branch: "new".into(),
                display_name: "new".into(),
            }),
        });

        let meta = state
            .worktree_meta
            .get(&wt("/tmp/wt-claude"))
            .expect("creation must record metadata");
        assert_eq!(
            meta.created_with_agent.as_deref(),
            Some("claude"),
            "the chosen agent id must be baked into the worktree metadata"
        );
    }

    /// 피커 옵션: 로그인 셸이 **항상 맨 앞**(기본)이고, 설치된 에이전트가 뒤따른다.
    /// 설치 감지(PATH)와 분리된 순수 함수라 합성 목록으로 직접 검증한다.
    #[test]
    fn agent_choices_puts_login_shell_first_then_installed() {
        let choices = agent_choices(&["claude", "codex"]);
        assert_eq!(
            choices,
            vec![
                AgentChoice::LOGIN_SHELL,
                AgentChoice(Some("claude")),
                AgentChoice(Some("codex")),
            ]
        );
        // 아무것도 설치 안 됐어도 로그인 셸은 늘 고를 수 있다.
        assert_eq!(agent_choices(&[]), vec![AgentChoice::LOGIN_SHELL]);
    }

    /// 드롭다운 라벨: 로그인 셸은 명시적 이름, 에이전트는 표의 display_name.
    #[test]
    fn agent_choice_labels_read_from_the_registry() {
        assert_eq!(AgentChoice::LOGIN_SHELL.label(), "Login shell");
        assert_eq!(AgentChoice(Some("claude")).label(), "Claude");
    }

    /// 피커 선택이 드래프트에 남고, 로그인 셸(기본)을 고르면 지워진다(= 없음).
    #[test]
    fn selecting_an_agent_updates_the_draft_then_login_shell_clears_it() {
        let mut state = AppState::default();
        let repo_id = RepoId("/tmp/r".into());

        // 기본은 로그인 셸.
        assert_eq!(
            state.worktree_agent_selection(&repo_id),
            AgentChoice::LOGIN_SHELL
        );

        let _ = state.update(Message::WorktreeAgentSelected {
            repo_id: repo_id.clone(),
            choice: AgentChoice(Some("claude")),
        });
        assert_eq!(
            state.worktree_agent_selection(&repo_id),
            AgentChoice(Some("claude")),
            "selecting claude must persist in the draft"
        );

        // 대조군: 다시 로그인 셸을 고르면 기본으로 되돌아간다.
        let _ = state.update(Message::WorktreeAgentSelected {
            repo_id: repo_id.clone(),
            choice: AgentChoice::LOGIN_SHELL,
        });
        assert_eq!(
            state.worktree_agent_selection(&repo_id),
            AgentChoice::LOGIN_SHELL,
            "choosing login shell must clear the draft"
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
                linked_github_pr: None,
                ..Default::default()
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
