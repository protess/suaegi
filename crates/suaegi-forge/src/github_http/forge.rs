//! `HttpGhForge` — 저장된 토큰으로 GitHub REST v3를 직접 치는 **세 번째** 백엔드. gh
//! CLI(`GhForge`)·glab(`GlabForge`)과 **같은 `ForgeProvider`/`PrActions` 트레잇 뒤**에 들어온다.
//! gh 백엔드가 stderr 문자열을 분류하는 자리에서, HTTP는 **상태코드+헤더**(더 신뢰도 높은
//! 신호)를 분류한다 — 하지만 found/none/unavailable·확정거부/일시실패 규율은 동일하다:
//! **일시(5xx/429/network) 실패는 절대 None·빈 Found·Rejected로 오독하지 않는다.**
//!
//! # 토큰 리댁션
//! 토큰은 오직 [`auth_headers`]에서 `Authorization` 헤더로만 노출된다([`Secret::expose`]).
//! 전송 에러/분류/로그 어디에도 토큰이 안 실린다 — 전송 에러는 고정 라벨만 담고
//! (`transport.rs`), 분류는 상태코드만 본다.
//!
//! # REST-only(v1) 한계 vs gh 경로
//! - **auto-merge 미지원**: GitHub auto-merge는 GraphQL 뮤테이션(`enablePullRequestAutoMerge`)
//!   이라 REST v1에서 뺀다 → 실행 가능한 Validation으로 안내한다.
//! - **브랜치 삭제 미지원**: REST merge 엔드포인트는 소스 브랜치를 안 지운다(별도 ref 삭제
//!   필요). 파괴적 기본을 피하려 v1은 `delete_branch`를 **무시**한다(Orca도 보수적).
//! - **CI 체크는 best-effort**: check-runs 엔드포인트로 롤업하되 실패해도 전체 조회를 안
//!   떨어뜨리고 빈 요약(모름)으로 둔다(gh `fetch_checks` 규율 미러).

use super::classify::{classify_http_merge_failure, classify_http_unavailable};
use super::parse::{
    api_base, parse_github_remote, user_login, HttpComment, HttpPr, HttpReview,
};
use suaegi_http::{
    HttpMethod, HttpRequest, HttpResponse, HttpTransport, ReqwestTransport, TransportError,
};
use crate::github::read_pr_template;
use crate::pr_actions::{
    CommentLookup, MergeFailure, MergeMethod, MergeOptions, MergeOutcome, MergeabilityState,
    PrActions, PrComment, PrReview, PrReviewState, ReviewThreadLookup,
};
use crate::provider::{
    ChecksSummary, CreateReviewInput, ForgeError, ForgeProvider, ForgeUnavailable, RepoCoords,
    Review, ReviewLookup, ReviewState,
};
use async_trait::async_trait;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use suaegi_secrets::Secret;

/// 토큰 키체인 좌표. service는 앱 이름, account는 호스트(멀티-호스트 확장 여지). env
/// fallback은 `SecretRequest::github`이 `GH_TOKEN`→`GITHUB_TOKEN` 순으로 잡는다.
pub const KEYCHAIN_SERVICE: &str = "suaegi";
pub const KEYCHAIN_ACCOUNT: &str = "github.com";

/// 읽기 조회 타임아웃(gh runner DEFAULT_TIMEOUT 미러).
const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// 쓰기(create/merge) 타임아웃 — 네트워크 왕복이라 넉넉히(gh CREATE_TIMEOUT 미러).
const WRITE_TIMEOUT: Duration = Duration::from_secs(60);

/// REST GitHub `ForgeProvider`/`PrActions` 구현. 전송은 주입 가능([`HttpTransport`]) —
/// 테스트는 fake 전송으로 real github.com을 안 친다.
#[derive(Clone)]
pub struct HttpGhForge {
    transport: Arc<dyn HttpTransport>,
    /// 저장된 토큰. `None`이면 인증 불가 → 모든 작업이 `Unavailable(NotAuthenticated)`.
    token: Option<Secret>,
}

/// Debug는 토큰을 **절대** 찍지 않는다(Secret가 이미 리댁션하지만 전체 표면을 고정 라벨로).
impl std::fmt::Debug for HttpGhForge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpGhForge")
            .field("authenticated", &self.token.is_some())
            .finish()
    }
}

impl HttpGhForge {
    /// 프로덕션 생성자: reqwest 전송 + 주어진 토큰(provider 선택이 시크릿에서 읽어 넘긴다).
    pub fn new(token: Option<Secret>) -> Self {
        Self {
            transport: Arc::new(ReqwestTransport::new()),
            token,
        }
    }

    /// 전송 주입 생성자(테스트/내부). 전송을 fake로 바꿔 canned 응답을 준다.
    pub fn with_transport(transport: Arc<dyn HttpTransport>, token: Option<Secret>) -> Self {
        Self { transport, token }
    }

    /// 토큰이 있으면 인증됨(= 이 백엔드가 실제로 API를 칠 수 있음). eligibility가 gh
    /// preflight 대신 이걸 본다.
    pub fn is_authenticated(&self) -> bool {
        self.token.is_some()
    }

    /// 요청 헤더를 만든다. **여기가 토큰이 노출되는 유일한 지점** — `Authorization`으로만.
    /// 토큰이 없으면 None(→ 호출부가 NotAuthenticated로 접는다).
    fn auth_headers(&self) -> Option<Vec<(String, String)>> {
        let token = self.token.as_ref()?;
        Some(vec![
            // expose()는 오직 여기서만. grep 감사점.
            ("Authorization".to_string(), format!("Bearer {}", token.expose())),
            ("Accept".to_string(), "application/vnd.github+json".to_string()),
            ("X-GitHub-Api-Version".to_string(), "2022-11-28".to_string()),
            ("User-Agent".to_string(), "suaegi".to_string()),
        ])
    }

    /// 요청을 보낸다. 토큰 없음/전송 실패 → `Err(ForgeUnavailable)`(분류됨, 토큰 없음).
    /// **HTTP 상태가 비-2xx여도 Ok(resp)로 돌려준다** — 상태 분류는 각 작업이 한다(None vs
    /// Unavailable 구별을 작업별로 정확히 하려는 것).
    async fn send(
        &self,
        method: HttpMethod,
        url: String,
        body: Option<String>,
        timeout: Duration,
    ) -> Result<HttpResponse, ForgeUnavailable> {
        let Some(headers) = self.auth_headers() else {
            return Err(ForgeUnavailable::NotAuthenticated);
        };
        let req = HttpRequest {
            method,
            url,
            headers,
            body,
            timeout,
        };
        match self.transport.execute(req).await {
            Ok(resp) => Ok(resp),
            // 전송 실패(타임아웃/연결)는 재시도 가능한 Network. **토큰/URL을 담지 않는다.**
            Err(TransportError::Timeout) | Err(TransportError::Connect(_)) => {
                Err(ForgeUnavailable::Network)
            }
        }
    }

    fn repos_url(&self, repo: &RepoCoords, suffix: &str) -> String {
        format!(
            "{}/repos/{}/{}{}",
            api_base(&repo.host),
            repo.owner,
            repo.repo,
            suffix
        )
    }

    /// PR 하나 → `Review`. checks는 best-effort(head sha가 있으면 check-runs 롤업, 실패는 빈 요약).
    async fn review_from_pr(&self, repo: &RepoCoords, pr: HttpPr) -> Review {
        let checks = match &pr.head {
            Some(h) if !h.sha.is_empty() => self.fetch_checks(repo, &h.sha).await,
            _ => ChecksSummary::default(),
        };
        Review {
            number: pr.number,
            state: pr.review_state(),
            title: pr.title.clone(),
            url: pr.html_url.clone(),
            checks,
        }
    }

    /// `GET /repos/{o}/{r}/commits/{sha}/check-runs` — best-effort. 실패/파싱 실패는 빈
    /// 요약(모름)이지 전체 조회를 안 떨어뜨린다(gh `fetch_checks` 규율 미러).
    async fn fetch_checks(&self, repo: &RepoCoords, sha: &str) -> ChecksSummary {
        let url = self.repos_url(repo, &format!("/commits/{sha}/check-runs"));
        let resp = match self.send(HttpMethod::Get, url, None, READ_TIMEOUT).await {
            Ok(resp) if resp.status == 200 => resp,
            _ => return ChecksSummary::default(),
        };
        match serde_json::from_str::<CheckRunsEnvelope>(&resp.body) {
            Ok(env) => summarize_check_runs(&env.check_runs),
            Err(_) => ChecksSummary::default(),
        }
    }

    /// 브랜치/번호 공통 조회 실패 매핑: 전송 에러(u) → Unavailable(u).
    async fn review_by_branch_inner(&self, repo: &RepoCoords, branch: &str) -> ReviewLookup {
        // head=owner:branch 필터. 값은 percent-encode(브랜치명에 /·특수문자 가능).
        let head = format!("{}:{}", encode_component(&repo.owner), encode_component(branch));
        let url = self.repos_url(repo, &format!("/pulls?head={head}&state=all&per_page=1"));
        let resp = match self.send(HttpMethod::Get, url, None, READ_TIMEOUT).await {
            Ok(resp) => resp,
            Err(u) => return ReviewLookup::Unavailable(u),
        };
        if resp.status == 200 {
            match serde_json::from_str::<Vec<HttpPr>>(&resp.body) {
                // 빈 배열(200) = **진짜 PR 없음** = None. non-empty = Found.
                Ok(prs) => match prs.into_iter().next() {
                    Some(pr) => ReviewLookup::Found(self.review_from_pr(repo, pr).await),
                    None => ReviewLookup::None,
                },
                // 200인데 파싱 실패 → **None이 아니다**(예상 밖 출력은 Unavailable).
                Err(_) => ReviewLookup::Unavailable(ForgeUnavailable::Other(
                    "unexpected GitHub response".to_string(),
                )),
            }
        } else {
            // 비-2xx는 목록 엔드포인트라 404=repo 없음 포함 전부 **분류된 Unavailable**.
            // (빈 결과는 200 []으로 오지 비-2xx로 오지 않는다 → 일시 실패가 None으로 안 샌다.)
            ReviewLookup::Unavailable(classify_response(&resp))
        }
    }

    async fn review_by_number_inner(&self, repo: &RepoCoords, number: u64) -> ReviewLookup {
        let url = self.repos_url(repo, &format!("/pulls/{number}"));
        let resp = match self.send(HttpMethod::Get, url, None, READ_TIMEOUT).await {
            Ok(resp) => resp,
            Err(u) => return ReviewLookup::Unavailable(u),
        };
        match resp.status {
            200 => match serde_json::from_str::<HttpPr>(&resp.body) {
                Ok(pr) => ReviewLookup::Found(self.review_from_pr(repo, pr).await),
                Err(_) => ReviewLookup::Unavailable(ForgeUnavailable::Other(
                    "unexpected GitHub response".to_string(),
                )),
            },
            // 404 = 이 번호의 PR이 없음 = None(gh "no pull requests found" 대응, 브랜치 폴백 허용).
            404 => ReviewLookup::None,
            // 그 밖(401/403/429/5xx 등) 일시/권한 = **분류된 Unavailable, 절대 None 아님**.
            _ => ReviewLookup::Unavailable(classify_response(&resp)),
        }
    }
}

/// 응답의 rate-limit 헤더를 읽어 상태를 분류한다(작업 공통).
fn classify_response(resp: &HttpResponse) -> ForgeUnavailable {
    classify_http_unavailable(
        resp.status,
        resp.header("x-ratelimit-remaining"),
        resp.header("retry-after"),
    )
}

/// `MergeMethod` → REST `merge_method` 값. **이 매핑이 load-bearing이다**(gh `gh_flag`와
/// 같은 규율) — 잘못 매핑하면 사용자가 고른 것과 다르게 히스토리를 쓴다.
fn merge_method_json(method: MergeMethod) -> &'static str {
    match method {
        MergeMethod::Merge => "merge",
        MergeMethod::Squash => "squash",
        MergeMethod::Rebase => "rebase",
    }
}

/// query 컴포넌트 percent-encoding(RFC3986 unreserved 외 전부 이스케이프). gitlab
/// `encoded_project`와 같은 규율.
fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `GET .../check-runs` 봉투.
#[derive(Debug, serde::Deserialize)]
struct CheckRunsEnvelope {
    #[serde(default)]
    check_runs: Vec<CheckRun>,
}

#[derive(Debug, serde::Deserialize)]
struct CheckRun {
    /// queued | in_progress | completed.
    #[serde(default)]
    status: String,
    /// completed일 때만: success|failure|neutral|cancelled|timed_out|action_required|skipped|...
    #[serde(default)]
    conclusion: Option<String>,
}

/// check-runs를 passing/failing/pending으로 요약한다(gh bucket·gitlab pipeline 롤업 정신).
/// 미완료 → pending. 완료+success/neutral → passing. 완료+실패류 → failing. skipped는
/// 비-차단이라 안 센다(gh가 skipping을 안 세는 것과 동일).
fn summarize_check_runs(runs: &[CheckRun]) -> ChecksSummary {
    let mut s = ChecksSummary::default();
    for r in runs {
        if r.status.to_ascii_lowercase() != "completed" {
            s.pending += 1;
            continue;
        }
        match r.conclusion.as_deref().unwrap_or("").to_ascii_lowercase().as_str() {
            "success" | "neutral" => s.passing += 1,
            "failure" | "timed_out" | "cancelled" | "action_required" | "startup_failure" => {
                s.failing += 1
            }
            // "skipped"·빈 값·미지 → 안 셈(보수적).
            _ => {}
        }
    }
    s
}

/// create PR 요청 바디.
#[derive(serde::Serialize)]
struct CreatePrBody<'a> {
    title: &'a str,
    body: &'a str,
    base: &'a str,
    head: &'a str,
    draft: bool,
}

/// merge 요청 바디.
#[derive(serde::Serialize)]
struct MergeBody<'a> {
    merge_method: &'a str,
}

#[async_trait]
impl ForgeProvider for HttpGhForge {
    async fn resolve_repository(&self, worktree: &Path) -> Result<Option<RepoCoords>, ForgeError> {
        // gh 백엔드는 `gh repo view`를 쓰지만 HTTP는 gh 부재가 전제라 origin 원격을 직접
        // 파싱한다(gitlab과 동일 축). API를 안 친다 — 브리프 지시.
        use suaegi_git::runner::{GitError, GitRunner};
        let git = GitRunner::new();
        match git.run(worktree, &["remote", "get-url", "origin"]).await {
            Ok(out) => Ok(parse_github_remote(out.stdout.trim())),
            // "no origin"/"not a git repo"는 exit 128 → Failed → "GitHub 아님"(None).
            Err(GitError::Failed { .. }) => Ok(None),
            Err(_) => Err(ForgeError::Unavailable(ForgeUnavailable::Other(
                "could not read git remote".to_string(),
            ))),
        }
    }

    async fn review_for_branch(&self, repo: &RepoCoords, branch: &str) -> ReviewLookup {
        self.review_by_branch_inner(repo, branch).await
    }

    async fn review_by_number(&self, repo: &RepoCoords, number: u64) -> ReviewLookup {
        self.review_by_number_inner(repo, number).await
    }

    fn supports_review_creation(&self) -> bool {
        true
    }

    async fn create_review(&self, input: CreateReviewInput) -> Result<Review, ForgeError> {
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

        // head: 명시됐으면 사용, 없으면 worktree의 현재 브랜치(REST는 head가 필수 —
        // gh처럼 암묵 기본이 없다).
        let head = match input.head.as_deref() {
            Some(h) => h.to_string(),
            None => match current_branch(&input.worktree_path).await {
                Some(b) => b,
                None => {
                    return Err(ForgeError::Validation(
                        "Create PR failed: could not determine the head branch.".to_string(),
                    ))
                }
            },
        };
        if head.eq_ignore_ascii_case(base) {
            return Err(ForgeError::Validation(
                "Create PR failed: choose a different base branch before creating a pull request."
                    .to_string(),
            ));
        }

        let body_text = if input.use_template && input.body.trim().is_empty() {
            read_pr_template(&input.worktree_path).unwrap_or_default()
        } else {
            input.body.clone()
        };

        let payload = CreatePrBody {
            title,
            body: &body_text,
            base,
            head: &head,
            draft: input.draft,
        };
        let json = serde_json::to_string(&payload)
            .map_err(|_| ForgeError::Parse("could not encode PR request".to_string()))?;

        let url = self.repos_url(&repo, "/pulls");
        let resp = self
            .send(HttpMethod::Post, url, Some(json), WRITE_TIMEOUT)
            .await
            .map_err(ForgeError::Unavailable)?;

        match resp.status {
            // 201 Created — 응답 JSON에서 번호를 직접 얻는다(gh와 달리 텍스트 파싱 불필요).
            201 => match serde_json::from_str::<HttpPr>(&resp.body) {
                Ok(pr) => Ok(Review {
                    number: pr.number,
                    state: if input.draft {
                        ReviewState::Draft
                    } else {
                        ReviewState::Open
                    },
                    title: title.to_string(),
                    url: pr.html_url,
                    checks: ChecksSummary::default(),
                }),
                Err(_) => Err(ForgeError::Parse(
                    "could not read the created PR from GitHub's response".to_string(),
                )),
            },
            // 422 = validation. "already exists"는 명시적 안내로, 그 밖은 정제된 일반 문구.
            422 => {
                let lower = resp.body.to_ascii_lowercase();
                if lower.contains("already exists") || lower.contains("pull request already exists") {
                    Err(ForgeError::Validation(
                        "A pull request already exists for this branch.".to_string(),
                    ))
                } else {
                    Err(ForgeError::Validation(
                        "Create PR failed: GitHub rejected the request (check base/head and commits)."
                            .to_string(),
                    ))
                }
            }
            // 그 밖(401/403/404/429/5xx) — 분류된 Unavailable.
            _ => Err(ForgeError::Unavailable(classify_response(&resp))),
        }
    }
}

#[async_trait]
impl PrActions for HttpGhForge {
    async fn merge_pr(
        &self,
        repo: &RepoCoords,
        number: u64,
        method: MergeMethod,
        _options: MergeOptions,
    ) -> Result<MergeOutcome, ForgeError> {
        // **파괴적**. UI가 먼저 확인한 뒤 부른다 — 이 백엔드는 auto-confirm을 안 한다.
        // delete_branch는 REST v1에서 무시한다(모듈 주석의 한계 참고).
        let payload = MergeBody {
            merge_method: merge_method_json(method),
        };
        let json = serde_json::to_string(&payload)
            .map_err(|_| ForgeError::Parse("could not encode merge request".to_string()))?;
        let url = self.repos_url(repo, &format!("/pulls/{number}/merge"));
        let resp = self
            .send(HttpMethod::Put, url, Some(json), WRITE_TIMEOUT)
            .await
            .map_err(ForgeError::Unavailable)?;

        if resp.status == 200 {
            return Ok(MergeOutcome::Merged);
        }
        // 확정적 거부(405/409/403-비리밋)는 데이터(Ok), 일시(5xx/429/network/401 등)는 에러.
        match classify_http_merge_failure(
            resp.status,
            resp.header("x-ratelimit-remaining"),
            resp.header("retry-after"),
            &resp.body,
        ) {
            MergeFailure::Rejected(reason) => Ok(MergeOutcome::Rejected(reason)),
            MergeFailure::Transient(u) => Err(ForgeError::Unavailable(u)),
        }
    }

    async fn set_auto_merge(
        &self,
        _repo: &RepoCoords,
        _number: u64,
        _method: MergeMethod,
    ) -> Result<(), ForgeError> {
        // GitHub auto-merge는 GraphQL 뮤테이션이라 REST-only v1에서 미지원.
        // 일시 실패가 아니므로 Unavailable이 아니라 실행 가능한 Validation으로 안내한다.
        Err(ForgeError::Validation(
            "Auto-merge isn't available with the token-only HTTP backend. Sign in with the gh CLI to use it."
                .to_string(),
        ))
    }

    async fn pr_reviews(&self, repo: &RepoCoords, number: u64) -> ReviewThreadLookup {
        let url = self.repos_url(repo, &format!("/pulls/{number}/reviews"));
        let resp = match self.send(HttpMethod::Get, url, None, READ_TIMEOUT).await {
            Ok(resp) => resp,
            Err(u) => return ReviewThreadLookup::Unavailable(u),
        };
        if resp.status == 200 {
            match serde_json::from_str::<Vec<HttpReview>>(&resp.body) {
                Ok(reviews) => ReviewThreadLookup::Found(
                    reviews
                        .into_iter()
                        .map(|r| PrReview {
                            author: user_login(r.user),
                            state: PrReviewState::from_api(&r.state),
                            body: r.body,
                            submitted_at: r.submitted_at,
                        })
                        .collect(),
                ),
                // 200인데 파싱 실패 → **빈 Found가 아니다** — Unavailable.
                Err(_) => ReviewThreadLookup::Unavailable(ForgeUnavailable::Other(
                    "unexpected GitHub response".to_string(),
                )),
            }
        } else {
            // 일시 실패는 "리뷰 없음"(빈 Found)이 아니라 분류된 Unavailable(캐시-오염 방지).
            ReviewThreadLookup::Unavailable(classify_response(&resp))
        }
    }

    async fn pr_comments(&self, repo: &RepoCoords, number: u64) -> CommentLookup {
        // 이슈-레벨 대화 코멘트(gh `--json comments` 대응) = issues/{n}/comments.
        let url = self.repos_url(repo, &format!("/issues/{number}/comments"));
        let resp = match self.send(HttpMethod::Get, url, None, READ_TIMEOUT).await {
            Ok(resp) => resp,
            Err(u) => return CommentLookup::Unavailable(u),
        };
        if resp.status == 200 {
            match serde_json::from_str::<Vec<HttpComment>>(&resp.body) {
                Ok(comments) => CommentLookup::Found(
                    comments
                        .into_iter()
                        .map(|c| PrComment {
                            author: user_login(c.user),
                            body: c.body,
                            created_at: c.created_at,
                            url: c.html_url,
                        })
                        .collect(),
                ),
                Err(_) => CommentLookup::Unavailable(ForgeUnavailable::Other(
                    "unexpected GitHub response".to_string(),
                )),
            }
        } else {
            CommentLookup::Unavailable(classify_response(&resp))
        }
    }

    async fn mergeability_state(&self, repo: &RepoCoords, number: u64) -> MergeabilityState {
        let url = self.repos_url(repo, &format!("/pulls/{number}"));
        let resp = match self.send(HttpMethod::Get, url, None, READ_TIMEOUT).await {
            Ok(resp) => resp,
            // 일시 실패는 Unknown(안전)이지 절대 Mergeable이 아니다.
            Err(_) => return MergeabilityState::Unknown,
        };
        if resp.status != 200 {
            return MergeabilityState::Unknown;
        }
        match serde_json::from_str::<HttpPr>(&resp.body) {
            Ok(pr) => pr.mergeability(),
            // 파싱 실패도 Unknown(안전).
            Err(_) => MergeabilityState::Unknown,
        }
    }
}

/// worktree의 현재 브랜치명(`git rev-parse --abbrev-ref HEAD`). detached HEAD("HEAD")나
/// 실패는 None.
async fn current_branch(worktree: &Path) -> Option<String> {
    use suaegi_git::runner::GitRunner;
    let git = GitRunner::new();
    let out = git
        .run(worktree, &["rev-parse", "--abbrev-ref", "HEAD"])
        .await
        .ok()?;
    let name = out.stdout.trim();
    if name.is_empty() || name == "HEAD" {
        None
    } else {
        Some(name.to_string())
    }
}

#[cfg(test)]
mod tests {
    use suaegi_http::FakeTransport;
    use super::*;

    fn repo() -> RepoCoords {
        RepoCoords {
            owner: "acme".into(),
            repo: "widget".into(),
            host: "github.com".into(),
        }
    }

    fn mk_forge(t: FakeTransport) -> (HttpGhForge, Arc<FakeTransport>) {
        let arc = Arc::new(t);
        let forge = HttpGhForge::with_transport(arc.clone(), Some(Secret::new("ghp_TESTTOKEN123")));
        (forge, arc)
    }

    #[test]
    fn merge_method_maps_to_the_right_json() {
        // **회귀 방어**: 방식→REST 값 매핑이 어긋나면 사용자가 고른 것과 다르게 머지된다.
        assert_eq!(merge_method_json(MergeMethod::Merge), "merge");
        assert_eq!(merge_method_json(MergeMethod::Squash), "squash");
        assert_eq!(merge_method_json(MergeMethod::Rebase), "rebase");
    }

    #[tokio::test]
    async fn no_token_is_not_authenticated_without_hitting_transport() {
        // 토큰 없으면 전송을 아예 안 치고 NotAuthenticated.
        let t = Arc::new(FakeTransport::default());
        let forge = HttpGhForge::with_transport(t.clone(), None);
        assert!(!forge.is_authenticated());
        let lookup = forge.review_for_branch(&repo(), "feat").await;
        assert_eq!(
            lookup,
            ReviewLookup::Unavailable(ForgeUnavailable::NotAuthenticated)
        );
        assert!(t.requests().is_empty(), "must not call transport without a token");
    }

    #[tokio::test]
    async fn branch_lookup_empty_array_is_none() {
        // 200 + [] = 진짜 PR 없음 = None.
        let (forge, _) = mk_forge(FakeTransport::ok_json(200, "[]"));
        assert_eq!(forge.review_for_branch(&repo(), "feat").await, ReviewLookup::None);
    }

    #[tokio::test]
    async fn branch_lookup_found_maps_fields() {
        let body = r#"[{"number":42,"title":"Add widget","state":"open","draft":false,
            "html_url":"https://github.com/acme/widget/pull/42","head":{"sha":""}}]"#;
        let (forge, _) = mk_forge(FakeTransport::ok_json(200, body));
        match forge.review_for_branch(&repo(), "feat").await {
            ReviewLookup::Found(r) => {
                assert_eq!(r.number, 42);
                assert_eq!(r.state, ReviewState::Open);
                assert_eq!(r.url, "https://github.com/acme/widget/pull/42");
            }
            other => panic!("expected Found, got {other:?}"),
        }
    }

    /// **핵심 회귀 방어 (a)**: 일시 HTTP 실패(5xx/429)는 브랜치 조회에서 **None이 아니라
    /// Unavailable**. `classify_http_unavailable`/상태 분기를 mutate해 None으로 접으면 깨진다
    /// (캐시-오염: 알려진 PR이 지워진다).
    #[tokio::test]
    async fn transient_branch_lookup_is_unavailable_never_none() {
        for status in [500u16, 502, 503, 429] {
            let (forge, _) = mk_forge(FakeTransport::ok_json(status, ""));
            let lookup = forge.review_for_branch(&repo(), "feat").await;
            assert!(
                matches!(lookup, ReviewLookup::Unavailable(_)),
                "status {status} must be Unavailable, got {lookup:?}"
            );
            assert_ne!(lookup, ReviewLookup::None, "status {status} must not be None");
        }
    }

    /// **회귀 방어**: forge가 응답 **헤더**를 실제로 읽어 403+`x-ratelimit-remaining: 0`을
    /// RateLimited(재시도 가능)로 분류한다 — permission으로 오독하지 않는다. `classify_response`가
    /// 헤더를 안 넘기게 mutate하면 이게 permission으로 접혀 깨진다.
    #[tokio::test]
    async fn branch_lookup_403_with_ratelimit_header_is_rate_limited() {
        let t = FakeTransport::with_response(
            403,
            &[("x-ratelimit-remaining", "0")],
            r#"{"message":"API rate limit exceeded"}"#,
        );
        let (forge, _) = mk_forge(t);
        assert_eq!(
            forge.review_for_branch(&repo(), "feat").await,
            ReviewLookup::Unavailable(ForgeUnavailable::RateLimited)
        );
    }

    /// 전송(네트워크) 실패도 None이 아니라 Unavailable(Network).
    #[tokio::test]
    async fn transport_failure_branch_lookup_is_unavailable_network() {
        let (forge, _) = mk_forge(FakeTransport::with_error(TransportError::Timeout));
        assert_eq!(
            forge.review_for_branch(&repo(), "feat").await,
            ReviewLookup::Unavailable(ForgeUnavailable::Network)
        );
    }

    /// by-number: 404 = PR 없음 = None(브랜치 폴백 허용). 하지만 429/5xx는 Unavailable.
    #[tokio::test]
    async fn by_number_404_is_none_but_transient_is_unavailable() {
        let (forge, _) = mk_forge(FakeTransport::ok_json(404, r#"{"message":"Not Found"}"#));
        assert_eq!(forge.review_by_number(&repo(), 7).await, ReviewLookup::None);

        let (forge2, _) = mk_forge(FakeTransport::ok_json(503, ""));
        assert!(matches!(
            forge2.review_by_number(&repo(), 7).await,
            ReviewLookup::Unavailable(_)
        ));
    }

    /// **핵심 회귀 방어 (b)**: 일시 merge 실패는 `Rejected`로 날조되지 않는다.
    #[tokio::test]
    async fn transient_merge_failure_is_error_not_rejection() {
        let (forge, _) = mk_forge(FakeTransport::ok_json(503, ""));
        let out = forge
            .merge_pr(&repo(), 42, MergeMethod::Squash, MergeOptions::default())
            .await;
        assert!(
            matches!(out, Err(ForgeError::Unavailable(_))),
            "transient merge failure must be Err(Unavailable), got {out:?}"
        );
    }

    /// 확정 거부(405)는 Ok(Rejected) 데이터.
    #[tokio::test]
    async fn definitive_merge_rejection_is_ok_rejected() {
        let (forge, _) = mk_forge(FakeTransport::ok_json(405, "Pull Request is not mergeable"));
        let out = forge
            .merge_pr(&repo(), 42, MergeMethod::Merge, MergeOptions::default())
            .await;
        assert!(matches!(out, Ok(MergeOutcome::Rejected(_))), "got {out:?}");
    }

    #[tokio::test]
    async fn successful_merge_is_merged() {
        let (forge, _) = mk_forge(FakeTransport::ok_json(200, r#"{"merged":true}"#));
        let out = forge
            .merge_pr(&repo(), 42, MergeMethod::Merge, MergeOptions::default())
            .await;
        assert_eq!(out.unwrap(), MergeOutcome::Merged);
    }

    /// **토큰 리댁션 (c)**: 토큰을 먹인 뒤 에러를 유발해도, 반환된 에러의 Debug/Display 어디에도
    /// 토큰이 안 나온다. 그리고 토큰은 **Authorization 헤더로만** 실렸음을 확인한다.
    #[tokio::test]
    async fn token_never_appears_in_errors_only_in_auth_header() {
        const TOKEN: &str = "ghp_SUPERSECRET_DO_NOT_LEAK";
        let t = Arc::new(FakeTransport::ok_json(500, "boom internal error"));
        let forge = HttpGhForge::with_transport(t.clone(), Some(Secret::new(TOKEN)));

        // 조회 실패를 유발.
        let lookup = forge.review_for_branch(&repo(), "feat").await;
        let rendered = format!("{lookup:?}");
        assert!(!rendered.contains(TOKEN), "token leaked into lookup: {rendered}");

        // merge 실패도 확인.
        let merr = forge
            .merge_pr(&repo(), 1, MergeMethod::Merge, MergeOptions::default())
            .await;
        let mrendered = format!("{merr:?}");
        assert!(!mrendered.contains(TOKEN), "token leaked into merge error: {mrendered}");

        // forge 자체의 Debug도 토큰을 안 찍는다.
        assert!(!format!("{forge:?}").contains(TOKEN));

        // 하지만 토큰은 Authorization 헤더로는 실제로 실렸다(엉뚱한 곳이 아님).
        let auth = t.last_header("authorization").expect("authorization header sent");
        assert_eq!(auth, format!("Bearer {TOKEN}"));
    }

    /// pr_reviews: 일시 실패는 빈 Found가 아니라 Unavailable.
    #[tokio::test]
    async fn transient_reviews_is_unavailable_not_empty_found() {
        let (forge, _) = mk_forge(FakeTransport::ok_json(500, ""));
        assert!(matches!(
            forge.pr_reviews(&repo(), 1).await,
            ReviewThreadLookup::Unavailable(_)
        ));
    }

    #[tokio::test]
    async fn reviews_found_maps_state_and_ghost_author() {
        let body = r#"[{"user":{"login":"octocat"},"state":"APPROVED","body":"lgtm","submitted_at":"t"},
            {"user":null,"state":"COMMENTED","body":"","submitted_at":"t2"}]"#;
        let (forge, _) = mk_forge(FakeTransport::ok_json(200, body));
        match forge.pr_reviews(&repo(), 1).await {
            ReviewThreadLookup::Found(rs) => {
                assert_eq!(rs.len(), 2);
                assert_eq!(rs[0].author, "octocat");
                assert_eq!(rs[0].state, PrReviewState::Approved);
                assert_eq!(rs[1].author, "ghost");
            }
            other => panic!("expected Found, got {other:?}"),
        }
    }

    /// mergeability: 일시 실패는 Unknown, 절대 Mergeable 아님.
    #[tokio::test]
    async fn transient_mergeability_is_unknown_never_mergeable() {
        let (forge, _) = mk_forge(FakeTransport::ok_json(503, ""));
        assert_eq!(
            forge.mergeability_state(&repo(), 1).await,
            MergeabilityState::Unknown
        );
    }

    /// create 흐름을 실제 임시 git repo(github origin)로 end-to-end 통과시킨다: 201 응답의
    /// JSON에서 **번호를 복구**한다(gh와 달리 텍스트 파싱 불필요). 그리고 요청이 POST
    /// /repos/acme/widget/pulls로 갔는지, Authorization이 실렸는지 확인.
    #[tokio::test]
    async fn create_recovers_number_from_json_response() {
        let dir = tempfile::tempdir().unwrap();
        init_github_repo(dir.path()).await;
        let body = r#"{"number":123,"html_url":"https://github.com/acme/widget/pull/123","state":"open"}"#;
        let t = Arc::new(FakeTransport::ok_json(201, body));
        let forge = HttpGhForge::with_transport(t.clone(), Some(Secret::new("ghp_X")));
        let input = CreateReviewInput {
            worktree_path: dir.path().to_path_buf(),
            base: "main".into(),
            head: Some("feat".into()),
            title: "Add widget".into(),
            body: "desc".into(),
            use_template: false,
            draft: false,
        };
        let review = forge.create_review(input).await.expect("create ok");
        assert_eq!(review.number, 123);
        assert_eq!(review.url, "https://github.com/acme/widget/pull/123");
        let reqs = t.requests();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].method, HttpMethod::Post);
        assert!(reqs[0].url.ends_with("/repos/acme/widget/pulls"), "url: {}", reqs[0].url);
    }

    /// create 422 "already exists" → 명시적 Validation(원격은 진짜 repo 필요).
    #[tokio::test]
    async fn create_already_exists_maps_to_validation() {
        let dir = tempfile::tempdir().unwrap();
        init_github_repo(dir.path()).await;
        let t = Arc::new(FakeTransport::ok_json(
            422,
            r#"{"message":"Validation Failed","errors":[{"message":"A pull request already exists for acme:feat."}]}"#,
        ));
        let forge = HttpGhForge::with_transport(t, Some(Secret::new("ghp_X")));
        let input = CreateReviewInput {
            worktree_path: dir.path().to_path_buf(),
            base: "main".into(),
            head: Some("feat".into()),
            title: "t".into(),
            body: String::new(),
            use_template: false,
            draft: false,
        };
        match forge.create_review(input).await {
            Err(ForgeError::Validation(m)) => assert!(m.contains("already exists"), "{m}"),
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    /// 임시 디렉터리를 github origin을 가진 git repo로 만든다.
    async fn init_github_repo(dir: &Path) {
        use suaegi_git::runner::GitRunner;
        let git = GitRunner::new();
        git.run(dir, &["init", "-q"]).await.unwrap();
        git.run(dir, &["remote", "add", "origin", "https://github.com/acme/widget.git"])
            .await
            .unwrap();
    }

    #[test]
    fn check_runs_summary_counts() {
        let runs = vec![
            CheckRun { status: "completed".into(), conclusion: Some("success".into()) },
            CheckRun { status: "completed".into(), conclusion: Some("neutral".into()) },
            CheckRun { status: "completed".into(), conclusion: Some("failure".into()) },
            CheckRun { status: "completed".into(), conclusion: Some("timed_out".into()) },
            CheckRun { status: "in_progress".into(), conclusion: None },
            CheckRun { status: "completed".into(), conclusion: Some("skipped".into()) },
        ];
        let s = summarize_check_runs(&runs);
        assert_eq!(s.passing, 2);
        assert_eq!(s.failing, 2);
        assert_eq!(s.pending, 1);
    }
}
