//! Conflict-detection tests. Ported from the `findKeybindingConflicts` vectors
//! in Orca `keybindings.test.ts` (the `keybinding resolution` + `digit-index
//! shortcuts` describe blocks), plus crux tests that pin each enumerable branch
//! (bucketing, `Mod`-resolved identity, the customized-only rule, digit-index
//! collapse, and a real-registry-bucket sample).

use super::*;
use crate::registry::KeybindingActionId as A;
use crate::registry::KeybindingPlatform::{Darwin, Linux, Win32};

/// Build a [`KeybindingOverrides`] from `(action, [chords])` pairs.
fn overrides(pairs: &[(KeybindingActionId, &[&str])]) -> KeybindingOverrides {
    pairs
        .iter()
        .map(|(action, chords)| (*action, chords.iter().map(|s| s.to_string()).collect()))
        .collect()
}

/// The single conflict whose `action_ids` (as a set) equals `expected`, if any.
fn find_conflict_on<'a>(
    conflicts: &'a [KeybindingConflict],
    binding: &str,
    expected: &[KeybindingActionId],
) -> Option<&'a KeybindingConflict> {
    let want: HashSet<KeybindingActionId> = expected.iter().copied().collect();
    conflicts.iter().find(|c| {
        c.binding == binding && c.action_ids.iter().copied().collect::<HashSet<_>>() == want
    })
}

/// Assert a conflict on `binding` between exactly `expected` (order-insensitive)
/// is present — the Rust analogue of `toContainEqual` + `arrayContaining`.
fn assert_contains_conflict(
    conflicts: &[KeybindingConflict],
    binding: &str,
    expected: &[KeybindingActionId],
) {
    assert!(
        find_conflict_on(conflicts, binding, expected).is_some(),
        "expected a conflict on {binding:?} between {expected:?}, got {conflicts:#?}"
    );
}

// --- Ported TS oracle vectors ------------------------------------------------

// keybindings.test.ts:289-298 — no conflicts by default; a custom `Mod+P` on
// view.tasks collides with worktree.quickOpen (both global scope).
#[test]
fn reports_conflicts_across_default_and_customized_actions() {
    assert_eq!(find_keybinding_conflicts(Linux, None), vec![]);

    let ov = overrides(&[(A::ViewTasks, &["Mod+P"])]);
    let conflicts = find_keybinding_conflicts(Linux, Some(&ov));
    assert_contains_conflict(&conflicts, "Mod+P", &[A::WorktreeQuickOpen, A::ViewTasks]);
}

// keybindings.test.ts:329-334 — customizing sidebar.focusWorktreeList onto Mod+0
// collides with zoom.reset (#8584).
#[test]
fn focus_worktree_list_on_mod_0_conflicts_with_zoom_reset() {
    let ov = overrides(&[(A::SidebarFocusWorktreeList, &["Mod+0"])]);
    let conflicts = find_keybinding_conflicts(Darwin, Some(&ov));
    assert_contains_conflict(
        &conflicts,
        "Mod+0",
        &[A::ZoomReset, A::SidebarFocusWorktreeList],
    );
}

// keybindings.test.ts:337-400 — the quick-commands menu (conflict_group "global")
// conflicts with global shortcuts AND the digit-index ranges, on every platform.
#[test]
fn quick_command_menu_conflicts_with_global_and_digit_ranges() {
    // Menu (Mod+P) vs worktree.quickOpen (Mod+P) — same "global" bucket.
    for chord in ["Mod+P", "Cmd+P"] {
        let ov = overrides(&[(A::TabOpenQuickCommandsMenu, &[chord])]);
        let conflicts = find_keybinding_conflicts(Darwin, Some(&ov));
        assert_contains_conflict(
            &conflicts,
            "Mod+P",
            &[A::WorktreeQuickOpen, A::TabOpenQuickCommandsMenu],
        );
    }
    // On linux, Ctrl+P resolves to the same identity as the Mod+P default.
    let ov = overrides(&[(A::TabOpenQuickCommandsMenu, &["Ctrl+P"])]);
    let conflicts = find_keybinding_conflicts(Linux, Some(&ov));
    assert_contains_conflict(
        &conflicts,
        "Mod+P",
        &[A::WorktreeQuickOpen, A::TabOpenQuickCommandsMenu],
    );

    // Menu vs workspace.selectByIndex (digit-index, "global" scope): the menu's
    // concrete Mod+3 collapses onto the range, and the reported binding is the
    // concrete chord, not the Mod+1 representative.
    for (platform, chord) in [(Darwin, "Mod+3"), (Darwin, "Cmd+3"), (Linux, "Ctrl+3")] {
        let ov = overrides(&[(A::TabOpenQuickCommandsMenu, &[chord])]);
        let conflicts = find_keybinding_conflicts(platform, Some(&ov));
        assert_contains_conflict(
            &conflicts,
            chord,
            &[A::WorkspaceSelectByIndex, A::TabOpenQuickCommandsMenu],
        );
    }

    // On linux, the menu's Alt+4 collides with tab.selectByIndex (default Alt+1).
    let ov = overrides(&[(A::TabOpenQuickCommandsMenu, &["Alt+4"])]);
    let conflicts = find_keybinding_conflicts(Linux, Some(&ov));
    assert_contains_conflict(
        &conflicts,
        "Alt+4",
        &[A::TabSelectByIndex, A::TabOpenQuickCommandsMenu],
    );
}

// keybindings.test.ts:458-472 — tab.rename (Mod+R) shares its chord with
// browser.reload but they are in different scopes, so customizing tab.rename to
// its default is conflict-free; but workspace.rename (conflict_group
// "workspace-shell") DOES collide with tab.rename.
#[test]
fn macos_rename_shortcuts_bucket_by_shared_group_not_scope() {
    // tab.rename shares Mod+R with browser.reload — different scopes (tabs vs
    // browser), so no conflict even when customized to its default.
    let ov = overrides(&[(A::TabRename, &["Mod+R"])]);
    assert_eq!(find_keybinding_conflicts(Darwin, Some(&ov)), vec![]);

    // workspace.rename customized to Mod+R collides with tab.rename via the
    // shared "workspace-shell" conflict group. Exact match, exact order.
    let ov = overrides(&[(A::WorkspaceRename, &["Mod+R"])]);
    assert_eq!(
        find_keybinding_conflicts(Darwin, Some(&ov)),
        vec![KeybindingConflict {
            binding: "Mod+R".to_string(),
            action_ids: vec![A::WorkspaceRename, A::TabRename],
        }]
    );

    // Customizing tab.rename onto workspace.rename's default Mod+Alt+R collides
    // the other way; encounter order still lists workspace.rename first.
    let ov = overrides(&[(A::TabRename, &["Mod+Alt+R"])]);
    assert_eq!(
        find_keybinding_conflicts(Darwin, Some(&ov)),
        vec![KeybindingConflict {
            binding: "Mod+Alt+R".to_string(),
            action_ids: vec![A::WorkspaceRename, A::TabRename],
        }]
    );
}

// keybindings.test.ts:610-613 — rebinding tab.closeAll onto Mod+W conflicts with
// tab.close (both Tabs scope).
#[test]
fn close_all_on_mod_w_conflicts_with_close() {
    let ov = overrides(&[(A::TabCloseAll, &["Mod+W"])]);
    let conflicts = find_keybinding_conflicts(Darwin, Some(&ov));
    assert_contains_conflict(&conflicts, "Mod+W", &[A::TabClose, A::TabCloseAll]);
}

// keybindings.test.ts:917-928 — a customized renderer binding collides with a
// native menu accelerator's chord: worktree.palette onto Mod+Shift+E collides
// with sidebar.explorer.toggle.
#[test]
fn customized_renderer_conflicts_with_menu_accelerator() {
    assert_eq!(find_keybinding_conflicts(Darwin, None), vec![]);

    let ov = overrides(&[(A::WorktreePalette, &["Mod+Shift+E"])]);
    let conflicts = find_keybinding_conflicts(Darwin, Some(&ov));
    assert_contains_conflict(
        &conflicts,
        "Mod+Shift+E",
        &[A::SidebarExplorerToggle, A::WorktreePalette],
    );
}

// keybindings.test.ts:904-914 — ignored actions are dropped from the scan. Ported
// with real actions (the agent-tab family is excluded from this crate, plan F2):
// two actions customized to the same chord in the same bucket conflict, but
// ignoring one removes the conflict entirely.
#[test]
fn ignores_selected_actions_when_checking_conflicts() {
    let ov = overrides(&[
        (A::ViewTasks, &["Mod+Alt+Shift+K"]),
        (A::WorkspaceOpenBoard, &["Mod+Alt+Shift+K"]),
    ]);
    // Both are global scope with the same chord -> a conflict without ignores.
    let conflicts = find_keybinding_conflicts(Darwin, Some(&ov));
    assert_contains_conflict(
        &conflicts,
        "Mod+Alt+Shift+K",
        &[A::ViewTasks, A::WorkspaceOpenBoard],
    );

    // Ignoring one participant removes it from both the scan and the customized
    // set, so nothing is reported.
    let options = FindKeybindingConflictOptions {
        ignored_action_ids: &[A::WorkspaceOpenBoard],
    };
    assert_eq!(
        find_keybinding_conflicts_with_options(Darwin, Some(&ov), &options),
        vec![]
    );
}

// keybindings.test.ts:1484-1495 — two actions sharing DoubleTap+Shift collide.
#[test]
fn reports_conflicts_across_two_double_tap_bindings() {
    let ov = overrides(&[
        (A::WorktreeQuickOpen, &["DoubleTap+Shift"]),
        (A::ViewTasks, &["DoubleTap+Shift"]),
    ]);
    let conflicts = find_keybinding_conflicts(Darwin, Some(&ov));
    assert_contains_conflict(
        &conflicts,
        "DoubleTap+Shift",
        &[A::WorktreeQuickOpen, A::ViewTasks],
    );
}

// keybindings.test.ts:1497-1517 — platform-primary double-tap aliases resolve to
// the same identity: DoubleTap+Mod vs DoubleTap+Cmd collide on darwin;
// DoubleTap+Mod vs DoubleTap+Ctrl collide on linux.
#[test]
fn reports_conflicts_across_platform_primary_double_tap_aliases() {
    let ov = overrides(&[
        (A::WorktreeQuickOpen, &["DoubleTap+Mod"]),
        (A::ViewTasks, &["DoubleTap+Cmd"]),
    ]);
    let conflicts = find_keybinding_conflicts(Darwin, Some(&ov));
    assert_contains_conflict(
        &conflicts,
        "DoubleTap+Mod",
        &[A::WorktreeQuickOpen, A::ViewTasks],
    );

    let ov = overrides(&[
        (A::WorktreeQuickOpen, &["DoubleTap+Mod"]),
        (A::ViewTasks, &["DoubleTap+Ctrl"]),
    ]);
    let conflicts = find_keybinding_conflicts(Linux, Some(&ov));
    assert_contains_conflict(
        &conflicts,
        "DoubleTap+Mod",
        &[A::WorktreeQuickOpen, A::ViewTasks],
    );
}

// keybindings.test.ts:1519-1530 — one action listing both double-tap aliases for
// ITSELF is not a self-conflict (the actionIds set stays size 1).
#[test]
fn no_conflict_when_one_action_lists_double_tap_aliases_for_itself() {
    let ov = overrides(&[(A::WorktreeQuickOpen, &["DoubleTap+Mod", "DoubleTap+Cmd"])]);
    assert_eq!(find_keybinding_conflicts(Darwin, Some(&ov)), vec![]);

    let ov = overrides(&[(A::WorktreeQuickOpen, &["DoubleTap+Mod", "DoubleTap+Ctrl"])]);
    assert_eq!(find_keybinding_conflicts(Linux, Some(&ov)), vec![]);
}

// keybindings.test.ts:1711-1720 — the two digit-index ranges can swap modifiers
// (tab -> Cmd, workspace -> Ctrl) without a false conflict: different scopes.
#[test]
fn digit_index_ranges_swap_modifiers_without_false_conflict() {
    let ov = overrides(&[
        (A::TabSelectByIndex, &["Mod+1"]),
        (A::WorkspaceSelectByIndex, &["Ctrl+1"]),
    ]);
    assert_eq!(find_keybinding_conflicts(Darwin, Some(&ov)), vec![]);
}

// --- Crux tests: each pins one enumerable branch -----------------------------

// Crux (bucketing): two actions with the SAME chord but DIFFERENT buckets do NOT
// conflict; the SAME bucket DOES. Mutating the bucket key (e.g. dropping the
// conflict_group/scope distinction) flips one of these.
#[test]
fn crux_bucketing_same_chord_different_bucket_does_not_conflict() {
    // browser.find (Browser scope) and editor.find (Editor scope) both default to
    // Mod+F. Customizing both to Mod+F must NOT conflict — different buckets.
    let ov = overrides(&[(A::BrowserFind, &["Mod+F"]), (A::EditorFind, &["Mod+F"])]);
    let conflicts = find_keybinding_conflicts(Darwin, Some(&ov));
    assert!(
        find_conflict_on(&conflicts, "Mod+F", &[A::BrowserFind, A::EditorFind]).is_none(),
        "different scopes must not conflict: {conflicts:#?}"
    );

    // Two Editor-scope actions on the same chord DO conflict (same bucket).
    let ov = overrides(&[
        (A::EditorFind, &["Mod+Shift+Y"]),
        (A::EditorSave, &["Mod+Shift+Y"]),
    ]);
    let conflicts = find_keybinding_conflicts(Darwin, Some(&ov));
    assert_contains_conflict(&conflicts, "Mod+Shift+Y", &[A::EditorFind, A::EditorSave]);
}

// Crux (Mod identity): Mod+P (a default) vs a Cmd+P override collide on darwin
// (both resolve to Meta) but NOT on linux (Mod -> Control, Cmd -> Meta). This
// pins the platform-resolution inside the identity function.
#[test]
fn crux_mod_identity_is_platform_resolved() {
    // darwin: worktree.quickOpen default Mod+P vs view.tasks override Cmd+P.
    let ov = overrides(&[(A::ViewTasks, &["Cmd+P"])]);
    let conflicts = find_keybinding_conflicts(Darwin, Some(&ov));
    assert_contains_conflict(&conflicts, "Mod+P", &[A::WorktreeQuickOpen, A::ViewTasks]);

    // linux: Cmd+P (Meta) and Mod+P (Control) are distinct identities -> no
    // conflict between the same two actions.
    let conflicts = find_keybinding_conflicts(Linux, Some(&ov));
    assert!(
        find_conflict_on(&conflicts, "Mod+P", &[A::WorktreeQuickOpen, A::ViewTasks]).is_none()
            && find_conflict_on(&conflicts, "Cmd+P", &[A::WorktreeQuickOpen, A::ViewTasks])
                .is_none(),
        "Cmd+P must not collide with Mod+P on linux: {conflicts:#?}"
    );
}

// Crux (customized-only rule). A collision is reported ONLY when a customized
// action participates.
//
// NOTE on the "always report" (predicate-removed) mutant: it is an *equivalent
// mutant* on this registry. The ported registry — like Orca's — has ZERO
// built-in same-bucket collisions on any platform (verified: mapping every
// action to its own effective default surfaces 0 conflicts on darwin/linux/
// win32). Since built-in bindings never change, no multi-action bucket can exist
// without a customized member, so removing the `customized` filter is
// unobservable through the public API. We therefore pin the two *killable*
// directions of the predicate:
//   - INVERSION (`report iff NONE customized`): killed here — a customized
//     collision must be reported, and every reported conflict must intersect the
//     customized set.
//   - THRESHOLD (`size > 1` -> `>= 1`): killed by the exact `assert_eq!` in
//     `macos_rename_shortcuts_bucket_by_shared_group_not_scope` (a size-1 bucket
//     containing the customized action would leak in as a spurious conflict).
// A regression that *introduces* a built-in collision is caught by
// `default_registry_is_conflict_free_on_all_platforms` below.
#[test]
fn crux_customized_only_rule() {
    // A customized action colliding with a built-in in the same bucket IS
    // reported (view.tasks customized onto sidebar.left.toggle's Mod+B, both
    // Global). Inversion would drop this.
    let ov = overrides(&[(A::ViewTasks, &["Mod+B"])]);
    let conflicts = find_keybinding_conflicts(Darwin, Some(&ov));
    assert_contains_conflict(&conflicts, "Mod+B", &[A::SidebarLeftToggle, A::ViewTasks]);

    // The predicate, encoded directly: EVERY reported conflict intersects the
    // customized set. Inverting the predicate would produce reports that do NOT
    // intersect it (or none at all), failing this.
    let customized = [A::ViewTasks];
    assert!(
        !conflicts.is_empty()
            && conflicts
                .iter()
                .all(|c| { c.action_ids.iter().any(|id| customized.contains(id)) }),
        "every conflict must involve a customized action: {conflicts:#?}"
    );
}

// The default (no-override) registry is conflict-free on every platform. This is
// both the Orca oracle (`findKeybindingConflicts('linux'|'darwin')` -> []) and
// the guard that a future registry edit introducing a built-in same-bucket
// collision fails loudly (which would also make the customized-only mutant
// killable again).
#[test]
fn default_registry_is_conflict_free_on_all_platforms() {
    assert_eq!(find_keybinding_conflicts(Darwin, None), vec![]);
    assert_eq!(find_keybinding_conflicts(Linux, None), vec![]);
    assert_eq!(find_keybinding_conflicts(Win32, None), vec![]);
}

// Crux (digit-index collapse): two digit-index actions colliding via the Mod+1..9
// identity are reported once. Put both ranges in the SAME bucket by giving them
// the same modifier, then confirm a single conflict. Mutating the collapse (so a
// digit-index chord claims only its literal `1` identity) would drop this when the
// two ranges use different literal digits.
#[test]
fn crux_digit_index_collapse_same_bucket() {
    // Both ranges customized into the "global" bucket via tab.openQuickCommandsMenu
    // is covered above; here pin the collapse directly: workspace.selectByIndex
    // default Mod+1 (Global) vs a menu override Mod+9 (conflict_group global).
    // Without the 1..9 collapse, Mod+9 (identity Meta..9) would not match the
    // range's literal Mod+1 (identity Meta..1).
    let ov = overrides(&[(A::TabOpenQuickCommandsMenu, &["Mod+9"])]);
    let conflicts = find_keybinding_conflicts(Darwin, Some(&ov));
    assert_contains_conflict(
        &conflicts,
        "Mod+9",
        &[A::WorkspaceSelectByIndex, A::TabOpenQuickCommandsMenu],
    );
    // Exactly one conflict entry for this pair (collapse must not double-report
    // across the nine identities).
    let matches: Vec<_> = conflicts
        .iter()
        .filter(|c| c.action_ids.contains(&A::WorkspaceSelectByIndex))
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "digit-index collapse reported {matches:#?}"
    );
}

// Registry-bucket coverage: exercise conflict detection across a representative
// SAMPLE of REAL registry buckets (not just synthetic overrides), so a registry
// scope/conflict_group change is caught. For each sampled action we customize a
// SECOND action (sharing that action's real bucket) onto the first's real default
// binding and assert the resulting conflict — coupling the test to the live
// scope/conflict_group of both rows.
#[test]
fn registry_bucket_sample_is_pinned() {
    // (default-owner, its real default chord on darwin, a bucket-mate to customize)
    // The bucket-mate genuinely shares the owner's bucket in the live registry.
    struct Case {
        owner: KeybindingActionId,
        chord: &'static str,
        mate: KeybindingActionId,
    }
    let cases = [
        // Global scope: worktree.quickOpen (Mod+P) <- view.tasks (Global).
        Case {
            owner: A::WorktreeQuickOpen,
            chord: "Mod+P",
            mate: A::ViewTasks,
        },
        // Tabs scope: tab.close (Mod+W) <- tab.closeAll (Tabs).
        Case {
            owner: A::TabClose,
            chord: "Mod+W",
            mate: A::TabCloseAll,
        },
        // Terminal scope: terminal.clear (Mod+K) <- terminal.setTitle (Terminal).
        Case {
            owner: A::TerminalClear,
            chord: "Mod+K",
            mate: A::TerminalSetTitle,
        },
        // Editor scope: editor.save (Mod+S) <- editor.addReviewNote (Editor).
        Case {
            owner: A::EditorSave,
            chord: "Mod+S",
            mate: A::EditorAddReviewNote,
        },
        // Browser scope: browser.find (Mod+F) <- browser.grabElement (Browser).
        Case {
            owner: A::BrowserFind,
            chord: "Mod+F",
            mate: A::BrowserGrabElement,
        },
        // "workspace-shell" conflict group: workspace.rename (Mod+Alt+R) <-
        // tab.rename (shares the workspace-shell group, different scope).
        Case {
            owner: A::WorkspaceRename,
            chord: "Mod+Alt+R",
            mate: A::TabRename,
        },
    ];
    for case in cases {
        let ov = overrides(&[(case.mate, &[case.chord])]);
        let conflicts = find_keybinding_conflicts(Darwin, Some(&ov));
        assert_contains_conflict(&conflicts, case.chord, &[case.owner, case.mate]);
    }

    // Cross-bucket negative from real scopes: browser.find (Browser) and
    // terminal.search (Terminal) both default to Mod+F; customizing terminal.search
    // to Mod+F must NOT conflict with browser.find (different real scopes).
    let ov = overrides(&[(A::TerminalSearch, &["Mod+F"])]);
    let conflicts = find_keybinding_conflicts(Darwin, Some(&ov));
    assert!(
        find_conflict_on(&conflicts, "Mod+F", &[A::BrowserFind, A::TerminalSearch]).is_none(),
        "browser vs terminal Mod+F must not conflict: {conflicts:#?}"
    );
}

// Reuse guard: the conflict module must call the shared effective-bindings
// resolver, not a second copy. (A source-level assertion; grep also enforced in
// review.)
#[test]
fn reuses_shared_effective_bindings_resolver() {
    let src = include_str!("../conflicts.rs");
    assert!(
        src.contains("get_effective_keybindings_for_action"),
        "conflicts.rs must reuse get_effective_keybindings_for_action"
    );
}
