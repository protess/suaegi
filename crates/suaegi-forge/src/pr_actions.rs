//! 7b — PR 상호작용 백엔드: merge / auto-merge (파괴적 쓰기) 와 리뷰·코멘트·머지가능성
//! **읽기**. `ForgeProvider`(7a: resolve/상태조회/생성)와 **분리된 sibling 트레잇**
//! [`PrActions`]로 둔다 — 7a의 트레잇은 그대로 두고, UI(PR 패널, 후속)가 이 표면에만
//! 의존하게 하려는 것이다(브리프, 플랜 §2). gh 커맨드 모양은 Orca `github/client.ts`
//! (`mergePR`, `setPRAutoMerge`, `getPRComments`, `github-pr-merge-state.ts`)를 미러하되,
//! 7a가 체크를 `gh pr checks --json bucket` 하나로 의식적으로 단순화한 것처럼
//! (Orca의 3단 폴백을 안 짊어짐) 여기서도 **리뷰 스레드 GraphQL 팬아웃을 안 짊어지고**
//! `gh pr view --json reviews,comments`만 쓴다. 인라인 스레드(해결 상태·파일/라인)는
//! 의식적 후속이다.
//!
//! **분류 규율은 7a와 동일**(`classify.rs`): 일시(transient) gh 실패는 절대 확정적 부정으로
//! 오독되면 안 된다. merge는 pinned stderr에만 확정적 거부([`MergeOutcome::Rejected`])를
//! 내고, 그 밖의 실패는 [`ForgeError::Unavailable`]이다. 리뷰·코멘트의 일시 실패는
//! `Unavailable`이지 "리뷰 없음/코멘트 없음"이 아니다(캐시-오염 방지). 머지가능성은 일시
//! 실패 시 [`MergeabilityState::Unknown`]이지 절대 `Mergeable`이 아니다.

use crate::classify::classify_unavailable;
use crate::provider::{ForgeError, ForgeUnavailable, RepoCoords};
use async_trait::async_trait;
use serde::Deserialize;

/// gh `pr merge` 방식 → 플래그. Orca `mergePR`(`client.ts:4552`)의 `--${method}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeMethod {
    Merge,
    Squash,
    Rebase,
}

impl MergeMethod {
    /// `gh pr merge`에 넘길 플래그. **이 매핑이 load-bearing이다** — 잘못 매핑하면 사용자가
    /// 고른 것과 다른 방식으로 히스토리를 쓴다(§5 mutation이 이걸 겨냥).
    pub fn gh_flag(self) -> &'static str {
        match self {
            MergeMethod::Merge => "--merge",
            MergeMethod::Squash => "--squash",
            MergeMethod::Rebase => "--rebase",
        }
    }
}

/// merge 옵션. `delete_branch`는 `--delete-branch`를 붙인다. 기본 false —
/// Orca는 worktree가 체크아웃한 로컬 브랜치를 지우다 실패하는 걸 피하려 아예 안 붙인다
/// (`client.ts:4584`). 우리는 호출부(UI)가 결정하게 노출하되 기본은 보수적으로 끈다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MergeOptions {
    pub delete_branch: bool,
}

/// merge 호출 결과. **`Rejected`(확정적 거부)는 데이터, 일시 실패는 에러**(`ForgeError`) —
/// `ReviewLookup`의 None(데이터) vs Unavailable(에러) 규율을 그대로 옮긴 것이다. 일시
/// gh 실패는 절대 `Rejected`로 오지 않는다(pinned stderr에만 `Rejected`, 그 밖은 `Unavailable`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    /// gh exit 0 — PR이 머지됐다.
    Merged,
    /// 확정적 거부 — pinned stderr로만 온다. 구조화된 사유(raw stderr 아님).
    Rejected(MergeRejection),
}

/// merge가 **확정적으로** 거부된 사유. raw stderr를 UI에 흘리지 않으려는 구조화 라벨이다.
/// 여기 안 잡히는(=인식 못 한) 실패는 `Rejected`가 아니라 `Unavailable`로 간다 — 일시
/// 실패를 지어낸 확정적 거부로 오독하지 않으려는 것(§5 규율).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeRejection {
    /// PR이 머지 가능한 상태가 아님(일반).
    NotMergeable,
    /// 머지 충돌.
    Conflict,
    /// 브랜치 보호/필수 체크/리뷰 등으로 차단됨.
    Blocked,
    /// 리뷰에서 변경 요청됨.
    ChangesRequested,
    /// 머지 권한 없음.
    PermissionDenied,
    /// 이미 머지/닫힌 PR.
    AlreadyClosed,
}

/// merge 실패 분류의 내부 결과. `Rejected`(확정) vs `Transient`(일시)로 가른다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeFailure {
    /// pinned stderr에 근거한 확정적 거부.
    Rejected(MergeRejection),
    /// 인식 못 했거나 일시적 — 분류된 `Unavailable`로.
    Transient(ForgeUnavailable),
}

/// `gh pr merge` 실패 stderr를 확정적 거부 vs 일시 실패로 가른다. **좁게** 간다:
/// pinned 구절에만 `Rejected`를 내고, 나머지는 전부 `Transient`(→ `Unavailable`)다.
/// 넓히면 일시 오류(네트워크·레이트리밋)를 "머지 거부됨"으로 오독해, 재시도하면 될
/// 상황을 확정적 실패로 못박는다 — 정확히 §5 mutation이 겨냥하는 붕괴다.
///
/// LC_ALL=C(`runner.rs`)로 gh stderr가 영어로 고정되므로 이 pinned substring이 안정적이다.
pub fn classify_merge_failure(stderr: &str) -> MergeFailure {
    let lower = stderr.to_lowercase();

    // 이미 닫힘/머지됨 — 재시도 무의미한 확정 상태.
    if lower.contains("already merged")
        || lower.contains("pull request is already merged")
        || lower.contains("closed pull request")
        || lower.contains("pull request is closed")
    {
        return MergeFailure::Rejected(MergeRejection::AlreadyClosed);
    }

    // 충돌 — "not mergeable"보다 먼저(더 구체적).
    if lower.contains("merge conflict")
        || lower.contains("has conflicts")
        || lower.contains("conflicts with the base branch")
    {
        return MergeFailure::Rejected(MergeRejection::Conflict);
    }

    // 변경 요청 — 리뷰 결정.
    if lower.contains("changes requested") || lower.contains("changes were requested") {
        return MergeFailure::Rejected(MergeRejection::ChangesRequested);
    }

    // 브랜치 보호/필수 체크/리뷰 승인 필요 등으로 차단.
    // **`at least`는 일반 영어 bigram이라 단독 매칭 금지** — transient stderr가 우연히
    // "wait at least a minute"처럼 포함하면 확정 거부로 날조된다(§89-92 narrow 규율 위반).
    // 리뷰-승인 맥락(`review`/`approv`)과 공존할 때만 차단으로 본다.
    let at_least_reviews =
        lower.contains("at least") && (lower.contains("review") || lower.contains("approv"));
    if lower.contains("required status check")
        || lower.contains("required status checks")
        || lower.contains("branch protection")
        || at_least_reviews
        || lower.contains("approving review")
        || lower.contains("required by reviewers")
        || lower.contains("review required")
        || lower.contains("reviews required")
        || lower.contains("is blocked")
        || lower.contains("protected branch")
    {
        return MergeFailure::Rejected(MergeRejection::Blocked);
    }

    // 권한 — 머지 특정 구절만(bare 403은 아래 classify_unavailable로 Unavailable).
    if lower.contains("not authorized to merge")
        || lower.contains("must have write access")
        || lower.contains("must have admin")
        || lower.contains("you do not have permission to merge")
    {
        return MergeFailure::Rejected(MergeRejection::PermissionDenied);
    }

    // 머지 불가(일반) — 위 구체 사유가 안 잡힌 pinned "not mergeable".
    if lower.contains("not mergeable")
        || lower.contains("is not mergeable")
        || lower.contains("cannot be cleanly created")
    {
        return MergeFailure::Rejected(MergeRejection::NotMergeable);
    }

    // 그 밖 전부 — 일시로 간주(분류된 Unavailable). 인식 못 한 실패를 지어낸 확정
    // 거부로 못박지 않는다.
    MergeFailure::Transient(classify_unavailable(stderr))
}

/// 머지가능성 4-상태(브리프). `github-pr-merge-state.ts`의 우선순위를 미러하되 UI 표현이
/// 아닌 백엔드 상태로 압축한다. **`Unknown`이 안전한 흡수 상태다** — 일시 실패·불완전
/// 메타데이터는 여기로 오지 절대 `Mergeable`로 오지 않는다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeabilityState {
    /// GitHub이 머지 가능하다고 보고.
    Mergeable,
    /// 리뷰 승인 필요·변경 요청·브랜치 보호·behind 등으로 차단.
    Blocked,
    /// 머지 충돌.
    Conflicting,
    /// 알 수 없음(GitHub이 계산 중이거나 메타데이터 누락, 또는 조회 실패). **기본 안전값.**
    Unknown,
}

/// `gh pr view --json mergeable,mergeStateStatus,reviewDecision` 파싱 형태. 모든 필드
/// 선택적 — gh가 계산 전이면 UNKNOWN/빈 문자열을 낼 수 있다.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct MergeabilityFields {
    #[serde(default)]
    pub mergeable: String,
    #[serde(rename = "mergeStateStatus", default)]
    pub merge_state_status: String,
    #[serde(rename = "reviewDecision", default)]
    pub review_decision: String,
}

/// gh 필드를 4-상태로. **우선순위가 load-bearing이다**(Orca `presentGitHubPRMergeState`의
/// 순서 미러): reviewDecision(승인 필요/변경 요청) → 충돌 → behind/blocked → mergeable →
/// 그 밖 Unknown. 어느 것도 안 맞으면 `Mergeable`이 아니라 `Unknown`으로 떨어진다.
pub fn mergeability_from_fields(f: &MergeabilityFields) -> MergeabilityState {
    let mergeable = f.mergeable.to_ascii_uppercase();
    let status = f.merge_state_status.to_ascii_uppercase();
    let decision = f.review_decision.to_ascii_uppercase();

    // 1. 리뷰 결정 — 승인 필요/변경 요청은 차단.
    if decision == "REVIEW_REQUIRED" || decision == "CHANGES_REQUESTED" {
        return MergeabilityState::Blocked;
    }
    // 2. 충돌.
    if mergeable == "CONFLICTING" || status == "DIRTY" {
        return MergeabilityState::Conflicting;
    }
    // 3. behind/blocked.
    if status == "BEHIND" || status == "BLOCKED" {
        return MergeabilityState::Blocked;
    }
    // 4. 머지 가능.
    if mergeable == "MERGEABLE" || status == "CLEAN" {
        return MergeabilityState::Mergeable;
    }
    // 5. 그 밖(UNKNOWN·빈 값·누락) — 안전한 Unknown. 절대 Mergeable로 넘기지 않는다.
    MergeabilityState::Unknown
}

/// gh JSON의 author 형태(`{ "login": ... }`). null이면 ghost.
#[derive(Debug, Clone, Deserialize)]
pub struct GhActor {
    #[serde(default)]
    pub login: String,
}

/// `gh pr view --json reviews` 원소. 리뷰 **요약**(승인/변경요청/코멘트)이다 — 인라인
/// 스레드가 아니다(의식적 단순화).
#[derive(Debug, Clone, Deserialize)]
pub struct GhReviewRaw {
    #[serde(default)]
    pub author: Option<GhActor>,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub state: String,
    #[serde(rename = "submittedAt", default)]
    pub submitted_at: String,
}

/// `gh pr view --json comments` 원소(이슈-레벨 대화 코멘트).
#[derive(Debug, Clone, Deserialize)]
pub struct GhCommentRaw {
    #[serde(default)]
    pub author: Option<GhActor>,
    #[serde(default)]
    pub body: String,
    #[serde(rename = "createdAt", default)]
    pub created_at: String,
    #[serde(default)]
    pub url: String,
}

/// PR 리뷰 요약(정제된 도메인 타입).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrReview {
    pub author: String,
    pub state: PrReviewState,
    pub body: String,
    pub submitted_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrReviewState {
    Approved,
    ChangesRequested,
    Commented,
    Dismissed,
    Pending,
    /// 알 수 없는 gh 상태(보수적 — 승인으로 오독하지 않는다).
    Other,
}

impl PrReviewState {
    /// GitHub 리뷰 상태 문자열(gh JSON·REST 공통 대문자 토큰)을 매핑한다. gh CLI와 HTTP REST
    /// 백엔드가 **같은 토큰**(APPROVED/CHANGES_REQUESTED/...)을 쓰므로 두 경로가 공유한다.
    pub fn from_api(state: &str) -> Self {
        match state.to_ascii_uppercase().as_str() {
            "APPROVED" => PrReviewState::Approved,
            "CHANGES_REQUESTED" => PrReviewState::ChangesRequested,
            "COMMENTED" => PrReviewState::Commented,
            "DISMISSED" => PrReviewState::Dismissed,
            "PENDING" => PrReviewState::Pending,
            _ => PrReviewState::Other,
        }
    }
}

impl From<GhReviewRaw> for PrReview {
    fn from(r: GhReviewRaw) -> Self {
        PrReview {
            author: actor_login(r.author),
            state: PrReviewState::from_api(&r.state),
            body: r.body,
            submitted_at: r.submitted_at,
        }
    }
}

/// PR 코멘트(정제된 도메인 타입).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrComment {
    pub author: String,
    pub body: String,
    pub created_at: String,
    pub url: String,
}

impl From<GhCommentRaw> for PrComment {
    fn from(c: GhCommentRaw) -> Self {
        PrComment {
            author: actor_login(c.author),
            body: c.body,
            created_at: c.created_at,
            url: c.url,
        }
    }
}

/// null author → "ghost"(Orca와 동일).
fn actor_login(a: Option<GhActor>) -> String {
    match a {
        Some(a) if !a.login.is_empty() => a.login,
        _ => "ghost".to_string(),
    }
}

/// 리뷰 조회 결과. **`Found`(성공, 빈 벡터 = 진짜 리뷰 없음)와 `Unavailable`(조회 실패)를
/// 구별**한다 — 일시 실패는 절대 "리뷰 없음"(빈 Found)으로 오지 않는다(캐시-오염 방지).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewThreadLookup {
    Found(Vec<PrReview>),
    Unavailable(ForgeUnavailable),
}

/// 코멘트 조회 결과. 리뷰와 같은 규율 — 일시 실패는 빈 `Found`가 아니라 `Unavailable`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommentLookup {
    Found(Vec<PrComment>),
    Unavailable(ForgeUnavailable),
}

/// 7b PR 상호작용 표면. `ForgeProvider`와 별개의 sibling 트레잇이다. HTTP(7a-2)·GitLab(7c)
/// impl이 뒤따를 때도 이 한 트레잇 뒤에 들어온다.
#[async_trait]
pub trait PrActions {
    /// **파괴적**: PR을 머지한다(`gh pr merge <n> --<method> [--delete-branch] --repo ...`).
    /// UI가 **먼저 확인**한 뒤 호출해야 한다 — 이 백엔드는 auto-confirm을 절대 하지 않는다.
    /// 확정적 거부는 `Ok(Rejected)`, 일시 실패는 `Err(Unavailable)`로 구별한다.
    async fn merge_pr(
        &self,
        repo: &RepoCoords,
        number: u64,
        method: MergeMethod,
        options: MergeOptions,
    ) -> Result<MergeOutcome, ForgeError>;

    /// auto-merge를 켠다(`gh pr merge <n> --auto --<method> --repo ...`). 요건 충족 시
    /// GitHub이 자동 머지하도록 예약만 한다 — 지금 머지하지 않는다.
    async fn set_auto_merge(
        &self,
        repo: &RepoCoords,
        number: u64,
        method: MergeMethod,
    ) -> Result<(), ForgeError>;

    /// PR 리뷰 요약 읽기(`gh pr view <n> --json reviews`).
    async fn pr_reviews(&self, repo: &RepoCoords, number: u64) -> ReviewThreadLookup;

    /// PR 대화 코멘트 읽기(`gh pr view <n> --json comments`).
    async fn pr_comments(&self, repo: &RepoCoords, number: u64) -> CommentLookup;

    /// 머지가능성 상태 읽기(`gh pr view <n> --json mergeable,mergeStateStatus,reviewDecision`).
    /// 일시 실패·불완전 메타데이터는 `Unknown`이지 절대 `Mergeable`이 아니다.
    async fn mergeability_state(&self, repo: &RepoCoords, number: u64) -> MergeabilityState;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_method_maps_to_the_right_flag() {
        // **회귀 방어 (c)**: 방식→플래그 매핑이 어긋나면 사용자가 고른 것과 다르게 머지된다.
        assert_eq!(MergeMethod::Merge.gh_flag(), "--merge");
        assert_eq!(MergeMethod::Squash.gh_flag(), "--squash");
        assert_eq!(MergeMethod::Rebase.gh_flag(), "--rebase");
    }

    #[test]
    fn merge_conflict_is_a_definitive_rejection() {
        assert_eq!(
            classify_merge_failure("Pull request is not mergeable: has merge conflicts"),
            MergeFailure::Rejected(MergeRejection::Conflict)
        );
    }

    #[test]
    fn merge_blocked_and_changes_requested_and_permission_and_closed() {
        assert_eq!(
            classify_merge_failure("At least 1 approving review is required by reviewers"),
            MergeFailure::Rejected(MergeRejection::Blocked)
        );
        assert_eq!(
            classify_merge_failure("changes requested on this pull request"),
            MergeFailure::Rejected(MergeRejection::ChangesRequested)
        );
        assert_eq!(
            classify_merge_failure("You're not authorized to merge this pull request"),
            MergeFailure::Rejected(MergeRejection::PermissionDenied)
        );
        assert_eq!(
            classify_merge_failure("Pull request is already merged"),
            MergeFailure::Rejected(MergeRejection::AlreadyClosed)
        );
        assert_eq!(
            classify_merge_failure("GraphQL: X is not mergeable (mergePullRequest)"),
            MergeFailure::Rejected(MergeRejection::NotMergeable)
        );
    }

    /// **핵심 회귀 방어**: 일시 gh 실패(레이트리밋·네트워크·auth)는 "머지 거부됨"이
    /// 아니라 `Transient`(→ Unavailable)여야 한다. `classify_merge_failure`를 넓혀 이걸
    /// 확정적 Rejected로 접으면 이 단언이 깨진다.
    #[test]
    fn transient_merge_failure_is_not_a_rejection() {
        assert_eq!(
            classify_merge_failure("HTTP 429: API rate limit exceeded"),
            MergeFailure::Transient(ForgeUnavailable::RateLimited)
        );
        assert_eq!(
            classify_merge_failure("error connecting: could not resolve host api.github.com"),
            MergeFailure::Transient(ForgeUnavailable::Network)
        );
        assert_eq!(
            classify_merge_failure("HTTP 401: Bad credentials"),
            MergeFailure::Transient(ForgeUnavailable::NotAuthenticated)
        );
        // 완전히 낯선 실패도 확정 거부가 아니라 일시로.
        assert!(matches!(
            classify_merge_failure("some unexpected gh explosion"),
            MergeFailure::Transient(_)
        ));
    }

    /// **회귀 방어 — `at least` pin이 transient를 날조하면 안 된다.** "at least"는 일반
    /// bigram이라, 리뷰-승인 맥락 없이 transient stderr에 우연히 들어도 확정 `Rejected`가
    /// 아니라 `Transient`(→ 분류된 Unavailable)로 유지돼야 한다. pin을 다시 넓은 단독
    /// `"at least"`로 되돌리면 첫 단언이 깨진다. 진짜 리뷰-승인 차단은 여전히 `Rejected(Blocked)`.
    #[test]
    fn at_least_pin_does_not_fabricate_rejection_from_transient() {
        // transient(레이트리밋)이 "at least"를 포함해도 확정 거부로 날조되지 않는다.
        assert_eq!(
            classify_merge_failure("HTTP 429: rate limit exceeded; please wait at least a minute"),
            MergeFailure::Transient(ForgeUnavailable::RateLimited)
        );
        // 리뷰 맥락 없는 다른 "at least"도 Transient(낯섦 → 분류된 Unavailable).
        assert!(matches!(
            classify_merge_failure("upload failed: file must be at least 1 byte"),
            MergeFailure::Transient(_)
        ));
        // 하지만 진짜 리뷰-승인 차단은 여전히 Blocked(양방향).
        assert_eq!(
            classify_merge_failure("At least 1 approving review is required by reviewers"),
            MergeFailure::Rejected(MergeRejection::Blocked)
        );
    }

    #[test]
    fn mergeability_conflict_and_blocked_and_mergeable() {
        assert_eq!(
            mergeability_from_fields(&MergeabilityFields {
                mergeable: "CONFLICTING".into(),
                merge_state_status: "DIRTY".into(),
                review_decision: "".into(),
            }),
            MergeabilityState::Conflicting
        );
        assert_eq!(
            mergeability_from_fields(&MergeabilityFields {
                mergeable: "MERGEABLE".into(),
                merge_state_status: "BLOCKED".into(),
                review_decision: "".into(),
            }),
            MergeabilityState::Blocked
        );
        assert_eq!(
            mergeability_from_fields(&MergeabilityFields {
                mergeable: "MERGEABLE".into(),
                merge_state_status: "CLEAN".into(),
                review_decision: "APPROVED".into(),
            }),
            MergeabilityState::Mergeable
        );
    }

    /// review_decision이 승인 필요/변경 요청이면 mergeable=MERGEABLE이어도 Blocked.
    #[test]
    fn review_required_blocks_even_if_mergeable() {
        assert_eq!(
            mergeability_from_fields(&MergeabilityFields {
                mergeable: "MERGEABLE".into(),
                merge_state_status: "CLEAN".into(),
                review_decision: "REVIEW_REQUIRED".into(),
            }),
            MergeabilityState::Blocked
        );
        assert_eq!(
            mergeability_from_fields(&MergeabilityFields {
                mergeable: "MERGEABLE".into(),
                merge_state_status: "CLEAN".into(),
                review_decision: "CHANGES_REQUESTED".into(),
            }),
            MergeabilityState::Blocked
        );
    }

    /// **핵심 회귀 방어**: 불완전/알 수 없는 메타데이터는 `Unknown`이지 절대 `Mergeable`이
    /// 아니다. 우선순위/기본값을 mutate해 이걸 Mergeable로 접으면 이 단언이 깨진다.
    #[test]
    fn unknown_metadata_is_unknown_never_mergeable() {
        assert_eq!(
            mergeability_from_fields(&MergeabilityFields::default()),
            MergeabilityState::Unknown
        );
        assert_eq!(
            mergeability_from_fields(&MergeabilityFields {
                mergeable: "UNKNOWN".into(),
                merge_state_status: "UNKNOWN".into(),
                review_decision: "".into(),
            }),
            MergeabilityState::Unknown
        );
    }

    #[test]
    fn review_state_mapping_is_conservative() {
        assert_eq!(PrReviewState::from_api("APPROVED"), PrReviewState::Approved);
        assert_eq!(
            PrReviewState::from_api("CHANGES_REQUESTED"),
            PrReviewState::ChangesRequested
        );
        assert_eq!(PrReviewState::from_api("COMMENTED"), PrReviewState::Commented);
        // 알 수 없는 상태는 Approved로 오독하지 않고 Other.
        assert_eq!(PrReviewState::from_api("WEIRD_NEW_STATE"), PrReviewState::Other);
    }

    #[test]
    fn null_author_becomes_ghost() {
        let c: PrComment = GhCommentRaw {
            author: None,
            body: "hi".into(),
            created_at: "t".into(),
            url: "u".into(),
        }
        .into();
        assert_eq!(c.author, "ghost");
    }
}
