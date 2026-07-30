//! Authenticated loopback RPC used by the standalone `suaegi` CLI.
//!
//! The desktop process is the authority for live UI state. A CLI process may
//! still inspect persisted data while the app is closed, but commands that
//! affect the running app are delivered here so the window and disk do not
//! drift apart.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::channel::mpsc;
use futures::stream::{BoxStream, StreamExt};
use iced::Subscription;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const MAX_LINE_BYTES: u64 = 1024 * 1024;
const QUEUE_CAPACITY: usize = 64;
// Repository/worktree creation and remote-aware commands can legitimately
// include fetches or setup work. Keep the authenticated local connection
// bounded, but do not abandon an in-flight app-owned mutation after 35 seconds
// and leave the caller believing it failed while it later succeeds.
const IO_TIMEOUT: Duration = Duration::from_secs(5 * 60);

type RpcResponse = Result<Value, String>;
type Responder = std::sync::mpsc::SyncSender<RpcResponse>;
type SharedSender = Arc<Mutex<mpsc::Sender<LocalRpcRequest>>>;

#[derive(Clone)]
pub struct LocalRpcRequest {
    pub method: String,
    pub params: Value,
    responder: Arc<Mutex<Option<Responder>>>,
}

impl std::fmt::Debug for LocalRpcRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LocalRpcRequest")
            .field("method", &self.method)
            .field("params", &self.params)
            .finish_non_exhaustive()
    }
}

impl LocalRpcRequest {
    pub fn respond(&self, response: RpcResponse) {
        if let Some(responder) = self
            .responder
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = responder.send(response);
        }
    }

    #[cfg(test)]
    pub fn for_test(method: &str, params: Value) -> (Self, std::sync::mpsc::Receiver<RpcResponse>) {
        let (responder, response) = std::sync::mpsc::sync_channel(1);
        (
            Self {
                method: method.to_string(),
                params,
                responder: Arc::new(Mutex::new(Some(responder))),
            },
            response,
        )
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeInfo {
    port: u16,
    token: String,
    pid: u32,
}

#[derive(Debug, Deserialize)]
struct WireRequest {
    token: String,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize, Deserialize)]
struct WireResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

pub struct LocalRpcServer {
    runtime_file: PathBuf,
    stop: Arc<AtomicBool>,
}

impl Drop for LocalRpcServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let owned_by_this_process = std::fs::read(&self.runtime_file)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<RuntimeInfo>(&bytes).ok())
            .is_some_and(|info| info.pid == std::process::id());
        if owned_by_this_process {
            let _ = std::fs::remove_file(&self.runtime_file);
        }
    }
}

#[derive(Clone)]
pub struct RpcSubscription {
    slot: Arc<Mutex<Option<mpsc::Receiver<LocalRpcRequest>>>>,
}

impl std::hash::Hash for RpcSubscription {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        "suaegi-local-rpc-v1".hash(state);
    }
}

impl RpcSubscription {
    fn new(receiver: mpsc::Receiver<LocalRpcRequest>) -> Self {
        Self {
            slot: Arc::new(Mutex::new(Some(receiver))),
        }
    }

    pub fn subscription(&self) -> Subscription<crate::state::Message> {
        Subscription::run_with(self.clone(), rpc_stream)
            .map(crate::state::Message::LocalRpcRequested)
    }
}

fn rpc_stream(subscription: &RpcSubscription) -> BoxStream<'static, LocalRpcRequest> {
    match subscription.slot.lock() {
        Ok(mut slot) => slot
            .take()
            .map(StreamExt::boxed)
            .unwrap_or_else(|| futures::stream::pending().boxed()),
        Err(_) => futures::stream::pending().boxed(),
    }
}

pub fn bind() -> Result<(LocalRpcServer, RpcSubscription), String> {
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .map_err(|error| format!("Could not bind the local CLI bridge: {error}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("Could not configure the local CLI bridge: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| format!("Could not inspect the local CLI bridge: {error}"))?
        .port();
    let token = random_token()?;
    let runtime_file = runtime_file();
    write_runtime_info(
        &runtime_file,
        &RuntimeInfo {
            port,
            token: token.clone(),
            pid: std::process::id(),
        },
    )?;

    let (sender, receiver) = mpsc::channel(QUEUE_CAPACITY);
    let sender = Arc::new(Mutex::new(sender));
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    std::thread::Builder::new()
        .name("suaegi-local-rpc".into())
        .spawn(move || serve(listener, token, sender, thread_stop))
        .map_err(|error| format!("Could not start the local CLI bridge: {error}"))?;

    Ok((
        LocalRpcServer { runtime_file, stop },
        RpcSubscription::new(receiver),
    ))
}

fn serve(listener: TcpListener, token: String, sender: SharedSender, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                let token = token.clone();
                let sender = Arc::clone(&sender);
                let _ = std::thread::Builder::new()
                    .name("suaegi-local-rpc-request".into())
                    .spawn(move || handle_connection(stream, &token, &sender));
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

fn handle_connection(mut stream: TcpStream, expected_token: &str, sender: &SharedSender) {
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    let response = read_request(&stream)
        .and_then(|request| {
            if !tokens_equal(&request.token, expected_token) {
                return Err("Local CLI authentication failed.".to_string());
            }
            let (responder, response) = std::sync::mpsc::sync_channel(1);
            let event = LocalRpcRequest {
                method: request.method,
                params: request.params,
                responder: Arc::new(Mutex::new(Some(responder))),
            };
            sender
                .lock()
                .map_err(|_| "The local CLI queue is unavailable.".to_string())?
                .try_send(event)
                .map_err(|_| "The local CLI queue is busy.".to_string())?;
            response
                .recv_timeout(IO_TIMEOUT)
                .map_err(|_| "The desktop app did not answer the CLI request.".to_string())?
        })
        .map_or_else(
            |error| WireResponse {
                ok: false,
                result: None,
                error: Some(error),
            },
            |result| WireResponse {
                ok: true,
                result: Some(result),
                error: None,
            },
        );
    if let Ok(mut encoded) = serde_json::to_vec(&response) {
        encoded.push(b'\n');
        let _ = stream.write_all(&encoded);
    }
}

fn read_request(stream: &TcpStream) -> Result<WireRequest, String> {
    let mut bytes = Vec::new();
    BufReader::new(stream)
        .take(MAX_LINE_BYTES + 1)
        .read_until(b'\n', &mut bytes)
        .map_err(|error| format!("Could not read the local CLI request: {error}"))?;
    if bytes.len() as u64 > MAX_LINE_BYTES {
        return Err("Local CLI request is too large.".to_string());
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("Local CLI request is invalid JSON: {error}"))
}

pub fn call(method: &str, params: Value) -> Result<Option<Value>, String> {
    let runtime_file = runtime_file();
    let bytes = match std::fs::read(&runtime_file) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Could not read the desktop runtime file: {error}")),
    };
    let info: RuntimeInfo = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Desktop runtime metadata is invalid: {error}"))?;
    let address = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, info.port));
    let mut stream = match TcpStream::connect_timeout(&address, Duration::from_millis(800)) {
        Ok(stream) => stream,
        Err(_) if !process_is_alive(info.pid) => {
            let _ = std::fs::remove_file(runtime_file);
            return Ok(None);
        }
        Err(error) => return Err(format!("Could not reach the running Suaegi app: {error}")),
    };
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .map_err(|error| format!("Could not configure the local CLI response: {error}"))?;
    let mut encoded = serde_json::to_vec(&serde_json::json!({
        "token": info.token,
        "method": method,
        "params": params,
    }))
    .map_err(|error| format!("Could not encode the local CLI request: {error}"))?;
    encoded.push(b'\n');
    stream
        .write_all(&encoded)
        .map_err(|error| format!("Could not send the local CLI request: {error}"))?;

    let mut response = Vec::new();
    BufReader::new(stream)
        .take(MAX_LINE_BYTES + 1)
        .read_until(b'\n', &mut response)
        .map_err(|error| format!("Could not read the local CLI response: {error}"))?;
    if response.len() as u64 > MAX_LINE_BYTES {
        return Err("Local CLI response is too large.".to_string());
    }
    let response: WireResponse = serde_json::from_slice(&response)
        .map_err(|error| format!("Local CLI response is invalid: {error}"))?;
    if response.ok {
        Ok(Some(response.result.unwrap_or(Value::Null)))
    } else {
        Err(response
            .error
            .unwrap_or_else(|| "The desktop app rejected the CLI request.".to_string()))
    }
}

fn runtime_file() -> PathBuf {
    crate::persistence_thread::default_data_file()
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("cli-runtime.json")
}

fn write_runtime_info(path: &std::path::Path, info: &RuntimeInfo) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Could not create the CLI runtime directory: {error}"))?;
    }
    let bytes = serde_json::to_vec(info)
        .map_err(|error| format!("Could not encode CLI runtime metadata: {error}"))?;
    std::fs::write(path, bytes)
        .map_err(|error| format!("Could not write CLI runtime metadata: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("Could not secure CLI runtime metadata: {error}"))?;
    }
    Ok(())
}

fn random_token() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|error| format!("Could not create a local CLI token: {error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn tokens_equal(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    // Signal 0 performs a read-only existence/permission probe.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(not(unix))]
fn process_is_alive(_pid: u32) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_comparison_checks_every_byte() {
        assert!(tokens_equal("abc", "abc"));
        assert!(!tokens_equal("abc", "abd"));
        assert!(!tokens_equal("abc", "ab"));
    }

    #[test]
    fn test_request_responds_exactly_once() {
        let (request, response) = LocalRpcRequest::for_test("status", Value::Null);
        request.respond(Ok(serde_json::json!({"running": true})));
        request.respond(Err("second".into()));
        assert_eq!(
            response.recv().expect("response").expect("success"),
            serde_json::json!({"running": true})
        );
    }
}
