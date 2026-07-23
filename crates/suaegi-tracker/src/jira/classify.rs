//! **분류 crux (§2).** Jira REST 응답 상태코드 → 성공 / 진짜 not-found / 분류된 실패.
//! `suaegi-forge/src/github_http/classify.rs`의 규율을 미러한다: **일시(5xx/429/network) 실패는
//! 절대 확정적 부정(None/empty)으로 오독하지 않는다**.
//!
//! Orca Jira의 `isAuthError`는 **401만** 크리덴셜 무효로 본다(`client.ts:632-636`) — 403은
//! 프로젝트/API 권한 갭이지 크리덴셜 무효가 아니다. 그래서 401→NotAuthenticated, 403→Forbidden로
//! **가른다**(합치면 재-인증하면 될 상황과 권한 부족을 못 구분).
//!
//! **404의 이중 의미**(forge `review_by_number` 미러): 특정 리소스 엔드포인트(`GET /issue/{key}`)의
//! 404 = 그 이슈 없음 = [`JiraStatus::NotFound`](→ 호출부가 `Lookup::NotFound`). 컬렉션/전역
//! 엔드포인트의 404 = 사이트/API 경로 문제 = `Unavailable`. 그래서 404는 여기서 **중립적**
//! `NotFound` 클래스로 두고, **어느 의미인지는 각 read op이** 정한다.

use super::model::TrackerUnavailable;

/// HTTP 상태 분류 결과. 성공(2xx), 중립적 404(호출부가 의미 결정), 또는 분류된 실패.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JiraStatus {
    /// 2xx. 바디를 파싱해 필드를 뽑는다.
    Success,
    /// 404. **의미는 호출부가 정한다** — 특정 이슈 조회면 not-found(None), 컬렉션이면 Unavailable.
    NotFound,
    /// 그 밖(비-2xx, 404 제외). 분류된 축.
    Unavailable(TrackerUnavailable),
}

fn is_2xx(status: u16) -> bool {
    (200..300).contains(&status)
}

/// 429 판정. Jira는 레이트리밋에 **429 + `Retry-After`**를 낸다. (Orca Jira는 명시적 429 처리가
/// 없지만, forge의 `is_rate_limited` 규율을 여기 적용한다 — transient→RateLimited, 거짓 음성 금지.)
pub fn is_rate_limited(status: u16, retry_after: Option<&str>) -> bool {
    if status == 429 {
        return true;
    }
    // 일부 프록시/게이트웨이는 403 + Retry-After로 스로틀을 낸다 — 그건 권한이 아니라 레이트리밋.
    if status == 403 {
        return retry_after.map(|v| !v.trim().is_empty()).unwrap_or(false);
    }
    false
}

/// 상태코드(+`Retry-After`) → [`JiraStatus`]. **순서가 load-bearing**(forge `classify_http_unavailable`
/// 미러): rate-limit → 2xx → 404(중립) → auth(401) → permission(403) → 그 밖.
pub fn classify_jira_status(status: u16, retry_after: Option<&str>) -> JiraStatus {
    // 1. rate-limit 먼저(429/403+retry-after). permission(bare 403)보다 앞. **재시도 가능.**
    if is_rate_limited(status, retry_after) {
        return JiraStatus::Unavailable(TrackerUnavailable::RateLimited);
    }
    // 2. 성공.
    if is_2xx(status) {
        return JiraStatus::Success;
    }
    match status {
        // 3. 404는 중립 — 호출부가 not-found(None) vs API-unavailable을 정한다.
        404 => JiraStatus::NotFound,
        // 4. 401만 크리덴셜 무효(Orca isAuthError).
        401 => JiraStatus::Unavailable(TrackerUnavailable::NotAuthenticated),
        // 5. bare 403(rate-limit 아님) = 권한 부족. 크리덴셜은 유효 → NotAuthenticated 아님.
        403 => JiraStatus::Unavailable(TrackerUnavailable::Forbidden),
        // 6. 서버측 일시(5xx). 500은 Internal, 그 밖 5xx는 재시도 가능한 Network.
        500 => JiraStatus::Unavailable(TrackerUnavailable::Internal),
        s if s >= 500 => JiraStatus::Unavailable(TrackerUnavailable::Network),
        // 7. 그 밖 4xx(400 등) = 클라이언트 확정 실패지만 **None이 아니다**.
        s if s >= 400 => JiraStatus::Unavailable(TrackerUnavailable::InvalidInput),
        // 8. 2xx/3xx도 아닌 예상 밖 → 모른다(성공/None 아님).
        _ => JiraStatus::Unavailable(TrackerUnavailable::Unknown),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_is_2xx() {
        assert_eq!(classify_jira_status(200, None), JiraStatus::Success);
        assert_eq!(classify_jira_status(201, None), JiraStatus::Success);
        assert_eq!(classify_jira_status(204, None), JiraStatus::Success);
    }

    /// 404는 **중립** NotFound — 축이 아니다. 호출부가 의미를 정한다.
    #[test]
    fn status_404_is_neutral_not_found() {
        assert_eq!(classify_jira_status(404, None), JiraStatus::NotFound);
    }

    /// **crux: 401만 크리덴셜 무효, 403은 permission**(Orca isAuthError 미러). 둘을 합치면
    /// 재-인증하면 될 상황과 권한 부족을 못 구분한다.
    #[test]
    fn auth_401_and_permission_403_are_distinct() {
        assert_eq!(
            classify_jira_status(401, None),
            JiraStatus::Unavailable(TrackerUnavailable::NotAuthenticated)
        );
        assert_eq!(
            classify_jira_status(403, None),
            JiraStatus::Unavailable(TrackerUnavailable::Forbidden)
        );
    }

    /// **crux (a): 일시 실패는 재시도 가능한 축이지 not-found/empty가 아니다.**
    #[test]
    fn transient_statuses_are_retryable_axes_never_notfound() {
        assert_eq!(
            classify_jira_status(429, None),
            JiraStatus::Unavailable(TrackerUnavailable::RateLimited)
        );
        assert_eq!(
            classify_jira_status(500, None),
            JiraStatus::Unavailable(TrackerUnavailable::Internal)
        );
        assert_eq!(
            classify_jira_status(503, None),
            JiraStatus::Unavailable(TrackerUnavailable::Network)
        );
        // 어느 것도 NotFound(None)로 새지 않는다.
        for s in [429, 500, 502, 503] {
            assert_ne!(classify_jira_status(s, None), JiraStatus::NotFound, "status={s}");
        }
    }

    /// 403 + Retry-After(프록시 스로틀)는 permission이 아니라 rate-limit(재시도 가능). rate-limit
    /// 판정이 permission보다 먼저 와야 한다.
    #[test]
    fn rate_limit_wins_over_permission() {
        assert!(is_rate_limited(403, Some("30")));
        assert_eq!(
            classify_jira_status(403, Some("30")),
            JiraStatus::Unavailable(TrackerUnavailable::RateLimited)
        );
        // bare 403(retry-after 없음)은 permission.
        assert!(!is_rate_limited(403, None));
        assert_eq!(
            classify_jira_status(403, None),
            JiraStatus::Unavailable(TrackerUnavailable::Forbidden)
        );
    }

    /// 그 밖 4xx(400 등)는 InvalidInput — 확정 실패지만 None이 아니다.
    #[test]
    fn other_4xx_is_invalid_input_not_none() {
        assert_eq!(
            classify_jira_status(400, None),
            JiraStatus::Unavailable(TrackerUnavailable::InvalidInput)
        );
    }
}
