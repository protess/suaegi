use std::collections::HashMap;
use std::path::PathBuf;

use suaegi_core::domain::{PersistedState, Repo, RepoId, WorktreeId};
use suaegi_git::worktree::{CreatedWorktree, RemoveOutcome, WorktreeEntry};
use suaegi_term::grid::TerminalSnapshot;
use suaegi_term::presence::AgentPresence;

use crate::persistence_thread::{LoadOrigin, SaveReport, SaveStatus};
use crate::session_store::{SessionId, StartedSession};

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
        }
    }
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
                self.selected_worktree = Some(id);
                iced::Task::none()
            }
            Message::Saved(report) => {
                self.last_save_status = Some(report.status);
                iced::Task::none()
            }
            // `SessionStore`를 `AppState`에 들이고 실제로 처리하는 건 Task
            // 6/7의 몫이다 — 지금은 `Message`가 컴파일되도록 변형만 있고
            // `AppState`는 아직 아무 세션 상태도 들고 있지 않다.
            Message::SessionStarted { .. }
            | Message::SnapshotReady { .. }
            | Message::PresenceReady { .. } => iced::Task::none(),
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
}
