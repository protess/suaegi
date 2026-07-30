//! Orca-compatible per-workspace environment recipe execution and lifecycle store.
//!
//! Recipe commands are deliberately shell commands because that is the public
//! Orca recipe contract. They run in their own process group, receive only the
//! documented `ORCA_*` context, have bounded output, and are terminated as a
//! group on timeout so a provisioning CLI cannot be orphaned.

use std::fs;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use wait_timeout::ChildExt;

use crate::plugin_content::VmRecipe;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const MAX_CAPTURE_BYTES: usize = 1024 * 1024;
const MAX_STORE_BYTES: u64 = 1024 * 1024;
const MAX_RUNTIMES: usize = 256;
static INSTANCE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecipeContext {
    pub instance_id: Option<String>,
    pub recipe_id: String,
    pub project_id: Option<String>,
    pub workspace_id: Option<String>,
    pub workspace_name: Option<String>,
    pub repo_path: PathBuf,
    pub repo_url: Option<String>,
    pub branch: Option<String>,
    pub git_ref: Option<String>,
    pub orca_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedPortForward {
    pub local_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipeSshTarget {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_host: Option<String>,
    pub host: String,
    pub port: u16,
    pub username: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identities_only: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jump_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_grace_period_seconds: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port_forwards: Option<Vec<SavedPortForward>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum RecipeConnection {
    #[serde(rename_all = "camelCase")]
    OrcaServer {
        pairing_code: String,
        project_root: String,
    },
    #[serde(rename_all = "camelCase")]
    Ssh {
        target: Box<RecipeSshTarget>,
        project_root: String,
    },
}

impl RecipeConnection {
    pub fn project_root(&self) -> &str {
        match self {
            Self::OrcaServer { project_root, .. } | Self::Ssh { project_root, .. } => project_root,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyRecipeResult {
    schema_version: u8,
    pairing_code: String,
    project_root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    user_data: Option<serde_json::Map<String, Value>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionRecipeResult {
    schema_version: u8,
    connection: RecipeConnection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    user_data: Option<serde_json::Map<String, Value>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RecipeResult {
    Connection {
        #[serde(flatten)]
        result: Box<ConnectionRecipeResult>,
    },
    Legacy {
        #[serde(flatten)]
        result: Box<LegacyRecipeResult>,
    },
}

impl RecipeResult {
    pub fn connection(&self) -> RecipeConnection {
        match self {
            Self::Connection { result } => result.connection.clone(),
            Self::Legacy { result } => RecipeConnection::OrcaServer {
                pairing_code: result.pairing_code.clone(),
                project_root: result.project_root.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StartSuccess {
    pub context: RecipeContext,
    pub result: RecipeResult,
    pub process: ProcessResult,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleResult {
    pub skipped: bool,
    pub process: ProcessResult,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStatus {
    Provisioning,
    Running,
    Suspended,
    SuspendFailed,
    ResumeFailed,
    Failed,
    CleanupPending,
    CleanupFailed,
    Cleaned,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupStatus {
    NotStarted,
    Disabled,
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeRecord {
    pub id: String,
    pub recipe_id: String,
    pub recipe: VmRecipe,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_name: Option<String>,
    pub status: RuntimeStatus,
    pub cleanup_status: CleanupStatus,
    pub cleanup_disabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_last_attempt_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_last_error: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    pub recipe_result: RecipeResult,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_environment: Option<suaegi_core::domain::RuntimeEnvironmentSetting>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pairing_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeStore {
    version: u8,
    runtimes: Vec<RuntimeRecord>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LifecyclePayload<'a> {
    schema_version: u8,
    mode: &'a str,
    recipe_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    instance_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    project_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    workspace_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    workspace_name: Option<&'a str>,
    recipe_result: &'a RecipeResult,
}

pub fn default_store_path() -> PathBuf {
    crate::persistence_thread::default_data_file()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("suaegi-ephemeral-vm-runtimes.json")
}

pub async fn start(
    recipe: VmRecipe,
    repo_path: PathBuf,
    mut context: RecipeContext,
) -> Result<StartSuccess, String> {
    tokio::task::spawn_blocking(move || {
        validate_repo_path(&repo_path)?;
        context.recipe_id.clone_from(&recipe.id);
        context.repo_path = repo_path;
        if context.instance_id.is_none() {
            context.instance_id = Some(new_instance_id());
        }
        let process =
            run_recipe_command(&recipe.create, "create", &context, None, DEFAULT_TIMEOUT)?;
        ensure_success("Recipe", &process)?;
        let result = parse_result(&process.stdout)?;
        Ok(StartSuccess {
            context,
            result,
            process,
        })
    })
    .await
    .map_err(|error| format!("VM recipe task failed: {error}"))?
}

pub async fn provision(
    store_path: PathBuf,
    recipe: VmRecipe,
    repo_path: PathBuf,
    context: RecipeContext,
    repo_id: Option<String>,
) -> Result<RuntimeRecord, String> {
    let start = start(recipe.clone(), repo_path, context).await?;
    let now = now_ms();
    let record = RuntimeRecord {
        id: start
            .context
            .instance_id
            .clone()
            .unwrap_or_else(|| start.context.recipe_id.clone()),
        recipe_id: recipe.id.clone(),
        recipe,
        repo_id,
        project_id: start.context.project_id,
        workspace_id: start.context.workspace_id,
        workspace_name: start.context.workspace_name,
        status: RuntimeStatus::Running,
        cleanup_status: CleanupStatus::NotStarted,
        cleanup_disabled: false,
        cleanup_last_attempt_at: None,
        cleanup_last_error: None,
        created_at: now,
        updated_at: now,
        recipe_result: start.result,
        runtime_environment: None,
        pairing_error: None,
    };
    let mut record = record;
    record.cleanup_disabled = record.recipe.destroy_disabled;
    if record.cleanup_disabled {
        record.cleanup_status = CleanupStatus::Disabled;
    }
    upsert(&store_path, record.clone())?;
    Ok(record)
}

/// Provisions a recipe and immediately imports an `orca-server` pairing offer.
///
/// Pairing failures remain attached to the durable runtime record so the user
/// can retry or clean up the provisioned machine instead of losing ownership
/// of a successfully-created VM.
pub async fn provision_and_pair(
    store_path: PathBuf,
    recipe: VmRecipe,
    repo_path: PathBuf,
    context: RecipeContext,
    repo_id: Option<String>,
) -> Result<RuntimeRecord, String> {
    let mut record = provision(store_path.clone(), recipe, repo_path, context, repo_id).await?;
    let RecipeConnection::OrcaServer { pairing_code, .. } = record.recipe_result.connection()
    else {
        return Ok(record);
    };
    let name = record
        .workspace_name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .map_or_else(
            || format!("{} VM", record.recipe.name),
            |name| format!("{name} · VM"),
        );
    match crate::remote_runtime::save_environment(name, pairing_code).await {
        Ok(environment) => {
            record.runtime_environment = Some(environment);
            record.pairing_error = None;
        }
        Err(error) => {
            record.pairing_error = Some(error);
        }
    }
    record.updated_at = now_ms();
    upsert(&store_path, record.clone())?;
    Ok(record)
}

pub async fn suspend(
    store_path: PathBuf,
    runtime_id: String,
    repo_path: PathBuf,
) -> Result<RuntimeRecord, String> {
    lifecycle(store_path, runtime_id, repo_path, "suspend").await
}

pub async fn resume(
    store_path: PathBuf,
    runtime_id: String,
    repo_path: PathBuf,
) -> Result<RuntimeRecord, String> {
    lifecycle(store_path, runtime_id, repo_path, "resume").await
}

/// Resumes a VM and refreshes the Orca server credentials returned by the
/// lifecycle recipe. Resume commands commonly rotate both endpoint and token.
pub async fn resume_and_pair(
    store_path: PathBuf,
    runtime_id: String,
    repo_path: PathBuf,
) -> Result<RuntimeRecord, String> {
    let mut record = resume(store_path.clone(), runtime_id, repo_path).await?;
    let RecipeConnection::OrcaServer { pairing_code, .. } = record.recipe_result.connection()
    else {
        return Ok(record);
    };
    let pairing = if let Some(environment) = record.runtime_environment.clone() {
        crate::remote_runtime::update_environment(environment, pairing_code).await
    } else {
        let name = record
            .workspace_name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .map_or_else(
                || format!("{} VM", record.recipe.name),
                |name| format!("{name} · VM"),
            );
        crate::remote_runtime::save_environment(name, pairing_code).await
    };
    match pairing {
        Ok(environment) => {
            record.runtime_environment = Some(environment);
            record.pairing_error = None;
        }
        Err(error) => record.pairing_error = Some(error),
    }
    record.updated_at = now_ms();
    upsert(&store_path, record.clone())?;
    Ok(record)
}

pub async fn cleanup(
    store_path: PathBuf,
    runtime_id: String,
    repo_path: PathBuf,
) -> Result<RuntimeRecord, String> {
    lifecycle(store_path, runtime_id, repo_path, "destroy").await
}

async fn lifecycle(
    store_path: PathBuf,
    runtime_id: String,
    repo_path: PathBuf,
    mode: &'static str,
) -> Result<RuntimeRecord, String> {
    tokio::task::spawn_blocking(move || {
        validate_repo_path(&repo_path)?;
        let mut record = list(&store_path)?
            .into_iter()
            .find(|entry| entry.id == runtime_id)
            .ok_or_else(|| format!("Unknown ephemeral VM runtime: {runtime_id}"))?;
        let command = match mode {
            "suspend" => record.recipe.suspend.as_deref(),
            "resume" => record.recipe.resume.as_deref(),
            "destroy" if record.recipe.destroy_disabled => None,
            "destroy" => record.recipe.destroy.as_deref(),
            _ => return Err("Unsupported VM lifecycle mode.".to_string()),
        };
        if mode == "destroy" {
            record.status = RuntimeStatus::CleanupPending;
            record.cleanup_status = if record.recipe.destroy_disabled {
                CleanupStatus::Disabled
            } else {
                CleanupStatus::Running
            };
            record.cleanup_last_attempt_at = Some(now_ms());
            upsert(&store_path, record.clone())?;
        }
        let Some(command) = command else {
            if mode == "destroy" {
                record.status = RuntimeStatus::Cleaned;
            }
            record.updated_at = now_ms();
            upsert(&store_path, record.clone())?;
            return Ok(record);
        };
        let context = context_from_record(&record, repo_path);
        let payload = LifecyclePayload {
            schema_version: 1,
            mode,
            recipe_id: &record.recipe_id,
            instance_id: Some(&record.id),
            project_id: record.project_id.as_deref(),
            workspace_id: record.workspace_id.as_deref(),
            workspace_name: record.workspace_name.as_deref(),
            recipe_result: &record.recipe_result,
        };
        let stdin = serde_json::to_string(&payload)
            .map(|json| format!("{json}\n"))
            .map_err(|error| format!("Could not encode VM lifecycle payload: {error}"))?;
        let process = run_recipe_command(command, mode, &context, Some(&stdin), DEFAULT_TIMEOUT)?;
        if let Err(error) = ensure_success(lifecycle_label(mode), &process) {
            record.status = match mode {
                "suspend" => RuntimeStatus::SuspendFailed,
                "resume" => RuntimeStatus::ResumeFailed,
                _ => RuntimeStatus::CleanupFailed,
            };
            if mode == "destroy" {
                record.cleanup_status = CleanupStatus::Failed;
                record.cleanup_last_error = Some(error.clone());
            }
            record.updated_at = now_ms();
            upsert(&store_path, record)?;
            return Err(error);
        }
        match mode {
            "suspend" => record.status = RuntimeStatus::Suspended,
            "resume" => {
                record.recipe_result = parse_result(&process.stdout)?;
                record.status = RuntimeStatus::Running;
            }
            "destroy" => {
                record.status = RuntimeStatus::Cleaned;
                record.cleanup_status = CleanupStatus::Succeeded;
                record.cleanup_last_error = None;
            }
            _ => {}
        }
        record.updated_at = now_ms();
        upsert(&store_path, record.clone())?;
        Ok(record)
    })
    .await
    .map_err(|error| format!("VM lifecycle task failed: {error}"))?
}

pub fn list(path: &Path) -> Result<Vec<RuntimeRecord>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let metadata = fs::metadata(path)
        .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
    if !metadata.is_file() || metadata.len() > MAX_STORE_BYTES {
        return Err("Ephemeral VM runtime store is invalid or too large.".to_string());
    }
    let bytes =
        fs::read(path).map_err(|error| format!("Could not read {}: {error}", path.display()))?;
    let store: RuntimeStore = serde_json::from_slice(&bytes)
        .map_err(|_| "Ephemeral VM runtime store contains invalid data.".to_string())?;
    if store.version != 1 || store.runtimes.len() > MAX_RUNTIMES {
        return Err("Ephemeral VM runtime store uses an unsupported format.".to_string());
    }
    Ok(store.runtimes)
}

pub fn upsert(path: &Path, record: RuntimeRecord) -> Result<(), String> {
    let mut runtimes = list(path)?;
    runtimes.retain(|entry| entry.id != record.id);
    runtimes.push(record);
    runtimes.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    if runtimes.len() > MAX_RUNTIMES {
        return Err("Ephemeral VM runtime store reached its durable capacity.".to_string());
    }
    write_store(
        path,
        &RuntimeStore {
            version: 1,
            runtimes,
        },
    )
}

fn write_store(path: &Path, store: &RuntimeStore) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(store)
        .map_err(|error| format!("Could not encode VM runtime store: {error}"))?;
    if bytes.len() as u64 > MAX_STORE_BYTES {
        return Err("Ephemeral VM runtime store exceeds 1 MiB.".to_string());
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create {}: {error}", parent.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("Could not secure {}: {error}", parent.display()))?;
    }
    let temporary = path.with_extension(format!("tmp.{}", std::process::id()));
    fs::write(&temporary, bytes)
        .map_err(|error| format!("Could not write {}: {error}", temporary.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("Could not secure {}: {error}", temporary.display()))?;
    }
    fs::rename(&temporary, path)
        .map_err(|error| format!("Could not publish {}: {error}", path.display()))
}

fn run_recipe_command(
    script: &str,
    mode: &str,
    context: &RecipeContext,
    stdin: Option<&str>,
    timeout: Duration,
) -> Result<ProcessResult, String> {
    let mut stdout_file = tempfile::tempfile().map_err(|error| error.to_string())?;
    let mut stderr_file = tempfile::tempfile().map_err(|error| error.to_string())?;
    let mut command = shell_command(script);
    command
        .current_dir(&context.repo_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(
            stdout_file.try_clone().map_err(|error| error.to_string())?,
        ))
        .stderr(Stdio::from(
            stderr_file.try_clone().map_err(|error| error.to_string())?,
        ))
        .env("ORCA_VM_MODE", mode)
        .env(
            "ORCA_VM_INSTANCE_ID",
            context.instance_id.as_deref().unwrap_or_default(),
        )
        .env("ORCA_RECIPE_ID", &context.recipe_id)
        .env(
            "ORCA_PROJECT_ID",
            context.project_id.as_deref().unwrap_or_default(),
        )
        .env(
            "ORCA_WORKSPACE_ID",
            context.workspace_id.as_deref().unwrap_or_default(),
        )
        .env(
            "ORCA_WORKSPACE_NAME",
            context.workspace_name.as_deref().unwrap_or_default(),
        )
        .env("ORCA_REPO_PATH", &context.repo_path)
        .env(
            "ORCA_REPO_URL",
            context.repo_url.as_deref().unwrap_or_default(),
        )
        .env(
            "ORCA_REPO_BRANCH",
            context.branch.as_deref().unwrap_or_default(),
        )
        .env(
            "ORCA_REPO_REF",
            context.git_ref.as_deref().unwrap_or_default(),
        )
        .env(
            "ORCA_VERSION",
            context.orca_version.as_deref().unwrap_or_default(),
        );
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("Could not start VM recipe: {error}"))?;
    if let Some(mut input) = child.stdin.take() {
        if let Some(stdin) = stdin {
            input
                .write_all(stdin.as_bytes())
                .map_err(|error| format!("Could not write VM recipe input: {error}"))?;
        }
    }
    let status = match child
        .wait_timeout(timeout)
        .map_err(|error| format!("Could not wait for VM recipe: {error}"))?
    {
        Some(status) => status,
        None => {
            terminate_process_group(&mut child);
            return Err("VM recipe timed out.".to_string());
        }
    };
    let stdout = read_tail(&mut stdout_file, MAX_CAPTURE_BYTES)?;
    let stderr = read_tail(&mut stderr_file, MAX_CAPTURE_BYTES)?;
    Ok(process_result(status, stdout, stderr))
}

#[cfg(unix)]
fn shell_command(script: &str) -> Command {
    let mut command = Command::new("/bin/sh");
    command.args(["-lc", script]);
    command
}

#[cfg(windows)]
fn shell_command(script: &str) -> Command {
    let mut command = Command::new("cmd.exe");
    command.args(["/d", "/s", "/c", script]);
    command
}

#[cfg(unix)]
fn terminate_process_group(child: &mut std::process::Child) {
    if let Ok(pid) = i32::try_from(child.id()) {
        // SAFETY: `pid` is the live child process group created above. A
        // negative pid targets only that group; no pointer memory is involved.
        unsafe {
            libc::kill(-pid, libc::SIGTERM);
        }
    }
    let _ = child.wait_timeout(Duration::from_secs(2));
    if child.try_wait().ok().flatten().is_none() {
        if let Ok(pid) = i32::try_from(child.id()) {
            // SAFETY: same process-group ownership argument as the SIGTERM call.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
        let _ = child.wait();
    }
}

#[cfg(windows)]
fn terminate_process_group(child: &mut std::process::Child) {
    let _ = Command::new("taskkill")
        .args(["/pid", &child.id().to_string(), "/t", "/f"])
        .status();
    let _ = child.wait();
}

fn process_result(status: ExitStatus, stdout: String, stderr: String) -> ProcessResult {
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt;
    ProcessResult {
        stdout,
        stderr,
        exit_code: status.code(),
        #[cfg(unix)]
        signal: status.signal(),
        #[cfg(not(unix))]
        signal: None,
    }
}

fn read_tail(file: &mut fs::File, max_bytes: usize) -> Result<String, String> {
    let length = file
        .seek(std::io::SeekFrom::End(0))
        .map_err(|error| error.to_string())? as usize;
    file.seek(std::io::SeekFrom::Start(
        length.saturating_sub(max_bytes) as u64
    ))
    .map_err(|error| error.to_string())?;
    let mut bytes = Vec::with_capacity(length.min(max_bytes));
    file.read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    while bytes
        .first()
        .is_some_and(|byte| (*byte & 0b1100_0000) == 0b1000_0000)
    {
        bytes.remove(0);
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn parse_result(stdout: &str) -> Result<RecipeResult, String> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Err("Recipe produced no JSON result.".to_string());
    }
    if trimmed.len() > MAX_CAPTURE_BYTES {
        return Err("Recipe result exceeds 1 MiB.".to_string());
    }
    let result: RecipeResult = serde_json::from_str(trimmed)
        .map_err(|_| "Recipe stdout must be one valid JSON object.".to_string())?;
    let schema_version = match &result {
        RecipeResult::Connection { result } => result.schema_version,
        RecipeResult::Legacy { result } => result.schema_version,
    };
    if schema_version != 1 {
        return Err("Recipe result schemaVersion must be 1.".to_string());
    }
    validate_connection(&result.connection())?;
    Ok(result)
}

fn validate_connection(connection: &RecipeConnection) -> Result<(), String> {
    let project_root = match connection {
        RecipeConnection::OrcaServer {
            pairing_code,
            project_root,
        } => {
            crate::remote_runtime::validate_runtime_pairing_code(pairing_code)
                .map_err(|_| "Recipe result pairingCode is not a valid Orca pairing code.")?;
            project_root
        }
        RecipeConnection::Ssh {
            target,
            project_root,
        } => {
            if target.label.trim().is_empty()
                || target.host.trim().is_empty()
                || target.port == 0
                || target
                    .relay_grace_period_seconds
                    .is_some_and(|seconds| seconds != 0 && !(5..=86_400).contains(&seconds))
                || target.port_forwards.as_ref().is_some_and(|forwards| {
                    forwards.len() > 128
                        || forwards
                            .iter()
                            .any(|forward| forward.remote_host.trim().is_empty())
                })
            {
                return Err("Recipe result contains an invalid SSH target.".to_string());
            }
            project_root
        }
    };
    if !is_absolute_runtime_path(project_root) {
        return Err("Recipe result projectRoot must be an absolute runtime path.".to_string());
    }
    Ok(())
}

fn is_absolute_runtime_path(path: &str) -> bool {
    path.starts_with('/')
        || path.starts_with("\\\\")
        || path.starts_with("//")
        || (path.len() >= 3
            && path.as_bytes()[1] == b':'
            && matches!(path.as_bytes()[2], b'/' | b'\\')
            && path.as_bytes()[0].is_ascii_alphabetic())
}

fn ensure_success(label: &str, result: &ProcessResult) -> Result<(), String> {
    if result.exit_code == Some(0) {
        Ok(())
    } else {
        Err(format!(
            "{label} exited with code {}.",
            result
                .exit_code
                .map_or_else(|| "unknown".to_string(), |code| code.to_string())
        ))
    }
}

fn lifecycle_label(mode: &str) -> &'static str {
    match mode {
        "suspend" => "Suspend",
        "resume" => "Resume",
        _ => "Destroy",
    }
}

fn validate_repo_path(path: &Path) -> Result<(), String> {
    if path.is_dir() {
        Ok(())
    } else {
        Err(format!(
            "Recipe repo path is not a directory: {}",
            path.display()
        ))
    }
}

fn context_from_record(record: &RuntimeRecord, repo_path: PathBuf) -> RecipeContext {
    RecipeContext {
        instance_id: Some(record.id.clone()),
        recipe_id: record.recipe_id.clone(),
        project_id: record.project_id.clone(),
        workspace_id: record.workspace_id.clone(),
        workspace_name: record.workspace_name.clone(),
        repo_path,
        ..RecipeContext::default()
    }
}

fn new_instance_id() -> String {
    let timestamp = now_ms();
    let sequence = INSTANCE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("orca-{timestamp:x}-{:x}-{sequence:x}", std::process::id())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ssh_json(root: &str) -> String {
        serde_json::json!({
            "schemaVersion": 1,
            "connection": {
                "type": "ssh",
                "target": {
                    "label": "Test",
                    "host": "127.0.0.1",
                    "port": 22,
                    "username": "runner"
                },
                "projectRoot": root
            },
            "userData": {"provider": "test"}
        })
        .to_string()
    }

    fn recipe(create: String) -> VmRecipe {
        VmRecipe {
            id: "test.vm".into(),
            name: "Test VM".into(),
            description: None,
            create,
            suspend: None,
            resume: None,
            destroy: None,
            destroy_disabled: false,
        }
    }

    #[tokio::test]
    async fn start_passes_orca_environment_and_parses_ssh_result() {
        let repo = tempfile::tempdir().unwrap();
        let output = ssh_json("/workspace/repo");
        let command = format!(
            "test \"$ORCA_VM_MODE\" = create && test \"$ORCA_RECIPE_ID\" = test.vm && printf '%s' '{}'",
            output.replace('\'', "'\"'\"'")
        );
        let started = start(
            recipe(command),
            repo.path().to_path_buf(),
            RecipeContext::default(),
        )
        .await
        .unwrap();
        assert!(started.context.instance_id.unwrap().starts_with("orca-"));
        assert!(matches!(
            started.result.connection(),
            RecipeConnection::Ssh { project_root, .. } if project_root == "/workspace/repo"
        ));
    }

    #[tokio::test]
    async fn lifecycle_uses_immutable_recipe_and_persists_statuses() {
        let repo = tempfile::tempdir().unwrap();
        let store_dir = tempfile::tempdir().unwrap();
        let store = store_dir.path().join("runtimes.json");
        let create = format!("printf '%s' '{}'", ssh_json("/workspace/repo"));
        let mut recipe = recipe(create);
        recipe.suspend =
            Some("test \"$ORCA_VM_MODE\" = suspend && test -n \"$ORCA_VM_INSTANCE_ID\"".into());
        recipe.resume = Some(format!(
            "test \"$ORCA_VM_MODE\" = resume && read payload && printf '%s' '{}'",
            ssh_json("/workspace/repo").replace('\'', "'\"'\"'")
        ));
        recipe.destroy = Some("test \"$ORCA_VM_MODE\" = destroy && read payload".into());
        let runtime = provision(
            store.clone(),
            recipe,
            repo.path().to_path_buf(),
            RecipeContext::default(),
            Some("repo-1".into()),
        )
        .await
        .unwrap();
        let runtime = suspend(store.clone(), runtime.id, repo.path().to_path_buf())
            .await
            .unwrap();
        assert_eq!(runtime.status, RuntimeStatus::Suspended);
        let runtime = resume(store.clone(), runtime.id, repo.path().to_path_buf())
            .await
            .unwrap();
        assert_eq!(runtime.status, RuntimeStatus::Running);
        let runtime = cleanup(store.clone(), runtime.id, repo.path().to_path_buf())
            .await
            .unwrap();
        assert_eq!(runtime.status, RuntimeStatus::Cleaned);
        assert_eq!(runtime.cleanup_status, CleanupStatus::Succeeded);
        assert_eq!(list(&store).unwrap(), vec![runtime]);
    }

    #[test]
    fn result_schema_rejects_relative_roots_unknown_fields_and_zero_ports() {
        assert!(parse_result(&ssh_json("relative/repo")).is_err());
        let unknown = ssh_json("/repo").replace(
            "\"username\":\"runner\"",
            "\"username\":\"runner\",\"secret\":\"nope\"",
        );
        assert!(parse_result(&unknown).is_err());
        let zero_port = ssh_json("/repo").replace("\"port\":22", "\"port\":0");
        assert!(parse_result(&zero_port).is_err());
    }

    #[test]
    fn store_is_bounded_and_written_with_current_recipe() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("runtime.json");
        let record = RuntimeRecord {
            id: "runtime-1".into(),
            recipe_id: "test.vm".into(),
            recipe: recipe("true".into()),
            repo_id: None,
            project_id: None,
            workspace_id: None,
            workspace_name: None,
            status: RuntimeStatus::Running,
            cleanup_status: CleanupStatus::NotStarted,
            cleanup_disabled: false,
            cleanup_last_attempt_at: None,
            cleanup_last_error: None,
            created_at: 1,
            updated_at: 1,
            recipe_result: parse_result(&ssh_json("/repo")).unwrap(),
            runtime_environment: None,
            pairing_error: None,
        };
        upsert(&path, record.clone()).unwrap();
        assert_eq!(list(&path).unwrap(), vec![record]);
    }
}
