use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use suaegi_core::domain::{Repo, RepoHookSetting};

const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_SHARED_DIRECTORIES: usize = 100;
const ARCHIVE_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SharedHookScripts {
    pub setup: Option<String>,
    pub archive: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookRunResult {
    pub success: bool,
    pub output: String,
}

pub fn normalize_setting(setting: &mut RepoHookSetting) {
    if !matches!(setting.mode.as_str(), "auto" | "override") {
        setting.mode = "auto".to_string();
    }
    if !matches!(
        setting.setup_run_policy.as_str(),
        "ask" | "run-by-default" | "skip-by-default"
    ) {
        setting.setup_run_policy = "run-by-default".to_string();
    }
    if !matches!(
        setting.setup_agent_startup_policy.as_str(),
        "start-immediately" | "wait-for-setup"
    ) {
        setting.setup_agent_startup_policy = "start-immediately".to_string();
    }
    if !setting
        .command_source_policy
        .as_deref()
        .is_some_and(|value| matches!(value, "shared-only" | "local-only" | "run-both"))
    {
        setting.command_source_policy = None;
    }
}

pub fn load_shared_scripts(root: &Path) -> Result<SharedHookScripts, String> {
    let Some(value) = load_orca_yaml(root)? else {
        return Ok(SharedHookScripts::default());
    };
    let scripts = value
        .as_mapping()
        .and_then(|root| root.get(serde_yaml::Value::String("scripts".to_string())))
        .and_then(serde_yaml::Value::as_mapping);
    let get = |name: &str| {
        scripts
            .and_then(|scripts| scripts.get(serde_yaml::Value::String(name.to_string())))
            .and_then(serde_yaml::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    Ok(SharedHookScripts {
        setup: get("setup"),
        archive: get("archive"),
    })
}

fn load_orca_yaml(root: &Path) -> Result<Option<serde_yaml::Value>, String> {
    let path = root.join("orca.yaml");
    let metadata = match std::fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(error) => return Err(format!("Could not inspect {}: {error}", path.display())),
    };
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err(format!(
            "{} is larger than the 1 MiB safety limit",
            path.display()
        ));
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
    let value: serde_yaml::Value = serde_yaml::from_str(&content)
        .map_err(|error| format!("Could not parse {}: {error}", path.display()))?;
    Ok(Some(value))
}

/// Read and normalize Orca's repository-shared worktree directories. Unsafe,
/// ambiguous, duplicate, and excess entries are dropped before any filesystem
/// operation can observe them.
pub fn load_shared_directories(root: &Path) -> Result<Vec<String>, String> {
    let Some(value) = load_orca_yaml(root)? else {
        return Ok(Vec::new());
    };
    let entries = value
        .as_mapping()
        .and_then(|root| root.get(serde_yaml::Value::String("worktree".to_string())))
        .and_then(serde_yaml::Value::as_mapping)
        .and_then(|worktree| {
            worktree.get(serde_yaml::Value::String("sharedDirectories".to_string()))
        })
        .and_then(serde_yaml::Value::as_sequence);
    let Some(entries) = entries else {
        return Ok(Vec::new());
    };

    let mut directories = Vec::new();
    for entry in entries.iter().take(MAX_SHARED_DIRECTORIES) {
        let Some(raw) = entry
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let mut normalized = raw.replace('\\', "/");
        if let Some(without_prefix) = normalized.strip_prefix("./") {
            normalized = without_prefix.to_string();
        }
        normalized = normalized.trim_end_matches('/').to_string();
        let segments = normalized.split('/').collect::<Vec<_>>();
        let unsafe_path = normalized.is_empty()
            || normalized.starts_with('/')
            || normalized.as_bytes().get(1) == Some(&b':')
            || segments
                .iter()
                .any(|segment| segment.is_empty() || matches!(*segment, "." | ".." | ".git"));
        if !unsafe_path && !directories.contains(&normalized) {
            directories.push(normalized);
        }
    }
    Ok(directories)
}

/// Parse Orca's repository-owned `environmentRecipes` catalog. Invalid entries
/// are dropped independently so one stale recipe never hides the usable ones.
/// Legacy `command`/`cleanup` aliases remain accepted exactly as Orca does.
pub fn load_environment_recipes(
    root: &Path,
) -> Result<Vec<crate::plugin_content::VmRecipe>, String> {
    let Some(value) = load_orca_yaml(root)? else {
        return Ok(Vec::new());
    };
    let entries = value
        .as_mapping()
        .and_then(|root| root.get(serde_yaml::Value::String("environmentRecipes".to_string())))
        .and_then(serde_yaml::Value::as_sequence);
    let Some(entries) = entries else {
        return Ok(Vec::new());
    };
    let mut seen = std::collections::HashSet::new();
    let mut recipes = Vec::new();
    for entry in entries.iter().take(64) {
        let Some(record) = entry.as_mapping() else {
            continue;
        };
        let string = |key: &str| {
            record
                .get(serde_yaml::Value::String(key.to_string()))
                .and_then(serde_yaml::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        let Some(id) = string("id") else {
            continue;
        };
        let valid_id = id.len() <= 64
            && id.bytes().enumerate().all(|(index, byte)| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || (index > 0 && matches!(byte, b'.' | b'_' | b'-'))
            });
        let Some(name) = string("name") else {
            continue;
        };
        let Some(create) = string("create").or_else(|| string("command")) else {
            continue;
        };
        let suspend = string("suspend");
        let resume = string("resume");
        let destroy_value = string("destroy").or_else(|| string("cleanup"));
        let destroy_disabled = destroy_value.as_deref() == Some("none");
        if !valid_id
            || !seen.insert(id.clone())
            || name.len() > 128
            || create.len() > 32 * 1024
            || suspend.is_some() != resume.is_some()
            || [suspend.as_ref(), resume.as_ref(), destroy_value.as_ref()]
                .into_iter()
                .flatten()
                .any(|command| command.len() > 32 * 1024 || command.contains('\0'))
        {
            continue;
        }
        recipes.push(crate::plugin_content::VmRecipe {
            id,
            name,
            description: string("description").filter(|value| value.len() <= 1_024),
            create,
            suspend,
            resume,
            destroy: (!destroy_disabled).then_some(destroy_value).flatten(),
            destroy_disabled,
        });
    }
    Ok(recipes)
}

fn resolved_source_policy(setting: &RepoHookSetting, local: &str) -> &'static str {
    match setting.command_source_policy.as_deref() {
        Some("local-only") => "local-only",
        Some("run-both") => "run-both",
        Some("shared-only") => "shared-only",
        _ if !local.trim().is_empty() => "local-only",
        _ => "shared-only",
    }
}

fn effective_script(
    setting: &RepoHookSetting,
    shared: Option<&str>,
    local: &str,
) -> Option<String> {
    let shared = shared.map(str::trim).filter(|value| !value.is_empty());
    let local = local.trim();
    match resolved_source_policy(setting, local) {
        "local-only" => (!local.is_empty()).then(|| local.to_string()),
        "run-both" => {
            let parts = [shared, (!local.is_empty()).then_some(local)]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            (!parts.is_empty()).then(|| parts.join("\n"))
        }
        _ => shared.map(str::to_string),
    }
}

pub fn effective_setup_script(
    setting: &RepoHookSetting,
    root: &Path,
) -> Result<Option<String>, String> {
    let shared = load_shared_scripts(root)?;
    Ok(effective_script(
        setting,
        shared.setup.as_deref(),
        &setting.setup_script,
    ))
}

pub fn effective_archive_script(
    setting: &RepoHookSetting,
    root: &Path,
) -> Result<Option<String>, String> {
    let shared = load_shared_scripts(root)?;
    Ok(effective_script(
        setting,
        shared.archive.as_deref(),
        &setting.archive_script,
    ))
}

pub fn setup_env(repo: &Repo, worktree_path: &Path) -> Vec<(String, String)> {
    let workspace_name = worktree_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("workspace");
    [
        ("ORCA_ROOT_PATH", repo.path.to_string_lossy().as_ref()),
        (
            "ORCA_WORKTREE_PATH",
            worktree_path.to_string_lossy().as_ref(),
        ),
        ("ORCA_WORKSPACE_NAME", workspace_name),
        ("CONDUCTOR_ROOT_PATH", repo.path.to_string_lossy().as_ref()),
        ("GHOSTX_ROOT_PATH", repo.path.to_string_lossy().as_ref()),
        ("ORCA_TERMINAL_GIT_CREDENTIAL_GUARD", "guard"),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value.to_string()))
    .collect()
}

pub fn prepare_setup_runner(worktree_path: &Path, script: &str) -> Result<PathBuf, String> {
    if script.len() as u64 > MAX_CONFIG_BYTES {
        return Err("Setup script is larger than the 1 MiB safety limit".to_string());
    }
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--git-path", "orca/setup-runner.sh"])
        .current_dir(worktree_path)
        .output()
        .map_err(|error| format!("Could not resolve the setup runner path: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if raw.is_empty() {
        return Err("Git returned an empty setup runner path".to_string());
    }
    let path = PathBuf::from(raw);
    let path = if path.is_absolute() {
        path
    } else {
        worktree_path.join(path)
    };
    let parent = path
        .parent()
        .ok_or_else(|| "Setup runner path has no parent directory".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create {}: {error}", parent.display()))?;
    let normalized = script.replace("\r\n", "\n");
    std::fs::write(
        &path,
        format!("#!/usr/bin/env bash\nset -e\n{normalized}\n"),
    )
    .map_err(|error| format!("Could not write {}: {error}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .map_err(|error| format!("Could not make {} executable: {error}", path.display()))?;
    }
    Ok(path)
}

pub async fn run_archive_script(
    repo: Repo,
    worktree_path: PathBuf,
    script: String,
) -> HookRunResult {
    let env: HashMap<String, String> = setup_env(&repo, &worktree_path).into_iter().collect();
    let output = tokio::time::timeout(
        ARCHIVE_TIMEOUT,
        tokio::process::Command::new("/bin/bash")
            .args(["-lc", &script])
            .current_dir(&worktree_path)
            .envs(env)
            .output(),
    )
    .await;
    match output {
        Ok(Ok(output)) => HookRunResult {
            success: output.status.success(),
            output: format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
            .trim()
            .to_string(),
        },
        Ok(Err(error)) => HookRunResult {
            success: false,
            output: error.to_string(),
        },
        Err(_) => HookRunResult {
            success: false,
            output: "Archive hook timed out after 120 seconds.".to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_policy_matches_orca_legacy_defaults() {
        let mut setting = RepoHookSetting {
            setup_script: "local".into(),
            ..RepoHookSetting::default()
        };
        assert_eq!(
            effective_script(&setting, Some("shared"), &setting.setup_script),
            Some("local".into())
        );
        setting.command_source_policy = Some("shared-only".into());
        assert_eq!(
            effective_script(&setting, Some("shared"), &setting.setup_script),
            Some("shared".into())
        );
        setting.command_source_policy = Some("run-both".into());
        assert_eq!(
            effective_script(&setting, Some("shared"), &setting.setup_script),
            Some("shared\nlocal".into())
        );
    }

    #[test]
    fn parses_shared_orca_yaml_scripts() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("orca.yaml"),
            "scripts:\n  setup: |\n    pnpm install\n  archive: echo bye\n",
        )
        .unwrap();
        assert_eq!(
            load_shared_scripts(dir.path()).unwrap(),
            SharedHookScripts {
                setup: Some("pnpm install".into()),
                archive: Some("echo bye".into()),
            }
        );
    }

    #[test]
    fn shared_directories_match_orca_normalization_and_bounds() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("orca.yaml"),
            "worktree:\n  sharedDirectories:\n    - node_modules/\n    - ./node_modules\n    - apps/web/.cache\n    - apps/./bad\n    - ../escape\n    - .git/objects\n    - /absolute\n",
        )
        .unwrap();
        assert_eq!(
            load_shared_directories(dir.path()).unwrap(),
            vec!["node_modules", "apps/web/.cache"]
        );
    }

    #[test]
    fn malformed_shared_directories_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("orca.yaml"),
            "worktree:\n  sharedDirectories: node_modules\n",
        )
        .unwrap();
        assert!(load_shared_directories(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn environment_recipes_match_orca_aliases_and_drop_invalid_entries() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("orca.yaml"),
            r#"
environmentRecipes:
  - id: cloud-sandbox
    name: Cloud Sandbox
    create: ./create.sh
    suspend: ./suspend.sh
    resume: ./resume.sh
    destroy: ./destroy.sh
  - id: manual-sandbox
    name: Manual Sandbox
    command: ./create-manual.sh
    cleanup: none
  - id: cloud-sandbox
    name: Duplicate
    create: nope
  - id: broken-pair
    name: Broken
    create: ./create.sh
    suspend: ./suspend.sh
"#,
        )
        .unwrap();
        let recipes = load_environment_recipes(dir.path()).unwrap();
        assert_eq!(recipes.len(), 2);
        assert_eq!(recipes[0].id, "cloud-sandbox");
        assert_eq!(recipes[0].destroy.as_deref(), Some("./destroy.sh"));
        assert_eq!(recipes[1].create, "./create-manual.sh");
        assert!(recipes[1].destroy_disabled);
        assert!(recipes[1].destroy.is_none());
    }
}
