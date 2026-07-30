use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::state::{Message, OpId, PortListener};

pub fn scan(op: OpId, workspace_roots: Vec<PathBuf>) -> iced::Task<Message> {
    let app_pid = std::process::id();
    iced::Task::perform(
        async move {
            let output = tokio::process::Command::new("lsof")
                .args(["-nP", "-iTCP", "-sTCP:LISTEN", "-F", "pcn"])
                .output()
                .await
                .map_err(|error| format!("Could not inspect listening ports: {error}"))?;
            if !output.status.success() {
                return Err("Listening ports could not be inspected.".to_string());
            }
            let raw = String::from_utf8_lossy(&output.stdout);
            let mut listeners = parse_lsof(&raw);
            let mut cwd_by_pid = HashMap::new();
            for pid in listeners
                .iter()
                .map(|listener| listener.pid)
                .collect::<HashSet<_>>()
            {
                let output = tokio::process::Command::new("lsof")
                    .args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"])
                    .output()
                    .await;
                let cwd = output
                    .ok()
                    .filter(|output| output.status.success())
                    .and_then(|output| {
                        String::from_utf8_lossy(&output.stdout)
                            .lines()
                            .find_map(|line| line.strip_prefix('n').map(PathBuf::from))
                    });
                cwd_by_pid.insert(pid, cwd);
            }
            for listener in &mut listeners {
                listener.workspace = listener.pid == app_pid
                    || cwd_by_pid
                        .get(&listener.pid)
                        .and_then(Option::as_deref)
                        .is_some_and(|cwd| {
                            workspace_roots.iter().any(|root| path_is_within(cwd, root))
                        });
            }
            Ok(listeners)
        },
        move |result| Message::PortsLoaded { op, result },
    )
}

fn path_is_within(path: &Path, root: &Path) -> bool {
    path == root || path.starts_with(root)
}

fn parse_lsof(raw: &str) -> Vec<PortListener> {
    let mut pid = 0;
    let mut process = String::new();
    let mut listeners = Vec::new();
    let mut seen = HashSet::new();

    for line in raw.lines() {
        match line.as_bytes().first().copied() {
            Some(b'p') => {
                pid = line[1..].parse().unwrap_or(0);
                process.clear();
            }
            Some(b'c') => process = line[1..].to_string(),
            Some(b'n') if pid > 0 && !process.is_empty() => {
                let address = line[1..].to_string();
                if seen.insert((pid, address.clone())) {
                    listeners.push(PortListener {
                        pid,
                        process: process.clone(),
                        address,
                        workspace: false,
                    });
                }
            }
            _ => {}
        }
    }

    listeners
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_deduplicates_lsof_listeners() {
        let listeners = parse_lsof(
            "p1\ncnode\nf10\nn127.0.0.1:3000\nf11\nn127.0.0.1:3000\np2\ncrust\nf3\nn*:8080\n",
        );

        assert_eq!(listeners.len(), 2);
        assert_eq!(listeners[0].pid, 1);
        assert_eq!(listeners[0].process, "node");
        assert_eq!(listeners[0].address, "127.0.0.1:3000");
        assert!(!listeners[0].workspace);
        assert_eq!(listeners[1].pid, 2);
        assert_eq!(listeners[1].process, "rust");
        assert_eq!(listeners[1].address, "*:8080");
    }

    #[test]
    fn workspace_path_matching_is_component_aware() {
        assert!(path_is_within(
            Path::new("/repo/worktree/subdir"),
            Path::new("/repo/worktree")
        ));
        assert!(!path_is_within(
            Path::new("/repo/worktree-copy"),
            Path::new("/repo/worktree")
        ));
    }
}
