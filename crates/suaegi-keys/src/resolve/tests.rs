//! Resolver tests. The bulk are ported verbatim from Orca
//! `src/shared/keybindings.test.ts` — the `keybindingFromInput*`,
//! `keybindingMatchesInput`, `keybindingMatchesAction`, and digit-index
//! resolution describe blocks (agent-tab, conflict, normalize, and format
//! vectors live in their own milestones and are not re-ported here). Crux tests
//! name their killing mutation inline.

use super::*;
use crate::normalize::InvalidReason;
use KeybindingActionId as A;
use KeybindingPlatform::*;

// --- Test helpers -----------------------------------------------------------

/// A normal keydown event (no double-tap). Fields mirror the DOM `KeyboardEvent`
/// order used across the TS vectors: `key`, `code`, then meta/control/alt/shift.
fn ev(key: &str, code: &str, meta: bool, control: bool, alt: bool, shift: bool) -> KeybindingInput {
    KeybindingInput {
        key: key.to_string(),
        code: code.to_string(),
        meta,
        control,
        alt,
        shift,
        double_tap_modifier: None,
    }
}

/// A synthetic double-tap gesture input.
fn double_tap(modifier: ModifierToken) -> KeybindingInput {
    KeybindingInput {
        double_tap_modifier: Some(modifier),
        ..KeybindingInput::default()
    }
}

fn overrides(entries: &[(KeybindingActionId, &[&str])]) -> KeybindingOverrides {
    entries
        .iter()
        .map(|(action, bindings)| (*action, bindings.iter().map(|b| (*b).to_string()).collect()))
        .collect()
}

/// The canonical value of a `Valid` capture result (panics on `Invalid`).
fn captured(result: KeybindingValidationResult) -> String {
    match result {
        KeybindingValidationResult::Valid { canonical } => canonical,
        KeybindingValidationResult::Invalid { reason } => panic!("expected valid, got {reason:?}"),
    }
}

const NO_OPTS: KeybindingMatchOptions = KeybindingMatchOptions {
    context: None,
    terminal_shortcut_policy: None,
};

fn terminal(policy: TerminalShortcutPolicy) -> KeybindingMatchOptions {
    KeybindingMatchOptions {
        context: Some(KeybindingContext::Terminal),
        terminal_shortcut_policy: Some(policy),
    }
}

/// `keybinding_matches_action` with no overrides and no context (the common case).
fn matches(
    action: KeybindingActionId,
    input: &KeybindingInput,
    platform: KeybindingPlatform,
) -> bool {
    keybinding_matches_action(action, input, platform, None, &NO_OPTS)
}

// ===========================================================================
// keybinding_from_input — capture: event -> canonical chord
// ===========================================================================

#[test]
fn captures_key_events_into_canonical_shortcuts() {
    // keybindings.test.ts:98-114.
    assert_eq!(
        captured(keybinding_from_input(
            &ev("j", "KeyJ", true, false, false, false),
            Darwin
        )),
        "Mod+J"
    );
    assert_eq!(
        captured(keybinding_from_input(
            &ev("J", "KeyJ", false, true, true, true),
            Linux
        )),
        "Mod+Alt+Shift+J"
    );
    // Only a modifier pressed -> rejected with the capture-specific message.
    assert_eq!(
        keybinding_from_input(
            &ev("Control", "ControlLeft", false, true, false, false),
            Linux
        )
        .reason(),
        Some(&InvalidReason::PressAKey)
    );
}

#[test]
fn press_a_key_message_matches_orca() {
    assert_eq!(
        InvalidReason::PressAKey.to_string(),
        "Press a key, not only a modifier."
    );
}

#[test]
fn captures_macos_option_composed_events_via_physical_code() {
    // keybindings.test.ts:116-141. Option+C -> 'ç', Option+[ -> '“'.
    assert_eq!(
        captured(keybinding_from_input(
            &ev("\u{e7}", "KeyC", true, false, true, false),
            Darwin
        )),
        "Mod+Alt+C"
    );
    assert_eq!(
        captured(keybinding_from_input(
            &ev("\u{201c}", "BracketLeft", true, false, true, false),
            Darwin
        )),
        "Mod+Alt+BracketLeft"
    );
    // A bare Option (modifier key) captures nothing.
    assert_eq!(
        keybinding_from_input(&ev("Alt", "AltLeft", false, false, true, false), Darwin).reason(),
        Some(&InvalidReason::PressAKey)
    );
    // Option+Digit composes to '¡' but the physical Digit1 is not captured
    // (only letters/punctuation take the mac-Option composed capture fallback).
    assert_eq!(
        keybinding_from_input(&ev("\u{a1}", "Digit1", true, false, true, false), Darwin).reason(),
        Some(&InvalidReason::PressAKey)
    );
}

#[test]
fn capture_applies_per_action_bare_rules() {
    // keybindings.test.ts:143-158.
    let delete_event = ev("Delete", "Delete", false, false, false, false);
    assert!(!keybinding_from_input(&delete_event, Linux).is_valid());
    assert_eq!(
        captured(keybinding_from_input_for_action(
            A::FileExplorerDelete,
            &delete_event,
            Linux
        )),
        "Delete"
    );
}

#[test]
fn capture_rejects_shift_only_unless_action_opts_in() {
    // keybindings.test.ts:82-96.
    let shift_space = ev(" ", "Space", false, false, false, true);
    assert!(!keybinding_from_input(&shift_space, Darwin).is_valid());
    assert_eq!(
        captured(keybinding_from_input_for_action(
            A::TerminalSwitchInputSource,
            &shift_space,
            Darwin
        )),
        "Shift+Space"
    );
}

#[test]
fn captures_numpad_tokens() {
    // keybindings.test.ts:184-197.
    assert_eq!(
        captured(keybinding_from_input(
            &ev("+", "NumpadAdd", true, false, false, false),
            Darwin
        )),
        "Mod+NumpadAdd"
    );
}

#[test]
fn captures_dvorak_by_logical_key() {
    // keybindings.test.ts:1142-1149.
    assert_eq!(
        captured(keybinding_from_input(
            &ev(",", "KeyW", true, false, false, false),
            Darwin
        )),
        "Mod+Comma"
    );
    assert_eq!(
        captured(keybinding_from_input(
            &ev("w", "Comma", true, false, false, false),
            Darwin
        )),
        "Mod+W"
    );
}

#[test]
fn captures_shifted_punctuation_alias() {
    // keybindings.test.ts:1224-1227.
    assert_eq!(
        captured(keybinding_from_input(
            &ev("<", "Comma", true, false, false, true),
            Darwin
        )),
        "Mod+Shift+Comma"
    );
}

#[test]
fn captures_double_tap_gestures() {
    // keybindings.test.ts:1406-1438.
    assert_eq!(
        captured(keybinding_from_input(
            &double_tap(ModifierToken::Shift),
            Darwin
        )),
        "DoubleTap+Shift"
    );
    // The platform-primary modifier canonicalizes to Mod.
    assert_eq!(
        captured(keybinding_from_input(
            &double_tap(ModifierToken::Cmd),
            Darwin
        )),
        "DoubleTap+Mod"
    );
    assert_eq!(
        captured(keybinding_from_input(
            &double_tap(ModifierToken::Ctrl),
            Win32
        )),
        "DoubleTap+Mod"
    );
    assert_eq!(
        captured(keybinding_from_input(
            &double_tap(ModifierToken::Ctrl),
            Linux
        )),
        "DoubleTap+Mod"
    );
    // A non-primary modifier keeps its explicit token.
    assert_eq!(
        captured(keybinding_from_input(
            &double_tap(ModifierToken::Ctrl),
            Darwin
        )),
        "DoubleTap+Ctrl"
    );
    assert_eq!(
        captured(keybinding_from_input(
            &double_tap(ModifierToken::Alt),
            Linux
        )),
        "DoubleTap+Alt"
    );
    assert_eq!(
        captured(keybinding_from_input(
            &double_tap(ModifierToken::Cmd),
            Linux
        )),
        "DoubleTap+Cmd"
    );
}

// ===========================================================================
// keybinding_matches_action — plain matches
// ===========================================================================

// Crux (plain match + modifier comparison): Cmd+Shift+A matches editor.addReviewNote,
// and Ctrl+Alt+N (a modifier mismatch) does not. Mutating any comparison in
// `modifier_state_matches` (e.g. dropping the `shift ==` term) flips one of these.
#[test]
fn crux_plain_match_and_modifier_mismatch() {
    // keybindings.test.ts:230-252. addReviewNote = Mod+Shift+A.
    let mac_chord = ev("a", "KeyA", true, false, false, true);
    let ctrl_chord = ev("a", "KeyA", false, true, false, true);
    assert!(matches(A::EditorAddReviewNote, &mac_chord, Darwin));
    assert!(matches(A::EditorAddReviewNote, &ctrl_chord, Linux));
    assert!(matches(A::EditorAddReviewNote, &ctrl_chord, Win32));
    // Ctrl+Alt+N is neither the chord nor the right key -> no match.
    let old_ctrl_alt = ev("n", "KeyN", false, true, true, false);
    assert!(!matches(A::EditorAddReviewNote, &old_ctrl_alt, Linux));
    assert!(!matches(A::EditorAddReviewNote, &old_ctrl_alt, Win32));
    // A modifier-only difference must not match: Cmd+Shift+A is not Cmd+A.
    assert!(!matches(
        A::EditorAddReviewNote,
        &ev("a", "KeyA", true, false, false, false),
        Darwin
    ));
}

#[test]
fn matches_numpad_zoom_shortcuts() {
    // keybindings.test.ts:198-209.
    assert!(matches(
        A::ZoomIn,
        &ev("+", "NumpadAdd", true, false, false, false),
        Darwin
    ));
    assert!(matches(
        A::ZoomOut,
        &ev("-", "NumpadSubtract", true, false, false, false),
        Darwin
    ));
}

#[test]
fn matches_f7_diff_navigation() {
    // keybindings.test.ts:170-175.
    let f7 = ev("F7", "F7", false, false, false, false);
    let shift_f7 = ev("F7", "F7", false, false, false, true);
    assert!(matches(A::EditorNextChange, &f7, Darwin));
    assert!(!matches(A::EditorNextChange, &shift_f7, Darwin));
    assert!(matches(A::EditorPreviousChange, &shift_f7, Darwin));
    assert!(!matches(A::EditorPreviousChange, &f7, Darwin));
}

#[test]
fn matches_browser_history_shortcuts() {
    // keybindings.test.ts:518-545.
    assert!(matches(
        A::BrowserBack,
        &ev("[", "BracketLeft", true, false, false, false),
        Darwin
    ));
    assert!(matches(
        A::BrowserForward,
        &ev("ArrowRight", "ArrowRight", false, false, true, false),
        Linux
    ));
}

#[test]
fn matches_zoom_reset_and_focus_list_on_distinct_chords() {
    // keybindings.test.ts:310-327. zoom.reset=Mod+0, focusWorktreeList=Mod+Shift+0.
    let zoom_reset = ev("0", "Digit0", true, false, false, false);
    let focus_list = ev("0", "Digit0", true, false, false, true);
    assert!(matches(A::ZoomReset, &zoom_reset, Darwin));
    assert!(!matches(A::SidebarFocusWorktreeList, &zoom_reset, Darwin));
    assert!(matches(A::SidebarFocusWorktreeList, &focus_list, Darwin));
    assert!(!matches(A::ZoomReset, &focus_list, Darwin));
}

#[test]
fn matches_explorer_vs_simulator_by_extra_alt() {
    // keybindings.test.ts:860-880. explorer=Mod+Shift+E, simulator=Mod+Alt+Shift+E.
    assert!(matches(
        A::SidebarExplorerToggle,
        &ev("e", "KeyE", true, false, false, true),
        Darwin
    ));
    assert!(!matches(
        A::TabNewSimulator,
        &ev("e", "KeyE", true, false, false, true),
        Darwin
    ));
    assert!(matches(
        A::TabNewSimulator,
        &ev("e", "KeyE", true, false, true, true),
        Darwin
    ));
}

#[test]
fn matches_undo_redo_by_logical_key_not_physical() {
    // keybindings.test.ts:1083-1117.
    // undo=Mod+Z: logical 'z' matches regardless of physical code...
    assert!(matches(
        A::FileExplorerUndo,
        &ev("z", "Semicolon", true, false, false, false),
        Darwin
    ));
    // ...but physical KeyZ producing logical ';' does not.
    assert!(!matches(
        A::FileExplorerUndo,
        &ev(";", "KeyZ", true, false, false, false),
        Darwin
    ));
    // redo=Mod+Shift+Z.
    assert!(matches(
        A::FileExplorerRedo,
        &ev("Z", "Semicolon", true, false, false, true),
        Darwin
    ));
    // redo also has Ctrl+Y on linux: logical 'y' matches.
    assert!(matches(
        A::FileExplorerRedo,
        &ev("y", "KeyF", false, true, false, false),
        Linux
    ));
    assert!(!matches(
        A::FileExplorerRedo,
        &ev("f", "KeyY", false, true, false, false),
        Linux
    ));
}

#[test]
fn matches_dvorak_by_logical_key() {
    // keybindings.test.ts:1138-1141. app.settings=Mod+Comma, tab.close=Mod+W.
    let dvorak_w = ev(",", "KeyW", true, false, false, false);
    let dvorak_comma = ev("w", "Comma", true, false, false, false);
    assert!(matches(A::AppSettings, &dvorak_w, Darwin));
    assert!(!matches(A::TabClose, &dvorak_w, Darwin));
    assert!(matches(A::TabClose, &dvorak_comma, Darwin));
    assert!(!matches(A::AppSettings, &dvorak_comma, Darwin));
}

#[test]
fn matches_terminal_paste_defaults() {
    // keybindings.test.ts:1045-1058.
    assert!(matches(
        A::TerminalPaste,
        &ev("v", "KeyV", false, true, false, false),
        Linux
    ));
    assert!(matches(
        A::TerminalPaste,
        &ev("Insert", "Insert", false, false, false, true),
        Linux
    ));
}

#[test]
fn matches_file_explorer_delete_default() {
    // keybindings.test.ts:1066-1072.
    assert!(matches(
        A::FileExplorerDelete,
        &ev("Delete", "Delete", false, false, false, false),
        Linux
    ));
}

// ===========================================================================
// mac Option physical fallback (letter + punctuation) — a CRUX
// ===========================================================================

// Crux (mac Option letter physical fallback): floatingWorkspace.maximize is
// Mod+Alt+Shift+A. macOS Option+A composes to 'å' (no logical Latin key), so the
// chord must resolve via the PHYSICAL code KeyA, and must NOT match as literal
// 'å'. Mutating `should_use_mac_option_letter_physical_fallback` to return false
// (i.e. using the logical key only) makes this stop matching -> test FAILS.
#[test]
fn crux_mac_option_letter_physical_fallback() {
    // keybindings.test.ts:801-813, 555-565.
    let mac_composed_maximize = ev("\u{e5}", "KeyA", true, false, true, true); // Option+Shift+A -> 'å'
    assert!(matches(
        A::FloatingWorkspaceMaximize,
        &mac_composed_maximize,
        Darwin
    ));
    // And capture round-trips to the same canonical chord.
    assert_eq!(
        captured(keybinding_from_input(&mac_composed_maximize, Darwin)),
        "Mod+Alt+Shift+A"
    );

    // tab.closeAll=Mod+Alt+W. Option+W composes to '∑' (U+2211) -> physical KeyW.
    let mac_composed_close_all = ev("\u{2211}", "KeyW", true, false, true, false);
    assert!(matches(A::TabCloseAll, &mac_composed_close_all, Darwin));
}

// Crux (mac Option false-fire guard): the composed 'å' must not be treated as a
// literal shortcut key, and the physical fallback only fires when Alt is held.
// tab.close=Mod+W and tab.closeAll=Mod+Alt+W are neighbors: the composed close-all
// chord must not fire tab.close, nor plain Cmd+W fire tab.closeAll.
#[test]
fn crux_mac_option_neighbors_do_not_cross_fire() {
    // keybindings.test.ts:600-601.
    let mac_composed_close_all = ev("\u{2211}", "KeyW", true, false, true, false);
    let mac_close_active = ev("w", "KeyW", true, false, false, false);
    assert!(!matches(A::TabClose, &mac_composed_close_all, Darwin));
    assert!(!matches(A::TabCloseAll, &mac_close_active, Darwin));
}

#[test]
fn matches_mac_option_composed_brackets() {
    // keybindings.test.ts:1454-1481. same-type defaults are Mod+Alt+bracket.
    // Option+[ -> '“' (U+201C), Option+] -> '‘' (U+2018), code stays Bracket*.
    let mac_option_left = ev("\u{201c}", "BracketLeft", true, false, true, false);
    let mac_option_right = ev("\u{2018}", "BracketRight", true, false, true, false);
    assert!(matches(A::TabPreviousSameType, &mac_option_left, Darwin));
    assert!(!matches(A::TabNextSameType, &mac_option_left, Darwin));
    assert!(matches(A::TabNextSameType, &mac_option_right, Darwin));
    assert!(!matches(A::TabPreviousSameType, &mac_option_right, Darwin));
}

// ===========================================================================
// non-Latin / AltGr physical fallback — a CRUX
// ===========================================================================

// Crux (non-Latin physical fallback): a Cyrillic/Greek layout reports a non-Latin
// logical key for a physical letter (#6274); the chord must fall through to the
// physical code. browser.grabElement=Mod+C: physical KeyC producing Cyrillic 'с'
// matches; but physical KeyV producing 'м' must NOT (fallback keys the *specific*
// code). Mutating `should_use_non_latin_shortcut_physical_fallback` to return
// false makes the positive stop matching -> test FAILS.
#[test]
fn crux_non_latin_physical_fallback() {
    // keybindings.test.ts:1156-1189.
    let cyrillic_ctrl_c = ev("\u{441}", "KeyC", false, true, false, false); // 'с'
    assert!(matches(A::BrowserGrabElement, &cyrillic_ctrl_c, Win32));
    assert!(matches(A::BrowserGrabElement, &cyrillic_ctrl_c, Linux));
    // Ctrl+Shift+C on the same layout -> terminal.copySelection.
    let cyrillic_ctrl_shift_c = ev("\u{441}", "KeyC", false, true, false, true);
    assert!(matches(
        A::TerminalCopySelection,
        &cyrillic_ctrl_shift_c,
        Win32
    ));
    // Greek layout: physical KeyP -> 'π' (U+03C0); Ctrl+P still matches.
    assert!(matches(
        A::WorktreeQuickOpen,
        &ev("\u{3c0}", "KeyP", false, true, false, false),
        Win32
    ));
    // The fallback must not steal a different physical key: Ctrl+V ('м') is not Ctrl+C.
    assert!(!matches(
        A::BrowserGrabElement,
        &ev("\u{43c}", "KeyV", false, true, false, false),
        Win32
    ));
}

// Crux (AltGr text-entry guard): Windows/Linux AltGr arrives as Ctrl+Alt. A char
// composed via AltGr (e.g. AltGr+C -> '¢') must stay text input, never a shortcut.
// editor.copyContext=Mod+Alt+C, so the modifier state otherwise matches — only the
// `control && alt` AltGr gate keeps it from firing. Mutating that gate off makes
// this text-entry input WRONGLY match the shortcut -> test FAILS.
#[test]
fn crux_altgr_text_entry_not_hijacked() {
    // keybindings.test.ts:1197-1210.
    let altgr_c = ev("\u{a2}", "KeyC", false, true, true, false); // '¢'
    assert!(!matches(A::EditorCopyContext, &altgr_c, Win32));
}

// ===========================================================================
// punctuation semantic vs physical fallback (JIS + shifted aliases)
// ===========================================================================

#[test]
fn matches_shifted_punctuation_only_while_shift_held() {
    // keybindings.test.ts:1223-1234.
    let shifted_comma = ev("<", "Comma", true, false, false, true);
    assert!(keybinding_matches_input(
        "Mod+Shift+Comma",
        &shifted_comma,
        Darwin
    ));
    // Without shift and via a different physical code, the alias does not apply.
    let no_shift = ev("<", "IntlBackslash", true, false, false, false);
    assert!(!keybinding_matches_input("Mod+Comma", &no_shift, Darwin));
}

#[test]
fn matches_jis_bracket_shortcuts() {
    // keybindings.test.ts:1237-1362 (representative subset).
    // JIS: physical BracketRight produces logical '[', Backslash produces ']'.
    let jis_left = ev("[", "BracketRight", true, false, false, false);
    let jis_right = ev("]", "Backslash", true, false, false, false);
    // Semantic (logical) punctuation drives these matches.
    assert!(matches(A::TerminalFocusPreviousPane, &jis_left, Darwin)); // Mod+BracketLeft
    assert!(!matches(A::TerminalFocusNextPane, &jis_left, Darwin));
    assert!(matches(A::TerminalFocusNextPane, &jis_right, Darwin)); // Mod+BracketRight
                                                                    // Alt+bracket same-type default (fresh install), still by logical key.
    assert!(matches(
        A::TabPreviousSameType,
        &ev("[", "BracketRight", true, false, true, false),
        Darwin
    ));
    assert!(matches(
        A::TabNextSameType,
        &ev("]", "Backslash", true, false, true, false),
        Darwin
    ));

    // A Dead composed key with no logical token falls back to the physical code.
    let dead_bracket_right = ev("Dead", "BracketRight", true, false, false, true);
    assert!(keybinding_matches_action(
        A::TabNextSameType,
        &dead_bracket_right,
        Darwin,
        Some(&overrides(&[(
            A::TabNextSameType,
            &["Mod+Shift+BracketRight"]
        )])),
        &NO_OPTS
    ));
    // linux AltGr Dead on BracketLeft (Ctrl+Alt) still resolves same-type prev
    // because physical punctuation is present (not the text-entry guard case).
    let dead_bracket_left = ev("Dead", "BracketLeft", false, true, true, false);
    assert!(matches(A::TabPreviousSameType, &dead_bracket_left, Linux));
    // But a shifted '[' on Digit8 (Ctrl+Alt, no physical punctuation) is text.
    let altgr_digit8 = ev("[", "Digit8", false, true, true, false);
    assert!(!matches(A::TabPreviousSameType, &altgr_digit8, Linux));
}

// ===========================================================================
// overrides + effective bindings
// ===========================================================================

#[test]
fn effective_bindings_use_overrides_wholesale() {
    // keybindings.test.ts:263-287.
    let ov = overrides(&[(A::WorktreeQuickOpen, &["Ctrl+Alt+O", "not-a-shortcut"])]);
    assert_eq!(
        get_effective_keybindings_for_action(A::WorktreeQuickOpen, Linux, Some(&ov)),
        vec!["Ctrl+Alt+O".to_string()]
    );
    assert!(keybinding_matches_action(
        A::WorktreeQuickOpen,
        &ev("o", "KeyO", false, true, true, false),
        Linux,
        Some(&ov),
        &NO_OPTS
    ));
    assert!(!keybinding_matches_action(
        A::WorktreeQuickOpen,
        &ev("p", "KeyP", false, true, false, false),
        Linux,
        Some(&ov),
        &NO_OPTS
    ));
}

#[test]
fn unassigned_actions_match_only_through_overrides() {
    // keybindings.test.ts:616-767 (representative).
    // equalizePaneSizes ships unbound.
    let equal = ev("=", "Equal", true, false, false, false);
    assert!(!matches(A::TerminalEqualizePaneSizes, &equal, Darwin));
    assert!(keybinding_matches_action(
        A::TerminalEqualizePaneSizes,
        &equal,
        Darwin,
        Some(&overrides(&[(
            A::TerminalEqualizePaneSizes,
            &["Mod+Equal"]
        )])),
        &NO_OPTS
    ));
    // workspace.delete ships unbound.
    let del = ev("Backspace", "Backspace", false, true, false, true);
    assert!(!matches(A::WorkspaceDelete, &del, Linux));
    assert!(keybinding_matches_action(
        A::WorkspaceDelete,
        &del,
        Linux,
        Some(&overrides(&[(
            A::WorkspaceDelete,
            &["Mod+Shift+Backspace"]
        )])),
        &NO_OPTS
    ));
}

// ===========================================================================
// terminal policy — a CRUX
// ===========================================================================

// Crux (terminal policy): worktree.quickOpen (allow_in_terminal = false) fires in
// app context and under orca-first, but NOT inside a terminal under terminal-first.
// terminal.search (scope=terminal) fires even under terminal-first. Mutating
// `keybinding_is_active_in_context` (e.g. always returning true) makes the
// quickOpen-under-terminal-first case WRONGLY fire -> test FAILS.
#[test]
fn crux_terminal_shortcut_policy_gates_non_terminal_actions() {
    // keybindings.test.ts:940-961.
    let ctrl_p = ev("p", "KeyP", false, true, false, false);
    assert!(matches(A::WorktreeQuickOpen, &ctrl_p, Linux));
    assert!(keybinding_matches_action(
        A::WorktreeQuickOpen,
        &ctrl_p,
        Linux,
        None,
        &terminal(TerminalShortcutPolicy::OrcaFirst)
    ));
    // terminal-first: a non-terminal action is suppressed inside a terminal.
    assert!(!keybinding_matches_action(
        A::WorktreeQuickOpen,
        &ctrl_p,
        Linux,
        None,
        &terminal(TerminalShortcutPolicy::TerminalFirst)
    ));
    // ...but a terminal-scoped action still fires under terminal-first.
    assert!(keybinding_matches_action(
        A::TerminalSearch,
        &ev("f", "KeyF", false, true, false, false),
        Linux,
        None,
        &terminal(TerminalShortcutPolicy::TerminalFirst)
    ));
}

#[test]
fn terminal_allowed_actions_active_under_terminal_first() {
    // keybindings.test.ts:990-1036.
    // floatingTerminal.toggle has allow_in_terminal = true.
    assert!(keybinding_matches_action(
        A::FloatingTerminalToggle,
        &ev("a", "KeyA", false, true, true, false),
        Linux,
        None,
        &terminal(TerminalShortcutPolicy::TerminalFirst)
    ));
    // tab.previousRecent (allow_in_terminal = true) via Ctrl+Tab.
    assert!(keybinding_matches_action(
        A::TabPreviousRecent,
        &ev("Tab", "Tab", false, true, false, false),
        Linux,
        None,
        &terminal(TerminalShortcutPolicy::TerminalFirst)
    ));
    // app context with a terminal-first policy configured does not gate anything.
    let app_focus = KeybindingMatchOptions {
        context: Some(KeybindingContext::App),
        terminal_shortcut_policy: Some(TerminalShortcutPolicy::TerminalFirst),
    };
    assert!(keybinding_matches_action(
        A::WorktreePalette,
        &ev("j", "KeyJ", true, false, false, false),
        Darwin,
        None,
        &app_focus
    ));
    // tab.rename in a terminal under terminal-first is suppressed (non-terminal).
    assert!(!keybinding_matches_action(
        A::TabRename,
        &ev("r", "KeyR", true, false, false, false),
        Darwin,
        None,
        &terminal(TerminalShortcutPolicy::TerminalFirst)
    ));
}

// ===========================================================================
// keybinding_matches_input — double-tap
// ===========================================================================

#[test]
fn matches_double_tap_only_against_double_tap_input() {
    // keybindings.test.ts:1365-1403.
    assert!(keybinding_matches_input(
        "DoubleTap+Shift",
        &double_tap(ModifierToken::Shift),
        Darwin
    ));
    // Mod resolves per platform: Cmd == Mod on mac, Ctrl == Mod elsewhere.
    assert!(keybinding_matches_input(
        "DoubleTap+Mod",
        &double_tap(ModifierToken::Cmd),
        Darwin
    ));
    assert!(keybinding_matches_input(
        "DoubleTap+Mod",
        &double_tap(ModifierToken::Ctrl),
        Win32
    ));
    assert!(!keybinding_matches_input(
        "DoubleTap+Mod",
        &double_tap(ModifierToken::Cmd),
        Win32
    ));
    assert!(!keybinding_matches_input(
        "DoubleTap+Mod",
        &double_tap(ModifierToken::Ctrl),
        Darwin
    ));
    assert!(!keybinding_matches_input(
        "DoubleTap+Shift",
        &double_tap(ModifierToken::Alt),
        Darwin
    ));
    // Cross-type negatives.
    assert!(!keybinding_matches_input(
        "DoubleTap+Shift",
        &ev("A", "KeyA", false, false, false, true),
        Darwin
    ));
    assert!(!keybinding_matches_input(
        "Mod+P",
        &double_tap(ModifierToken::Cmd),
        Darwin
    ));
    // Action-level matching via overrides.
    assert!(keybinding_matches_action(
        A::WorktreeQuickOpen,
        &double_tap(ModifierToken::Shift),
        Darwin,
        Some(&overrides(&[(A::WorktreeQuickOpen, &["DoubleTap+Shift"])])),
        &NO_OPTS
    ));
    assert!(!keybinding_matches_action(
        A::WorktreeQuickOpen,
        &double_tap(ModifierToken::Alt),
        Darwin,
        Some(&overrides(&[(A::WorktreeQuickOpen, &["DoubleTap+Shift"])])),
        &NO_OPTS
    ));
}

// ===========================================================================
// digit-index resolution — a CRUX
// ===========================================================================

fn digit_input(digit: &str, meta: bool, control: bool, alt: bool, shift: bool) -> KeybindingInput {
    ev(digit, &format!("Digit{digit}"), meta, control, alt, shift)
}

// Crux (digit-index resolution): Mod+N resolves to index N-1 for the right
// modifier, and Mod+0 never resolves (loop is 1..=9). tab.selectByIndex defaults
// to Ctrl+1-9 on darwin. Mutating the `value - 1` index math, or widening the
// 1..=9 loop to include 0, fails one of these.
#[test]
fn crux_digit_index_resolution() {
    // keybindings.test.ts:1552-1596.
    // darwin: workspace = Cmd(Mod)+1-9, tab = Ctrl+1-9.
    assert_eq!(
        match_keybinding_digit_index(
            A::WorkspaceSelectByIndex,
            &digit_input("3", true, false, false, false),
            Darwin,
            None,
            &NO_OPTS
        ),
        Some(2)
    );
    assert_eq!(
        match_keybinding_digit_index(
            A::TabSelectByIndex,
            &digit_input("3", true, false, false, false),
            Darwin,
            None,
            &NO_OPTS
        ),
        None
    );
    assert_eq!(
        match_keybinding_digit_index(
            A::TabSelectByIndex,
            &digit_input("3", false, true, false, false),
            Darwin,
            None,
            &NO_OPTS
        ),
        Some(2)
    );
    // linux: workspace = Ctrl(Mod)+1-9, tab = Alt+1-9.
    assert_eq!(
        match_keybinding_digit_index(
            A::WorkspaceSelectByIndex,
            &digit_input("4", false, true, false, false),
            Linux,
            None,
            &NO_OPTS
        ),
        Some(3)
    );
    assert_eq!(
        match_keybinding_digit_index(
            A::TabSelectByIndex,
            &digit_input("4", false, false, true, false),
            Linux,
            None,
            &NO_OPTS
        ),
        Some(3)
    );
    // Mod+0 never resolves (index range is 1-9).
    assert_eq!(
        match_keybinding_digit_index(
            A::TabSelectByIndex,
            &digit_input("0", false, true, false, false),
            Darwin,
            None,
            &NO_OPTS
        ),
        None
    );
    // Extra modifiers (Shift) break the exact chord match.
    assert_eq!(
        match_keybinding_digit_index(
            A::WorkspaceSelectByIndex,
            &digit_input("3", true, false, false, true),
            Darwin,
            None,
            &NO_OPTS
        ),
        None
    );
    // A non-digit press never resolves.
    assert_eq!(
        match_keybinding_digit_index(
            A::TabSelectByIndex,
            &ev("p", "KeyP", false, true, false, false),
            Darwin,
            None,
            &NO_OPTS
        ),
        None
    );
}

#[test]
fn digit_index_honors_custom_bindings_and_terminal_gate() {
    // keybindings.test.ts:1598-1651.
    let swapped = overrides(&[
        (A::TabSelectByIndex, &["Mod+1"]),
        (A::WorkspaceSelectByIndex, &["Ctrl+1"]),
    ]);
    assert_eq!(
        match_keybinding_digit_index(
            A::TabSelectByIndex,
            &digit_input("5", true, false, false, false),
            Darwin,
            Some(&swapped),
            &NO_OPTS
        ),
        Some(4)
    );
    assert_eq!(
        match_keybinding_digit_index(
            A::WorkspaceSelectByIndex,
            &digit_input("5", false, true, false, false),
            Darwin,
            Some(&swapped),
            &NO_OPTS
        ),
        Some(4)
    );
    // A disabled (empty) override never fires.
    assert_eq!(
        match_keybinding_digit_index(
            A::TabSelectByIndex,
            &digit_input("5", false, true, false, false),
            Darwin,
            Some(&overrides(&[(A::TabSelectByIndex, &[])])),
            &NO_OPTS
        ),
        None
    );
    // Terminal-first suppresses the non-terminal digit-index action.
    assert_eq!(
        match_keybinding_digit_index(
            A::TabSelectByIndex,
            &digit_input("2", false, true, false, false),
            Darwin,
            None,
            &terminal(TerminalShortcutPolicy::TerminalFirst)
        ),
        None
    );
    assert_eq!(
        match_keybinding_digit_index(
            A::TabSelectByIndex,
            &digit_input("2", false, true, false, false),
            Darwin,
            None,
            &terminal(TerminalShortcutPolicy::OrcaFirst)
        ),
        Some(1)
    );
}

#[test]
fn digit_index_matches_via_physical_code_fallback() {
    // keybindings.test.ts:1692-1701. key empty, code carries the digit.
    assert_eq!(
        match_keybinding_digit_index(
            A::TabSelectByIndex,
            &ev("", "Digit5", false, true, false, false),
            Darwin,
            None,
            &NO_OPTS
        ),
        Some(4)
    );
}

#[test]
fn digit_index_capture_canonicalizes_to_one() {
    // keybindings.test.ts:1654-1690.
    assert_eq!(
        captured(keybinding_from_input_for_action(
            A::WorkspaceSelectByIndex,
            &digit_input("7", true, false, false, false),
            Darwin
        )),
        "Mod+1"
    );
    assert_eq!(
        captured(keybinding_from_input_for_action(
            A::TabSelectByIndex,
            &digit_input("9", false, true, false, false),
            Darwin
        )),
        "Ctrl+1"
    );
    // Extra modifiers survive the digit collapse.
    assert_eq!(
        captured(keybinding_from_input_for_action(
            A::TabSelectByIndex,
            &digit_input("5", false, true, false, true),
            Darwin
        )),
        "Ctrl+Shift+1"
    );
    // A non-number chord for a digit-index action is rejected.
    assert!(!keybinding_from_input_for_action(
        A::TabSelectByIndex,
        &ev("p", "KeyP", true, false, false, false),
        Darwin
    )
    .is_valid());
}

// ===========================================================================
// tab.rename mac-only + context interplay (round-trip / capture edge)
// ===========================================================================

#[test]
fn matches_tab_rename_mac_only() {
    // keybindings.test.ts:410-453.
    assert!(matches(
        A::TabRename,
        &ev("r", "KeyR", true, false, false, false),
        Darwin
    ));
    // linux has no default binding.
    assert!(!matches(
        A::TabRename,
        &ev("r", "KeyR", false, true, false, false),
        Linux
    ));
}

#[test]
fn matches_new_agent_mac_default() {
    // keybindings.test.ts:839-845.
    assert!(matches(
        A::TabNewAgent,
        &ev("t", "KeyT", true, false, true, false),
        Darwin
    ));
}
