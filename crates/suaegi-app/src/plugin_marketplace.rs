//! Orca-compatible, Git-backed plugin marketplace catalog.
//!
//! Marketplace data is untrusted. Sources are content-addressed, fetched with
//! system Git, validated with strict limits, and only the last valid snapshot
//! is exposed when a refresh fails.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MARKETPLACE_FILENAME: &str = "orca-marketplace.json";
pub const OFFICIAL_MARKETPLACE_URL: &str = "https://github.com/stablyai/orca-plugins.git";
pub const SOURCE_LIMIT: usize = 64;
const ENTRY_LIMIT: usize = 2_048;
const CATEGORY_LIMIT: usize = 16;
const SOURCE_FILE_MAX_BYTES: u64 = 2 * 1024 * 1024;
const SNAPSHOT_FILE_MAX_BYTES: u64 = 16 * 1024 * 1024;
const INDEX_MAX_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MarketplaceGitSource {
    pub kind: String,
    pub url: String,
    #[serde(rename = "ref")]
    pub git_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MarketplaceEntry {
    pub id: String,
    pub source: MarketplaceGitSource,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub categories: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Marketplace {
    pub name: String,
    pub owner: String,
    pub plugins: Vec<MarketplaceEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RegisteredSource {
    pub id: String,
    pub source: MarketplaceGitSource,
    #[serde(rename = "addedAt")]
    pub added_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CachedSnapshot {
    #[serde(rename = "schemaVersion")]
    pub schema_version: u8,
    #[serde(rename = "sourceId")]
    pub source_id: String,
    pub source: MarketplaceGitSource,
    #[serde(rename = "marketplaceCommit")]
    pub marketplace_commit: String,
    #[serde(rename = "fetchedAt")]
    pub fetched_at: u64,
    pub marketplace: Marketplace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceState {
    pub registration: RegisteredSource,
    pub snapshot: Option<CachedSnapshot>,
    pub refresh_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogListing {
    pub marketplace_source_id: String,
    pub marketplace_commit: String,
    pub marketplace_name: String,
    pub marketplace_owner: String,
    pub entry: MarketplaceEntry,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SourceFile {
    #[serde(rename = "schemaVersion")]
    schema_version: u8,
    sources: Vec<RegisteredSource>,
}

pub fn default_marketplaces_dir() -> PathBuf {
    crate::plugins::default_plugins_data_dir().join("marketplaces")
}

pub fn source_id(source: &MarketplaceGitSource) -> Result<String, String> {
    validate_git_source(source)?;
    let mut digest = Sha256::new();
    digest.update(b"orca-plugin-marketplace-source-v1\0");
    digest.update(source.url.as_bytes());
    digest.update(b"\0");
    digest.update(source.git_ref.as_bytes());
    Ok(format!("{:x}", digest.finalize())[..32].to_string())
}

pub fn is_official_source(source: &MarketplaceGitSource) -> bool {
    source.url.trim_end_matches('/') == OFFICIAL_MARKETPLACE_URL && source.git_ref == "main"
}

fn is_stablyai_git_source(url: &str) -> bool {
    let normalized = url.trim().to_ascii_lowercase();
    normalized.starts_with("https://github.com/stablyai/")
        || normalized.starts_with("ssh://git@github.com/stablyai/")
        || normalized.starts_with("git@github.com:stablyai/")
}

fn reserved_plugin_identity(plugin_key: &str) -> bool {
    plugin_key
        .split_once('.')
        .is_some_and(|(publisher, id)| publisher == "stablyai" || id.starts_with("orca-"))
}

fn valid_slug(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

fn valid_qualified_key(value: &str) -> bool {
    value
        .split_once('.')
        .is_some_and(|(publisher, id)| valid_slug(publisher, 64) && valid_slug(id, 64))
}

fn valid_owner(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && value
            .bytes()
            .last()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
}

fn validate_git_source(source: &MarketplaceGitSource) -> Result<(), String> {
    if source.kind != "git" {
        return Err("Marketplace source kind must be git.".into());
    }
    let url = source.url.trim();
    let allowed = url.starts_with("https://")
        || url.starts_with("ssh://")
        || url
            .split_once('@')
            .is_some_and(|(user, rest)| !user.is_empty() && rest.contains(':'));
    if !allowed || url.len() > 32 * 1024 || url.chars().any(char::is_control) {
        return Err("Marketplace Git URL must use HTTPS or SSH.".into());
    }
    let git_ref = source.git_ref.trim();
    if git_ref.is_empty()
        || git_ref.len() > 4_096
        || git_ref.starts_with('-')
        || git_ref.starts_with('/')
        || git_ref.contains("..")
        || git_ref.chars().any(char::is_control)
    {
        return Err("Marketplace Git ref is invalid.".into());
    }
    Ok(())
}

fn validate_marketplace(marketplace: &Marketplace) -> Result<(), String> {
    if marketplace.name.is_empty() || marketplace.name.len() > 256 {
        return Err("Marketplace name is invalid.".into());
    }
    if !valid_owner(&marketplace.owner) {
        return Err("Marketplace owner is invalid.".into());
    }
    if marketplace.plugins.len() > ENTRY_LIMIT {
        return Err(format!(
            "Marketplace exceeds the {ENTRY_LIMIT}-plugin limit."
        ));
    }
    let mut ids = HashSet::new();
    for plugin in &marketplace.plugins {
        if !valid_qualified_key(&plugin.id) || !ids.insert(plugin.id.as_str()) {
            return Err("Marketplace plugin identities are invalid or duplicated.".into());
        }
        validate_git_source(&plugin.source)?;
        if plugin
            .description
            .as_ref()
            .is_some_and(|description| description.is_empty() || description.len() > 4_096)
        {
            return Err("Marketplace plugin description is invalid.".into());
        }
        if plugin.categories.len() > CATEGORY_LIMIT {
            return Err("Marketplace plugin has too many categories.".into());
        }
        let mut categories = HashSet::new();
        if plugin
            .categories
            .iter()
            .any(|category| !valid_slug(category, 64) || !categories.insert(category))
        {
            return Err("Marketplace categories are invalid or duplicated.".into());
        }
    }
    Ok(())
}

fn read_bounded_json<T: for<'de> Deserialize<'de>>(path: &Path, limit: u64) -> Result<T, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > limit {
        return Err(format!("{} is not a bounded regular file.", path.display()));
    }
    serde_json::from_slice(
        &std::fs::read(path)
            .map_err(|error| format!("Could not read {}: {error}", path.display()))?,
    )
    .map_err(|error| format!("Invalid {}: {error}", path.display()))
}

fn write_atomic_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Marketplace data path has no parent.".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create {}: {error}", parent.display()))?;
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("Could not encode marketplace data: {error}"))?;
    let temp = parent.join(format!(
        ".{}.tmp.{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("marketplace"),
        std::process::id()
    ));
    std::fs::write(&temp, bytes)
        .map_err(|error| format!("Could not write {}: {error}", temp.display()))?;
    std::fs::rename(&temp, path)
        .map_err(|error| format!("Could not publish {}: {error}", path.display()))
}

fn sources_path(root: &Path) -> PathBuf {
    root.join("sources.json")
}

fn snapshot_path(root: &Path, id: &str) -> PathBuf {
    root.join("snapshots").join(format!("{id}.json"))
}

pub fn load_sources(root: &Path) -> Result<Vec<RegisteredSource>, String> {
    let path = sources_path(root);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file: SourceFile = read_bounded_json(&path, SOURCE_FILE_MAX_BYTES)?;
    if file.schema_version != 1 || file.sources.len() > SOURCE_LIMIT {
        return Err(
            "Marketplace source file has an unsupported schema or too many entries.".into(),
        );
    }
    let mut ids = HashSet::new();
    for source in &file.sources {
        if source.id != source_id(&source.source)? || !ids.insert(source.id.as_str()) {
            return Err("Marketplace source identity is inconsistent or duplicated.".into());
        }
    }
    Ok(file.sources)
}

fn save_sources(root: &Path, sources: Vec<RegisteredSource>) -> Result<(), String> {
    if sources.len() > SOURCE_LIMIT {
        return Err(format!(
            "Marketplace source limit ({SOURCE_LIMIT}) reached."
        ));
    }
    write_atomic_json(
        &sources_path(root),
        &SourceFile {
            schema_version: 1,
            sources,
        },
    )
}

pub fn add_source(root: &Path, source: MarketplaceGitSource) -> Result<RegisteredSource, String> {
    let id = source_id(&source)?;
    let mut sources = load_sources(root)?;
    if let Some(existing) = sources.iter().find(|candidate| candidate.id == id) {
        return Ok(existing.clone());
    }
    if sources.len() >= SOURCE_LIMIT {
        return Err(format!(
            "Marketplace source limit ({SOURCE_LIMIT}) reached."
        ));
    }
    let registration = RegisteredSource {
        id,
        source,
        added_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX),
    };
    sources.push(registration.clone());
    save_sources(root, sources)?;
    Ok(registration)
}

pub fn remove_source(root: &Path, id: &str) -> Result<bool, String> {
    if id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Marketplace source ID is invalid.".into());
    }
    let mut sources = load_sources(root)?;
    if sources
        .iter()
        .any(|source| source.id == id && is_official_source(&source.source))
    {
        return Err("The official marketplace is managed by Suaegi and cannot be removed.".into());
    }
    let before = sources.len();
    sources.retain(|source| source.id != id);
    let removed = sources.len() != before;
    if removed {
        save_sources(root, sources)?;
        match std::fs::remove_file(snapshot_path(root, id)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("Could not remove marketplace cache: {error}")),
        }
    }
    Ok(removed)
}

pub fn read_snapshot(root: &Path, id: &str) -> Result<Option<CachedSnapshot>, String> {
    let path = snapshot_path(root, id);
    if !path.exists() {
        return Ok(None);
    }
    let snapshot: CachedSnapshot = read_bounded_json(&path, SNAPSHOT_FILE_MAX_BYTES)?;
    if snapshot.schema_version != 1
        || snapshot.source_id != id
        || snapshot.source_id != source_id(&snapshot.source)?
        || snapshot.marketplace_commit.len() != 40
        || !snapshot
            .marketplace_commit
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err("Marketplace snapshot identity is invalid.".into());
    }
    validate_marketplace(&snapshot.marketplace)?;
    Ok(Some(snapshot))
}

async fn git_output(args: &[String]) -> Result<String, String> {
    let output = tokio::process::Command::new("git")
        .args(args)
        .output()
        .await
        .map_err(|error| format!("Could not start system Git: {error}"))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
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

pub async fn refresh_source(
    root: PathBuf,
    source: RegisteredSource,
) -> Result<CachedSnapshot, String> {
    if source.id != source_id(&source.source)? {
        return Err("Marketplace source identity is inconsistent.".into());
    }
    let staging = tempfile::tempdir()
        .map_err(|error| format!("Could not create marketplace staging directory: {error}"))?;
    let checkout = staging.path().join("checkout");
    git_output(&[
        "clone".into(),
        "--filter=blob:none".into(),
        "--no-checkout".into(),
        "--".into(),
        source.source.url.clone(),
        checkout.to_string_lossy().into_owned(),
    ])
    .await?;
    git_output(&[
        "-C".into(),
        checkout.to_string_lossy().into_owned(),
        "fetch".into(),
        "--depth=1".into(),
        "origin".into(),
        source.source.git_ref.clone(),
    ])
    .await?;
    git_output(&[
        "-C".into(),
        checkout.to_string_lossy().into_owned(),
        "checkout".into(),
        "--detach".into(),
        "--force".into(),
        "FETCH_HEAD".into(),
    ])
    .await?;
    let commit = git_output(&[
        "-C".into(),
        checkout.to_string_lossy().into_owned(),
        "rev-parse".into(),
        "HEAD".into(),
    ])
    .await?;
    let marketplace: Marketplace =
        read_bounded_json(&checkout.join(MARKETPLACE_FILENAME), INDEX_MAX_BYTES)?;
    validate_marketplace(&marketplace)?;
    if is_official_source(&source.source) && !marketplace.owner.eq_ignore_ascii_case("stablyai") {
        return Err("Official marketplace metadata has an unexpected owner.".into());
    }
    if marketplace.plugins.iter().any(|entry| {
        reserved_plugin_identity(&entry.id) && !is_stablyai_git_source(&entry.source.url)
    }) {
        return Err(
            "A reserved stablyai/orca plugin identity points outside the stablyai organization."
                .into(),
        );
    }
    let snapshot = CachedSnapshot {
        schema_version: 1,
        source_id: source.id.clone(),
        source: source.source,
        marketplace_commit: commit,
        fetched_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX),
        marketplace,
    };
    write_atomic_json(&snapshot_path(&root, &source.id), &snapshot)?;
    Ok(snapshot)
}

pub async fn load_catalog(root: PathBuf) -> Result<Vec<SourceState>, String> {
    let sources = load_sources(&root)?;
    Ok(sources
        .into_iter()
        .map(
            |registration| match read_snapshot(&root, &registration.id) {
                Ok(snapshot) => SourceState {
                    registration,
                    snapshot,
                    refresh_error: None,
                },
                Err(error) => SourceState {
                    registration,
                    snapshot: None,
                    refresh_error: Some(error),
                },
            },
        )
        .collect())
}

pub async fn seed_official_source(root: PathBuf) -> Result<Vec<SourceState>, String> {
    let official = MarketplaceGitSource {
        kind: "git".into(),
        url: OFFICIAL_MARKETPLACE_URL.into(),
        git_ref: "main".into(),
    };
    let registration = add_source(&root, official)?;
    if read_snapshot(&root, &registration.id)?.is_none() {
        if let Err(error) = refresh_source(root.clone(), registration.clone()).await {
            return Ok(vec![SourceState {
                registration,
                snapshot: None,
                refresh_error: Some(error),
            }]);
        }
    }
    load_catalog(root).await
}

pub async fn add_and_refresh(
    root: PathBuf,
    source: MarketplaceGitSource,
) -> Result<Vec<SourceState>, String> {
    let id = source_id(&source)?;
    let existing = load_sources(&root)?
        .into_iter()
        .find(|candidate| candidate.id == id);
    let registration = existing.clone().unwrap_or(RegisteredSource {
        id,
        source: source.clone(),
        added_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX),
    });
    refresh_source(root.clone(), registration).await?;
    if existing.is_none() {
        add_source(&root, source)?;
    }
    load_catalog(root).await
}

pub async fn refresh_catalog(
    root: PathBuf,
    only_source_id: Option<String>,
) -> Result<Vec<SourceState>, String> {
    let sources = load_sources(&root)?;
    let mut states = Vec::with_capacity(sources.len());
    for source in sources {
        if only_source_id.as_deref().is_none_or(|id| id == source.id) {
            match refresh_source(root.clone(), source.clone()).await {
                Ok(snapshot) => states.push(SourceState {
                    registration: source,
                    snapshot: Some(snapshot),
                    refresh_error: None,
                }),
                Err(error) => states.push(SourceState {
                    snapshot: read_snapshot(&root, &source.id).unwrap_or(None),
                    registration: source,
                    refresh_error: Some(error),
                }),
            }
        } else {
            states.push(SourceState {
                snapshot: read_snapshot(&root, &source.id).unwrap_or(None),
                registration: source,
                refresh_error: None,
            });
        }
    }
    Ok(states)
}

pub fn listings(states: &[SourceState]) -> Vec<CatalogListing> {
    let mut result = states
        .iter()
        .filter_map(|state| state.snapshot.as_ref().map(|snapshot| (state, snapshot)))
        .flat_map(|(state, snapshot)| {
            snapshot
                .marketplace
                .plugins
                .iter()
                .filter(|entry| listing_supported(entry))
                .cloned()
                .map(|entry| CatalogListing {
                    marketplace_source_id: state.registration.id.clone(),
                    marketplace_commit: snapshot.marketplace_commit.clone(),
                    marketplace_name: snapshot.marketplace.name.clone(),
                    marketplace_owner: snapshot.marketplace.owner.clone(),
                    entry,
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    result.sort_by(|left, right| left.entry.id.cmp(&right.entry.id));
    result
}

fn listing_supported(entry: &MarketplaceEntry) -> bool {
    let unsupported = [
        "themes",
        "icons",
        "icon-themes",
        "terminal-themes",
        "skills",
    ];
    !entry
        .categories
        .iter()
        .any(|category| unsupported.contains(&category.as_str()))
}

pub async fn install_listing(
    root: PathBuf,
    plugins_dir: PathBuf,
    source_id: String,
    marketplace_commit: String,
    plugin_key: String,
) -> Result<String, String> {
    let snapshot = read_snapshot(&root, &source_id)?
        .ok_or_else(|| "Marketplace snapshot is no longer available.".to_string())?;
    if snapshot.marketplace_commit != marketplace_commit {
        return Err(
            "Marketplace changed after review. Refresh and review the listing again.".into(),
        );
    }
    let entry = snapshot
        .marketplace
        .plugins
        .into_iter()
        .find(|entry| entry.id == plugin_key && listing_supported(entry))
        .ok_or_else(|| "Plugin is no longer listed by this marketplace.".to_string())?;
    crate::plugins::install_git_source_expected(
        entry.source.url,
        entry.source.git_ref,
        plugins_dir,
        plugin_key,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(url: &str, git_ref: &str) -> MarketplaceGitSource {
        MarketplaceGitSource {
            kind: "git".into(),
            url: url.into(),
            git_ref: git_ref.into(),
        }
    }

    #[test]
    fn source_store_is_content_addressed_and_removes_owned_snapshot_only() {
        let temp = tempfile::tempdir().unwrap();
        let first = add_source(
            temp.path(),
            source("https://example.com/plugins.git", "main"),
        )
        .unwrap();
        let duplicate = add_source(
            temp.path(),
            source("https://example.com/plugins.git", "main"),
        )
        .unwrap();
        assert_eq!(first.id, duplicate.id);
        assert_eq!(load_sources(temp.path()).unwrap().len(), 1);
        std::fs::create_dir_all(temp.path().join("snapshots")).unwrap();
        std::fs::write(snapshot_path(temp.path(), &first.id), b"invalid").unwrap();
        let sentinel = temp.path().join("snapshots").join("sentinel");
        std::fs::write(&sentinel, b"keep").unwrap();
        assert!(remove_source(temp.path(), &first.id).unwrap());
        assert!(sentinel.exists());
        assert!(load_sources(temp.path()).unwrap().is_empty());
    }

    #[test]
    fn marketplace_validation_rejects_duplicate_ids_and_unsupported_sources() {
        let entry = MarketplaceEntry {
            id: "acme.notes".into(),
            source: source("https://example.com/notes.git", "v1"),
            description: None,
            categories: vec!["productivity".into()],
        };
        let marketplace = Marketplace {
            name: "Acme".into(),
            owner: "acme".into(),
            plugins: vec![entry.clone(), entry],
        };
        assert!(validate_marketplace(&marketplace).is_err());
        assert!(source_id(&source("file:///tmp/plugins", "main")).is_err());
    }

    #[test]
    fn catalog_hides_categories_orca_cannot_install() {
        let registration = add_source(
            tempfile::tempdir().unwrap().path(),
            source("https://x.test/m.git", "main"),
        )
        .unwrap();
        let make = |id: &str, category: &str| MarketplaceEntry {
            id: id.into(),
            source: source("https://x.test/p.git", "main"),
            description: None,
            categories: vec![category.into()],
        };
        let states = vec![SourceState {
            registration: registration.clone(),
            snapshot: Some(CachedSnapshot {
                schema_version: 1,
                source_id: registration.id,
                source: registration.source,
                marketplace_commit: "a".repeat(40),
                fetched_at: 1,
                marketplace: Marketplace {
                    name: "Test".into(),
                    owner: "test".into(),
                    plugins: vec![
                        make("acme.notes", "productivity"),
                        make("acme.theme", "themes"),
                    ],
                },
            }),
            refresh_error: None,
        }];
        assert_eq!(listings(&states).len(), 1);
        assert_eq!(listings(&states)[0].entry.id, "acme.notes");
    }
}
