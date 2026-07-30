use std::path::{Path, PathBuf};
use std::time::Duration;

use iced::Task;
use serde::Deserialize;
use suaegi_core::domain::{GithubRepositoryIdentitySetting, RepoId};
use suaegi_git::runner::GitRunner;

use crate::state::Message;

const REMOTE_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForkSyncBlockedReason {
    MissingOrigin,
    MissingUpstream,
    UpstreamMismatch,
    MissingUpstreamDefaultBranch,
    MissingOriginBranch,
    Diverged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForkSyncStatus {
    UpToDate,
    Synced,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkSyncResult {
    pub status: ForkSyncStatus,
    pub reason: Option<ForkSyncBlockedReason>,
    pub origin_remote: String,
    pub upstream_remote: String,
    pub branch_name: Option<String>,
    pub ahead: u64,
    pub behind: u64,
}

fn blocked(
    reason: ForkSyncBlockedReason,
    branch_name: Option<String>,
    ahead: u64,
    behind: u64,
) -> ForkSyncResult {
    ForkSyncResult {
        status: ForkSyncStatus::Blocked,
        reason: Some(reason),
        origin_remote: "origin".to_string(),
        upstream_remote: "upstream".to_string(),
        branch_name,
        ahead,
        behind,
    }
}

fn github_remote_identity(remote_url: &str) -> Option<GithubRepositoryIdentitySetting> {
    let trimmed = remote_url
        .trim()
        .strip_prefix("git+")
        .unwrap_or(remote_url.trim());
    let path = if let Some(value) = trimmed.strip_prefix("github:") {
        value.to_string()
    } else if !trimmed.contains("://") {
        let (host, value) = trimmed.rsplit_once(':')?;
        let host = host.rsplit_once('@').map_or(host, |(_, host)| host);
        if !matches!(
            host.to_ascii_lowercase().as_str(),
            "github.com" | "ssh.github.com"
        ) {
            return None;
        }
        value.to_string()
    } else {
        let parsed = url::Url::parse(trimmed).ok()?;
        if !matches!(parsed.scheme(), "git" | "http" | "https" | "ssh")
            || !matches!(
                parsed.host_str()?.to_ascii_lowercase().as_str(),
                "github.com" | "ssh.github.com"
            )
        {
            return None;
        }
        parsed.path().to_string()
    };
    let normalized = path
        .trim_matches('/')
        .strip_suffix(".git")
        .unwrap_or(path.trim_matches('/'));
    let mut parts = normalized.split('/');
    let owner = parts.next()?.trim();
    let repo = parts.next()?.trim();
    if owner.is_empty() || repo.is_empty() || parts.next().is_some() {
        return None;
    }
    Some(GithubRepositoryIdentitySetting {
        owner: owner.to_string(),
        repo: repo.to_string(),
    })
}

async fn remote_identity(
    runner: &GitRunner,
    repo_path: &Path,
    remote: &str,
) -> Option<GithubRepositoryIdentitySetting> {
    runner
        .run(repo_path, &["remote", "get-url", remote])
        .await
        .ok()
        .and_then(|output| github_remote_identity(&output.stdout))
}

#[derive(Deserialize)]
struct GhRepoView {
    #[serde(rename = "isFork")]
    is_fork: Option<bool>,
    parent: Option<GhParent>,
}

#[derive(Deserialize)]
struct GhParent {
    name: Option<String>,
    owner: Option<GhOwner>,
}

#[derive(Deserialize)]
struct GhOwner {
    login: Option<String>,
}

/// Resolve the upstream GitHub repository exactly as Orca does: prefer a
/// distinct local `upstream` remote (works offline), then best-effort query the
/// origin fork metadata through `gh`.
pub async fn discover_upstream(repo_path: PathBuf) -> Option<GithubRepositoryIdentitySetting> {
    let runner = GitRunner::new();
    let origin = remote_identity(&runner, &repo_path, "origin").await?;
    if let Some(upstream) = remote_identity(&runner, &repo_path, "upstream").await {
        if !upstream.owner.eq_ignore_ascii_case(&origin.owner)
            || !upstream.repo.eq_ignore_ascii_case(&origin.repo)
        {
            return Some(upstream);
        }
    }

    let slug = format!("{}/{}", origin.owner, origin.repo);
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::process::Command::new("gh")
            .args(["repo", "view", &slug, "--json", "isFork,parent"])
            .current_dir(repo_path)
            .env("GH_PROMPT_DISABLED", "1")
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() || output.stdout.len() > 256 * 1024 {
        return None;
    }
    let payload: GhRepoView = serde_json::from_slice(&output.stdout).ok()?;
    let parent = payload.parent?;
    let owner = parent.owner?.login?;
    let repo = parent.name?;
    (payload.is_fork == Some(true) && !owner.trim().is_empty() && !repo.trim().is_empty())
        .then_some(GithubRepositoryIdentitySetting { owner, repo })
}

async fn remote_exists(runner: &GitRunner, repo_path: &Path, remote: &str) -> bool {
    runner
        .run(repo_path, &["remote"])
        .await
        .is_ok_and(|output| output.stdout.lines().any(|line| line.trim() == remote))
}

async fn fetch_remote_branch(
    runner: &GitRunner,
    repo_path: &Path,
    remote: &str,
    branch: &str,
) -> bool {
    let refspec = format!("+refs/heads/{branch}:refs/remotes/{remote}/{branch}");
    runner
        .run_with_timeout(
            repo_path,
            &["fetch", "--no-tags", "--prune", remote, &refspec],
            REMOTE_TIMEOUT,
        )
        .await
        .is_ok()
}

async fn resolve_commit(runner: &GitRunner, repo_path: &Path, reference: &str) -> Option<String> {
    let commit_ref = format!("{reference}^{{commit}}");
    runner
        .run(repo_path, &["rev-parse", "--verify", &commit_ref])
        .await
        .ok()
        .map(|output| output.stdout.trim().to_string())
        .filter(|oid| !oid.is_empty())
}

async fn resolve_default_branch(runner: &GitRunner, repo_path: &Path) -> Option<String> {
    if let Ok(output) = runner
        .run_with_timeout(
            repo_path,
            &["ls-remote", "--symref", "upstream", "HEAD"],
            REMOTE_TIMEOUT,
        )
        .await
    {
        for line in output.stdout.lines() {
            let Some(rest) = line.trim().strip_prefix("ref: refs/heads/") else {
                continue;
            };
            if let Some(branch) = rest.strip_suffix("\tHEAD") {
                if !branch.is_empty() {
                    return Some(branch.to_string());
                }
            }
        }
    }
    for branch in ["main", "master"] {
        let reference = format!("refs/remotes/upstream/{branch}^{{commit}}");
        if runner
            .run(repo_path, &["rev-parse", "--verify", &reference])
            .await
            .is_ok()
        {
            return Some(branch.to_string());
        }
    }
    None
}

/// Conservatively fast-forward a GitHub fork's default branch. This never
/// checks out, resets, rebases, force-pushes, or modifies the working tree.
pub async fn sync_default_branch(
    repo_path: PathBuf,
    expected_upstream: GithubRepositoryIdentitySetting,
) -> Result<ForkSyncResult, String> {
    sync_default_branch_inner(repo_path, expected_upstream, true).await
}

async fn sync_default_branch_inner(
    repo_path: PathBuf,
    expected_upstream: GithubRepositoryIdentitySetting,
    verify_identity: bool,
) -> Result<ForkSyncResult, String> {
    if expected_upstream.owner.trim().is_empty() || expected_upstream.repo.trim().is_empty() {
        return Err("Invalid expected upstream.".to_string());
    }
    let runner = GitRunner::new();
    if !remote_exists(&runner, &repo_path, "origin").await {
        return Ok(blocked(ForkSyncBlockedReason::MissingOrigin, None, 0, 0));
    }
    if !remote_exists(&runner, &repo_path, "upstream").await {
        return Ok(blocked(ForkSyncBlockedReason::MissingUpstream, None, 0, 0));
    }
    if verify_identity {
        let actual_upstream = remote_identity(&runner, &repo_path, "upstream").await;
        let matches = actual_upstream.is_some_and(|actual| {
            actual
                .owner
                .eq_ignore_ascii_case(expected_upstream.owner.trim())
                && actual
                    .repo
                    .eq_ignore_ascii_case(expected_upstream.repo.trim())
        });
        if !matches {
            return Ok(blocked(ForkSyncBlockedReason::UpstreamMismatch, None, 0, 0));
        }
    }

    let Some(branch) = resolve_default_branch(&runner, &repo_path).await else {
        return Ok(blocked(
            ForkSyncBlockedReason::MissingUpstreamDefaultBranch,
            None,
            0,
            0,
        ));
    };
    let full_branch_ref = format!("refs/heads/{branch}");
    runner
        .run(&repo_path, &["check-ref-format", &full_branch_ref])
        .await
        .map_err(|_| "The upstream default branch name is invalid.".to_string())?;

    if !fetch_remote_branch(&runner, &repo_path, "upstream", &branch).await {
        return Ok(blocked(
            ForkSyncBlockedReason::MissingUpstreamDefaultBranch,
            Some(branch),
            0,
            0,
        ));
    }
    if !fetch_remote_branch(&runner, &repo_path, "origin", &branch).await {
        return Ok(blocked(
            ForkSyncBlockedReason::MissingOriginBranch,
            Some(branch),
            0,
            0,
        ));
    }

    let upstream_ref = format!("refs/remotes/upstream/{branch}");
    let origin_ref = format!("refs/remotes/origin/{branch}");
    let Some(upstream_oid) = resolve_commit(&runner, &repo_path, &upstream_ref).await else {
        return Ok(blocked(
            ForkSyncBlockedReason::MissingUpstreamDefaultBranch,
            Some(branch),
            0,
            0,
        ));
    };
    let Some(origin_oid) = resolve_commit(&runner, &repo_path, &origin_ref).await else {
        return Ok(blocked(
            ForkSyncBlockedReason::MissingOriginBranch,
            Some(branch),
            0,
            0,
        ));
    };

    let range = format!("{origin_oid}...{upstream_oid}");
    let counts = runner
        .run(&repo_path, &["rev-list", "--left-right", "--count", &range])
        .await
        .map_err(|_| "Could not compare the fork with upstream.".to_string())?;
    let mut fields = counts.stdout.split_whitespace();
    let ahead = fields.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let behind = fields.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let ancestor = runner
        .run_expecting(
            &repo_path,
            &["merge-base", "--is-ancestor", &origin_oid, &upstream_oid],
            &[1],
        )
        .await
        .is_ok_and(|output| output.code == 0);
    if ahead > 0 || !ancestor {
        return Ok(blocked(
            ForkSyncBlockedReason::Diverged,
            Some(branch),
            ahead,
            behind,
        ));
    }
    if behind == 0 {
        return Ok(ForkSyncResult {
            status: ForkSyncStatus::UpToDate,
            reason: None,
            origin_remote: "origin".to_string(),
            upstream_remote: "upstream".to_string(),
            branch_name: Some(branch),
            ahead,
            behind,
        });
    }

    let push_ref = format!("{upstream_oid}:refs/heads/{branch}");
    runner
        .run_with_timeout(&repo_path, &["push", "origin", &push_ref], REMOTE_TIMEOUT)
        .await
        .map_err(|_| "Fork sync failed while updating origin.".to_string())?;
    let _ = fetch_remote_branch(&runner, &repo_path, "origin", &branch).await;
    Ok(ForkSyncResult {
        status: ForkSyncStatus::Synced,
        reason: None,
        origin_remote: "origin".to_string(),
        upstream_remote: "upstream".to_string(),
        branch_name: Some(branch),
        ahead,
        behind,
    })
}

pub fn discover(repo_id: RepoId, repo_path: PathBuf) -> Task<Message> {
    Task::perform(discover_upstream(repo_path), move |upstream| {
        Message::RepoUpstreamDiscovered(repo_id.clone(), upstream)
    })
}

pub fn sync(
    repo_id: RepoId,
    repo_path: PathBuf,
    expected_upstream: GithubRepositoryIdentitySetting,
) -> Task<Message> {
    Task::perform(
        sync_default_branch(repo_path, expected_upstream),
        move |result| Message::RepoForkSyncFinished(repo_id.clone(), result),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(cwd: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(cwd)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Suaegi Test")
            .env("GIT_AUTHOR_EMAIL", "suaegi@example.invalid")
            .env("GIT_COMMITTER_NAME", "Suaegi Test")
            .env("GIT_COMMITTER_EMAIL", "suaegi@example.invalid")
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn setup_fork_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().expect("tempdir");
        let origin = temp.path().join("origin.git");
        let upstream = temp.path().join("upstream.git");
        let seed = temp.path().join("seed");
        let checkout = temp.path().join("checkout");
        git(
            temp.path(),
            &["init", "--bare", "-b", "main", origin.to_str().unwrap()],
        );
        git(
            temp.path(),
            &["init", "--bare", "-b", "main", upstream.to_str().unwrap()],
        );
        git(temp.path(), &["init", "-b", "main", seed.to_str().unwrap()]);
        std::fs::write(seed.join("work.txt"), "base\n").expect("write base");
        git(&seed, &["add", "work.txt"]);
        git(&seed, &["commit", "-m", "base"]);
        git(
            &seed,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        git(
            &seed,
            &["remote", "add", "upstream", upstream.to_str().unwrap()],
        );
        git(&seed, &["push", "origin", "main"]);
        git(&seed, &["push", "upstream", "main"]);
        git(
            temp.path(),
            &[
                "clone",
                "--branch",
                "main",
                origin.to_str().unwrap(),
                checkout.to_str().unwrap(),
            ],
        );
        git(
            &checkout,
            &["remote", "add", "upstream", upstream.to_str().unwrap()],
        );

        std::fs::write(seed.join("work.txt"), "base\nupstream\n").expect("write upstream");
        git(&seed, &["add", "work.txt"]);
        git(&seed, &["commit", "-m", "upstream"]);
        git(&seed, &["push", "upstream", "main"]);

        for (slug, path) in [
            ("https://github.com/fork/orca.git", &origin),
            ("https://github.com/stablyai/orca.git", &upstream),
        ] {
            let file_url = url::Url::from_file_path(path)
                .expect("file URL")
                .to_string();
            git(
                &checkout,
                &["config", &format!("url.{file_url}.insteadOf"), slug],
            );
        }
        git(
            &checkout,
            &[
                "remote",
                "set-url",
                "origin",
                "https://github.com/fork/orca.git",
            ],
        );
        git(
            &checkout,
            &[
                "remote",
                "set-url",
                "upstream",
                "https://github.com/stablyai/orca.git",
            ],
        );
        (temp, checkout, origin)
    }

    #[test]
    fn parses_supported_github_remote_forms() {
        for value in [
            "git@github.com:stablyai/orca.git",
            "ssh://git@github.com/stablyai/orca.git",
            "https://github.com/stablyai/orca.git",
            "github:stablyai/orca",
        ] {
            assert_eq!(
                github_remote_identity(value),
                Some(GithubRepositoryIdentitySetting {
                    owner: "stablyai".to_string(),
                    repo: "orca".to_string(),
                })
            );
        }
    }

    #[test]
    fn rejects_non_github_and_ambiguous_paths() {
        assert_eq!(
            github_remote_identity("ssh://evil.example/stablyai/orca.git"),
            None
        );
        assert_eq!(
            github_remote_identity("https://github.com/extra/stablyai/orca.git"),
            None
        );
    }

    #[tokio::test]
    async fn safely_fast_forwards_only_the_origin_default_branch() {
        let (_temp, checkout, origin) = setup_fork_fixture();
        let before = git(&origin, &["rev-parse", "refs/heads/main"]);
        let result = sync_default_branch_inner(
            checkout,
            GithubRepositoryIdentitySetting {
                owner: "stablyai".to_string(),
                repo: "orca".to_string(),
            },
            false,
        )
        .await
        .expect("sync result");
        let after = git(&origin, &["rev-parse", "refs/heads/main"]);

        assert_eq!(
            result.status,
            ForkSyncStatus::Synced,
            "unexpected result: {result:?}"
        );
        assert_eq!(result.ahead, 0);
        assert_eq!(result.behind, 1);
        assert_ne!(before, after);
    }

    #[tokio::test]
    async fn rejects_an_upstream_identity_mismatch_before_fetch_or_push() {
        let (_temp, checkout, origin) = setup_fork_fixture();
        let before = git(&origin, &["rev-parse", "refs/heads/main"]);
        let result = sync_default_branch(
            checkout,
            GithubRepositoryIdentitySetting {
                owner: "someone-else".to_string(),
                repo: "orca".to_string(),
            },
        )
        .await
        .expect("sync result");
        let after = git(&origin, &["rev-parse", "refs/heads/main"]);

        assert_eq!(result.status, ForkSyncStatus::Blocked);
        assert_eq!(result.reason, Some(ForkSyncBlockedReason::UpstreamMismatch));
        assert_eq!(before, after);
    }
}
