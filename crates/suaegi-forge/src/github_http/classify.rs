//! HTTP 상태·헤더 → 분류된 `ForgeUnavailable` / merge 실패. gh 백엔드의 `classify.rs`가
//! **문자열 stderr**를 분류하는 것과 달리 여기선 **구조화된 상태코드+헤더**를 본다 — 더
//! 신뢰도 높은 신호다. 하지만 규율은 동일하다(Orca `pr-refresh-error-classification.ts`):
//! rate-limit을 permission보다 먼저, 404(repo)를 network와 구별, 일시(5xx/429/network)는
//! **절대 확정적 부정으로 오독하지 않는다**.

use crate::pr_actions::{MergeFailure, MergeRejection};
use crate::provider::ForgeUnavailable;

/// 응답이 rate-limit인지. GitHub은 primary/secondary 리밋 모두에 **403 또는 429**를 낸다:
/// - 429 → 항상 rate-limit.
/// - 403 + `x-ratelimit-remaining: 0` → primary rate-limit(403으로 옴).
/// - 403/429 + `retry-after` → secondary/abuse rate-limit.
///
/// **이 판정이 permission(bare 403)보다 먼저 와야 한다** — 안 그러면 재시도하면 될 리밋을
/// 확정적 permission 실패로 오독한다.
pub fn is_rate_limited(status: u16, ratelimit_remaining: Option<&str>, retry_after: Option<&str>) -> bool {
    if status == 429 {
        return true;
    }
    if status == 403 {
        let remaining_zero = ratelimit_remaining.map(|v| v.trim() == "0").unwrap_or(false);
        let has_retry_after = retry_after.map(|v| !v.trim().is_empty()).unwrap_or(false);
        return remaining_zero || has_retry_after;
    }
    false
}

/// 비-성공 HTTP 상태를 **분류된** `ForgeUnavailable`로. **순서가 load-bearing이다**(gh
/// `classify_unavailable` 미러): rate-limit → auth(401) → permission(403) → repo(404) →
/// network(5xx). raw 바디를 담지 않는다 — 정제된 라벨만.
pub fn classify_http_unavailable(
    status: u16,
    ratelimit_remaining: Option<&str>,
    retry_after: Option<&str>,
) -> ForgeUnavailable {
    // 1. rate-limit 먼저(403/429 둘 다 가능).
    if is_rate_limited(status, ratelimit_remaining, retry_after) {
        return ForgeUnavailable::RateLimited;
    }
    match status {
        // 2. 인증 실패.
        401 => ForgeUnavailable::NotAuthenticated,
        // 3. permission(rate-limit 아닌 403). 정제 라벨만.
        403 => ForgeUnavailable::Other("permission denied".to_string()),
        // 4. repo 해석 실패(404). **None이 아니다** — repo 자체가 없음. 정제 라벨만.
        404 => ForgeUnavailable::Other("repository unavailable".to_string()),
        // 5. 서버측 일시 실패(5xx). 재시도 가능 → Network 축.
        s if s >= 500 => ForgeUnavailable::Network,
        // 그 밖(예상 밖 4xx) — 정제된 일반 라벨. **원본 바디를 넣지 않는다.**
        _ => ForgeUnavailable::Other("GitHub is unavailable".to_string()),
    }
}

/// `PUT /pulls/{n}/merge` 실패를 **확정적 거부**(재시도 무의미) vs **일시**(재시도 가능)로
/// 가른다. gh `classify_merge_failure`의 규율을 HTTP 상태로 옮긴 것 — **좁게** 간다: 확정
/// 상태(405/409/rate-limit 아닌 403)에만 `Rejected`, 나머지는 전부 `Transient`. 넓히면
/// 일시 오류(5xx/429/network)를 "머지 거부됨"으로 날조해, 재시도하면 될 상황을 못박는다.
///
/// GitHub merge 엔드포인트의 확정 신호:
/// - **405 Method Not Allowed** → PR이 머지 가능한 상태가 아님. 바디 메시지로 세부 사유
///   (승인 필요/충돌/일반)를 정제한다.
/// - **409 Conflict** → head가 바뀌었거나 base와 충돌 → Conflict.
/// - **403**(rate-limit 아님) → 머지 권한 없음 → PermissionDenied.
///
/// 401/404/422/5xx/429/network 등 그 밖은 전부 `Transient` — 확정 거부로 날조하지 않는다.
pub fn classify_http_merge_failure(
    status: u16,
    ratelimit_remaining: Option<&str>,
    retry_after: Option<&str>,
    body: &str,
) -> MergeFailure {
    // rate-limit은 **절대 거부가 아니다** — 먼저 걸러 Transient로.
    if is_rate_limited(status, ratelimit_remaining, retry_after) {
        return MergeFailure::Transient(ForgeUnavailable::RateLimited);
    }
    match status {
        // 405: 머지 불가 확정. 바디로 세부 사유를 정제(없으면 일반 NotMergeable).
        405 => MergeFailure::Rejected(refine_405_reason(body)),
        // 409: 충돌 확정.
        409 => MergeFailure::Rejected(MergeRejection::Conflict),
        // 403(rate-limit 아님): 권한 확정.
        403 => MergeFailure::Rejected(MergeRejection::PermissionDenied),
        // 그 밖(401/404/422/5xx/네트워크 등) — 일시로. 확정 거부 날조 금지.
        _ => MergeFailure::Transient(classify_http_unavailable(
            status,
            ratelimit_remaining,
            retry_after,
        )),
    }
}

/// 405 바디에서 세부 거부 사유를 정제한다. GitHub은 405와 함께 "Pull Request is not
/// mergeable"(일반), 브랜치 보호로 인한 "At least N approving review..."(승인/차단),
/// 충돌 언급 등을 낸다. 인식 못 하면 일반 `NotMergeable`(405가 이미 확정이므로 Transient로
/// 떨어뜨리지 않는다).
fn refine_405_reason(body: &str) -> MergeRejection {
    let lower = body.to_ascii_lowercase();
    if lower.contains("conflict") {
        return MergeRejection::Conflict;
    }
    if lower.contains("changes requested") || lower.contains("changes were requested") {
        return MergeRejection::ChangesRequested;
    }
    // 승인/브랜치 보호/필수 체크. "at least"는 승인 맥락과 공존할 때만(gh 규율 미러).
    let at_least_reviews =
        lower.contains("at least") && (lower.contains("review") || lower.contains("approv"));
    if at_least_reviews
        || lower.contains("required status check")
        || lower.contains("branch protection")
        || lower.contains("protected branch")
        || lower.contains("review required")
        || lower.contains("approving review")
        || lower.contains("required by reviewers")
        || lower.contains("is blocked")
    {
        return MergeRejection::Blocked;
    }
    if lower.contains("already merged") || lower.contains("closed") {
        return MergeRejection::AlreadyClosed;
    }
    MergeRejection::NotMergeable
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_429_is_rate_limited() {
        assert!(is_rate_limited(429, None, None));
        assert_eq!(
            classify_http_unavailable(429, None, None),
            ForgeUnavailable::RateLimited
        );
    }

    /// **핵심 회귀 방어**: 403 + `x-ratelimit-remaining: 0`은 rate-limit(재시도 가능)이지
    /// permission이 아니다. rate-limit 판정을 없애면 이게 permission으로 오독된다.
    #[test]
    fn rate_limit_wins_over_permission() {
        assert!(is_rate_limited(403, Some("0"), None));
        assert_eq!(
            classify_http_unavailable(403, Some("0"), None),
            ForgeUnavailable::RateLimited
        );
        // 403 + retry-after(secondary/abuse)도 rate-limit.
        assert!(is_rate_limited(403, None, Some("60")));
        assert_eq!(
            classify_http_unavailable(403, None, Some("60")),
            ForgeUnavailable::RateLimited
        );
    }

    /// bare 403(리밋 신호 없음)은 permission이지 rate-limit이 아니다(양방향).
    #[test]
    fn bare_403_is_permission_not_rate_limit() {
        assert!(!is_rate_limited(403, Some("4999"), None));
        assert_eq!(
            classify_http_unavailable(403, Some("4999"), None),
            ForgeUnavailable::Other("permission denied".to_string())
        );
    }

    #[test]
    fn status_map_auth_repo_network() {
        assert_eq!(
            classify_http_unavailable(401, None, None),
            ForgeUnavailable::NotAuthenticated
        );
        assert_eq!(
            classify_http_unavailable(404, None, None),
            ForgeUnavailable::Other("repository unavailable".to_string())
        );
        assert_eq!(
            classify_http_unavailable(503, None, None),
            ForgeUnavailable::Network
        );
        assert_eq!(
            classify_http_unavailable(500, None, None),
            ForgeUnavailable::Network
        );
    }

    #[test]
    fn merge_405_and_409_and_403_are_definitive_rejections() {
        assert_eq!(
            classify_http_merge_failure(405, None, None, "Pull Request is not mergeable"),
            MergeFailure::Rejected(MergeRejection::NotMergeable)
        );
        assert_eq!(
            classify_http_merge_failure(
                405,
                None,
                None,
                "At least 1 approving review is required by reviewers with write access."
            ),
            MergeFailure::Rejected(MergeRejection::Blocked)
        );
        assert_eq!(
            classify_http_merge_failure(409, None, None, "Head branch was modified"),
            MergeFailure::Rejected(MergeRejection::Conflict)
        );
        assert_eq!(
            classify_http_merge_failure(403, None, None, "Must have write access"),
            MergeFailure::Rejected(MergeRejection::PermissionDenied)
        );
    }

    /// **핵심 회귀 방어 (b)**: 일시 merge 실패(5xx/429/network/401)는 `Rejected`가 아니라
    /// `Transient`여야 한다. rate-limit이 403으로 와도 거부로 날조되면 안 된다.
    #[test]
    fn transient_merge_failure_is_never_a_rejection() {
        assert_eq!(
            classify_http_merge_failure(503, None, None, ""),
            MergeFailure::Transient(ForgeUnavailable::Network)
        );
        assert_eq!(
            classify_http_merge_failure(429, None, None, ""),
            MergeFailure::Transient(ForgeUnavailable::RateLimited)
        );
        // 403 + rate-limit 신호 → 거부가 아니라 Transient(RateLimited).
        assert_eq!(
            classify_http_merge_failure(403, Some("0"), None, "rate limit"),
            MergeFailure::Transient(ForgeUnavailable::RateLimited)
        );
        assert_eq!(
            classify_http_merge_failure(401, None, None, "Bad credentials"),
            MergeFailure::Transient(ForgeUnavailable::NotAuthenticated)
        );
        // 404(PR/repo 없음)도 확정 거부로 날조하지 않는다.
        assert!(matches!(
            classify_http_merge_failure(404, None, None, "Not Found"),
            MergeFailure::Transient(_)
        ));
    }
}
