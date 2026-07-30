use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionRole {
    Control,
    Stream,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    pub version: u32,
    pub token: String,
    pub client_id: String,
    pub role: ConnectionRole,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloAck {
    pub ok: bool,
    pub pid: u32,
    pub started_at_unix_ms: u64,
    pub launch_nonce: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpawnSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum ControlRequest {
    Ping {
        id: u64,
    },
    CreateOrAttach {
        id: u64,
        session_id: String,
        spec: SpawnSpec,
    },
    Write {
        id: u64,
        session_id: String,
        data_base64: String,
    },
    Resize {
        id: u64,
        session_id: String,
        rows: u16,
        cols: u16,
    },
    Kill {
        id: u64,
        session_id: String,
    },
    KillAll {
        id: u64,
    },
    Shutdown {
        id: u64,
    },
    ListSessions {
        id: u64,
    },
    GetSize {
        id: u64,
        session_id: String,
    },
    GetForegroundPgid {
        id: u64,
        session_id: String,
    },
}

impl ControlRequest {
    pub fn id(&self) -> u64 {
        match self {
            Self::Ping { id }
            | Self::CreateOrAttach { id, .. }
            | Self::Write { id, .. }
            | Self::Resize { id, .. }
            | Self::Kill { id, .. }
            | Self::KillAll { id }
            | Self::Shutdown { id }
            | Self::ListSessions { id }
            | Self::GetSize { id, .. }
            | Self::GetForegroundPgid { id, .. } => *id,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionInfo {
    pub session_id: String,
    #[serde(default)]
    pub pid: u32,
    pub running: bool,
    pub exit_code: Option<i32>,
    pub rows: u16,
    pub cols: u16,
    pub next_sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ControlResult {
    Pong,
    Attached { is_new: bool, next_sequence: u64 },
    Written,
    Resized,
    Killed,
    KilledAll { count: usize },
    ShuttingDown,
    Sessions(Vec<SessionInfo>),
    Size { rows: u16, cols: u16 },
    ForegroundPgid(Option<i32>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlResponse {
    pub id: u64,
    pub result: Option<ControlResult>,
    pub error: Option<String>,
}

impl ControlResponse {
    pub fn ok(id: u64, result: ControlResult) -> Self {
        Self {
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: u64, error: impl Into<String>) -> Self {
        Self {
            id,
            result: None,
            error: Some(error.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamSubscribe {
    pub session_id: String,
    pub after_sequence: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum StreamEvent {
    Data { sequence: u64, data_base64: String },
    Exit { code: i32 },
    Gap { oldest_sequence: u64 },
    Error { message: String },
}
