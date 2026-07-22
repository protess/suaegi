//! GlabForge를 스크립트 fake glab(PATH 앞에 얹음)으로 실 단위 테스트한다. github_test.rs와
//! pr_actions_test.rs를 미러하되 glab 커맨드 모양·GitLab 신호로. 출력 파싱·exit code 분류·
//! None/Unavailable/Found 분기를 트레잇 추상화 없이 검증한다.
//!
//! **사람 눈**: 실제 glab(설치본 없음)은 헤드리스로 못 돌린다 — fake glab은 커맨드 모양/
//! exit/분류만 검증한다.

mod glab_fixture;

use glab_fixture::{env_lock, init_gitlab_repo, FakeGlab};
use suaegi_forge::{
    CommentLookup, CreateReviewInput, ForgeError, ForgeProvider, ForgeUnavailable, GlabForge,
    MergeMethod, MergeOptions, MergeOutcome, MergeRejection, MergeabilityState, PrActions,
    PrReviewState, RepoCoords, Review, ReviewLookup, ReviewState, ReviewThreadLookup,
};

fn coords() -> RepoCoords {
    RepoCoords {
        owner: "acme".into(),
        repo: "widget".into(),
        host: "gitlab.com".into(),
    }
}

const MR_VIEW_JSON: &str = r#"{"iid":57,"title":"Fix the bug","state":"opened","web_url":"https://gitlab.com/acme/widget/-/merge_requests/57","draft":false,"head_pipeline":{"status":"success"}}"#;

// ---- resolve_repository ----------------------------------------------------

#[tokio::test]
async fn resolve_repository_reads_owner_name_host_from_origin() {
    let dir = tempfile::tempdir().unwrap();
    init_gitlab_repo(dir.path(), "https://gitlab.example.com/group/sub/widget.git");
    let repo = GlabForge::new()
        .resolve_repository(dir.path())
        .await
        .unwrap()
        .expect("some repo");
    assert_eq!(repo.owner, "group/sub");
    assert_eq!(repo.repo, "widget");
    assert_eq!(repo.host, "gitlab.example.com");
}

#[tokio::test]
async fn resolve_repository_none_when_not_gitlab() {
    let dir = tempfile::tempdir().unwrap();
    init_gitlab_repo(dir.path(), "https://github.com/acme/widget.git");
    let out = GlabForge::new().resolve_repository(dir.path()).await.unwrap();
    assert_eq!(out, None);
}

// ---- review lookup ---------------------------------------------------------

#[tokio::test]
async fn review_for_branch_found_with_pipeline_checks() {
    let _g = env_lock();
    let fake = FakeGlab::new().rule("mr view feat -R acme/widget --output json", MR_VIEW_JSON, "", 0);
    let _p = fake.activate();
    let lookup = GlabForge::new().review_for_branch(&coords(), "feat").await;
    match lookup {
        ReviewLookup::Found(Review {
            number,
            state,
            checks,
            ..
        }) => {
            assert_eq!(number, 57);
            assert_eq!(state, ReviewState::Open);
            assert_eq!(checks.passing, 1);
        }
        other => panic!("expected Found, got {other:?}"),
    }
}

#[tokio::test]
async fn review_for_branch_none_on_no_mr_stderr() {
    let _g = env_lock();
    // "MR 없음"은 성공 데이터가 아니라 비-0 exit + 고정 stderr다.
    let fake = FakeGlab::new().rule(
        "mr view",
        "",
        "no open merge request available for \"feat\"\n",
        1,
    );
    let _p = fake.activate();
    let lookup = GlabForge::new().review_for_branch(&coords(), "feat").await;
    assert_eq!(lookup, ReviewLookup::None);
}

/// **회귀 방어 (a) — Unavailable→None 붕괴.** 일시 glab 오류가 알려진 MR을 지우면 안 된다.
#[tokio::test]
async fn transient_glab_error_is_unavailable_not_none() {
    let _g = env_lock();
    let fake = FakeGlab::new().rule("mr view", "", "HTTP 429: rate limit exceeded\n", 1);
    let _p = fake.activate();
    let lookup = GlabForge::new().review_for_branch(&coords(), "feat").await;
    assert_ne!(
        lookup,
        ReviewLookup::None,
        "a transient glab error must NOT read as 'no MR' — it would erase known MR state"
    );
    assert_eq!(lookup, ReviewLookup::Unavailable(ForgeUnavailable::RateLimited));
}

/// project 404(repo 사라짐)는 "MR 없음"이 아니라 Unavailable이어야 한다(같은 붕괴 방어의 갈래).
#[tokio::test]
async fn project_404_is_unavailable_not_none() {
    let _g = env_lock();
    let fake = FakeGlab::new().rule("mr view", "", "GET .../projects/x: 404 Project Not Found\n", 1);
    let _p = fake.activate();
    let lookup = GlabForge::new().review_for_branch(&coords(), "feat").await;
    assert_ne!(lookup, ReviewLookup::None);
    assert!(matches!(lookup, ReviewLookup::Unavailable(_)));
}

#[tokio::test]
async fn success_with_unparseable_json_is_unavailable_not_none() {
    let _g = env_lock();
    let fake = FakeGlab::new().rule("mr view", "not json at all", "", 0);
    let _p = fake.activate();
    let lookup = GlabForge::new().review_for_branch(&coords(), "feat").await;
    assert!(
        matches!(lookup, ReviewLookup::Unavailable(_)),
        "unexpected output must be Unavailable, not None: {lookup:?}"
    );
}

#[tokio::test]
async fn review_for_branch_unauthenticated_is_classified() {
    let _g = env_lock();
    let fake = FakeGlab::new().rule("mr view", "", "HTTP 401: unauthorized\n", 1);
    let _p = fake.activate();
    let lookup = GlabForge::new().review_for_branch(&coords(), "feat").await;
    assert_eq!(
        lookup,
        ReviewLookup::Unavailable(ForgeUnavailable::NotAuthenticated)
    );
}

// ---- create ----------------------------------------------------------------

#[tokio::test]
async fn create_review_parses_mr_number_from_url() {
    let _g = env_lock();
    let dir = tempfile::tempdir().unwrap();
    init_gitlab_repo(dir.path(), "https://gitlab.com/acme/widget.git");
    let fake = FakeGlab::new().rule(
        "mr create",
        "https://gitlab.com/acme/widget/-/merge_requests/99\n",
        "",
        0,
    );
    let _p = fake.activate();
    let review = GlabForge::new()
        .create_review(CreateReviewInput {
            worktree_path: dir.path().to_path_buf(),
            base: "main".into(),
            head: Some("feat".into()),
            title: "My MR".into(),
            body: "body".into(),
            use_template: false,
            draft: false,
        })
        .await
        .expect("created");
    assert_eq!(review.number, 99);
    assert_eq!(review.url, "https://gitlab.com/acme/widget/-/merge_requests/99");
    assert_eq!(review.state, ReviewState::Open);
}

#[tokio::test]
async fn create_review_already_exists_is_validation() {
    let _g = env_lock();
    let dir = tempfile::tempdir().unwrap();
    init_gitlab_repo(dir.path(), "https://gitlab.com/acme/widget.git");
    let fake = FakeGlab::new().rule(
        "mr create",
        "",
        "a merge request already exists for this source branch\n",
        1,
    );
    let _p = fake.activate();
    let err = GlabForge::new()
        .create_review(CreateReviewInput {
            worktree_path: dir.path().to_path_buf(),
            base: "main".into(),
            head: Some("feat".into()),
            title: "My MR".into(),
            body: "body".into(),
            use_template: false,
            draft: false,
        })
        .await
        .unwrap_err();
    assert!(matches!(err, ForgeError::Validation(_)), "got {err:?}");
}

#[tokio::test]
async fn create_review_rejects_base_equal_head() {
    let _g = env_lock();
    let dir = tempfile::tempdir().unwrap();
    init_gitlab_repo(dir.path(), "https://gitlab.com/acme/widget.git");
    // resolve만 성공하면 되고, base==head 검증이 glab create 호출 전에 막는다.
    let fake = FakeGlab::new();
    let _p = fake.activate();
    let err = GlabForge::new()
        .create_review(CreateReviewInput {
            worktree_path: dir.path().to_path_buf(),
            base: "main".into(),
            head: Some("MAIN".into()), // 대소문자 무시 동일
            title: "t".into(),
            body: "b".into(),
            use_template: false,
            draft: false,
        })
        .await
        .unwrap_err();
    assert!(matches!(err, ForgeError::Validation(_)), "got {err:?}");
}

// ---- merge -----------------------------------------------------------------

#[tokio::test]
async fn merge_squash_success_uses_squash_flag() {
    let _g = env_lock();
    // 규칙 접두사가 "--squash"를 포함하므로, 코드가 --rebase나 잘못된 방식을 보내면 이 규칙이
    // 매칭되지 않아 fake glab이 exit 97(unexpected)로 떨어진다 → 플래그 매핑을 실검증.
    let fake = FakeGlab::new().rule("mr merge 57 -R acme/widget --yes --squash", "", "", 0);
    let _p = fake.activate();
    let out = GlabForge::new()
        .merge_pr(&coords(), 57, MergeMethod::Squash, MergeOptions::default())
        .await
        .expect("merge ok");
    assert_eq!(out, MergeOutcome::Merged);
}

#[tokio::test]
async fn merge_default_and_rebase_flags_are_distinct() {
    let _g = env_lock();
    // Merge는 플래그 없음(GitLab 기본 merge commit), Rebase는 --rebase.
    let fake = FakeGlab::new()
        .rule("mr merge 12 -R acme/widget --yes --rebase", "", "", 0) // rebase 먼저(더 구체)
        .rule("mr merge 12 -R acme/widget --yes", "", "", 0);
    let _p = fake.activate();
    let f = GlabForge::new();
    assert_eq!(
        f.merge_pr(&coords(), 12, MergeMethod::Merge, MergeOptions::default())
            .await
            .unwrap(),
        MergeOutcome::Merged
    );
    assert_eq!(
        f.merge_pr(&coords(), 12, MergeMethod::Rebase, MergeOptions::default())
            .await
            .unwrap(),
        MergeOutcome::Merged
    );
}

#[tokio::test]
async fn merge_with_remove_source_branch_appends_flag() {
    let _g = env_lock();
    let fake = FakeGlab::new().rule(
        "mr merge 57 -R acme/widget --yes --squash --remove-source-branch",
        "",
        "",
        0,
    );
    let _p = fake.activate();
    let out = GlabForge::new()
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
    let fake = FakeGlab::new().rule(
        "mr merge",
        "",
        "Merge request is not mergeable: has merge conflicts with the target branch\n",
        1,
    );
    let _p = fake.activate();
    let out = GlabForge::new()
        .merge_pr(&coords(), 57, MergeMethod::Squash, MergeOptions::default())
        .await
        .expect("rejection is Ok-data, not Err");
    assert_eq!(out, MergeOutcome::Rejected(MergeRejection::Conflict));
}

#[tokio::test]
async fn merge_permission_denied_is_rejected() {
    let _g = env_lock();
    let fake = FakeGlab::new().rule(
        "mr merge",
        "",
        "You are not allowed to merge this merge request\n",
        1,
    );
    let _p = fake.activate();
    let out = GlabForge::new()
        .merge_pr(&coords(), 57, MergeMethod::Squash, MergeOptions::default())
        .await
        .unwrap();
    assert_eq!(out, MergeOutcome::Rejected(MergeRejection::PermissionDenied));
}

/// **회귀 방어 (a) — 일시 실패가 확정 거부로 오독되면 안 된다.**
#[tokio::test]
async fn transient_merge_failure_is_unavailable_not_rejected() {
    let _g = env_lock();
    let fake = FakeGlab::new().rule("mr merge", "", "HTTP 429: rate limit exceeded\n", 1);
    let _p = fake.activate();
    let res = GlabForge::new()
        .merge_pr(&coords(), 57, MergeMethod::Squash, MergeOptions::default())
        .await;
    match res {
        Err(ForgeError::Unavailable(ForgeUnavailable::RateLimited)) => {}
        other => panic!("a transient merge failure must be Err(Unavailable::RateLimited), got {other:?}"),
    }
}

// ---- auto-merge ------------------------------------------------------------

#[tokio::test]
async fn set_auto_merge_uses_auto_merge_flag() {
    let _g = env_lock();
    let fake = FakeGlab::new().rule(
        "mr merge 57 -R acme/widget --yes --auto-merge --squash",
        "",
        "",
        0,
    );
    let _p = fake.activate();
    GlabForge::new()
        .set_auto_merge(&coords(), 57, MergeMethod::Squash)
        .await
        .expect("auto-merge ok");
}

#[tokio::test]
async fn set_auto_merge_transient_is_unavailable() {
    let _g = env_lock();
    let fake = FakeGlab::new().rule("mr merge", "", "HTTP 401: unauthorized\n", 1);
    let _p = fake.activate();
    let err = GlabForge::new()
        .set_auto_merge(&coords(), 57, MergeMethod::Squash)
        .await
        .unwrap_err();
    assert!(
        matches!(err, ForgeError::Unavailable(ForgeUnavailable::NotAuthenticated)),
        "got {err:?}"
    );
}

// ---- reviews (approvals) ---------------------------------------------------

const APPROVALS_JSON: &str = r#"{"approved_by":[
  {"user":{"username":"octocat"}},
  {"user":{"username":"hubot"}}
]}"#;

#[tokio::test]
async fn pr_reviews_parses_approvals() {
    let _g = env_lock();
    let fake = FakeGlab::new().rule(
        "api projects/acme%2Fwidget/merge_requests/57/approvals",
        APPROVALS_JSON,
        "",
        0,
    );
    let _p = fake.activate();
    let lookup = GlabForge::new().pr_reviews(&coords(), 57).await;
    match lookup {
        ReviewThreadLookup::Found(reviews) => {
            assert_eq!(reviews.len(), 2);
            assert_eq!(reviews[0].author, "octocat");
            assert_eq!(reviews[0].state, PrReviewState::Approved);
        }
        other => panic!("expected Found, got {other:?}"),
    }
}

/// **회귀 방어 (a) — 캐시-오염.** 일시 실패는 "리뷰 없음"(빈 Found)이 아니라 Unavailable.
#[tokio::test]
async fn pr_reviews_transient_failure_is_unavailable_not_empty() {
    let _g = env_lock();
    let fake = FakeGlab::new().rule("api", "", "HTTP 429: rate limit exceeded\n", 1);
    let _p = fake.activate();
    let lookup = GlabForge::new().pr_reviews(&coords(), 57).await;
    assert_ne!(
        lookup,
        ReviewThreadLookup::Found(vec![]),
        "a transient failure must NOT read as 'no reviews'"
    );
    assert_eq!(
        lookup,
        ReviewThreadLookup::Unavailable(ForgeUnavailable::RateLimited)
    );
}

// ---- comments (notes) ------------------------------------------------------

const NOTES_JSON: &str = r#"[
  {"author":{"username":"octocat"},"body":"first","created_at":"2024-01-02T00:00:00Z","system":false},
  {"author":{"username":"gitlab-bot"},"body":"changed the description","created_at":"2024-01-02T00:00:01Z","system":true},
  {"author":null,"body":"drive-by","created_at":"2024-01-03T00:00:00Z","system":false}
]"#;

#[tokio::test]
async fn pr_comments_parses_and_filters_system_notes() {
    let _g = env_lock();
    let fake = FakeGlab::new().rule(
        "api projects/acme%2Fwidget/merge_requests/57/notes",
        NOTES_JSON,
        "",
        0,
    );
    let _p = fake.activate();
    let lookup = GlabForge::new().pr_comments(&coords(), 57).await;
    match lookup {
        CommentLookup::Found(comments) => {
            // system note는 제외 → 2개.
            assert_eq!(comments.len(), 2);
            assert_eq!(comments[0].author, "octocat");
            assert_eq!(comments[0].body, "first");
            // null author → ghost.
            assert_eq!(comments[1].author, "ghost");
        }
        other => panic!("expected Found, got {other:?}"),
    }
}

/// **회귀 방어 (a) — 캐시-오염.** 일시 실패는 "코멘트 없음"이 아니라 Unavailable.
#[tokio::test]
async fn pr_comments_transient_failure_is_unavailable_not_empty() {
    let _g = env_lock();
    let fake = FakeGlab::new().rule("api", "", "could not resolve host gitlab.com\n", 1);
    let _p = fake.activate();
    let lookup = GlabForge::new().pr_comments(&coords(), 57).await;
    assert_ne!(lookup, CommentLookup::Found(vec![]));
    assert_eq!(lookup, CommentLookup::Unavailable(ForgeUnavailable::Network));
}

// ---- mergeability ----------------------------------------------------------

#[tokio::test]
async fn mergeability_parses_mergeable() {
    let _g = env_lock();
    let fake = FakeGlab::new().rule(
        "mr view 57 -R acme/widget --output json",
        r#"{"iid":57,"detailed_merge_status":"mergeable","merge_status":"can_be_merged"}"#,
        "",
        0,
    );
    let _p = fake.activate();
    let state = GlabForge::new().mergeability_state(&coords(), 57).await;
    assert_eq!(state, MergeabilityState::Mergeable);
}

#[tokio::test]
async fn mergeability_parses_conflicting_and_blocked() {
    let _g = env_lock();
    let f = GlabForge::new();
    {
        let fake = FakeGlab::new().rule(
            "mr view",
            r#"{"iid":57,"has_conflicts":true,"detailed_merge_status":"conflict"}"#,
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
        let fake = FakeGlab::new().rule(
            "mr view",
            r#"{"iid":57,"detailed_merge_status":"not_approved"}"#,
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

/// **회귀 방어 (b) — 일시 실패는 절대 Mergeable이 아니다.** 조회 실패는 `Unknown`이어야 한다.
#[tokio::test]
async fn mergeability_transient_failure_is_unknown_never_mergeable() {
    let _g = env_lock();
    let fake = FakeGlab::new().rule("mr view", "", "HTTP 429: rate limit exceeded\n", 1);
    let _p = fake.activate();
    let state = GlabForge::new().mergeability_state(&coords(), 57).await;
    assert_ne!(
        state,
        MergeabilityState::Mergeable,
        "a transient failure must NEVER read as Mergeable"
    );
    assert_eq!(state, MergeabilityState::Unknown);
}
