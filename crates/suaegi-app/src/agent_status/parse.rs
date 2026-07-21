//! 훅 요청의 **순수** 정규화. 소켓도 `iced`도 모른다.
//!
//! 두 층으로 나뉜다:
//! - [`parse_head`] → [`route`] — 바이트 → [`RequestHead`] → 라우팅·인증(HTTP 한 겹).
//!   실패는 그대로 상태 코드다.
//! - [`parse_hook`] — 헤더 둘 + 본문 → [`HookEvent`]. 와이어 포맷을 모른다.
//!
//! **둘 다 순수한 것이 요구사항이다.** 검증 표(Task 2)를 표 테스트로 그대로 옮길 수
//! 있는 유일한 형태이고, slowloris·타임아웃처럼 진짜 IO가 필요한 것만 `server.rs`에 남는다.
//!
//! **관대하게 파싱하지 않는다.** 기대한 모양과 정확히 맞지 않으면 거절한다 — 루프백
//! 전용에 라우트 하나뿐이라 관용이 사줄 것이 없고, 공격 표면만 넓힌다.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use suaegi_core::domain::WorktreeId;

use crate::agent_status::contract::{HookEvent, HookEventName, PaneKey, SpawnNonce};

/// 본문 상한. 넘으면 413이고 **읽기를 멈춘다** — 다 읽고 나서 재는 것은 이미 늦다.
pub const MAX_BODY: usize = 1024 * 1024;

pub const HEADER_TOKEN: &str = "x-suaegi-token";
pub const HEADER_PANE: &str = "x-suaegi-pane";
pub const HEADER_NONCE: &str = "x-suaegi-nonce";

/// 검증 표의 응답들. **서버는 어떤 경우에도 계속 산다** — 이 값은 이 요청 하나의 운명이다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok = 204,
    BadRequest = 400,
    Forbidden = 403,
    NotFound = 404,
    MethodNotAllowed = 405,
    RequestTimeout = 408,
    PayloadTooLarge = 413,
    /// 동시 연결 상한을 넘었다. **즉시** 돌려주고 끊는다 — 여기서 기다리면
    /// 상한이 지키려던 것을 그대로 내주는 셈이다.
    ServiceUnavailable = 503,
}

impl Status {
    pub fn code(self) -> u16 {
        self as u16
    }

    /// 상태줄 문구. 임의 문자열이 아니라 고정 표다.
    pub fn reason(self) -> &'static str {
        match self {
            Status::Ok => "No Content",
            Status::BadRequest => "Bad Request",
            Status::Forbidden => "Forbidden",
            Status::NotFound => "Not Found",
            Status::MethodNotAllowed => "Method Not Allowed",
            Status::RequestTimeout => "Request Timeout",
            Status::PayloadTooLarge => "Payload Too Large",
            Status::ServiceUnavailable => "Service Unavailable",
        }
    }
}

/// 요청 **머리**(상태줄 + 헤더)를 파싱한 결과. 본문은 `Content-Length`만큼 따로 읽으므로
/// 여기서는 길이만 담는다 — 그래야 상한 초과를 **읽기 전에** 알 수 있다.
///
/// 헤더 이름은 **소문자로 정규화**해 담는다(HTTP 헤더는 대소문자 구분이 없는데
/// `curl`이 무엇을 보낼지에 우리 검증이 의존하면 안 된다).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestHead {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub content_length: usize,
}

impl RequestHead {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// `\r\n\r\n`까지의 바이트를 받아 머리를 판정한다.
///
/// **`Content-Length`가 없으면 0으로 본다.** 청크 전송은 지원하지 않는다 — 우리 훅
/// 스크립트는 `curl --data-binary @-`로 항상 길이를 보낸다. `Transfer-Encoding`이 오면
/// 거절한다(관대하게 받아주면 본문 상한을 우회하는 길이 생긴다).
pub fn parse_head(bytes: &[u8]) -> Result<RequestHead, Status> {
    let text = std::str::from_utf8(bytes).map_err(|_| Status::BadRequest)?;
    let mut lines = text.split("\r\n");

    let request_line = lines.next().ok_or(Status::BadRequest)?;
    let mut parts = request_line.split(' ');
    let (Some(method), Some(path), Some(version)) = (parts.next(), parts.next(), parts.next())
    else {
        return Err(Status::BadRequest);
    };
    if parts.next().is_some() || !version.starts_with("HTTP/1.") {
        return Err(Status::BadRequest);
    }

    let mut headers = Vec::new();
    let mut content_length = 0usize;
    for line in lines {
        if line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(':').ok_or(Status::BadRequest)?;
        // 헤더 이름 뒤 공백은 HTTP에서 금지다(요청 스머글링 벡터).
        if name.is_empty() || name.ends_with(' ') {
            return Err(Status::BadRequest);
        }
        let name = name.to_ascii_lowercase();
        let value = value.trim().to_string();
        if name == "transfer-encoding" {
            return Err(Status::BadRequest);
        }
        if name == "content-length" {
            // 중복 `Content-Length`는 스머글링의 고전이다. 하나만 허용한다.
            if content_length != 0 || headers.iter().any(|(k, _): &(String, String)| k == &name) {
                return Err(Status::BadRequest);
            }
            content_length = value.parse::<usize>().map_err(|_| Status::BadRequest)?;
            if content_length > MAX_BODY {
                return Err(Status::PayloadTooLarge);
            }
        }
        headers.push((name, value));
    }

    Ok(RequestHead {
        method: method.to_string(),
        path: path.to_string(),
        headers,
        content_length,
    })
}

/// 라우팅과 인증. **토큰을 경로보다 먼저 본다** — 토큰 없는 상대에게 어떤 경로가
/// 존재하는지 알려줄 이유가 없다.
///
/// `<source>`는 에이전트 종류다(`/hook/claude`). Codex가 붙을 자리이므로 경로 세그먼트로
/// 남겨두되, **지금 아는 것만** 받는다.
pub fn route(head: &RequestHead, expected_token: &str) -> Result<HookSource, Status> {
    match head.header(HEADER_TOKEN) {
        Some(token) if constant_time_eq(token.as_bytes(), expected_token.as_bytes()) => {}
        _ => return Err(Status::Forbidden),
    }
    let Some(source) = head.path.strip_prefix("/hook/") else {
        return Err(Status::NotFound);
    };
    // 경로를 먼저 알아본 **뒤에** 메서드를 따진다 — 405는 "이 경로는 있다"는 뜻이라
    // 없는 경로에 405를 주면 존재를 흘린다.
    let source = match source {
        "claude" => HookSource::Claude,
        _ => return Err(Status::NotFound),
    };
    if head.method != "POST" {
        return Err(Status::MethodNotAllowed);
    }
    Ok(source)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookSource {
    Claude,
}

/// 토큰 비교는 길이·내용 모두 상수 시간으로. 루프백이라 위협 모델이 얕지만, 같은
/// 기계의 다른 사용자가 타이밍으로 토큰을 복원하는 것을 막는 비용이 이 정도다.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// pane 헤더가 없거나 비었거나 base64url로 디코딩되지 않는다.
    Pane,
    /// nonce 헤더가 없거나 `u64`가 아니다.
    Nonce,
    /// 본문이 JSON 객체가 아니다.
    Json,
    /// `session_id`가 없거나 문자열이 아니다.
    SessionId,
    /// `hook_event_name`이 없거나 우리가 등록한 이벤트가 아니다.
    EventName,
}

impl From<ParseError> for Status {
    fn from(_: ParseError) -> Self {
        // 검증 표: pane/nonce/JSON/session_id 실패는 전부 400이다.
        Status::BadRequest
    }
}

/// 헤더 둘 + 본문 → [`HookEvent`].
///
/// **`pane`은 base64url(RFC 4648, 패딩 없음)이다.** 날것이 아니다 — `PaneKey`는
/// 파일시스템 경로에서 나오고 unix 경로는 개행과 임의 바이트를 담을 수 있어서, 날것으로
/// 헤더에 실으면 헤더 주입이다. 앱이 스폰 시 이미 인코딩된 값을 심고 여기서 엄격히 푼다.
pub fn parse_hook(pane: &str, nonce: &str, body: &[u8]) -> Result<HookEvent, ParseError> {
    if pane.is_empty() {
        return Err(ParseError::Pane);
    }
    let decoded = URL_SAFE_NO_PAD
        .decode(pane.as_bytes())
        .map_err(|_| ParseError::Pane)?;
    let pane_key = String::from_utf8(decoded).map_err(|_| ParseError::Pane)?;
    if pane_key.is_empty() {
        return Err(ParseError::Pane);
    }

    let spawn_nonce = SpawnNonce(nonce.parse::<u64>().map_err(|_| ParseError::Nonce)?);

    let value: serde_json::Value = serde_json::from_slice(body).map_err(|_| ParseError::Json)?;
    let obj = value.as_object().ok_or(ParseError::Json)?;

    let claude_session_id = obj
        .get("session_id")
        .and_then(|v| v.as_str())
        .ok_or(ParseError::SessionId)?
        .to_string();

    let event = obj
        .get("hook_event_name")
        .and_then(|v| v.as_str())
        .and_then(event_name)
        .ok_or(ParseError::EventName)?;

    // **없으면 `None`이다. `Some(true)`가 아니다.** `Stop`에서는 항상 있지만
    // `StopFailure`에는 **구조적으로 없다**(실측 §1.6.2) — 그래서 리듀서가
    // `HookEventName`으로 분기한다. 여기서는 있는 그대로만 옮긴다.
    let background_tasks_empty = obj
        .get("background_tasks")
        .and_then(|v| v.as_array())
        .map(|a| a.is_empty());

    Ok(HookEvent {
        pane_key: PaneKey(WorktreeId(pane_key)),
        spawn_nonce,
        claude_session_id,
        event,
        tool_name: obj
            .get("tool_name")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        // **`Some` = 서브에이전트, `None` = 리드.** 리드 이벤트는 이 키를 아예 갖지 않는다.
        agent_id: obj
            .get("agent_id")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        background_tasks_empty,
    })
}

/// 우리가 **등록한** 이름만 받는다. 모르는 이름은 거절이다 — 훅 스크립트는 우리가
/// 등록한 것만 보내므로, 모르는 이름이 온다는 것은 우리 가정이 깨졌다는 뜻이다.
fn event_name(raw: &str) -> Option<HookEventName> {
    Some(match raw {
        "SessionStart" => HookEventName::SessionStart,
        "UserPromptSubmit" => HookEventName::UserPromptSubmit,
        "PreToolUse" => HookEventName::PreToolUse,
        "PostToolUse" => HookEventName::PostToolUse,
        "PostToolUseFailure" => HookEventName::PostToolUseFailure,
        "PermissionRequest" => HookEventName::PermissionRequest,
        "Stop" => HookEventName::Stop,
        "StopFailure" => HookEventName::StopFailure,
        "SubagentStop" => HookEventName::SubagentStop,
        "SessionEnd" => HookEventName::SessionEnd,
        _ => return None,
    })
}

/// 앱이 스폰 시 `SUAEGI_PANE_KEY`에 심는 값. 서버의 디코딩과 **같은 엔진**을 쓰는 것이
/// 요점이라 두 함수를 같은 파일에 둔다 — 떨어뜨려 두면 패딩 관례가 어긋난다.
pub fn encode_pane_key(key: &PaneKey) -> String {
    URL_SAFE_NO_PAD.encode(key.0 .0.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body_for(event: &str) -> Vec<u8> {
        format!(r#"{{"session_id":"s1","hook_event_name":"{event}"}}"#).into_bytes()
    }

    #[test]
    fn pane_key_round_trips_through_the_header_encoding() {
        // 경로에 들어갈 수 있는 못된 것들 — 공백, 한글, %, 따옴표, **개행**.
        for raw in [
            "/tmp/ws/demo",
            "/tmp/ws/my worktree",
            "/tmp/ws/한글-브랜치",
            "/tmp/ws/100%-done",
            "/tmp/ws/\"quoted\"",
            "/tmp/ws/line\nbreak",
            "/tmp/ws/tab\there",
        ] {
            let key = PaneKey(WorktreeId(raw.to_string()));
            let encoded = encode_pane_key(&key);
            assert!(
                !encoded.contains('\n') && !encoded.contains('\r'),
                "인코딩 결과에 개행이 남으면 헤더 주입이다: {encoded:?}"
            );
            assert!(
                !encoded.contains('='),
                "패딩이 붙으면 엄격 디코딩에서 어긋난다: {encoded:?}"
            );
            let event = parse_hook(&encoded, "7", &body_for("Stop")).expect("round trip");
            assert_eq!(event.pane_key, key, "왕복이 원본을 보존하지 않았다");
        }
    }

    #[test]
    fn pane_header_must_be_valid_unpadded_base64url() {
        // 표준 base64(패딩 있음)를 보내면 거절해야 한다 — 관례가 어긋난 것을
        // 조용히 받아주면 양쪽이 다른 규칙을 쓰게 된다.
        // **길이가 3의 배수가 아닌 입력**이어야 패딩이 생긴다("/tmp/ws/demo"는 12바이트라
        // 패딩이 없다 — 이 단언이 실제로 그 실수를 잡았다).
        let padded = base64::engine::general_purpose::URL_SAFE.encode("/tmp/ws/demo1");
        assert!(padded.contains('='), "픽스처 전제: 패딩이 붙어 있다");
        assert_eq!(
            parse_hook(&padded, "1", &body_for("Stop")),
            Err(ParseError::Pane)
        );

        for bad in ["", "!!!!", "a b", "%%%"] {
            assert_eq!(
                parse_hook(bad, "1", &body_for("Stop")),
                Err(ParseError::Pane),
                "{bad:?}를 받아들였다"
            );
        }
    }

    #[test]
    fn nonce_must_parse_as_u64() {
        let pane = encode_pane_key(&PaneKey(WorktreeId("/w".into())));
        assert_eq!(
            parse_hook(&pane, "9", &body_for("Stop"))
                .unwrap()
                .spawn_nonce,
            SpawnNonce(9)
        );
        for bad in ["", "-1", "abc", "1.5", "18446744073709551616"] {
            assert_eq!(
                parse_hook(&pane, bad, &body_for("Stop")),
                Err(ParseError::Nonce),
                "{bad:?}를 받아들였다"
            );
        }
    }

    #[test]
    fn body_must_be_a_json_object_with_a_session_id_and_known_event() {
        let pane = encode_pane_key(&PaneKey(WorktreeId("/w".into())));
        assert_eq!(parse_hook(&pane, "1", b"not json"), Err(ParseError::Json));
        assert_eq!(parse_hook(&pane, "1", b"[1,2]"), Err(ParseError::Json));
        assert_eq!(
            parse_hook(&pane, "1", br#"{"hook_event_name":"Stop"}"#),
            Err(ParseError::SessionId)
        );
        assert_eq!(
            parse_hook(
                &pane,
                "1",
                br#"{"session_id":"s","hook_event_name":"Nope"}"#
            ),
            Err(ParseError::EventName)
        );
        assert_eq!(
            parse_hook(&pane, "1", br#"{"session_id":"s"}"#),
            Err(ParseError::EventName)
        );
    }

    /// 조사 §1.6의 **실측 페이로드**를 그대로 쓴다.
    #[test]
    fn measured_payloads_map_to_the_right_event_and_fields() {
        let pane = encode_pane_key(&PaneKey(WorktreeId("/tmp/ws/demo".into())));
        let p = |b: &str| parse_hook(&pane, "3", b.as_bytes()).expect("측정된 페이로드");

        // §1.6.6 ① — 서브에이전트가 도는 중의 Stop
        let running = p(
            r#"{"session_id":"s","hook_event_name":"Stop","stop_hook_active":false,
            "background_tasks":[{"id":"a4e0","type":"subagent","status":"running",
            "description":"Read and summarize","agent_type":"general-purpose"}],"session_crons":[]}"#,
        );
        assert_eq!(running.event, HookEventName::Stop);
        assert_eq!(
            running.background_tasks_empty,
            Some(false),
            "비지 않은 background_tasks를 비었다고 읽으면 배지가 일찍 done이 된다"
        );

        // §1.6.6 ② — 진짜 끝
        let done = p(r#"{"session_id":"s","hook_event_name":"Stop","background_tasks":[]}"#);
        assert_eq!(done.background_tasks_empty, Some(true));

        // §1.6.2 — StopFailure엔 background_tasks가 **구조적으로 없다**
        let sf = p(
            r#"{"session_id":"s","hook_event_name":"StopFailure","error":"server_error",
            "last_assistant_message":"API Error: 500"}"#,
        );
        assert_eq!(sf.event, HookEventName::StopFailure);
        assert_eq!(
            sf.background_tasks_empty, None,
            "StopFailure에 없는 필드를 Some으로 합성하면 안 된다"
        );

        // §1.6.3 — PostToolUseFailure
        let ptf = p(
            r#"{"session_id":"s","hook_event_name":"PostToolUseFailure","tool_name":"Bash",
            "error":"Exit code 1","is_interrupt":false,"duration_ms":508}"#,
        );
        assert_eq!(ptf.event, HookEventName::PostToolUseFailure);
        assert_eq!(ptf.tool_name.as_deref(), Some("Bash"));

        // §1.6.4 — 유령 SubagentStop. agent_id가 있으니 서브에이전트다
        let ss = p(r#"{"session_id":"s","hook_event_name":"SubagentStop",
            "agent_id":"a22e0af17822ae8e3","agent_type":"","background_tasks":[]}"#);
        assert_eq!(ss.event, HookEventName::SubagentStop);
        assert_eq!(ss.agent_id.as_deref(), Some("a22e0af17822ae8e3"));
    }

    /// 리드/서브 구별은 **`agent_id` 키의 유무**다(실측 §1.4.2).
    #[test]
    fn lead_events_have_no_agent_id() {
        let pane = encode_pane_key(&PaneKey(WorktreeId("/w".into())));
        let lead = parse_hook(
            &pane,
            "1",
            br#"{"session_id":"s","hook_event_name":"PreToolUse","tool_name":"Bash"}"#,
        )
        .unwrap();
        assert_eq!(lead.agent_id, None, "리드 이벤트에 agent_id가 생겼다");

        let sub = parse_hook(
            &pane,
            "1",
            br#"{"session_id":"s","hook_event_name":"PreToolUse","agent_id":"a1"}"#,
        )
        .unwrap();
        assert_eq!(sub.agent_id.as_deref(), Some("a1"));
    }

    // ---- HTTP 한 겹 ----

    fn head_of(raw: &str) -> Result<RequestHead, Status> {
        parse_head(raw.as_bytes())
    }

    #[test]
    fn parses_a_well_formed_request_head() {
        let head = head_of(
            "POST /hook/claude HTTP/1.1\r\nHost: 127.0.0.1\r\n\
             X-Suaegi-Token: tok\r\nContent-Length: 12\r\n\r\n",
        )
        .expect("정상 요청");
        assert_eq!(head.method, "POST");
        assert_eq!(head.path, "/hook/claude");
        assert_eq!(head.content_length, 12);
        // 헤더 이름은 소문자로 정규화된다 — curl의 대소문자에 의존하지 않는다.
        assert_eq!(head.header(HEADER_TOKEN), Some("tok"));
        assert_eq!(head.header("host"), Some("127.0.0.1"));
    }

    #[test]
    fn rejects_smuggling_shaped_heads() {
        // 청크 전송은 본문 상한을 우회할 수 있으므로 아예 안 받는다.
        assert_eq!(
            head_of("POST /hook/claude HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n"),
            Err(Status::BadRequest)
        );
        // 중복 Content-Length
        assert_eq!(
            head_of("POST /hook/claude HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\n"),
            Err(Status::BadRequest)
        );
        // 헤더 이름 뒤 공백
        assert_eq!(
            head_of("POST /hook/claude HTTP/1.1\r\nFoo : bar\r\n\r\n"),
            Err(Status::BadRequest)
        );
        // 상태줄이 망가진 것들
        for bad in [
            "GARBAGE\r\n\r\n",
            "POST /hook/claude\r\n\r\n",
            "POST /hook/claude HTTP/2.0\r\n\r\n",
            "POST /hook/claude HTTP/1.1 extra\r\n\r\n",
        ] {
            assert_eq!(
                head_of(bad),
                Err(Status::BadRequest),
                "{bad:?}를 받아들였다"
            );
        }
    }

    #[test]
    fn oversized_content_length_is_rejected_before_reading_the_body() {
        let raw = format!(
            "POST /hook/claude HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            MAX_BODY + 1
        );
        assert_eq!(head_of(&raw), Err(Status::PayloadTooLarge));
        // 경계값은 통과한다 — off-by-one으로 정상 요청을 막으면 안 된다.
        let ok = format!("POST /hook/claude HTTP/1.1\r\nContent-Length: {MAX_BODY}\r\n\r\n");
        assert_eq!(head_of(&ok).unwrap().content_length, MAX_BODY);
    }

    #[test]
    fn routing_checks_the_token_before_revealing_whether_a_path_exists() {
        let with = |path: &str, method: &str, token: &str| {
            let raw = format!("{method} {path} HTTP/1.1\r\nX-Suaegi-Token: {token}\r\n\r\n");
            route(&head_of(&raw).unwrap(), "secret")
        };

        assert_eq!(
            with("/hook/claude", "POST", "secret"),
            Ok(HookSource::Claude)
        );

        // 토큰이 틀리면 **경로가 뭐든** 403이다. 404였다면 토큰 없이도 라우트를
        // 열거할 수 있다는 뜻이다.
        assert_eq!(
            with("/hook/claude", "POST", "wrong"),
            Err(Status::Forbidden)
        );
        assert_eq!(with("/nope", "POST", "wrong"), Err(Status::Forbidden));
        assert_eq!(
            route(
                &head_of("POST /hook/claude HTTP/1.1\r\n\r\n").unwrap(),
                "secret"
            ),
            Err(Status::Forbidden),
            "토큰 헤더가 아예 없는 경우"
        );

        // 토큰이 맞을 때에만 경로/메서드를 구별한다.
        assert_eq!(with("/nope", "POST", "secret"), Err(Status::NotFound));
        assert_eq!(with("/hook/codex", "POST", "secret"), Err(Status::NotFound));
        assert_eq!(
            with("/hook/claude", "GET", "secret"),
            Err(Status::MethodNotAllowed)
        );
        // 없는 경로에 잘못된 메서드는 405가 아니라 404다(존재를 흘리지 않는다).
        assert_eq!(with("/nope", "GET", "secret"), Err(Status::NotFound));
    }

    #[test]
    fn token_comparison_rejects_prefixes_and_length_mismatches() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(!constant_time_eq(b"abcd", b"abc"));
        assert!(!constant_time_eq(b"", b"a"));
        assert!(!constant_time_eq(b"abd", b"abc"));
    }
}
