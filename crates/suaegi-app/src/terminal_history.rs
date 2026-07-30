//! Worktree-scoped shell history, ported from Orca's `terminal-history.ts`.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

fn history_filename(shell: &Path) -> Option<&'static str> {
    let name = shell.file_name()?.to_string_lossy().to_ascii_lowercase();
    if name.starts_with("zsh") {
        Some("zsh_history")
    } else if name.starts_with("bash") {
        Some("bash_history")
    } else {
        None
    }
}

fn worktree_hash(id: &str) -> String {
    let digest = Sha256::digest(id.as_bytes());
    digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn history_root() -> PathBuf {
    match dirs::config_dir() {
        Some(config) => config.join("suaegi").join("terminal-history"),
        None => dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".suaegi")
            .join("terminal-history"),
    }
}

/// Inject `HISTFILE` for zsh/bash unless the caller already supplied one.
/// Directory and metadata failures are non-fatal and fall back to global shell
/// history, matching Orca.
pub fn inject(env: &mut Vec<(String, String)>, worktree_id: &str) {
    if env.iter().any(|(key, _)| key == "HISTFILE") || std::env::var_os("HISTFILE").is_some() {
        return;
    }
    let shell = std::env::var_os("SHELL")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/bin/sh"));
    let Some(filename) = history_filename(&shell) else {
        return;
    };
    let directory = history_root().join(worktree_hash(worktree_id));
    if std::fs::create_dir_all(&directory).is_err() {
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700));
    }
    let metadata = directory.join("meta.json");
    if !metadata.exists() {
        let escaped = worktree_id.replace('\\', "\\\\").replace('"', "\\\"");
        let _ = std::fs::write(metadata, format!("{{\"worktreeId\":\"{escaped}\"}}\n"));
    }
    env.push((
        "HISTFILE".to_string(),
        directory.join(filename).display().to_string(),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_matches_orca_sha256_prefix() {
        assert_eq!(worktree_hash("abc"), "ba7816bf8f01cfea");
    }

    #[test]
    fn shell_detection_matches_versioned_names() {
        assert_eq!(history_filename(Path::new("/bin/zsh")), Some("zsh_history"));
        assert_eq!(
            history_filename(Path::new("/nix/bin/bash-5.2")),
            Some("bash_history")
        );
        assert_eq!(history_filename(Path::new("/bin/fish")), None);
    }
}
