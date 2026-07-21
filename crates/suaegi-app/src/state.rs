use std::collections::HashMap;
use std::path::{Path, PathBuf};

use iced::widget::pane_grid;
use suaegi_core::domain::{PersistedState, Repo, RepoId, Worktree, WorktreeId};
use suaegi_git::worktree::{CreatedWorktree, RemoveOutcome, WorktreeEntry};
use suaegi_term::agent::AgentKind;
use suaegi_term::grid::TerminalSnapshot;
use suaegi_term::presence::AgentPresence;

use crate::persistence_thread::{LoadOrigin, SaveReport, SaveStatus};
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
    /// 영속화 스레드(Task 2)의 저장 결과. **지금은 아무것도 이 메시지를 보내지
    /// 않는다** — `PersistenceHandle`을 부팅 시 스폰하고 `results` 스트림을
    /// 여기로 연결하는 건 Task 8(통합)의 몫이다. 상태 표시줄(`status_line`)이
    /// 미리 반응할 수 있도록 자리만 만들어 둔다.
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

    /// 사이드바 상태 표시줄이 읽는 영속화 진단 정보. 부팅 시 `PersistenceHandle`이
    /// 채우는 게 정상 경로지만, 그 배선은 Task 8 몫이라 지금은 `Fresh`/`None`
    /// 기본값으로만 존재한다 — 헛경고를 내지 않기 위한 안전한 기본값이다.
    load_origin: LoadOrigin,
    last_save_status: Option<SaveStatus>,

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
    /// pane 타이틀바에 쓰는 표시용 이름. 세션 시작을 요청한 시점에 미리
    /// 채워둔다 — `SessionStarted`가 도착하기 전에도(또는 실패해도) 어떤
    /// worktree를 위한 시도였는지 사용자에게 보여줄 수 있다.
    session_titles: HashMap<SessionId, String>,
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
            session_store: SessionStore::new(),
            panes: None,
            focused_pane: None,
            worktree_sessions: HashMap::new(),
            session_worktrees: HashMap::new(),
            pending_session_starts: HashMap::new(),
            session_titles: HashMap::new(),
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
    fn worktree_still_exists(&self, id: &WorktreeId) -> bool {
        self.worktrees_by_repo
            .values()
            .any(|entries| entries.iter().any(|e| worktree_id_for(&e.path) == *id))
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
                    if repo.worktree_base_ref.is_none() {
                        repo.worktree_base_ref = head_branch;
                    }
                    let repo_id = repo.id.clone();
                    self.upsert_repo(repo);
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
                    self.apply_worktree_listing(repo_id, request, entries);
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
                repo_id, result, ..
            } => match result {
                Ok(_outcome) => self.refresh_worktrees(repo_id),
                Err(err) => {
                    self.last_error = Some(err);
                    iced::Task::none()
                }
            },
            Message::WorktreeSelected(id) => {
                self.selected_worktree = Some(id.clone());
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

            // Task 7(존재 폴링)의 몫 — 지금은 `Message`가 컴파일되도록 변형만
            // 있고 `AppState`는 아직 프레즌스 결과를 반영하지 않는다.
            Message::PresenceReady { .. } => iced::Task::none(),
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
}
