//! Cached emergency revocation list for third-party plugins.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Duration, Utc};
use futures::StreamExt;
use serde::{Deserialize, Serialize};

pub const KILL_LIST_URL: &str = "https://onorca.dev/plugins/kill-list.json";
const MAX_BYTES: u64 = 4 * 1024 * 1024;
const ENTRY_LIMIT: usize = 4_096;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct KillListEntry {
    pub plugin_key: String,
    pub reason: String,
    #[serde(default)]
    pub advisory_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PluginKillList {
    pub version: u8,
    pub generated_at: String,
    pub plugins: Vec<KillListEntry>,
}

pub fn default_cache_path() -> PathBuf {
    crate::plugins::default_plugins_data_dir().join("plugin-kill-list.json")
}

fn valid_slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

fn validate(list: &PluginKillList) -> Result<DateTime<chrono::FixedOffset>, String> {
    if list.version != 1 || list.plugins.len() > ENTRY_LIMIT {
        return Err("Plugin safety list has an unsupported version or too many entries.".into());
    }
    let generated_at = DateTime::parse_from_rfc3339(&list.generated_at)
        .map_err(|_| "Plugin safety-list generatedAt is invalid.".to_string())?;
    let mut keys = HashSet::new();
    for entry in &list.plugins {
        let key_valid = entry
            .plugin_key
            .split_once('.')
            .is_some_and(|(publisher, id)| valid_slug(publisher) && valid_slug(id));
        if !key_valid || !keys.insert(entry.plugin_key.as_str()) {
            return Err("Plugin safety-list identities are invalid or duplicated.".into());
        }
        if entry.reason.is_empty() || entry.reason.len() > 1_024 {
            return Err("Plugin safety-list reason is invalid.".into());
        }
        if let Some(advisory) = &entry.advisory_url {
            let parsed = url::Url::parse(advisory)
                .map_err(|_| "Plugin advisory URL is invalid.".to_string())?;
            if parsed.scheme() != "https" || advisory.len() > 2_048 {
                return Err("Plugin advisory URL must use HTTPS.".into());
            }
        }
    }
    Ok(generated_at)
}

pub fn read_cache(path: &Path) -> Result<Option<PluginKillList>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("Could not inspect plugin safety-list cache: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > MAX_BYTES {
        return Err("Plugin safety-list cache is not a bounded regular file.".into());
    }
    let list: PluginKillList = serde_json::from_slice(
        &std::fs::read(path)
            .map_err(|error| format!("Could not read plugin safety-list cache: {error}"))?,
    )
    .map_err(|error| format!("Plugin safety-list cache is invalid: {error}"))?;
    validate(&list)?;
    Ok(Some(list))
}

fn write_cache(path: &Path, list: &PluginKillList) -> Result<(), String> {
    validate(list)?;
    let parent = path
        .parent()
        .ok_or_else(|| "Plugin safety-list cache path has no parent.".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create plugin safety-list directory: {error}"))?;
    let temp = parent.join(format!(".plugin-kill-list.tmp.{}", std::process::id()));
    std::fs::write(
        &temp,
        serde_json::to_vec_pretty(list)
            .map_err(|error| format!("Could not encode plugin safety list: {error}"))?,
    )
    .map_err(|error| format!("Could not write plugin safety-list cache: {error}"))?;
    std::fs::rename(&temp, path)
        .map_err(|error| format!("Could not publish plugin safety-list cache: {error}"))
}

pub async fn fetch() -> Result<PluginKillList, String> {
    let response = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|error| format!("Could not create plugin safety-list client: {error}"))?
        .get(KILL_LIST_URL)
        .header(reqwest::header::CACHE_CONTROL, "no-store")
        .send()
        .await
        .map_err(|error| format!("Could not download plugin safety list: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "Plugin safety-list request failed with HTTP {}.",
            response.status()
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_BYTES)
    {
        return Err("Plugin safety-list response exceeds its size limit.".into());
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk
            .map_err(|error| format!("Could not read plugin safety-list response: {error}"))?;
        if bytes.len().saturating_add(chunk.len()) > MAX_BYTES as usize {
            return Err("Plugin safety-list response exceeds its size limit.".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let list: PluginKillList = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Invalid plugin safety-list response: {error}"))?;
    let generated_at = validate(&list)?;
    let now = DateTime::<Utc>::from(SystemTime::now()).fixed_offset();
    if generated_at > now + Duration::hours(24) {
        return Err("Refusing a plugin safety list generated too far in the future.".into());
    }
    Ok(list)
}

pub async fn refresh(path: PathBuf) -> Result<PluginKillList, String> {
    let current = read_cache(&path).ok().flatten();
    let fetched = fetch().await?;
    let fetched_at = validate(&fetched)?;
    if let Some(current) = &current {
        let current_at = validate(current)?;
        if fetched_at < current_at {
            return Err(
                "Refusing to replace the plugin safety list with an older snapshot.".into(),
            );
        }
    }
    write_cache(&path, &fetched)?;
    Ok(fetched)
}

pub fn find<'a>(list: &'a PluginKillList, plugin_key: &str) -> Option<&'a KillListEntry> {
    list.plugins
        .iter()
        .find(|entry| entry.plugin_key == plugin_key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(date: &str) -> PluginKillList {
        PluginKillList {
            version: 1,
            generated_at: date.into(),
            plugins: vec![KillListEntry {
                plugin_key: "acme.unsafe".into(),
                reason: "Malware advisory".into(),
                advisory_url: Some("https://example.com/advisory".into()),
            }],
        }
    }

    #[test]
    fn cache_round_trip_is_bounded_and_strict() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("kill-list.json");
        write_cache(&path, &list("2026-07-12T20:00:00Z")).unwrap();
        assert_eq!(
            find(&read_cache(&path).unwrap().unwrap(), "acme.unsafe")
                .unwrap()
                .reason,
            "Malware advisory"
        );
        std::fs::write(&path, br#"{"version":1,"generatedAt":"bad","plugins":[]}"#).unwrap();
        assert!(read_cache(&path).is_err());
    }

    #[test]
    fn validation_rejects_duplicate_keys_and_non_https_advisories() {
        let mut value = list("2026-07-12T20:00:00Z");
        value.plugins.push(value.plugins[0].clone());
        assert!(validate(&value).is_err());
        value.plugins.pop();
        value.plugins[0].advisory_url = Some("http://example.com".into());
        assert!(validate(&value).is_err());
    }
}
