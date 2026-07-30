//! Safe, bounded import of Warp-format terminal theme YAML files.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_yaml::{Mapping, Value};
use suaegi_core::domain::TerminalCustomThemeSetting;

const MAX_THEME_BYTES: u64 = 1_048_576;
const MAX_DISCOVERED_FILES: usize = 200;
const MAX_SCAN_DEPTH: usize = 12;
const ANSI_NAMES: [&str; 8] = [
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
];

#[derive(Debug, Clone, Default)]
pub struct ThemeImportResult {
    pub themes: Vec<TerminalCustomThemeSetting>,
    pub skipped: Vec<String>,
    pub source_label: String,
}

pub fn custom_selection(id: &str) -> String {
    format!("custom:{id}")
}

pub fn normalize_themes(
    themes: impl IntoIterator<Item = TerminalCustomThemeSetting>,
) -> Vec<TerminalCustomThemeSetting> {
    let mut by_id = HashMap::new();
    for mut theme in themes {
        theme.id = normalize_id(&theme.id, "theme");
        theme.name = normalize_name(&theme.name, "Imported Theme");
        theme.source = match theme.source.as_str() {
            "warp" | "ghostty" | "manual" => theme.source,
            _ => "manual".to_string(),
        };
        theme.mode = match theme.mode.as_str() {
            "dark" | "light" | "unknown" => theme.mode,
            _ => "unknown".to_string(),
        };
        theme.terminal = theme
            .terminal
            .into_iter()
            .filter_map(|(key, value)| {
                supported_color_key(&key)
                    .then(|| normalize_hex(&value).map(|value| (key, value)))
                    .flatten()
            })
            .collect();
        if has_usable_colors(&theme.terminal) {
            by_id.insert(theme.id.clone(), theme);
        }
    }
    let mut themes = by_id.into_values().collect::<Vec<_>>();
    themes.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));
    themes.truncate(MAX_DISCOVERED_FILES);
    themes
}

pub fn discover_and_import() -> Result<ThemeImportResult, String> {
    let roots = warp_theme_directories();
    let mut files = Vec::new();
    let mut seen = HashSet::new();
    for root in &roots {
        scan_theme_directory(root, 0, &mut files, &mut seen);
        if files.len() >= MAX_DISCOVERED_FILES {
            break;
        }
    }
    let mut result = import_files(&files)?;
    result.source_label = "Warp themes".to_string();
    Ok(result)
}

pub fn import_files(paths: &[PathBuf]) -> Result<ThemeImportResult, String> {
    let mut result = ThemeImportResult {
        source_label: paths.first().and_then(|path| path.parent()).map_or_else(
            || "Theme YAML".to_string(),
            |path| path.display().to_string(),
        ),
        ..Default::default()
    };
    let mut seen = HashSet::new();
    for path in paths.iter().take(MAX_DISCOVERED_FILES) {
        let canonical = match path.canonicalize() {
            Ok(path) if path.is_file() => path,
            _ => {
                result
                    .skipped
                    .push(format!("{}: file is unavailable", path.display()));
                continue;
            }
        };
        if !seen.insert(canonical.clone()) || !is_yaml_path(&canonical) {
            continue;
        }
        let metadata = std::fs::metadata(&canonical)
            .map_err(|error| format!("Could not inspect {}: {error}", canonical.display()))?;
        if metadata.len() > MAX_THEME_BYTES {
            result.skipped.push(format!(
                "{}: file is larger than 1 MiB",
                canonical.display()
            ));
            continue;
        }
        let content = std::fs::read_to_string(&canonical)
            .map_err(|error| format!("Could not read {}: {error}", canonical.display()))?;
        match parse_warp_theme_yaml(&content, &canonical) {
            Ok(theme) => result.themes.push(theme),
            Err(error) => result
                .skipped
                .push(format!("{}: {error}", canonical.display())),
        }
    }
    result.themes = normalize_themes(result.themes);
    Ok(result)
}

pub fn parse_warp_theme_yaml(
    content: &str,
    path: &Path,
) -> Result<TerminalCustomThemeSetting, String> {
    if content.len() as u64 > MAX_THEME_BYTES {
        return Err("Theme file is larger than 1 MiB.".to_string());
    }
    let value: Value =
        serde_yaml::from_str(content).map_err(|error| format!("Invalid YAML: {error}"))?;
    let root = value
        .as_mapping()
        .ok_or_else(|| "Theme file must contain a YAML object.".to_string())?;
    let fallback = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("Imported Theme");
    let name = normalize_name(string_at(root, "name").unwrap_or(fallback), fallback);
    let mut terminal = HashMap::new();
    copy_color(root, "background", "background", &mut terminal);
    copy_color(root, "foreground", "foreground", &mut terminal);
    if !copy_color(root, "cursor", "cursor", &mut terminal) {
        copy_color(root, "accent", "cursor", &mut terminal);
    }
    if let Some(terminal_colors) = mapping_at(root, "terminal_colors") {
        if let Some(normal) = mapping_at(terminal_colors, "normal") {
            copy_ansi(normal, false, &mut terminal);
        }
        if let Some(bright) = mapping_at(terminal_colors, "bright") {
            copy_ansi(bright, true, &mut terminal);
        }
    }
    if !has_usable_colors(&terminal) {
        return Err(
            "Theme must include background, foreground, and at least one ANSI color.".to_string(),
        );
    }
    let mut unsupported = Vec::new();
    if root.contains_key(Value::String("background_image".to_string())) {
        unsupported.push("background image not supported".to_string());
    }
    if root
        .get(Value::String("background".to_string()))
        .is_some_and(Value::is_mapping)
    {
        unsupported.push("background gradient not supported".to_string());
    }
    let id = normalize_id(&format!("warp:{name}:{fallback}"), "warp:theme");
    let background = terminal.get("background").cloned();
    Ok(TerminalCustomThemeSetting {
        id,
        name,
        source: "warp".to_string(),
        mode: infer_mode(background.as_deref(), string_at(root, "details")),
        terminal,
        imported_at: chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now())
            .to_rfc3339(),
        source_label: Some(path.display().to_string()),
        unsupported_features: unsupported,
    })
}

fn copy_ansi(source: &Mapping, bright: bool, terminal: &mut HashMap<String, String>) {
    for name in ANSI_NAMES {
        let target = if bright {
            format!("bright{}", uppercase_first(name))
        } else {
            name.to_string()
        };
        copy_color(source, name, &target, terminal);
    }
}

fn copy_color(
    source: &Mapping,
    source_key: &str,
    target_key: &str,
    target: &mut HashMap<String, String>,
) -> bool {
    let Some(value) = source.get(Value::String(source_key.to_string())) else {
        return false;
    };
    let scalar = value.as_str().and_then(normalize_hex).or_else(|| {
        value.as_mapping().and_then(|mapping| {
            ["top", "bottom", "left", "right"]
                .into_iter()
                .find_map(|key| string_at(mapping, key).and_then(normalize_hex))
        })
    });
    if let Some(color) = scalar {
        target.insert(target_key.to_string(), color);
        true
    } else {
        false
    }
}

fn mapping_at<'a>(mapping: &'a Mapping, key: &str) -> Option<&'a Mapping> {
    mapping
        .get(Value::String(key.to_string()))
        .and_then(Value::as_mapping)
}

fn string_at<'a>(mapping: &'a Mapping, key: &str) -> Option<&'a str> {
    mapping
        .get(Value::String(key.to_string()))
        .and_then(Value::as_str)
}

fn normalize_hex(value: &str) -> Option<String> {
    crate::terminal::palette::normalize_hex_color(value)
}

fn has_usable_colors(colors: &HashMap<String, String>) -> bool {
    colors.contains_key("background")
        && colors.contains_key("foreground")
        && ANSI_NAMES.iter().any(|name| colors.contains_key(*name))
}

fn supported_color_key(key: &str) -> bool {
    matches!(
        key,
        "foreground"
            | "background"
            | "cursor"
            | "cursorAccent"
            | "selectionBackground"
            | "selectionForeground"
            | "black"
            | "red"
            | "green"
            | "yellow"
            | "blue"
            | "magenta"
            | "cyan"
            | "white"
            | "brightBlack"
            | "brightRed"
            | "brightGreen"
            | "brightYellow"
            | "brightBlue"
            | "brightMagenta"
            | "brightCyan"
            | "brightWhite"
            | "bold"
    )
}

fn normalize_id(value: &str, fallback: &str) -> String {
    let normalized = value
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>()
        .trim()
        .to_lowercase()
        .replace(['\'', '"'], "")
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, ':' | '_' | '-') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    let normalized = normalized
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let normalized = normalized.trim_matches('-');
    if normalized.is_empty() {
        fallback.to_string()
    } else {
        normalized.to_string()
    }
}

fn normalize_name(value: &str, fallback: &str) -> String {
    let normalized = value
        .chars()
        .filter(|character| !character.is_control())
        .map(|character| {
            if matches!(character, '/' | '\\') {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if normalized.is_empty() {
        fallback.to_string()
    } else {
        normalized
    }
}

fn uppercase_first(value: &str) -> String {
    let mut characters = value.chars();
    characters.next().map_or_else(String::new, |first| {
        first.to_uppercase().collect::<String>() + characters.as_str()
    })
}

fn infer_mode(background: Option<&str>, details: Option<&str>) -> String {
    if let Some(value) = background.and_then(|value| value.strip_prefix('#')) {
        if value.len() == 6 {
            let channel = |range| u8::from_str_radix(&value[range], 16).unwrap_or(0) as f32 / 255.0;
            let luminance =
                0.2126 * channel(0..2) + 0.7152 * channel(2..4) + 0.0722 * channel(4..6);
            return if luminance >= 0.55 { "light" } else { "dark" }.to_string();
        }
    }
    match details {
        Some("lighter") => "light",
        Some("darker") => "dark",
        _ => "unknown",
    }
    .to_string()
}

fn is_yaml_path(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| matches!(extension.to_ascii_lowercase().as_str(), "yaml" | "yml"))
}

fn scan_theme_directory(
    directory: &Path,
    depth: usize,
    files: &mut Vec<PathBuf>,
    seen: &mut HashSet<PathBuf>,
) {
    if depth > MAX_SCAN_DEPTH || files.len() >= MAX_DISCOVERED_FILES {
        return;
    }
    let canonical = match directory.canonicalize() {
        Ok(path) if path.is_dir() && seen.insert(path.clone()) => path,
        _ => return,
    };
    let Ok(entries) = std::fs::read_dir(canonical) else {
        return;
    };
    let mut entries = entries.flatten().collect::<Vec<_>>();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        if files.len() >= MAX_DISCOVERED_FILES {
            break;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            scan_theme_directory(&path, depth + 1, files, seen);
        } else if file_type.is_file() && is_yaml_path(&path) {
            files.push(path);
        }
    }
}

fn warp_theme_directories() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    #[cfg(target_os = "macos")]
    {
        return [
            ".warp",
            ".warp-preview",
            ".warp-oss",
            ".warp-dev",
            ".warp-local",
            ".warp-integration",
        ]
        .into_iter()
        .map(|channel| home.join(channel).join("themes"))
        .collect();
    }
    #[cfg(target_os = "windows")]
    {
        let data = std::env::var_os("APPDATA").map_or_else(|| home.clone(), PathBuf::from);
        return ["Warp", "WarpPreview", "WarpOss", "WarpDev", "WarpLocal"]
            .into_iter()
            .map(|channel| data.join("warp").join(channel).join("data/themes"))
            .collect();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| home.join(".local/share"));
        return [
            "warp-terminal",
            "warp-terminal-preview",
            "warp-oss",
            "warp-dev",
        ]
        .into_iter()
        .map(|channel| data.join(channel).join("themes"))
        .collect();
    }
    #[allow(unreachable_code)]
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r##"
name: Tokyo Night
background: "#1a1b26"
foreground: "#c0caf5"
cursor: "#7aa2f7"
details: darker
terminal_colors:
  normal:
    black: "#15161e"
    red: "#f7768e"
    green: "#9ece6a"
    blue: "#7aa2f7"
  bright:
    black: "#414868"
    red: "#f7768e"
"##;

    #[test]
    fn parses_warp_palette_and_mode() {
        let theme = parse_warp_theme_yaml(VALID, Path::new("tokyo_night.yaml")).unwrap();
        assert_eq!(theme.name, "Tokyo Night");
        assert_eq!(theme.mode, "dark");
        assert_eq!(theme.terminal["background"], "#1a1b26");
        assert_eq!(theme.terminal["brightBlack"], "#414868");
        assert_eq!(custom_selection(&theme.id), format!("custom:{}", theme.id));
    }

    #[test]
    fn rejects_incomplete_and_non_mapping_yaml() {
        assert!(parse_warp_theme_yaml("- one\n- two", Path::new("list.yaml")).is_err());
        assert!(parse_warp_theme_yaml(
            "name: Broken\nbackground: '#000'\nforeground: '#fff'",
            Path::new("broken.yaml")
        )
        .is_err());
    }

    #[test]
    fn gradient_colors_use_a_supported_edge_and_report_the_limitation() {
        let yaml = VALID.replace(
            "background: \"#1a1b26\"",
            "background:\n  top: \"#101010\"\n  bottom: \"#202020\"",
        );
        let theme = parse_warp_theme_yaml(&yaml, Path::new("gradient.yaml")).unwrap();
        assert_eq!(theme.terminal["background"], "#101010");
        assert_eq!(
            theme.unsupported_features,
            vec!["background gradient not supported"]
        );
    }
}
