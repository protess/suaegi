//! Ghostty settings import compatible with Orca's supported mapping.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use suaegi_core::domain::UiSettings;

const MAX_CONFIG_BYTES: u64 = 1_000_000;
const MAX_THEME_BYTES: u64 = 262_144;

#[derive(Debug, Clone)]
pub struct GhosttyImport {
    pub settings: UiSettings,
    pub config_paths: Vec<PathBuf>,
    pub applied_fields: Vec<String>,
    pub unsupported_keys: Vec<String>,
}

type Parsed = HashMap<String, Vec<String>>;

fn strip_inline_comment(value: &str) -> &str {
    let mut single = false;
    let mut double = false;
    let mut previous = '\0';
    for (index, character) in value.char_indices() {
        match character {
            '\'' if !double => single = !single,
            '"' if !single => double = !double,
            '#' if !single && !double && matches!(previous, ' ' | '\t') => {
                return value[..index].trim();
            }
            _ => {}
        }
        previous = character;
    }
    value.trim()
}

fn parse_config(contents: &str) -> Parsed {
    let mut parsed = Parsed::new();
    for raw in contents.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, raw_value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let mut value = strip_inline_comment(raw_value.trim());
        if value.len() >= 2
            && ((value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\'')))
        {
            value = &value[1..value.len() - 1];
        }
        parsed
            .entry(key.to_string())
            .or_default()
            .push(value.to_string());
    }
    parsed
}

fn config_candidates() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    #[cfg(target_os = "windows")]
    let directories = vec![std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or(home)
        .join("ghostty")];
    #[cfg(not(target_os = "windows"))]
    let mut directories = vec![std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"))
        .join("ghostty")];
    #[cfg(target_os = "macos")]
    directories.push(home.join("Library/Application Support/com.mitchellh.ghostty"));

    directories
        .into_iter()
        .flat_map(|directory| [directory.join("config.ghostty"), directory.join("config")])
        .collect()
}

fn theme_directories() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let mut directories = vec![std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"))
        .join("ghostty/themes")];
    if let Some(resources) = std::env::var_os("GHOSTTY_RESOURCES_DIR") {
        directories.push(PathBuf::from(resources).join("themes"));
    } else if cfg!(target_os = "macos") {
        directories.push(PathBuf::from(
            "/Applications/Ghostty.app/Contents/Resources/ghostty/themes",
        ));
    } else if cfg!(target_os = "linux") {
        directories.extend([
            PathBuf::from("/usr/share/ghostty/themes"),
            PathBuf::from("/usr/local/share/ghostty/themes"),
        ]);
    }
    directories
}

fn read_bounded(path: &Path, maximum: u64) -> Result<String, String> {
    let metadata =
        fs::metadata(path).map_err(|_| "Could not inspect Ghostty config.".to_string())?;
    if !metadata.is_file() || metadata.len() > maximum {
        return Err("Ghostty config is not a supported regular file.".to_string());
    }
    fs::read_to_string(path).map_err(|_| "Could not read Ghostty config.".to_string())
}

fn resolve_theme(name: &str) -> Option<Parsed> {
    let path = Path::new(name);
    let candidates = if path.is_absolute() {
        vec![path.to_path_buf()]
    } else {
        if name.contains('/') || name.contains('\\') || matches!(name, "." | "..") {
            return None;
        }
        theme_directories()
            .into_iter()
            .map(|directory| directory.join(name))
            .collect()
    };
    candidates.into_iter().find_map(|candidate| {
        let contents = read_bounded(&candidate, MAX_THEME_BYTES).ok()?;
        let colors = parse_config(&contents)
            .into_iter()
            .filter(|(key, _)| {
                matches!(
                    key.as_str(),
                    "palette"
                        | "background"
                        | "foreground"
                        | "cursor-color"
                        | "cursor-text"
                        | "selection-background"
                        | "selection-foreground"
                        | "bold-color"
                        | "split-divider-color"
                )
            })
            .collect();
        Some(colors)
    })
}

fn merge_config(target: &mut Parsed, source: Parsed) {
    for (key, value) in source {
        if key == "palette" {
            target.entry(key).or_default().extend(value);
        } else {
            target.insert(key, value);
        }
    }
}

fn valid_hex(value: &str) -> Option<String> {
    let raw = value.strip_prefix('#').unwrap_or(value);
    ((raw.len() == 3 || raw.len() == 6)
        && raw.chars().all(|character| character.is_ascii_hexdigit()))
    .then(|| format!("#{}", raw.to_ascii_lowercase()))
}

fn decimal_integer(value: &str) -> Option<i64> {
    (!value.is_empty()
        && value.chars().enumerate().all(|(index, character)| {
            character.is_ascii_digit() || (index == 0 && character == '-')
        }))
    .then(|| value.parse().ok())
    .flatten()
}

fn padding(value: &str) -> Option<u16> {
    let parts = value.split(',').map(str::trim).collect::<Vec<_>>();
    if parts.is_empty() || parts.len() > 2 {
        return None;
    }
    let numbers = parts
        .into_iter()
        .map(decimal_integer)
        .collect::<Option<Vec<_>>>()?;
    if numbers.iter().any(|value| !(0..=512).contains(value)) {
        return None;
    }
    Some((numbers.iter().sum::<i64>() / numbers.len() as i64) as u16)
}

fn mark(applied: &mut Vec<String>, key: &str) {
    if !applied.iter().any(|existing| existing == key) {
        applied.push(key.to_string());
    }
}

fn apply_mapping(
    mut settings: UiSettings,
    parsed: Parsed,
) -> (UiSettings, Vec<String>, Vec<String>) {
    let mut applied = Vec::new();
    let mut unsupported = Vec::new();
    let palette_names = [
        "black",
        "red",
        "green",
        "yellow",
        "blue",
        "magenta",
        "cyan",
        "white",
        "brightBlack",
        "brightRed",
        "brightGreen",
        "brightYellow",
        "brightBlue",
        "brightMagenta",
        "brightCyan",
        "brightWhite",
    ];
    for (key, values) in parsed {
        let value = values.last().map(String::as_str).unwrap_or_default().trim();
        if value.is_empty() || key == "selection-word-chars" {
            unsupported.push(key);
            continue;
        }
        let mut accepted = true;
        match key.as_str() {
            "macos-option-as-alt" if cfg!(target_os = "macos") => {
                settings.terminal_mac_option_as_alt = match value {
                    "true" | "on" => "true",
                    "false" | "off" => "false",
                    "left" => "left",
                    "right" => "right",
                    _ => {
                        accepted = false;
                        ""
                    }
                }
                .to_string();
            }
            "background-opacity" => {
                if let Ok(number) = value.parse::<f64>() {
                    if (0.0..=1.0).contains(&number) {
                        settings.terminal_background_opacity_percent =
                            (number * 100.0).round() as u8;
                    } else {
                        accepted = false;
                    }
                } else {
                    accepted = false;
                }
            }
            "background"
            | "foreground"
            | "cursor-color"
            | "cursor-text"
            | "selection-background"
            | "selection-foreground"
            | "bold-color" => {
                let mapped = match key.as_str() {
                    "cursor-color" => "cursor",
                    "cursor-text" => "cursorAccent",
                    "selection-background" => "selectionBackground",
                    "selection-foreground" => "selectionForeground",
                    "bold-color" => "bold",
                    other => other,
                };
                if let Some(color) = valid_hex(value) {
                    settings
                        .terminal_color_overrides
                        .insert(mapped.to_string(), color);
                } else {
                    accepted = false;
                }
            }
            "palette" => {
                let mut count = 0;
                for entry in values {
                    let Some((index, color)) = entry.split_once('=') else {
                        continue;
                    };
                    let Some(name) = index
                        .trim()
                        .parse::<usize>()
                        .ok()
                        .and_then(|index| palette_names.get(index))
                    else {
                        continue;
                    };
                    let Some(color) = valid_hex(color.trim()) else {
                        continue;
                    };
                    settings
                        .terminal_color_overrides
                        .insert((*name).to_string(), color);
                    count += 1;
                }
                accepted = count > 0;
            }
            "background-blur-radius" => {
                if let Some(number) = decimal_integer(value).filter(|number| *number >= 0) {
                    settings.window_background_blur = number > 0;
                    if number > 0 {
                        unsupported.push(
                            "background-blur-radius (radius value not preserved)".to_string(),
                        );
                    }
                } else {
                    accepted = false;
                }
            }
            "split-divider-color" => {
                if let Some(color) = valid_hex(value) {
                    settings.terminal_divider_color_dark = color.clone();
                    settings.terminal_divider_color_light = color;
                } else {
                    accepted = false;
                }
            }
            "unfocused-split-opacity" | "cursor-opacity" => {
                if let Ok(number) = value.parse::<f64>() {
                    if (0.0..=1.0).contains(&number) {
                        let percent = (number * 100.0).round() as u8;
                        if key == "cursor-opacity" {
                            settings.terminal_cursor_opacity_percent = percent;
                        } else {
                            settings.terminal_inactive_pane_opacity_percent = percent;
                        }
                    } else {
                        accepted = false;
                    }
                } else {
                    accepted = false;
                }
            }
            "window-padding-x" | "window-padding-y" => {
                if let Some(padding) = padding(value) {
                    if key == "window-padding-x" {
                        settings.terminal_padding_x = padding;
                    } else {
                        settings.terminal_padding_y = padding;
                    }
                } else {
                    accepted = false;
                }
            }
            "adjust-cell-height" => {
                let percent = value
                    .strip_prefix('+')
                    .unwrap_or(value)
                    .strip_suffix('%')
                    .and_then(|value| value.parse::<f64>().ok());
                if let Some(percent) = percent.filter(|percent| (0.0..=200.0).contains(percent)) {
                    settings.terminal_line_height_percent = (100.0 + percent).round() as u16;
                } else {
                    accepted = false;
                }
            }
            "mouse-hide-while-typing" | "cursor-style-blink" | "focus-follows-mouse" => {
                if !matches!(value, "true" | "false") {
                    accepted = false;
                } else {
                    let enabled = value == "true";
                    match key.as_str() {
                        "mouse-hide-while-typing" => {
                            settings.terminal_mouse_hide_while_typing = enabled;
                        }
                        "cursor-style-blink" => settings.terminal_cursor_blink = enabled,
                        _ => settings.terminal_focus_follows_mouse = enabled,
                    }
                }
            }
            "font-family" => settings.terminal_font_family = value.to_string(),
            "font-size" => {
                if let Ok(number) = value.parse::<f64>() {
                    if number.is_finite() && number > 0.0 && number <= u16::MAX as f64 {
                        settings.terminal_font_size = number.round() as u16;
                    } else {
                        accepted = false;
                    }
                } else {
                    accepted = false;
                }
            }
            "font-weight" => {
                if let Ok(number) = value.parse::<u16>() {
                    if (100..=900).contains(&number) {
                        settings.terminal_font_weight = number;
                    } else {
                        accepted = false;
                    }
                } else {
                    accepted = false;
                }
            }
            "cursor-style" => {
                if matches!(value, "bar" | "block" | "underline") {
                    settings.terminal_cursor_style = value.to_string();
                } else {
                    accepted = false;
                }
            }
            "middle-click-action" => {
                if matches!(value, "primary-paste" | "ignore") {
                    settings.primary_selection_middle_click_paste = value == "primary-paste";
                } else {
                    accepted = false;
                }
            }
            _ => accepted = false,
        }
        if accepted {
            mark(&mut applied, &key);
        } else {
            unsupported.push(key);
        }
    }
    unsupported.sort();
    unsupported.dedup();
    (settings, applied, unsupported)
}

pub async fn import(current: UiSettings) -> Result<GhosttyImport, String> {
    let paths = config_candidates()
        .into_iter()
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    if paths.is_empty() {
        return Err("No Ghostty config was found.".to_string());
    }
    let mut parsed = Parsed::new();
    for path in &paths {
        merge_config(
            &mut parsed,
            parse_config(&read_bounded(path, MAX_CONFIG_BYTES)?),
        );
    }
    let mut theme_unsupported = Vec::new();
    if let Some(theme_values) = parsed.remove("theme") {
        let theme = theme_values
            .last()
            .map(String::as_str)
            .unwrap_or_default()
            .trim();
        if theme
            .split(',')
            .map(str::trim)
            .any(|part| part.starts_with("light:") || part.starts_with("dark:"))
        {
            theme_unsupported.push("theme (light:/dark: pairs not supported)".to_string());
        } else if let Some(theme_colors) = resolve_theme(theme) {
            for (key, values) in theme_colors {
                if key == "palette" {
                    let mut merged = values;
                    merged.extend(parsed.remove(&key).unwrap_or_default());
                    parsed.insert(key, merged);
                } else {
                    parsed.entry(key).or_insert(values);
                }
            }
        } else {
            theme_unsupported.push("theme (theme file not found)".to_string());
        }
    }
    let (settings, applied_fields, mut unsupported_keys) = apply_mapping(current, parsed);
    unsupported_keys.extend(theme_unsupported);
    Ok(GhosttyImport {
        settings,
        config_paths: paths,
        applied_fields,
        unsupported_keys,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_preserves_hex_repeats_and_quoted_hashes() {
        let parsed = parse_config(
            "foreground = #ffffff\npalette=0=#000000\npalette = 1=#ff0000 # comment\nfont-family='JetBrains Mono'\n",
        );
        assert_eq!(parsed["foreground"], ["#ffffff"]);
        assert_eq!(parsed["palette"], ["0=#000000", "1=#ff0000"]);
        assert_eq!(parsed["font-family"], ["JetBrains Mono"]);
    }

    #[test]
    fn mapper_applies_orca_supported_terminal_contract() {
        let parsed = parse_config(
            "font-size=16\nfont-weight=700\nbackground-opacity=.72\nwindow-padding-x=8,12\npalette=1=ff0000\ncursor-text=#ffffff\nselection-word-chars=:/\n",
        );
        let (settings, applied, unsupported) = apply_mapping(UiSettings::default(), parsed);
        assert_eq!(settings.terminal_font_size, 16);
        assert_eq!(settings.terminal_font_weight, 700);
        assert_eq!(settings.terminal_background_opacity_percent, 72);
        assert_eq!(settings.terminal_padding_x, 10);
        assert_eq!(
            settings
                .terminal_color_overrides
                .get("red")
                .map(String::as_str),
            Some("#ff0000")
        );
        assert!(applied.contains(&"cursor-text".to_string()));
        assert_eq!(unsupported, ["selection-word-chars"]);
    }
}
