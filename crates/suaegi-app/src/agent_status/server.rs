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
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
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

/// 훅 스크립트의 `curl --max-time`. **서버의 지연 예산은 이 값이다** —
/// 여기를 넘기면 curl이 포기하므로 이벤트가 늦게 오는 게 아니라 **사라진다**.
/// `RECV_TIMEOUT`(5s)은 악의적 연결을 끊는 상한이지 정상 훅의 예산이 아니다.
pub const HOOK_CURL_MAX_TIME: Duration = Duration::from_millis(1500);

/// 동시에 처리하는 연결 수의 상한.
///
/// 루프백 전용에 훅은 턴당 한 줌이라 실사용은 한 자릿수다. 16이면 정상 트래픽이
/// 닿을 일이 없고, 악의적 연결이 스레드를 무한히 만들지도 못한다.
/// **무제한 스폰은 고갈 지점을 옮길 뿐 없애지 않는다.**
const MAX_CONNECTIONS: usize = 16;

/// 살아 있는 서버의 핸들. **앱 수명 내내 하나**이고 포트·토큰은 부팅 시 정해져
/// 바뀌지 않는다 — 세션 재시작은 같은 값을 다시 심는다.
pub struct HookServer {
    addr: SocketAddr,
    token: String,
    dropped: Arc<AtomicU64>,
    refused: Arc<AtomicU64>,
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

    /// [`MAX_CONNECTIONS`]를 넘겨 **받아주지도 못한** 연결 수.
    ///
    /// [`dropped`](Self::dropped)와 **따로 세는 것이 요점이다.** 둘은 원인이
    /// 다르다: `dropped`는 앱이 이벤트를 소비하지 못해 큐가 찬 것이고,
    /// `refused`는 연결이 몰린 것이다. 하나로 합치면 "앱이 느리다"와
    /// "누가 두드린다"를 구별할 수 없어 지표가 진단에 쓸모없어진다.
    ///
    /// **408(타임아웃)은 세지 않는다.** 느린 정상 훅과 악의적 연결을 구별할
    /// 방법이 없어서, 공격 트래픽이 유실 지표를 부풀리면 지표를 못 믿게 된다.
    pub fn refused(&self) -> u64 {
        self.refused.load(Ordering::Relaxed)
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
    let refused = Arc::new(AtomicU64::new(0));
    let in_flight = Arc::new(AtomicUsize::new(0));

    let (tx, rx) = mpsc::channel(HOOK_QUEUE);
    let tx: SharedSender = Arc::new(std::sync::Mutex::new(tx));
    let server = HookServer {
        addr,
        token: token.clone(),
        dropped: Arc::clone(&dropped),
        refused: Arc::clone(&refused),
    };

    std::thread::Builder::new()
        .name("suaegi-hook-server".into())
        .spawn(move || serve(listener, token, tx, dropped, refused, in_flight))?;

    Ok((server, rx))
}

/// 인플라이트 슬롯 하나. **`Drop`으로 반납하는 것이 요점이다** — 핸들러가
/// 패닉해도 슬롯이 새지 않는다. 세는 것을 수동으로 감소시키면 이른 `return`
/// 하나에 상한이 영구히 줄어든다.
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// **송신자를 복제하지 않고 공유한다.** `futures::mpsc`의 실제 용량은
/// `buffer + 송신자 수`다 — 연결마다 `Sender`를 복제하면 **복제본마다 보장된
/// 슬롯이 하나씩 붙어** 상한이 연결 수만큼 늘어난다. 즉 유계 큐가 사실상
/// 무계가 되고, 그건 이 플랜이 명시적으로 막으려던 OOM 경로다.
/// (`full_queue_drops_the_newest_and_counts_it`가 실제로 이 회귀를 잡았다.)
///
/// 락은 `try_send` 한 번 동안만 잡는다 — **IO를 락 안에서 하지 않는다.**
type SharedSender = Arc<std::sync::Mutex<mpsc::Sender<HookEvent>>>;

fn serve(
    listener: TcpListener,
    token: String,
    tx: SharedSender,
    dropped: Arc<AtomicU64>,
    refused: Arc<AtomicU64>,
    in_flight: Arc<AtomicUsize>,
) {
    for incoming in listener.incoming() {
        // `accept` 실패는 루프를 끝내지 않는다. 파일 디스크립터 고갈 같은 일시적
        // 조건에서 서버가 영영 죽으면 그 뒤 모든 배지가 멎는다.
        let Ok(stream) = incoming else { continue };

        // **연결마다 스레드를 띄운다.** 순차 루프로 두면 아무것도 보내지 않는
        // 연결 하나가 `RECV_TIMEOUT`(5초) 동안 서버 전체를 막는다. 막히는 곳이
        // `read_head`라 **토큰 검사보다 앞이고**, 따라서 자격 증명 없이 배지를
        // 통째로 멈출 수 있다. 훅은 `curl --max-time 1.5`라 그 지연은 유실이다.
        //
        // **무제한으로 띄우지 않는다** — 그러면 고갈 지점을 스레드로 옮길 뿐이다.
        // 상한을 넘으면 **즉시** 503으로 끊는다(기다리면 상한이 무의미하다).
        let current = in_flight.fetch_add(1, Ordering::AcqRel);
        let slot = Slot(Arc::clone(&in_flight));
        if current >= MAX_CONNECTIONS {
            refused.fetch_add(1, Ordering::Relaxed);
            let _ = respond(&stream, Status::ServiceUnavailable);
            drop(slot);
            continue;
        }

        let token = token.clone();
        let tx = Arc::clone(&tx);
        let dropped = Arc::clone(&dropped);
        // 스폰 실패도 루프를 끝내지 않는다 — 이 연결만 포기한다.
        let spawned = std::thread::Builder::new()
            .name("suaegi-hook-conn".into())
            .spawn(move || {
                // `slot`을 이 스레드가 소유한다 — 끝나거나 패닉하면 반납된다.
                let _slot = slot;
                let status = handle(&stream, &token, &tx, &dropped);
                // 응답 실패는 무시한다 — `curl --max-time 1.5`가 이미 끊었을 수
                // 있고, 그건 우리 문제가 아니다.
                let _ = respond(&stream, status);
            });
        if spawned.is_err() {
            refused.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// 연결 하나. **어떤 경로로도 패닉하지 않는다** — 돌려주는 것은 항상 상태 코드다.
fn handle(stream: &TcpStream, token: &str, tx: &SharedSender, dropped: &AtomicU64) -> Status {
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
    // 락은 여기서만, `try_send` 한 번 동안만. 뮤텍스가 오염됐어도 패닉하지
    // 않는다 — 이벤트 하나를 버리고 계속 간다(서버는 어떤 경우에도 산다).
    let sent = match tx.lock() {
        Ok(mut sender) => sender.try_send(event).is_ok(),
        Err(_) => false,
    };
    if !sent {
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
    ///
    /// **쓰기·읽기 오류를 흘려보낸다.** 서버가 상한을 넘겨 **읽지 않고** 끊는
    /// 경로(503)에서는 클라이언트에 아직 안 보낸 본문이 남아 있어 커널이 RST를
    /// 보내고, `write_all`/`read_to_string`이 `ConnectionReset`으로 실패할 수
    /// 있다. 그래도 응답 바이트는 대개 먼저 도착하므로, 받은 만큼으로 상태를
    /// 읽는다. (실제 훅도 마찬가지다 — curl이 오류를 보든 503을 보든 스크립트는
    /// 항상 exit 0이고 이벤트는 어느 쪽이든 사라진다.)
    fn request(port: u16, raw: &[u8]) -> u16 {
        let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        let _ = s.write_all(raw);
        s.flush().ok();
        let mut response = Vec::new();
        s.set_read_timeout(Some(Duration::from_secs(10))).ok();
        let _ = s.read_to_end(&mut response);
        String::from_utf8_lossy(&response)
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

    /// **토큰 없는 조용한 연결 하나가 서버 전체를 막으면 안 된다.**
    ///
    /// 이게 실제 위협인 이유: 막히는 지점이 `read_head`라 **토큰 검사보다 앞이다.**
    /// 같은 기계의 아무 프로세스나 자격 증명 없이 배지를 통째로 멈출 수 있고,
    /// 훅 스크립트는 `curl --max-time 1.5`라 1.5초를 넘기면 **포기하고 이벤트가
    /// 사라진다** — 늦게 도착하는 게 아니라 유실이다. 잃어버린 `Stop`이나
    /// `PermissionRequest`는 재구성할 방법이 없다.
    ///
    /// 예산을 `RECV_TIMEOUT`이 아니라 **훅의 `--max-time`**에 맞춘다: 서버가
    /// 5초 안에 응답해도 훅은 이미 죽어 있다.
    #[test]
    fn a_silent_unauthenticated_connection_cannot_stall_a_real_hook() {
        let (server, mut rx) = bind("tok".into()).expect("bind");
        let p = server.port();
        let pane = "L3RtcC93cy9kZW1v";

        // 대조군: 방해가 없을 때의 정상 왕복.
        let baseline = std::time::Instant::now();
        assert_eq!(request(p, &post(pane, "1", "tok", BODY)), 204);
        let baseline = baseline.elapsed();

        // 조용한 연결: 붙이기만 하고 **한 바이트도 보내지 않는다**.
        let idle = TcpStream::connect(("127.0.0.1", p)).expect("connect");
        // 연결이 accept될 시간을 준다.
        std::thread::sleep(Duration::from_millis(200));

        let started = std::time::Instant::now();
        assert_eq!(
            request(p, &post(pane, "2", "tok", BODY)),
            204,
            "정상 훅이 처리되지 않았다"
        );
        let stalled = started.elapsed();
        drop(idle);

        assert!(
            stalled < HOOK_CURL_MAX_TIME,
            "조용한 연결 하나가 정상 훅을 {stalled:?} 지연시켰다 (대조군 {baseline:?}). \
             훅은 --max-time {HOOK_CURL_MAX_TIME:?}라 이 이벤트는 유실된다."
        );

        // 이벤트 둘 다 실제로 도착했는지 — 지연만 보고 유실을 놓치지 않도록.
        for expected in [1u64, 2] {
            let ev = futures::executor::block_on(rx.next()).expect("이벤트");
            assert_eq!(ev.spawn_nonce.0, expected);
        }
    }

    /// **상한을 넘으면 즉시 503이고, 그 수를 센다.**
    ///
    /// 이 테스트는 **남아 있는 한계도 같이 못 박는다**: 조용한 연결
    /// [`MAX_CONNECTIONS`]개는 여전히 정상 훅을 막는다. 스레드 풀은 그 비용을
    /// **1개 → 16개**로 올리고, 실패를 **5초 지연 → 즉시 503 + 카운터**로 바꿀
    /// 뿐 없애지는 않는다. 즉시 실패라 훅의 `--max-time 1.5` 안에 끝나고
    /// `refused()`에 흔적이 남는다는 것이 개선의 내용이다 — "완전히 막았다"가
    /// 아니다. 진짜로 없애려면 헤더 첫 바이트에 짧은 별도 타임아웃이 필요하다
    /// (follow-up).
    #[test]
    fn past_the_connection_cap_it_refuses_immediately_and_counts() {
        let (server, _rx) = bind("tok".into()).expect("bind");
        let p = server.port();
        let pane = "L3RtcC93cy9kZW1v";

        // 슬롯을 전부 조용한 연결로 채운다.
        let mut idle = Vec::new();
        for _ in 0..MAX_CONNECTIONS {
            idle.push(TcpStream::connect(("127.0.0.1", p)).expect("connect"));
        }
        // 전부 accept되어 슬롯을 잡을 시간을 준다.
        std::thread::sleep(Duration::from_millis(400));

        let started = std::time::Instant::now();
        let code = request(p, &post(pane, "1", "tok", BODY));
        let elapsed = started.elapsed();

        assert_eq!(code, 503, "상한을 넘겼는데 503이 아니다");
        assert!(
            elapsed < HOOK_CURL_MAX_TIME,
            "거절이 {elapsed:?} 걸렸다 — 즉시 끊지 않으면 상한이 지키려던 것을 \
             그대로 내주는 셈이다"
        );
        assert!(
            server.refused() > 0,
            "거절했는데 refused() 카운터가 0이다 — 이 실패 경로가 관측 불가능해진다"
        );
        assert_eq!(
            server.dropped(),
            0,
            "연결 거절을 dropped()에 세면 '앱이 느리다'와 '누가 두드린다'가 섞인다"
        );

        // 대조군: 슬롯을 놓아주면 다시 정상 동작한다(영구 고장이 아니다).
        drop(idle);
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(
            request(p, &post(pane, "2", "tok", BODY)),
            204,
            "연결이 풀린 뒤에도 서버가 살아 있어야 한다"
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
