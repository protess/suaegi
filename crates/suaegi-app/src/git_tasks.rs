use std::path::PathBuf;

use iced::Task;
use suaegi_core::domain::{Repo, WorktreeId};
use suaegi_git::repo_probe::probe_repo;
use suaegi_git::runner::GitRunner;
use suaegi_git::worktree::{
    add_worktree, list_worktrees as git_list_worktrees, remove_worktree as git_remove_worktree,
    CreatedWorktree, RemoveOutcome, WorktreeEntry,
};

use crate::background;
use crate::state::{Message, OpId};

// ---- `*_now`: the real work, testable directly (no iced::Task involved) ----

/// 1단계(블로킹): canonicalize + Repo 구성. `Repo::from_path`가 canonicalize를
/// 하므로 **여기서만** 부른다 — 2단계에서 다시 부르면 tokio 워커가 막힌다.
pub fn build_repo_now(path: PathBuf) -> Result<Repo, String> {
    Repo::from_path(&path).map_err(|e| e.to_string())
}

/// 2단계(tokio): 이미 만들어진 Repo로 git probe. Repo를 다시 만들지 않는다.
pub async fn probe_repo_now(repo: Repo) -> Result<(Repo, Option<String>), String> {
    let runner = GitRunner::new();
    let probe = probe_repo(&runner, &repo.path)
        .await
        .map_err(|e| e.to_string())?;
    if !probe.is_git_repo {
        return Err(format!("{} is not a git repository", repo.path.display()));
    }
    Ok((repo, probe.head_branch))
}

pub async fn list_worktrees_now(repo: Repo) -> Result<Vec<WorktreeEntry>, String> {
    let runner = GitRunner::new();
    git_list_worktrees(&runner, &repo.path)
        .await
        .map_err(|e| e.to_string())
}

pub async fn create_worktree_now(
    repo: Repo,
    requested_name: String,
    base_ref: String,
    workspace_root: PathBuf,
) -> Result<CreatedWorktree, String> {
    let runner = GitRunner::new();
    add_worktree(
        &runner,
        &repo.path,
        &requested_name,
        &base_ref,
        &workspace_root,
    )
    .await
    .map_err(|e| e.to_string())
}

pub async fn remove_worktree_now(
    repo: Repo,
    worktree_path: PathBuf,
    force: bool,
    delete_branch: Option<String>,
) -> Result<RemoveOutcome, String> {
    let runner = GitRunner::new();
    git_remove_worktree(
        &runner,
        &repo.path,
        &worktree_path,
        force,
        delete_branch.as_deref(),
    )
    .await
    .map_err(|e| e.to_string())
}

// ---- Thin `Task<Message>` wrappers: untestable glue, kept as small as possible ----

/// repo 등록. 2단계 Task로 합성한다: canonicalize(블로킹) → git probe(tokio).
/// 한 `Task::perform`에 둘 다 넣으면 canonicalize가 tokio 워커를 막는다.
pub fn add_repo(request: OpId, path: PathBuf) -> Task<Message> {
    let path_for_build = path.clone();
    background::blocking(move |mut sender| {
        let _ = sender.try_send(build_repo_now(path_for_build.clone()));
    })
    .then(move |result: Result<Repo, String>| {
        let requested_path = path.clone();
        match result {
            Ok(repo) => Task::perform(probe_repo_now(repo), move |result| Message::RepoProbed {
                request,
                requested_path,
                result,
            }),
            Err(err) => Task::done(Message::RepoProbed {
                request,
                requested_path,
                result: Err(err),
            }),
        }
    })
}

pub fn list_worktrees(request: OpId, repo: Repo) -> Task<Message> {
    let repo_id = repo.id.clone();
    Task::perform(list_worktrees_now(repo), move |result| {
        Message::WorktreesListed {
            request,
            repo_id,
            result,
        }
    })
}

pub fn create_worktree(
    request: OpId,
    repo: Repo,
    requested_name: String,
    base_ref: String,
    workspace_root: PathBuf,
) -> Task<Message> {
    let repo_id = repo.id.clone();
    Task::perform(
        create_worktree_now(repo, requested_name, base_ref, workspace_root),
        move |result| Message::WorktreeCreated {
            request,
            repo_id,
            result,
        },
    )
}

pub fn remove_worktree(
    request: OpId,
    repo: Repo,
    worktree_id: WorktreeId,
    worktree_path: PathBuf,
    force: bool,
    delete_branch: Option<String>,
) -> Task<Message> {
    let repo_id = repo.id.clone();
    Task::perform(
        remove_worktree_now(repo, worktree_path, force, delete_branch),
        move |result| Message::WorktreeRemoved {
            request,
            repo_id,
            worktree_id,
            result,
        },
    )
}
