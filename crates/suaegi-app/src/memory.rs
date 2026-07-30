//! Point-in-time process and host-memory diagnostics used by the status bar
//! and the Orca-compatible `diagnostics memory` CLI.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use suaegi_core::domain::PersistedState;

const HISTORY_CAPACITY: usize = 60;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageValues {
    pub cpu: f64,
    pub memory: u64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppMemory {
    pub cpu: f64,
    pub memory: u64,
    pub main: UsageValues,
    pub renderer: UsageValues,
    pub other: UsageValues,
    pub history: Vec<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMemory {
    pub session_id: String,
    pub pane_key: Option<String>,
    pub pid: u32,
    pub cpu: f64,
    pub memory: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeMemory {
    pub worktree_id: String,
    pub worktree_name: String,
    pub repo_id: String,
    pub repo_name: String,
    pub sessions: Vec<SessionMemory>,
    pub cpu: f64,
    pub memory: u64,
    pub history: Vec<u64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostMemory {
    pub total_memory: u64,
    pub free_memory: u64,
    pub available_memory: u64,
    pub available_memory_source: String,
    pub used_memory: u64,
    pub memory_usage_percent: f64,
    pub cpu_core_count: usize,
    pub load_average_1m: f64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorySnapshot {
    pub app: AppMemory,
    pub worktrees: Vec<WorktreeMemory>,
    pub host: HostMemory,
    pub process_memory_metric: String,
    pub total_cpu: f64,
    pub total_memory: u64,
    pub collected_at: u64,
}

#[derive(Debug, Clone)]
struct ProcessRow {
    pid: u32,
    parent: u32,
    cpu: f64,
    memory: u64,
    command: String,
}

fn process_rows() -> Vec<ProcessRow> {
    let output = match Command::new("ps")
        .args(["-eo", "pid=,ppid=,pcpu=,rss=,command="])
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .output()
    {
        Ok(output) if output.status.success() => output.stdout,
        _ => return Vec::new(),
    };
    String::from_utf8_lossy(&output)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let parent = fields.next()?.parse().ok()?;
            let cpu = fields
                .next()
                .and_then(|value| value.parse::<f64>().ok())
                .filter(|value| value.is_finite() && *value > 0.0)
                .unwrap_or(0.0);
            let memory = fields
                .next()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0)
                .saturating_mul(1024);
            Some(ProcessRow {
                pid,
                parent,
                cpu,
                memory,
                command: fields.collect::<Vec<_>>().join(" "),
            })
        })
        .collect()
}

fn subtree(rows: &[ProcessRow], root: u32) -> Vec<&ProcessRow> {
    if root == 0 {
        return Vec::new();
    }
    let mut children = HashMap::<u32, Vec<u32>>::new();
    let by_pid = rows
        .iter()
        .map(|row| {
            children.entry(row.parent).or_default().push(row.pid);
            (row.pid, row)
        })
        .collect::<HashMap<_, _>>();
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    let mut queue = vec![root];
    while let Some(pid) = queue.pop() {
        if !seen.insert(pid) {
            continue;
        }
        if let Some(row) = by_pid.get(&pid) {
            result.push(*row);
        }
        queue.extend(children.get(&pid).into_iter().flatten().copied());
    }
    result
}

fn total<'a>(rows: impl IntoIterator<Item = &'a ProcessRow>) -> UsageValues {
    rows.into_iter()
        .fold(UsageValues::default(), |mut sum, row| {
            sum.cpu += row.cpu;
            sum.memory = sum.memory.saturating_add(row.memory);
            sum
        })
}

fn command_number(args: &[&str]) -> Option<u64> {
    let output = Command::new(args.first()?).args(&args[1..]).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().parse().ok())
        .flatten()
}

fn host_memory() -> HostMemory {
    let total_memory = command_number(&["sysctl", "-n", "hw.memsize"]).unwrap_or(0);
    let vm = Command::new("vm_stat").output().ok();
    let mut page_size = 4096_u64;
    let mut available_pages = 0_u64;
    if let Some(output) = vm.filter(|output| output.status.success()) {
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            if line.contains("page size of") {
                page_size = line
                    .split_whitespace()
                    .find_map(|word| word.parse().ok())
                    .unwrap_or(page_size);
            }
            if [
                "Pages free:",
                "Pages inactive:",
                "Pages speculative:",
                "Pages purgeable:",
            ]
            .iter()
            .any(|prefix| line.starts_with(prefix))
            {
                available_pages = available_pages.saturating_add(
                    line.split_whitespace()
                        .last()
                        .map(|value| value.trim_end_matches('.'))
                        .and_then(|value| value.parse::<u64>().ok())
                        .unwrap_or(0),
                );
            }
        }
    }
    let available_memory = available_pages.saturating_mul(page_size).min(total_memory);
    let used_memory = total_memory.saturating_sub(available_memory);
    HostMemory {
        total_memory,
        free_memory: available_memory,
        available_memory,
        available_memory_source: "memory-pressure".into(),
        used_memory,
        memory_usage_percent: if total_memory == 0 {
            0.0
        } else {
            used_memory as f64 * 100.0 / total_memory as f64
        },
        cpu_core_count: std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1),
        load_average_1m: Command::new("sysctl")
            .args(["-n", "vm.loadavg"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .split_whitespace()
                    .find_map(|value| {
                        value
                            .trim_matches(|character| matches!(character, '{' | '}'))
                            .parse()
                            .ok()
                    })
            })
            .unwrap_or(0.0),
    }
}

fn push_history(key: &str, value: u64) -> Vec<u64> {
    static HISTORY: OnceLock<Mutex<HashMap<String, VecDeque<u64>>>> = OnceLock::new();
    let mut history = HISTORY
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let samples = history.entry(key.to_string()).or_default();
    samples.push_back(value);
    while samples.len() > HISTORY_CAPACITY {
        samples.pop_front();
    }
    samples.iter().copied().collect()
}

fn app_pid(rows: &[ProcessRow]) -> u32 {
    rows.iter()
        .find(|row| {
            row.command.contains("/Contents/MacOS/suaegi-app")
                && !row.command.contains("--pty-daemon")
        })
        .map(|row| row.pid)
        .unwrap_or(0)
}

fn legacy_daemon_session_pids(
    rows: &[ProcessRow],
    sessions: &[suaegi_term::daemon::SessionInfo],
) -> HashMap<String, u32> {
    let daemon_pid = rows
        .iter()
        .find(|row| {
            row.command.contains("/Contents/MacOS/suaegi-app")
                && row.command.contains("--pty-daemon")
        })
        .map(|row| row.pid);
    let Some(daemon_pid) = daemon_pid else {
        return HashMap::new();
    };
    let mut result = HashMap::new();
    for child in rows.iter().filter(|row| row.parent == daemon_pid) {
        let cwd = Command::new("lsof")
            .args(["-a", "-p", &child.pid.to_string(), "-d", "cwd", "-Fn"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .find_map(|line| line.strip_prefix('n').map(str::to_string))
            });
        let Some(cwd) = cwd else {
            continue;
        };
        let matches = sessions
            .iter()
            .filter(|session| session.pid == 0 && session.session_id.contains(&cwd))
            .collect::<Vec<_>>();
        if let [session] = matches.as_slice() {
            result.insert(session.session_id.clone(), child.pid);
        }
    }
    result
}

pub fn collect(state: &PersistedState, preferred_app_pid: Option<u32>) -> MemorySnapshot {
    let rows = process_rows();
    let app_pid = preferred_app_pid.unwrap_or_else(|| app_pid(&rows));
    let app_rows = subtree(&rows, app_pid);
    let main = total(app_rows.iter().copied().filter(|row| row.pid == app_pid));
    let renderer = total(app_rows.iter().copied().filter(|row| {
        row.pid != app_pid && (row.command.contains("WebContent") || row.command.contains("GPU"))
    }));
    let other = total(app_rows.iter().copied().filter(|row| {
        row.pid != app_pid && !row.command.contains("WebContent") && !row.command.contains("GPU")
    }));
    let app_cpu = main.cpu + renderer.cpu + other.cpu;
    let app_memory = main
        .memory
        .saturating_add(renderer.memory)
        .saturating_add(other.memory);
    let app = AppMemory {
        cpu: app_cpu,
        memory: app_memory,
        main,
        renderer,
        other,
        history: push_history("__app__", app_memory),
    };

    let daemon_sessions = suaegi_term::daemon::list_sessions().unwrap_or_default();
    let legacy_pids = legacy_daemon_session_pids(&rows, &daemon_sessions);
    let mut worktree_buckets = HashMap::<String, WorktreeMemory>::new();
    for session in daemon_sessions {
        let pid = if session.pid == 0 {
            suaegi_term::daemon::session_foreground_pgid(&session.session_id)
                .ok()
                .flatten()
                .and_then(|pid| u32::try_from(pid).ok())
                .or_else(|| legacy_pids.get(&session.session_id).copied())
                .unwrap_or_default()
        } else {
            session.pid
        };
        let usage = total(subtree(&rows, pid));
        let worktree = session
            .session_id
            .strip_prefix("worktree:")
            .and_then(|suffix| {
                state
                    .worktrees
                    .iter()
                    .filter(|worktree| suffix.starts_with(&worktree.id.0))
                    .max_by_key(|worktree| worktree.id.0.len())
            });
        let (worktree_id, worktree_name, repo_id, repo_name) = worktree.map_or_else(
            || {
                (
                    "__orphan__".to_string(),
                    "Unattributed terminals".to_string(),
                    "__orphan__".to_string(),
                    "Other".to_string(),
                )
            },
            |worktree| {
                let repo_name = state
                    .repos
                    .iter()
                    .find(|repo| repo.id == worktree.repo_id)
                    .map(|repo| repo.display_name.clone())
                    .unwrap_or_else(|| worktree.repo_id.0.clone());
                (
                    worktree.id.0.clone(),
                    worktree.display_name.clone(),
                    worktree.repo_id.0.clone(),
                    repo_name,
                )
            },
        );
        let bucket = worktree_buckets
            .entry(worktree_id.clone())
            .or_insert_with(|| WorktreeMemory {
                worktree_id,
                worktree_name,
                repo_id,
                repo_name,
                sessions: Vec::new(),
                cpu: 0.0,
                memory: 0,
                history: Vec::new(),
            });
        bucket.cpu += usage.cpu;
        bucket.memory = bucket.memory.saturating_add(usage.memory);
        bucket.sessions.push(SessionMemory {
            session_id: session.session_id,
            pane_key: None,
            pid,
            cpu: usage.cpu,
            memory: usage.memory,
        });
    }
    let mut worktrees = worktree_buckets.into_values().collect::<Vec<_>>();
    worktrees.sort_by(|left, right| left.worktree_name.cmp(&right.worktree_name));
    for worktree in &mut worktrees {
        worktree.history = push_history(&worktree.worktree_id, worktree.memory);
    }
    let terminal_cpu = worktrees.iter().map(|worktree| worktree.cpu).sum::<f64>();
    let terminal_memory = worktrees
        .iter()
        .map(|worktree| worktree.memory)
        .sum::<u64>();
    MemorySnapshot {
        total_cpu: app_cpu + terminal_cpu,
        total_memory: app_memory.saturating_add(terminal_memory),
        app,
        worktrees,
        host: host_memory(),
        process_memory_metric: "rss".into(),
        collected_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
    }
}

pub fn format(snapshot: &MemorySnapshot) -> String {
    let mib = |bytes: u64| bytes as f64 / (1024.0 * 1024.0);
    format!(
        "Suaegi: {:.1} MiB · terminals: {:.1} MiB · total CPU: {:.1}% · host memory: {:.0}%",
        mib(snapshot.app.memory),
        mib(snapshot.total_memory.saturating_sub(snapshot.app.memory)),
        snapshot.total_cpu,
        snapshot.host.memory_usage_percent
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_schema_is_bounded_and_never_reports_negative_values() {
        let snapshot = collect(&PersistedState::default(), Some(u32::MAX));
        assert_eq!(snapshot.app.memory, 0);
        assert!(snapshot.host.memory_usage_percent >= 0.0);
        assert!(snapshot.host.memory_usage_percent <= 100.0);
        assert_eq!(snapshot.process_memory_metric, "rss");
        assert!(snapshot.collected_at > 0);
    }
}
