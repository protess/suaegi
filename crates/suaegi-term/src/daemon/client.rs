use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Cursor, Read, Write};
use std::net::Shutdown;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
#[cfg(unix)]
use std::os::unix::process::CommandExt;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;

use super::protocol::{
    ConnectionRole, ControlRequest, ControlResponse, ControlResult, Hello, HelloAck, SessionInfo,
    SpawnSpec, StreamEvent, StreamSubscribe, MAX_FRAME_BYTES, PROTOCOL_VERSION,
};
use super::DaemonError;

const SOCKET_FILE: &str = "pty-v1.sock";
const TOKEN_FILE: &str = "pty-v1.token";
const PID_FILE: &str = "pty-v1.pid";
const IDENTITY_FILE: &str = "pty-v1.identity";
const LAUNCH_LOCK_FILE: &str = "pty-v1.launch.lock";
const NO_EXIT: i64 = i64::MIN;
const START_TIMEOUT: Duration = Duration::from_secs(5);
const RPC_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonConfiguration {
    pub executable: PathBuf,
    pub runtime_dir: PathBuf,
}

static CONFIGURATION: OnceLock<DaemonConfiguration> = OnceLock::new();

pub fn configure(configuration: DaemonConfiguration) -> Result<(), DaemonError> {
    if let Some(existing) = CONFIGURATION.get() {
        if existing == &configuration {
            return Ok(());
        }
        return Err(DaemonError::Start(
            "daemon was already configured differently".to_string(),
        ));
    }
    CONFIGURATION
        .set(configuration)
        .map_err(|_| DaemonError::Start("daemon configuration raced".to_string()))
}

pub fn configured() -> bool {
    CONFIGURATION.get().is_some()
}

pub fn default_runtime_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("suaegi")
        .join("daemon")
}

struct ControlChannel {
    stream: Mutex<ControlConnection>,
    next_id: AtomicU64,
}

struct ControlConnection {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
}

impl ControlChannel {
    fn connect(configuration: &DaemonConfiguration) -> Result<Self, DaemonError> {
        ensure_daemon(configuration)?;
        let token = read_token(&configuration.runtime_dir)?;
        let mut writer = UnixStream::connect(configuration.runtime_dir.join(SOCKET_FILE))?;
        writer.set_read_timeout(Some(RPC_TIMEOUT))?;
        writer.set_write_timeout(Some(RPC_TIMEOUT))?;
        let reader_stream = writer.try_clone()?;
        let mut reader = BufReader::new(reader_stream);
        handshake(&mut writer, &mut reader, &token, ConnectionRole::Control)?;
        Ok(Self {
            stream: Mutex::new(ControlConnection { writer, reader }),
            next_id: AtomicU64::new(1),
        })
    }

    fn request(
        &self,
        build: impl FnOnce(u64) -> ControlRequest,
    ) -> Result<ControlResult, DaemonError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = build(id);
        let mut connection = self
            .stream
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        write_json_line(&mut connection.writer, &request)?;
        let response: ControlResponse = read_json_line(&mut connection.reader)?;
        if response.id != id {
            return Err(DaemonError::Protocol(format!(
                "response id {} did not match request {id}",
                response.id
            )));
        }
        match (response.result, response.error) {
            (Some(result), None) => Ok(result),
            (_, Some(error)) => Err(DaemonError::Rejected(error)),
            _ => Err(DaemonError::Protocol(
                "response had neither result nor error".to_string(),
            )),
        }
    }
}

pub struct DaemonClientSession {
    session_id: String,
    control: Arc<ControlChannel>,
    running: Arc<AtomicBool>,
    exit_code: Arc<AtomicI64>,
    terminal_replies_enabled: Arc<AtomicBool>,
    stream_shutdown: Mutex<Option<UnixStream>>,
}

impl DaemonClientSession {
    pub fn create_or_attach(
        session_id: String,
        spec: SpawnSpec,
    ) -> Result<(Arc<Self>, DaemonReader, bool), DaemonError> {
        let configuration = CONFIGURATION.get().ok_or(DaemonError::NotConfigured)?;
        let control = Arc::new(ControlChannel::connect(configuration)?);
        let result = control.request(|id| ControlRequest::CreateOrAttach {
            id,
            session_id: session_id.clone(),
            spec,
        })?;
        let (is_new, next_sequence) = match result {
            ControlResult::Attached {
                is_new,
                next_sequence,
            } => (is_new, next_sequence),
            other => {
                return Err(DaemonError::Protocol(format!(
                    "unexpected create result: {other:?}"
                )))
            }
        };

        let running = Arc::new(AtomicBool::new(true));
        let exit_code = Arc::new(AtomicI64::new(NO_EXIT));
        let terminal_replies_enabled = Arc::new(AtomicBool::new(is_new));
        let (reader, stream_shutdown) = connect_stream(
            configuration,
            &session_id,
            Arc::clone(&running),
            Arc::clone(&exit_code),
            Arc::clone(&terminal_replies_enabled),
            (!is_new).then_some(next_sequence),
        )?;
        let session = Arc::new(Self {
            session_id,
            control,
            running,
            exit_code,
            terminal_replies_enabled,
            stream_shutdown: Mutex::new(Some(stream_shutdown)),
        });
        Ok((session, reader, is_new))
    }

    pub fn write(&self, bytes: &[u8]) -> Result<(), DaemonError> {
        let session_id = self.session_id.clone();
        let data_base64 = BASE64.encode(bytes);
        match self.control.request(|id| ControlRequest::Write {
            id,
            session_id,
            data_base64,
        })? {
            ControlResult::Written => Ok(()),
            result => Err(unexpected("write", result)),
        }
    }

    pub fn resize(&self, rows: u16, cols: u16) -> Result<(), DaemonError> {
        let session_id = self.session_id.clone();
        match self.control.request(|id| ControlRequest::Resize {
            id,
            session_id,
            rows,
            cols,
        })? {
            ControlResult::Resized => Ok(()),
            result => Err(unexpected("resize", result)),
        }
    }

    pub fn kill(&self) -> Result<(), DaemonError> {
        let session_id = self.session_id.clone();
        match self
            .control
            .request(|id| ControlRequest::Kill { id, session_id })?
        {
            ControlResult::Killed => Ok(()),
            result => Err(unexpected("kill", result)),
        }
    }

    pub fn size(&self) -> Result<(u16, u16), DaemonError> {
        let session_id = self.session_id.clone();
        match self
            .control
            .request(|id| ControlRequest::GetSize { id, session_id })?
        {
            ControlResult::Size { rows, cols } => Ok((rows, cols)),
            result => Err(unexpected("get_size", result)),
        }
    }

    #[cfg(unix)]
    pub fn foreground_pgid(&self) -> Option<i32> {
        let session_id = self.session_id.clone();
        match self
            .control
            .request(|id| ControlRequest::GetForegroundPgid { id, session_id })
        {
            Ok(ControlResult::ForegroundPgid(pgid)) => pgid,
            _ => None,
        }
    }

    pub fn running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    pub fn exit_code(&self) -> Option<i32> {
        let code = self.exit_code.load(Ordering::Acquire);
        (code != NO_EXIT).then_some(code as i32)
    }

    pub fn disconnect(&self) {
        if let Some(stream) = self
            .stream_shutdown
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }

    pub fn terminal_replies_enabled(&self) -> bool {
        self.terminal_replies_enabled.load(Ordering::Acquire)
    }
}

pub struct DaemonReader {
    reader: BufReader<UnixStream>,
    pending: Cursor<Vec<u8>>,
    running: Arc<AtomicBool>,
    exit_code: Arc<AtomicI64>,
    terminal_replies_enabled: Arc<AtomicBool>,
    replay_until_sequence: Option<u64>,
}

impl Read for DaemonReader {
    fn read(&mut self, target: &mut [u8]) -> std::io::Result<usize> {
        loop {
            let copied = self.pending.read(target)?;
            if copied > 0 {
                return Ok(copied);
            }
            let event: StreamEvent = read_json_line(&mut self.reader).map_err(to_io_error)?;
            match event {
                StreamEvent::Data {
                    sequence,
                    data_base64,
                } => {
                    self.terminal_replies_enabled.store(
                        self.replay_until_sequence
                            .is_none_or(|replay_until| sequence >= replay_until),
                        Ordering::Release,
                    );
                    let bytes = BASE64.decode(data_base64).map_err(to_io_error)?;
                    self.pending = Cursor::new(bytes);
                }
                StreamEvent::Exit { code } => {
                    self.terminal_replies_enabled
                        .store(false, Ordering::Release);
                    self.exit_code.store(code as i64, Ordering::Release);
                    self.running.store(false, Ordering::Release);
                    return Ok(0);
                }
                StreamEvent::Gap { .. } => {
                    // The retained tail is still useful. A future exact-snapshot
                    // mode can surface this marker to the UI.
                }
                StreamEvent::Error { message } => {
                    self.running.store(false, Ordering::Release);
                    return Err(std::io::Error::other(message));
                }
            }
        }
    }
}

pub fn list_sessions() -> Result<Vec<SessionInfo>, DaemonError> {
    let configuration = CONFIGURATION.get().ok_or(DaemonError::NotConfigured)?;
    let control = ControlChannel::connect(configuration)?;
    match control.request(|id| ControlRequest::ListSessions { id })? {
        ControlResult::Sessions(sessions) => Ok(sessions),
        result => Err(unexpected("list_sessions", result)),
    }
}

pub fn session_foreground_pgid(session_id: &str) -> Result<Option<i32>, DaemonError> {
    let configuration = CONFIGURATION.get().ok_or(DaemonError::NotConfigured)?;
    let control = ControlChannel::connect(configuration)?;
    match control.request(|id| ControlRequest::GetForegroundPgid {
        id,
        session_id: session_id.to_string(),
    })? {
        ControlResult::ForegroundPgid(pgid) => Ok(pgid),
        result => Err(unexpected("session_foreground_pgid", result)),
    }
}

pub fn kill_session(session_id: &str) -> Result<(), DaemonError> {
    let configuration = CONFIGURATION.get().ok_or(DaemonError::NotConfigured)?;
    let control = ControlChannel::connect(configuration)?;
    match control.request(|id| ControlRequest::Kill {
        id,
        session_id: session_id.to_string(),
    })? {
        ControlResult::Killed => Ok(()),
        result => Err(unexpected("kill_session", result)),
    }
}

pub fn kill_all_sessions() -> Result<usize, DaemonError> {
    let configuration = CONFIGURATION.get().ok_or(DaemonError::NotConfigured)?;
    let control = ControlChannel::connect(configuration)?;
    if let Ok(result) = control.request(|id| ControlRequest::KillAll { id }) {
        return match result {
            ControlResult::KilledAll { count } => Ok(count),
            result => Err(unexpected("kill_all_sessions", result)),
        };
    }
    // Protocol-v1 daemons predate the atomic request. Preserve update
    // compatibility by issuing the already-supported per-session kill.
    let sessions = list_sessions()?;
    let count = sessions.len();
    for session in sessions {
        kill_session(&session.session_id)?;
    }
    Ok(count)
}

pub fn restart() -> Result<(), DaemonError> {
    let configuration = CONFIGURATION.get().ok_or(DaemonError::NotConfigured)?;
    let identity = daemon_identity(configuration)?;
    let control = ControlChannel::connect(configuration)?;
    let graceful = matches!(
        control.request(|id| ControlRequest::Shutdown { id }),
        Ok(ControlResult::ShuttingDown)
    );
    if !graceful {
        let _ = kill_all_sessions();
        terminate_matching_daemon(configuration, &identity)?;
    }
    let deadline = Instant::now() + START_TIMEOUT;
    while Instant::now() < deadline {
        if probe(configuration).is_err() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    ensure_daemon(configuration)
}

fn daemon_identity(configuration: &DaemonConfiguration) -> Result<HelloAck, DaemonError> {
    let token = read_token(&configuration.runtime_dir)?;
    let mut writer = UnixStream::connect(configuration.runtime_dir.join(SOCKET_FILE))?;
    writer.set_read_timeout(Some(RPC_TIMEOUT))?;
    writer.set_write_timeout(Some(RPC_TIMEOUT))?;
    let reader_stream = writer.try_clone()?;
    let mut reader = BufReader::new(reader_stream);
    handshake(&mut writer, &mut reader, &token, ConnectionRole::Control)
}

fn terminate_matching_daemon(
    configuration: &DaemonConfiguration,
    identity: &HelloAck,
) -> Result<(), DaemonError> {
    let pid_record = fs::read_to_string(configuration.runtime_dir.join(PID_FILE))?;
    let mut fields = pid_record.split_whitespace();
    let pid = fields
        .next()
        .and_then(|value| value.parse::<libc::pid_t>().ok())
        .ok_or_else(|| DaemonError::Protocol("invalid daemon PID record".to_string()))?;
    let started_at = fields
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| DaemonError::Protocol("invalid daemon start record".to_string()))?;
    let nonce = fields
        .next()
        .ok_or_else(|| DaemonError::Protocol("missing daemon launch nonce".to_string()))?;
    let identity_nonce = fs::read_to_string(configuration.runtime_dir.join(IDENTITY_FILE))?;
    if pid <= 0
        || pid as u32 != identity.pid
        || started_at != identity.started_at_unix_ms
        || nonce != identity.launch_nonce
        || identity_nonce.trim() != identity.launch_nonce
    {
        return Err(DaemonError::Protocol(
            "daemon identity changed before restart".to_string(),
        ));
    }
    let result = unsafe { libc::kill(pid, libc::SIGTERM) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().into())
    }
}

fn connect_stream(
    configuration: &DaemonConfiguration,
    session_id: &str,
    running: Arc<AtomicBool>,
    exit_code: Arc<AtomicI64>,
    terminal_replies_enabled: Arc<AtomicBool>,
    replay_until_sequence: Option<u64>,
) -> Result<(DaemonReader, UnixStream), DaemonError> {
    let token = read_token(&configuration.runtime_dir)?;
    let mut writer = UnixStream::connect(configuration.runtime_dir.join(SOCKET_FILE))?;
    let reader_stream = writer.try_clone()?;
    let mut reader = BufReader::new(reader_stream);
    handshake(&mut writer, &mut reader, &token, ConnectionRole::Stream)?;
    write_json_line(
        &mut writer,
        &StreamSubscribe {
            session_id: session_id.to_string(),
            after_sequence: None,
        },
    )?;
    let shutdown = writer.try_clone()?;
    Ok((
        DaemonReader {
            reader,
            pending: Cursor::new(Vec::new()),
            running,
            exit_code,
            terminal_replies_enabled,
            replay_until_sequence,
        },
        shutdown,
    ))
}

fn handshake(
    writer: &mut UnixStream,
    reader: &mut BufReader<UnixStream>,
    token: &str,
    role: ConnectionRole,
) -> Result<HelloAck, DaemonError> {
    write_json_line(
        writer,
        &Hello {
            version: PROTOCOL_VERSION,
            token: token.to_string(),
            client_id: format!("{}-{}", std::process::id(), monotonic_nonce()),
            role,
        },
    )?;
    let ack: HelloAck = read_json_line(reader)?;
    if ack.ok {
        Ok(ack)
    } else {
        Err(DaemonError::Rejected(
            ack.error
                .unwrap_or_else(|| "daemon rejected handshake".to_string()),
        ))
    }
}

fn ensure_daemon(configuration: &DaemonConfiguration) -> Result<(), DaemonError> {
    #[cfg(not(unix))]
    {
        let _ = configuration;
        return Err(DaemonError::UnsupportedPlatform);
    }
    #[cfg(unix)]
    {
        fs::create_dir_all(&configuration.runtime_dir)?;
        ensure_token(&configuration.runtime_dir)?;
        if probe(configuration).is_ok() {
            return Ok(());
        }

        let lock_path = configuration.runtime_dir.join(LAUNCH_LOCK_FILE);
        let lock = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&lock_path);
        if let Err(error) = lock {
            if error.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(error.into());
            }
            let deadline = Instant::now() + START_TIMEOUT;
            while Instant::now() < deadline {
                if probe(configuration).is_ok() {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            if launch_owner_is_alive(&lock_path) {
                return Err(DaemonError::Start(
                    "another live process owns the daemon launch lock".to_string(),
                ));
            }
            fs::remove_file(&lock_path)?;
            return ensure_daemon(configuration);
        }
        let mut lock = lock.expect("successful launch lock was checked above");
        writeln!(lock, "{}", std::process::id())?;
        lock.flush()?;
        let _lock_cleanup = LaunchLockCleanup(lock_path);

        if probe(configuration).is_ok() {
            return Ok(());
        }
        let socket = configuration.runtime_dir.join(SOCKET_FILE);
        if socket.exists() {
            if UnixStream::connect(&socket).is_ok() {
                return Err(DaemonError::Start(
                    "a process accepts the daemon socket but failed authentication or health check"
                        .to_string(),
                ));
            }
            fs::remove_file(&socket)?;
        }
        let mut command = Command::new(&configuration.executable);
        command
            .arg("--pty-daemon")
            .arg("--runtime-dir")
            .arg(&configuration.runtime_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        command
            .spawn()
            .map_err(|error| DaemonError::Start(error.to_string()))?;

        let deadline = Instant::now() + START_TIMEOUT;
        let mut last_error = None;
        while Instant::now() < deadline {
            match probe(configuration) {
                Ok(()) => return Ok(()),
                Err(error) => last_error = Some(error),
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        Err(DaemonError::Start(format!(
            "daemon did not become ready: {}",
            last_error
                .map(|error| error.to_string())
                .unwrap_or_else(|| "unknown error".to_string())
        )))
    }
}

fn probe(configuration: &DaemonConfiguration) -> Result<(), DaemonError> {
    let token = read_token(&configuration.runtime_dir)?;
    let mut writer = UnixStream::connect(configuration.runtime_dir.join(SOCKET_FILE))?;
    writer.set_read_timeout(Some(Duration::from_millis(250)))?;
    writer.set_write_timeout(Some(Duration::from_millis(250)))?;
    let reader_stream = writer.try_clone()?;
    let mut reader = BufReader::new(reader_stream);
    handshake(&mut writer, &mut reader, &token, ConnectionRole::Control)?;
    let request = ControlRequest::Ping { id: 1 };
    write_json_line(&mut writer, &request)?;
    let response: ControlResponse = read_json_line(&mut reader)?;
    match response.result {
        Some(ControlResult::Pong) => Ok(()),
        _ => Err(DaemonError::Protocol("ping failed".to_string())),
    }
}

struct LaunchLockCleanup(PathBuf);

impl Drop for LaunchLockCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn ensure_token(runtime_dir: &Path) -> Result<(), DaemonError> {
    let path = runtime_dir.join(TOKEN_FILE);
    if fs::read_to_string(&path).is_ok_and(|token| !token.trim().is_empty()) {
        return Ok(());
    }
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| DaemonError::Start(error.to_string()))?;
    let token = BASE64.encode(bytes);
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    match options.open(&path) {
        Ok(mut file) => {
            writeln!(file, "{token}")?;
            file.flush()?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if fs::read_to_string(path).is_ok_and(|value| !value.trim().is_empty()) {
                Ok(())
            } else {
                Err(DaemonError::Protocol(
                    "existing daemon token is empty".to_string(),
                ))
            }
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
fn launch_owner_is_alive(path: &Path) -> bool {
    let Ok(contents) = fs::read_to_string(path) else {
        return false;
    };
    let Ok(pid) = contents.trim().parse::<libc::pid_t>() else {
        return false;
    };
    pid > 0 && unsafe { libc::kill(pid, 0) == 0 }
}

fn read_token(runtime_dir: &Path) -> Result<String, DaemonError> {
    let token = fs::read_to_string(runtime_dir.join(TOKEN_FILE))?
        .trim()
        .to_string();
    if token.is_empty() {
        Err(DaemonError::Protocol("daemon token is empty".to_string()))
    } else {
        Ok(token)
    }
}

fn read_json_line<T: serde::de::DeserializeOwned>(
    reader: &mut impl BufRead,
) -> Result<T, DaemonError> {
    let mut line = Vec::new();
    let read = reader.read_until(b'\n', &mut line)?;
    if read == 0 {
        return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into());
    }
    if line.len() > MAX_FRAME_BYTES || !line.ends_with(b"\n") {
        return Err(DaemonError::Protocol(
            "invalid or oversized frame".to_string(),
        ));
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

fn unexpected(operation: &str, result: ControlResult) -> DaemonError {
    DaemonError::Protocol(format!("unexpected {operation} result: {result:?}"))
}

fn to_io_error(error: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(error.to_string())
}

fn monotonic_nonce() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}
