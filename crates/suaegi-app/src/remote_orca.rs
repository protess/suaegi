//! Request/response adapters for recipe-owned Orca Server runtimes.

use std::collections::HashMap;
use std::time::{Duration, UNIX_EPOCH};

use base64::Engine as _;
use suaegi_core::domain::RuntimeEnvironmentSetting;
use suaegi_git::compare::{BranchCompare, ChangeStatus, ChangedFile, CompareOutcome, FileDiff};
use suaegi_git::fs::{FileSignature, WriteOutcome, MAX_TEXT_FILE_SIZE};
use suaegi_git::status::{ConflictKind, DetailedFileStatus, FileStatus};
use suaegi_search::{SearchOptions, SearchResult};

use crate::editor::EditorLoad;
use crate::file_explorer::ExplorerEntry;
use crate::source_control::SourceControlOperation;
use crate::state::DiffFailure;

const RPC_TIMEOUT: Duration = Duration::from_secs(30);

async fn call(
    environment: RuntimeEnvironmentSetting,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    crate::remote_runtime::request(environment, method, params, RPC_TIMEOUT).await
}

fn signature(size: u64, mtime_ms: f64) -> Result<FileSignature, String> {
    if !mtime_ms.is_finite() || mtime_ms < 0.0 {
        return Err("Remote file timestamp is invalid.".to_string());
    }
    let nanos = (mtime_ms * 1_000_000.0).round();
    let nanos = u64::try_from(nanos as u128)
        .map_err(|_| "Remote file timestamp is out of range.".to_string())?;
    Ok(FileSignature {
        size,
        mtime: UNIX_EPOCH + Duration::from_nanos(nanos),
        change_marker: None,
        content_hash: None,
    })
}

async fn stat(
    environment: RuntimeEnvironmentSetting,
    worktree: &str,
    path: &str,
) -> Result<FileSignature, String> {
    let value = call(
        environment,
        "files.stat",
        serde_json::json!({"worktree": worktree, "relativePath": path}),
    )
    .await?;
    let size = value
        .get("size")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "Remote file stat is missing size.".to_string())?;
    let mtime = value
        .get("mtime")
        .and_then(serde_json::Value::as_f64)
        .ok_or_else(|| "Remote file stat is missing mtime.".to_string())?;
    signature(size, mtime)
}

pub async fn file_signature(
    environment: RuntimeEnvironmentSetting,
    worktree: String,
    path: String,
) -> Result<FileSignature, String> {
    stat(environment, &worktree, &path).await
}

pub async fn list_directory(
    environment: RuntimeEnvironmentSetting,
    worktree: String,
    directory: String,
) -> Result<Vec<ExplorerEntry>, String> {
    let value = call(
        environment,
        "files.readDir",
        serde_json::json!({"worktree": worktree, "relativePath": directory}),
    )
    .await?;
    let rows = value
        .as_array()
        .ok_or_else(|| "Remote directory response is invalid.".to_string())?;
    if rows.len() > 100_000 {
        return Err("Remote directory contains too many entries.".to_string());
    }
    let mut entries = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(name) = row.get("name").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if suaegi_git::status::HARDCODED_HIDES.contains(&name) {
            continue;
        }
        entries.push(ExplorerEntry {
            path: if directory.is_empty() {
                name.to_string()
            } else {
                format!("{directory}/{name}")
            },
            name: name.to_string(),
            is_dir: row
                .get("isDirectory")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            is_symlink: row
                .get("isSymlink")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            ignored: false,
        });
    }
    Ok(entries)
}

pub async fn read_file(
    environment: RuntimeEnvironmentSetting,
    worktree: String,
    path: String,
) -> Result<EditorLoad, String> {
    let disk = stat(environment.clone(), &worktree, &path).await?;
    if disk.size > MAX_TEXT_FILE_SIZE {
        return Ok(EditorLoad::TooLarge {
            limit: MAX_TEXT_FILE_SIZE,
        });
    }
    let value = call(
        environment,
        "files.readPreview",
        serde_json::json!({"worktree": worktree, "relativePath": path}),
    )
    .await?;
    if value
        .get("isBinary")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        let Some(content) = value.get("content").and_then(serde_json::Value::as_str) else {
            return Ok(EditorLoad::Binary { size: disk.size });
        };
        if content.is_empty() {
            return Ok(EditorLoad::Binary { size: disk.size });
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(content)
            .map_err(|_| "Remote binary preview is not valid base64.".to_string())?;
        return Ok(crate::editor::binary_preview_async(path, bytes, disk.size).await);
    }
    let text = value
        .get("content")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "Remote file response is missing content.".to_string())?
        .to_string();
    Ok(EditorLoad::Ready {
        text,
        size: disk.size,
        signature: disk,
    })
}

pub async fn write_file(
    environment: RuntimeEnvironmentSetting,
    worktree: String,
    path: String,
    text: String,
    expected: FileSignature,
) -> Result<WriteOutcome, String> {
    if text.len() as u64 > MAX_TEXT_FILE_SIZE {
        return Err("Remote file exceeds the 50 MB editor limit.".to_string());
    }
    let disk = stat(environment.clone(), &worktree, &path).await?;
    if disk != expected {
        return Ok(WriteOutcome::StaleConflict { disk: Some(disk) });
    }
    call(
        environment.clone(),
        "files.write",
        serde_json::json!({
            "worktree": worktree,
            "relativePath": path,
            "content": text
        }),
    )
    .await?;
    Ok(WriteOutcome::Written {
        signature: stat(environment, &worktree, &path).await?,
    })
}

pub async fn list_files(
    environment: RuntimeEnvironmentSetting,
    worktree: String,
) -> Result<Vec<String>, String> {
    let value = call(
        environment,
        "files.listAll",
        serde_json::json!({"worktree": worktree, "excludePaths": []}),
    )
    .await?;
    serde_json::from_value(value).map_err(|_| "Remote Quick Open response is invalid.".to_string())
}

pub async fn search(
    environment: RuntimeEnvironmentSetting,
    worktree: String,
    options: SearchOptions,
) -> Result<SearchResult, String> {
    let value = call(
        environment,
        "files.search",
        serde_json::json!({
            "worktree": worktree,
            "query": options.query,
            "caseSensitive": options.case_sensitive,
            "wholeWord": options.whole_word,
            "useRegex": options.use_regex,
            "includePattern": options.include_pattern,
            "excludePattern": options.exclude_pattern,
            "maxResults": options.max_results
        }),
    )
    .await?;
    serde_json::from_value(value)
        .map(suaegi_search::normalize_search_result)
        .map_err(|_| "Remote content-search response is invalid.".to_string())
}

fn conflict_kind(value: &str) -> Option<ConflictKind> {
    Some(match value {
        "both_modified" => ConflictKind::BothModified,
        "both_added" => ConflictKind::BothAdded,
        "both_deleted" => ConflictKind::BothDeleted,
        "added_by_us" => ConflictKind::AddedByUs,
        "added_by_them" => ConflictKind::AddedByThem,
        "deleted_by_us" => ConflictKind::DeletedByUs,
        "deleted_by_them" => ConflictKind::DeletedByThem,
        _ => return None,
    })
}

fn file_status(row: &serde_json::Value) -> FileStatus {
    if let Some(kind) = row
        .get("conflictKind")
        .and_then(serde_json::Value::as_str)
        .and_then(conflict_kind)
    {
        return FileStatus::Conflicted(kind);
    }
    match row
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
    {
        "modified" => FileStatus::Modified,
        "added" => FileStatus::Added,
        "deleted" => FileStatus::Deleted,
        "renamed" => FileStatus::Renamed {
            from: row
                .get("oldPath")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
        },
        "copied" => FileStatus::Copied {
            from: row
                .get("oldPath")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
        },
        "untracked" => FileStatus::Untracked,
        other => FileStatus::Other(other.to_string()),
    }
}

pub async fn status_detailed(
    environment: RuntimeEnvironmentSetting,
    worktree: String,
) -> Result<Vec<DetailedFileStatus>, String> {
    let value = call(
        environment,
        "git.status",
        serde_json::json!({"worktree": worktree}),
    )
    .await?;
    let rows = value
        .get("entries")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "Remote Git status response is invalid.".to_string())?;
    let mut entries: HashMap<String, DetailedFileStatus> = HashMap::new();
    for row in rows {
        let Some(path) = row.get("path").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let area = row
            .get("area")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unstaged");
        let status = file_status(row);
        entries
            .entry(path.to_string())
            .and_modify(|entry| {
                entry.staged |= area == "staged";
                entry.unstaged |= area != "staged";
                if matches!(status, FileStatus::Conflicted(_)) {
                    entry.status = status.clone();
                }
            })
            .or_insert(DetailedFileStatus {
                path: path.to_string(),
                status,
                staged: area == "staged",
                unstaged: area != "staged",
            });
    }
    let mut entries = entries.into_values().collect::<Vec<_>>();
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(entries)
}

pub async fn status_map(
    environment: RuntimeEnvironmentSetting,
    worktree: String,
) -> Result<HashMap<String, FileStatus>, String> {
    Ok(status_detailed(environment, worktree)
        .await?
        .into_iter()
        .map(|entry| (entry.path, entry.status))
        .collect())
}

pub async fn run_operation(
    environment: RuntimeEnvironmentSetting,
    worktree: String,
    operation: SourceControlOperation,
) -> Result<String, String> {
    let (method, params, notice) = match operation {
        SourceControlOperation::Stage(path) => (
            "git.stage",
            serde_json::json!({"worktree": worktree, "filePath": path}),
            format!("Staged {path}"),
        ),
        SourceControlOperation::Unstage(path) => (
            "git.unstage",
            serde_json::json!({"worktree": worktree, "filePath": path}),
            format!("Unstaged {path}"),
        ),
        SourceControlOperation::Discard(path) => (
            "git.discard",
            serde_json::json!({"worktree": worktree, "filePath": path}),
            format!("Discarded {path}"),
        ),
        SourceControlOperation::Commit(message) => (
            "git.commit",
            serde_json::json!({"worktree": worktree, "message": message}),
            "Commit created".to_string(),
        ),
        SourceControlOperation::Fetch => (
            "git.fetch",
            serde_json::json!({"worktree": worktree}),
            "Fetch completed".to_string(),
        ),
        SourceControlOperation::Pull => (
            "git.pull",
            serde_json::json!({"worktree": worktree}),
            "Pull completed".to_string(),
        ),
        SourceControlOperation::Push { .. } => (
            "git.push",
            serde_json::json!({"worktree": worktree, "publish": true}),
            "Push completed".to_string(),
        ),
    };
    call(environment, method, params).await?;
    Ok(notice)
}

fn change_status(value: &serde_json::Value) -> ChangeStatus {
    match value
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
    {
        "modified" => ChangeStatus::Modified,
        "added" => ChangeStatus::Added,
        "deleted" => ChangeStatus::Deleted,
        "renamed" => ChangeStatus::Renamed {
            from: value
                .get("oldPath")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
        },
        "copied" => ChangeStatus::Copied {
            from: value
                .get("oldPath")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
        },
        other => ChangeStatus::Other(other.chars().next().unwrap_or('?')),
    }
}

pub async fn compare_worktree(
    environment: RuntimeEnvironmentSetting,
    worktree: String,
    base_ref: String,
) -> Result<CompareOutcome, DiffFailure> {
    let value = call(
        environment,
        "git.branchCompare",
        serde_json::json!({"worktree": worktree, "baseRef": base_ref}),
    )
    .await
    .map_err(DiffFailure::Failed)?;
    let summary = value
        .get("summary")
        .ok_or_else(|| DiffFailure::Failed("Remote comparison is invalid.".to_string()))?;
    match summary
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("error")
    {
        "invalid-base" => return Ok(CompareOutcome::InvalidBase),
        "unborn-head" => return Ok(CompareOutcome::UnbornHead),
        "no-merge-base" => return Ok(CompareOutcome::NoMergeBase),
        "ready" => {}
        _ => {
            return Err(DiffFailure::Failed(
                summary
                    .get("errorMessage")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("Remote comparison failed.")
                    .to_string(),
            ));
        }
    }
    let merge_base = summary
        .get("mergeBase")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| DiffFailure::Failed("Remote comparison has no merge base.".to_string()))?
        .to_string();
    let ahead_count = summary
        .get("commitsAhead")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(0);
    let files = value
        .get("entries")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| DiffFailure::Failed("Remote comparison entries are invalid.".to_string()))?
        .iter()
        .filter_map(|row| {
            Some(ChangedFile {
                path: row.get("path")?.as_str()?.to_string(),
                status: change_status(row),
                additions: row
                    .get("added")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok()),
                deletions: row
                    .get("removed")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok()),
            })
        })
        .collect();
    Ok(CompareOutcome::Ready(BranchCompare {
        merge_base,
        ahead_count,
        files,
    }))
}

fn unified_patch(path: &str, original: &str, modified: &str) -> String {
    if original == modified {
        return String::new();
    }
    let old = original.lines().collect::<Vec<_>>();
    let new = modified.lines().collect::<Vec<_>>();
    let prefix = old
        .iter()
        .zip(&new)
        .take_while(|(left, right)| left == right)
        .count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(left, right)| left == right)
        .count();
    let context_start = prefix.saturating_sub(3);
    let old_end = old.len().saturating_sub(suffix);
    let new_end = new.len().saturating_sub(suffix);
    let suffix_end_old = (old_end + 3).min(old.len());
    let suffix_end_new = (new_end + 3).min(new.len());
    let mut patch = format!(
        "--- a/{path}\n+++ b/{path}\n@@ -{},{} +{},{} @@\n",
        context_start + 1,
        suffix_end_old - context_start,
        context_start + 1,
        suffix_end_new - context_start
    );
    for line in &old[context_start..prefix] {
        patch.push_str(&format!(" {line}\n"));
    }
    for line in &old[prefix..old_end] {
        patch.push_str(&format!("-{line}\n"));
    }
    for line in &new[prefix..new_end] {
        patch.push_str(&format!("+{line}\n"));
    }
    for line in &old[old_end..suffix_end_old] {
        patch.push_str(&format!(" {line}\n"));
    }
    patch
}

pub async fn file_diff(
    environment: RuntimeEnvironmentSetting,
    worktree: String,
    base_ref: String,
    path: String,
    status: ChangeStatus,
) -> Result<FileDiff, String> {
    if let ChangeStatus::Other(code) = status {
        return Ok(FileDiff::NonRenderable(code));
    }
    let comparison = call(
        environment.clone(),
        "git.branchCompare",
        serde_json::json!({"worktree": worktree, "baseRef": base_ref}),
    )
    .await?;
    let summary = comparison
        .get("summary")
        .ok_or_else(|| "Remote comparison is invalid.".to_string())?;
    let compare = serde_json::json!({
        "baseRef": summary.get("baseRef"),
        "baseOid": summary.get("baseOid"),
        "headOid": summary.get("headOid"),
        "mergeBase": summary.get("mergeBase")
    });
    let value = call(
        environment,
        "git.branchDiff",
        serde_json::json!({
            "worktree": worktree,
            "compare": compare,
            "filePath": path
        }),
    )
    .await?;
    if value.get("kind").and_then(serde_json::Value::as_str) == Some("binary") {
        return Ok(FileDiff::Binary);
    }
    let original = value
        .get("originalContent")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let modified = value
        .get("modifiedContent")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    Ok(FileDiff::Patch(unified_patch(&path, original, modified)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unified_patch_keeps_context_and_changed_lines() {
        let patch = unified_patch("a.txt", "same\nold\ntail\n", "same\nnew\ntail\n");
        assert!(patch.contains(" same"));
        assert!(patch.contains("-old"));
        assert!(patch.contains("+new"));
        assert!(patch.contains(" tail"));
    }

    #[test]
    fn runtime_conflict_rows_preserve_conflict_kind() {
        assert!(matches!(
            file_status(&serde_json::json!({
                "status": "modified",
                "conflictKind": "both_modified"
            })),
            FileStatus::Conflicted(ConflictKind::BothModified)
        ));
    }
}
