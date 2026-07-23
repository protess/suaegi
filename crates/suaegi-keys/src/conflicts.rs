//! Conflict detection: which actions' *effective* bindings collide, so Settings
//! can refuse an override that would shadow another action.
//!
//! Ported from Orca `src/shared/keybindings.ts`:
//!   - `keybindingConflictIdentityForParsed` (:2043-2058) — reduce a chord to a
//!     platform-resolved identity string (`Mod` -> Cmd/Ctrl per platform).
//!   - `keybindingConflictIdentity` (:2060-2063), `keybindingConflictIdentities`
//!     (:2065-2081) — digit-index chords collapse to nine `1`-`9` identities.
//!   - `findKeybindingConflicts` (:2235-2290), `setIntersects` (:2292-2299).
//!   - `KeybindingConflict` (:188-191), `FindKeybindingConflictOptions` (:193-195).
//!
//! The two subtleties that drive the whole design:
//!   1. **Bucketing.** Two actions collide only if they share a *bucket*, keyed by
//!      `conflict_group.unwrap_or(scope)`. An action *with* a conflict group is
//!      additionally bucketed under its raw scope (Orca `:2253-2257`): a native
//!      menu accelerator (e.g. `Mod+Comma`) can eat a global chord, so a custom
//!      binding is checked against both the `menu` bucket and the scope bucket.
//!   2. **Customized-only reporting.** A collision is reported only if at least
//!      one participating action is *customized* (present in `overrides` and not
//!      ignored). Built-in default collisions the user cannot fix are never
//!      flagged (Orca `:2277`).

use std::collections::{HashMap, HashSet};

use crate::chord::{parse_keybinding, resolve_modifier_token, ParsedKeybinding, PhysicalModifier};
use crate::normalize::is_digit_index_key;
use crate::registry::{
    is_digit_index_action_id, KeybindingActionId, KeybindingPlatform, KEYBINDING_DEFINITIONS,
};
use crate::resolve::{
    get_effective_keybindings_for_action, platform_modifiers, KeybindingOverrides,
};

/// One reported conflict: a representative `binding` string and the set of
/// actions that collide on it (insertion-ordered — the order actions are
/// encountered while scanning [`KEYBINDING_DEFINITIONS`]). Mirror of Orca
/// `KeybindingConflict` (`keybindings.ts:188-191`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeybindingConflict {
    pub binding: String,
    pub action_ids: Vec<KeybindingActionId>,
}

/// Options for [`find_keybinding_conflicts_with_options`]. Mirror of Orca
/// `FindKeybindingConflictOptions` (`keybindings.ts:193-195`). `ignored_action_ids`
/// drops actions from the scan entirely (Orca uses this to exclude the per-agent
/// tab family — plan F2 — from cross-checks while one agent is being edited).
#[derive(Debug, Clone, Copy, Default)]
pub struct FindKeybindingConflictOptions<'a> {
    pub ignored_action_ids: &'a [KeybindingActionId],
}

/// The `resolveModifierToken` string form used in a double-tap identity. Orca
/// interpolates the resolved token (`'Meta'`/`'Control'`/`'Alt'`/`'Shift'`)
/// directly (`keybindings.ts:2048`).
fn physical_modifier_str(modifier: PhysicalModifier) -> &'static str {
    match modifier {
        PhysicalModifier::Meta => "Meta",
        PhysicalModifier::Control => "Control",
        PhysicalModifier::Alt => "Alt",
        PhysicalModifier::Shift => "Shift",
    }
}

/// Reduce a parsed chord to its platform-specific identity string. Mirror of Orca
/// `keybindingConflictIdentityForParsed` (`keybindings.ts:2043-2058`).
///
/// The `Mod` resolution is the crux: [`platform_modifiers`] resolves the virtual
/// `Mod` to `Meta` on darwin / `Control` elsewhere — the *same* resolution the
/// matcher uses — so `Mod+P` and a `Cmd+P` override share the identity
/// `Meta+++P` on darwin and thus collide, but on linux `Mod+P` (-> Control) and
/// `Cmd+P` (-> Meta) do not.
fn conflict_identity_for_parsed(parsed: &ParsedKeybinding, platform: KeybindingPlatform) -> String {
    if let Some(double_tap) = parsed.double_tap_modifier {
        return format!(
            "DoubleTap:{}",
            physical_modifier_str(resolve_modifier_token(double_tap, platform))
        );
    }
    let modifiers = platform_modifiers(parsed, platform);
    [
        if modifiers.meta { "Meta" } else { "" },
        if modifiers.control { "Control" } else { "" },
        if modifiers.alt { "Alt" } else { "" },
        if modifiers.shift { "Shift" } else { "" },
        parsed.key.as_str(),
    ]
    .join("+")
}

/// The identity of a binding string, falling back to the raw string when it does
/// not parse. Mirror of Orca `keybindingConflictIdentity` (`keybindings.ts:2060-2063`).
fn conflict_identity(binding: &str, platform: KeybindingPlatform) -> String {
    match parse_keybinding(binding) {
        Ok(parsed) => conflict_identity_for_parsed(&parsed, platform),
        Err(_) => binding.to_string(),
    }
}

/// All identities a binding claims within a bucket. Mirror of Orca
/// `keybindingConflictIdentities` (`keybindings.ts:2065-2081`). A digit-index
/// action's representative `1`-`9` chord **collapses** to all nine identities
/// (`Mod+1`..`Mod+9`), so any `Mod+<digit>` from another action in the bucket
/// collides with it.
fn conflict_identities(
    action: KeybindingActionId,
    binding: &str,
    platform: KeybindingPlatform,
) -> Vec<String> {
    let exact = conflict_identity(binding, platform);
    if !is_digit_index_action_id(action) {
        return vec![exact];
    }
    let Ok(parsed) = parse_keybinding(binding) else {
        return vec![exact];
    };
    if parsed.double_tap_modifier.is_some() || !is_digit_index_key(&parsed.key) {
        return vec![exact];
    }
    (1..=9)
        .map(|index| {
            let mut candidate = parsed.clone();
            candidate.key = index.to_string();
            conflict_identity_for_parsed(&candidate, platform)
        })
        .collect()
}

/// One bucket+identity's owner: the representative binding and the actions that
/// claim it, in encounter order. Mirror of the anonymous
/// `{ binding, actionIds: Set }` object in Orca `findKeybindingConflicts`.
struct Owner {
    binding: String,
    action_ids: Vec<KeybindingActionId>,
}

impl Owner {
    /// Add an action to the ordered set (Orca `Set.add` — insertion order, no dup).
    fn add_action(&mut self, action: KeybindingActionId) {
        if !self.action_ids.contains(&action) {
            self.action_ids.push(action);
        }
    }
}

/// Find every keybinding conflict for `platform` under `overrides`. Mirror of Orca
/// `findKeybindingConflicts` (`keybindings.ts:2235-2290`) with default options.
///
/// Reuses [`get_effective_keybindings_for_action`] for each action's effective
/// bindings (no second effective-bindings implementation), buckets by
/// `conflict_group.unwrap_or(scope)` (plus the raw scope when a conflict group is
/// present), reduces each binding to its platform-resolved identity, and reports
/// an identity shared by >=2 actions **only if** at least one is customized.
pub fn find_keybinding_conflicts(
    platform: KeybindingPlatform,
    overrides: Option<&KeybindingOverrides>,
) -> Vec<KeybindingConflict> {
    find_keybinding_conflicts_with_options(
        platform,
        overrides,
        &FindKeybindingConflictOptions::default(),
    )
}

/// [`find_keybinding_conflicts`] with an ignore list. Mirror of Orca
/// `findKeybindingConflicts` (`keybindings.ts:2235-2290`).
pub fn find_keybinding_conflicts_with_options(
    platform: KeybindingPlatform,
    overrides: Option<&KeybindingOverrides>,
    options: &FindKeybindingConflictOptions,
) -> Vec<KeybindingConflict> {
    let is_ignored = |action: KeybindingActionId| options.ignored_action_ids.contains(&action);

    // customizedActions: override keys, minus ignored. (In Rust the keys are
    // already typed `KeybindingActionId`, so Orca's `isKeybindingActionId` guard
    // at `:2244` is automatic.)
    let customized: HashSet<KeybindingActionId> = overrides
        .map(|map| map.keys().copied().filter(|id| !is_ignored(*id)).collect())
        .unwrap_or_default();

    // Insertion-ordered map: `order` preserves the sequence conflict keys are
    // first seen (JS `Map` iteration order), so the reported conflict order and
    // `action_ids` order match Orca exactly.
    let mut order: Vec<String> = Vec::new();
    let mut owners: HashMap<String, Owner> = HashMap::new();

    for definition in KEYBINDING_DEFINITIONS {
        if is_ignored(definition.id) {
            continue;
        }
        for binding in get_effective_keybindings_for_action(definition.id, platform, overrides) {
            // Buckets: conflict_group ?? scope, and additionally the scope when a
            // conflict group is set (Orca `:2253-2257`, deduped like a JS `Set`).
            let scope_bucket = definition.scope.as_bucket_str();
            let mut groups: Vec<&str> = Vec::new();
            match definition.conflict_group {
                Some(conflict_group) => {
                    groups.push(conflict_group);
                    if conflict_group != scope_bucket {
                        groups.push(scope_bucket);
                    }
                }
                None => groups.push(scope_bucket),
            }

            for group in &groups {
                for identity in conflict_identities(definition.id, &binding, platform) {
                    let conflict_key = format!("{group}\u{0}{identity}");
                    if !owners.contains_key(&conflict_key) {
                        order.push(conflict_key.clone());
                        owners.insert(
                            conflict_key.clone(),
                            Owner {
                                binding: binding.clone(),
                                action_ids: Vec::new(),
                            },
                        );
                    }
                    let owner = owners.get_mut(&conflict_key).expect("just inserted");
                    // Prefer a non-digit-index binding as the representative when
                    // this identity is shared with a digit-index row, so the
                    // report shows e.g. the concrete `Mod+3` rather than the
                    // digit-index representative `Mod+1` (Orca `:2262-2267`).
                    if !is_digit_index_action_id(definition.id)
                        && owner
                            .action_ids
                            .iter()
                            .any(|id| is_digit_index_action_id(*id))
                    {
                        owner.binding = binding.clone();
                    }
                    owner.add_action(definition.id);
                }
            }
        }
    }

    // Keep buckets shared by >1 action where at least one is customized; dedup
    // identical (binding, action_ids) rows produced by multiple buckets/identities
    // (Orca `:2276-2289`).
    let mut seen: HashSet<String> = HashSet::new();
    let mut conflicts: Vec<KeybindingConflict> = Vec::new();
    for key in &order {
        let owner = &owners[key];
        if owner.action_ids.len() <= 1 {
            continue;
        }
        if !owner.action_ids.iter().any(|id| customized.contains(id)) {
            continue;
        }
        let dedup_key = format!(
            "{}\u{0}{}",
            owner.binding,
            owner
                .action_ids
                .iter()
                .map(|id| id.as_str())
                .collect::<Vec<_>>()
                .join("\u{0}")
        );
        if seen.insert(dedup_key) {
            conflicts.push(KeybindingConflict {
                binding: owner.binding.clone(),
                action_ids: owner.action_ids.clone(),
            });
        }
    }
    conflicts
}

#[cfg(test)]
mod tests;
