//! Bounded SSH filesystem operations for recipe-owned workspaces.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use serde::Deserialize;
use suaegi_git::fs::{FileSignature, WriteOutcome, MAX_TEXT_FILE_SIZE};

use crate::editor::EditorLoad;
use crate::ephemeral_vm::RecipeSshTarget;
use crate::file_explorer::ExplorerEntry;

const DIRECTORY_OUTPUT_LIMIT: u64 = 8 * 1024 * 1024;
const EDITOR_OUTPUT_LIMIT: u64 = 70 * 1024 * 1024;
const STAT_OUTPUT_LIMIT: u64 = 16 * 1024;

const LIST_SCRIPT: &str = r#"
import json, os, sys
root = os.path.realpath(sys.argv[1])
rel = sys.argv[2]
if os.path.isabs(rel) or any(p in ("", ".", "..") for p in rel.split("/") if rel):
    raise SystemExit("unsafe relative directory")
path = root if not rel else os.path.realpath(os.path.join(root, rel))
if path != root and not path.startswith(root + os.sep):
    raise SystemExit("directory escapes workspace")
rows = []
for item in os.scandir(path):
    try:
        rows.append({"name": item.name, "isDir": item.is_dir(follow_symlinks=False), "isSymlink": item.is_symlink()})
    except OSError:
        pass
rows.sort(key=lambda row: (not row["isDir"], row["name"]))
print(json.dumps(rows, ensure_ascii=False, separators=(",", ":")))
"#;

const READ_SCRIPT: &str = r#"
import base64, json, os, sys
root = os.path.realpath(sys.argv[1])
rel = sys.argv[2]
limit = int(sys.argv[3])
if not rel or os.path.isabs(rel) or any(p in ("", ".", "..") for p in rel.split("/")):
    raise SystemExit("unsafe relative file")
raw = os.path.join(root, rel)
if os.path.islink(raw):
    raise SystemExit("symbolic links cannot be edited")
path = os.path.realpath(raw)
if not path.startswith(root + os.sep):
    raise SystemExit("file escapes workspace")
stat = os.stat(path)
if stat.st_size > limit:
    print(json.dumps({"kind": "tooLarge", "limit": limit}))
    raise SystemExit(0)
data = open(path, "rb").read()
try:
    text = data.decode("utf-8")
    binary = b"\0" in data[:8192]
except UnicodeDecodeError:
    binary = True
if binary:
    print(json.dumps({"kind": "binary", "size": stat.st_size, "data": base64.b64encode(data).decode("ascii")}, separators=(",", ":")))
else:
    print(json.dumps({"kind": "ready", "size": stat.st_size, "mtimeNs": stat.st_mtime_ns, "data": base64.b64encode(data).decode("ascii")}, separators=(",", ":")))
"#;

const STAT_SCRIPT: &str = r#"
import json, os, sys
root = os.path.realpath(sys.argv[1])
rel = sys.argv[2]
if not rel or os.path.isabs(rel) or any(p in ("", ".", "..") for p in rel.split("/")):
    raise SystemExit("unsafe relative file")
raw = os.path.join(root, rel)
if os.path.islink(raw):
    raise SystemExit("symbolic links cannot be edited")
path = os.path.realpath(raw)
if not path.startswith(root + os.sep):
    raise SystemExit("file escapes workspace")
stat = os.stat(path)
print(json.dumps({"size": stat.st_size, "mtimeNs": stat.st_mtime_ns}, separators=(",", ":")))
"#;

const WRITE_SCRIPT: &str = r#"
import base64, json, os, secrets, sys
root = os.path.realpath(sys.argv[1])
rel = sys.argv[2]
if not rel or os.path.isabs(rel) or any(p in ("", ".", "..") for p in rel.split("/")):
    raise SystemExit("unsafe relative file")
raw = os.path.join(root, rel)
if os.path.islink(raw):
    raise SystemExit("symbolic links cannot be edited")
path = os.path.realpath(raw)
if not path.startswith(root + os.sep):
    raise SystemExit("file escapes workspace")
payload = json.load(sys.stdin)
stat = os.stat(path)
if stat.st_size != payload["expectedSize"] or stat.st_mtime_ns != payload["expectedMtimeNs"]:
    print(json.dumps({"kind": "stale", "size": stat.st_size, "mtimeNs": stat.st_mtime_ns}, separators=(",", ":")))
    raise SystemExit(0)
data = base64.b64decode(payload["data"], validate=True)
if len(data) > 52428800:
    raise SystemExit("file exceeds editor limit")
temporary = path + ".suaegi-" + secrets.token_hex(8)
fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, stat.st_mode & 0o777)
try:
    with os.fdopen(fd, "wb") as output:
        output.write(data)
        output.flush()
        os.fsync(output.fileno())
    os.replace(temporary, path)
finally:
    try:
        os.unlink(temporary)
    except FileNotFoundError:
        pass
next_stat = os.stat(path)
print(json.dumps({"kind": "written", "size": next_stat.st_size, "mtimeNs": next_stat.st_mtime_ns}, separators=(",", ":")))
"#;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteDirectoryEntry {
    name: String,
    is_dir: bool,
    is_symlink: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteStat {
    size: u64,
    mtime_ns: u64,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum RemoteRead {
    Ready {
        data: String,
        size: u64,
        mtime_ns: u64,
    },
    Binary {
        data: String,
        size: u64,
    },
    TooLarge {
        limit: u64,
    },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum RemoteWrite {
    Written { size: u64, mtime_ns: u64 },
    Stale { size: u64, mtime_ns: u64 },
}

fn python_command(script: &str, arguments: &[&str]) -> Result<String, String> {
    fn quote(value: &str) -> Result<String, String> {
        if value.contains('\0') {
            return Err("Remote command arguments cannot contain NUL bytes.".to_string());
        }
        Ok(format!("'{}'", value.replace('\'', "'\"'\"'")))
    }
    let mut command = format!("python3 -c {}", quote(script)?);
    for argument in arguments {
        command.push(' ');
        command.push_str(&quote(argument)?);
    }
    Ok(command)
}

fn joined_path(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

fn signature(size: u64, mtime_ns: u64) -> FileSignature {
    FileSignature {
        size,
        mtime: UNIX_EPOCH
            .checked_add(Duration::from_nanos(mtime_ns))
            .unwrap_or(SystemTime::UNIX_EPOCH),
    }
}

fn signature_nanos(signature: &FileSignature) -> Result<u64, String> {
    let nanos = signature
        .mtime
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "Remote file timestamp is before the Unix epoch.".to_string())?
        .as_nanos();
    u64::try_from(nanos).map_err(|_| "Remote file timestamp is out of range.".to_string())
}

pub async fn list_directory(
    target: RecipeSshTarget,
    project_root: String,
    directory: String,
) -> Result<Vec<ExplorerEntry>, String> {
    let command = python_command(LIST_SCRIPT, &[&project_root, &directory])?;
    let output = crate::ssh::run_recipe_remote_command(
        target,
        project_root,
        command,
        None,
        DIRECTORY_OUTPUT_LIMIT,
    )
    .await?;
    let entries: Vec<RemoteDirectoryEntry> = serde_json::from_str(&output)
        .map_err(|_| "Remote directory response is invalid.".to_string())?;
    if entries.len() > 100_000 {
        return Err("Remote directory contains too many entries.".to_string());
    }
    Ok(entries
        .into_iter()
        .filter(|entry| !suaegi_git::status::HARDCODED_HIDES.contains(&entry.name.as_str()))
        .map(|entry| ExplorerEntry {
            path: joined_path(&directory, &entry.name),
            name: entry.name,
            is_dir: entry.is_dir,
            is_symlink: entry.is_symlink,
            ignored: false,
        })
        .collect())
}

pub async fn read_file(
    target: RecipeSshTarget,
    project_root: String,
    path: String,
) -> Result<EditorLoad, String> {
    let command = python_command(
        READ_SCRIPT,
        &[&project_root, &path, &MAX_TEXT_FILE_SIZE.to_string()],
    )?;
    let output = crate::ssh::run_recipe_remote_command(
        target,
        project_root,
        command,
        None,
        EDITOR_OUTPUT_LIMIT,
    )
    .await?;
    match serde_json::from_str::<RemoteRead>(&output)
        .map_err(|_| "Remote file response is invalid.".to_string())?
    {
        RemoteRead::Ready {
            data,
            size,
            mtime_ns,
        } => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(data)
                .map_err(|_| "Remote file content is not valid base64.".to_string())?;
            let text = String::from_utf8(bytes)
                .map_err(|_| "Remote file content is not valid UTF-8.".to_string())?;
            Ok(EditorLoad::Ready {
                text,
                size,
                signature: signature(size, mtime_ns),
            })
        }
        RemoteRead::Binary { data, size } => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(data)
                .map_err(|_| "Remote binary content is not valid base64.".to_string())?;
            Ok(crate::editor::binary_preview_async(path, bytes, size).await)
        }
        RemoteRead::TooLarge { limit } => Ok(EditorLoad::TooLarge { limit }),
    }
}

pub async fn file_signature(
    target: RecipeSshTarget,
    project_root: String,
    path: String,
) -> Result<FileSignature, String> {
    let command = python_command(STAT_SCRIPT, &[&project_root, &path])?;
    let output = crate::ssh::run_recipe_remote_command(
        target,
        project_root,
        command,
        None,
        STAT_OUTPUT_LIMIT,
    )
    .await?;
    let stat: RemoteStat = serde_json::from_str(&output)
        .map_err(|_| "Remote file stat response is invalid.".to_string())?;
    Ok(signature(stat.size, stat.mtime_ns))
}

pub async fn write_file(
    target: RecipeSshTarget,
    project_root: String,
    path: String,
    text: String,
    expected: FileSignature,
) -> Result<WriteOutcome, String> {
    if text.len() as u64 > MAX_TEXT_FILE_SIZE {
        return Err("Remote file exceeds the 50 MB editor limit.".to_string());
    }
    let payload = serde_json::to_vec(&serde_json::json!({
        "data": base64::engine::general_purpose::STANDARD.encode(text.as_bytes()),
        "expectedSize": expected.size,
        "expectedMtimeNs": signature_nanos(&expected)?
    }))
    .map_err(|error| format!("Could not encode remote file update: {error}"))?;
    let command = python_command(WRITE_SCRIPT, &[&project_root, &path])?;
    let output = crate::ssh::run_recipe_remote_command(
        target,
        project_root,
        command,
        Some(payload),
        1024 * 1024,
    )
    .await?;
    match serde_json::from_str::<RemoteWrite>(&output)
        .map_err(|_| "Remote write response is invalid.".to_string())?
    {
        RemoteWrite::Written { size, mtime_ns } => Ok(WriteOutcome::Written {
            signature: signature(size, mtime_ns),
        }),
        RemoteWrite::Stale { size, mtime_ns } => Ok(WriteOutcome::StaleConflict {
            disk: Some(signature(size, mtime_ns)),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_arguments_are_single_quoted_and_nul_is_rejected() {
        let command = python_command("print('ok')", &["O'Brien"]).unwrap();
        assert!(command.contains("'O'\"'\"'Brien'"));
        assert!(python_command("x", &["bad\0value"]).is_err());
    }

    #[test]
    fn remote_signatures_round_trip_nanoseconds() {
        let original = FileSignature {
            size: 7,
            mtime: UNIX_EPOCH + Duration::from_nanos(123_456),
        };
        assert_eq!(
            signature(original.size, signature_nanos(&original).unwrap()),
            original
        );
    }
}
