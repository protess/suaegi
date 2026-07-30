//! Capability-scoped persistent services for Orca plugin API v1 workers.
//!
//! Process supervision is kept separate from these stores so every future
//! worker and panel bridge uses the same bounded, plugin-private implementation.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use suaegi_secrets::{Secret, SecretRequest};

const STORAGE_VALUE_MAX_BYTES: usize = 256 * 1024;
const STORAGE_TOTAL_MAX_BYTES: usize = 5 * 1024 * 1024;
const STORAGE_KEY_LIMIT: usize = 1024;
const SECRET_VALUE_MAX_BYTES: usize = 64 * 1024;

pub fn required_capability(method: &str) -> Option<&'static str> {
    match method {
        "workspace.readContext" => Some("workspace:read"),
        "terminal.sendText" => Some("terminal:send"),
        "notifications.show" => Some("notifications:show"),
        "storage.get" | "storage.set" | "storage.delete" | "storage.keys" => Some("storage"),
        "secrets.get" | "secrets.set" | "secrets.delete" => Some("secrets"),
        "settings.get" | "settings.set" => Some("settings:own"),
        "events.subscribe" => Some("events:subscribe"),
        _ => None,
    }
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

fn safe_plugin_key(value: &str) -> bool {
    value
        .split_once('.')
        .is_some_and(|(publisher, id)| safe_id(publisher) && safe_id(id))
}

fn safe_storage_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && !matches!(value, "__proto__" | "prototype" | "constructor")
        && !value.contains('\0')
}

fn plugin_directory(root: &Path, plugin_key: &str) -> Result<PathBuf, String> {
    if !safe_plugin_key(plugin_key) {
        return Err("invalid qualified plugin key".into());
    }
    std::fs::create_dir_all(root)
        .map_err(|error| format!("could not create plugin data root: {error}"))?;
    let root = root
        .canonicalize()
        .map_err(|error| format!("could not resolve plugin data root: {error}"))?;
    let directory = root.join(plugin_key);
    if directory.exists() {
        let metadata = std::fs::symlink_metadata(&directory)
            .map_err(|error| format!("could not inspect plugin data directory: {error}"))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err("plugin data path is not a private directory".into());
        }
    } else {
        std::fs::create_dir(&directory)
            .map_err(|error| format!("could not create plugin data directory: {error}"))?;
    }
    let resolved = directory
        .canonicalize()
        .map_err(|error| format!("could not resolve plugin data directory: {error}"))?;
    if !resolved.starts_with(&root) || resolved == root {
        return Err("plugin data path escapes its private root".into());
    }
    Ok(resolved)
}

fn load_map(path: &Path) -> Result<BTreeMap<String, Value>, String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(BTreeMap::new());
        }
        Err(error) => return Err(format!("could not inspect plugin store: {error}")),
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() as usize > STORAGE_TOTAL_MAX_BYTES
    {
        return Err("plugin store is not a bounded regular file".into());
    }
    let value: Value = serde_json::from_slice(
        &std::fs::read(path).map_err(|error| format!("could not read plugin store: {error}"))?,
    )
    .map_err(|_| "plugin store contains invalid JSON".to_string())?;
    let Value::Object(entries) = value else {
        return Err("plugin store root must be an object".into());
    };
    Ok(entries.into_iter().collect())
}

fn save_map(path: &Path, values: &BTreeMap<String, Value>) -> Result<(), String> {
    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
    if values.len() > STORAGE_KEY_LIMIT {
        return Err("plugin store exceeds the key limit".into());
    }
    let bytes =
        serde_json::to_vec(values).map_err(|_| "could not encode plugin store".to_string())?;
    if bytes.len() > STORAGE_TOTAL_MAX_BYTES {
        return Err("plugin store exceeds the total byte limit".into());
    }
    let temporary = path.with_extension(format!(
        "tmp.{}.{}",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&temporary, bytes)
        .map_err(|error| format!("could not write plugin store: {error}"))?;
    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("could not publish plugin store: {error}"));
    }
    Ok(())
}

fn store_path(root: &Path, plugin_key: &str, name: &str) -> Result<PathBuf, String> {
    if !matches!(name, "storage.json" | "settings.json") {
        return Err("invalid plugin store name".into());
    }
    Ok(plugin_directory(root, plugin_key)?.join(name))
}

pub fn get(root: &Path, plugin_key: &str, store: &str, key: &str) -> Result<Option<Value>, String> {
    if !safe_storage_key(key) {
        return Err("invalid plugin storage key".into());
    }
    Ok(load_map(&store_path(root, plugin_key, store)?)?
        .get(key)
        .cloned())
}

pub fn set(
    root: &Path,
    plugin_key: &str,
    store: &str,
    key: &str,
    value: Value,
) -> Result<(), String> {
    if !safe_storage_key(key) {
        return Err("invalid plugin storage key".into());
    }
    let encoded =
        serde_json::to_vec(&value).map_err(|_| "plugin value is not valid JSON".to_string())?;
    if encoded.len() > STORAGE_VALUE_MAX_BYTES {
        return Err("plugin value exceeds the byte limit".into());
    }
    let path = store_path(root, plugin_key, store)?;
    let mut values = load_map(&path)?;
    if !values.contains_key(key) && values.len() >= STORAGE_KEY_LIMIT {
        return Err("plugin store exceeds the key limit".into());
    }
    values.insert(key.to_string(), value);
    save_map(&path, &values)
}

pub fn delete(root: &Path, plugin_key: &str, store: &str, key: &str) -> Result<(), String> {
    if !safe_storage_key(key) {
        return Err("invalid plugin storage key".into());
    }
    let path = store_path(root, plugin_key, store)?;
    let mut values = load_map(&path)?;
    values.remove(key);
    save_map(&path, &values)
}

pub fn keys(root: &Path, plugin_key: &str, store: &str) -> Result<Vec<String>, String> {
    Ok(load_map(&store_path(root, plugin_key, store)?)?
        .into_keys()
        .collect())
}

fn secret_service(plugin_key: &str) -> Result<String, String> {
    safe_plugin_key(plugin_key)
        .then(|| format!("suaegi-plugin-{plugin_key}"))
        .ok_or_else(|| "invalid qualified plugin key".into())
}

pub fn secret_get(plugin_key: &str, key: &str) -> Result<Option<String>, String> {
    if !safe_storage_key(key) {
        return Err("invalid plugin secret key".into());
    }
    let service = secret_service(plugin_key)?;
    let resolved = suaegi_secrets::load(&SecretRequest::new(&service, key));
    if resolved.keychain_error.is_some() {
        return Err("plugin secret storage failed".into());
    }
    Ok(resolved.secret.map(|secret| secret.expose().to_string()))
}

pub fn secret_set(plugin_key: &str, key: &str, value: String) -> Result<(), String> {
    if !safe_storage_key(key) || value.len() > SECRET_VALUE_MAX_BYTES || value.contains('\0') {
        return Err("invalid or oversized plugin secret".into());
    }
    suaegi_secrets::store(&secret_service(plugin_key)?, key, &Secret::new(value))
        .map_err(|_| "plugin secret storage failed".to_string())
}

pub fn secret_delete(plugin_key: &str, key: &str) -> Result<(), String> {
    if !safe_storage_key(key) {
        return Err("invalid plugin secret key".into());
    }
    suaegi_secrets::delete(&secret_service(plugin_key)?, key)
        .map_err(|_| "plugin secret storage failed".to_string())
}

fn object_params(
    params: Value,
    allowed: &[&str],
) -> Result<serde_json::Map<String, Value>, String> {
    let Value::Object(params) = params else {
        return Err("plugin host-call params must be an object".into());
    };
    if params.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err("plugin host-call params contain an unknown field".into());
    }
    Ok(params)
}

fn string_param<'a>(
    params: &'a serde_json::Map<String, Value>,
    name: &str,
) -> Result<&'a str, String> {
    params
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("plugin host-call requires string field {name}"))
}

/// Dispatches the plugin-private half of the v1 host API. UI-scoped methods
/// are intentionally handled by `AppState`, but share this capability table.
pub fn invoke_private(
    data_root: &Path,
    plugin_key: &str,
    capabilities: &[String],
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let capability =
        required_capability(method).ok_or_else(|| "unknown plugin host method".to_string())?;
    if !capabilities.iter().any(|granted| granted == capability) {
        return Err(format!("plugin capability {capability} was not granted"));
    }
    match method {
        "storage.get" => {
            let params = object_params(params, &["key"])?;
            Ok(serde_json::json!({
                "value": get(
                    data_root,
                    plugin_key,
                    "storage.json",
                    string_param(&params, "key")?
                )?
                .unwrap_or(Value::Null)
            }))
        }
        "storage.set" => {
            let mut params = object_params(params, &["key", "value"])?;
            let key = string_param(&params, "key")?.to_string();
            let value = params
                .remove("value")
                .ok_or_else(|| "plugin host-call requires field value".to_string())?;
            set(data_root, plugin_key, "storage.json", &key, value)?;
            Ok(serde_json::json!({"ok": true}))
        }
        "storage.delete" => {
            let params = object_params(params, &["key"])?;
            delete(
                data_root,
                plugin_key,
                "storage.json",
                string_param(&params, "key")?,
            )?;
            Ok(serde_json::json!({"ok": true}))
        }
        "storage.keys" => {
            let _ = object_params(params, &[])?;
            Ok(serde_json::json!({
                "keys": keys(data_root, plugin_key, "storage.json")?
            }))
        }
        "settings.get" => {
            let _ = object_params(params, &[])?;
            Ok(serde_json::json!({
                "settings": load_map(&store_path(data_root, plugin_key, "settings.json")?)?
            }))
        }
        "settings.set" => {
            let mut params = object_params(params, &["key", "value"])?;
            let key = string_param(&params, "key")?.to_string();
            let value = params
                .remove("value")
                .ok_or_else(|| "plugin host-call requires field value".to_string())?;
            set(data_root, plugin_key, "settings.json", &key, value)?;
            Ok(serde_json::json!({"ok": true}))
        }
        "secrets.get" => {
            let params = object_params(params, &["key"])?;
            Ok(serde_json::json!({
                "value": secret_get(plugin_key, string_param(&params, "key")?)?
            }))
        }
        "secrets.set" => {
            let params = object_params(params, &["key", "value"])?;
            secret_set(
                plugin_key,
                string_param(&params, "key")?,
                string_param(&params, "value")?.to_string(),
            )?;
            Ok(serde_json::json!({"ok": true}))
        }
        "secrets.delete" => {
            let params = object_params(params, &["key"])?;
            secret_delete(plugin_key, string_param(&params, "key")?)?;
            Ok(serde_json::json!({"ok": true}))
        }
        _ => Err("plugin host method requires an active app context".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_json_stores_are_bounded_atomic_and_namespaced() {
        let root = tempfile::tempdir().unwrap();
        set(
            root.path(),
            "acme.notes",
            "storage.json",
            "theme",
            serde_json::json!({"dark": true}),
        )
        .unwrap();
        assert_eq!(
            get(root.path(), "acme.notes", "storage.json", "theme").unwrap(),
            Some(serde_json::json!({"dark": true}))
        );
        assert_eq!(
            keys(root.path(), "acme.notes", "storage.json").unwrap(),
            vec!["theme"]
        );
        delete(root.path(), "acme.notes", "storage.json", "theme").unwrap();
        assert_eq!(
            get(root.path(), "acme.notes", "storage.json", "theme").unwrap(),
            None
        );
        assert!(set(
            root.path(),
            "acme.notes",
            "storage.json",
            "__proto__",
            Value::Null
        )
        .is_err());

        let result = invoke_private(
            root.path(),
            "acme.notes",
            &["storage".into()],
            "storage.set",
            serde_json::json!({"key":"count","value":3}),
        )
        .unwrap();
        assert_eq!(result, serde_json::json!({"ok":true}));
        assert!(invoke_private(
            root.path(),
            "acme.notes",
            &[],
            "storage.get",
            serde_json::json!({"key":"count"})
        )
        .is_err());
        assert!(set(
            root.path(),
            "../outside",
            "storage.json",
            "key",
            Value::Null
        )
        .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn private_store_rejects_symlinked_plugin_directories() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("acme.notes")).unwrap();
        assert!(set(
            root.path(),
            "acme.notes",
            "storage.json",
            "key",
            Value::Null
        )
        .is_err());
    }
}
