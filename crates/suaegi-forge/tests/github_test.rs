//! GhForge를 스크립트 fake gh(PATH 앞에 얹음)로 실 단위 테스트한다. 출력 파싱·exit code
//! 분류·None/Unavailable/Found 분기를 트레잇 추상화 없이 검증한다(플랜 §5).

mod fixture;

use fixture::{env_lock, FakeGh};
use suaegi_forge::{
    ForgeError, ForgeProvider, ForgeUnavailable, GhForge, RepoCoords, Review, ReviewLookup,
    ReviewState,
};

fn coords() -> RepoCoords {
    RepoCoords {
        owner: "acme".into(),
        repo: "widget".into(),
        host: "github.com".into(),
    }
}

const PR_VIEW_JSON: &str =
    r#"{"number":57,"title":"Fix the bug","state":"OPEN","url":"https://github.com/acme/widget/pull/57","isDraft":false}"#;

#[tokio::test]
async fn resolve_repository_reads_owner_name_host() {
    let _g = env_lock();
    let fake = FakeGh::new().rule(
        "repo view",
        r#"{"name":"widget","owner":{"login":"acme"},"url":"https://ghe.corp.example/acme/widget"}"#,
        "",
        0,
    );
    let _p = fake.activate();
    let dir = tempfile::tempdir().unwrap();
    let repo = GhForge::new()
        .resolve_repository(dir.path())
        .await
        .unwrap()
        .expect("some repo");
    assert_eq!(repo.owner, "acme");
    assert_eq!(repo.repo, "widget");
    assert_eq!(repo.host, "ghe.corp.example");
}

#[tokio::test]
async fn resolve_repository_none_when_not_github() {
    let _g = env_lock();
    let fake = FakeGh::new().rule(
        "repo view",
        "",
        "none of the git remotes configured for this repository point to a known GitHub host\n",
        1,
    );
    let _p = fake.activate();
    let dir = tempfile::tempdir().unwrap();
    let out = GhForge::new().resolve_repository(dir.path()).await.unwrap();
    assert_eq!(out, None);
}

#[tokio::test]
async fn review_for_branch_found_with_checks() {
    let _g = env_lock();
    let fake = FakeGh::new()
        .rule("pr view", PR_VIEW_JSON, "", 0)
        .rule(
            "pr checks",
            r#"[{"bucket":"pass"},{"bucket":"pass"},{"bucket":"fail"},{"bucket":"pending"}]"#,
            "",
            8,
        );
    let _p = fake.activate();
    let lookup = GhForge::new().review_for_branch(&coords(), "feat").await;
    match lookup {
        ReviewLookup::Found(Review {
            number,
            state,
            checks,
            ..
        }) => {
            assert_eq!(number, 57);
            assert_eq!(state, ReviewState::Open);
            assert_eq!(checks.passing, 2);
            assert_eq!(checks.failing, 1);
            assert_eq!(checks.pending, 1);
        }
        other => panic!("expected Found, got {other:?}"),
    }
}

#[tokio::test]
async fn review_for_branch_none_on_no_pr_stderr() {
    let _g = env_lock();
    // "PR 없음"은 성공 데이터가 아니라 비-0 exit + 고정 stderr다(§3.3 S3).
    let fake = FakeGh::new().rule(
        "pr view",
        "",
        "no pull requests found for branch \"feat\"\n",
        1,
    );
    let _p = fake.activate();
    let lookup = GhForge::new().review_for_branch(&coords(), "feat").await;
    assert_eq!(lookup, ReviewLookup::None);
}

/// **회귀 방어 (a) — Unavailable→None 붕괴.** 일시 gh 오류가 알려진 PR을 지우면 안 된다.
/// 레이트리밋 실패는 반드시 `Unavailable`이지 `None`이 아니어야 한다. classifier/라우팅을
/// mutate해 이걸 None으로 접으면 이 단언이 깨진다.
#[tokio::test]
async fn transient_gh_error_is_unavailable_not_none() {
    let _g = env_lock();
    let fake = FakeGh::new().rule(
        "pr view",
        "",
        "HTTP 429: API rate limit exceeded (https://api.github.com/...)\n",
        1,
    );
    let _p = fake.activate();
    let lookup = GhForge::new().review_for_branch(&coords(), "feat").await;
    assert_ne!(
        lookup,
        ReviewLookup::None,
        "a transient gh error must NOT read as 'no PR' — it would erase known PR state"
    );
    assert_eq!(
        lookup,
        ReviewLookup::Unavailable(ForgeUnavailable::RateLimited)
    );
}

/// 성공 exit인데 JSON이 안 풀리면 None이 아니라 Unavailable이다(같은 붕괴 방어의 다른 갈래).
#[tokio::test]
async fn success_with_unparseable_json_is_unavailable_not_none() {
    let _g = env_lock();
    let fake = FakeGh::new().rule("pr view", "not json at all", "", 0);
    let _p = fake.activate();
    let lookup = GhForge::new().review_for_branch(&coords(), "feat").await;
    assert!(
        matches!(lookup, ReviewLookup::Unavailable(_)),
        "unexpected output must be Unavailable, not None: {lookup:?}"
    );
}

#[tokio::test]
async fn review_for_branch_unauthenticated_is_classified() {
    let _g = env_lock();
    let fake = FakeGh::new().rule(
        "pr view",
        "",
        "gh auth login required: you are not logged in to any GitHub hosts\n",
        1,
    );
    let _p = fake.activate();
    let lookup = GhForge::new().review_for_branch(&coords(), "feat").await;
    assert_eq!(
        lookup,
        ReviewLookup::Unavailable(ForgeUnavailable::NotAuthenticated)
    );
}

#[tokio::test]
async fn create_review_parses_pr_number_from_url() {
    let _g = env_lock();
    let fake = FakeGh::new()
        .rule(
            "repo view",
            r#"{"name":"widget","owner":{"login":"acme"},"url":"https://github.com/acme/widget"}"#,
            "",
            0,
        )
        .rule(
            "pr create",
            "https://github.com/acme/widget/pull/99\n",
            "",
            0,
        );
    let _p = fake.activate();
    let dir = tempfile::tempdir().unwrap();
    let review = GhForge::new()
        .create_review(suaegi_forge::CreateReviewInput {
            worktree_path: dir.path().to_path_buf(),
            base: "main".into(),
            head: Some("feat".into()),
            title: "My PR".into(),
            body: "body".into(),
            use_template: false,
            draft: false,
        })
        .await
        .expect("created");
    assert_eq!(review.number, 99);
    assert_eq!(review.url, "https://github.com/acme/widget/pull/99");
    assert_eq!(review.state, ReviewState::Open);
}

#[tokio::test]
async fn create_review_already_exists_is_validation() {
    let _g = env_lock();
    let fake = FakeGh::new()
        .rule(
            "repo view",
            r#"{"name":"widget","owner":{"login":"acme"},"url":"https://github.com/acme/widget"}"#,
            "",
            0,
        )
        .rule(
            "pr create",
            "",
            "a pull request for branch \"feat\" into branch \"main\" already exists\n",
            1,
        );
    let _p = fake.activate();
    let dir = tempfile::tempdir().unwrap();
    let err = GhForge::new()
        .create_review(suaegi_forge::CreateReviewInput {
            worktree_path: dir.path().to_path_buf(),
            base: "main".into(),
            head: Some("feat".into()),
            title: "My PR".into(),
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
    // resolve만 성공하면 되고, base==head 검증이 gh create 호출 전에 막는다.
    let fake = FakeGh::new().rule(
        "repo view",
        r#"{"name":"widget","owner":{"login":"acme"},"url":"https://github.com/acme/widget"}"#,
        "",
        0,
    );
    let _p = fake.activate();
    let dir = tempfile::tempdir().unwrap();
    let err = GhForge::new()
        .create_review(suaegi_forge::CreateReviewInput {
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
