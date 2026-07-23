//! Format a chord for display: glyphs (⌘⌥⇧⌃) on darwin, spelled-out modifier
//! names elsewhere. Pure. Used by conflict messages (M4) and the Settings UI.
//!
//! Ported from Orca `src/shared/keybindings.ts`:
//!   - `formatModifierGlyph` (:2141), `formatKeyToken` (:2201)
//!   - `formatKeybinding` (:2156), `formatKeybindingList` (:2186)

use crate::chord::{parse_keybinding, ModifierToken, ParsedKeybinding};
use crate::registry::KeybindingPlatform;

/// The display glyph for a double-tap modifier. Mirror of Orca
/// `formatModifierGlyph` (`keybindings.ts:2141-2154`). Note `Mod` renders as the
/// Cmd glyph on mac but plain `Ctrl` text elsewhere.
fn format_modifier_glyph(modifier: ModifierToken, is_mac: bool) -> &'static str {
    match modifier {
        ModifierToken::Mod => {
            if is_mac {
                "\u{2318}"
            } else {
                "Ctrl"
            }
        }
        ModifierToken::Cmd => {
            if is_mac {
                "\u{2318}"
            } else {
                "Cmd"
            }
        }
        ModifierToken::Ctrl => {
            if is_mac {
                "\u{2303}"
            } else {
                "Ctrl"
            }
        }
        ModifierToken::Alt => {
            if is_mac {
                "\u{2325}"
            } else {
                "Alt"
            }
        }
        ModifierToken::Shift => {
            if is_mac {
                "\u{21e7}"
            } else {
                "Shift"
            }
        }
    }
}

/// The display label for a key token. Mirror of Orca `formatKeyToken`
/// (`keybindings.ts:2201-2233`). Unknown tokens render verbatim.
fn format_key_token(token: &str) -> String {
    let label = match token {
        "BracketLeft" => "[",
        "BracketRight" => "]",
        "Minus" => "-",
        "Underscore" => "_",
        "Equal" => "=",
        "Plus" => "+",
        "ArrowLeft" => "\u{2190}",
        "ArrowRight" => "\u{2192}",
        "ArrowUp" => "\u{2191}",
        "ArrowDown" => "\u{2193}",
        "PageUp" => "PageUp",
        "PageDown" => "PageDown",
        "NumpadAdd" => "Numpad +",
        "NumpadSubtract" => "Numpad -",
        "Comma" => ",",
        "Period" => ".",
        "Slash" => "/",
        "Backslash" => "\\",
        "Semicolon" => ";",
        "Quote" => "'",
        "Backquote" => "`",
        "Enter" => "Enter",
        "Backspace" => "Backspace",
        "Delete" => "Delete",
        "Insert" => "Insert",
        "Tab" => "Tab",
        "Escape" => "Esc",
        "Space" => "Space",
        other => other,
    };
    label.to_string()
}

/// The per-part display pieces of a chord, in order. Mirror of Orca
/// `formatKeybinding` (`keybindings.ts:2156-2184`) — but taking an already
/// [`ParsedKeybinding`] instead of re-parsing. Double-tap chords render as the
/// modifier glyph twice.
fn format_keybinding_parts(parsed: &ParsedKeybinding, platform: KeybindingPlatform) -> Vec<String> {
    let is_mac = platform == KeybindingPlatform::Darwin;
    if let Some(modifier) = parsed.double_tap_modifier {
        let glyph = format_modifier_glyph(modifier, is_mac).to_string();
        return vec![glyph.clone(), glyph];
    }
    let mut parts: Vec<String> = Vec::new();
    // The virtual Mod renders as ⌘ on mac, Ctrl otherwise (inlined in Orca).
    if parsed.is_mod {
        parts.push(if is_mac { "\u{2318}" } else { "Ctrl" }.to_string());
    }
    if parsed.meta {
        parts.push(if is_mac { "\u{2318}" } else { "Cmd" }.to_string());
    }
    if parsed.control {
        parts.push(if is_mac { "\u{2303}" } else { "Ctrl" }.to_string());
    }
    if parsed.alt {
        parts.push(if is_mac { "\u{2325}" } else { "Alt" }.to_string());
    }
    if parsed.shift {
        parts.push(if is_mac { "\u{21e7}" } else { "Shift" }.to_string());
    }
    parts.push(format_key_token(&parsed.key));
    parts
}

/// Format one parsed chord into a single display string. The separator matches
/// Orca `formatKeybindingList` (`keybindings.ts:2195`): a space for double-tap,
/// nothing on darwin (glyphs abut: `⌘⇧P`), a `+` elsewhere (`Ctrl+Shift+P`).
pub fn format_keybinding(parsed: &ParsedKeybinding, platform: KeybindingPlatform) -> String {
    let separator = if parsed.double_tap_modifier.is_some() {
        " "
    } else if platform == KeybindingPlatform::Darwin {
        ""
    } else {
        "+"
    };
    format_keybinding_parts(parsed, platform).join(separator)
}

/// Format a list of chord strings for display, joined by `, `. Mirror of Orca
/// `formatKeybindingList` (`keybindings.ts:2186-2199`). An empty list is
/// `"Unassigned"`; a chord that fails to parse renders verbatim (Orca returns
/// `[binding]`).
pub fn format_keybinding_list(bindings: &[&str], platform: KeybindingPlatform) -> String {
    if bindings.is_empty() {
        return "Unassigned".to_string();
    }
    bindings
        .iter()
        .map(|binding| match parse_keybinding(binding) {
            Ok(parsed) => format_keybinding(&parsed, platform),
            Err(_) => binding.to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use KeybindingPlatform::*;

    fn fmt(binding: &str, platform: KeybindingPlatform) -> String {
        format_keybinding(&parse_keybinding(binding).expect("parses"), platform)
    }

    // Crux: darwin glyphs. Cmd+Shift+P -> ⌘⇧P. Mutating any glyph (or the
    // darwin/else branch) fails here.
    #[test]
    fn crux_darwin_glyphs() {
        assert_eq!(fmt("Cmd+Shift+P", Darwin), "\u{2318}\u{21e7}P");
        assert_eq!(fmt("Mod+Shift+J", Darwin), "\u{2318}\u{21e7}J"); // test.ts:179
        assert_eq!(fmt("Mod+Alt+F", Darwin), "\u{2318}\u{2325}F"); // test.ts:259
        assert_eq!(fmt("Ctrl+A", Darwin), "\u{2303}A");
    }

    #[test]
    fn linux_spells_out_modifiers() {
        assert_eq!(fmt("Mod+Shift+J", Linux), "Ctrl+Shift+J"); // test.ts:180
        assert_eq!(fmt("Cmd+Shift+P", Linux), "Cmd+Shift+P");
        assert_eq!(fmt("Mod+H", Linux), "Ctrl+H"); // test.ts:260
    }

    #[test]
    fn double_tap_renders_glyph_twice() {
        // keybindings.test.ts:1442-1451.
        assert_eq!(fmt("DoubleTap+Shift", Darwin), "\u{21e7} \u{21e7}");
        assert_eq!(fmt("DoubleTap+Shift", Linux), "Shift Shift");
        assert_eq!(fmt("DoubleTap+Mod", Darwin), "\u{2318} \u{2318}");
        assert_eq!(fmt("DoubleTap+Mod", Win32), "Ctrl Ctrl");
        assert_eq!(fmt("DoubleTap+Cmd", Win32), "Cmd Cmd");
        assert_eq!(fmt("DoubleTap+Alt", Darwin), "\u{2325} \u{2325}");
        assert_eq!(fmt("DoubleTap+Ctrl", Darwin), "\u{2303} \u{2303}");
    }

    #[test]
    fn key_tokens_render_as_labels() {
        assert_eq!(fmt("Mod+BracketLeft", Darwin), "\u{2318}[");
        assert_eq!(fmt("Mod+ArrowUp", Darwin), "\u{2318}\u{2191}");
        assert_eq!(fmt("Mod+NumpadAdd", Linux), "Ctrl+Numpad +");
        assert_eq!(fmt("Mod+Escape", Linux), "Ctrl+Esc");
    }

    #[test]
    fn list_formatting() {
        // keybindings.test.ts:179-181, 1450-1451.
        assert_eq!(
            format_keybinding_list(&["Mod+Shift+J"], Darwin),
            "\u{2318}\u{21e7}J"
        );
        assert_eq!(
            format_keybinding_list(&["Mod+Shift+J"], Linux),
            "Ctrl+Shift+J"
        );
        assert_eq!(format_keybinding_list(&[], Win32), "Unassigned");
        assert_eq!(
            format_keybinding_list(&["DoubleTap+Shift"], Darwin),
            "\u{21e7} \u{21e7}"
        );
        assert_eq!(
            format_keybinding_list(&["Mod+P", "Mod+Shift+E"], Linux),
            "Ctrl+P, Ctrl+Shift+E"
        );
    }

    #[test]
    fn unparseable_binding_renders_verbatim() {
        assert_eq!(format_keybinding_list(&["Ctrl+Nope"], Darwin), "Ctrl+Nope");
    }
}
