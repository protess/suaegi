//! **분류 crux (§1.1).** Linear GraphQL 응답을 성공(data) vs 분류된 실패로 가른다.
//!
//! ground truth는 Orca가 아니라 `@linear/sdk`(`graphql-client.ts` `rawRequest`, `error.ts`
//! `parseLinearError`)다. Orca의 `error.message` substring 매칭(`linear/issues.ts:857-891`)은
//! **미러하지 않는다** — 열등하다. `errors[0].extensions.type` enum으로 직접 분류한다.
//!
//! **핵심 계약**:
//! - 성공 = `HTTP 2xx` **AND** `errors` 키 **부재** **AND** `data` 비-null. 셋 다여야 한다.
//! - `errors` 키가 존재하면(빈 배열 `[]`이어도!) 성공 아님 — **"키 부재 ≠ 성공" 함정**. JS에선
//!   빈 배열도 truthy라 실패로 가고, 우리도 `Some(errors)`면 성공 아님으로 본다.
//! - `HTTP 2xx + errors 없음 + data:null` → `Unknown`, **절대 None/empty 아님**(캐시-오염 방지).

use super::model::{Classified, TrackerUnavailable};
use serde_json::Value;

/// [`classify_graphql`]의 결과. 성공이면 `data` 값을, 실패면 분류된 [`Classified`]를 준다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphqlOutcome {
    /// 성공. `data`(비-null)를 그대로 넘긴다 — 각 read op이 자기 필드를 뽑는다.
    Success(Value),
    /// 실패. 분류된 축(+ 사용자용 메시지). **절대 None/empty로 접히지 않는다.**
    Failure(Classified),
}

fn is_2xx(status: u16) -> bool {
    (200..300).contains(&status)
}

/// HTTP 상태만으로 축을 정한다(extensions.type이 없거나 못 알아볼 때의 폴백). §1.1:
/// 403→Forbidden, 429→Rate, 500→Internal, 그 밖 5xx→Network, 그 밖 4xx→Auth, 나머지→Unknown.
fn classify_status(status: u16) -> TrackerUnavailable {
    match status {
        403 => TrackerUnavailable::Forbidden,
        429 => TrackerUnavailable::RateLimited,
        500 => TrackerUnavailable::Internal,
        s if s >= 500 => TrackerUnavailable::Network,
        // 401 포함 그 밖 4xx는 인증 축으로(§1.1 "4xx→Auth").
        s if s >= 400 => TrackerUnavailable::NotAuthenticated,
        // 2xx/3xx인데 신호가 없다 → 모른다(성공/None 아님).
        _ => TrackerUnavailable::Unknown,
    }
}

/// `errors[0].extensions.type` 문자열 enum → 축. 못 알아보거나 없으면 `None`(호출부가 상태로 폴백).
fn kind_from_extensions_type(type_str: &str) -> Option<TrackerUnavailable> {
    match type_str {
        "authentication error" => Some(TrackerUnavailable::NotAuthenticated),
        "ratelimited" => Some(TrackerUnavailable::RateLimited),
        "forbidden" => Some(TrackerUnavailable::Forbidden),
        "network error" => Some(TrackerUnavailable::Network),
        "internal error" => Some(TrackerUnavailable::Internal),
        // 둘 다 클라이언트-측 확정 실패지만 **None이 아니다**.
        "invalid input" | "user error" => Some(TrackerUnavailable::InvalidInput),
        _ => None,
    }
}

/// `errors` 배열(존재 확정)을 분류한다. **`errors[0]`만** 본다(배열 전체 아님). 사용자 메시지는
/// `errors[0].extensions.userPresentableMessage`에서만 — raw `.message`(쿼리 내부 누출)는 절대.
fn classify_errors(errors: &Value, status: u16) -> Classified {
    // 빈 배열 `errors: []`이거나 배열이 아니면 → 상태로 폴백(성공 아님). "키 부재 ≠ 성공" 함정의
    // 반대편: 키는 있으나 첫 항목이 없다 → 여전히 실패로 취급하되 상태로 축을 정한다.
    let Some(first) = errors.get(0) else {
        return Classified::new(classify_status(status));
    };
    let ext = first.get("extensions");
    // 사용자에게 보여도 되는 유일한 문자열. raw message는 절대 읽지 않는다.
    let user_message = ext
        .and_then(|e| e.get("userPresentableMessage"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    // extensions.type으로 분류, 없거나 못 알아보면 상태로 폴백.
    let kind = ext
        .and_then(|e| e.get("type"))
        .and_then(|v| v.as_str())
        .and_then(kind_from_extensions_type)
        .unwrap_or_else(|| classify_status(status));
    Classified { kind, user_message }
}

/// **분류 진입점.** 상태코드 + raw 바디 → 성공(data) 또는 분류된 실패.
///
/// 순서가 load-bearing이다:
/// 1. 바디 파싱 실패 → 2xx면 `Unknown`(예상 밖 출력), 아니면 상태로 분류. **절대 성공/None 아님.**
/// 2. `errors` 키 존재(빈 배열이어도) → 성공 아님, `classify_errors`.
/// 3. errors 없음 + 비-2xx → 상태로 분류(성공 아님).
/// 4. errors 없음 + 2xx + `data` 비-null → 성공.
/// 5. errors 없음 + 2xx + `data` null/부재 → `Unknown`(절대 None/empty 아님).
pub fn classify_graphql(status: u16, body: &str) -> GraphqlOutcome {
    let value: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => {
            // 파싱 불가. 2xx면 예상 밖 출력(모름), 아니면 상태로. raw 바디는 담지 않는다.
            let kind = if is_2xx(status) {
                TrackerUnavailable::Unknown
            } else {
                classify_status(status)
            };
            return GraphqlOutcome::Failure(Classified::new(kind));
        }
    };

    // **errors 키가 있으면 성공 아님** — 빈 배열이어도. `get`이 Some이면 키가 존재하는 것.
    if let Some(errors) = value.get("errors") {
        return GraphqlOutcome::Failure(classify_errors(errors, status));
    }

    // errors 없음. 비-2xx면 상태로 분류(성공/None 아님).
    if !is_2xx(status) {
        return GraphqlOutcome::Failure(Classified::new(classify_status(status)));
    }

    // 2xx + errors 없음. data가 비-null이어야 성공.
    match value.get("data") {
        Some(data) if !data.is_null() => GraphqlOutcome::Success(data.clone()),
        // data:null 또는 data 부재 → Unknown. **절대 None/empty가 아니다**(캐시-오염 방지 crux).
        _ => GraphqlOutcome::Failure(Classified::new(TrackerUnavailable::Unknown)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fail(o: GraphqlOutcome) -> Classified {
        match o {
            GraphqlOutcome::Failure(c) => c,
            GraphqlOutcome::Success(_) => panic!("expected Failure, got Success"),
        }
    }

    #[test]
    fn success_requires_2xx_no_errors_and_nonnull_data() {
        let o = classify_graphql(200, r#"{"data":{"viewer":{"id":"u1"}}}"#);
        match o {
            GraphqlOutcome::Success(data) => {
                assert_eq!(data["viewer"]["id"], "u1");
            }
            GraphqlOutcome::Failure(c) => panic!("expected success, got {c:?}"),
        }
    }

    /// **crux (a)**: HTTP 200 + `errors` 존재 → 성공 아님. 이걸 성공/빈결과로 뭉개면 조회가
    /// None/empty로 읽히는 캐시-오염 회귀가 난다.
    #[test]
    fn http_200_with_errors_is_never_success() {
        let body = r#"{"errors":[{"message":"raw internal query text",
            "extensions":{"type":"ratelimited","userPresentableMessage":"Slow down"}}]}"#;
        let c = fail(classify_graphql(200, body));
        assert_eq!(c.kind, TrackerUnavailable::RateLimited);
        assert_eq!(c.user_message.as_deref(), Some("Slow down"));
    }

    #[test]
    fn extensions_type_maps_each_axis() {
        for (t, want) in [
            ("authentication error", TrackerUnavailable::NotAuthenticated),
            ("ratelimited", TrackerUnavailable::RateLimited),
            ("forbidden", TrackerUnavailable::Forbidden),
            ("network error", TrackerUnavailable::Network),
            ("internal error", TrackerUnavailable::Internal),
            ("invalid input", TrackerUnavailable::InvalidInput),
            ("user error", TrackerUnavailable::InvalidInput),
        ] {
            let body = format!(r#"{{"errors":[{{"extensions":{{"type":"{t}"}}}}]}}"#);
            assert_eq!(fail(classify_graphql(200, &body)).kind, want, "type={t}");
        }
    }

    /// extensions.type 없으면 HTTP 상태로 폴백.
    #[test]
    fn missing_extensions_type_falls_back_to_status() {
        let body = r#"{"errors":[{"message":"boom"}]}"#;
        assert_eq!(
            fail(classify_graphql(403, body)).kind,
            TrackerUnavailable::Forbidden
        );
        assert_eq!(
            fail(classify_graphql(429, body)).kind,
            TrackerUnavailable::RateLimited
        );
        assert_eq!(
            fail(classify_graphql(401, body)).kind,
            TrackerUnavailable::NotAuthenticated
        );
        assert_eq!(
            fail(classify_graphql(503, body)).kind,
            TrackerUnavailable::Network
        );
    }

    /// **crux (b)**: 200 + errors 없음 + data:null → Unknown, 절대 None/empty 아님.
    #[test]
    fn http_200_data_null_is_unknown_not_none() {
        let c = fail(classify_graphql(200, r#"{"data":null}"#));
        assert_eq!(c.kind, TrackerUnavailable::Unknown);
    }

    /// **"키 부재 ≠ 성공" 함정**: `errors: []`(빈 배열, 존재)는 성공 아님.
    #[test]
    fn empty_errors_array_is_not_success() {
        // data가 있어도 errors 키가 존재하면 성공 아님.
        let c = fail(classify_graphql(200, r#"{"data":{"x":1},"errors":[]}"#));
        // 빈 배열 → 상태 폴백(200) → Unknown.
        assert_eq!(c.kind, TrackerUnavailable::Unknown);
    }

    /// **crux (e)**: raw `errors[0].message`는 분류 결과 어디에도 안 샌다. userPresentableMessage만.
    #[test]
    fn raw_error_message_never_leaks_into_classified() {
        let body = r#"{"errors":[{"message":"SECRET query internals AbCdEf",
            "extensions":{"type":"forbidden","userPresentableMessage":"You lack access"}}]}"#;
        let c = fail(classify_graphql(200, body));
        let rendered = format!("{c:?}");
        assert!(
            !rendered.contains("SECRET query internals"),
            "raw message leaked into classified output: {rendered}"
        );
        assert_eq!(c.user_message.as_deref(), Some("You lack access"));
    }

    /// 비-2xx인데 바디가 JSON도 아니면 상태로 분류(성공/None 아님).
    #[test]
    fn non_2xx_unparseable_body_classifies_by_status() {
        assert_eq!(
            fail(classify_graphql(401, "not json")).kind,
            TrackerUnavailable::NotAuthenticated
        );
        assert_eq!(
            fail(classify_graphql(500, "<html>oops</html>")).kind,
            TrackerUnavailable::Internal
        );
    }

    /// 비-2xx + errors 없는 정상 JSON도 상태로 분류(성공 아님).
    #[test]
    fn non_2xx_without_errors_is_classified_not_success() {
        let c = fail(classify_graphql(503, r#"{"data":{"viewer":{"id":"u1"}}}"#));
        assert_eq!(c.kind, TrackerUnavailable::Network);
    }
}
