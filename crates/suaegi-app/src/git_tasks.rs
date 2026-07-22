use std::path::PathBuf;

use iced::Task;
use suaegi_core::domain::{Repo, WorktreeId};
use suaegi_git::compare::{
    branch_compare, file_diff, ChangeStatus, CompareHandle, CompareOutcome, FileDiff,
};
use suaegi_git::repo_probe::probe_repo;
use suaegi_git::runner::{GitError, GitRunner};
use suaegi_git::worktree::{
    add_worktree, list_worktrees as git_list_worktrees, remove_worktree as git_remove_worktree,
    CreatedWorktree, RemoveOutcome, WorktreeEntry,
};

use crate::background;
use crate::state::{DiffFailure, Message, OpId, WorktreeListing};

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

/// **분류된 결과는 `Ok`로 나온다.** `NoMergeBase`/`UnbornHead`/`InvalidBase`는
/// 오류가 아니라 패널이 그릴 상태이고, `Cancelled`도 마찬가지다.
///
/// `Err`로 남는 것은 진짜 오류뿐인데, 그중 **출력 상한 초과만은 따로 뽑는다** —
/// 그건 "너무 크다"는 상태이고 그리려면 `limit`이 필요하다. 여기가 타입 있는
/// `GitError`를 아직 쥐고 있는 마지막 지점이라 이 변환의 자리는 여기다.
pub async fn compare_worktree_now(
    worktree_path: PathBuf,
    base_ref: String,
    cancel: CompareHandle,
) -> Result<CompareOutcome, DiffFailure> {
    let runner = GitRunner::new();
    branch_compare(&runner, &worktree_path, &base_ref, &cancel)
        .await
        .map_err(|e| match e {
            GitError::OutputTooLarge { limit } => DiffFailure::TooLarge { limit },
            other => DiffFailure::Failed(other.to_string()),
        })
}

pub async fn file_patch_now(
    worktree_path: PathBuf,
    base_ref: String,
    path: String,
    status: ChangeStatus,
) -> Result<FileDiff, String> {
    let runner = GitRunner::new();
    file_diff(&runner, &worktree_path, &base_ref, &path, &status)
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

/// **git이 성공한 목록만 `Authoritative`다.** 실패를 `Authoritative(vec![])`로
/// 옮기면 실패한 스캔 한 번이 그 repo의 세션과 복원된 레이아웃을 전부 지운다 —
/// 그 변환이 일어날 수 있는 유일한 지점이 여기라서 이 함수가 경계다.
pub fn list_worktrees(request: OpId, repo: Repo) -> Task<Message> {
    let repo_id = repo.id.clone();
    Task::perform(list_worktrees_now(repo), move |result| {
        Message::WorktreesListed {
            request,
            repo_id,
            result: listing_from(result),
        }
    })
}

/// git 결과 → [`WorktreeListing`]. **이 크레이트에서 `Authoritative`가 만들어지는
/// 유일한 지점**이라 순수 함수로 뽑아 직접 검사한다.
pub fn listing_from(result: Result<Vec<WorktreeEntry>, String>) -> WorktreeListing {
    #[allow(clippy::let_and_return)]
    let listing = match result {
        // **빈 목록은 증거가 아니다.** `git worktree list`는 유효한
        // 저장소에서 **항상 최소한 main 체크아웃 하나**를 낸다. 따라서
        // 0개는 "전부 지워졌다"가 아니라 "porcelain을 못 읽었다"는 뜻이다 —
        // `list_worktrees`의 파서는 `worktree ` 접두사에만 append하고
        // "아무것도 파싱하지 못했다" 가지가 없어서, exit 0인 낯선 출력이
        // 그대로 `Ok(vec![])`이 된다.
        //
        // 그걸 `Authoritative`로 올리면 모든 worktree가 사라진 것으로
        // 판정돼 세션과 pane이 전부 닫히고, 그 상태가 곧바로 저장된다 —
        // **성공했지만 이해하지 못한 스캔 한 번이 복원된 레이아웃을
        // 지운다.** exit 코드가 0이라는 것과 결과를 신뢰할 수 있다는 것은
        // 다른 문장이다.
        Ok(entries) if entries.is_empty() => WorktreeListing::Degraded(
            "git worktree list returned no entries; a valid repository always \
                     lists at least its main worktree, so this scan is not trustworthy"
                .to_string(),
        ),
        Ok(entries) => WorktreeListing::Authoritative(entries),
        Err(err) => WorktreeListing::Degraded(err),
    };
    listing
}

pub fn create_worktree(
    request: OpId,
    repo: Repo,
    requested_name: String,
    base_ref: String,
    workspace_root: PathBuf,
    // 이 create를 시작할 때 사이드바 피커가 고른 에이전트 id(`None`=로그인 셸).
    // create op와 함께 실어 보내야 응답을 기다리는 사이 사용자가 피커를 바꿔도
    // 이 worktree가 엉뚱한 에이전트로 굳지 않는다(이름 드래프트와 같은 원칙).
    selected_agent: Option<String>,
) -> Task<Message> {
    let repo_id = repo.id.clone();
    Task::perform(
        create_worktree_now(repo, requested_name, base_ref, workspace_root),
        move |result| Message::WorktreeCreated {
            request,
            repo_id,
            created_with_agent: selected_agent.clone(),
            result,
        },
    )
}

pub fn compare_worktree(
    request: OpId,
    worktree: WorktreeId,
    worktree_path: PathBuf,
    base_ref: String,
    cancel: CompareHandle,
) -> Task<Message> {
    Task::perform(
        compare_worktree_now(worktree_path, base_ref, cancel),
        move |result| Message::DiffLoaded {
            worktree: worktree.clone(),
            op: request,
            result,
        },
    )
}

pub fn file_patch(
    request: OpId,
    worktree: WorktreeId,
    worktree_path: PathBuf,
    base_ref: String,
    path: String,
    status: ChangeStatus,
) -> Task<Message> {
    Task::perform(
        file_patch_now(worktree_path, base_ref, path.clone(), status),
        move |result| Message::FileDiffLoaded {
            worktree: worktree.clone(),
            path: path.clone(),
            op: request,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str) -> WorktreeEntry {
        WorktreeEntry {
            path: PathBuf::from(path),
            branch: Some("b".to_string()),
            head: None,
            is_main: false,
        }
    }

    /// **exit 0이라는 것과 결과를 믿을 수 있다는 것은 다른 문장이다.**
    /// `list_worktrees`의 porcelain 파서는 "아무것도 파싱하지 못했다" 가지가
    /// 없어서 낯선 성공 출력이 그대로 `Ok(vec![])`이 된다. 그것을 권위로 올리면
    /// 세션과 pane이 전부 닫히고 그 상태가 저장된다.
    #[test]
    fn an_empty_successful_listing_is_not_treated_as_authoritative() {
        assert!(
            matches!(listing_from(Ok(Vec::new())), WorktreeListing::Degraded(_)),
            "a valid repository always lists at least its main worktree, so zero entries \
             means the scan was not understood — never that everything was deleted"
        );
    }

    #[test]
    fn a_real_listing_and_a_real_failure_are_classified_as_before() {
        // 대조군: 진짜 목록은 여전히 권위다.
        match listing_from(Ok(vec![entry("/tmp/wt")])) {
            WorktreeListing::Authoritative(entries) => assert_eq!(entries.len(), 1),
            WorktreeListing::Degraded(e) => panic!("a real listing must stay authoritative: {e}"),
        }
        match listing_from(Err("git exploded".to_string())) {
            WorktreeListing::Degraded(e) => assert_eq!(e, "git exploded"),
            WorktreeListing::Authoritative(_) => panic!("a failure must never be authoritative"),
        }
    }
}
