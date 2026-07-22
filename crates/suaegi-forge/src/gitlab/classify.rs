use crate::pr_actions::{MergeFailure, MergeRejection};
use crate::provider::ForgeUnavailable;

/// "MR 없음"을 판정한다(gh의 `is_no_pull_request` 미러). **성공 조회는 no-MR을 데이터로
/// 안 돌려준다** — `glab mr view`가 비-0 exit + stderr로 낸다. glab의 실제 신호는
/// "no open merge request available for <branch>"(mrutils)와, IID 조회 시 API의
/// `404 Merge Request Not Found`다. LC_ALL=C(`runner.rs`)로 stderr가 영어로 고정되므로
/// 이 고정 substring이 안정적이다.
///
/// **이 함수가 좁아야 한다.** 넓히면 일시 오류(레이트리밋·네트워크)를 "MR 없음"으로 뭉개
/// 알려진 MR 상태를 지운다 — 정확히 §5 mutation이 겨냥하는 붕괴다(gh와 동일 규율).
///
/// 특히 `404`는 **"merge request"가 함께 있을 때만** no-MR으로 본다: 프로젝트 404
/// ("404 Project Not Found")는 repo가 사라진 것이지 MR이 없는 게 아니므로 Unavailable로
/// 남아야 한다.
pub fn is_no_merge_request(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    // glab CLI의 브랜치 조회 실패 문구.
    if lower.contains("no open merge request") || lower.contains("no merge request found") {
        return true;
    }
    if lower.contains("no merge requests found") {
        return true;
    }
    // API의 404 — 반드시 "merge request"와 공존해야 한다(프로젝트 404와 구별).
    if (lower.contains("404") || lower.contains("not found")) && lower.contains("merge request") {
        return true;
    }
    false
}

/// raw glab stderr를 **분류된** `ForgeUnavailable`로 매핑한다(Orca
/// `glab-error-classification.ts` + `merge-request-creation.ts:classifyCreateMRError`의
/// 신호를 미러). raw 문자열이 UI에 닿지 않게 하는 경계다 — `Other`에도 원본 stderr를 넣지
/// 않고 정제된 라벨만 넣는다(gh classify와 동일 규율).
///
/// **순서가 load-bearing이다**(gh classify 미러): rate-limit(429)을 permission(403)보다
/// 먼저 — 둘 다 http 상태로 오므로. 404(project)를 network보다 먼저 — "network" substring이
/// 우연히 든 404를 연결 실패로 오독하지 않게. auth는 마지막 — "author" 안의 "auth"에
/// 오발하지 않게 전체 구절/401로만 본다.
pub fn classify_glab_unavailable(stderr: &str) -> ForgeUnavailable {
    let lower = stderr.to_lowercase();

    // 1. 레이트 리밋 먼저.
    if lower.contains("http 429")
        || lower.contains("429 too many requests")
        || lower.contains("rate limit")
        || lower.contains("retry later")
    {
        return ForgeUnavailable::RateLimited;
    }

    // 2. project 해석 실패(404)를 network보다 먼저 — 정제 라벨만.
    if lower.contains("http 404")
        || lower.contains("404 project not found")
        || lower.contains("project not found")
    {
        return ForgeUnavailable::Other("repository unavailable".to_string());
    }

    // 3. 네트워크: 구조화된 오류 코드/완전한 연결 구절만. 맨 "network" substring은 금지.
    if lower.contains("etimedout")
        || lower.contains("econnreset")
        || lower.contains("econnrefused")
        || lower.contains("enotfound")
        || lower.contains("eai_again")
        || lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("could not resolve host")
        || lower.contains("no such host")
        || lower.contains("network is unreachable")
        || lower.contains("network is down")
        || lower.contains("connection refused")
        || lower.contains("connection reset")
        || lower.contains("dial tcp")
    {
        return ForgeUnavailable::Network;
    }

    // 4. permission(403), rate-limit 뒤에.
    if lower.contains("http 403")
        || lower.contains("forbidden")
        || lower.contains("insufficient_scope")
        || lower.contains("insufficient scopes")
    {
        return ForgeUnavailable::Other("permission denied".to_string());
    }

    // 5. glab 실행 실패(문자열로 새어 온 경우; spawn ENOENT는 호출부가 먼저 NotInstalled로 처리).
    if lower.contains("glab: command not found") || lower.contains("'glab' is not recognized") {
        return ForgeUnavailable::NotInstalled;
    }

    // 6. auth 마지막 — 전체 구절/401만, 맨 "auth"(author) 금지.
    if lower.contains("http 401")
        || lower.contains("unauthorized")
        || lower.contains("authentication")
        || lower.contains("glab auth")
        || lower.contains("not logged")
        || lower.contains("not authenticated")
    {
        return ForgeUnavailable::NotAuthenticated;
    }

    // 분류 밖 — 정제된 일반 라벨. **원본 stderr를 넣지 않는다.**
    ForgeUnavailable::Other("GitLab is unavailable".to_string())
}

/// `glab mr merge` 실패 stderr를 확정적 거부 vs 일시 실패로 가른다(gh
/// `classify_merge_failure` 미러). **좁게** 간다: pinned GitLab 구절에만 `Rejected`를 내고,
/// 나머지는 전부 `Transient`(→ `Unavailable`)다. 넓히면 일시 오류(네트워크·레이트리밋)를
/// "머지 거부됨"으로 오독해, 재시도하면 될 상황을 확정적 실패로 못박는다 — 정확히 §5
/// mutation이 겨냥하는 붕괴다.
///
/// GitLab의 신호는 gh와 다르므로 GitLab 실제 문구로 pin한다(브리프 지시): merge conflict,
/// "not mergeable"/"cannot be merged", 승인 필요, 브랜치 보호/파이프라인, 이미 머지/닫힘 등.
/// LC_ALL=C로 glab stderr가 영어로 고정되므로 pinned substring이 안정적이다.
pub fn classify_glab_merge_failure(stderr: &str) -> MergeFailure {
    let lower = stderr.to_lowercase();

    // 이미 닫힘/머지됨 — 재시도 무의미한 확정 상태.
    if lower.contains("already merged")
        || lower.contains("merge request is already merged")
        || lower.contains("already been merged")
        || lower.contains("merge request is closed")
        || lower.contains("closed merge request")
    {
        return MergeFailure::Rejected(MergeRejection::AlreadyClosed);
    }

    // 충돌 — "not mergeable"보다 먼저(더 구체적).
    if lower.contains("merge conflict")
        || lower.contains("has conflicts")
        || lower.contains("conflicts with the")
        || lower.contains("cannot be merged due to conflict")
    {
        return MergeFailure::Rejected(MergeRejection::Conflict);
    }

    // 변경 요청 — 리뷰 결정.
    if lower.contains("changes requested") || lower.contains("changes were requested") {
        return MergeFailure::Rejected(MergeRejection::ChangesRequested);
    }

    // 브랜치 보호/필수 승인/파이프라인 등으로 차단.
    // **`at least`는 일반 영어 bigram이라 단독 매칭 금지**(gh와 동일 규율) — transient stderr가
    // 우연히 포함하면 확정 거부로 날조된다. 승인 맥락(`approv`/`review`)과 공존할 때만 차단.
    let at_least_approvals =
        lower.contains("at least") && (lower.contains("approv") || lower.contains("review"));
    if lower.contains("not approved")
        || lower.contains("requires approval")
        || lower.contains("approvals are required")
        || lower.contains("approval is required")
        || lower.contains("required approvals")
        || at_least_approvals
        || lower.contains("pipeline must succeed")
        || lower.contains("pipeline did not succeed")
        || lower.contains("ci must pass")
        || lower.contains("protected branch")
        || lower.contains("branch protection")
        || lower.contains("discussions_not_resolved")
        || lower.contains("unresolved discussion")
        || lower.contains("blocked_status")
    {
        return MergeFailure::Rejected(MergeRejection::Blocked);
    }

    // 권한 — 머지 특정 구절만(bare 403은 아래 classify_glab_unavailable로 Unavailable).
    if lower.contains("not allowed to merge")
        || lower.contains("not authorized to merge")
        || lower.contains("insufficient permissions to merge")
        || lower.contains("no permission to merge")
        || lower.contains("developer access")
    {
        return MergeFailure::Rejected(MergeRejection::PermissionDenied);
    }

    // 머지 불가(일반) — 위 구체 사유가 안 잡힌 pinned "not mergeable"/"cannot be merged".
    if lower.contains("not mergeable")
        || lower.contains("is not mergeable")
        || lower.contains("cannot be merged")
        || lower.contains("branch cannot be merged")
        || lower.contains("method is not allowed")
    {
        return MergeFailure::Rejected(MergeRejection::NotMergeable);
    }

    // 그 밖 전부 — 일시로 간주(분류된 Unavailable). 인식 못 한 실패를 지어낸 확정
    // 거부로 못박지 않는다.
    MergeFailure::Transient(classify_glab_unavailable(stderr))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_the_no_mr_phrases() {
        assert!(is_no_merge_request(
            "no open merge request available for \"feat\""
        ));
        assert!(is_no_merge_request("no merge request found for this branch"));
        assert!(is_no_merge_request("GET .../merge_requests/9: 404 Merge Request Not Found"));
    }

    /// **핵심 회귀 방어**: 일시 오류(레이트리밋·네트워크·auth)와 **프로젝트 404**는
    /// "MR 없음"이 아니다. `is_no_merge_request`가 넓어지면 알려진 MR 상태를 지운다.
    #[test]
    fn transient_and_project_404_are_not_read_as_no_mr() {
        assert!(!is_no_merge_request("HTTP 429: Too Many Requests"));
        assert!(!is_no_merge_request("could not resolve host gitlab.com"));
        assert!(!is_no_merge_request("HTTP 401: unauthorized"));
        // 프로젝트 404는 repo가 사라진 것 — "merge request"가 없으니 no-MR 아니다.
        assert!(!is_no_merge_request("GET .../projects/x: 404 Project Not Found"));
        // 맨 404 하나로는 안 된다.
        assert!(!is_no_merge_request("404 Not Found"));
    }

    #[test]
    fn rate_limit_wins_over_permission() {
        assert_eq!(
            classify_glab_unavailable("HTTP 429: rate limit exceeded"),
            ForgeUnavailable::RateLimited
        );
        assert_eq!(
            classify_glab_unavailable("You have hit the rate limit; retry later"),
            ForgeUnavailable::RateLimited
        );
    }

    #[test]
    fn project_404_is_repository_unavailable_before_network() {
        // "network"이 우연히 들어도 404가 먼저 잡혀 network로 오독하지 않는다.
        assert_eq!(
            classify_glab_unavailable("HTTP 404: project not found on net-work host"),
            ForgeUnavailable::Other("repository unavailable".to_string())
        );
    }

    #[test]
    fn network_phrases_map_to_network() {
        assert_eq!(
            classify_glab_unavailable("dial tcp: connection refused"),
            ForgeUnavailable::Network
        );
        assert_eq!(
            classify_glab_unavailable("could not resolve host: gitlab.example.com"),
            ForgeUnavailable::Network
        );
    }

    #[test]
    fn forbidden_is_permission_not_auth() {
        assert_eq!(
            classify_glab_unavailable("HTTP 403: insufficient_scope"),
            ForgeUnavailable::Other("permission denied".to_string())
        );
    }

    #[test]
    fn auth_phrases_map_to_not_authenticated() {
        assert_eq!(
            classify_glab_unavailable("HTTP 401: unauthorized"),
            ForgeUnavailable::NotAuthenticated
        );
        assert_eq!(
            classify_glab_unavailable("run `glab auth login` to authenticate"),
            ForgeUnavailable::NotAuthenticated
        );
    }

    /// "author"의 "auth"에 오발하면 안 된다 — 완전 구절만 auth로 본다.
    #[test]
    fn author_substring_does_not_trip_auth() {
        assert_eq!(
            classify_glab_unavailable("failed to read author field from commit"),
            ForgeUnavailable::Other("GitLab is unavailable".to_string())
        );
    }

    /// `Other`는 **원본 stderr를 담지 않는다** — 정제된 라벨만.
    #[test]
    fn other_does_not_leak_raw_stderr() {
        let secret = "fatal: token glpat-SECRET_LEAK leaked in stderr blah";
        match classify_glab_unavailable(secret) {
            ForgeUnavailable::Other(msg) => {
                assert!(
                    !msg.contains("glpat-SECRET_LEAK"),
                    "raw stderr leaked to UI: {msg}"
                );
            }
            other => panic!("expected Other, got {other:?}"),
        }
    }

    #[test]
    fn merge_conflict_and_blocked_and_permission_and_closed() {
        assert_eq!(
            classify_glab_merge_failure("Merge request is not mergeable: has conflicts"),
            MergeFailure::Rejected(MergeRejection::Conflict)
        );
        assert_eq!(
            classify_glab_merge_failure("At least 1 approval is required by this project"),
            MergeFailure::Rejected(MergeRejection::Blocked)
        );
        assert_eq!(
            classify_glab_merge_failure("You are not allowed to merge this merge request"),
            MergeFailure::Rejected(MergeRejection::PermissionDenied)
        );
        assert_eq!(
            classify_glab_merge_failure("Merge request is already merged"),
            MergeFailure::Rejected(MergeRejection::AlreadyClosed)
        );
        assert_eq!(
            classify_glab_merge_failure("The merge request cannot be merged"),
            MergeFailure::Rejected(MergeRejection::NotMergeable)
        );
    }

    /// **핵심 회귀 방어**: 일시 glab 실패(레이트리밋·네트워크·auth)는 "머지 거부됨"이
    /// 아니라 `Transient`(→ Unavailable)여야 한다.
    #[test]
    fn transient_merge_failure_is_not_a_rejection() {
        assert_eq!(
            classify_glab_merge_failure("HTTP 429: rate limit exceeded"),
            MergeFailure::Transient(ForgeUnavailable::RateLimited)
        );
        assert_eq!(
            classify_glab_merge_failure("could not resolve host gitlab.com"),
            MergeFailure::Transient(ForgeUnavailable::Network)
        );
        assert_eq!(
            classify_glab_merge_failure("HTTP 401: unauthorized"),
            MergeFailure::Transient(ForgeUnavailable::NotAuthenticated)
        );
        assert!(matches!(
            classify_glab_merge_failure("some unexpected glab explosion"),
            MergeFailure::Transient(_)
        ));
    }

    /// **회귀 방어 — `at least` pin이 transient를 날조하면 안 된다.** 승인 맥락 없이
    /// transient stderr에 "at least"가 우연히 들어도 `Transient`로 유지돼야 한다.
    #[test]
    fn at_least_pin_does_not_fabricate_rejection_from_transient() {
        assert_eq!(
            classify_glab_merge_failure("HTTP 429: rate limited; wait at least a minute"),
            MergeFailure::Transient(ForgeUnavailable::RateLimited)
        );
        assert!(matches!(
            classify_glab_merge_failure("upload failed: file must be at least 1 byte"),
            MergeFailure::Transient(_)
        ));
        // 진짜 승인 차단은 여전히 Blocked(양방향).
        assert_eq!(
            classify_glab_merge_failure("At least 2 approvals are required before merging"),
            MergeFailure::Rejected(MergeRejection::Blocked)
        );
    }
}
