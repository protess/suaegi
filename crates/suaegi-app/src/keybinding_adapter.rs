//! M6a — the **pure** part of the iced -> `suaegi-keys` adapter.
//!
//! One stateless function, [`keybinding_input_from_iced`], turns a single iced
//! key event into a [`suaegi_keys::KeybindingInput`] (the DOM `KeyboardEvent`
//! vocabulary the resolver switches on). Everything here is a total,
//! side-effect-free mapping — no iced event loop, no focus, no dispatch. Action
//! DISPATCH, the double-tap detector, and the `tab.newAgent.${agent}` family are
//! M6b (stateful, human-eyes) and are intentionally NOT here.
//!
//! **F1 compliance — the `to_latin` ban.** This path NEVER calls
//! `iced::keyboard::Key::to_latin` (nor any equivalent latinization). Read the
//! doc-comment on [`suaegi_keys::KeybindingInput`]: the resolver's macOS
//! Option-compose and non-Latin / AltGr physical-code fallbacks require the RAW
//! logical `key` (a composed `'å'` stays `'å'`) kept SEPARATE from the physical
//! `code`. `to_latin` would collapse `Option+A` to `'a'` and destroy exactly the
//! signal those fallbacks need. That is why we carry `Key::Character(s)` through
//! verbatim and derive `code` from `Physical::Code` independently. `to_latin` is
//! not imported in this module; the composed-key unit test
//! (`mac_option_a_keeps_composed_logical_key`) is the executable proof.

use iced::keyboard::key::{Code, Named, Physical};
use iced::keyboard::{Key, Modifiers};

use suaegi_keys::KeybindingInput;

/// Map one iced key event to a [`KeybindingInput`] in the DOM `KeyboardEvent`
/// vocabulary the `suaegi-keys` resolver expects.
///
/// - `key` (logical): `Key::Character(s)` is carried through as the RAW string
///   `s` (never latinized — see the F1 note on the module); `Key::Named` becomes
///   its DOM `KeyboardEvent.key` name (`ArrowLeft`, `Delete`, Space = `" "`,
///   ...); `Key::Unidentified` becomes `""`.
/// - `code` (physical): `Physical::Code(c)` becomes the exact DOM
///   `KeyboardEvent.code` string via an explicit match ([`code_to_dom`]);
///   `Physical::Unidentified(_)` becomes `""`.
/// - modifiers: `alt`/`meta`(=logo)/`control`/`shift` read straight off
///   [`Modifiers`].
/// - `double_tap_modifier` is always `None` — double-tap detection is stateful
///   and lives in M6b.
pub fn keybinding_input_from_iced(
    key: &Key,
    physical: &Physical,
    modifiers: &Modifiers,
) -> KeybindingInput {
    KeybindingInput {
        // RAW logical key — no `to_latin`, no latinization. See the module F1 note.
        key: logical_key_string(key),
        code: physical_code_string(physical),
        alt: modifiers.alt(),
        // DOM `metaKey` is iced's `logo` (Cmd on macOS, Super/Win elsewhere).
        meta: modifiers.logo(),
        control: modifiers.control(),
        shift: modifiers.shift(),
        // Double-tap is a stateful gesture detected in M6b, never from one event.
        double_tap_modifier: None,
    }
}

/// The RAW logical key string (DOM `KeyboardEvent.key`).
///
/// `Key::Character(s)` is returned verbatim — a macOS Option+A event whose
/// logical key composed to `"å"` stays `"å"`; the resolver's physical fallback,
/// not this adapter, decides whether the physical code drives the match.
fn logical_key_string(key: &Key) -> String {
    match key {
        // Verbatim. NOT `key.to_latin(..)`: latinizing here would defeat F1.
        Key::Character(s) => s.to_string(),
        Key::Named(named) => named_key_dom(*named).to_string(),
        Key::Unidentified => String::new(),
    }
}

/// iced `Named` -> DOM `KeyboardEvent.key` name (Orca's vocabulary).
///
/// Space is `" "` (a single space), matching `normalize_key_token(" ")`.
/// Unmapped named keys return `""`, which the resolver reads as "no logical key"
/// and lets the physical `code` fall back — the correct behavior for keys
/// outside the shortcut vocabulary. **No wildcard for the covered set**: every
/// key the registry's default bindings can name is listed explicitly.
fn named_key_dom(named: Named) -> &'static str {
    match named {
        // Whitespace / editing.
        Named::Enter => "Enter",
        Named::Tab => "Tab",
        Named::Space => " ",
        Named::Backspace => "Backspace",
        Named::Delete => "Delete",
        Named::Insert => "Insert",
        Named::Escape => "Escape",
        // Navigation.
        Named::ArrowUp => "ArrowUp",
        Named::ArrowDown => "ArrowDown",
        Named::ArrowLeft => "ArrowLeft",
        Named::ArrowRight => "ArrowRight",
        Named::Home => "Home",
        Named::End => "End",
        Named::PageUp => "PageUp",
        Named::PageDown => "PageDown",
        // Function keys (the resolver honors F1-F24; F25+ resolve to no key,
        // which is correct — we still emit the faithful DOM name for 1-24).
        Named::F1 => "F1",
        Named::F2 => "F2",
        Named::F3 => "F3",
        Named::F4 => "F4",
        Named::F5 => "F5",
        Named::F6 => "F6",
        Named::F7 => "F7",
        Named::F8 => "F8",
        Named::F9 => "F9",
        Named::F10 => "F10",
        Named::F11 => "F11",
        Named::F12 => "F12",
        Named::F13 => "F13",
        Named::F14 => "F14",
        Named::F15 => "F15",
        Named::F16 => "F16",
        Named::F17 => "F17",
        Named::F18 => "F18",
        Named::F19 => "F19",
        Named::F20 => "F20",
        Named::F21 => "F21",
        Named::F22 => "F22",
        Named::F23 => "F23",
        Named::F24 => "F24",
        // Modifier logical keys — the resolver's `is_modifier_key` recognizes
        // these DOM names and treats them as "no pressed key". iced's `Super`
        // (Cmd / Windows) is DOM `"Meta"`.
        Named::Alt => "Alt",
        Named::AltGraph => "AltGraph",
        Named::Control => "Control",
        Named::Shift => "Shift",
        Named::Super => "Meta",
        Named::Meta => "Meta",
        Named::Hyper => "Hyper",
        Named::Symbol => "Symbol",
        Named::CapsLock => "CapsLock",
        Named::NumLock => "NumLock",
        Named::ScrollLock => "ScrollLock",
        Named::Fn => "Fn",
        Named::FnLock => "FnLock",
        Named::ContextMenu => "ContextMenu",
        // Anything outside the shortcut vocabulary: report "no logical key" and
        // let the physical `code` fall back.
        _ => "",
    }
}

/// The physical key string (DOM `KeyboardEvent.code`).
fn physical_code_string(physical: &Physical) -> String {
    match physical {
        Physical::Code(code) => code_to_dom(*code).to_string(),
        // A native scancode iced couldn't classify: no DOM code.
        Physical::Unidentified(_) => String::new(),
    }
}

/// iced `Code` -> DOM `KeyboardEvent.code` string, by **explicit match** (never
/// `format!("{code:?}")`: Debug is fragile — an upstream rename would silently
/// emit a wrong code and the resolver would misfire or go dead).
///
/// The iced `Code` variant names track the W3C UI Events `code` values, so the
/// mapping is 1:1 for every standard-position key. Coverage spans every class
/// the resolver's default bindings and physical fallbacks touch: letters
/// (`KeyA`-`KeyZ`), digits (`Digit0`-`Digit9`), the full punctuation set
/// (`BracketLeft`/`Right`, `Minus`, `Equal`, `Comma`, `Period`, `Slash`,
/// `Backslash`, `Semicolon`, `Quote`, `Backquote`, plus `Intl*`), whitespace /
/// editing, navigation, function `F1`-`F24`, the numpad (incl. `NumpadAdd` /
/// `NumpadSubtract`, which the resolver treats specially), and the modifier
/// codes. Media / browser / legacy / vendor keys — none of which appear in a
/// keybinding chord — collapse to `""` via the final arm.
fn code_to_dom(code: Code) -> &'static str {
    match code {
        // --- Letters ---
        Code::KeyA => "KeyA",
        Code::KeyB => "KeyB",
        Code::KeyC => "KeyC",
        Code::KeyD => "KeyD",
        Code::KeyE => "KeyE",
        Code::KeyF => "KeyF",
        Code::KeyG => "KeyG",
        Code::KeyH => "KeyH",
        Code::KeyI => "KeyI",
        Code::KeyJ => "KeyJ",
        Code::KeyK => "KeyK",
        Code::KeyL => "KeyL",
        Code::KeyM => "KeyM",
        Code::KeyN => "KeyN",
        Code::KeyO => "KeyO",
        Code::KeyP => "KeyP",
        Code::KeyQ => "KeyQ",
        Code::KeyR => "KeyR",
        Code::KeyS => "KeyS",
        Code::KeyT => "KeyT",
        Code::KeyU => "KeyU",
        Code::KeyV => "KeyV",
        Code::KeyW => "KeyW",
        Code::KeyX => "KeyX",
        Code::KeyY => "KeyY",
        Code::KeyZ => "KeyZ",
        // --- Digits (main row) ---
        Code::Digit0 => "Digit0",
        Code::Digit1 => "Digit1",
        Code::Digit2 => "Digit2",
        Code::Digit3 => "Digit3",
        Code::Digit4 => "Digit4",
        Code::Digit5 => "Digit5",
        Code::Digit6 => "Digit6",
        Code::Digit7 => "Digit7",
        Code::Digit8 => "Digit8",
        Code::Digit9 => "Digit9",
        // --- Punctuation ---
        Code::Backquote => "Backquote",
        Code::Backslash => "Backslash",
        Code::BracketLeft => "BracketLeft",
        Code::BracketRight => "BracketRight",
        Code::Comma => "Comma",
        Code::Equal => "Equal",
        Code::Minus => "Minus",
        Code::Period => "Period",
        Code::Quote => "Quote",
        Code::Semicolon => "Semicolon",
        Code::Slash => "Slash",
        Code::IntlBackslash => "IntlBackslash",
        Code::IntlRo => "IntlRo",
        Code::IntlYen => "IntlYen",
        // --- Whitespace / editing ---
        Code::Space => "Space",
        Code::Tab => "Tab",
        Code::Enter => "Enter",
        Code::Backspace => "Backspace",
        Code::Delete => "Delete",
        Code::Insert => "Insert",
        Code::Escape => "Escape",
        // --- Navigation ---
        Code::Home => "Home",
        Code::End => "End",
        Code::PageUp => "PageUp",
        Code::PageDown => "PageDown",
        Code::ArrowUp => "ArrowUp",
        Code::ArrowDown => "ArrowDown",
        Code::ArrowLeft => "ArrowLeft",
        Code::ArrowRight => "ArrowRight",
        // --- Modifiers / locks ---
        Code::AltLeft => "AltLeft",
        Code::AltRight => "AltRight",
        Code::ControlLeft => "ControlLeft",
        Code::ControlRight => "ControlRight",
        Code::ShiftLeft => "ShiftLeft",
        Code::ShiftRight => "ShiftRight",
        Code::SuperLeft => "MetaLeft",
        Code::SuperRight => "MetaRight",
        Code::CapsLock => "CapsLock",
        Code::NumLock => "NumLock",
        Code::ScrollLock => "ScrollLock",
        Code::ContextMenu => "ContextMenu",
        Code::Fn => "Fn",
        Code::FnLock => "FnLock",
        // --- Numpad ---
        Code::Numpad0 => "Numpad0",
        Code::Numpad1 => "Numpad1",
        Code::Numpad2 => "Numpad2",
        Code::Numpad3 => "Numpad3",
        Code::Numpad4 => "Numpad4",
        Code::Numpad5 => "Numpad5",
        Code::Numpad6 => "Numpad6",
        Code::Numpad7 => "Numpad7",
        Code::Numpad8 => "Numpad8",
        Code::Numpad9 => "Numpad9",
        Code::NumpadAdd => "NumpadAdd",
        Code::NumpadSubtract => "NumpadSubtract",
        Code::NumpadMultiply => "NumpadMultiply",
        Code::NumpadDivide => "NumpadDivide",
        Code::NumpadDecimal => "NumpadDecimal",
        Code::NumpadComma => "NumpadComma",
        Code::NumpadEnter => "NumpadEnter",
        Code::NumpadEqual => "NumpadEqual",
        Code::NumpadParenLeft => "NumpadParenLeft",
        Code::NumpadParenRight => "NumpadParenRight",
        Code::NumpadBackspace => "NumpadBackspace",
        Code::NumpadClear => "NumpadClear",
        Code::NumpadClearEntry => "NumpadClearEntry",
        Code::NumpadHash => "NumpadHash",
        Code::NumpadStar => "NumpadStar",
        // --- Function keys F1-F24 ---
        Code::F1 => "F1",
        Code::F2 => "F2",
        Code::F3 => "F3",
        Code::F4 => "F4",
        Code::F5 => "F5",
        Code::F6 => "F6",
        Code::F7 => "F7",
        Code::F8 => "F8",
        Code::F9 => "F9",
        Code::F10 => "F10",
        Code::F11 => "F11",
        Code::F12 => "F12",
        Code::F13 => "F13",
        Code::F14 => "F14",
        Code::F15 => "F15",
        Code::F16 => "F16",
        Code::F17 => "F17",
        Code::F18 => "F18",
        Code::F19 => "F19",
        Code::F20 => "F20",
        Code::F21 => "F21",
        Code::F22 => "F22",
        Code::F23 => "F23",
        Code::F24 => "F24",
        // --- System keys with DOM names ---
        Code::PrintScreen => "PrintScreen",
        Code::Pause => "Pause",
        // Media / browser / vendor / legacy keys never appear in a chord.
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use suaegi_keys::{keybinding_from_input, keybinding_matches_input, KeybindingPlatform};

    fn mods(alt: bool, meta: bool, control: bool, shift: bool) -> Modifiers {
        let mut m = Modifiers::empty();
        m.set(Modifiers::ALT, alt);
        m.set(Modifiers::LOGO, meta);
        m.set(Modifiers::CTRL, control);
        m.set(Modifiers::SHIFT, shift);
        m
    }

    fn no_mods() -> Modifiers {
        Modifiers::empty()
    }

    fn from(key: Key, code: Code, m: Modifiers) -> KeybindingInput {
        keybinding_input_from_iced(&key, &Physical::Code(code), &m)
    }

    // --- Physical code vocabulary ------------------------------------------

    /// The physical `code` must be the exact DOM string. Mutating any arm (e.g.
    /// `KeyA => "KeyB"`) flips one of these. Covers a letter, a digit, a
    /// punctuation key, and a numpad key — the classes the resolver branches on.
    #[test]
    fn physical_code_maps_to_dom_string() {
        assert_eq!(
            from(Key::Character("a".into()), Code::KeyA, no_mods()).code,
            "KeyA"
        );
        assert_eq!(
            from(Key::Character("1".into()), Code::Digit1, no_mods()).code,
            "Digit1"
        );
        assert_eq!(
            from(Key::Character("[".into()), Code::BracketLeft, no_mods()).code,
            "BracketLeft"
        );
        assert_eq!(
            from(Key::Named(Named::ArrowUp), Code::Numpad8, no_mods()).code,
            "Numpad8"
        );
        assert_eq!(
            from(Key::Character("+".into()), Code::NumpadAdd, no_mods()).code,
            "NumpadAdd"
        );
        assert_eq!(
            from(Key::Character(";".into()), Code::Semicolon, no_mods()).code,
            "Semicolon"
        );
    }

    /// `Physical::Unidentified` carries no DOM code.
    #[test]
    fn unidentified_physical_is_empty() {
        let input = keybinding_input_from_iced(
            &Key::Character("a".into()),
            &Physical::Unidentified(iced::keyboard::key::NativeCode::Unidentified),
            &no_mods(),
        );
        assert_eq!(input.code, "");
    }

    // --- to_latin ban / composed key ---------------------------------------

    /// **F1 proof.** A macOS Option+A event: logical key composed to `"å"`,
    /// physical `KeyA`, alt held. The adapter keeps the RAW `"å"` — it does NOT
    /// latinize to `"a"`. Mutating `logical_key_string` to hardcode/`to_latin`
    /// the char (`"a"`) fails this.
    #[test]
    fn mac_option_a_keeps_composed_logical_key() {
        let input = from(
            Key::Character("å".into()),
            Code::KeyA,
            mods(true, false, false, false),
        );
        assert_eq!(
            input.key, "å",
            "logical key must stay composed, never latinized"
        );
        assert_eq!(input.code, "KeyA");
        assert!(input.alt);
    }

    /// End-to-end smoke: that same composed input, fed to the resolver, matches
    /// an `Alt+A` binding on darwin via the mac-Option physical fallback. Proves
    /// the adapter feeds the resolver the exact shape its fallback needs — which
    /// only works BECAUSE `key` stayed `"å"` and `code` stayed `"KeyA"`.
    #[test]
    fn composed_option_a_resolves_to_alt_a_on_darwin() {
        let input = from(
            Key::Character("å".into()),
            Code::KeyA,
            mods(true, false, false, false),
        );
        assert!(keybinding_matches_input(
            "Alt+A",
            &input,
            KeybindingPlatform::Darwin
        ));
    }

    // --- Named key -> DOM name ---------------------------------------------

    /// Named keys map to their DOM `key` name; Space is `" "`. Mutating a name
    /// fails this.
    #[test]
    fn named_keys_map_to_dom_names() {
        assert_eq!(
            from(Key::Named(Named::ArrowLeft), Code::ArrowLeft, no_mods()).key,
            "ArrowLeft"
        );
        assert_eq!(
            from(Key::Named(Named::Space), Code::Space, no_mods()).key,
            " "
        );
        assert_eq!(
            from(Key::Named(Named::Delete), Code::Delete, no_mods()).key,
            "Delete"
        );
        assert_eq!(from(Key::Named(Named::F5), Code::F5, no_mods()).key, "F5");
    }

    /// A named key outside the vocabulary reports no logical key (`""`), letting
    /// the physical code fall back.
    #[test]
    fn unmapped_named_key_is_empty() {
        assert_eq!(
            from(Key::Named(Named::MediaPlayPause), Code::Space, no_mods()).key,
            ""
        );
        assert_eq!(from(Key::Unidentified, Code::KeyA, no_mods()).key, "");
    }

    // --- Modifiers ---------------------------------------------------------

    /// Each of alt / meta(logo) / control / shift maps to its own bool. Mutating
    /// any single wiring (e.g. `meta` reading `control()`) fails exactly one of
    /// the isolated assertions below.
    #[test]
    fn modifiers_map_each_bit_independently() {
        let base = Key::Character("a".into());

        let alt_only = from(base.clone(), Code::KeyA, mods(true, false, false, false));
        assert!(alt_only.alt && !alt_only.meta && !alt_only.control && !alt_only.shift);

        let meta_only = from(base.clone(), Code::KeyA, mods(false, true, false, false));
        assert!(!meta_only.alt && meta_only.meta && !meta_only.control && !meta_only.shift);

        let control_only = from(base.clone(), Code::KeyA, mods(false, false, true, false));
        assert!(
            !control_only.alt && !control_only.meta && control_only.control && !control_only.shift
        );

        let shift_only = from(base, Code::KeyA, mods(false, false, false, true));
        assert!(!shift_only.alt && !shift_only.meta && !shift_only.control && shift_only.shift);
    }

    /// `double_tap_modifier` is never set on the single-event path (that is M6b).
    #[test]
    fn double_tap_modifier_is_always_none() {
        let input = from(
            Key::Character("a".into()),
            Code::KeyA,
            mods(true, true, true, true),
        );
        assert!(input.double_tap_modifier.is_none());
    }

    // --- Integration smoke -------------------------------------------------

    /// A plain `Cmd+Shift+P` event (logical `"P"`, physical `KeyP`, logo+shift)
    /// canonicalizes to `Mod+Shift+P` on darwin.
    #[test]
    fn cmd_shift_p_canonicalizes_to_mod_shift_p() {
        let input = from(
            Key::Character("P".into()),
            Code::KeyP,
            mods(false, true, false, true),
        );
        let result = keybinding_from_input(&input, KeybindingPlatform::Darwin);
        assert_eq!(result.canonical(), Some("Mod+Shift+P"));
    }

    /// And the same event matches a `Mod+Shift+P` binding directly.
    #[test]
    fn cmd_shift_p_matches_mod_shift_p_binding() {
        let input = from(
            Key::Character("P".into()),
            Code::KeyP,
            mods(false, true, false, true),
        );
        assert!(keybinding_matches_input(
            "Mod+Shift+P",
            &input,
            KeybindingPlatform::Darwin
        ));
    }
}
