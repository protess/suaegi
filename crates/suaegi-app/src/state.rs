use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use futures::StreamExt;
use iced::widget::pane_grid;
use suaegi_core::domain::{
    PersistedState, Repo, RepoId, SessionState, Settings, Worktree, WorktreeId, SCHEMA_VERSION,
};
use suaegi_git::worktree::{CreatedWorktree, RemoveOutcome, WorktreeEntry};
use suaegi_term::agent::AgentKind;
use suaegi_term::grid::TerminalSnapshot;
use suaegi_term::presence::AgentPresence;

use crate::persistence_thread::{
    LoadDiagnostics, LoadOrigin, PersistenceHandle, SaveReport, SaveStatus,
};
use crate::session_store::{SessionId, SessionStore, StartedSession};

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
    pub fn apply_worktree_listing(&mut self, repo: RepoId, op: OpId, entries: Vec<WorktreeEntry>) {
        if let Some(latest) = self.latest_list_op.get(&repo) {
            if op.0 < latest.0 {
                return;
            }
        }
        self.worktrees_by_repo.insert(repo, entries);
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

    /// 세션을 스토어에서 닫고(Reaper로 은퇴) worktree ↔ 세션 매핑을 정리한다.
    /// pane_grid 쪽 정리(닫을 pane 자체를 지우는 것)는 호출자(`PaneCloseRequested`
    /// 핸들러) 몫이다 — pane_grid `close()`는 마지막 pane을 지울 수 없어서 그
    /// 결정은 세션 정리와 분리해 둬야 한다.
    fn close_session(&mut self, id: SessionId) {
        self.session_store.close(id);
        if let Some(worktree_id) = self.session_worktrees.remove(&id) {
            self.worktree_sessions.remove(&worktree_id);
        }
        self.session_titles.remove(&id);
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
                // 세션이 살아 있는 상태로 worktree를 지우면 셸이 사라진 디렉터리를
                // 가리키게 된다 — pane은 남겨두되(사용자가 스크롤백을 볼 수 있게)
                // 세션 자체는 정리해 PTY가 새지 않게 한다. pane_grid의 pane 자체는
                // `PaneCloseRequested`가 올 때까지 그대로 둔다(닫는 UX는 이 태스크
                // 범위 밖 — 워크벤치가 "세션이 종료됨"을 그리는 건 Plan 4).
                if let Some(&session_id) = self.worktree_sessions.get(&worktree_id) {
                    self.close_session(session_id);
                }
                // 아직 시작 중인(`pending_session_starts`) 세션은 위에서 못 잡는다
                // — `worktree_sessions`엔 `SessionStarted`가 도착해야 올라가기
                // 때문이다. 그 경합을 `worktree_still_exists`가 알아채도록
                // 표시해 둔다(위 `pending_worktree_removals` 문서 참고).
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
                    Ok(_outcome) => {
                        self.last_error = None;
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
            Message::PaneClicked(pane) => {
                self.focused_pane = Some(pane);
                iced::Task::none()
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
                if let Some(panes) = &mut self.panes {
                    if panes.len() <= 1 {
                        // pane_grid는 형제가 없는 마지막 pane을 `close()`로
                        // 지울 수 없다 — 워크벤치 전체를 빈 상태로 되돌린다.
                        if let Some(&session_id) = panes.get(pane) {
                            self.close_session(session_id);
                        }
                        self.panes = None;
                        self.focused_pane = None;
                    } else if let Some((session_id, sibling)) = panes.close(pane) {
                        self.close_session(session_id);
                        self.focused_pane = Some(sibling);
                    }
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
}
