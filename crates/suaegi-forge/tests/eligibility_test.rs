//! creation-eligibility 게이팅(플랜 §3.4). **실제 git**으로 `@{u}` 체크를, **fake gh**로
//! preflight/resolve/PR 조회를 검증한다.

mod fixture;

use fixture::{env_lock, init_repo_with_branch, FakeGh};
use suaegi_forge::{
    creation_eligibility, CreationBlockedReason, CreationEligibility, ForgeUnavailable, GhForge,
    GhRunner,
};
use suaegi_git::runner::GitRunner;

const REPO_VIEW_JSON: &str =
    r#"{"name":"widget","owner":{"login":"acme"},"url":"https://github.com/acme/widget"}"#;
const PR_VIEW_JSON: &str =
    r#"{"number":57,"title":"Fix","state":"OPEN","url":"https://github.com/acme/widget/pull/57","isDraft":false}"#;

fn ready_github(pr_stdout: &str, pr_stderr: &str, pr_exit: i32) -> FakeGh {
    FakeGh::new()
        .with_ready_preflight()
        .rule("repo view", REPO_VIEW_JSON, "", 0)
        .rule("pr view", pr_stdout, pr_stderr, pr_exit)
}

async fn eligibility(dir: &std::path::Path, branch: &str) -> CreationEligibility {
    creation_eligibility(
        &GhForge::new(),
        &GitRunner::new(),
        &GhRunner::new(),
        dir,
        branch,
    )
    .await
}

#[tokio::test]
async fn eligible_when_pushed_github_branch_has_no_pr() {
    let dir = tempfile::tempdir().unwrap();
    init_repo_with_branch(dir.path(), "feat", true); // upstream 있음
    let _g = env_lock();
    let fake = ready_github("", "no pull requests found for branch \"feat\"\n", 1);
    let _p = fake.activate();
    assert_eq!(eligibility(dir.path(), "feat").await, CreationEligibility::Eligible);
}

/// **회귀 방어 (c) — upstream 게이트.** push 안 된 브랜치는 자격이 없다. `@{u}` 체크를
/// 지우면(mutation) 이 테스트가 Eligible을 받고 실패한다.
#[tokio::test]
async fn blocked_no_upstream_when_branch_not_pushed() {
    let dir = tempfile::tempdir().unwrap();
    init_repo_with_branch(dir.path(), "feat", false); // upstream 없음
    let _g = env_lock();
    // gh는 ready + PR 없음이라 upstream만이 유일한 차단 사유여야 한다.
    let fake = ready_github("", "no pull requests found for branch \"feat\"\n", 1);
    let _p = fake.activate();
    assert_eq!(
        eligibility(dir.path(), "feat").await,
        CreationEligibility::Blocked(CreationBlockedReason::NoUpstream)
    );
}

#[tokio::test]
async fn blocked_already_exists_when_pr_found() {
    let dir = tempfile::tempdir().unwrap();
    init_repo_with_branch(dir.path(), "feat", true);
    let _g = env_lock();
    let fake = ready_github(PR_VIEW_JSON, "", 0);
    let _p = fake.activate();
    assert_eq!(
        eligibility(dir.path(), "feat").await,
        CreationEligibility::Blocked(CreationBlockedReason::AlreadyExists)
    );
}

/// PR 조회가 일시 실패하면 자격을 **AlreadyExists로 뭉개지 않고** Unavailable로 둔다 —
/// 재시도 가능. (Degraded 규율의 eligibility 층 반영.)
#[tokio::test]
async fn blocked_unavailable_not_already_exists_on_transient() {
    let dir = tempfile::tempdir().unwrap();
    init_repo_with_branch(dir.path(), "feat", true);
    let _g = env_lock();
    let fake = ready_github("", "HTTP 429: API rate limit exceeded\n", 1);
    let _p = fake.activate();
    assert_eq!(
        eligibility(dir.path(), "feat").await,
        CreationEligibility::Blocked(CreationBlockedReason::Unavailable(
            ForgeUnavailable::RateLimited
        ))
    );
}

#[tokio::test]
async fn blocked_not_authenticated_short_circuits_before_git() {
    let dir = tempfile::tempdir().unwrap();
    init_repo_with_branch(dir.path(), "feat", true);
    let _g = env_lock();
    let fake = FakeGh::new()
        .rule("--version", "gh version 2.40.0 (2024-01-01)\n", "", 0)
        .rule("auth status", "", "not logged in; run gh auth login\n", 1);
    let _p = fake.activate();
    assert_eq!(
        eligibility(dir.path(), "feat").await,
        CreationEligibility::Blocked(CreationBlockedReason::NotAuthenticated)
    );
}

#[tokio::test]
async fn blocked_not_github_repo() {
    let dir = tempfile::tempdir().unwrap();
    init_repo_with_branch(dir.path(), "feat", true);
    let _g = env_lock();
    let fake = FakeGh::new()
        .with_ready_preflight()
        .rule(
            "repo view",
            "",
            "none of the git remotes point to a known GitHub host\n",
            1,
        );
    let _p = fake.activate();
    assert_eq!(
        eligibility(dir.path(), "feat").await,
        CreationEligibility::Blocked(CreationBlockedReason::NotGitHubRepo)
    );
}
