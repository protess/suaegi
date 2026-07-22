//! 7b PR 상호작용을 스크립트 fake gh(PATH 앞)로 실 단위 테스트한다. 커맨드 모양·exit
//! 분류·확정거부/일시실패 분기를 트레잇 추상화 없이 검증한다(플랜 §5, 7a와 같은 하네스).
//!
//! **사람 눈**: 실제 `gh pr merge`(파괴적, 실 PR 필요)는 헤드리스로 못 돌린다 — fake gh는
//! 커맨드 모양/exit/분류만 검증한다(플랜 §6).

mod fixture;

use fixture::{env_lock, FakeGh};
use suaegi_forge::{
    CommentLookup, ForgeError, ForgeUnavailable, GhForge, MergeMethod, MergeOptions, MergeOutcome,
    MergeRejection, MergeabilityState, PrActions, PrReviewState, RepoCoords, ReviewThreadLookup,
};

fn coords() -> RepoCoords {
    RepoCoords {
        owner: "acme".into(),
        repo: "widget".into(),
        host: "github.com".into(),
    }
}

// ---- merge ----------------------------------------------------------------

#[tokio::test]
async fn merge_squash_success_uses_squash_flag() {
    let _g = env_lock();
    // 규칙 접두사가 "--squash"를 포함하므로, 코드가 --merge/--rebase를 보내면 이 규칙이
    // 매칭되지 않아 fake gh가 exit 97(unexpected)로 떨어진다 → 플래그 매핑을 실검증.
    let fake = FakeGh::new().rule("pr merge 57 --squash --repo acme/widget", "", "", 0);
    let _p = fake.activate();
    let out = GhForge::new()
        .merge_pr(&coords(), 57, MergeMethod::Squash, MergeOptions::default())
        .await
        .expect("merge ok");
    assert_eq!(out, MergeOutcome::Merged);
}

#[tokio::test]
async fn merge_rebase_and_merge_flags_are_distinct() {
    let _g = env_lock();
    let fake = FakeGh::new()
        .rule("pr merge 12 --merge --repo acme/widget", "", "", 0)
        .rule("pr merge 34 --rebase --repo acme/widget", "", "", 0);
    let _p = fake.activate();
    let f = GhForge::new();
    assert_eq!(
        f.merge_pr(&coords(), 12, MergeMethod::Merge, MergeOptions::default())
            .await
            .unwrap(),
        MergeOutcome::Merged
    );
    assert_eq!(
        f.merge_pr(&coords(), 34, MergeMethod::Rebase, MergeOptions::default())
            .await
            .unwrap(),
        MergeOutcome::Merged
    );
}

#[tokio::test]
async fn merge_with_delete_branch_appends_flag() {
    let _g = env_lock();
    // --delete-branch가 있어야만 매칭 — 없으면 exit 97로 떨어져 Unavailable이 된다.
    let fake = FakeGh::new().rule(
        "pr merge 57 --squash --repo acme/widget --delete-branch",
        "",
        "",
        0,
    );
    let _p = fake.activate();
    let out = GhForge::new()
        .merge_pr(
            &coords(),
            57,
            MergeMethod::Squash,
            MergeOptions {
                delete_branch: true,
            },
        )
        .await
        .expect("merge ok");
    assert_eq!(out, MergeOutcome::Merged);
}

#[tokio::test]
async fn merge_conflict_is_rejected_data_not_error() {
    let _g = env_lock();
    let fake = FakeGh::new().rule(
        "pr merge",
        "",
        "Pull request is not mergeable: has merge conflicts with the base branch\n",
        1,
    );
    let _p = fake.activate();
    let out = GhForge::new()
        .merge_pr(&coords(), 57, MergeMethod::Squash, MergeOptions::default())
        .await
        .expect("rejection is Ok-data, not Err");
    assert_eq!(out, MergeOutcome::Rejected(MergeRejection::Conflict));
}

#[tokio::test]
async fn merge_permission_denied_is_rejected() {
    let _g = env_lock();
    let fake = FakeGh::new().rule(
        "pr merge",
        "",
        "You're not authorized to merge this pull request\n",
        1,
    );
    let _p = fake.activate();
    let out = GhForge::new()
        .merge_pr(&coords(), 57, MergeMethod::Squash, MergeOptions::default())
        .await
        .unwrap();
    assert_eq!(out, MergeOutcome::Rejected(MergeRejection::PermissionDenied));
}

/// **회귀 방어 (a) — 일시 실패가 확정 거부로 오독되면 안 된다.** 레이트리밋 merge 실패는
/// 반드시 `Err(Unavailable)`이지 `Ok(Rejected)`가 아니어야 한다(재시도하면 될 상황을
/// 확정 실패로 못박지 않는다).
#[tokio::test]
async fn transient_merge_failure_is_unavailable_not_rejected() {
    let _g = env_lock();
    let fake = FakeGh::new().rule(
        "pr merge",
        "",
        "HTTP 429: API rate limit exceeded (https://api.github.com/...)\n",
        1,
    );
    let _p = fake.activate();
    let res = GhForge::new()
        .merge_pr(&coords(), 57, MergeMethod::Squash, MergeOptions::default())
        .await;
    match res {
        Err(ForgeError::Unavailable(ForgeUnavailable::RateLimited)) => {}
        other => panic!("a transient merge failure must be Err(Unavailable::RateLimited), got {other:?}"),
    }
}

// ---- auto-merge -----------------------------------------------------------

#[tokio::test]
async fn set_auto_merge_uses_auto_flag() {
    let _g = env_lock();
    let fake = FakeGh::new().rule("pr merge 57 --auto --squash --repo acme/widget", "", "", 0);
    let _p = fake.activate();
    GhForge::new()
        .set_auto_merge(&coords(), 57, MergeMethod::Squash)
        .await
        .expect("auto-merge ok");
}

#[tokio::test]
async fn set_auto_merge_clean_status_is_validation() {
    let _g = env_lock();
    let fake = FakeGh::new().rule(
        "pr merge",
        "",
        "GraphQL: Pull request is in clean status (enablePullRequestAutoMerge)\n",
        1,
    );
    let _p = fake.activate();
    let err = GhForge::new()
        .set_auto_merge(&coords(), 57, MergeMethod::Squash)
        .await
        .unwrap_err();
    assert!(matches!(err, ForgeError::Validation(_)), "got {err:?}");
}

#[tokio::test]
async fn set_auto_merge_transient_is_unavailable() {
    let _g = env_lock();
    let fake = FakeGh::new().rule(
        "pr merge",
        "",
        "HTTP 401: Bad credentials\n",
        1,
    );
    let _p = fake.activate();
    let err = GhForge::new()
        .set_auto_merge(&coords(), 57, MergeMethod::Squash)
        .await
        .unwrap_err();
    assert!(
        matches!(err, ForgeError::Unavailable(ForgeUnavailable::NotAuthenticated)),
        "got {err:?}"
    );
}

// ---- reviews --------------------------------------------------------------

const REVIEWS_JSON: &str = r#"{"reviews":[
  {"author":{"login":"octocat"},"state":"APPROVED","body":"lgtm","submittedAt":"2024-01-02T00:00:00Z"},
  {"author":{"login":"hubot"},"state":"CHANGES_REQUESTED","body":"nit","submittedAt":"2024-01-03T00:00:00Z"},
  {"author":null,"state":"COMMENTED","body":"drive-by","submittedAt":"2024-01-04T00:00:00Z"}
]}"#;

#[tokio::test]
async fn pr_reviews_parses_summaries() {
    let _g = env_lock();
    let fake = FakeGh::new().rule("pr view 57 --repo acme/widget --json reviews", REVIEWS_JSON, "", 0);
    let _p = fake.activate();
    let lookup = GhForge::new().pr_reviews(&coords(), 57).await;
    match lookup {
        ReviewThreadLookup::Found(reviews) => {
            assert_eq!(reviews.len(), 3);
            assert_eq!(reviews[0].author, "octocat");
            assert_eq!(reviews[0].state, PrReviewState::Approved);
            assert_eq!(reviews[1].state, PrReviewState::ChangesRequested);
            // null author → ghost.
            assert_eq!(reviews[2].author, "ghost");
        }
        other => panic!("expected Found, got {other:?}"),
    }
}

#[tokio::test]
async fn pr_reviews_empty_is_found_empty_not_unavailable() {
    let _g = env_lock();
    let fake = FakeGh::new().rule("pr view", r#"{"reviews":[]}"#, "", 0);
    let _p = fake.activate();
    let lookup = GhForge::new().pr_reviews(&coords(), 57).await;
    assert_eq!(lookup, ReviewThreadLookup::Found(vec![]));
}

/// **회귀 방어 (a) — 캐시-오염.** 일시 gh 실패는 "리뷰 없음"(빈 Found)이 아니라
/// 분류된 `Unavailable`이어야 한다. 이걸 빈 Found로 접으면 이미 있는 리뷰가 화면에서 사라진다.
#[tokio::test]
async fn pr_reviews_transient_failure_is_unavailable_not_empty() {
    let _g = env_lock();
    let fake = FakeGh::new().rule("pr view", "", "HTTP 429: API rate limit exceeded\n", 1);
    let _p = fake.activate();
    let lookup = GhForge::new().pr_reviews(&coords(), 57).await;
    assert_ne!(
        lookup,
        ReviewThreadLookup::Found(vec![]),
        "a transient failure must NOT read as 'no reviews' — it would erase known reviews"
    );
    assert_eq!(
        lookup,
        ReviewThreadLookup::Unavailable(ForgeUnavailable::RateLimited)
    );
}

#[tokio::test]
async fn pr_reviews_unparseable_success_is_unavailable_not_empty() {
    let _g = env_lock();
    let fake = FakeGh::new().rule("pr view", "not json", "", 0);
    let _p = fake.activate();
    let lookup = GhForge::new().pr_reviews(&coords(), 57).await;
    assert!(
        matches!(lookup, ReviewThreadLookup::Unavailable(_)),
        "unexpected output must be Unavailable, not empty Found: {lookup:?}"
    );
}

// ---- comments -------------------------------------------------------------

const COMMENTS_JSON: &str = r#"{"comments":[
  {"author":{"login":"octocat"},"body":"first","createdAt":"2024-01-02T00:00:00Z","url":"https://github.com/acme/widget/pull/57#issuecomment-1"},
  {"author":{"login":"hubot"},"body":"second","createdAt":"2024-01-03T00:00:00Z","url":"https://github.com/acme/widget/pull/57#issuecomment-2"}
]}"#;

#[tokio::test]
async fn pr_comments_parses_list() {
    let _g = env_lock();
    let fake = FakeGh::new().rule("pr view 57 --repo acme/widget --json comments", COMMENTS_JSON, "", 0);
    let _p = fake.activate();
    let lookup = GhForge::new().pr_comments(&coords(), 57).await;
    match lookup {
        CommentLookup::Found(comments) => {
            assert_eq!(comments.len(), 2);
            assert_eq!(comments[0].author, "octocat");
            assert_eq!(comments[0].body, "first");
            assert_eq!(
                comments[1].url,
                "https://github.com/acme/widget/pull/57#issuecomment-2"
            );
        }
        other => panic!("expected Found, got {other:?}"),
    }
}

/// **회귀 방어 (a) — 캐시-오염.** 일시 실패는 "코멘트 없음"이 아니라 Unavailable.
#[tokio::test]
async fn pr_comments_transient_failure_is_unavailable_not_empty() {
    let _g = env_lock();
    let fake = FakeGh::new().rule(
        "pr view",
        "",
        "error connecting: could not resolve host api.github.com\n",
        1,
    );
    let _p = fake.activate();
    let lookup = GhForge::new().pr_comments(&coords(), 57).await;
    assert_ne!(
        lookup,
        CommentLookup::Found(vec![]),
        "a transient failure must NOT read as 'no comments'"
    );
    assert_eq!(
        lookup,
        CommentLookup::Unavailable(ForgeUnavailable::Network)
    );
}

// ---- mergeability ---------------------------------------------------------

#[tokio::test]
async fn mergeability_parses_mergeable() {
    let _g = env_lock();
    let fake = FakeGh::new().rule(
        "pr view 57 --repo acme/widget --json mergeable,mergeStateStatus,reviewDecision",
        r#"{"mergeable":"MERGEABLE","mergeStateStatus":"CLEAN","reviewDecision":"APPROVED"}"#,
        "",
        0,
    );
    let _p = fake.activate();
    let state = GhForge::new().mergeability_state(&coords(), 57).await;
    assert_eq!(state, MergeabilityState::Mergeable);
}

#[tokio::test]
async fn mergeability_parses_conflicting_and_blocked() {
    let _g = env_lock();
    let f = GhForge::new();
    {
        let fake = FakeGh::new().rule(
            "pr view",
            r#"{"mergeable":"CONFLICTING","mergeStateStatus":"DIRTY","reviewDecision":""}"#,
            "",
            0,
        );
        let _p = fake.activate();
        assert_eq!(
            f.mergeability_state(&coords(), 57).await,
            MergeabilityState::Conflicting
        );
    }
    {
        let fake = FakeGh::new().rule(
            "pr view",
            r#"{"mergeable":"MERGEABLE","mergeStateStatus":"CLEAN","reviewDecision":"REVIEW_REQUIRED"}"#,
            "",
            0,
        );
        let _p = fake.activate();
        assert_eq!(
            f.mergeability_state(&coords(), 57).await,
            MergeabilityState::Blocked
        );
    }
}

/// **회귀 방어 (b) — 일시 실패는 절대 Mergeable이 아니다.** 조회 실패는 `Unknown`이어야
/// 한다 — 이걸 Mergeable로 접으면 UI가 머지 불가한 PR에 직접 머지 버튼을 켠다.
#[tokio::test]
async fn mergeability_transient_failure_is_unknown_never_mergeable() {
    let _g = env_lock();
    let fake = FakeGh::new().rule("pr view", "", "HTTP 429: API rate limit exceeded\n", 1);
    let _p = fake.activate();
    let state = GhForge::new().mergeability_state(&coords(), 57).await;
    assert_ne!(
        state,
        MergeabilityState::Mergeable,
        "a transient failure must NEVER read as Mergeable"
    );
    assert_eq!(state, MergeabilityState::Unknown);
}

#[tokio::test]
async fn mergeability_unparseable_success_is_unknown() {
    let _g = env_lock();
    let fake = FakeGh::new().rule("pr view", "not json", "", 0);
    let _p = fake.activate();
    let state = GhForge::new().mergeability_state(&coords(), 57).await;
    assert_eq!(state, MergeabilityState::Unknown);
}
