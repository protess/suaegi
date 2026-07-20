use crate::runner::{GitError, GitRunner};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoProbe {
    pub is_git_repo: bool,
    pub head_branch: Option<String>,
}

pub async fn probe_repo(runner: &GitRunner, path: &Path) -> Result<RepoProbe, GitError> {
    match runner
        .run(path, &["rev-parse", "--is-inside-work-tree"])
        .await
    {
        Ok(out) if out.stdout.trim() == "true" => {}
        Ok(_) => {
            return Ok(RepoProbe {
                is_git_repo: false,
                head_branch: None,
            })
        }
        // "not a git repository"만 정상적인 false. 권한/손상/기타 실패는 전파해
        // 호출자(UI)가 "repo가 아님"과 "확인 불가"를 구분할 수 있게 한다.
        Err(GitError::Failed { ref stderr, .. })
            if stderr.to_lowercase().contains("not a git repository") =>
        {
            return Ok(RepoProbe {
                is_git_repo: false,
                head_branch: None,
            })
        }
        Err(e) => return Err(e),
    }
    // symbolic-ref exit 1 or 128 = detached HEAD (정상), but must verify stderr contains
    // "not a symbolic ref". 그 외 실패는 전파.
    let head = match runner.run(path, &["symbolic-ref", "--short", "HEAD"]).await {
        Ok(o) => {
            let s = o.stdout.trim().to_string();
            (!s.is_empty()).then_some(s)
        }
        Err(GitError::Failed {
            code: Some(1) | Some(128),
            ref stderr,
            ..
        }) if stderr.to_lowercase().contains("not a symbolic ref") => None,
        Err(e) => return Err(e),
    };
    Ok(RepoProbe {
        is_git_repo: true,
        head_branch: head,
    })
}
