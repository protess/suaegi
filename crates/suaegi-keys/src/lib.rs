//! `suaegi-keys` — the keybinding registry, chord parser/canonicalizer, and
//! formatter, ported from Orca's `src/shared/keybindings.ts` (@ v1.4.150-rc.0).
//!
//! This is a **leaf crate**: it depends on nothing else in the workspace so the
//! whole layer stays a pure `String` <-> struct transform that can be
//! mutation-verified in isolation (repo hard rule).
//!
//! Milestones landed so far:
//!   - M1: the registry (`KeybindingActionId`, `KeybindingDefinition`, the 84
//!     defs), the chord grammar (parse / canonicalize / double-tap / `Mod`
//!     resolution), and the formatter (glyphs on darwin, text elsewhere).
//!   - M2: normalize/validate (`normalize_keybinding*`, `KeybindingValidationResult`)
//!     plus the digit-index (`1`-`9` -> `1`) canonicalization and per-action
//!     bare/shift-only rules.
//!   - M3: the event->action resolver (`keybinding_from_input*`,
//!     `keybinding_matches_input`, `keybinding_matches_action`,
//!     `match_keybinding_digit_index`) with the macOS Option-compose, non-Latin /
//!     AltGr physical-code fallbacks, and terminal-shortcut policy.
//!   - M4: conflict detection (`find_keybinding_conflicts`,
//!     `KeybindingConflict`) — reduces each action's effective bindings to a
//!     platform-resolved identity, buckets by conflict-group/scope, and reports
//!     collisions only when a customized action participates.
//!   - M5: the on-disk file layer (`read_keybinding_file`,
//!     `write_keybinding_override`, `KeybindingFileSnapshot`, `Diagnostic`) —
//!     parses a `keybindings.json` (tolerating the legacy flat root), drops
//!     conflicting overrides via a bounded fixpoint, and writes a single
//!     override back atomically (own `tempfile` temp+rename, F3) into the active
//!     platform section only. The file *path* is always injected by the caller;
//!     this crate never resolves a config dir (that stays in M6).
//!
//! The templated
//! `tab.newAgent.${agent}` family (Orca `keybindings.ts:26,1059`) is intentionally
//! **excluded** here (see F2 in the plan) and gets wired at the app boundary in M6.

mod chord;
mod conflicts;
mod file;
mod format;
mod normalize;
mod registry;
mod resolve;

pub use chord::{
    canonicalize_parsed_keybinding, is_double_tap_binding, normalize_key_token, parse_keybinding,
    parse_modifier_token, resolve_modifier_token, ModifierToken, ParseError, ParsedKeybinding,
    PhysicalModifier,
};
pub use conflicts::{
    find_keybinding_conflicts, find_keybinding_conflicts_with_options,
    FindKeybindingConflictOptions, KeybindingConflict,
};
pub use file::{
    read_keybinding_file, write_keybinding_override, Diagnostic, KeybindingFileSnapshot, Severity,
    WriteError,
};
pub use format::{format_keybinding, format_keybinding_list};
pub use normalize::{
    normalize_keybinding, normalize_keybinding_array_for_action, normalize_keybinding_list,
    normalize_keybinding_list_for_action, InvalidReason, KeybindingListResult,
    KeybindingValidationResult,
};
pub use registry::{
    is_digit_index_action_id, KeybindingActionId, KeybindingDefinition, KeybindingPlatform,
    PerPlatform, Scope, DIGIT_INDEX_ACTION_IDS, KEYBINDING_DEFINITIONS,
};
pub use resolve::{
    get_effective_keybindings_for_action, keybinding_from_input, keybinding_from_input_for_action,
    keybinding_matches_action, keybinding_matches_input, match_keybinding_digit_index,
    KeybindingContext, KeybindingInput, KeybindingMatchOptions, KeybindingOverrides,
    TerminalShortcutPolicy,
};
