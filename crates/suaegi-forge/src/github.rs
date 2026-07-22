use crate::classify::{classify_unavailable, is_no_pull_request};
use crate::parse::{
    parse_created_pr, summarize_checks, GhCheck, GhPrView, GhRepoView,
};
use crate::pr_actions::{
    classify_merge_failure, mergeability_from_fields, CommentLookup, GhCommentRaw, GhReviewRaw,
    MergeFailure, MergeMethod, MergeOptions, MergeOutcome, MergeabilityFields, MergeabilityState,
    PrActions, PrComment, PrReview, ReviewThreadLookup,
};
use crate::provider::{
    ChecksSummary, CreateReviewInput, ForgeError, ForgeProvider, ForgeUnavailable, RepoCoords,
    Review, ReviewLookup, ReviewState,
};
use crate::runner::{GhError, GhOutput, GhRunner, CREATE_TIMEOUT};
use async_trait::async_trait;
use std::path::Path;

/// preflight에서 고정하는 최소 gh 버전. `--json`은 gh 2.0 이후 널리 있으므로
/// Orca식 다중 폴백을 안 짊어지는 대신(플랜 §3.1) 여기서 하한을 못 박는다.
pub const MIN_GH_VERSION: (u32, u32) = (2, 0);

/// gh CLI를 통한 GitHub `ForgeProvider` 구현. 7a-1의 유일한 impl.
#[derive(Debug, Clone, Default)]
pub struct GhForge {
    runner: GhRunner,
}

impl GhForge {
    pub fn new() -> Self {
        Self {
            runner: GhRunner::new(),
        }
    }

    /// --repo가 명시된 호출용 중립 cwd. gh는 auth/config를 전역에서 읽으므로
    /// cwd가 무의미하다.
    fn neutral_cwd() -> &'static Path {
        Path::new(".")
    }
}

/// preflight 결과. gh 미설치와 미인증을 **구분**한다(플랜 §3.2, Orca `client.ts:1682`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Preflight {
    Ready,
    /// gh 없음.
    NotInstalled,
    /// gh 있지만 `gh auth status` 실패 → "gh auth login" 안내.
    NotAuthenticated,
    /// gh가 하한보다 낮음.
    OutdatedVersion { found: String, min: String },
}

/// gh 설치·버전·인증을 검사한다. 실패를 불투명하게 던지지 않고 분류해 돌려준다.
pub async fn preflight(runner: &GhRunner) -> Preflight {
    let cwd = Path::new(".");
    match runner.run(cwd, &["--version"]).await {
        Err(e) if e.is_gh_not_found() => return Preflight::NotInstalled,
        Err(_) => return Preflight::NotInstalled,
        Ok(out) => {
            if let Some((maj, min)) = parse_gh_version(&out.stdout) {
                if (maj, min) < MIN_GH_VERSION {
                    return Preflight::OutdatedVersion {
                        found: format!("{maj}.{min}"),
                        min: format!("{}.{}", MIN_GH_VERSION.0, MIN_GH_VERSION.1),
                    };
                }
            }
        }
    }
    // `gh auth status`는 인증돼 있으면 exit 0, 아니면 비-0(주로 1).
    match runner.run_expecting(cwd, &["auth", "status"], &[1]).await {
        Ok(out) if out.code == 0 => Preflight::Ready,
        Ok(_) => Preflight::NotAuthenticated,
        Err(e) if e.is_gh_not_found() => Preflight::NotInstalled,
        Err(_) => Preflight::NotAuthenticated,
    }
}

/// "gh version 2.40.0 (...)"의 첫 줄에서 (major, minor)를 뽑는다.
pub fn parse_gh_version(stdout: &str) -> Option<(u32, u32)> {
    let first = stdout.lines().next()?;
    // "version" 다음 토큰이 x.y.z.
    let token = first
        .split_whitespace()
        .skip_while(|t| *t != "version")
        .nth(1)?;
    let mut parts = token.split('.');
    let maj: u32 = parts.next()?.parse().ok()?;
    let min: u32 = parts.next().unwrap_or("0").parse().unwrap_or(0);
    Some((maj, min))
}

/// gh repo view 실패가 "GitHub repo가 아님"(→ None)인지 일시 오류(→ Unavailable)인지.
fn is_not_github_repo(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    lower.contains("not a git repository")
        || lower.contains("no git remotes")
        || lower.contains("none of the git remotes")
        || lower.contains("not point to a known github host")
        || lower.contains("to a known github host")
}

/// Io/Timeout/TooLarge 계열 GhError를 분류된 조회-불가로. (Failed는 호출부가 stderr로
/// 별도 처리한다.)
fn unavailable_from_gh_error(e: &GhError) -> ForgeUnavailable {
    if e.is_gh_not_found() {
        return ForgeUnavailable::NotInstalled;
    }
    match e {
        GhError::Timeout { .. } => ForgeUnavailable::Network,
        _ => ForgeUnavailable::Other("GitHub is unavailable".to_string()),
    }
}

impl GhForge {
    /// pr view/checks 한 조합 → ReviewLookup. selector는 브랜치명 또는 번호 문자열.
    async fn review_by_selector(&self, repo: &RepoCoords, selector: &str) -> ReviewLookup {
        let repo_arg = repo.repo_arg();
        let view = self
            .runner
            .run(
                Self::neutral_cwd(),
                &[
                    "pr",
                    "view",
                    selector,
                    "--repo",
                    &repo_arg,
                    "--json",
                    "number,title,state,url,isDraft",
                ],
            )
            .await;

        match view {
            Ok(out) => match serde_json::from_str::<GhPrView>(&out.stdout) {
                Ok(pr) => {
                    let checks = self.fetch_checks(&repo_arg, selector).await;
                    ReviewLookup::Found(Review {
                        number: pr.number,
                        state: pr.review_state(),
                        title: pr.title,
                        url: pr.url,
                        checks,
                    })
                }
                // 성공 exit인데 JSON이 안 풀리면 **None이 아니다** — 예상 밖 출력은
                // Unavailable이다. (None으로 접으면 §5 mutation이 잡는 붕괴다.)
                Err(_) => ReviewLookup::Unavailable(ForgeUnavailable::Other(
                    "unexpected gh output".to_string(),
                )),
            },
            Err(GhError::Failed { stderr, .. }) => {
                // 여기서만 None이 나온다: 비-0 exit + 고정 영어 stderr substring.
                if is_no_pull_request(&stderr) {
                    ReviewLookup::None
                } else {
                    ReviewLookup::Unavailable(classify_unavailable(&stderr))
                }
            }
            Err(e) => ReviewLookup::Unavailable(unavailable_from_gh_error(&e)),
        }
    }

    /// `gh pr checks` — best-effort. PR은 이미 Found이므로 체크 조회 실패는 전체를
    /// Unavailable로 떨어뜨리지 않고 빈 요약(모름)으로 돌린다.
    async fn fetch_checks(&self, repo_arg: &str, selector: &str) -> ChecksSummary {
        // pr checks는 실패 체크에 1, pending에 8을 낸다 — 둘 다 정상 데이터다.
        let res = self
            .runner
            .run_expecting(
                Self::neutral_cwd(),
                &[
                    "pr", "checks", selector, "--repo", repo_arg, "--json", "bucket",
                ],
                &[1, 8],
            )
            .await;
        let out: GhOutput = match res {
            Ok(out) => out,
            Err(_) => return ChecksSummary::default(),
        };
        // 체크가 하나도 없으면 gh는 "no checks reported"를 낸다 → 빈 요약.
        if out.stdout.trim().is_empty()
            || out.stderr.to_lowercase().contains("no checks reported")
        {
            return ChecksSummary::default();
        }
        match serde_json::from_str::<Vec<GhCheck>>(&out.stdout) {
            Ok(checks) => summarize_checks(&checks),
            Err(_) => ChecksSummary::default(),
        }
    }
}

#[async_trait]
impl ForgeProvider for GhForge {
    async fn resolve_repository(
        &self,
        worktree: &Path,
    ) -> Result<Option<RepoCoords>, ForgeError> {
        let out = self
            .runner
            .run(worktree, &["repo", "view", "--json", "owner,name,url"])
            .await;
        match out {
            Ok(out) => match serde_json::from_str::<GhRepoView>(&out.stdout) {
                Ok(rv) => Ok(Some(RepoCoords {
                    owner: rv.owner.login.clone(),
                    repo: rv.name.clone(),
                    host: rv.host(),
                })),
                Err(_) => Err(ForgeError::Parse(
                    "could not read repository identity from gh".to_string(),
                )),
            },
            Err(GhError::Failed { stderr, .. }) => {
                if is_not_github_repo(&stderr) {
                    Ok(None)
                } else {
                    Err(ForgeError::Unavailable(classify_unavailable(&stderr)))
                }
            }
            Err(e) => Err(ForgeError::Unavailable(unavailable_from_gh_error(&e))),
        }
    }

    async fn review_for_branch(&self, repo: &RepoCoords, branch: &str) -> ReviewLookup {
        self.review_by_selector(repo, branch).await
    }

    async fn review_by_number(&self, repo: &RepoCoords, number: u64) -> ReviewLookup {
        self.review_by_selector(repo, &number.to_string()).await
    }

    fn supports_review_creation(&self) -> bool {
        true
    }

    async fn create_review(&self, input: CreateReviewInput) -> Result<Review, ForgeError> {
        // create는 origin(head 브랜치를 소유한)을 대상으로 한다. worktree에서 좌표를 얻어
        // --repo를 host-qualify한다.
        let repo = match self.resolve_repository(&input.worktree_path).await? {
            Some(r) => r,
            None => {
                return Err(ForgeError::Validation(
                    "Creating pull requests requires a GitHub remote.".to_string(),
                ))
            }
        };

        let base = input.base.trim();
        let title = input.title.trim();
        if base.is_empty() || title.is_empty() {
            return Err(ForgeError::Validation(
                "Create PR failed: base branch and title are required.".to_string(),
            ));
        }
        if let Some(head) = input.head.as_deref() {
            if head.eq_ignore_ascii_case(base) {
                return Err(ForgeError::Validation(
                    "Create PR failed: choose a different base branch before creating a pull request."
                        .to_string(),
                ));
            }
        }

        // body 결정: use_template && body 비었으면 repo 템플릿을 채운다.
        let body = if input.use_template && input.body.trim().is_empty() {
            read_pr_template(&input.worktree_path).unwrap_or_default()
        } else {
            input.body.clone()
        };

        // body를 임시 파일로. TempDir는 run이 끝날 때까지 살려 둔다.
        let tmp = tempfile::Builder::new()
            .prefix("suaegi-pr-body-")
            .tempdir()
            .map_err(|e| ForgeError::Parse(format!("could not create temp body file: {e}")))?;
        let body_path = tmp.path().join("body.md");
        std::fs::write(&body_path, body)
            .map_err(|e| ForgeError::Parse(format!("could not write temp body file: {e}")))?;
        let body_path_str = body_path.to_string_lossy().into_owned();
        let repo_arg = repo.repo_arg();

        let mut args: Vec<&str> = vec![
            "pr",
            "create",
            "--repo",
            &repo_arg,
            "--base",
            base,
            "--title",
            title,
            "--body-file",
            &body_path_str,
        ];
        if let Some(head) = input.head.as_deref() {
            args.push("--head");
            args.push(head);
        }
        if input.draft {
            args.push("--draft");
        }

        let res = self
            .runner
            .run_with_timeout(&input.worktree_path, &args, CREATE_TIMEOUT)
            .await;

        match res {
            Ok(out) => match parse_created_pr(&out.stdout) {
                Some((number, url)) => Ok(Review {
                    number,
                    state: if input.draft {
                        ReviewState::Draft
                    } else {
                        ReviewState::Open
                    },
                    title: title.to_string(),
                    url,
                    checks: ChecksSummary::default(),
                }),
                None => Err(ForgeError::Parse(
                    "could not determine the created PR number from gh output".to_string(),
                )),
            },
            Err(GhError::Failed { stderr, .. }) => {
                let lower = stderr.to_lowercase();
                if lower.contains("already exists")
                    || lower.contains("a pull request for branch")
                {
                    Err(ForgeError::Validation(
                        "A pull request already exists for this branch.".to_string(),
                    ))
                } else {
                    Err(ForgeError::Unavailable(classify_unavailable(&stderr)))
                }
            }
            Err(e) => Err(ForgeError::Unavailable(unavailable_from_gh_error(&e))),
        }
    }
}

/// worktree에서 PR 템플릿을 찾아 읽는다(Orca `readPullRequestTemplate` 미러). 첫 매치만.
fn read_pr_template(worktree: &Path) -> Option<String> {
    const CANDIDATES: &[&str] = &[
        ".github/PULL_REQUEST_TEMPLATE.md",
        ".github/pull_request_template.md",
        "PULL_REQUEST_TEMPLATE.md",
        "pull_request_template.md",
        "docs/PULL_REQUEST_TEMPLATE.md",
        "docs/pull_request_template.md",
    ];
    for rel in CANDIDATES {
        let p = worktree.join(rel);
        if let Ok(text) = std::fs::read_to_string(&p) {
            return Some(text);
        }
    }
    None
}

/// `gh pr view --json reviews` 봉투(`{ "reviews": [...] }`).
#[derive(Debug, serde::Deserialize)]
struct GhReviewsEnvelope {
    #[serde(default)]
    reviews: Vec<GhReviewRaw>,
}

/// `gh pr view --json comments` 봉투(`{ "comments": [...] }`).
#[derive(Debug, serde::Deserialize)]
struct GhCommentsEnvelope {
    #[serde(default)]
    comments: Vec<GhCommentRaw>,
}

/// merge write op은 네트워크 왕복이라 create와 같은 넉넉한 타임아웃(60초)을 쓴다.
const MERGE_TIMEOUT: std::time::Duration = CREATE_TIMEOUT;

#[async_trait]
impl PrActions for GhForge {
    async fn merge_pr(
        &self,
        repo: &RepoCoords,
        number: u64,
        method: MergeMethod,
        options: MergeOptions,
    ) -> Result<MergeOutcome, ForgeError> {
        // **파괴적**. 이 백엔드는 auto-confirm을 하지 않는다 — UI가 먼저 확인한 뒤 부른다.
        let repo_arg = repo.repo_arg();
        let number_str = number.to_string();
        let mut args: Vec<&str> = vec![
            "pr",
            "merge",
            &number_str,
            method.gh_flag(),
            "--repo",
            &repo_arg,
        ];
        if options.delete_branch {
            args.push("--delete-branch");
        }

        let res = self
            .runner
            .run_with_timeout(Self::neutral_cwd(), &args, MERGE_TIMEOUT)
            .await;
        match res {
            Ok(_) => Ok(MergeOutcome::Merged),
            Err(GhError::Failed { stderr, .. }) => match classify_merge_failure(&stderr) {
                // 확정적 거부는 데이터(Ok), 일시 실패는 에러(Err) — None vs Unavailable 규율.
                MergeFailure::Rejected(reason) => Ok(MergeOutcome::Rejected(reason)),
                MergeFailure::Transient(u) => Err(ForgeError::Unavailable(u)),
            },
            Err(e) => Err(ForgeError::Unavailable(unavailable_from_gh_error(&e))),
        }
    }

    async fn set_auto_merge(
        &self,
        repo: &RepoCoords,
        number: u64,
        method: MergeMethod,
    ) -> Result<(), ForgeError> {
        let repo_arg = repo.repo_arg();
        let number_str = number.to_string();
        let args: Vec<&str> = vec![
            "pr",
            "merge",
            &number_str,
            "--auto",
            method.gh_flag(),
            "--repo",
            &repo_arg,
        ];
        let res = self
            .runner
            .run_with_timeout(Self::neutral_cwd(), &args, MERGE_TIMEOUT)
            .await;
        match res {
            Ok(_) => Ok(()),
            Err(GhError::Failed { stderr, .. }) => {
                // GitHub은 이미 머지 가능한 PR에 auto-merge를 거부한다("clean status") —
                // raw 에러 대신 실행 가능한 안내로(Orca `classifySetAutoMergeError`).
                if stderr.to_lowercase().contains("clean status") {
                    Err(ForgeError::Validation(
                        "This pull request can already be merged. Use Merge instead of auto-merge."
                            .to_string(),
                    ))
                } else {
                    Err(ForgeError::Unavailable(classify_unavailable(&stderr)))
                }
            }
            Err(e) => Err(ForgeError::Unavailable(unavailable_from_gh_error(&e))),
        }
    }

    async fn pr_reviews(&self, repo: &RepoCoords, number: u64) -> ReviewThreadLookup {
        let repo_arg = repo.repo_arg();
        let number_str = number.to_string();
        let res = self
            .runner
            .run(
                Self::neutral_cwd(),
                &[
                    "pr", "view", &number_str, "--repo", &repo_arg, "--json", "reviews",
                ],
            )
            .await;
        match res {
            Ok(out) => match serde_json::from_str::<GhReviewsEnvelope>(&out.stdout) {
                Ok(env) => {
                    ReviewThreadLookup::Found(env.reviews.into_iter().map(PrReview::from).collect())
                }
                // 성공 exit인데 JSON이 안 풀리면 **빈 Found가 아니다** — Unavailable이다.
                Err(_) => ReviewThreadLookup::Unavailable(ForgeUnavailable::Other(
                    "unexpected gh output".to_string(),
                )),
            },
            // 일시 실패는 "리뷰 없음"(빈 Found)이 아니라 분류된 Unavailable(캐시-오염 방지).
            Err(GhError::Failed { stderr, .. }) => {
                ReviewThreadLookup::Unavailable(classify_unavailable(&stderr))
            }
            Err(e) => ReviewThreadLookup::Unavailable(unavailable_from_gh_error(&e)),
        }
    }

    async fn pr_comments(&self, repo: &RepoCoords, number: u64) -> CommentLookup {
        let repo_arg = repo.repo_arg();
        let number_str = number.to_string();
        let res = self
            .runner
            .run(
                Self::neutral_cwd(),
                &[
                    "pr", "view", &number_str, "--repo", &repo_arg, "--json", "comments",
                ],
            )
            .await;
        match res {
            Ok(out) => match serde_json::from_str::<GhCommentsEnvelope>(&out.stdout) {
                Ok(env) => {
                    CommentLookup::Found(env.comments.into_iter().map(PrComment::from).collect())
                }
                Err(_) => CommentLookup::Unavailable(ForgeUnavailable::Other(
                    "unexpected gh output".to_string(),
                )),
            },
            Err(GhError::Failed { stderr, .. }) => {
                CommentLookup::Unavailable(classify_unavailable(&stderr))
            }
            Err(e) => CommentLookup::Unavailable(unavailable_from_gh_error(&e)),
        }
    }

    async fn mergeability_state(&self, repo: &RepoCoords, number: u64) -> MergeabilityState {
        let repo_arg = repo.repo_arg();
        let number_str = number.to_string();
        let res = self
            .runner
            .run(
                Self::neutral_cwd(),
                &[
                    "pr",
                    "view",
                    &number_str,
                    "--repo",
                    &repo_arg,
                    "--json",
                    "mergeable,mergeStateStatus,reviewDecision",
                ],
            )
            .await;
        match res {
            Ok(out) => match serde_json::from_str::<MergeabilityFields>(&out.stdout) {
                Ok(fields) => mergeability_from_fields(&fields),
                // 파싱 실패는 Unknown(안전) — 절대 Mergeable로 넘기지 않는다.
                Err(_) => MergeabilityState::Unknown,
            },
            // 일시 실패는 Unknown이지 절대 Mergeable이 아니다.
            Err(_) => MergeabilityState::Unknown,
        }
    }
}
