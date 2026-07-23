//! `suaegi-keys` — the keybinding registry, chord parser/canonicalizer, and
//! formatter, ported from Orca's `src/shared/keybindings.ts` (@ v1.4.150-rc.0).
//!
//! This is a **leaf crate**: it depends on nothing else in the workspace so the
//! whole layer stays a pure `String` <-> struct transform that can be
//! mutation-verified in isolation (repo hard rule).
//!
//! Milestone M1 covers:
//!   - the registry (`KeybindingActionId`, `KeybindingDefinition`, the 84 defs),
//!   - the chord grammar (parse / canonicalize / double-tap / `Mod` resolution),
//!   - the formatter (glyphs on darwin, text elsewhere).
//!
//! Normalize/validate (M2), the event->action resolver (M3), conflict detection
//! (M4), and the on-disk file layer (M5) land in later milestones. The templated
//! `tab.newAgent.${agent}` family (Orca `keybindings.ts:26,1059`) is intentionally
//! **excluded** here (see F2 in the plan) and gets wired at the app boundary in M6.

mod chord;
mod format;
mod registry;

pub use chord::{
    canonicalize_parsed_keybinding, is_double_tap_binding, normalize_key_token, parse_keybinding,
    parse_modifier_token, resolve_modifier_token, ModifierToken, ParseError, ParsedKeybinding,
    PhysicalModifier,
};
pub use format::{format_keybinding, format_keybinding_list};
pub use registry::{
    KeybindingActionId, KeybindingDefinition, KeybindingPlatform, PerPlatform, Scope,
    DIGIT_INDEX_ACTION_IDS, KEYBINDING_DEFINITIONS,
};
