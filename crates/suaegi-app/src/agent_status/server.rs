//! 루프백 훅 수신 서버. **판정은 전부 [`parse`](crate::agent_status::parse)에 있고**
//! 여기 남은 것은 진짜 IO가 필요한 것뿐이다 — 바인딩, slowloris 타임아웃, 유계 송신.
//!
//! **하이퍼/axum을 쓰지 않는다.** 루프백 전용에 라우트가 하나뿐이라 프레임워크가
//! 사줄 것이 없고 공격 표면만 넓어진다.
//!
//! **서버는 어떤 요청에도 죽지 않는다.** 연결 하나의 실패는 그 연결의 응답일 뿐이고,
//! `accept` 실패도 루프를 끝내지 않는다 — 훅 서버가 멎어도 사용자의 에이전트는 계속
//! 돌아야 한다는 것이 이 플랜의 절대 규칙이다.

use std::io::{BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::channel::mpsc;

use crate::agent_status::contract::{HookEvent, HOOK_QUEUE};
use crate::agent_status::parse::{
    parse_head, parse_hook, route, Status, HEADER_NONCE, HEADER_PANE, MAX_BODY,
};

/// 요청 하나를 받는 데 허용하는 시간. slowloris(느리게 흘려 연결을 붙잡아 두는 공격)를
/// 막는다. 훅 스크립트는 `curl --max-time 1.5`라 정상 요청은 한참 안쪽이다.
pub const RECV_TIMEOUT: Duration = Duration::from_secs(5);

/// 요청 머리의 상한. 본문과 달리 머리는 작아야 한다 — 무한히 긴 헤더로 메모리를
/// 밀어 넣는 길을 막는다.
const MAX_HEAD: usize = 16 * 1024;

/// 살아 있는 서버의 핸들. **앱 수명 내내 하나**이고 포트·토큰은 부팅 시 정해져
/// 바뀌지 않는다 — 세션 재시작은 같은 값을 다시 심는다.
pub struct HookServer {
    addr: SocketAddr,
    token: String,
    dropped: Arc<AtomicU64>,
}

impl HookServer {
    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    /// 유계 채널이 가득 차서 **버린** 이벤트 수.
    ///
    /// **드러내는 이유**: drop-newest는 조용히 틀린 배지를 만들 수 있다. 큐가
    /// `Working`으로 차 있는 동안 마지막 `PermissionRequest`나 `Stop`이 버려지면
    /// 폴링은 계속 `Agent`를 보고, 잃어버린 `Waiting`은 재구성할 방법이 없다.
    /// "다음 이벤트가 곧 고친다"가 **항상 참은 아니다** — 그래서 센다.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// 루프백 임시 포트에 바인딩하고 수신 스레드를 띄운다.
///
/// **`boot()`보다 먼저 부른다.** 세션 스폰이 포트를 알아야 하기 때문이다. 실패하면
/// 배지 없이 계속 간다 — 치명적이지 않다.
pub fn bind(token: String) -> std::io::Result<(HookServer, mpsc::Receiver<HookEvent>)> {
    // `127.0.0.1`에만 붙인다. `0.0.0.0`이면 같은 네트워크의 누구나 훅을 위조할 수 있다.
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))?;
    let addr = listener.local_addr()?;
    let dropped = Arc::new(AtomicU64::new(0));

    let (tx, rx) = mpsc::channel(HOOK_QUEUE);
    let server = HookServer {
        addr,
        token: token.clone(),
        dropped: Arc::clone(&dropped),
    };

    std::thread::Builder::new()
        .name("suaegi-hook-server".into())
        .spawn(move || serve(listener, token, tx, dropped))?;

    Ok((server, rx))
}

fn serve(
    listener: TcpListener,
    token: String,
    mut tx: mpsc::Sender<HookEvent>,
    dropped: Arc<AtomicU64>,
) {
    for incoming in listener.incoming() {
        // `accept` 실패는 루프를 끝내지 않는다. 파일 디스크립터 고갈 같은 일시적
        // 조건에서 서버가 영영 죽으면 그 뒤 모든 배지가 멎는다.
        let Ok(stream) = incoming else { continue };
        let status = handle(&stream, &token, &mut tx, &dropped);
        // 응답 실패는 무시한다 — `curl --max-time 1.5`가 이미 끊었을 수 있고,
        // 그건 우리 문제가 아니다.
        let _ = respond(&stream, status);
    }
}

/// 연결 하나. **어떤 경로로도 패닉하지 않는다** — 돌려주는 것은 항상 상태 코드다.
fn handle(
    stream: &TcpStream,
    token: &str,
    tx: &mut mpsc::Sender<HookEvent>,
    dropped: &AtomicU64,
) -> Status {
    if stream.set_read_timeout(Some(RECV_TIMEOUT)).is_err() {
        return Status::RequestTimeout;
    }
    let mut reader = BufReader::new(stream);

    let (head_bytes, leftover) = match read_head(&mut reader) {
        Ok(v) => v,
        Err(status) => return status,
    };
    let head = match parse_head(&head_bytes) {
        Ok(h) => h,
        Err(status) => return status,
    };
    // 토큰 → 경로 → 메서드. 순서가 계약이다(`route`의 주석 참고).
    if let Err(status) = route(&head, token) {
        return status;
    }

    let body = match read_body(&mut reader, leftover, head.content_length) {
        Ok(b) => b,
        Err(status) => return status,
    };

    let (Some(pane), Some(nonce)) = (head.header(HEADER_PANE), head.header(HEADER_NONCE)) else {
        return Status::BadRequest;
    };
    let event = match parse_hook(pane, nonce, &body) {
        Ok(e) => e,
        Err(err) => return Status::from(err),
    };

    // **drop-newest다.** `futures::mpsc`는 가득 차면 새 전송을 거절하고, 보내는 쪽이
    // 가장 오래된 것을 꺼낼 수 없다. 재시도도 블로킹도 하지 않는다 — 훅은 턴을
    // 잡고 있으므로 여기서 기다리면 사용자의 에이전트가 멎는다.
    if tx.try_send(event).is_err() {
        dropped.fetch_add(1, Ordering::Relaxed);
    }
    // **버려도 204다.** 훅 스크립트에게 알릴 수 있는 것이 없고, 오류를 주면
    // 사용자 트랜스크립트에 잡음이 뜬다.
    Status::Ok
}

/// `\r\n\r\n`까지 읽는다. 함께 읽힌 본문 앞부분은 두 번째 값으로 돌려준다 —
/// 버리면 `Content-Length`만큼 다시 못 채운다.
fn read_head(reader: &mut BufReader<&TcpStream>) -> Result<(Vec<u8>, Vec<u8>), Status> {
    let mut buf = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];
    loop {
        match reader.read(&mut byte) {
            Ok(0) => return Err(Status::BadRequest), // 머리가 끝나기 전에 끊겼다
            Ok(_) => buf.push(byte[0]),
            // 타임아웃과 그 밖의 IO 오류를 가른다: 전자만 408이다.
            Err(e) => return Err(io_status(&e)),
        }
        if buf.len() >= 4 && buf[buf.len() - 4..] == *b"\r\n\r\n" {
            return Ok((buf, Vec::new()));
        }
        if buf.len() > MAX_HEAD {
            return Err(Status::PayloadTooLarge);
        }
    }
}

fn read_body(
    reader: &mut BufReader<&TcpStream>,
    mut body: Vec<u8>,
    content_length: usize,
) -> Result<Vec<u8>, Status> {
    // `parse_head`가 이미 상한을 봤지만 여기서도 본다 — 이 함수만 따로 불릴 수 있고,
    // 상한 검사가 한 곳에만 있으면 그 한 곳이 옮겨질 때 조용히 사라진다.
    if content_length > MAX_BODY {
        return Err(Status::PayloadTooLarge);
    }
    body.reserve(content_length.saturating_sub(body.len()));
    let mut chunk = [0u8; 8192];
    while body.len() < content_length {
        let want = (content_length - body.len()).min(chunk.len());
        match reader.read(&mut chunk[..want]) {
            Ok(0) => return Err(Status::BadRequest), // 예고한 길이보다 짧게 끊겼다
            Ok(n) => body.extend_from_slice(&chunk[..n]),
            Err(e) => return Err(io_status(&e)),
        }
    }
    Ok(body)
}

/// 읽기 타임아웃만 408이다. 플랫폼마다 `WouldBlock`/`TimedOut`으로 갈려서 둘 다 본다.
fn io_status(e: &std::io::Error) -> Status {
    match e.kind() {
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => Status::RequestTimeout,
        _ => Status::BadRequest,
    }
}

fn respond(mut stream: &TcpStream, status: Status) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 {} {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        status.code(),
        status.reason()
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    /// 서버에 요청 한 방을 보내고 상태 코드를 돌려준다.
    fn request(port: u16, raw: &[u8]) -> u16 {
        let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        s.write_all(raw).expect("write");
        s.flush().ok();
        let mut response = String::new();
        s.set_read_timeout(Some(Duration::from_secs(10))).ok();
        s.read_to_string(&mut response).expect("read");
        response
            .split_whitespace()
            .nth(1)
            .and_then(|c| c.parse().ok())
            .unwrap_or(0)
    }

    fn post(pane: &str, nonce: &str, token: &str, body: &str) -> Vec<u8> {
        format!(
            "POST /hook/claude HTTP/1.1\r\nX-Suaegi-Token: {token}\r\n\
             X-Suaegi-Pane: {pane}\r\nX-Suaegi-Nonce: {nonce}\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    const BODY: &str = r#"{"session_id":"s1","hook_event_name":"Stop","background_tasks":[]}"#;

    #[test]
    fn accepts_a_well_formed_hook_and_delivers_it() {
        let (server, mut rx) = bind("tok".into()).expect("bind");
        let pane =
            crate::agent_status::parse::encode_pane_key(&crate::agent_status::contract::PaneKey(
                suaegi_core::domain::WorktreeId("/tmp/ws/demo".into()),
            ));
        assert_eq!(request(server.port(), &post(&pane, "42", "tok", BODY)), 204);

        let event = futures::executor::block_on(rx.next()).expect("이벤트가 전달돼야 한다");
        assert_eq!(event.claude_session_id, "s1");
        assert_eq!(event.spawn_nonce.0, 42);
        assert_eq!(event.pane_key.0 .0, "/tmp/ws/demo");
        assert_eq!(server.dropped(), 0, "여유가 있는데 버렸다");
    }

    /// 검증 표를 그대로 옮긴다. **대조군(204)이 같은 표에 있어야** 각 행이
    /// "무엇 때문에" 거절됐는지가 고정된다.
    #[test]
    fn validation_table_holds() {
        let (server, _rx) = bind("tok".into()).expect("bind");
        let p = server.port();
        let pane = "L3RtcC93cy9kZW1v"; // base64url("/tmp/ws/demo"), 패딩 없음

        // 대조군 — 이 표의 다른 행들은 여기서 한 가지씩만 달라진다.
        assert_eq!(request(p, &post(pane, "1", "tok", BODY)), 204);

        // **길이가 같고 내용만 다른** 토큰이어야 한다. 길이가 다르면 비교가
        // 길이만 보고 있어도 통과해 버려서, 이 행이 "내용을 본다"를 증명하지 못한다
        // (mutation으로 실제로 확인했다 — 길이만 보는 구현이 이 표를 통과했다).
        assert_eq!(request(p, &post(pane, "1", "tok", BODY)), 204, "대조군");
        assert_eq!(
            request(p, &post(pane, "1", "toz", BODY)),
            403,
            "같은 길이, 틀린 토큰"
        );
        assert_eq!(
            request(p, &post(pane, "1", "wrong", BODY)),
            403,
            "다른 길이, 틀린 토큰"
        );
        assert_eq!(
            request(
                p,
                b"POST /hook/claude HTTP/1.1\r\nContent-Length: 0\r\n\r\n"
            ),
            403,
            "토큰 헤더 없음"
        );
        assert_eq!(request(p, &post("", "1", "tok", BODY)), 400, "빈 pane");
        assert_eq!(
            request(p, &post("!!!", "1", "tok", BODY)),
            400,
            "pane 디코딩 실패"
        );
        assert_eq!(
            request(p, &post(pane, "abc", "tok", BODY)),
            400,
            "nonce 파싱 실패"
        );
        assert_eq!(
            request(p, &post(pane, "1", "tok", "not json")),
            400,
            "JSON 아님"
        );
        assert_eq!(
            request(p, &post(pane, "1", "tok", r#"{"hook_event_name":"Stop"}"#)),
            400,
            "session_id 없음"
        );

        // pane/nonce 헤더가 통째로 빠진 경우
        let missing = format!(
            "POST /hook/claude HTTP/1.1\r\nX-Suaegi-Token: tok\r\nContent-Length: {}\r\n\r\n{BODY}",
            BODY.len()
        );
        assert_eq!(request(p, missing.as_bytes()), 400, "pane/nonce 헤더 없음");

        // 경로와 메서드
        let wrong_path = "POST /nope HTTP/1.1\r\nX-Suaegi-Token: tok\r\nContent-Length: 0\r\n\r\n";
        assert_eq!(request(p, wrong_path.as_bytes()), 404);
        let wrong_method =
            "GET /hook/claude HTTP/1.1\r\nX-Suaegi-Token: tok\r\nContent-Length: 0\r\n\r\n";
        assert_eq!(request(p, wrong_method.as_bytes()), 405);

        // 본문 상한
        let too_big = format!(
            "POST /hook/claude HTTP/1.1\r\nX-Suaegi-Token: tok\r\nContent-Length: {}\r\n\r\n",
            MAX_BODY + 1
        );
        assert_eq!(request(p, too_big.as_bytes()), 413);
    }

    /// **서버는 어떤 요청에도 죽지 않는다.** 위 표를 다 맞은 뒤에도 정상 요청이
    /// 통해야 그 말이 참이다.
    #[test]
    fn server_survives_every_rejected_request() {
        let (server, mut rx) = bind("tok".into()).expect("bind");
        let p = server.port();
        let pane = "L3RtcC93cy9kZW1v";

        for bad in [
            post("", "1", "tok", BODY),
            post(pane, "x", "tok", BODY),
            post(pane, "1", "nope", BODY),
            post(pane, "1", "tok", "{"),
            b"GARBAGE\r\n\r\n".to_vec(),
            b"POST /hook/claude HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec(),
        ] {
            let _ = request(p, &bad);
        }

        assert_eq!(
            request(p, &post(pane, "9", "tok", BODY)),
            204,
            "거절 여섯 번 뒤에 서버가 죽었다"
        );
        let event = futures::executor::block_on(rx.next()).expect("정상 요청은 여전히 전달된다");
        assert_eq!(event.spawn_nonce.0, 9);
    }

    /// **drop-newest다.** 큐를 채우고 나면 새 이벤트가 거절되고, 카운터가 오르며,
    /// **기존 것은 남아 있다**. 대조군은 위 `accepts_...`(여유가 있으면 들어간다).
    #[test]
    fn full_queue_drops_the_newest_and_counts_it() {
        let (server, mut rx) = bind("tok".into()).expect("bind");
        let p = server.port();
        let pane = "L3RtcC93cy9kZW1v";

        // 용량은 `buffer + 송신자 수`라 정확히 HOOK_QUEUE가 아니다 — 넉넉히 넘긴다.
        let overshoot = HOOK_QUEUE + 16;
        for i in 0..overshoot {
            let body = format!(
                r#"{{"session_id":"s{i}","hook_event_name":"Stop","background_tasks":[]}}"#
            );
            assert_eq!(
                request(p, &post(pane, &i.to_string(), "tok", &body)),
                204,
                "버려도 204여야 한다 — 훅에게 알릴 것이 없다"
            );
        }

        assert!(
            server.dropped() > 0,
            "큐를 {overshoot}개로 넘겼는데 버린 것이 0이다 — 유계가 아니거나 카운터가 안 는다"
        );

        // **가장 오래된 것이 살아 있어야 한다.** drop-oldest였다면 첫 이벤트가 없다.
        let first = futures::executor::block_on(rx.next()).expect("첫 이벤트");
        assert_eq!(
            first.claude_session_id, "s0",
            "가장 오래된 이벤트가 사라졌다 — drop-newest가 아니라 drop-oldest다"
        );
    }

    /// slowloris: 머리를 끝내지 않고 붙잡고 있으면 408로 끊는다. 5초를 실제로
    /// 기다리므로 느린 테스트다.
    #[test]
    fn slow_request_is_timed_out() {
        let (server, _rx) = bind("tok".into()).expect("bind");
        let mut s = TcpStream::connect(("127.0.0.1", server.port())).expect("connect");
        // 머리를 **끝내지 않는다**(`\r\n\r\n` 없음).
        s.write_all(b"POST /hook/claude HTTP/1.1\r\nX-Suaegi-Token: tok\r\n")
            .expect("write");
        s.flush().ok();

        let started = std::time::Instant::now();
        let mut response = String::new();
        s.set_read_timeout(Some(RECV_TIMEOUT * 3)).ok();
        s.read_to_string(&mut response).expect("read");
        let code: u16 = response
            .split_whitespace()
            .nth(1)
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);

        assert_eq!(code, 408, "느린 요청을 끊지 않았다: {response:?}");
        assert!(
            started.elapsed() >= RECV_TIMEOUT.saturating_sub(Duration::from_millis(500)),
            "타임아웃을 기다리지 않고 즉시 끊었다 — 정상 요청도 끊길 수 있다"
        );
    }

    /// 본문이 `Content-Length`보다 짧게 끊기면 400이다. 여기서 무한정 기다리면
    /// 연결 하나가 스레드를 5초씩 잡는다.
    #[test]
    fn truncated_body_is_rejected() {
        let (server, _rx) = bind("tok".into()).expect("bind");
        let mut s = TcpStream::connect(("127.0.0.1", server.port())).expect("connect");
        s.write_all(
            b"POST /hook/claude HTTP/1.1\r\nX-Suaegi-Token: tok\r\n\
              X-Suaegi-Pane: L3RtcC93cy9kZW1v\r\nX-Suaegi-Nonce: 1\r\n\
              Content-Length: 500\r\n\r\n{\"session_id\"",
        )
        .expect("write");
        s.shutdown(std::net::Shutdown::Write).ok();

        let mut response = String::new();
        s.set_read_timeout(Some(Duration::from_secs(10))).ok();
        s.read_to_string(&mut response).expect("read");
        assert!(response.starts_with("HTTP/1.1 400"), "got {response:?}");
    }
}
