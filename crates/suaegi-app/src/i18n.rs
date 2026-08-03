//! Runtime UI localization backed by Orca's shipped locale catalogs.
//!
//! Orca's generated translation keys are an implementation detail.  Native
//! Rust views mostly own the same English copy, so this module builds a reverse
//! index from the English catalog and resolves that copy in the selected
//! locale. Dynamic strings safely fall back to their original text.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::process::Command;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{OnceLock, RwLock};

use iced::advanced::text::IntoFragment;
use iced::widget::Text;

const EN: &str = include_str!("../assets/orca/locales/en.json");
const ZH: &str = include_str!("../assets/orca/locales/zh.json");
const KO: &str = include_str!("../assets/orca/locales/ko.json");
const JA: &str = include_str!("../assets/orca/locales/ja.json");
const ES: &str = include_str!("../assets/orca/locales/es.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Locale {
    En = 0,
    Zh = 1,
    Ko = 2,
    Ja = 3,
    Es = 4,
}

static ACTIVE_LOCALE: AtomicU8 = AtomicU8::new(Locale::En as u8);
static TRANSLATIONS: OnceLock<HashMap<Locale, HashMap<String, String>>> = OnceLock::new();
static ACTIVE_PLUGIN_LANGUAGE: OnceLock<RwLock<Option<String>>> = OnceLock::new();
static PLUGIN_LANGUAGES: OnceLock<RwLock<BTreeMap<String, PluginLanguage>>> = OnceLock::new();

#[derive(Debug, Clone)]
struct PluginLanguage {
    label: String,
    reverse: HashMap<String, String>,
}

fn active_plugin_language() -> &'static RwLock<Option<String>> {
    ACTIVE_PLUGIN_LANGUAGE.get_or_init(|| RwLock::new(None))
}

fn plugin_languages() -> &'static RwLock<BTreeMap<String, PluginLanguage>> {
    PLUGIN_LANGUAGES.get_or_init(|| RwLock::new(BTreeMap::new()))
}

fn flatten(value: &serde_json::Value, path: &mut Vec<String>, out: &mut HashMap<String, String>) {
    match value {
        serde_json::Value::Object(entries) => {
            for (key, value) in entries {
                path.push(key.clone());
                flatten(value, path, out);
                path.pop();
            }
        }
        serde_json::Value::String(value) => {
            out.insert(path.join("."), value.clone());
        }
        _ => {}
    }
}

fn catalog(raw: &str) -> HashMap<String, String> {
    let value = serde_json::from_str(raw).unwrap_or(serde_json::Value::Null);
    let mut result = HashMap::new();
    flatten(&value, &mut Vec::new(), &mut result);
    result
}

fn translations() -> &'static HashMap<Locale, HashMap<String, String>> {
    TRANSLATIONS.get_or_init(|| {
        let english = catalog(EN);
        let mut result = HashMap::new();
        for (locale, raw) in [
            (Locale::Zh, ZH),
            (Locale::Ko, KO),
            (Locale::Ja, JA),
            (Locale::Es, ES),
        ] {
            let localized = catalog(raw);
            let mut reverse = HashMap::new();
            for (key, english_value) in &english {
                if let Some(localized_value) = localized.get(key) {
                    let entry = reverse
                        .entry(english_value.clone())
                        .or_insert_with(|| localized_value.clone());
                    if entry == english_value && localized_value != english_value {
                        *entry = localized_value.clone();
                    }
                }
            }
            result.insert(locale, reverse);
        }
        result
    })
}

fn system_locale() -> Locale {
    let locale = std::env::var("LC_ALL")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var("LC_MESSAGES").ok())
        .or_else(|| std::env::var("LANG").ok())
        .or_else(|| {
            #[cfg(target_os = "macos")]
            {
                Command::new("/usr/bin/defaults")
                    .args(["read", "-g", "AppleLocale"])
                    .output()
                    .ok()
                    .filter(|output| output.status.success())
                    .and_then(|output| String::from_utf8(output.stdout).ok())
            }
            #[cfg(not(target_os = "macos"))]
            {
                None
            }
        })
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if locale.starts_with("zh") {
        Locale::Zh
    } else if locale.starts_with("ko") {
        Locale::Ko
    } else if locale.starts_with("ja") {
        Locale::Ja
    } else if locale.starts_with("es") {
        Locale::Es
    } else {
        Locale::En
    }
}

pub fn set_language(setting: &str) {
    *active_plugin_language()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
        setting.starts_with("plugin:").then(|| setting.to_string());
    let locale = match setting {
        "en" => Locale::En,
        "zh" => Locale::Zh,
        "ko" => Locale::Ko,
        "ja" => Locale::Ja,
        "es" => Locale::Es,
        _ => system_locale(),
    };
    ACTIVE_LOCALE.store(locale as u8, Ordering::Relaxed);
}

fn active_locale() -> Locale {
    match ACTIVE_LOCALE.load(Ordering::Relaxed) {
        1 => Locale::Zh,
        2 => Locale::Ko,
        3 => Locale::Ja,
        4 => Locale::Es,
        _ => Locale::En,
    }
}

fn translate_for(locale: Locale, value: &str) -> Cow<'_, str> {
    if locale == Locale::En {
        return Cow::Borrowed(value);
    }
    let canonical = value.replace("Suaegi", "Orca");
    let Some(translated) = translations()
        .get(&locale)
        .and_then(|catalog| catalog.get(&canonical))
    else {
        return Cow::Borrowed(value);
    };
    Cow::Owned(translated.replace("Orca", "Suaegi"))
}

pub fn translate(value: &str) -> Cow<'_, str> {
    let plugin_id = active_plugin_language()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    if let Some(plugin_id) = plugin_id {
        let canonical = value.replace("Suaegi", "Orca");
        if let Some(translated) = plugin_languages()
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&plugin_id)
            .and_then(|language| language.reverse.get(&canonical))
        {
            return Cow::Owned(translated.replace("Orca", "Suaegi"));
        }
        return Cow::Borrowed(value);
    }
    translate_for(active_locale(), value)
}

pub fn text<'a, Theme, Renderer>(value: impl IntoFragment<'a>) -> Text<'a, Theme, Renderer>
where
    Theme: iced::widget::text::Catalog + 'a,
    Renderer: iced::advanced::text::Renderer,
{
    let fragment = value.into_fragment();
    let translated = translate(&fragment);
    iced::widget::text(match translated {
        Cow::Borrowed(_) => fragment,
        Cow::Owned(value) => Cow::Owned(value),
    })
}

pub fn language_label(setting: &str) -> &'static str {
    match setting {
        "en" => "English",
        "zh" => "中文（简体）",
        "ko" => "한국어",
        "ja" => "日本語",
        "es" => "Español",
        _ => "System",
    }
}

pub fn language_label_owned(setting: &str) -> String {
    plugin_languages()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(setting)
        .map(|language| language.label.clone())
        .unwrap_or_else(|| language_label(setting).to_string())
}

pub fn language_options() -> Vec<String> {
    let mut options = vec![
        "System".to_string(),
        "English".to_string(),
        "中文（简体）".to_string(),
        "한국어".to_string(),
        "日本語".to_string(),
        "Español".to_string(),
    ];
    options.extend(
        plugin_languages()
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .values()
            .map(|language| language.label.clone()),
    );
    options
}

pub fn language_id_for_label(label: &str) -> String {
    let builtin = match label {
        "English" => Some("en"),
        "中文（简体）" => Some("zh"),
        "한국어" => Some("ko"),
        "日本語" => Some("ja"),
        "Español" => Some("es"),
        "System" => Some("system"),
        _ => None,
    };
    if let Some(builtin) = builtin {
        return builtin.to_string();
    }
    plugin_languages()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .find_map(|(id, language)| (language.label == label).then(|| id.clone()))
        .unwrap_or_else(|| "system".to_string())
}

fn plugin_reverse_catalog(
    english: &HashMap<String, String>,
    catalog_value: &serde_json::Value,
) -> HashMap<String, String> {
    let mut localized = HashMap::new();
    flatten(catalog_value, &mut Vec::new(), &mut localized);
    let mut reverse = HashMap::new();
    for (key, english_value) in english {
        if let Some(localized_value) = localized.get(key) {
            reverse.insert(english_value.clone(), localized_value.clone());
        }
    }
    reverse
}

pub fn set_plugin_language_packs(plugins: &[crate::plugins::PluginEntry]) {
    let english = catalog(EN);
    let mut languages = BTreeMap::new();
    for plugin in plugins.iter().filter(|plugin| {
        plugin.status == crate::plugins::PluginStatus::Idle && plugin.blocked_by_kill_list.is_none()
    }) {
        for (locale, catalog_value) in &plugin.language_pack_catalogs {
            let reverse = plugin_reverse_catalog(&english, catalog_value);
            let id = format!("plugin:{}/{}", plugin.plugin_key, locale);
            languages.insert(
                id,
                PluginLanguage {
                    label: format!("{locale} — {}", plugin.plugin_key),
                    reverse,
                },
            );
        }
    }
    *plugin_languages()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = languages;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_orca_copy_and_rebrands_it() {
        assert_eq!(translate_for(Locale::Ko, "Settings"), "설정");
        assert_eq!(translate_for(Locale::Ko, "Open Suaegi"), "Suaegi 열기");
    }

    #[test]
    fn unknown_and_dynamic_copy_falls_back_verbatim() {
        assert_eq!(
            translate_for(Locale::Ja, "not in the catalog 123"),
            "not in the catalog 123"
        );
    }

    #[test]
    fn plugin_catalogs_use_the_same_english_copy_reverse_index() {
        let english = HashMap::from([
            ("settings.title".to_string(), "Settings".to_string()),
            ("settings.open".to_string(), "Open Orca".to_string()),
        ]);
        let plugin = serde_json::json!({
            "settings": {"title": "Paramètres", "open": "Ouvrir Orca"}
        });
        let reverse = plugin_reverse_catalog(&english, &plugin);
        assert_eq!(
            reverse.get("Settings").map(String::as_str),
            Some("Paramètres")
        );
        assert_eq!(
            reverse.get("Open Orca").map(String::as_str),
            Some("Ouvrir Orca")
        );
    }
}
