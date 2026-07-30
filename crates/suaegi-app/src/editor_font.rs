//! Font resolution shared by editable files and diff surfaces.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use iced::font::Family;
use iced::Font;
use suaegi_core::domain::UiSettings;

const MAX_CUSTOM_FAMILIES: usize = 256;
const MAX_FAMILY_NAME_BYTES: usize = 128;

static CUSTOM_FAMILIES: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();

/// Resolve Orca's editor-font fallback: an empty editor font follows the
/// terminal font, and a missing terminal font uses the platform monospace.
pub fn resolve(settings: &UiSettings) -> Font {
    resolve_names(&settings.editor_font_family, &settings.terminal_font_family)
}

fn resolve_names(editor: &str, terminal: &str) -> Font {
    let requested = match editor.trim() {
        "" => terminal.trim(),
        editor => editor,
    };
    if requested.is_empty() || requested.eq_ignore_ascii_case("monospace") {
        return Font::MONOSPACE;
    }

    let Some(name) = intern_family(requested) else {
        return Font::MONOSPACE;
    };
    Font {
        family: Family::Name(name),
        ..Font::MONOSPACE
    }
}

fn intern_family(value: &str) -> Option<&'static str> {
    if value.len() > MAX_FAMILY_NAME_BYTES || value.contains('\0') {
        return None;
    }
    let families = CUSTOM_FAMILIES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut families = families
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(existing) = families.get(value) {
        return Some(*existing);
    }
    if families.len() >= MAX_CUSTOM_FAMILIES {
        return None;
    }
    let name: &'static str = Box::leak(value.to_string().into_boxed_str());
    families.insert(value.to_string(), name);
    Some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_editor_font_follows_terminal_font() {
        let font = resolve_names("", "Menlo");
        assert_eq!(font.family, Family::Name("Menlo"));
    }

    #[test]
    fn explicit_editor_font_wins_and_is_trimmed() {
        let font = resolve_names("  D2Coding Nerd Font  ", "Menlo");
        assert_eq!(font.family, Family::Name("D2Coding Nerd Font"));
    }

    #[test]
    fn empty_chain_uses_generic_monospace() {
        assert_eq!(resolve_names(" ", "").family, Family::Monospace);
    }
}
