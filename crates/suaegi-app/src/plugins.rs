//! Experimental Orca-compatible plugin manifest discovery and consent gating.
//!
//! Discovery is deliberately code-free: no plugin worker or panel bytes run
//! until the master switch is enabled and the exact trust fingerprint has been
//! approved.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const ORCA_PLUGIN_COMPAT_VERSION: &str = "1.4.162";
const MANIFEST_FILE: &str = "orca-plugin.json";
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_INSTALL_FILES: usize = 10_000;
const MAX_INSTALL_BYTES: u64 = 256 * 1024 * 1024;
const CAPABILITIES: &[&str] = &[
    "workspace:read",
    "terminal:send",
    "notifications:show",
    "storage",
    "secrets",
    "events:subscribe",
    "settings:own",
];
const COMMAND_ALIAS_ACTIONS: &[&str] = &[
    "worktree.history.back",
    "worktree.history.forward",
    "sidebar.left.toggle",
    "sidebar.sleepingWorkspaces.toggle",
    "floatingWorkspace.maximize",
    "tab.rename",
    "workspace.rename",
    "workspace.openBoard",
    "view.tasks",
    "sidebar.right.toggle",
    "sidebar.explorer.toggle",
    "sidebar.search.toggle",
    "sidebar.sourceControl.toggle",
    "sidebar.checks.toggle",
    "sidebar.ports.toggle",
];

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PluginManifest {
    pub manifest_version: u8,
    pub id: String,
    pub publisher: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: Option<PluginAuthor>,
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    pub engines: PluginEngines,
    pub plugin_api: u8,
    #[serde(default)]
    pub main: Option<String>,
    #[serde(default)]
    pub contributes: PluginContributions,
    #[serde(default)]
    pub capabilities: Vec<PluginCapability>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginAuthor {
    pub name: String,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginEngines {
    pub orca: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginCapability {
    pub kind: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct PluginContributions {
    pub panels: Vec<PluginPanel>,
    pub commands: Vec<PluginCommand>,
    pub events: Vec<PluginEvent>,
    #[serde(rename = "languagePacks")]
    pub language_packs: Vec<PluginLanguagePack>,
    pub keybindings: Vec<PluginKeybinding>,
    #[serde(rename = "vmRecipes")]
    pub vm_recipes: Vec<PluginPathContribution>,
    pub agents: Vec<PluginPathContribution>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginPanel {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub icon: Option<String>,
    pub entry: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginCommand {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub action: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginEvent {
    pub on: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginLanguagePack {
    pub locale: String,
    pub path: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginKeybinding {
    pub command: String,
    pub key: String,
    #[serde(default)]
    pub when: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginPathContribution {
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginStatus {
    Idle,
    Pending,
    Disabled,
    Invalid,
}

#[derive(Debug, Clone)]
pub struct PluginEntry {
    pub plugin_key: String,
    pub root: PathBuf,
    pub content_hash: Option<String>,
    pub name: String,
    pub version: String,
    pub publisher: String,
    pub description: String,
    pub status: PluginStatus,
    pub error: Option<String>,
    pub is_dev: bool,
    pub consent_fingerprint: Option<String>,
    pub capabilities: Vec<String>,
    pub panels: Vec<PluginPanel>,
    pub commands: Vec<PluginCommand>,
    pub events: Vec<PluginEvent>,
    pub language_packs: Vec<PluginLanguagePack>,
    pub language_pack_catalogs: Vec<(String, serde_json::Value)>,
    pub keybindings: Vec<PluginKeybinding>,
    pub vm_recipes: Vec<PluginPathContribution>,
    pub vm_recipe_specs: Vec<crate::plugin_content::VmRecipe>,
    pub agents: Vec<PluginPathContribution>,
    pub has_worker: bool,
    pub main_entry: Option<String>,
    pub rollback_available: bool,
    pub blocked_by_kill_list: Option<crate::plugin_kill_list::KillListEntry>,
}

impl PluginEntry {
    fn invalid(root: PathBuf, plugin_key: String, error: String, is_dev: bool) -> Self {
        Self {
            name: plugin_key.clone(),
            plugin_key,
            root,
            content_hash: None,
            version: "0.0.0".into(),
            publisher: String::new(),
            description: String::new(),
            status: PluginStatus::Invalid,
            error: Some(error),
            is_dev,
            consent_fingerprint: None,
            capabilities: Vec::new(),
            panels: Vec::new(),
            commands: Vec::new(),
            events: Vec::new(),
            language_packs: Vec::new(),
            language_pack_catalogs: Vec::new(),
            keybindings: Vec::new(),
            vm_recipes: Vec::new(),
            vm_recipe_specs: Vec::new(),
            agents: Vec::new(),
            has_worker: false,
            main_entry: None,
            rollback_available: false,
            blocked_by_kill_list: None,
        }
    }
}

pub fn default_plugins_dir() -> PathBuf {
    crate::persistence_thread::default_data_file()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("plugins")
}

pub fn default_plugins_data_dir() -> PathBuf {
    crate::persistence_thread::default_data_file()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("plugin-data")
}

fn safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && !matches!(value, "__proto__" | "prototype" | "constructor")
        && value.split('-').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

fn safe_command_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn valid_content_hash(value: &str) -> bool {
    matches!(value.len(), 32 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn read_current_pointer(plugin_dir: &Path) -> Result<String, String> {
    let path = plugin_dir.join("current");
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|_| "missing current-version pointer".to_string())?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 128 {
        return Err("corrupt current-version pointer".into());
    }
    let value = std::fs::read_to_string(path)
        .map_err(|_| "could not read current-version pointer".to_string())?;
    let value = value.trim().to_string();
    valid_content_hash(&value)
        .then_some(value)
        .ok_or_else(|| "corrupt current-version pointer".into())
}

fn safe_relative_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 1024
        && !value.contains('\\')
        && !Path::new(value).is_absolute()
        && Path::new(value).components().all(|component| {
            matches!(component, Component::Normal(_))
                && component
                    .as_os_str()
                    .to_str()
                    .is_some_and(|segment| segment != ".git")
        })
}

fn semver_triplet(value: &str) -> Option<[u64; 3]> {
    let core = value.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let parsed = [
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ];
    parts.next().is_none().then_some(parsed)
}

fn valid_semver(value: &str) -> bool {
    static SEMVER: OnceLock<Regex> = OnceLock::new();
    SEMVER
        .get_or_init(|| {
            Regex::new(
                r"^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*))*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$",
            )
            .expect("plugin semver regex")
        })
        .is_match(value)
}

fn validate_manifest(manifest: &PluginManifest) -> Result<(), String> {
    if manifest.manifest_version != 1 || manifest.plugin_api != 1 {
        return Err("manifestVersion and pluginApi must both be 1".into());
    }
    if !safe_id(&manifest.id) || !safe_id(&manifest.publisher) {
        return Err("publisher and id must be safe kebab-case identifiers".into());
    }
    if manifest.name.trim().is_empty() || manifest.name.len() > 256 {
        return Err("name must contain 1-256 bytes".into());
    }
    if !valid_semver(&manifest.version) {
        return Err("version must be semantic versioning".into());
    }
    if manifest.description.len() > 4096
        || manifest
            .repository
            .as_ref()
            .is_some_and(|url| url.len() > 2048)
        || manifest.author.as_ref().is_some_and(|author| {
            author.name.trim().is_empty()
                || author.name.len() > 256
                || author.url.as_ref().is_some_and(|url| url.len() > 2048)
        })
        || manifest
            .icon
            .as_deref()
            .is_some_and(|path| !safe_relative_path(path))
    {
        return Err("manifest metadata is invalid".into());
    }
    let engine_version = manifest
        .engines
        .orca
        .strip_prefix(">=")
        .filter(|version| {
            manifest.engines.orca.len() <= 64
                && version
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || byte == b'.')
        })
        .ok_or_else(|| "engines.orca must use >=x.y.z".to_string())?;
    let minimum = semver_triplet(engine_version)
        .ok_or_else(|| "engines.orca must use >=x.y.z".to_string())?;
    let host = semver_triplet(ORCA_PLUGIN_COMPAT_VERSION).unwrap_or([0, 0, 0]);
    if host < minimum {
        return Err(format!(
            "requires Orca {} (compatibility host is {ORCA_PLUGIN_COMPAT_VERSION})",
            manifest.engines.orca
        ));
    }
    if manifest.capabilities.len() > 32
        || manifest
            .capabilities
            .iter()
            .any(|capability| !CAPABILITIES.contains(&capability.kind.as_str()))
    {
        return Err("capabilities contains an unsupported grant".into());
    }
    if manifest.contributes.panels.len() > 64
        || manifest.contributes.commands.len() > 256
        || manifest.contributes.events.len() > 3
        || manifest.contributes.language_packs.len() > 16
        || manifest.contributes.keybindings.len() > 256
        || manifest.contributes.vm_recipes.len() > 64
        || manifest.contributes.agents.len() > 64
    {
        return Err("contribution count exceeds the plugin API limit".into());
    }
    if manifest.contributes.panels.iter().any(|panel| {
        !safe_id(&panel.id)
            || panel.title.trim().is_empty()
            || panel.title.len() > 256
            || !safe_relative_path(&panel.entry)
    }) {
        return Err("panel contribution is invalid".into());
    }
    if manifest.contributes.commands.iter().any(|command| {
        !safe_command_id(&command.id)
            || command.title.trim().is_empty()
            || !command
                .context
                .as_deref()
                .is_none_or(|context| matches!(context, "global" | "worktree"))
            || !command
                .action
                .as_deref()
                .is_none_or(|action| COMMAND_ALIAS_ACTIONS.contains(&action))
    }) {
        return Err("command contribution is invalid".into());
    }
    let duplicate_ids = |values: Vec<&str>| {
        let count = values.len();
        values.into_iter().collect::<HashSet<_>>().len() != count
    };
    if duplicate_ids(
        manifest
            .contributes
            .panels
            .iter()
            .map(|panel| panel.id.as_str())
            .collect(),
    ) || duplicate_ids(
        manifest
            .contributes
            .commands
            .iter()
            .map(|command| command.id.as_str())
            .collect(),
    ) {
        return Err("panel and command contribution IDs must be unique".into());
    }
    static LOCALE: OnceLock<Regex> = OnceLock::new();
    let locale = LOCALE.get_or_init(|| {
        Regex::new(r"^[A-Za-z]{2,3}(?:-[A-Za-z0-9]{2,8})*$").expect("plugin locale regex")
    });
    let mut locales = HashSet::new();
    if manifest.contributes.language_packs.iter().any(|pack| {
        pack.locale.len() < 2
            || pack.locale.len() > 35
            || !locale.is_match(&pack.locale)
            || !safe_relative_path(&pack.path)
            || !locales.insert(pack.locale.to_ascii_lowercase())
    }) {
        return Err("language pack contribution is invalid".into());
    }
    let commands = manifest
        .contributes
        .commands
        .iter()
        .map(|command| {
            (
                command.id.as_str(),
                command.context.as_deref().unwrap_or("global"),
            )
        })
        .collect::<HashMap<_, _>>();
    let mut normalized_keys = HashSet::new();
    for keybinding in &manifest.contributes.keybindings {
        let Some(command_context) = commands.get(keybinding.command.as_str()) else {
            return Err("keybinding references an unknown contributed command".into());
        };
        if keybinding.key.is_empty()
            || keybinding.key.len() > 128
            || keybinding
                .when
                .as_deref()
                .is_some_and(|when| !matches!(when, "global" | "worktree"))
            || keybinding
                .when
                .as_deref()
                .is_some_and(|when| when != *command_context)
        {
            return Err("keybinding contribution is invalid".into());
        }
        let normalized = suaegi_keys::normalize_keybinding(&keybinding.key);
        let Some(canonical) = normalized.canonical() else {
            return Err("keybinding contribution contains an invalid shortcut".into());
        };
        if !normalized_keys.insert(canonical.to_ascii_lowercase()) {
            return Err("keybinding contributions contain a duplicate shortcut".into());
        }
    }
    for paths in [
        &manifest.contributes.vm_recipes,
        &manifest.contributes.agents,
    ] {
        if paths.iter().any(|entry| !safe_relative_path(&entry.path))
            || duplicate_ids(paths.iter().map(|entry| entry.path.as_str()).collect())
        {
            return Err("content-pack path contribution is invalid".into());
        }
    }
    let allowed_events = [
        "worktree.created",
        "worktree.removed",
        "agent.status.changed",
    ];
    if manifest
        .contributes
        .events
        .iter()
        .any(|event| !allowed_events.contains(&event.on.as_str()))
    {
        return Err("event contribution is invalid".into());
    }
    if manifest.main.is_none()
        && (manifest
            .contributes
            .commands
            .iter()
            .any(|command| command.action.is_none())
            || !manifest.contributes.events.is_empty())
    {
        return Err("main is required for worker commands and event subscriptions".into());
    }
    if !manifest.contributes.events.is_empty()
        && !manifest
            .capabilities
            .iter()
            .any(|capability| capability.kind == "events:subscribe")
    {
        return Err("events:subscribe capability is required for contributed events".into());
    }
    if manifest
        .main
        .as_deref()
        .is_some_and(|path| !safe_relative_path(path))
    {
        return Err("main must be a safe relative path".into());
    }
    Ok(())
}

fn validate_artifact(root: &Path, relative: &str) -> Result<(), String> {
    let root = root
        .canonicalize()
        .map_err(|error| format!("could not resolve plugin root: {error}"))?;
    let resolved = root
        .join(relative)
        .canonicalize()
        .map_err(|error| format!("missing declared artifact {relative}: {error}"))?;
    if !resolved.starts_with(&root) || !resolved.is_file() {
        return Err(format!(
            "declared artifact {relative} escapes the plugin or is not a file"
        ));
    }
    Ok(())
}

fn fingerprint(manifest: &PluginManifest) -> String {
    let mut grants = manifest
        .capabilities
        .iter()
        .map(|capability| capability.kind.as_str())
        .collect::<Vec<_>>();
    grants.sort_unstable();
    grants.dedup();
    let canonical = serde_json::json!({
        "capabilities": grants,
        "worker": manifest.main.is_some(),
        "contributions": manifest.contributes
    });
    let mut digest = Sha256::new();
    digest.update(canonical.to_string());
    format!("{:x}", digest.finalize())
}

fn read_plugin(
    root: PathBuf,
    fallback_key: String,
    is_dev: bool,
    disabled: &HashSet<String>,
    consents: &HashMap<String, String>,
) -> PluginEntry {
    let path = root.join(MANIFEST_FILE);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() && metadata.len() <= MAX_MANIFEST_BYTES => {}
        Ok(_) => {
            return PluginEntry::invalid(
                root,
                fallback_key,
                format!("invalid or oversized {MANIFEST_FILE}"),
                is_dev,
            );
        }
        Err(_) => {
            return PluginEntry::invalid(
                root,
                fallback_key,
                format!("missing {MANIFEST_FILE}"),
                is_dev,
            );
        }
    }
    let manifest: PluginManifest = match std::fs::read(&path)
        .map_err(|error| error.to_string())
        .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|error| error.to_string()))
    {
        Ok(manifest) => manifest,
        Err(error) => {
            return PluginEntry::invalid(
                root,
                fallback_key,
                format!("invalid {MANIFEST_FILE}: {error}"),
                is_dev,
            );
        }
    };
    if let Err(error) = validate_manifest(&manifest) {
        return PluginEntry::invalid(
            root,
            format!("{}.{}", manifest.publisher, manifest.id),
            format!("invalid manifest: {error}"),
            is_dev,
        );
    }
    let plugin_key = format!("{}.{}", manifest.publisher, manifest.id);
    if !is_dev && plugin_key != fallback_key {
        return PluginEntry::invalid(
            root,
            fallback_key,
            format!("manifest identity {plugin_key} does not match install directory"),
            is_dev,
        );
    }
    let artifacts = manifest
        .main
        .iter()
        .chain(manifest.icon.iter())
        .map(String::as_str)
        .chain(
            manifest
                .contributes
                .panels
                .iter()
                .map(|panel| panel.entry.as_str()),
        )
        .chain(
            manifest
                .contributes
                .language_packs
                .iter()
                .map(|pack| pack.path.as_str()),
        )
        .chain(
            manifest
                .contributes
                .vm_recipes
                .iter()
                .chain(manifest.contributes.agents.iter())
                .map(|entry| entry.path.as_str()),
        );
    for artifact in artifacts {
        if let Err(error) = validate_artifact(&root, artifact) {
            return PluginEntry::invalid(root, plugin_key, error, is_dev);
        }
    }
    let mut vm_recipe_specs = Vec::with_capacity(manifest.contributes.vm_recipes.len());
    for contribution in &manifest.contributes.vm_recipes {
        match crate::plugin_content::parse_vm_recipe(&root, &contribution.path) {
            Ok(recipe) => vm_recipe_specs.push(recipe),
            Err(error) => return PluginEntry::invalid(root, plugin_key, error, is_dev),
        }
    }
    if let Err(error) = crate::plugin_content::validate_vm_recipe_set(&vm_recipe_specs) {
        return PluginEntry::invalid(root, plugin_key, error, is_dev);
    }
    let mut language_pack_catalogs = Vec::with_capacity(manifest.contributes.language_packs.len());
    for contribution in &manifest.contributes.language_packs {
        match crate::plugin_content::load_language_pack(&root, &contribution.path) {
            Ok(catalog) => {
                language_pack_catalogs.push((contribution.locale.clone(), catalog));
            }
            Err(error) => return PluginEntry::invalid(root, plugin_key, error, is_dev),
        }
    }
    let consent_fingerprint = fingerprint(&manifest);
    let status = if disabled.contains(&plugin_key) {
        PluginStatus::Disabled
    } else if consents.get(&plugin_key) != Some(&consent_fingerprint) {
        PluginStatus::Pending
    } else {
        PluginStatus::Idle
    };
    let rollback_available = !is_dev && rollback_candidate(&root).is_some();
    PluginEntry {
        plugin_key,
        content_hash: (!is_dev)
            .then(|| root.file_name()?.to_str().map(str::to_string))
            .flatten(),
        root,
        name: manifest.name,
        version: manifest.version,
        publisher: manifest.publisher,
        description: manifest.description,
        status,
        error: None,
        is_dev,
        consent_fingerprint: Some(consent_fingerprint),
        capabilities: manifest
            .capabilities
            .into_iter()
            .map(|capability| capability.kind)
            .collect(),
        panels: manifest.contributes.panels,
        commands: manifest.contributes.commands,
        events: manifest.contributes.events,
        language_packs: manifest.contributes.language_packs,
        language_pack_catalogs,
        keybindings: manifest.contributes.keybindings,
        vm_recipes: manifest.contributes.vm_recipes,
        vm_recipe_specs,
        agents: manifest.contributes.agents,
        has_worker: manifest.main.is_some(),
        main_entry: manifest.main,
        rollback_available,
        blocked_by_kill_list: None,
    }
}

pub fn apply_kill_list(
    plugins: &mut [PluginEntry],
    kill_list: Option<&crate::plugin_kill_list::PluginKillList>,
) {
    for plugin in plugins {
        plugin.blocked_by_kill_list = kill_list
            .and_then(|list| crate::plugin_kill_list::find(list, &plugin.plugin_key))
            .cloned();
    }
}

fn rollback_candidate(current_root: &Path) -> Option<PathBuf> {
    let current = current_root.file_name()?.to_str()?;
    let plugin_dir = current_root.parent()?;
    let candidates = std::fs::read_dir(plugin_dir)
        .ok()?
        .flatten()
        .filter(|entry| {
            entry.file_type().is_ok_and(|kind| kind.is_dir())
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| valid_content_hash(name) && name != current)
        })
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    (candidates.len() == 1).then(|| candidates[0].clone())
}

pub async fn discover(
    plugins_dir: PathBuf,
    dev_paths: Vec<String>,
    disabled_plugins: Vec<String>,
    consents: HashMap<String, String>,
) -> Vec<PluginEntry> {
    tokio::task::spawn_blocking(move || {
        let disabled = disabled_plugins.into_iter().collect::<HashSet<_>>();
        let mut by_key = HashMap::<String, PluginEntry>::new();
        if let Ok(entries) = std::fs::read_dir(&plugins_dir) {
            for entry in entries.flatten() {
                let key = entry.file_name().to_string_lossy().to_string();
                let valid_key = key
                    .split_once('.')
                    .is_some_and(|(publisher, id)| safe_id(publisher) && safe_id(id));
                if !valid_key || !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    continue;
                }
                let plugin = match read_current_pointer(&entry.path()) {
                    Ok(pointer)
                        if std::fs::symlink_metadata(entry.path().join(&pointer)).is_ok_and(
                            |metadata| metadata.is_dir() && !metadata.file_type().is_symlink(),
                        ) =>
                    {
                        read_plugin(
                            entry.path().join(pointer),
                            key.clone(),
                            false,
                            &disabled,
                            &consents,
                        )
                    }
                    Ok(_) => PluginEntry::invalid(
                        entry.path(),
                        key.clone(),
                        "current version is not an immutable directory".into(),
                        false,
                    ),
                    Err(error) => PluginEntry::invalid(entry.path(), key.clone(), error, false),
                };
                by_key.insert(key, plugin);
            }
        }
        for (index, path) in dev_paths.into_iter().enumerate() {
            let plugin = read_plugin(
                PathBuf::from(path),
                format!("invalid-development-plugin-{}", index + 1),
                true,
                &disabled,
                &consents,
            );
            by_key.insert(plugin.plugin_key.clone(), plugin);
        }
        let mut plugins = by_key.into_values().collect::<Vec<_>>();
        plugins.sort_by(|left, right| left.name.cmp(&right.name));
        let mut recipe_owners = HashMap::<String, Vec<usize>>::new();
        for (index, plugin) in plugins.iter().enumerate() {
            if plugin.status != PluginStatus::Idle || plugin.blocked_by_kill_list.is_some() {
                continue;
            }
            for recipe in &plugin.vm_recipe_specs {
                recipe_owners
                    .entry(recipe.id.clone())
                    .or_default()
                    .push(index);
            }
        }
        for (recipe_id, owners) in recipe_owners {
            if owners.len() < 2 {
                continue;
            }
            for owner in owners {
                plugins[owner].status = PluginStatus::Invalid;
                plugins[owner].error = Some(format!(
                    "VM recipe id \"{recipe_id}\" is contributed by multiple approved plugins"
                ));
            }
        }
        plugins
    })
    .await
    .unwrap_or_default()
}

fn collect_install_files(root: &Path) -> Result<Vec<(PathBuf, u64)>, String> {
    fn visit(
        root: &Path,
        relative: &Path,
        files: &mut Vec<(PathBuf, u64)>,
        total: &mut u64,
    ) -> Result<(), String> {
        let directory = root.join(relative);
        let mut entries = std::fs::read_dir(&directory)
            .map_err(|error| format!("Could not read {}: {error}", directory.display()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Could not enumerate plugin files: {error}"))?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            if relative.as_os_str().is_empty() && entry.file_name() == ".git" {
                continue;
            }
            let child = relative.join(entry.file_name());
            let metadata = std::fs::symlink_metadata(entry.path()).map_err(|error| {
                format!("Could not inspect {}: {error}", entry.path().display())
            })?;
            if metadata.file_type().is_symlink() {
                return Err(format!(
                    "Plugin install source contains a symbolic link: {}",
                    child.display()
                ));
            }
            if metadata.is_dir() {
                visit(root, &child, files, total)?;
            } else if metadata.is_file() {
                *total = total.saturating_add(metadata.len());
                files.push((child, metadata.len()));
                if files.len() > MAX_INSTALL_FILES || *total > MAX_INSTALL_BYTES {
                    return Err("Plugin install source exceeds the file or byte limit.".into());
                }
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    let mut total = 0;
    visit(root, Path::new(""), &mut files, &mut total)?;
    Ok(files)
}

fn hash_install_tree(root: &Path, files: &[(PathBuf, u64)]) -> Result<String, String> {
    let mut digest = Sha256::new();
    for (relative, size) in files {
        digest.update(relative.to_string_lossy().as_bytes());
        digest.update([0]);
        digest.update(size.to_le_bytes());
        digest.update(
            std::fs::read(root.join(relative))
                .map_err(|error| format!("Could not hash {}: {error}", relative.display()))?,
        );
    }
    Ok(format!("{:x}", digest.finalize()))
}

pub fn verify_installed_content(root: &Path, expected_hash: Option<&str>) -> Result<(), String> {
    let Some(expected_hash) = expected_hash else {
        return Ok(());
    };
    if !valid_content_hash(expected_hash)
        || root.file_name().and_then(|name| name.to_str()) != Some(expected_hash)
    {
        return Err("Installed plugin content identity is invalid.".into());
    }
    let actual = hash_install_tree(root, &collect_install_files(root)?)?;
    if actual != expected_hash {
        return Err(
            "Installed plugin content changed after discovery; refresh before running it.".into(),
        );
    }
    Ok(())
}

fn install_local_blocking(
    source: &Path,
    plugins_dir: &Path,
    expected_plugin_key: Option<&str>,
) -> Result<String, String> {
    if !source.is_absolute() || !source.is_dir() {
        return Err("Plugin install path must be an existing absolute directory.".into());
    }
    let preview = read_plugin(
        source.to_path_buf(),
        "local-plugin".into(),
        true,
        &HashSet::new(),
        &HashMap::new(),
    );
    if preview.status == PluginStatus::Invalid {
        return Err(preview
            .error
            .unwrap_or_else(|| "Plugin manifest is invalid.".into()));
    }
    let plugin_key = preview.plugin_key;
    if expected_plugin_key.is_some_and(|expected| expected != plugin_key) {
        return Err(format!(
            "Plugin manifest identity {plugin_key} does not match the reviewed marketplace listing."
        ));
    }
    let files = collect_install_files(source)?;
    let content_hash = hash_install_tree(source, &files)?;
    let plugin_dir = plugins_dir.join(&plugin_key);
    let previous_hash = read_current_pointer(&plugin_dir).ok();
    let version_dir = plugin_dir.join(&content_hash);
    std::fs::create_dir_all(&plugin_dir)
        .map_err(|error| format!("Could not create {}: {error}", plugin_dir.display()))?;
    if !version_dir.exists() {
        let staging = plugin_dir.join(format!(".staging-{}-{content_hash}", std::process::id()));
        if staging.exists() {
            std::fs::remove_dir_all(&staging)
                .map_err(|error| format!("Could not clear stale plugin staging: {error}"))?;
        }
        let result = (|| {
            std::fs::create_dir(&staging)
                .map_err(|error| format!("Could not create plugin staging: {error}"))?;
            for (relative, _) in &files {
                let target = staging.join(relative);
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|error| format!("Could not create plugin directory: {error}"))?;
                }
                std::fs::copy(source.join(relative), &target)
                    .map_err(|error| format!("Could not copy {}: {error}", relative.display()))?;
            }
            std::fs::rename(&staging, &version_dir)
                .map_err(|error| format!("Could not publish plugin install: {error}"))
        })();
        if result.is_err() && staging.exists() {
            let _ = std::fs::remove_dir_all(&staging);
        }
        result?;
    }
    let pointer_temp = plugin_dir.join(format!("current.tmp.{}", std::process::id()));
    std::fs::write(&pointer_temp, format!("{content_hash}\n"))
        .map_err(|error| format!("Could not write plugin pointer: {error}"))?;
    std::fs::rename(&pointer_temp, plugin_dir.join("current"))
        .map_err(|error| format!("Could not publish plugin pointer: {error}"))?;
    let retained = [Some(content_hash.as_str()), previous_hash.as_deref()]
        .into_iter()
        .flatten()
        .collect::<HashSet<_>>();
    if let Ok(entries) = std::fs::read_dir(&plugin_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if entry.file_type().is_ok_and(|kind| kind.is_dir())
                && valid_content_hash(&name)
                && !retained.contains(name.as_str())
            {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    Ok(plugin_key)
}

pub async fn install_local(source: PathBuf, plugins_dir: PathBuf) -> Result<String, String> {
    tokio::task::spawn_blocking(move || install_local_blocking(&source, &plugins_dir, None))
        .await
        .map_err(|error| format!("Plugin installer failed: {error}"))?
}

fn parse_git_source(input: &str) -> Result<(&str, Option<&str>), String> {
    let (url, reference) = input
        .rsplit_once('#')
        .map_or((input, None), |(url, reference)| {
            (url, (!reference.is_empty()).then_some(reference))
        });
    let allowed_url = url.starts_with("https://")
        || url.starts_with("ssh://")
        || url
            .split_once('@')
            .is_some_and(|(user, rest)| !user.is_empty() && rest.contains(':'));
    if !allowed_url || url.len() > 4096 || url.chars().any(char::is_control) {
        return Err("Plugin Git URL must use HTTPS or SSH.".into());
    }
    if reference.is_some_and(|reference| {
        reference.len() > 256
            || reference.starts_with('-')
            || reference.starts_with('/')
            || reference.contains("..")
            || !reference.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'/' | b'-')
            })
    }) {
        return Err("Plugin Git reference is invalid.".into());
    }
    Ok((url, reference))
}

async fn git_command(args: Vec<String>) -> Result<(), String> {
    let output = tokio::process::Command::new("git")
        .args(&args)
        .output()
        .await
        .map_err(|error| format!("Could not start system Git: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let message = String::from_utf8_lossy(&output.stderr)
        .trim()
        .chars()
        .take(500)
        .collect::<String>();
    Err(if message.is_empty() {
        format!("Git exited with status {}.", output.status)
    } else {
        message
    })
}

pub async fn install_git(input: String, plugins_dir: PathBuf) -> Result<String, String> {
    install_git_inner(input, plugins_dir, None).await
}

pub async fn install_git_expected(
    input: String,
    plugins_dir: PathBuf,
    expected_plugin_key: String,
) -> Result<String, String> {
    install_git_inner(input, plugins_dir, Some(expected_plugin_key)).await
}

pub async fn install_git_source_expected(
    url: String,
    reference: String,
    plugins_dir: PathBuf,
    expected_plugin_key: String,
) -> Result<String, String> {
    let (_, _) = parse_git_source(&url)?;
    if reference.is_empty()
        || reference.len() > 4_096
        || reference.starts_with('-')
        || reference.starts_with('/')
        || reference.contains("..")
        || reference.chars().any(char::is_control)
    {
        return Err("Plugin Git reference is invalid.".into());
    }
    install_git_checkout(url, Some(reference), plugins_dir, Some(expected_plugin_key)).await
}

async fn install_git_inner(
    input: String,
    plugins_dir: PathBuf,
    expected_plugin_key: Option<String>,
) -> Result<String, String> {
    let (url, reference) = parse_git_source(input.trim())?;
    let url = url.to_string();
    let reference = reference.map(str::to_string);
    install_git_checkout(url, reference, plugins_dir, expected_plugin_key).await
}

async fn install_git_checkout(
    url: String,
    reference: Option<String>,
    plugins_dir: PathBuf,
    expected_plugin_key: Option<String>,
) -> Result<String, String> {
    let staging = tempfile::tempdir()
        .map_err(|error| format!("Could not create plugin staging directory: {error}"))?;
    let destination = staging.path().join("checkout");
    git_command(vec![
        "clone".into(),
        "--filter=blob:none".into(),
        "--no-checkout".into(),
        "--".into(),
        url,
        destination.to_string_lossy().into_owned(),
    ])
    .await?;
    if let Some(reference) = reference {
        git_command(vec![
            "-C".into(),
            destination.to_string_lossy().into_owned(),
            "fetch".into(),
            "--depth=1".into(),
            "origin".into(),
            reference,
        ])
        .await?;
        git_command(vec![
            "-C".into(),
            destination.to_string_lossy().into_owned(),
            "checkout".into(),
            "--detach".into(),
            "--force".into(),
            "FETCH_HEAD".into(),
        ])
        .await?;
    } else {
        git_command(vec![
            "-C".into(),
            destination.to_string_lossy().into_owned(),
            "checkout".into(),
            "--detach".into(),
            "--force".into(),
            "HEAD".into(),
        ])
        .await?;
    }
    tokio::task::spawn_blocking(move || {
        install_local_blocking(&destination, &plugins_dir, expected_plugin_key.as_deref())
    })
    .await
    .map_err(|error| format!("Plugin installer failed: {error}"))?
}

pub async fn install_source(input: String, plugins_dir: PathBuf) -> Result<String, String> {
    let trimmed = input.trim();
    if trimmed.starts_with("https://")
        || trimmed.starts_with("ssh://")
        || trimmed
            .split_once('@')
            .is_some_and(|(user, rest)| !user.is_empty() && rest.contains(':'))
    {
        install_git(trimmed.to_string(), plugins_dir).await
    } else {
        install_local(PathBuf::from(trimmed), plugins_dir).await
    }
}

fn rollback_blocking(plugin_key: &str, plugins_dir: &Path) -> Result<String, String> {
    let valid_key = plugin_key
        .split_once('.')
        .is_some_and(|(publisher, id)| safe_id(publisher) && safe_id(id));
    if !valid_key {
        return Err("Invalid qualified plugin key.".into());
    }
    let plugin_dir = plugins_dir.join(plugin_key);
    let current = read_current_pointer(&plugin_dir)
        .map_err(|error| format!("Installed plugin has no usable current version: {error}"))?;
    let current_root = plugin_dir.join(current);
    let candidate = rollback_candidate(&current_root)
        .ok_or_else(|| "No unambiguous rollback version is available.".to_string())?;
    let candidate_hash = candidate
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "Rollback version name is invalid.".to_string())?;
    let preview = read_plugin(
        candidate.clone(),
        plugin_key.to_string(),
        false,
        &HashSet::new(),
        &HashMap::new(),
    );
    if preview.status == PluginStatus::Invalid {
        return Err(preview
            .error
            .unwrap_or_else(|| "Rollback manifest is invalid.".into()));
    }
    let files = collect_install_files(&candidate)?;
    if hash_install_tree(&candidate, &files)? != candidate_hash {
        return Err("Rollback version failed integrity verification.".into());
    }
    let pointer_temp = plugin_dir.join(format!("current.tmp.{}", std::process::id()));
    std::fs::write(&pointer_temp, format!("{candidate_hash}\n"))
        .map_err(|error| format!("Could not write rollback pointer: {error}"))?;
    std::fs::rename(&pointer_temp, plugin_dir.join("current"))
        .map_err(|error| format!("Could not publish rollback pointer: {error}"))?;
    Ok(plugin_key.to_string())
}

pub async fn rollback(plugin_key: String, plugins_dir: PathBuf) -> Result<String, String> {
    tokio::task::spawn_blocking(move || rollback_blocking(&plugin_key, &plugins_dir))
        .await
        .map_err(|error| format!("Plugin rollback failed: {error}"))?
}

fn remove_resolved_plugin_directory(root: &Path, plugin_key: &str) -> Result<(), String> {
    if !root.exists() {
        return Ok(());
    }
    let root = root
        .canonicalize()
        .map_err(|error| format!("Could not resolve plugin directory: {error}"))?;
    let unresolved = root.join(plugin_key);
    if !unresolved.exists() {
        return Ok(());
    }
    let target = unresolved
        .canonicalize()
        .map_err(|error| format!("Could not resolve installed plugin: {error}"))?;
    if target == root || !target.starts_with(&root) {
        return Err("Refusing to remove a plugin path outside the plugin directory.".into());
    }
    std::fs::remove_dir_all(&unresolved)
        .map_err(|error| format!("Could not remove installed plugin: {error}"))
}

fn remove_blocking(
    plugin_key: &str,
    plugins_dir: &Path,
    plugins_data_dir: &Path,
) -> Result<String, String> {
    let valid_key = plugin_key
        .split_once('.')
        .is_some_and(|(publisher, id)| safe_id(publisher) && safe_id(id));
    if !valid_key {
        return Err("Invalid qualified plugin key.".into());
    }
    remove_resolved_plugin_directory(plugins_dir, plugin_key)?;
    remove_resolved_plugin_directory(plugins_data_dir, plugin_key)?;
    Ok(plugin_key.to_string())
}

pub async fn remove(
    plugin_key: String,
    plugins_dir: PathBuf,
    plugins_data_dir: PathBuf,
) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        remove_blocking(&plugin_key, &plugins_dir, &plugins_data_dir)
    })
    .await
    .map_err(|error| format!("Plugin removal failed: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_plugin(root: &Path, main: Option<&str>) {
        std::fs::create_dir_all(root).unwrap();
        if let Some(main) = main {
            std::fs::write(root.join(main), "console.log('ready')").unwrap();
        }
        std::fs::write(
            root.join(MANIFEST_FILE),
            serde_json::json!({
                "manifestVersion": 1,
                "id": "notes",
                "publisher": "acme",
                "name": "Notes",
                "version": "1.2.3",
                "engines": {"orca": ">=1.4.0"},
                "pluginApi": 1,
                "main": main,
                "capabilities": [{"kind": "storage"}]
            })
            .to_string(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn dev_plugin_is_pending_until_exact_fingerprint_is_approved() {
        let dir = tempfile::tempdir().unwrap();
        write_plugin(dir.path(), Some("main.js"));
        let first = discover(
            dir.path().join("installed"),
            vec![dir.path().to_string_lossy().to_string()],
            Vec::new(),
            HashMap::new(),
        )
        .await;
        assert_eq!(first[0].status, PluginStatus::Pending);
        let fingerprint = first[0].consent_fingerprint.clone().unwrap();
        let approved = discover(
            dir.path().join("installed"),
            vec![dir.path().to_string_lossy().to_string()],
            Vec::new(),
            HashMap::from([("acme.notes".into(), fingerprint)]),
        )
        .await;
        assert_eq!(approved[0].status, PluginStatus::Idle);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn artifact_symlink_escape_is_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let plugin = dir.path().join("plugin");
        write_plugin(&plugin, None);
        let outside = dir.path().join("outside.html");
        std::fs::write(&outside, "private").unwrap();
        std::os::unix::fs::symlink(&outside, plugin.join("panel.html")).unwrap();
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(plugin.join(MANIFEST_FILE)).unwrap()).unwrap();
        value["contributes"] =
            serde_json::json!({"panels":[{"id":"panel","title":"Panel","entry":"panel.html"}]});
        std::fs::write(plugin.join(MANIFEST_FILE), value.to_string()).unwrap();
        let result = discover(
            dir.path().join("installed"),
            vec![plugin.to_string_lossy().to_string()],
            Vec::new(),
            HashMap::new(),
        )
        .await;
        assert_eq!(result[0].status, PluginStatus::Invalid);
    }

    #[tokio::test]
    async fn instructional_content_is_parsed_before_a_plugin_can_activate() {
        let dir = tempfile::tempdir().unwrap();
        let plugin = dir.path().join("plugin");
        write_plugin(&plugin, None);
        std::fs::write(
            plugin.join("vm.json"),
            r#"{"schemaVersion":1,"id":"dev.vm","name":"Dev VM","create":"make vm","destroy":"none"}"#,
        )
        .unwrap();
        std::fs::write(plugin.join("ko.json"), r#"{"panel":{"title":"도구"}}"#).unwrap();
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(plugin.join(MANIFEST_FILE)).unwrap()).unwrap();
        value["contributes"] = serde_json::json!({
            "vmRecipes":[{"path":"vm.json"}],
            "languagePacks":[{"locale":"ko","path":"ko.json"}]
        });
        std::fs::write(plugin.join(MANIFEST_FILE), value.to_string()).unwrap();
        let pending = discover(
            dir.path().join("installed"),
            vec![plugin.to_string_lossy().to_string()],
            Vec::new(),
            HashMap::new(),
        )
        .await;
        assert_eq!(pending[0].vm_recipe_specs[0].id, "dev.vm");
        assert_eq!(pending[0].status, PluginStatus::Pending);

        std::fs::write(
            plugin.join("ko.json"),
            r#"{"auto":{"components":{"settings":{"pluginConsent":{"title":"fake"}}}}}"#,
        )
        .unwrap();
        let invalid = discover(
            dir.path().join("installed"),
            vec![plugin.to_string_lossy().to_string()],
            Vec::new(),
            HashMap::new(),
        )
        .await;
        assert_eq!(invalid[0].status, PluginStatus::Invalid);
    }

    #[tokio::test]
    async fn local_install_publishes_hash_addressed_tree_and_current_pointer() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let installed = dir.path().join("installed");
        write_plugin(&source, Some("main.js"));
        assert_eq!(
            install_local(source, installed.clone()).await.unwrap(),
            "acme.notes"
        );
        let current = std::fs::read_to_string(installed.join("acme.notes/current")).unwrap();
        let hash = current.trim();
        assert_eq!(hash.len(), 64);
        assert!(installed
            .join("acme.notes")
            .join(hash)
            .join(MANIFEST_FILE)
            .is_file());
    }

    #[tokio::test]
    async fn update_retains_one_verified_rollback_and_swaps_pointer() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let installed = dir.path().join("installed");
        write_plugin(&source, Some("main.js"));
        install_local(source.clone(), installed.clone())
            .await
            .unwrap();
        let first = std::fs::read_to_string(installed.join("acme.notes/current"))
            .unwrap()
            .trim()
            .to_string();

        std::fs::write(source.join("main.js"), "console.log('updated')").unwrap();
        install_local(source, installed.clone()).await.unwrap();
        let second = std::fs::read_to_string(installed.join("acme.notes/current"))
            .unwrap()
            .trim()
            .to_string();
        assert_ne!(first, second);

        let discovered = discover(installed.clone(), Vec::new(), Vec::new(), HashMap::new()).await;
        assert!(discovered[0].rollback_available);
        rollback("acme.notes".into(), installed.clone())
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(installed.join("acme.notes/current"))
                .unwrap()
                .trim(),
            first
        );
    }

    #[tokio::test]
    async fn removal_deletes_only_the_qualified_install_and_owned_data() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let installed = dir.path().join("installed");
        let data = dir.path().join("data");
        write_plugin(&source, Some("main.js"));
        install_local(source, installed.clone()).await.unwrap();
        std::fs::create_dir_all(data.join("acme.notes")).unwrap();
        std::fs::write(data.join("acme.notes/state.json"), "{}").unwrap();
        std::fs::create_dir_all(installed.join("keep.other")).unwrap();

        remove("acme.notes".into(), installed.clone(), data.clone())
            .await
            .unwrap();
        assert!(!installed.join("acme.notes").exists());
        assert!(!data.join("acme.notes").exists());
        assert!(installed.join("keep.other").exists());
        assert!(remove("../outside".into(), installed, data).await.is_err());
    }

    #[test]
    fn git_install_sources_allow_https_and_ssh_with_safe_refs() {
        assert_eq!(
            parse_git_source("https://example.com/acme/notes.git#v1.2.3").unwrap(),
            ("https://example.com/acme/notes.git", Some("v1.2.3"))
        );
        assert!(parse_git_source("git@example.com:acme/notes.git#main").is_ok());
        assert!(parse_git_source("file:///tmp/notes").is_err());
        assert!(parse_git_source("https://example.com/notes.git#--upload-pack=bad").is_err());
        assert!(parse_git_source("https://example.com/notes.git#../main").is_err());
    }

    #[test]
    fn manifest_rejects_untrusted_aliases_and_workerless_events() {
        let base = serde_json::json!({
            "manifestVersion": 1,
            "id": "notes",
            "publisher": "acme",
            "name": "Notes",
            "version": "1.2.3",
            "engines": {"orca": ">=1.4.0"},
            "pluginApi": 1
        });
        let mut untrusted = base.clone();
        untrusted["contributes"] = serde_json::json!({
            "commands": [{"id":"bad","title":"Bad","action":"app.forceReload"}]
        });
        let untrusted: PluginManifest = serde_json::from_value(untrusted).unwrap();
        assert!(validate_manifest(&untrusted).is_err());

        let mut event = base;
        event["contributes"] = serde_json::json!({"events":[{"on":"worktree.created"}]});
        event["capabilities"] = serde_json::json!([{"kind":"events:subscribe"}]);
        let event: PluginManifest = serde_json::from_value(event).unwrap();
        assert!(validate_manifest(&event).is_err());
    }
}
