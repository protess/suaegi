use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;

use super::protocol::{SessionInfo, SpawnSpec};
use super::DaemonError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonConfiguration {
    pub executable: PathBuf,
    pub runtime_dir: PathBuf,
}

pub fn configure(_configuration: DaemonConfiguration) -> Result<(), DaemonError> {
    // Keep the application usable with its existing in-process PTY backend.
    // `configured()` remains false, so no persistent session is attempted.
    Ok(())
}

pub fn configured() -> bool {
    false
}

pub fn default_runtime_dir() -> PathBuf {
    std::env::temp_dir().join("suaegi").join("daemon")
}

pub fn list_sessions() -> Result<Vec<SessionInfo>, DaemonError> {
    Err(DaemonError::UnsupportedPlatform)
}

pub fn session_foreground_pgid(_session_id: &str) -> Result<Option<i32>, DaemonError> {
    Err(DaemonError::UnsupportedPlatform)
}

pub fn kill_session(_session_id: &str) -> Result<(), DaemonError> {
    Err(DaemonError::UnsupportedPlatform)
}

pub fn kill_all_sessions() -> Result<usize, DaemonError> {
    Err(DaemonError::UnsupportedPlatform)
}

pub fn restart() -> Result<(), DaemonError> {
    Err(DaemonError::UnsupportedPlatform)
}

pub struct DaemonClientSession;

impl DaemonClientSession {
    pub fn create_or_attach(
        _session_id: String,
        _spec: SpawnSpec,
    ) -> Result<(Arc<Self>, DaemonReader, bool), DaemonError> {
        Err(DaemonError::UnsupportedPlatform)
    }

    pub fn write(&self, _bytes: &[u8]) -> Result<(), DaemonError> {
        Err(DaemonError::UnsupportedPlatform)
    }

    pub fn resize(&self, _rows: u16, _cols: u16) -> Result<(), DaemonError> {
        Err(DaemonError::UnsupportedPlatform)
    }

    pub fn kill(&self) -> Result<(), DaemonError> {
        Err(DaemonError::UnsupportedPlatform)
    }

    pub fn size(&self) -> Result<(u16, u16), DaemonError> {
        Err(DaemonError::UnsupportedPlatform)
    }

    pub fn running(&self) -> bool {
        false
    }

    pub fn exit_code(&self) -> Option<i32> {
        None
    }

    pub fn disconnect(&self) {}

    pub fn terminal_replies_enabled(&self) -> bool {
        false
    }
}

pub struct DaemonReader;

impl Read for DaemonReader {
    fn read(&mut self, _target: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "PTY daemon is unsupported on this platform",
        ))
    }
}
