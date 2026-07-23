//! Tests for the file layer. Ports the meaningful vectors from Orca
//! `keybinding-file.test.ts` (the two legacy-migration describe-blocks are
//! intentionally not ported — see the module docs) and adds crux tests that pin
//! each behavior the plan calls out, every one designed to fail under a specific
//! mutation of `file.rs`.

use std::path::{Path, PathBuf};

use serde_json::Value;
use tempfile::tempdir;

use super::*;
use crate::registry::KeybindingActionId as A;
use crate::registry::KeybindingPlatform::{Darwin, Linux};

/// Write `contents` to `<dir>/keybindings.json` and return its path.
fn write_file(dir: &Path, contents: &str) -> PathBuf {
    let path = dir.join("keybindings.json");
    std::fs::write(&path, contents).expect("write fixture");
    path
}

/// Read `path` back as raw JSON for on-disk assertions.
fn read_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("read")).expect("parse")
}

/// An owned `Vec<String>` for the `Option<&[String]>` write argument.
fn bindings(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_string()).collect()
}

fn overrides_of(snapshot: &KeybindingFileSnapshot, action: A) -> Option<&Vec<String>> {
    snapshot.overrides.get(&action)
}

// --- Ported oracle vectors -------------------------------------------------

/// keybinding-file.test.ts:33-40 — a missing file is an empty snapshot, not an
/// error, with no diagnostics.
#[test]
fn missing_file_yields_empty_snapshot() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("keybindings.json");
    let snapshot = read_keybinding_file(&path, Linux);
    assert!(!snapshot.exists);
    assert_eq!(snapshot.platform, Linux);
    assert!(snapshot.overrides.is_empty());
    assert!(snapshot.diagnostics.is_empty());
}

/// keybinding-file.test.ts:42-74 — common + platform-specific overrides merge
/// (active platform wins), a `null` disables an action, and no diagnostics.
#[test]
fn parses_common_and_platform_overrides() {
    let dir = tempdir().unwrap();
    let path = write_file(
        dir.path(),
        r#"{
          "version": 1,
          "keybindings": {
            "worktree.quickOpen": "Mod+Shift+P",
            "view.tasks": null
          },
          "platforms": {
            "linux": {
              "terminal.paste": ["Ctrl+Shift+V", "Shift+Insert"],
              "terminal.search": "Ctrl+Shift+F"
            },
            "darwin": {
              "terminal.search": "Mod+F"
            }
          }
        }"#,
    );

    let snapshot = read_keybinding_file(&path, Linux);
    assert!(snapshot.exists);
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:?}",
        snapshot.diagnostics
    );
    assert_eq!(
        overrides_of(&snapshot, A::WorktreeQuickOpen),
        Some(&bindings(&["Mod+Shift+P"]))
    );
    assert_eq!(overrides_of(&snapshot, A::ViewTasks), Some(&Vec::new()));
    assert_eq!(
        overrides_of(&snapshot, A::TerminalPaste),
        Some(&bindings(&["Ctrl+Shift+V", "Shift+Insert"]))
    );
    // The linux section supplies terminal.search; darwin's Mod+F is ignored.
    assert_eq!(
        overrides_of(&snapshot, A::TerminalSearch),
        Some(&bindings(&["Ctrl+Shift+F"]))
    );
}

/// keybinding-file.test.ts:76-93 — a bare key is accepted for an opt-in action.
#[test]
fn accepts_bare_keys_for_opt_in_actions() {
    let dir = tempdir().unwrap();
    let path = write_file(
        dir.path(),
        r#"{ "keybindings": { "fileExplorer.delete": "Delete" } }"#,
    );
    let snapshot = read_keybinding_file(&path, Linux);
    assert_eq!(
        overrides_of(&snapshot, A::FileExplorerDelete),
        Some(&bindings(&["Delete"]))
    );
    assert!(snapshot.diagnostics.is_empty());
}

/// keybinding-file.test.ts:95-116 — an unknown action, an invalid chord, and a
/// conflicting-with-default chord are all dropped, each with a diagnostic, and
/// the effective overrides end up empty. Order-tolerant (see module docs): we
/// assert the diagnostic *set*, not the sequence.
#[test]
fn ignores_invalid_unknown_and_conflicting_edits() {
    let dir = tempdir().unwrap();
    let path = write_file(
        dir.path(),
        r#"{
          "keybindings": {
            "unknownAction": "Ctrl+Alt+U",
            "terminal.search": "not-a-keybinding",
            "view.tasks": "Mod+P"
          }
        }"#,
    );
    let snapshot = read_keybinding_file(&path, Linux);
    assert!(snapshot.overrides.is_empty(), "{:?}", snapshot.overrides);
    let warnings = snapshot
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .count();
    let errors = snapshot
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    assert_eq!(
        warnings, 1,
        "one unknown-action warning: {:?}",
        snapshot.diagnostics
    );
    assert_eq!(errors, 2, "invalid + conflict: {:?}", snapshot.diagnostics);
}

/// keybinding-file.test.ts:118-144 — writing a linux override preserves the
/// common section and the darwin section verbatim.
#[test]
fn writes_active_platform_preserving_others() {
    let dir = tempdir().unwrap();
    let path = write_file(
        dir.path(),
        r#"{
          "version": 1,
          "keybindings": { "worktree.quickOpen": "Mod+Shift+P" },
          "platforms": { "darwin": { "terminal.search": "Mod+F" } }
        }"#,
    );

    write_keybinding_override(
        &path,
        A::TerminalSearch,
        Some(&bindings(&["Ctrl+Shift+F"])),
        Linux,
    )
    .expect("write");

    let written = read_json(&path);
    // Common section preserved verbatim (still the original string form).
    assert_eq!(written["keybindings"]["worktree.quickOpen"], "Mod+Shift+P");
    // Darwin section untouched.
    assert_eq!(written["platforms"]["darwin"]["terminal.search"], "Mod+F");
    // Linux section got the new override, as an array.
    assert_eq!(
        written["platforms"]["linux"]["terminal.search"],
        Value::Array(vec!["Ctrl+Shift+F".into()])
    );
}

/// keybinding-file.test.ts:146-175 — a legacy flat-root override is folded into
/// the `keybindings` section on the next write, and other platforms survive.
#[test]
fn migrates_flat_root_overrides_on_write() {
    let dir = tempdir().unwrap();
    let path = write_file(
        dir.path(),
        r#"{
          "version": 1,
          "worktree.quickOpen": "Mod+Shift+P",
          "platforms": { "darwin": { "terminal.search": "Mod+F" } }
        }"#,
    );

    write_keybinding_override(
        &path,
        A::TerminalSearch,
        Some(&bindings(&["Ctrl+Shift+F"])),
        Linux,
    )
    .expect("write");

    let written = read_json(&path);
    // The flat-root key is gone from the root...
    assert!(written.get("worktree.quickOpen").is_none());
    // ...and folded into `keybindings` (now in normalized array form).
    assert_eq!(
        written["keybindings"]["worktree.quickOpen"],
        Value::Array(vec!["Mod+Shift+P".into()])
    );
    assert_eq!(written["platforms"]["darwin"]["terminal.search"], "Mod+F");

    let snapshot = read_keybinding_file(&path, Linux);
    assert_eq!(
        overrides_of(&snapshot, A::WorktreeQuickOpen),
        Some(&bindings(&["Mod+Shift+P"]))
    );
    assert_eq!(
        overrides_of(&snapshot, A::TerminalSearch),
        Some(&bindings(&["Ctrl+Shift+F"]))
    );
}

/// keybinding-file.test.ts:177-182 — a write that would collide with another
/// effective shortcut is rejected and nothing is written.
#[test]
fn rejects_conflicting_write() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("keybindings.json");

    // Mod+P is worktree.quickOpen's default (Global); binding view.tasks (Global)
    // to it must be refused.
    let error = write_keybinding_override(&path, A::ViewTasks, Some(&bindings(&["Mod+P"])), Linux)
        .expect_err("must conflict");
    assert!(matches!(error, WriteError::Conflict { .. }), "{error:?}");
    assert!(error
        .to_string()
        .contains("conflicts with another shortcut"));
    // Nothing was written.
    assert!(!path.exists());
    assert!(read_keybinding_file(&path, Linux).overrides.is_empty());
}

/// keybinding-file.test.ts:184-191 (content half) — an unparseable chord is
/// rejected at the boundary; nothing is written. (The "unknown action" / "not an
/// array" string guards are elided by Rust typing — see module docs.)
#[test]
fn rejects_invalid_binding_on_write() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("keybindings.json");

    let error = write_keybinding_override(
        &path,
        A::ViewTasks,
        Some(&bindings(&["not-a-chord"])),
        Linux,
    )
    .expect_err("must reject");
    assert!(
        matches!(error, WriteError::InvalidBinding { .. }),
        "{error:?}"
    );
    assert!(!path.exists());
}

/// keybinding-file.test.ts:193-219 — resetting removes only the active-platform
/// mask; a hand-authored common binding shows back through.
///
/// The linux section carries a SIBLING override (`terminal.paste`) alongside the
/// reset target so the removal is scoped: `active_platform.remove(action)` must
/// drop only `terminal.search` and leave the sibling. A `remove(action)` ->
/// `clear()` mutation (which would wipe the whole active section — a data-loss
/// bug) fails on the sibling-survives assertion below.
#[test]
fn resets_only_the_active_platform() {
    let dir = tempdir().unwrap();
    let path = write_file(
        dir.path(),
        r#"{
          "keybindings": { "terminal.search": "Ctrl+Alt+F" },
          "platforms": {
            "linux": {
              "terminal.search": "Ctrl+Shift+F",
              "terminal.paste": "Ctrl+Alt+V"
            }
          }
        }"#,
    );

    write_keybinding_override(&path, A::TerminalSearch, None, Linux).expect("reset");

    let snapshot = read_keybinding_file(&path, Linux);
    assert_eq!(
        snapshot.common_overrides.get(&A::TerminalSearch),
        Some(&bindings(&["Ctrl+Alt+F"]))
    );
    // The linux section lost ONLY the reset target; the sibling override survives.
    let linux = snapshot
        .platform_overrides
        .get(&Linux)
        .expect("linux section present");
    assert!(
        !linux.contains_key(&A::TerminalSearch),
        "reset target removed: {linux:?}"
    );
    assert_eq!(
        linux.get(&A::TerminalPaste),
        Some(&bindings(&["Ctrl+Alt+V"])),
        "sibling override must survive a scoped reset (not a section-wide clear): {linux:?}"
    );
    // The common binding shows through as the effective override for the reset action.
    assert_eq!(
        overrides_of(&snapshot, A::TerminalSearch),
        Some(&bindings(&["Ctrl+Alt+F"]))
    );
}

/// PIN (#2 — unknown root keys preserved): a write clones the existing document
/// and only re-inserts `version` / `keybindings` / `platforms`, so any unknown
/// root key (a `$schema` pointer, a hand-added field) must survive verbatim. A
/// mutation that rebuilds the document from scratch would drop these.
#[test]
fn write_preserves_unknown_root_keys() {
    let dir = tempdir().unwrap();
    let path = write_file(
        dir.path(),
        r#"{
          "$schema": "https://example.com/keybindings.schema.json",
          "customRootKey": { "nested": [1, 2, 3] },
          "keybindings": { "worktree.quickOpen": "Mod+Shift+P" }
        }"#,
    );

    write_keybinding_override(
        &path,
        A::TerminalSearch,
        Some(&bindings(&["Mod+Shift+F"])),
        Darwin,
    )
    .expect("write");

    let written = read_json(&path);
    assert_eq!(
        written["$schema"],
        "https://example.com/keybindings.schema.json"
    );
    assert_eq!(
        written["customRootKey"],
        serde_json::json!({ "nested": [1, 2, 3] })
    );
    // And the write still landed.
    assert_eq!(
        written["platforms"]["darwin"]["terminal.search"],
        Value::Array(vec!["Mod+Shift+F".into()])
    );
}

/// PIN (#3 — same-platform section merge): writing action B into a platform that
/// already holds action A must keep A. The active section is cloned before the
/// single insertion, not rebuilt — a mutation that starts the active section from
/// an empty map would drop the prior sibling.
#[test]
fn write_preserves_sibling_in_same_platform_section() {
    let dir = tempdir().unwrap();
    let path = write_file(
        dir.path(),
        r#"{
          "platforms": { "darwin": { "terminal.search": "Mod+F" } }
        }"#,
    );

    // Write a DIFFERENT action into the SAME (darwin) section.
    write_keybinding_override(
        &path,
        A::TerminalPaste,
        Some(&bindings(&["Mod+Shift+V"])),
        Darwin,
    )
    .expect("write");

    let written = read_json(&path);
    // The prior sibling survives...
    assert_eq!(written["platforms"]["darwin"]["terminal.search"], "Mod+F");
    // ...alongside the newly written override.
    assert_eq!(
        written["platforms"]["darwin"]["terminal.paste"],
        Value::Array(vec!["Mod+Shift+V".into()])
    );
}

// --- Crux / mutation-targeted tests ----------------------------------------

/// CRUX (active-platform-only write): writing a darwin override touches darwin
/// ONLY — not linux, win32, or the common section. A mutation that writes to the
/// wrong section (e.g. `platform_key` returning a constant, or the mutation
/// writing into `keybindings`) fails here.
#[test]
fn crux_write_touches_only_the_active_platform_section() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("keybindings.json");

    write_keybinding_override(
        &path,
        A::TerminalSearch,
        Some(&bindings(&["Mod+Shift+F"])),
        Darwin,
    )
    .expect("write");

    let written = read_json(&path);
    // Darwin got it.
    assert_eq!(
        written["platforms"]["darwin"]["terminal.search"],
        Value::Array(vec!["Mod+Shift+F".into()])
    );
    // Linux and win32 sections exist but stay empty; common stays empty.
    assert_eq!(written["platforms"]["linux"], serde_json::json!({}));
    assert_eq!(written["platforms"]["win32"], serde_json::json!({}));
    assert_eq!(written["keybindings"], serde_json::json!({}));
}

/// CRUX (atomic write, own temp+rename): the write produces a valid, round-
/// tripping JSON document, creates a missing parent directory, and leaves NO
/// stray temp file behind (proof the sibling temp was renamed, not abandoned).
#[test]
fn crux_atomic_write_roundtrips_and_leaves_no_temp() {
    let dir = tempdir().unwrap();
    // A parent that does not exist yet — the writer must create it.
    let nested = dir.path().join("suaegi");
    let path = nested.join("keybindings.json");

    write_keybinding_override(
        &path,
        A::TerminalSearch,
        Some(&bindings(&["Mod+Shift+F"])),
        Linux,
    )
    .expect("write");

    // Valid JSON that round-trips through the reader.
    let snapshot = read_keybinding_file(&path, Linux);
    assert_eq!(
        overrides_of(&snapshot, A::TerminalSearch),
        Some(&bindings(&["Mod+Shift+F"]))
    );

    // The directory holds exactly the target file — no leftover `*.tmp` sibling.
    let entries: Vec<_> = std::fs::read_dir(&nested)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(entries, vec![std::ffi::OsString::from("keybindings.json")]);
}

/// CRUX (drop-conflicts fixpoint on read): a file whose override collides with a
/// built-in default parses to a snapshot with the override DROPPED plus a
/// diagnostic — a soft degrade, not a hard error. A mutation that skips the drop
/// leaves view.tasks present (and no diagnostic); either fails.
#[test]
fn crux_read_drops_conflicting_override_with_diagnostic() {
    let dir = tempdir().unwrap();
    let path = write_file(
        dir.path(),
        r#"{ "keybindings": { "view.tasks": "Mod+P" } }"#,
    );
    let snapshot = read_keybinding_file(&path, Linux);
    assert!(overrides_of(&snapshot, A::ViewTasks).is_none());
    assert!(
        snapshot
            .diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error
                && d.message
                    .contains("Conflicting custom shortcuts were ignored")),
        "{:?}",
        snapshot.diagnostics
    );
}

/// CRUX (fixpoint terminates): a pathological file where many customizable
/// Global actions all claim the same chord still returns (bounded by the 20-round
/// cap) with every conflicting override dropped — no infinite loop.
#[test]
fn crux_fixpoint_terminates_on_pathological_input() {
    let dir = tempdir().unwrap();
    // Several Global-scope actions all bound to the same chord. worktree.quickOpen
    // keeps its Mod+P default; the four custom ones all collide with it and each
    // other and must all be dropped.
    let path = write_file(
        dir.path(),
        r#"{
          "keybindings": {
            "view.tasks": "Mod+P",
            "sidebar.left.toggle": "Mod+P",
            "worktree.palette": "Mod+P",
            "app.settings": "Mod+P"
          }
        }"#,
    );
    // Reaching this assertion at all proves termination.
    let snapshot = read_keybinding_file(&path, Linux);
    assert!(overrides_of(&snapshot, A::ViewTasks).is_none());
    assert!(overrides_of(&snapshot, A::WorktreePalette).is_none());
    assert!(!snapshot.diagnostics.is_empty());
}

/// CRUX (invalid entry -> diagnostic, never silent): a section with one bad chord
/// and one good one keeps the good one and drops the bad one WITH an error
/// diagnostic naming it. A mutation that silently drops (no diagnostic) or keeps
/// the invalid entry fails here.
#[test]
fn crux_invalid_entry_dropped_with_diagnostic_valid_kept() {
    let dir = tempdir().unwrap();
    let path = write_file(
        dir.path(),
        r#"{
          "keybindings": {
            "terminal.search": "not-a-keybinding",
            "worktree.quickOpen": "Mod+Shift+P"
          }
        }"#,
    );
    let snapshot = read_keybinding_file(&path, Linux);
    // The valid override survives.
    assert_eq!(
        overrides_of(&snapshot, A::WorktreeQuickOpen),
        Some(&bindings(&["Mod+Shift+P"]))
    );
    // The invalid one is gone AND announced.
    assert!(overrides_of(&snapshot, A::TerminalSearch).is_none());
    assert!(
        snapshot
            .diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error
                && d.action_id.as_deref() == Some("terminal.search")),
        "{:?}",
        snapshot.diagnostics
    );
}

/// CRUX (malformed file -> diagnostic, missing -> clean): a present-but-garbage
/// file yields an error diagnostic and an empty snapshot (no panic), while a
/// missing file yields a clean empty snapshot with NO diagnostic. A mutation that
/// turns the malformed case into a clean empty (silent) fails the first half.
#[test]
fn crux_malformed_file_diagnoses_missing_stays_clean() {
    let dir = tempdir().unwrap();
    let path = write_file(dir.path(), "{{{not json");
    let snapshot = read_keybinding_file(&path, Linux);
    assert!(snapshot.exists);
    assert!(snapshot.overrides.is_empty());
    assert_eq!(snapshot.diagnostics.len(), 1);
    assert_eq!(snapshot.diagnostics[0].severity, Severity::Error);
    assert!(snapshot.diagnostics[0]
        .message
        .contains("Could not read keybindings file"));

    // Contrast: a genuinely absent file is not an error.
    let missing = dir.path().join("absent.json");
    let clean = read_keybinding_file(&missing, Linux);
    assert!(!clean.exists);
    assert!(clean.diagnostics.is_empty());
}

/// CRUX (never clobber an unreadable file): a write against a present-but-garbage
/// file is refused (`Unreadable`) and the original bytes are left intact. Mirror
/// of keybinding-file.test.ts:386-394 (the seed-path guard, applied to the write
/// path we DO port).
#[test]
fn crux_write_refuses_to_clobber_unreadable_file() {
    let dir = tempdir().unwrap();
    let garbage = "{{{not json";
    let path = write_file(dir.path(), garbage);

    let error = write_keybinding_override(
        &path,
        A::TerminalSearch,
        Some(&bindings(&["Mod+Shift+F"])),
        Linux,
    )
    .expect_err("must refuse");
    assert!(matches!(error, WriteError::Unreadable(_)), "{error:?}");
    // The file is byte-for-byte unchanged.
    assert_eq!(std::fs::read_to_string(&path).unwrap(), garbage);
}

/// Legacy flat-root READ tolerance (keybinding-file.test.ts:146-159 read half):
/// an old flat-shape document (action ids at the root, no `keybindings` key) is
/// still parsed, with the reserved root keys skipped.
#[test]
fn reads_legacy_flat_root_document() {
    let dir = tempdir().unwrap();
    let path = write_file(
        dir.path(),
        r#"{
          "version": 1,
          "worktree.quickOpen": "Mod+Shift+P",
          "platforms": { "darwin": { "terminal.search": "Mod+F" } }
        }"#,
    );
    let snapshot = read_keybinding_file(&path, Linux);
    assert_eq!(
        snapshot.common_overrides.get(&A::WorktreeQuickOpen),
        Some(&bindings(&["Mod+Shift+P"]))
    );
    assert_eq!(
        overrides_of(&snapshot, A::WorktreeQuickOpen),
        Some(&bindings(&["Mod+Shift+P"]))
    );
    // `version` / `platforms` root keys were not mistaken for actions.
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:?}",
        snapshot.diagnostics
    );
}

/// A write into a brand-new file writes ONLY the requested override into the
/// active platform section, with empty common + sibling platform sections.
#[test]
fn write_into_new_file_seeds_active_platform_only() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("keybindings.json");
    write_keybinding_override(
        &path,
        A::TerminalSearch,
        Some(&bindings(&["Mod+Shift+F"])),
        Linux,
    )
    .expect("write");
    let written = read_json(&path);
    assert_eq!(written["version"], 1);
    assert_eq!(written["keybindings"], serde_json::json!({}));
    assert_eq!(
        written["platforms"]["linux"]["terminal.search"],
        Value::Array(vec!["Mod+Shift+F".into()])
    );
    assert_eq!(written["platforms"]["darwin"], serde_json::json!({}));
}
