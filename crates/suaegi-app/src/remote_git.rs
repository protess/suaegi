//! Git operations for SSH recipe workspaces.
//!
//! All user-controlled values remain quoted shell arguments and all status
//! output is base64 wrapped so porcelain-v1's NUL separators survive OpenSSH.

use std::collections::HashMap;

use base64::Engine;
use serde::Deserialize;
use suaegi_git::compare::{
    BranchCompare, ChangeStatus, ChangedFile, CompareHandle, CompareOutcome, FileDiff,
    BINARY_SNIFF_BYTES,
};
use suaegi_git::remote::{normalize_git_error_message, RemoteOp};
use suaegi_git::status::{DetailedFileStatus, FileStatus};

use crate::ephemeral_vm::RecipeSshTarget;
use crate::source_control::SourceControlOperation;
use crate::state::DiffFailure;

const STATUS_OUTPUT_LIMIT: u64 = 16 * 1024 * 1024;
const COMMAND_OUTPUT_LIMIT: u64 = 4 * 1024 * 1024;
const GIT_OUTPUT_LIMIT: usize = suaegi_git::runner::MAX_DIFF_BYTES;
const CAPTURE_RESPONSE_LIMIT: u64 = 9 * 1024 * 1024;
const STATUS_SCRIPT: &str = r#"
import base64, subprocess, sys
result = subprocess.run(["git", "status", "--porcelain=v1", "-z"], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
if result.returncode:
    sys.stderr.buffer.write(result.stderr)
    raise SystemExit(result.returncode)
sys.stdout.write(base64.b64encode(result.stdout).decode("ascii"))
"#;
const CAPTURE_SCRIPT: &str = r#"
import base64, json, subprocess, sys
request = json.load(sys.stdin)
result = subprocess.run(["git"] + request["args"], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
if len(result.stdout) + len(result.stderr) > request["limit"]:
    print('{"tooLarge":true}')
else:
    print(json.dumps({"tooLarge": False, "code": result.returncode, "stdout": base64.b64encode(result.stdout).decode("ascii"), "stderr": base64.b64encode(result.stderr).decode("ascii")}, separators=(",", ":")))
"#;
const HEAD_SCRIPT: &str = r#"
import base64, json, os, sys
request = json.load(sys.stdin)
root = os.path.realpath(os.getcwd())
rel = request["path"]
if not rel or os.path.isabs(rel) or any(p in ("", ".", "..") for p in rel.split("/")):
    raise SystemExit("unsafe relative file")
raw = os.path.join(root, rel)
if os.path.islink(raw):
    data = os.readlink(raw).encode("utf-8")[:8192]
else:
    path = os.path.realpath(raw)
    if not path.startswith(root + os.sep):
        raise SystemExit("file escapes workspace")
    data = open(path, "rb").read(8192)
print(base64.b64encode(data).decode("ascii"))
"#;

#[derive(Debug)]
enum CaptureError {
    Failed(String),
    TooLarge,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CaptureResponse {
    #[serde(default)]
    too_large: bool,
    #[serde(default)]
    code: i32,
    #[serde(default)]
    stdout: String,
    #[serde(default)]
    stderr: String,
}

#[derive(Debug)]
struct Captured {
    code: i32,
    stdout: Vec<u8>,
}

fn shell_quote(value: &str) -> Result<String, String> {
    if value.contains('\0') {
        return Err("Git arguments cannot contain NUL bytes.".to_string());
    }
    Ok(format!("'{}'", value.replace('\'', "'\"'\"'")))
}

fn python_command(script: &str) -> Result<String, String> {
    Ok(format!("python3 -c {}", shell_quote(script)?))
}

fn safe_path(path: &str, allow_dot: bool) -> Result<String, String> {
    if path == "." && allow_dot {
        return Ok(path.to_string());
    }
    if path.is_empty()
        || path.starts_with('/')
        || path
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
        || path.contains(['\\', '\0'])
    {
        return Err("Git file path must stay inside the workspace.".to_string());
    }
    Ok(path.to_string())
}

fn literal_pathspec(path: &str, allow_dot: bool) -> Result<String, String> {
    let path = safe_path(path, allow_dot)?;
    Ok(if path == "." {
        path
    } else {
        format!(":(literal){path}")
    })
}

fn git_command(arguments: &[String]) -> Result<String, String> {
    let mut command = "git".to_string();
    for argument in arguments {
        command.push(' ');
        command.push_str(&shell_quote(argument)?);
    }
    Ok(command)
}

async fn execute(
    target: RecipeSshTarget,
    project_root: String,
    arguments: Vec<String>,
) -> Result<String, String> {
    let command = git_command(&arguments)?;
    crate::ssh::run_recipe_remote_command(target, project_root, command, None, COMMAND_OUTPUT_LIMIT)
        .await
}

async fn capture_git(
    target: RecipeSshTarget,
    project_root: String,
    arguments: Vec<String>,
    allowed_codes: &[i32],
) -> Result<Captured, CaptureError> {
    let payload = serde_json::to_vec(&serde_json::json!({
        "args": arguments,
        "limit": GIT_OUTPUT_LIMIT,
    }))
    .map_err(|error| CaptureError::Failed(error.to_string()))?;
    let output = crate::ssh::run_recipe_remote_command(
        target,
        project_root,
        python_command(CAPTURE_SCRIPT).map_err(CaptureError::Failed)?,
        Some(payload),
        CAPTURE_RESPONSE_LIMIT,
    )
    .await
    .map_err(CaptureError::Failed)?;
    let response: CaptureResponse = serde_json::from_str(&output)
        .map_err(|_| CaptureError::Failed("Remote Git response is invalid.".to_string()))?;
    if response.too_large {
        return Err(CaptureError::TooLarge);
    }
    let stdout = base64::engine::general_purpose::STANDARD
        .decode(response.stdout)
        .map_err(|_| CaptureError::Failed("Remote Git stdout is invalid.".to_string()))?;
    let stderr = base64::engine::general_purpose::STANDARD
        .decode(response.stderr)
        .map_err(|_| CaptureError::Failed("Remote Git stderr is invalid.".to_string()))?;
    if response.code != 0 && !allowed_codes.contains(&response.code) {
        let detail = String::from_utf8_lossy(&stderr).trim().to_string();
        return Err(CaptureError::Failed(if detail.is_empty() {
            format!("Remote Git exited with code {}.", response.code)
        } else {
            suaegi_git::remote::strip_credentials_from_message(&detail)
        }));
    }
    Ok(Captured {
        code: response.code,
        stdout,
    })
}

fn captured_text(bytes: Vec<u8>, label: &str) -> Result<String, CaptureError> {
    String::from_utf8(bytes)
        .map_err(|_| CaptureError::Failed(format!("Remote Git {label} is not valid UTF-8.")))
}

fn compare_failure(error: CaptureError) -> DiffFailure {
    match error {
        CaptureError::TooLarge => DiffFailure::TooLarge {
            limit: GIT_OUTPUT_LIMIT,
        },
        CaptureError::Failed(error) => DiffFailure::Failed(error),
    }
}

fn file_failure(error: CaptureError) -> String {
    match error {
        CaptureError::TooLarge => {
            format!("Remote Git output exceeded {GIT_OUTPUT_LIMIT} bytes.")
        }
        CaptureError::Failed(error) => error,
    }
}

pub async fn status_detailed(
    target: RecipeSshTarget,
    project_root: String,
) -> Result<Vec<DetailedFileStatus>, String> {
    let command = python_command(STATUS_SCRIPT)?;
    let encoded = crate::ssh::run_recipe_remote_command(
        target,
        project_root,
        command,
        None,
        STATUS_OUTPUT_LIMIT,
    )
    .await?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| "Remote Git status was not valid base64.".to_string())?;
    let porcelain = String::from_utf8(bytes)
        .map_err(|_| "Remote Git returned a non-UTF-8 file path.".to_string())?;
    suaegi_git::status::parse_porcelain_details(&porcelain).map_err(|error| error.to_string())
}

pub async fn status_map(
    target: RecipeSshTarget,
    project_root: String,
) -> Result<HashMap<String, FileStatus>, String> {
    let command = python_command(STATUS_SCRIPT)?;
    let encoded = crate::ssh::run_recipe_remote_command(
        target,
        project_root,
        command,
        None,
        STATUS_OUTPUT_LIMIT,
    )
    .await?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| "Remote Git status was not valid base64.".to_string())?;
    let porcelain = String::from_utf8(bytes)
        .map_err(|_| "Remote Git returned a non-UTF-8 file path.".to_string())?;
    suaegi_git::status::parse_porcelain_status(&porcelain).map_err(|error| error.to_string())
}

pub async fn compare_worktree(
    target: RecipeSshTarget,
    project_root: String,
    base_ref: String,
    cancel: CompareHandle,
) -> Result<CompareOutcome, DiffFailure> {
    suaegi_git::refname::validate_user_ref(&base_ref)
        .map_err(|error| DiffFailure::Failed(error.to_string()))?;
    if cancel.is_stopped() {
        return Ok(CompareOutcome::Cancelled);
    }
    let base = capture_git(
        target.clone(),
        project_root.clone(),
        vec![
            "rev-parse".into(),
            "--verify".into(),
            "--quiet".into(),
            base_ref.clone(),
        ],
        &[1],
    )
    .await
    .map_err(compare_failure)?;
    if base.code != 0 {
        return Ok(CompareOutcome::InvalidBase);
    }
    if cancel.is_stopped() {
        return Ok(CompareOutcome::Cancelled);
    }
    let head = capture_git(
        target.clone(),
        project_root.clone(),
        vec![
            "rev-parse".into(),
            "--verify".into(),
            "--quiet".into(),
            "HEAD".into(),
        ],
        &[1],
    )
    .await
    .map_err(compare_failure)?;
    if head.code != 0 {
        return Ok(CompareOutcome::UnbornHead);
    }
    if cancel.is_stopped() {
        return Ok(CompareOutcome::Cancelled);
    }
    let merge_base = capture_git(
        target.clone(),
        project_root.clone(),
        vec!["merge-base".into(), "HEAD".into(), base_ref],
        &[1],
    )
    .await
    .map_err(compare_failure)?;
    if merge_base.code != 0 {
        return Ok(CompareOutcome::NoMergeBase);
    }
    let merge_base = captured_text(merge_base.stdout, "merge base")
        .map_err(compare_failure)?
        .trim()
        .to_string();
    if cancel.is_stopped() {
        return Ok(CompareOutcome::Cancelled);
    }
    let ahead = capture_git(
        target.clone(),
        project_root.clone(),
        vec![
            "rev-list".into(),
            "--count".into(),
            format!("{merge_base}..HEAD"),
        ],
        &[],
    )
    .await
    .map_err(compare_failure)?;
    let ahead_count = captured_text(ahead.stdout, "ahead count")
        .map_err(compare_failure)?
        .trim()
        .parse::<u32>()
        .map_err(|error| DiffFailure::Failed(format!("Invalid remote ahead count: {error}")))?;
    if cancel.is_stopped() {
        return Ok(CompareOutcome::Cancelled);
    }
    let name_status = capture_git(
        target.clone(),
        project_root.clone(),
        vec![
            "diff".into(),
            "--name-status".into(),
            "-z".into(),
            "-M".into(),
            "-C".into(),
            merge_base.clone(),
        ],
        &[],
    )
    .await
    .map_err(compare_failure)?;
    let numstat = capture_git(
        target.clone(),
        project_root.clone(),
        vec![
            "diff".into(),
            "--numstat".into(),
            "-z".into(),
            "-M".into(),
            "-C".into(),
            merge_base.clone(),
        ],
        &[],
    )
    .await
    .map_err(compare_failure)?;
    let name_status =
        captured_text(name_status.stdout, "name-status output").map_err(compare_failure)?;
    let numstat = captured_text(numstat.stdout, "numstat output").map_err(compare_failure)?;
    let counts = suaegi_git::compare::parse_numstat_z(&numstat)
        .map_err(|error| DiffFailure::Failed(error.to_string()))?;
    let mut files = suaegi_git::compare::parse_name_status_z(&name_status, &counts)
        .map_err(|error| DiffFailure::Failed(error.to_string()))?;
    if cancel.is_stopped() {
        return Ok(CompareOutcome::Cancelled);
    }
    for entry in status_detailed(target, project_root)
        .await
        .map_err(DiffFailure::Failed)?
    {
        if entry.status == FileStatus::Untracked && entry.path != ".claude/settings.local.json" {
            files.push(ChangedFile {
                path: entry.path,
                status: ChangeStatus::Added,
                additions: None,
                deletions: None,
            });
        }
    }
    Ok(CompareOutcome::Ready(BranchCompare {
        merge_base,
        ahead_count,
        files,
    }))
}

async fn working_file_head(
    target: RecipeSshTarget,
    project_root: String,
    path: String,
) -> Result<Vec<u8>, CaptureError> {
    safe_path(&path, false).map_err(CaptureError::Failed)?;
    let payload = serde_json::to_vec(&serde_json::json!({ "path": path }))
        .map_err(|error| CaptureError::Failed(error.to_string()))?;
    let output = crate::ssh::run_recipe_remote_command(
        target,
        project_root,
        python_command(HEAD_SCRIPT).map_err(CaptureError::Failed)?,
        Some(payload),
        32 * 1024,
    )
    .await
    .map_err(CaptureError::Failed)?;
    base64::engine::general_purpose::STANDARD
        .decode(output)
        .map_err(|_| CaptureError::Failed("Remote file head is invalid.".to_string()))
}

pub async fn file_diff(
    target: RecipeSshTarget,
    project_root: String,
    base_ref: String,
    path: String,
    status: ChangeStatus,
) -> Result<FileDiff, String> {
    if let ChangeStatus::Other(code) = status {
        return Ok(FileDiff::NonRenderable(code));
    }
    suaegi_git::refname::validate_user_ref(&base_ref).map_err(|error| error.to_string())?;
    let merge_base = capture_git(
        target.clone(),
        project_root.clone(),
        vec!["merge-base".into(), "HEAD".into(), base_ref],
        &[],
    )
    .await
    .map_err(file_failure)?;
    let merge_base = captured_text(merge_base.stdout, "merge base")
        .map_err(file_failure)?
        .trim()
        .to_string();
    let head = if status == ChangeStatus::Deleted {
        let shown = capture_git(
            target.clone(),
            project_root.clone(),
            vec!["show".into(), format!("{merge_base}:{path}")],
            &[],
        )
        .await;
        match shown {
            Ok(shown) => shown.stdout.into_iter().take(BINARY_SNIFF_BYTES).collect(),
            Err(CaptureError::TooLarge) => {
                return Ok(FileDiff::TooLarge {
                    limit: GIT_OUTPUT_LIMIT,
                });
            }
            Err(error) => return Err(file_failure(error)),
        }
    } else {
        working_file_head(target.clone(), project_root.clone(), path.clone())
            .await
            .map_err(file_failure)?
    };
    if head.contains(&0) {
        return Ok(FileDiff::Binary);
    }
    let patch = capture_git(
        target.clone(),
        project_root.clone(),
        vec![
            "diff".into(),
            "-M".into(),
            "-C".into(),
            merge_base,
            "--".into(),
            literal_pathspec(&path, false)?,
        ],
        &[],
    )
    .await;
    let patch = match patch {
        Ok(patch) => captured_text(patch.stdout, "patch").map_err(file_failure)?,
        Err(CaptureError::TooLarge) => {
            return Ok(FileDiff::TooLarge {
                limit: GIT_OUTPUT_LIMIT,
            });
        }
        Err(error) => return Err(file_failure(error)),
    };
    if !patch.is_empty() {
        return Ok(FileDiff::Patch(patch));
    }
    let untracked = status_detailed(target.clone(), project_root.clone())
        .await?
        .into_iter()
        .any(|entry| entry.path == path && entry.status == FileStatus::Untracked);
    if !untracked {
        return Ok(FileDiff::Patch(patch));
    }
    let patch = capture_git(
        target,
        project_root,
        vec![
            "diff".into(),
            "--no-index".into(),
            "--".into(),
            "/dev/null".into(),
            safe_path(&path, false)?,
        ],
        &[1],
    )
    .await;
    match patch {
        Ok(patch) => captured_text(patch.stdout, "untracked patch")
            .map(FileDiff::Patch)
            .map_err(file_failure),
        Err(CaptureError::TooLarge) => Ok(FileDiff::TooLarge {
            limit: GIT_OUTPUT_LIMIT,
        }),
        Err(error) => Err(file_failure(error)),
    }
}

pub async fn run_operation(
    target: RecipeSshTarget,
    project_root: String,
    operation: SourceControlOperation,
) -> Result<String, String> {
    match operation {
        SourceControlOperation::Stage(path) => {
            let pathspec = literal_pathspec(&path, true)?;
            execute(
                target,
                project_root,
                vec!["add".into(), "--".into(), pathspec],
            )
            .await?;
            Ok(format!("Staged {path}"))
        }
        SourceControlOperation::Unstage(path) => {
            let pathspec = literal_pathspec(&path, false)?;
            execute(
                target,
                project_root,
                vec!["restore".into(), "--staged".into(), "--".into(), pathspec],
            )
            .await?;
            Ok(format!("Unstaged {path}"))
        }
        SourceControlOperation::Discard(path) => {
            let pathspec = literal_pathspec(&path, false)?;
            let entries = status_detailed(target.clone(), project_root.clone()).await?;
            if entries
                .iter()
                .any(|entry| entry.path == path && entry.status == FileStatus::Untracked)
            {
                execute(
                    target,
                    project_root,
                    vec!["clean".into(), "-ffdx".into(), "--".into(), pathspec],
                )
                .await?;
                Ok(format!("Removed untracked {path}"))
            } else {
                execute(
                    target,
                    project_root,
                    vec![
                        "restore".into(),
                        "--worktree".into(),
                        "--source=HEAD".into(),
                        "--".into(),
                        pathspec,
                    ],
                )
                .await?;
                Ok(format!("Restored {path} from HEAD"))
            }
        }
        SourceControlOperation::Commit(message) => {
            let message = message.trim();
            if message.is_empty() || message.len() > 100_000 {
                return Err("Commit message is empty or too large.".to_string());
            }
            execute(
                target,
                project_root,
                vec!["commit".into(), "-m".into(), message.to_string()],
            )
            .await?;
            Ok("Commit created".to_string())
        }
        SourceControlOperation::Fetch => {
            execute(target, project_root, vec!["fetch".into(), "--prune".into()])
                .await
                .map(|_| "Fetch completed".to_string())
                .map_err(|error| normalize_git_error_message(&error, RemoteOp::Fetch))
        }
        SourceControlOperation::Pull => execute(
            target,
            project_root,
            vec!["pull".into(), "--ff-only".into()],
        )
        .await
        .map(|_| "Pull completed".to_string())
        .map_err(|error| normalize_git_error_message(&error, RemoteOp::Pull)),
        SourceControlOperation::Push { branch } => {
            if branch.trim().is_empty() || branch.len() > 1024 {
                return Err("The branch name is invalid.".to_string());
            }
            execute(
                target,
                project_root,
                vec![
                    "push".into(),
                    "--set-upstream".into(),
                    "origin".into(),
                    branch,
                ],
            )
            .await
            .map(|_| "Push completed".to_string())
            .map_err(|error| normalize_git_error_message(&error, RemoteOp::Push))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_git_quotes_arguments_and_rejects_traversal() {
        assert_eq!(
            git_command(&["commit".into(), "-m".into(), "it's done; rm -rf /".into()]).unwrap(),
            "git 'commit' '-m' 'it'\"'\"'s done; rm -rf /'"
        );
        assert!(literal_pathspec("../secret", false).is_err());
        assert!(literal_pathspec("/etc/passwd", false).is_err());
        assert_eq!(
            literal_pathspec("src/a[1].rs", false).unwrap(),
            ":(literal)src/a[1].rs"
        );
    }
}
