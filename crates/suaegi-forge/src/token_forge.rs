//! Token-authenticated hosted review providers used by Orca.
//!
//! Gitea/Forgejo and Azure DevOps support pull-request creation. Bitbucket is
//! intentionally read-only, matching Orca's desktop capability contract.

use crate::eligibility::{has_upstream, CreationBlockedReason, CreationEligibility};
use crate::pr_actions::{
    CommentLookup, MergeMethod, MergeOptions, MergeOutcome, MergeRejection, MergeabilityState,
    PrActions, PrComment, PrReview, PrReviewState, ReviewThreadLookup,
};
use crate::provider::{
    ChecksSummary, CreateReviewInput, ForgeError, ForgeProvider, ForgeUnavailable, RepoCoords,
    Review, ReviewLookup, ReviewState,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use suaegi_git::runner::GitRunner;
use suaegi_http::{
    HttpMethod, HttpRequest, HttpResponse, HttpTransport, ReqwestTransport, TransportError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenForgeKind {
    Gitea,
    AzureDevOps,
    Bitbucket,
}

#[derive(Clone)]
pub struct TokenForge {
    kind: TokenForgeKind,
    base_url: String,
    authorization: String,
    transport: Arc<dyn HttpTransport>,
}

impl std::fmt::Debug for TokenForge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenForge")
            .field("kind", &self.kind)
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

impl TokenForge {
    pub fn from_environment(kind: TokenForgeKind) -> Option<Self> {
        let value = |name: &str| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.trim().is_empty())
        };
        let (base_url, authorization) = match kind {
            TokenForgeKind::Gitea => {
                let base = normalize_gitea_base(&value("ORCA_GITEA_API_BASE_URL")?);
                let token = value("ORCA_GITEA_TOKEN")?;
                (base, format!("token {token}"))
            }
            TokenForgeKind::AzureDevOps => {
                let base = normalize_azure_base(&value("ORCA_AZURE_DEVOPS_API_BASE_URL")?);
                let token = value("ORCA_AZURE_DEVOPS_TOKEN")
                    .or_else(|| value("ORCA_AZURE_DEVOPS_PAT"))
                    .or_else(|| value("ORCA_AZURE_DEVOPS_ACCESS_TOKEN"))?;
                let username = value("ORCA_AZURE_DEVOPS_USERNAME").unwrap_or_default();
                (
                    base,
                    format!("Basic {}", base64_basic(&format!("{username}:{token}"))),
                )
            }
            TokenForgeKind::Bitbucket => {
                let base = value("ORCA_BITBUCKET_API_BASE_URL")
                    .unwrap_or_else(|| "https://api.bitbucket.org/2.0".into())
                    .trim_end_matches('/')
                    .to_string();
                if let Some(token) = value("ORCA_BITBUCKET_ACCESS_TOKEN") {
                    (base, format!("Bearer {token}"))
                } else {
                    let email = value("ORCA_BITBUCKET_EMAIL")?;
                    let token = value("ORCA_BITBUCKET_API_TOKEN")?;
                    (
                        base,
                        format!("Basic {}", base64_basic(&format!("{email}:{token}"))),
                    )
                }
            }
        };
        Some(Self {
            kind,
            base_url,
            authorization,
            transport: Arc::new(ReqwestTransport::new()),
        })
    }

    #[cfg(test)]
    pub fn with_transport(
        kind: TokenForgeKind,
        base_url: impl Into<String>,
        authorization: impl Into<String>,
        transport: Arc<dyn HttpTransport>,
    ) -> Self {
        Self {
            kind,
            base_url: base_url.into(),
            authorization: authorization.into(),
            transport,
        }
    }

    pub fn kind(&self) -> TokenForgeKind {
        self.kind
    }

    pub fn matches_remote(&self, remote: &str) -> bool {
        let Some(remote_host) = remote_host(remote) else {
            return false;
        };
        match self.kind {
            TokenForgeKind::AzureDevOps => {
                remote_host == "dev.azure.com"
                    || remote_host.ends_with(".visualstudio.com")
                    || base_host(&self.base_url).as_deref() == Some(remote_host.as_str())
            }
            TokenForgeKind::Bitbucket => {
                remote_host == "bitbucket.org"
                    || base_host(&self.base_url).as_deref() == Some(remote_host.as_str())
            }
            TokenForgeKind::Gitea => {
                base_host(&self.base_url).as_deref() == Some(remote_host.as_str())
            }
        }
    }

    async fn request(
        &self,
        method: HttpMethod,
        url: String,
        body: Option<Value>,
    ) -> Result<HttpResponse, ForgeUnavailable> {
        let request = HttpRequest {
            method,
            url,
            headers: vec![
                ("Authorization".into(), self.authorization.clone()),
                ("Accept".into(), "application/json".into()),
                ("Content-Type".into(), "application/json".into()),
            ],
            body: body.map(|value| value.to_string()),
            timeout: Duration::from_secs(15),
        };
        self.transport
            .execute(request)
            .await
            .map_err(|error| match error {
                TransportError::Timeout | TransportError::Connect(_) => ForgeUnavailable::Network,
            })
    }

    async fn lookup_url(&self, repo: &RepoCoords, number: u64) -> ReviewLookup {
        let url = match self.kind {
            TokenForgeKind::Gitea => format!(
                "{}/repos/{}/{}/pulls/{number}",
                self.base_url,
                encode(&repo.owner),
                encode(&repo.repo)
            ),
            TokenForgeKind::Bitbucket => format!(
                "{}/repositories/{}/{}/pullrequests/{number}",
                self.base_url,
                encode(&repo.owner),
                encode(&repo.repo)
            ),
            TokenForgeKind::AzureDevOps => format!(
                "{}/_apis/git/repositories/{}/pullRequests/{number}?api-version=7.1",
                self.base_url,
                encode(&repo.repo)
            ),
        };
        match self.request(HttpMethod::Get, url, None).await {
            Ok(response) if response.status == 404 => ReviewLookup::None,
            Ok(response) if (200..300).contains(&response.status) => {
                parse_review(self.kind, &response.body)
                    .map(ReviewLookup::Found)
                    .unwrap_or_else(|| {
                        ReviewLookup::Unavailable(ForgeUnavailable::Other(
                            "The provider returned an invalid pull request".into(),
                        ))
                    })
            }
            Ok(response) => ReviewLookup::Unavailable(classify_status(response.status)),
            Err(error) => ReviewLookup::Unavailable(error),
        }
    }

    async fn lookup_value(
        &self,
        repo: &RepoCoords,
        number: u64,
    ) -> Result<Value, ForgeUnavailable> {
        let url = match self.kind {
            TokenForgeKind::Gitea => format!(
                "{}/repos/{}/{}/pulls/{number}",
                self.base_url,
                encode(&repo.owner),
                encode(&repo.repo)
            ),
            TokenForgeKind::Bitbucket => format!(
                "{}/repositories/{}/{}/pullrequests/{number}",
                self.base_url,
                encode(&repo.owner),
                encode(&repo.repo)
            ),
            TokenForgeKind::AzureDevOps => format!(
                "{}/_apis/git/repositories/{}/pullRequests/{number}?api-version=7.1",
                self.base_url,
                encode(&repo.repo)
            ),
        };
        let response = self.request(HttpMethod::Get, url, None).await?;
        if !(200..300).contains(&response.status) {
            return Err(classify_status(response.status));
        }
        serde_json::from_str(&response.body).map_err(|_| {
            ForgeUnavailable::Other("The provider returned an invalid pull request".into())
        })
    }

    async fn azure_identity(&self) -> Result<String, ForgeError> {
        let response = self
            .request(
                HttpMethod::Get,
                format!(
                    "{}/_apis/connectionData?api-version=7.1-preview.1",
                    self.base_url
                ),
                None,
            )
            .await
            .map_err(ForgeError::Unavailable)?;
        if !(200..300).contains(&response.status) {
            return Err(ForgeError::Unavailable(classify_status(response.status)));
        }
        serde_json::from_str::<Value>(&response.body)
            .ok()
            .and_then(|value| {
                value
                    .pointer("/authenticatedUser/id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .filter(|id| !id.is_empty())
            .ok_or_else(|| {
                ForgeError::Parse("Azure DevOps did not return the current user identity".into())
            })
    }
}

pub async fn token_creation_eligibility(
    provider: &TokenForge,
    git_runner: &GitRunner,
    worktree: &Path,
    branch: &str,
) -> CreationEligibility {
    use CreationBlockedReason as Blocked;
    if !provider.supports_review_creation() {
        return CreationEligibility::Blocked(Blocked::Unavailable(ForgeUnavailable::Other(
            "This provider is read-only in Orca".into(),
        )));
    }
    let repo = match provider.resolve_repository(worktree).await {
        Ok(Some(repo)) => repo,
        Ok(None) => return CreationEligibility::Blocked(Blocked::NotGitHubRepo),
        Err(ForgeError::Unavailable(error)) => {
            return CreationEligibility::Blocked(Blocked::Unavailable(error))
        }
        Err(_) => {
            return CreationEligibility::Blocked(Blocked::Unavailable(ForgeUnavailable::Other(
                "The provider is unavailable".into(),
            )))
        }
    };
    if !has_upstream(git_runner, worktree).await {
        return CreationEligibility::Blocked(Blocked::NoUpstream);
    }
    match provider.review_for_branch(&repo, branch).await {
        ReviewLookup::None => CreationEligibility::Eligible,
        ReviewLookup::Found(_) => CreationEligibility::Blocked(Blocked::AlreadyExists),
        ReviewLookup::Unavailable(error) => {
            CreationEligibility::Blocked(Blocked::Unavailable(error))
        }
    }
}

#[async_trait]
impl ForgeProvider for TokenForge {
    async fn resolve_repository(&self, worktree: &Path) -> Result<Option<RepoCoords>, ForgeError> {
        let output = GitRunner::new()
            .run(worktree, &["remote", "get-url", "origin"])
            .await
            .map_err(|_| ForgeError::Parse("Could not resolve the origin remote".into()))?;
        Ok(parse_remote(self.kind, output.stdout.trim()))
    }

    async fn review_for_branch(&self, repo: &RepoCoords, branch: &str) -> ReviewLookup {
        let url = match self.kind {
            TokenForgeKind::Gitea => format!(
                "{}/repos/{}/{}/pulls?state=all&limit=50",
                self.base_url,
                encode(&repo.owner),
                encode(&repo.repo)
            ),
            TokenForgeKind::Bitbucket => format!(
                "{}/repositories/{}/{}/pullrequests?state=OPEN&state=MERGED&state=DECLINED",
                self.base_url,
                encode(&repo.owner),
                encode(&repo.repo)
            ),
            TokenForgeKind::AzureDevOps => format!(
                "{}/_apis/git/repositories/{}/pullRequests?searchCriteria.status=all&searchCriteria.sourceRefName={}&api-version=7.1",
                self.base_url,
                encode(&repo.repo),
                encode(&format!("refs/heads/{branch}"))
            ),
        };
        match self.request(HttpMethod::Get, url, None).await {
            Ok(response) if (200..300).contains(&response.status) => {
                let values = list_values(self.kind, &response.body);
                values
                    .into_iter()
                    .find(|value| review_branch(self.kind, value).as_deref() == Some(branch))
                    .and_then(|value| parse_review_value(self.kind, &value))
                    .map(ReviewLookup::Found)
                    .unwrap_or(ReviewLookup::None)
            }
            Ok(response) => ReviewLookup::Unavailable(classify_status(response.status)),
            Err(error) => ReviewLookup::Unavailable(error),
        }
    }

    async fn review_by_number(&self, repo: &RepoCoords, number: u64) -> ReviewLookup {
        self.lookup_url(repo, number).await
    }

    fn supports_review_creation(&self) -> bool {
        self.kind != TokenForgeKind::Bitbucket
    }

    async fn create_review(&self, input: CreateReviewInput) -> Result<Review, ForgeError> {
        if !self.supports_review_creation() {
            return Err(ForgeError::Validation(
                "Bitbucket pull-request creation is not supported by Orca".into(),
            ));
        }
        let repo = self
            .resolve_repository(&input.worktree_path)
            .await?
            .ok_or_else(|| {
                ForgeError::Validation("The origin does not match this provider".into())
            })?;
        let head = match input.head {
            Some(head) => head,
            None => current_branch(&input.worktree_path).await?,
        };
        if head == input.base {
            return Err(ForgeError::Validation(
                "The pull request base and head branches must differ".into(),
            ));
        }
        let (url, body) = match self.kind {
            TokenForgeKind::Gitea => (
                format!(
                    "{}/repos/{}/{}/pulls",
                    self.base_url,
                    encode(&repo.owner),
                    encode(&repo.repo)
                ),
                json!({
                    "base": input.base,
                    "head": head,
                    "title": input.title,
                    "body": input.body,
                    "draft": input.draft,
                }),
            ),
            TokenForgeKind::AzureDevOps => (
                format!(
                    "{}/_apis/git/repositories/{}/pullRequests?api-version=7.1",
                    self.base_url,
                    encode(&repo.repo)
                ),
                json!({
                    "sourceRefName": format!("refs/heads/{head}"),
                    "targetRefName": format!("refs/heads/{}", input.base),
                    "title": input.title,
                    "description": input.body,
                    "isDraft": input.draft,
                }),
            ),
            TokenForgeKind::Bitbucket => unreachable!(),
        };
        let response = self
            .request(HttpMethod::Post, url, Some(body))
            .await
            .map_err(ForgeError::Unavailable)?;
        if !(200..300).contains(&response.status) {
            return Err(ForgeError::Unavailable(classify_status(response.status)));
        }
        parse_review(self.kind, &response.body).ok_or_else(|| {
            ForgeError::Parse("The provider returned an invalid pull request".into())
        })
    }
}

#[async_trait]
impl PrActions for TokenForge {
    async fn merge_pr(
        &self,
        repo: &RepoCoords,
        number: u64,
        method: MergeMethod,
        _options: MergeOptions,
    ) -> Result<MergeOutcome, ForgeError> {
        let (http_method, url, body) = match self.kind {
            TokenForgeKind::Gitea => (
                HttpMethod::Post,
                format!(
                    "{}/repos/{}/{}/pulls/{number}/merge",
                    self.base_url,
                    encode(&repo.owner),
                    encode(&repo.repo)
                ),
                json!({"Do": merge_strategy(self.kind, method)}),
            ),
            TokenForgeKind::Bitbucket => (
                HttpMethod::Post,
                format!(
                    "{}/repositories/{}/{}/pullrequests/{number}/merge",
                    self.base_url,
                    encode(&repo.owner),
                    encode(&repo.repo)
                ),
                json!({"merge_strategy": merge_strategy(self.kind, method)}),
            ),
            TokenForgeKind::AzureDevOps => (
                HttpMethod::Patch,
                format!(
                    "{}/_apis/git/repositories/{}/pullRequests/{number}?api-version=7.1",
                    self.base_url,
                    encode(&repo.repo)
                ),
                json!({
                    "status": "completed",
                    "completionOptions": {
                        "mergeStrategy": merge_strategy(self.kind, method),
                        "deleteSourceBranch": false,
                    }
                }),
            ),
        };
        let response = self
            .request(http_method, url, Some(body))
            .await
            .map_err(ForgeError::Unavailable)?;
        match response.status {
            200..=299 => Ok(MergeOutcome::Merged),
            409 => Ok(MergeOutcome::Rejected(MergeRejection::Conflict)),
            400 | 405 | 422 => Ok(MergeOutcome::Rejected(MergeRejection::NotMergeable)),
            status => Err(ForgeError::Unavailable(classify_status(status))),
        }
    }

    async fn set_auto_merge(
        &self,
        repo: &RepoCoords,
        number: u64,
        method: MergeMethod,
    ) -> Result<(), ForgeError> {
        let (http_method, url, body) = match self.kind {
            TokenForgeKind::Gitea => (
                HttpMethod::Post,
                format!(
                    "{}/repos/{}/{}/pulls/{number}/merge",
                    self.base_url,
                    encode(&repo.owner),
                    encode(&repo.repo)
                ),
                json!({
                    "Do": merge_strategy(self.kind, method),
                    "merge_when_checks_succeed": true,
                }),
            ),
            TokenForgeKind::AzureDevOps => {
                let identity = self.azure_identity().await?;
                (
                    HttpMethod::Patch,
                    format!(
                        "{}/_apis/git/repositories/{}/pullRequests/{number}?api-version=7.1",
                        self.base_url,
                        encode(&repo.repo)
                    ),
                    json!({
                        "autoCompleteSetBy": {"id": identity},
                        "completionOptions": {
                            "mergeStrategy": merge_strategy(self.kind, method),
                            "deleteSourceBranch": false,
                        }
                    }),
                )
            }
            TokenForgeKind::Bitbucket => {
                return Err(ForgeError::Validation(
                    "Bitbucket auto-merge is not supported by Orca".into(),
                ))
            }
        };
        let response = self
            .request(http_method, url, Some(body))
            .await
            .map_err(ForgeError::Unavailable)?;
        if (200..300).contains(&response.status) {
            Ok(())
        } else {
            Err(ForgeError::Unavailable(classify_status(response.status)))
        }
    }

    async fn pr_reviews(&self, repo: &RepoCoords, number: u64) -> ReviewThreadLookup {
        let url = match self.kind {
            TokenForgeKind::Gitea => format!(
                "{}/repos/{}/{}/pulls/{number}/reviews",
                self.base_url,
                encode(&repo.owner),
                encode(&repo.repo)
            ),
            TokenForgeKind::Bitbucket | TokenForgeKind::AzureDevOps => {
                return match self.lookup_value(repo, number).await {
                    Ok(value) => ReviewThreadLookup::Found(reviewers_from_value(self.kind, &value)),
                    Err(error) => ReviewThreadLookup::Unavailable(error),
                }
            }
        };
        match self.request(HttpMethod::Get, url, None).await {
            Ok(response) if (200..300).contains(&response.status) => {
                let values = serde_json::from_str::<Vec<Value>>(&response.body).unwrap_or_default();
                ReviewThreadLookup::Found(
                    values.iter().filter_map(gitea_review_from_value).collect(),
                )
            }
            Ok(response) => ReviewThreadLookup::Unavailable(classify_status(response.status)),
            Err(error) => ReviewThreadLookup::Unavailable(error),
        }
    }

    async fn pr_comments(&self, repo: &RepoCoords, number: u64) -> CommentLookup {
        let url = match self.kind {
            TokenForgeKind::Gitea => format!(
                "{}/repos/{}/{}/issues/{number}/comments",
                self.base_url,
                encode(&repo.owner),
                encode(&repo.repo)
            ),
            TokenForgeKind::Bitbucket => format!(
                "{}/repositories/{}/{}/pullrequests/{number}/comments",
                self.base_url,
                encode(&repo.owner),
                encode(&repo.repo)
            ),
            TokenForgeKind::AzureDevOps => format!(
                "{}/_apis/git/repositories/{}/pullRequests/{number}/threads?api-version=7.1",
                self.base_url,
                encode(&repo.repo)
            ),
        };
        match self.request(HttpMethod::Get, url, None).await {
            Ok(response) if (200..300).contains(&response.status) => {
                let values = list_values(self.kind, &response.body);
                let comments = if self.kind == TokenForgeKind::AzureDevOps {
                    values
                        .iter()
                        .flat_map(|thread| {
                            thread
                                .get("comments")
                                .and_then(Value::as_array)
                                .into_iter()
                                .flatten()
                                .filter_map(|comment| comment_from_value(self.kind, comment))
                        })
                        .collect()
                } else {
                    values
                        .iter()
                        .filter_map(|comment| comment_from_value(self.kind, comment))
                        .collect()
                };
                CommentLookup::Found(comments)
            }
            Ok(response) => CommentLookup::Unavailable(classify_status(response.status)),
            Err(error) => CommentLookup::Unavailable(error),
        }
    }

    async fn mergeability_state(&self, repo: &RepoCoords, number: u64) -> MergeabilityState {
        let Ok(value) = self.lookup_value(repo, number).await else {
            return MergeabilityState::Unknown;
        };
        match self.kind {
            TokenForgeKind::Gitea => {
                if value
                    .get("has_merge_conflict")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    MergeabilityState::Conflicting
                } else if value.get("mergeable").and_then(Value::as_bool) == Some(true) {
                    MergeabilityState::Mergeable
                } else {
                    MergeabilityState::Unknown
                }
            }
            TokenForgeKind::AzureDevOps => match value
                .get("mergeStatus")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_ascii_lowercase()
                .as_str()
            {
                "succeeded" => MergeabilityState::Mergeable,
                "conflicts" => MergeabilityState::Conflicting,
                "rejectedbypolicy" | "failure" => MergeabilityState::Blocked,
                _ => MergeabilityState::Unknown,
            },
            TokenForgeKind::Bitbucket => MergeabilityState::Unknown,
        }
    }
}

fn normalize_gitea_base(value: &str) -> String {
    let value = value.trim().trim_end_matches('/');
    if value.to_ascii_lowercase().ends_with("/api/v1") {
        value.to_string()
    } else {
        format!("{value}/api/v1")
    }
}

fn normalize_azure_base(value: &str) -> String {
    value
        .trim()
        .trim_end_matches('/')
        .trim_end_matches("/_apis")
        .to_string()
}

fn base64_basic(value: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(value.as_bytes())
}

fn encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn base_host(value: &str) -> Option<String> {
    url::Url::parse(value)
        .ok()?
        .host_str()
        .map(str::to_ascii_lowercase)
}

fn remote_host(remote: &str) -> Option<String> {
    if let Ok(url) = url::Url::parse(remote) {
        return url.host_str().map(str::to_ascii_lowercase);
    }
    let after_at = remote
        .split_once('@')
        .map(|(_, rest)| rest)
        .unwrap_or(remote);
    after_at
        .split_once(':')
        .map(|(host, _)| host.to_ascii_lowercase())
}

fn remote_path(remote: &str) -> Option<(String, String)> {
    let path = if let Ok(url) = url::Url::parse(remote) {
        url.path().trim_matches('/').to_string()
    } else {
        remote
            .split_once(':')
            .map(|(_, path)| path.trim_matches('/').to_string())?
    };
    let path = path.strip_suffix(".git").unwrap_or(&path);
    let mut parts = path.split('/').filter(|part| !part.is_empty());
    let owner = parts.next()?.to_string();
    let rest = parts.collect::<Vec<_>>();
    if rest.is_empty() {
        return None;
    }
    Some((owner, rest.join("/")))
}

fn parse_remote(kind: TokenForgeKind, remote: &str) -> Option<RepoCoords> {
    let host = remote_host(remote)?;
    let (owner, path) = remote_path(remote)?;
    if kind == TokenForgeKind::AzureDevOps {
        let parts = path.split('/').collect::<Vec<_>>();
        if parts.len() >= 3 && parts[parts.len() - 2].eq_ignore_ascii_case("_git") {
            return Some(RepoCoords {
                owner: format!("{owner}/{}", parts[..parts.len() - 2].join("/")),
                repo: parts.last()?.to_string(),
                host,
            });
        }
    }
    Some(RepoCoords {
        owner,
        repo: path,
        host,
    })
}

fn classify_status(status: u16) -> ForgeUnavailable {
    match status {
        401 | 403 => ForgeUnavailable::NotAuthenticated,
        429 => ForgeUnavailable::RateLimited,
        500..=599 => ForgeUnavailable::Network,
        _ => ForgeUnavailable::Other(format!("Provider request failed ({status})")),
    }
}

fn parse_review(kind: TokenForgeKind, body: &str) -> Option<Review> {
    let value = serde_json::from_str(body).ok()?;
    parse_review_value(kind, &value)
}

fn parse_review_value(kind: TokenForgeKind, value: &Value) -> Option<Review> {
    let (number, state, title, url, draft) = match kind {
        TokenForgeKind::Gitea => (
            value.get("number")?.as_u64()?,
            value.get("state").and_then(Value::as_str).unwrap_or("open"),
            value
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            value
                .get("html_url")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            value.get("draft").and_then(Value::as_bool).unwrap_or(false),
        ),
        TokenForgeKind::Bitbucket => (
            value.get("id")?.as_u64()?,
            value.get("state").and_then(Value::as_str).unwrap_or("OPEN"),
            value
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            value
                .pointer("/links/html/href")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            value.get("draft").and_then(Value::as_bool).unwrap_or(false),
        ),
        TokenForgeKind::AzureDevOps => (
            value.get("pullRequestId")?.as_u64()?,
            value
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("active"),
            value
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            value
                .pointer("/_links/web/href")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            value
                .get("isDraft")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        ),
    };
    let lower = state.to_ascii_lowercase();
    let state = if draft {
        ReviewState::Draft
    } else if matches!(lower.as_str(), "merged" | "completed" | "fulfilled") {
        ReviewState::Merged
    } else if matches!(
        lower.as_str(),
        "closed" | "declined" | "abandoned" | "superseded"
    ) {
        ReviewState::Closed
    } else {
        ReviewState::Open
    };
    Some(Review {
        number,
        state,
        title: title.to_string(),
        url: url.to_string(),
        checks: ChecksSummary::default(),
    })
}

fn list_values(kind: TokenForgeKind, body: &str) -> Vec<Value> {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    match kind {
        TokenForgeKind::Gitea => value.as_array().cloned().unwrap_or_default(),
        TokenForgeKind::AzureDevOps | TokenForgeKind::Bitbucket => value
            .get("value")
            .or_else(|| value.get("values"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    }
}

fn review_branch(kind: TokenForgeKind, value: &Value) -> Option<String> {
    let branch = match kind {
        TokenForgeKind::Gitea => value.pointer("/head/ref")?.as_str()?,
        TokenForgeKind::Bitbucket => value.pointer("/source/branch/name")?.as_str()?,
        TokenForgeKind::AzureDevOps => value.get("sourceRefName")?.as_str()?,
    };
    Some(
        branch
            .strip_prefix("refs/heads/")
            .unwrap_or(branch)
            .to_string(),
    )
}

fn merge_strategy(kind: TokenForgeKind, method: MergeMethod) -> &'static str {
    match (kind, method) {
        (TokenForgeKind::AzureDevOps, MergeMethod::Merge) => "noFastForward",
        (TokenForgeKind::AzureDevOps, MergeMethod::Squash) => "squash",
        (TokenForgeKind::AzureDevOps, MergeMethod::Rebase) => "rebase",
        (TokenForgeKind::Bitbucket, MergeMethod::Merge) => "merge_commit",
        (TokenForgeKind::Bitbucket, MergeMethod::Squash) => "squash",
        (TokenForgeKind::Bitbucket, MergeMethod::Rebase) => "fast_forward",
        (TokenForgeKind::Gitea, MergeMethod::Merge) => "merge",
        (TokenForgeKind::Gitea, MergeMethod::Squash) => "squash",
        (TokenForgeKind::Gitea, MergeMethod::Rebase) => "rebase",
    }
}

fn gitea_review_from_value(value: &Value) -> Option<PrReview> {
    Some(PrReview {
        author: value
            .pointer("/user/login")
            .and_then(Value::as_str)
            .unwrap_or("ghost")
            .to_string(),
        state: PrReviewState::from_api(
            value
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ),
        body: value
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        submitted_at: value
            .get("submitted_at")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

fn reviewers_from_value(kind: TokenForgeKind, value: &Value) -> Vec<PrReview> {
    match kind {
        TokenForgeKind::Bitbucket => value
            .get("participants")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|participant| {
                participant
                    .get("approved")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            })
            .map(|participant| PrReview {
                author: participant
                    .pointer("/user/nickname")
                    .or_else(|| participant.pointer("/user/display_name"))
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string(),
                state: PrReviewState::Approved,
                body: String::new(),
                submitted_at: String::new(),
            })
            .collect(),
        TokenForgeKind::AzureDevOps => value
            .get("reviewers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|reviewer| {
                let vote = reviewer.get("vote").and_then(Value::as_i64).unwrap_or(0);
                let state = match vote {
                    10 | 5 => PrReviewState::Approved,
                    -10 => PrReviewState::ChangesRequested,
                    -5 => PrReviewState::Pending,
                    0 => return None,
                    _ => PrReviewState::Other,
                };
                Some(PrReview {
                    author: reviewer
                        .get("displayName")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_string(),
                    state,
                    body: String::new(),
                    submitted_at: String::new(),
                })
            })
            .collect(),
        TokenForgeKind::Gitea => Vec::new(),
    }
}

fn comment_from_value(kind: TokenForgeKind, value: &Value) -> Option<PrComment> {
    let (author, body, created_at, url) = match kind {
        TokenForgeKind::Gitea => (
            value.pointer("/user/login").and_then(Value::as_str),
            value.get("body").and_then(Value::as_str),
            value.get("created_at").and_then(Value::as_str),
            value.get("html_url").and_then(Value::as_str),
        ),
        TokenForgeKind::Bitbucket => (
            value
                .pointer("/user/nickname")
                .or_else(|| value.pointer("/user/display_name"))
                .and_then(Value::as_str),
            value.pointer("/content/raw").and_then(Value::as_str),
            value.get("created_on").and_then(Value::as_str),
            value.pointer("/links/html/href").and_then(Value::as_str),
        ),
        TokenForgeKind::AzureDevOps => (
            value.pointer("/author/displayName").and_then(Value::as_str),
            value.get("content").and_then(Value::as_str),
            value.get("publishedDate").and_then(Value::as_str),
            None,
        ),
    };
    let body = body.unwrap_or_default();
    if body.is_empty() {
        return None;
    }
    Some(PrComment {
        author: author.unwrap_or("unknown").to_string(),
        body: body.to_string(),
        created_at: created_at.unwrap_or_default().to_string(),
        url: url.unwrap_or_default().to_string(),
    })
}

async fn current_branch(worktree: &Path) -> Result<String, ForgeError> {
    let output = GitRunner::new()
        .run(worktree, &["branch", "--show-current"])
        .await
        .map_err(|_| ForgeError::Validation("Could not resolve the current branch".into()))?;
    let branch = output.stdout.trim();
    if branch.is_empty() {
        Err(ForgeError::Validation(
            "A pull request cannot be created from detached HEAD".into(),
        ))
    } else {
        Ok(branch.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_supported_remote_shapes() {
        let gitea = parse_remote(
            TokenForgeKind::Gitea,
            "ssh://git@git.example.com/team/project.git",
        )
        .unwrap();
        assert_eq!(
            (gitea.owner.as_str(), gitea.repo.as_str()),
            ("team", "project")
        );

        let bitbucket =
            parse_remote(TokenForgeKind::Bitbucket, "git@bitbucket.org:team/repo.git").unwrap();
        assert_eq!(bitbucket.host, "bitbucket.org");

        let azure = parse_remote(
            TokenForgeKind::AzureDevOps,
            "https://dev.azure.com/acme/Project/_git/repo",
        )
        .unwrap();
        assert_eq!(azure.owner, "acme/Project");
        assert_eq!(azure.repo, "repo");
    }

    #[test]
    fn normalizes_provider_api_roots() {
        assert_eq!(
            normalize_gitea_base("https://git.example.com/code"),
            "https://git.example.com/code/api/v1"
        );
        assert_eq!(
            normalize_azure_base("https://dev.azure.com/acme/Project/_apis/"),
            "https://dev.azure.com/acme/Project"
        );
    }

    #[test]
    fn maps_provider_review_states_without_treating_unknown_as_closed() {
        let open = parse_review(
            TokenForgeKind::Bitbucket,
            r#"{"id":7,"state":"OPEN","title":"T","links":{"html":{"href":"https://x/7"}}}"#,
        )
        .unwrap();
        assert_eq!(open.state, ReviewState::Open);
        let merged = parse_review(
            TokenForgeKind::AzureDevOps,
            r#"{"pullRequestId":8,"status":"completed","title":"T","_links":{"web":{"href":"https://x/8"}}}"#,
        )
        .unwrap();
        assert_eq!(merged.state, ReviewState::Merged);
    }
}
