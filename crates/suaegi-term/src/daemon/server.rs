use std::collections::{HashMap, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;

use super::protocol::{
    ConnectionRole, ControlRequest, ControlResponse, ControlResult, Hello, HelloAck, SessionInfo,
    SpawnSpec, StreamEvent, StreamSubscribe, MAX_FRAME_BYTES, PROTOCOL_VERSION,
};
use super::DaemonError;
use crate::pty::{PtySession, PtySpawn};

const SOCKET_FILE: &str = "pty-v1.sock";
const TOKEN_FILE: &str = "pty-v1.token";
const PID_FILE: &str = "pty-v1.pid";
const IDENTITY_FILE: &str = "pty-v1.identity";
const HISTORY_LIMIT_BYTES: usize = 8 * 1024 * 1024;
const SUBSCRIBER_QUEUE_EVENTS: usize = 256;
const NO_EXIT: i64 = i64::MIN;

struct OutputChunk {
    sequence: u64,
    bytes: Vec<u8>,
}

struct History {
    chunks: VecDeque<OutputChunk>,
    bytes: usize,
}

impl History {
    fn push(&mut self, chunk: OutputChunk) {
        self.bytes += chunk.bytes.len();
        self.chunks.push_back(chunk);
        while self.bytes > HISTORY_LIMIT_BYTES {
            let Some(removed) = self.chunks.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(removed.bytes.len());
        }
    }
}

struct HostedSession {
    pty: Arc<PtySession>,
    history: Mutex<History>,
    subscribers: Mutex<Vec<SyncSender<StreamEvent>>>,
    next_sequence: AtomicU64,
    running: AtomicBool,
    exit_code: AtomicI64,
    size: Mutex<(u16, u16)>,
}

impl HostedSession {
    fn spawn(spec: SpawnSpec) -> Result<Arc<Self>, DaemonError> {
        let size = (spec.rows.max(1), spec.cols.max(1));
        let (pty, mut reader) = PtySession::spawn(PtySpawn {
            program: spec.program,
            args: spec.args,
            cwd: spec.cwd,
            env: spec.env,
            env_remove: Vec::new(),
            rows: size.0,
            cols: size.1,
        })
        .map_err(|error| DaemonError::Start(error.to_string()))?;
        let session = Arc::new(Self {
            pty: Arc::new(pty),
            history: Mutex::new(History {
                chunks: VecDeque::new(),
                bytes: 0,
            }),
            subscribers: Mutex::new(Vec::new()),
            next_sequence: AtomicU64::new(0),
            running: AtomicBool::new(true),
            exit_code: AtomicI64::new(NO_EXIT),
            size: Mutex::new(size),
        });

        let hosted = Arc::clone(&session);
        std::thread::Builder::new()
            .name("suaegi-daemon-pty-reader".to_string())
            .spawn(move || {
                let mut buffer = vec![0; 64 * 1024];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(read) => hosted.publish(&buffer[..read]),
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
                let _ = hosted.pty.kill();
                let code = hosted.pty.wait().unwrap_or(-1);
                hosted.exit_code.store(code as i64, Ordering::Release);
                hosted.running.store(false, Ordering::Release);
                hosted.broadcast(StreamEvent::Exit { code });
            })
            .map_err(|error| DaemonError::Start(error.to_string()))?;

        Ok(session)
    }

    fn publish(&self, bytes: &[u8]) {
        let sequence = self.next_sequence.fetch_add(1, Ordering::AcqRel);
        self.history
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(OutputChunk {
                sequence,
                bytes: bytes.to_vec(),
            });
        self.broadcast(StreamEvent::Data {
            sequence,
            data_base64: BASE64.encode(bytes),
        });
    }

    fn broadcast(&self, event: StreamEvent) {
        self.subscribers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|subscriber| match subscriber.try_send(event.clone()) {
                Ok(()) => true,
                Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => false,
            });
    }

    fn subscribe(&self, after_sequence: Option<u64>) -> (Vec<StreamEvent>, Receiver<StreamEvent>) {
        let (sender, receiver) = mpsc::sync_channel(SUBSCRIBER_QUEUE_EVENTS);
        let mut replay = Vec::new();
        let history = self
            .history
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut subscribers = self
            .subscribers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let requested = after_sequence.map_or(0, |sequence| sequence.saturating_add(1));
        if let Some(oldest) = history.chunks.front().map(|chunk| chunk.sequence) {
            if requested < oldest {
                replay.push(StreamEvent::Gap {
                    oldest_sequence: oldest,
                });
            }
        }
        for chunk in history
            .chunks
            .iter()
            .filter(|chunk| chunk.sequence >= requested)
        {
            replay.push(StreamEvent::Data {
                sequence: chunk.sequence,
                data_base64: BASE64.encode(&chunk.bytes),
            });
        }
        if self.running.load(Ordering::Acquire) {
            subscribers.push(sender);
        } else {
            let code = self.exit_code.load(Ordering::Acquire) as i32;
            replay.push(StreamEvent::Exit { code });
        }
        (replay, receiver)
    }

    fn info(&self, session_id: String) -> SessionInfo {
        let (rows, cols) = *self
            .size
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let exit = self.exit_code.load(Ordering::Acquire);
        SessionInfo {
            session_id,
            pid: self.pty.process_id(),
            running: self.running.load(Ordering::Acquire),
            exit_code: (exit != NO_EXIT).then_some(exit as i32),
            rows,
            cols,
            next_sequence: self.next_sequence.load(Ordering::Acquire),
        }
    }
}

struct ServerState {
    token: String,
    started_at_unix_ms: u64,
    launch_nonce: String,
    sessions: Mutex<HashMap<String, Arc<HostedSession>>>,
    shutdown: AtomicBool,
    socket_path: PathBuf,
}

pub fn run(runtime_dir: &Path) -> Result<(), DaemonError> {
    fs::create_dir_all(runtime_dir)?;
    fs::set_permissions(runtime_dir, fs::Permissions::from_mode(0o700))?;
    let token = read_or_create_token(&runtime_dir.join(TOKEN_FILE))?;
    let socket_path = runtime_dir.join(SOCKET_FILE);
    let listener = UnixListener::bind(&socket_path)?;
    fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))?;

    let started_at_unix_ms = unix_ms();
    let launch_nonce = random_token()?;
    write_private(
        &runtime_dir.join(PID_FILE),
        format!(
            "{} {started_at_unix_ms} {launch_nonce}\n",
            std::process::id()
        )
        .as_bytes(),
    )?;
    write_private(
        &runtime_dir.join(IDENTITY_FILE),
        format!("{launch_nonce}\n").as_bytes(),
    )?;
    let cleanup = SocketCleanup {
        socket_path: socket_path.clone(),
        pid_path: runtime_dir.join(PID_FILE),
        identity_path: runtime_dir.join(IDENTITY_FILE),
        launch_nonce: launch_nonce.clone(),
    };
    let state = Arc::new(ServerState {
        token,
        started_at_unix_ms,
        launch_nonce,
        sessions: Mutex::new(HashMap::new()),
        shutdown: AtomicBool::new(false),
        socket_path,
    });

    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                if state.shutdown.load(Ordering::Acquire) {
                    break;
                }
                let client_state = Arc::clone(&state);
                let _ = std::thread::Builder::new()
                    .name("suaegi-daemon-client".to_string())
                    .spawn(move || {
                        let _ = handle_connection(stream, client_state);
                    });
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        }
    }
    drop(cleanup);
    Ok(())
}

struct SocketCleanup {
    socket_path: PathBuf,
    pid_path: PathBuf,
    identity_path: PathBuf,
    launch_nonce: String,
}

impl Drop for SocketCleanup {
    fn drop(&mut self) {
        let still_ours = fs::read_to_string(&self.identity_path)
            .is_ok_and(|nonce| nonce.trim() == self.launch_nonce);
        if still_ours {
            let _ = fs::remove_file(&self.socket_path);
            let _ = fs::remove_file(&self.pid_path);
            let _ = fs::remove_file(&self.identity_path);
        }
    }
}

fn handle_connection(mut stream: UnixStream, state: Arc<ServerState>) -> Result<(), DaemonError> {
    let reader_stream = stream.try_clone()?;
    let mut reader = BufReader::new(reader_stream);
    let hello: Hello = read_json_line(&mut reader)?;
    let error = if hello.version != PROTOCOL_VERSION {
        Some(format!("protocol version {} is unsupported", hello.version))
    } else if hello.token != state.token {
        Some("authentication failed".to_string())
    } else {
        None
    };
    write_json_line(
        &mut stream,
        &HelloAck {
            ok: error.is_none(),
            pid: std::process::id(),
            started_at_unix_ms: state.started_at_unix_ms,
            launch_nonce: state.launch_nonce.clone(),
            error: error.clone(),
        },
    )?;
    if let Some(error) = error {
        return Err(DaemonError::Rejected(error));
    }

    match hello.role {
        ConnectionRole::Control => handle_control(reader, stream, state),
        ConnectionRole::Stream => handle_stream(reader, stream, state),
    }
}

fn handle_control(
    mut reader: BufReader<UnixStream>,
    mut stream: UnixStream,
    state: Arc<ServerState>,
) -> Result<(), DaemonError> {
    loop {
        let request: ControlRequest = match read_json_line(&mut reader) {
            Ok(request) => request,
            Err(DaemonError::Io(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::UnexpectedEof
                        | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::BrokenPipe
                ) =>
            {
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        let response = dispatch_control(&state, request);
        write_json_line(&mut stream, &response)?;
    }
}

fn dispatch_control(state: &ServerState, request: ControlRequest) -> ControlResponse {
    let id = request.id();
    let result: Result<ControlResult, String> = (|| match request {
        ControlRequest::Ping { .. } => Ok(ControlResult::Pong),
        ControlRequest::CreateOrAttach {
            session_id, spec, ..
        } => {
            let mut sessions = state
                .sessions
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(existing) = sessions.get(&session_id) {
                return Ok(ControlResult::Attached {
                    is_new: false,
                    next_sequence: existing.next_sequence.load(Ordering::Acquire),
                });
            }
            let session = HostedSession::spawn(spec).map_err(|error| error.to_string())?;
            let next_sequence = session.next_sequence.load(Ordering::Acquire);
            sessions.insert(session_id, session);
            Ok(ControlResult::Attached {
                is_new: true,
                next_sequence,
            })
        }
        ControlRequest::Write {
            session_id,
            data_base64,
            ..
        } => {
            let session = find_session(state, &session_id)?;
            let bytes = BASE64
                .decode(data_base64)
                .map_err(|error| format!("invalid base64 data: {error}"))?;
            session
                .pty
                .write(&bytes)
                .map_err(|error| error.to_string())?;
            Ok(ControlResult::Written)
        }
        ControlRequest::Resize {
            session_id,
            rows,
            cols,
            ..
        } => {
            let session = find_session(state, &session_id)?;
            session
                .pty
                .resize(rows, cols)
                .map_err(|error| error.to_string())?;
            *session
                .size
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = (rows.max(1), cols.max(1));
            Ok(ControlResult::Resized)
        }
        ControlRequest::Kill { session_id, .. } => {
            let session = state
                .sessions
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&session_id)
                .ok_or_else(|| format!("unknown session {session_id:?}"))?;
            session.pty.kill().map_err(|error| error.to_string())?;
            Ok(ControlResult::Killed)
        }
        ControlRequest::KillAll { .. } => {
            let sessions = {
                let mut sessions = state
                    .sessions
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                sessions
                    .drain()
                    .map(|(_, session)| session)
                    .collect::<Vec<_>>()
            };
            let count = sessions.len();
            for session in sessions {
                let _ = session.pty.kill();
            }
            Ok(ControlResult::KilledAll { count })
        }
        ControlRequest::Shutdown { .. } => {
            let sessions = {
                let mut sessions = state
                    .sessions
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                sessions
                    .drain()
                    .map(|(_, session)| session)
                    .collect::<Vec<_>>()
            };
            for session in sessions {
                let _ = session.pty.kill();
            }
            state.shutdown.store(true, Ordering::Release);
            let _ = UnixStream::connect(&state.socket_path);
            Ok(ControlResult::ShuttingDown)
        }
        ControlRequest::ListSessions { .. } => {
            let sessions = state
                .sessions
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut infos = sessions
                .iter()
                .map(|(id, session)| session.info(id.clone()))
                .collect::<Vec<_>>();
            infos.sort_by(|a, b| a.session_id.cmp(&b.session_id));
            Ok(ControlResult::Sessions(infos))
        }
        ControlRequest::GetSize { session_id, .. } => {
            let session = find_session(state, &session_id)?;
            let (rows, cols) = *session
                .size
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Ok(ControlResult::Size { rows, cols })
        }
        ControlRequest::GetForegroundPgid { session_id, .. } => {
            let session = find_session(state, &session_id)?;
            #[cfg(unix)]
            let pgid = session.pty.foreground_pgid();
            #[cfg(not(unix))]
            let pgid = None;
            Ok(ControlResult::ForegroundPgid(pgid))
        }
    })();
    match result {
        Ok(result) => ControlResponse::ok(id, result),
        Err(error) => ControlResponse::error(id, error),
    }
}

fn find_session(state: &ServerState, session_id: &str) -> Result<Arc<HostedSession>, String> {
    state
        .sessions
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(session_id)
        .cloned()
        .ok_or_else(|| format!("unknown session {session_id:?}"))
}

fn handle_stream(
    mut reader: BufReader<UnixStream>,
    mut stream: UnixStream,
    state: Arc<ServerState>,
) -> Result<(), DaemonError> {
    let subscribe: StreamSubscribe = read_json_line(&mut reader)?;
    let session = match find_session(&state, &subscribe.session_id) {
        Ok(session) => session,
        Err(message) => {
            write_json_line(&mut stream, &StreamEvent::Error { message })?;
            return Ok(());
        }
    };
    let (replay, events) = session.subscribe(subscribe.after_sequence);
    for event in replay {
        write_json_line(&mut stream, &event)?;
        if matches!(event, StreamEvent::Exit { .. }) {
            return Ok(());
        }
    }
    for event in events {
        if write_json_line(&mut stream, &event).is_err() {
            break;
        }
        if matches!(event, StreamEvent::Exit { .. }) {
            break;
        }
    }
    Ok(())
}

fn read_json_line<T: serde::de::DeserializeOwned>(
    reader: &mut impl BufRead,
) -> Result<T, DaemonError> {
    let mut line = Vec::new();
    let read = reader
        .take((MAX_FRAME_BYTES + 1) as u64)
        .read_until(b'\n', &mut line)?;
    if read == 0 {
        return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into());
    }
    if line.len() > MAX_FRAME_BYTES || !line.ends_with(b"\n") {
        return Err(DaemonError::Protocol("frame exceeds limit".to_string()));
    }
    serde_json::from_slice(&line).map_err(|error| DaemonError::Protocol(error.to_string()))
}

fn write_json_line<T: serde::Serialize>(
    stream: &mut UnixStream,
    value: &T,
) -> Result<(), DaemonError> {
    serde_json::to_writer(&mut *stream, value)
        .map_err(|error| DaemonError::Protocol(error.to_string()))?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    Ok(())
}

fn read_or_create_token(path: &Path) -> Result<String, DaemonError> {
    if let Ok(token) = fs::read_to_string(path) {
        let token = token.trim().to_string();
        if !token.is_empty() {
            return Ok(token);
        }
    }
    let token = random_token()?;
    match OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
    {
        Ok(mut file) => {
            file.write_all(format!("{token}\n").as_bytes())?;
            file.flush()?;
            Ok(token)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let existing = fs::read_to_string(path)?.trim().to_string();
            if existing.is_empty() {
                Err(DaemonError::Protocol(
                    "existing daemon token is empty".to_string(),
                ))
            } else {
                Ok(existing)
            }
        }
        Err(error) => Err(error.into()),
    }
}

fn random_token() -> Result<String, DaemonError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| DaemonError::Start(error.to_string()))?;
    Ok(BASE64.encode(bytes))
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), DaemonError> {
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.flush()?;
    Ok(())
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
