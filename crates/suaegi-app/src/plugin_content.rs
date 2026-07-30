//! Strict parsers for instructional plugin content packs.

use std::collections::HashSet;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

const LANGUAGE_PACK_MAX_BYTES: u64 = 5 * 1024 * 1024;
const LANGUAGE_PACK_MAX_ENTRIES: usize = 20_000;
const LANGUAGE_PACK_MAX_DEPTH: usize = 16;
const VM_RECIPE_MAX_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VmRecipe {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub create: String,
    pub suspend: Option<String>,
    pub resume: Option<String>,
    pub destroy: Option<String>,
    pub destroy_disabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawVmRecipe {
    schema_version: u8,
    id: String,
    name: String,
    #[serde(default)]
    description: Option<String>,
    create: String,
    #[serde(default)]
    suspend: Option<String>,
    #[serde(default)]
    resume: Option<String>,
    #[serde(default)]
    destroy: Option<String>,
}

fn read_contained(root: &Path, relative: &str, max_bytes: u64) -> Result<Vec<u8>, String> {
    let root = root
        .canonicalize()
        .map_err(|error| format!("Could not resolve plugin root: {error}"))?;
    let path = root
        .join(relative)
        .canonicalize()
        .map_err(|error| format!("Could not resolve plugin artifact {relative}: {error}"))?;
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|error| format!("Could not inspect plugin artifact {relative}: {error}"))?;
    if !path.starts_with(&root)
        || metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > max_bytes
    {
        return Err(format!(
            "Plugin artifact {relative} is not a contained bounded regular file."
        ));
    }
    std::fs::read(path)
        .map_err(|error| format!("Could not read plugin artifact {relative}: {error}"))
}

fn valid_command(command: &str) -> bool {
    !command.trim().is_empty() && command.len() <= 32 * 1024 && !command.contains('\0')
}

pub fn parse_vm_recipe(root: &Path, relative: &str) -> Result<VmRecipe, String> {
    let raw: RawVmRecipe =
        serde_json::from_slice(&read_contained(root, relative, VM_RECIPE_MAX_BYTES)?)
            .map_err(|error| format!("VM recipe {relative} is invalid JSON: {error}"))?;
    let valid_id = !raw.id.is_empty()
        && raw.id.len() <= 64
        && raw.id.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || (index > 0 && matches!(byte, b'.' | b'_' | b'-'))
        });
    if raw.schema_version != 1
        || !valid_id
        || raw.name.trim().is_empty()
        || raw.name.len() > 128
        || raw
            .description
            .as_ref()
            .is_some_and(|description| description.trim().is_empty() || description.len() > 1_024)
        || !valid_command(&raw.create)
        || raw
            .suspend
            .as_deref()
            .is_some_and(|value| !valid_command(value))
        || raw
            .resume
            .as_deref()
            .is_some_and(|value| !valid_command(value))
        || raw
            .destroy
            .as_deref()
            .is_some_and(|value| value != "none" && !valid_command(value))
        || raw.suspend.is_some() != raw.resume.is_some()
    {
        return Err(format!(
            "VM recipe {relative} does not match the Orca v1 schema."
        ));
    }
    let destroy_disabled = raw.destroy.as_deref() == Some("none");
    Ok(VmRecipe {
        id: raw.id,
        name: raw.name.trim().to_string(),
        description: raw.description.map(|value| value.trim().to_string()),
        create: raw.create.trim().to_string(),
        suspend: raw.suspend.map(|value| value.trim().to_string()),
        resume: raw.resume.map(|value| value.trim().to_string()),
        destroy: (!destroy_disabled)
            .then_some(raw.destroy)
            .flatten()
            .map(|value| value.trim().to_string()),
        destroy_disabled,
    })
}

fn parse_language_pack(root: &Path, relative: &str) -> Result<(Value, usize), String> {
    let value: Value =
        serde_json::from_slice(&read_contained(root, relative, LANGUAGE_PACK_MAX_BYTES)?)
            .map_err(|_| format!("Language pack {relative} must contain one JSON object."))?;
    let Value::Object(catalog) = &value else {
        return Err(format!("Language pack {relative} root must be an object."));
    };
    let mut stack = vec![(catalog, String::new(), 0usize)];
    let mut entries = 0usize;
    while let Some((object, prefix, depth)) = stack.pop() {
        if depth > LANGUAGE_PACK_MAX_DEPTH {
            return Err(format!(
                "Language pack {relative} exceeds depth {LANGUAGE_PACK_MAX_DEPTH}."
            ));
        }
        for (key, value) in object {
            entries += 1;
            if entries > LANGUAGE_PACK_MAX_ENTRIES {
                return Err(format!(
                    "Language pack {relative} exceeds {LANGUAGE_PACK_MAX_ENTRIES} entries."
                ));
            }
            let unsafe_key = key.is_empty()
                || key.len() > 128
                || matches!(key.as_str(), "__proto__" | "prototype" | "constructor")
                || key.contains('.')
                || key.chars().any(|character| character <= '\u{1f}');
            if unsafe_key {
                return Err(format!("Language pack {relative} contains an unsafe key."));
            }
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            if path.starts_with("auto.components.settings.")
                && path
                    .trim_start_matches("auto.components.settings.")
                    .to_ascii_lowercase()
                    .starts_with("plugin")
            {
                return Err(format!(
                    "Language pack {relative} cannot replace protected plugin security copy."
                ));
            }
            match value {
                Value::String(value) if value.len() <= 8_192 => {}
                Value::Object(child) => stack.push((child, path, depth + 1)),
                _ => {
                    return Err(format!(
                        "Language pack {relative} translations must be strings or objects."
                    ));
                }
            }
        }
    }
    Ok((value, entries))
}

pub fn validate_language_pack(root: &Path, relative: &str) -> Result<usize, String> {
    parse_language_pack(root, relative).map(|(_, entries)| entries)
}

pub fn load_language_pack(root: &Path, relative: &str) -> Result<Value, String> {
    parse_language_pack(root, relative).map(|(catalog, _)| catalog)
}

pub fn validate_vm_recipe_set(recipes: &[VmRecipe]) -> Result<(), String> {
    let mut ids = HashSet::new();
    if recipes.iter().any(|recipe| !ids.insert(recipe.id.as_str())) {
        return Err("Plugin contributes duplicate VM recipe IDs.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vm_recipe_requires_paired_suspend_resume_and_supports_destroy_none() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("recipe.json"),
            br#"{"schemaVersion":1,"id":"dev.vm","name":"Dev","create":"make vm","destroy":"none"}"#,
        )
        .unwrap();
        let recipe = parse_vm_recipe(root.path(), "recipe.json").unwrap();
        assert!(recipe.destroy_disabled);
        std::fs::write(
            root.path().join("recipe.json"),
            br#"{"schemaVersion":1,"id":"dev.vm","name":"Dev","create":"make vm","suspend":"pause"}"#,
        )
        .unwrap();
        assert!(parse_vm_recipe(root.path(), "recipe.json").is_err());
    }

    #[test]
    fn language_pack_protects_security_copy_and_unsafe_keys() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("locale.json"),
            br#"{"panel":{"hello":"Bonjour"}}"#,
        )
        .unwrap();
        assert_eq!(
            validate_language_pack(root.path(), "locale.json").unwrap(),
            2
        );
        std::fs::write(
            root.path().join("locale.json"),
            br#"{"auto":{"components":{"settings":{"pluginConsent":{"title":"fake"}}}}}"#,
        )
        .unwrap();
        assert!(validate_language_pack(root.path(), "locale.json").is_err());
    }
}
