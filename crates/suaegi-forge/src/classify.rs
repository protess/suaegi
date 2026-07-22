use crate::provider::ForgeUnavailable;

/// "PR 없음"을 판정한다. **성공 조회는 no-PR을 데이터로 안 돌려준다** — 그냥 비-0 exit +
/// stderr다(Orca `isNoPullRequestError` `client.ts:220-223`:
/// `/no pull requests? found|could not find.*pull request/i`). LC_ALL=C(§3.1)로 stderr가
/// 영어로 고정되므로 이 고정 substring이 안정적이다.
///
/// **이 함수가 좁아야 한다.** 넓히면 일시 오류(레이트리밋·네트워크)를 "PR 없음"으로
/// 뭉개 알려진 PR 상태를 지운다 — §5의 mutation이 정확히 이 붕괴를 겨냥한다.
pub fn is_no_pull_request(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    // "no pull request found" | "no pull requests found"
    if lower.contains("no pull request found") || lower.contains("no pull requests found") {
        return true;
    }
    // "could not find ... pull request"
    if lower.contains("could not find") && lower.contains("pull request") {
        return true;
    }
    false
}

/// raw gh stderr를 **분류된** `ForgeUnavailable`로 매핑한다(Orca
/// `pr-refresh-error-classification.ts:20-122`의 순서를 미러). raw 문자열이 UI에 닿지
/// 않게 하는 경계다[Codex S1]. `Other`에도 원본 stderr를 넣지 않고 정제된 라벨만 넣는다.
///
/// **순서가 load-bearing이다**: rate-limit(429/secondary)을 permission(403)보다 먼저 —
/// GitHub은 두 리밋 모두에 403 OR 429를 낸다. 404(repo)를 network보다 먼저 — "network"
/// substring이 우연히 든 repo 오류를 연결 실패로 오독하지 않게. auth는 마지막 — "author"
/// 안의 "auth"에 오발하지 않게 전체 구절로만 본다.
pub fn classify_unavailable(stderr: &str) -> ForgeUnavailable {
    let lower = stderr.to_lowercase();

    // 1. 레이트 리밋 먼저.
    let is_429 = lower.contains("http 429") || lower.contains("429 too many requests");
    let is_403 = lower.contains("http 403");
    let has_retry_after = lower.contains("retry-after");
    if is_429
        || lower.contains("secondary rate limit")
        || lower.contains("abuse detection")
        || lower.contains("you have triggered an abuse")
        || ((is_403 || is_429) && has_retry_after)
        || lower.contains("api rate limit exceeded")
        || lower.contains("rate limit")
    {
        return ForgeUnavailable::RateLimited;
    }

    // 2. repo 해석 실패(404)를 network보다 먼저 — 정제 라벨만.
    if lower.contains("http 404") || lower.contains("could not resolve to a repository") {
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
    {
        return ForgeUnavailable::Network;
    }

    // 4. permission(403), rate-limit 뒤에.
    if is_403 || lower.contains("resource not accessible") {
        return ForgeUnavailable::Other("permission denied".to_string());
    }

    // 5. gh 실행 실패(문자열로 새어 온 경우; spawn ENOENT는 호출부가 먼저 NotInstalled로 처리).
    if lower.contains("gh: command not found") || lower.contains("'gh' is not recognized") {
        return ForgeUnavailable::NotInstalled;
    }

    // 6. auth 마지막 — 전체 구절/401만, 맨 "auth"(author) 금지.
    if lower.contains("http 401")
        || lower.contains("unauthorized")
        || lower.contains("authentication")
        || lower.contains("bad credentials")
        || lower.contains("gh auth")
        || lower.contains("not logged")
    {
        return ForgeUnavailable::NotAuthenticated;
    }

    // 분류 밖 — 정제된 일반 라벨. **원본 stderr를 넣지 않는다.**
    ForgeUnavailable::Other("GitHub is unavailable".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_the_no_pr_phrases() {
        assert!(is_no_pull_request(
            "no pull requests found for branch \"feat\""
        ));
        assert!(is_no_pull_request("no pull request found"));
        assert!(is_no_pull_request(
            "GraphQL: Could not find a pull request for this branch"
        ));
    }

    /// **핵심 회귀 방어**: 일시 오류(레이트리밋·네트워크·auth)는 "PR 없음"이 아니다.
    /// `is_no_pull_request`가 넓어지면 알려진 PR 상태를 지운다(§5 Degraded 규율).
    #[test]
    fn transient_failures_are_not_read_as_no_pr() {
        assert!(!is_no_pull_request("HTTP 429: API rate limit exceeded"));
        assert!(!is_no_pull_request("error connecting: could not resolve host"));
        assert!(!is_no_pull_request("HTTP 401: Bad credentials"));
        assert!(!is_no_pull_request("HTTP 503: Service Unavailable"));
        // "could not find" 하나만으로는 안 된다 — "pull request"가 함께 있어야.
        assert!(!is_no_pull_request("could not find the config file"));
    }

    #[test]
    fn rate_limit_wins_over_permission() {
        // 403 + retry-after는 레이트리밋(secondary)이지 permission이 아니다.
        assert_eq!(
            classify_unavailable("HTTP 403: retry-after: 60"),
            ForgeUnavailable::RateLimited
        );
        assert_eq!(
            classify_unavailable("You have exceeded a secondary rate limit"),
            ForgeUnavailable::RateLimited
        );
        assert_eq!(
            classify_unavailable("API rate limit exceeded for user"),
            ForgeUnavailable::RateLimited
        );
    }

    #[test]
    fn plain_403_is_permission_not_auth() {
        assert_eq!(
            classify_unavailable("HTTP 403: Resource not accessible by integration"),
            ForgeUnavailable::Other("permission denied".to_string())
        );
    }

    #[test]
    fn repo_404_ranks_before_network() {
        // "network"이 우연히 들어도 404가 먼저 잡혀 network로 오독하지 않는다.
        assert_eq!(
            classify_unavailable("HTTP 404: Could not resolve to a Repository named net-work"),
            ForgeUnavailable::Other("repository unavailable".to_string())
        );
    }

    #[test]
    fn network_phrases_map_to_network() {
        assert_eq!(
            classify_unavailable("dial tcp: connection refused"),
            ForgeUnavailable::Network
        );
        assert_eq!(
            classify_unavailable("could not resolve host: api.github.com"),
            ForgeUnavailable::Network
        );
    }

    #[test]
    fn auth_phrases_map_to_not_authenticated() {
        assert_eq!(
            classify_unavailable("HTTP 401: Bad credentials"),
            ForgeUnavailable::NotAuthenticated
        );
        assert_eq!(
            classify_unavailable("To get started with GitHub CLI, please run: gh auth login"),
            ForgeUnavailable::NotAuthenticated
        );
    }

    /// "author"의 "auth"에 오발하면 안 된다 — 완전 구절만 auth로 본다.
    #[test]
    fn author_substring_does_not_trip_auth() {
        assert_eq!(
            classify_unavailable("failed to read author field from commit"),
            ForgeUnavailable::Other("GitHub is unavailable".to_string())
        );
    }

    /// `Other`는 **원본 stderr를 담지 않는다**[Codex S1] — 정제된 라벨만.
    #[test]
    fn other_does_not_leak_raw_stderr() {
        let secret = "fatal: token ghp_SECRET_LEAK leaked in stderr blah";
        match classify_unavailable(secret) {
            ForgeUnavailable::Other(msg) => {
                assert!(!msg.contains("ghp_SECRET_LEAK"), "raw stderr leaked to UI: {msg}");
            }
            other => panic!("expected Other, got {other:?}"),
        }
    }
}
