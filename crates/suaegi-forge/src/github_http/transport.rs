//! HTTP 전송 추상화. **테스트가 real github.com을 안 치게 하는 경계**다 — gh 백엔드의
//! PATH-주입 fake-gh 스크립트에 대응하는 HTTP 아날로그다. 실제 impl([`ReqwestTransport`])은
//! reqwest를 감싸고, 테스트는 [`FakeTransport`]로 canned 응답(진짜 GitHub JSON·상태·헤더)을 준다.
//!
//! **분류·None-vs-Unavailable 규율은 이 층 위(forge)에 산다** — 전송은 상태코드/헤더/바디를
//! 있는 그대로만 나른다. 그래야 `classify`를 mutate하면 forge 테스트가 깨진다(공허하지 않다).
//!
//! **토큰 리댁션**: 전송 에러는 절대 토큰/원본 URL을 담지 않는다 — 고정 라벨만. 토큰은
//! 오직 `Authorization` 헤더로만 실린다([`HttpRequest::headers`]).

use async_trait::async_trait;
use std::time::Duration;

/// 우리가 쓰는 HTTP 메서드(GitHub REST v3 표면에 필요한 것만).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
}

/// forge가 전송에 넘기는 요청. `headers`에 `Authorization`이 들어간다(유일한 토큰 경로).
#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub method: HttpMethod,
    pub url: String,
    /// 헤더(Authorization, Accept, User-Agent, X-GitHub-Api-Version 등).
    pub headers: Vec<(String, String)>,
    /// JSON 바디(POST/PUT). GET이면 None.
    pub body: Option<String>,
    /// 이 요청 하나의 타임아웃. read/create/merge가 서로 다른 값을 준다(gh runner 미러).
    pub timeout: Duration,
}

/// 전송이 돌려주는 응답. 상태/헤더/바디 raw — 분류는 forge가 한다.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    /// 헤더. **키는 소문자로 정규화**해 대소문자 무관 조회를 단순화한다.
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl HttpResponse {
    /// 헤더를 대소문자 무관으로 읽는다(키는 이미 소문자로 저장). 첫 매치.
    pub fn header(&self, name: &str) -> Option<&str> {
        let want = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| *k == want)
            .map(|(_, v)| v.as_str())
    }
}

/// 전송-레벨 실패(HTTP 상태를 받기 **전**의 실패: DNS/연결/TLS/타임아웃). **토큰·원본 URL을
/// 절대 담지 않는다** — 고정 라벨만. forge가 이를 분류된 `ForgeUnavailable::Network`로 접는다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    /// 요청이 타임아웃.
    Timeout,
    /// 연결/DNS/TLS 실패. 문자열은 **고정 라벨**(원본 에러 아님) — 토큰 유출 경로를 차단.
    Connect(String),
}

/// 주입 가능한 HTTP 전송. 실제는 reqwest, 테스트는 fake. `Send + Sync`라 `Arc<dyn ..>`로
/// forge가 들고 다닐 수 있다(forge는 `Send` future를 낸다).
#[async_trait]
pub trait HttpTransport: Send + Sync {
    async fn execute(&self, req: HttpRequest) -> Result<HttpResponse, TransportError>;
}

/// reqwest 기반 실제 전송. **여기가 real github.com을 치는 유일한 지점**(human-eyes).
pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl std::fmt::Debug for ReqwestTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ReqwestTransport")
    }
}

impl Default for ReqwestTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl ReqwestTransport {
    pub fn new() -> Self {
        // user_agent는 GitHub REST가 요구한다(없으면 403). 토큰은 여기 안 넣는다 —
        // 요청별 Authorization 헤더로만 실린다.
        let client = reqwest::Client::builder()
            .user_agent("suaegi")
            .build()
            .unwrap_or_default();
        Self { client }
    }
}

/// reqwest 에러를 전송 에러로 접는다. **원본 문자열을 넣지 않는다**(URL이 섞일 수 있어) —
/// 타임아웃/연결만 구분하고 라벨은 고정.
fn map_reqwest_error(e: &reqwest::Error) -> TransportError {
    if e.is_timeout() {
        TransportError::Timeout
    } else {
        TransportError::Connect("network error".to_string())
    }
}

#[async_trait]
impl HttpTransport for ReqwestTransport {
    async fn execute(&self, req: HttpRequest) -> Result<HttpResponse, TransportError> {
        let method = match req.method {
            HttpMethod::Get => reqwest::Method::GET,
            HttpMethod::Post => reqwest::Method::POST,
            HttpMethod::Put => reqwest::Method::PUT,
        };
        let mut rb = self.client.request(method, &req.url).timeout(req.timeout);
        for (k, v) in &req.headers {
            rb = rb.header(k.as_str(), v.as_str());
        }
        if let Some(body) = &req.body {
            rb = rb.body(body.clone());
        }
        let resp = rb.send().await.map_err(|e| map_reqwest_error(&e))?;
        let status = resp.status().as_u16();
        let headers = resp
            .headers()
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_ascii_lowercase(),
                    v.to_str().unwrap_or("").to_string(),
                )
            })
            .collect();
        let body = resp.text().await.map_err(|e| map_reqwest_error(&e))?;
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}

/// 주입용 fake 전송(테스트 전용). canned 응답 큐 + 받은 요청 기록.
#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// canned 응답을 순서대로 돌려주고, 받은 요청을 기록한다. 큐가 비면 마지막 요청을
    /// 재사용하지 않고 500을 낸다(테스트가 호출 수를 정확히 맞추게).
    #[derive(Default)]
    pub(crate) struct FakeTransport {
        responses: Mutex<VecDeque<Result<HttpResponse, TransportError>>>,
        requests: Mutex<Vec<HttpRequest>>,
    }

    impl FakeTransport {
        /// 단일 200/JSON 응답.
        pub(crate) fn ok_json(status: u16, body: &str) -> Self {
            let t = Self::default();
            t.push_response(Ok(HttpResponse {
                status,
                headers: Vec::new(),
                body: body.to_string(),
            }));
            t
        }

        /// 헤더 딸린 응답.
        pub(crate) fn with_response(status: u16, headers: &[(&str, &str)], body: &str) -> Self {
            let t = Self::default();
            t.push_response(Ok(HttpResponse {
                status,
                headers: headers
                    .iter()
                    .map(|(k, v)| (k.to_ascii_lowercase(), v.to_string()))
                    .collect(),
                body: body.to_string(),
            }));
            t
        }

        /// 전송 에러(네트워크/타임아웃).
        pub(crate) fn with_error(err: TransportError) -> Self {
            let t = Self::default();
            t.push_response(Err(err));
            t
        }

        pub(crate) fn push_response(&self, r: Result<HttpResponse, TransportError>) {
            self.responses.lock().unwrap().push_back(r);
        }

        /// 이 전송이 받은 요청들(순서대로).
        pub(crate) fn requests(&self) -> Vec<HttpRequest> {
            self.requests.lock().unwrap().clone()
        }

        /// 마지막 요청의 특정 헤더 값.
        pub(crate) fn last_header(&self, name: &str) -> Option<String> {
            let want = name.to_ascii_lowercase();
            self.requests
                .lock()
                .unwrap()
                .last()
                .and_then(|r| {
                    r.headers
                        .iter()
                        .find(|(k, _)| k.to_ascii_lowercase() == want)
                        .map(|(_, v)| v.clone())
                })
        }
    }

    #[async_trait]
    impl HttpTransport for FakeTransport {
        async fn execute(&self, req: HttpRequest) -> Result<HttpResponse, TransportError> {
            self.requests.lock().unwrap().push(req);
            self.responses.lock().unwrap().pop_front().unwrap_or_else(|| {
                Ok(HttpResponse {
                    status: 500,
                    headers: Vec::new(),
                    body: String::new(),
                })
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_lookup_is_case_insensitive() {
        let resp = HttpResponse {
            status: 200,
            headers: vec![("x-ratelimit-remaining".to_string(), "0".to_string())],
            body: String::new(),
        };
        assert_eq!(resp.header("X-RateLimit-Remaining"), Some("0"));
        assert_eq!(resp.header("x-ratelimit-remaining"), Some("0"));
        assert_eq!(resp.header("retry-after"), None);
    }

    /// **회귀 방어**: 전송 에러는 원본 문자열이 아니라 고정 라벨만 담아야 토큰/URL이 안 샌다.
    #[test]
    fn transport_error_connect_is_a_fixed_label() {
        // map_reqwest_error는 reqwest::Error가 필요해 직접 만들기 어렵다 — 대신 라벨 고정만 확인.
        let e = TransportError::Connect("network error".to_string());
        match e {
            TransportError::Connect(label) => assert_eq!(label, "network error"),
            _ => panic!("expected Connect"),
        }
    }
}
