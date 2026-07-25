//! Tests for the cross-platform-path port.
//!
//! The 7 `oracle_*` tests are a 1:1 port of Orca's `cross-platform-path.test.ts`
//! @ v1.4.150-rc.0 (the ORACLE). The `pin_*` tests lock down the C2-C5 + C4
//! decisions and the oracle-UNTESTED exports. Every test is written to be
//! mutation-verifiable (a targeted mutation of the corresponding logic must
//! flip an assertion / panic).

use super::*;

// ===========================================================================
// Oracle port (all 7 `it()` blocks)
// ===========================================================================

// it :11-16 "keeps POSIX sibling prefixes outside the root"
#[test]
fn oracle_posix_sibling_prefixes() {
    assert!(is_path_inside_or_equal("/repo/app", "/repo/app"));
    assert!(is_path_inside_or_equal(
        "/repo/app",
        "/repo/app/src/index.ts"
    ));
    assert!(!is_path_inside_or_equal(
        "/repo/app",
        "/repo/application/src/index.ts"
    ));
    assert_eq!(
        relative_path_inside_root("/repo/app/", "/repo/app/src/index.ts"),
        Some("src/index.ts".to_string())
    );
}

// it :18-23 "keeps literal POSIX backslashes distinct from separators"
#[test]
fn oracle_posix_literal_backslashes() {
    assert_eq!(
        normalize_runtime_path_for_comparison("/srv/team\\repo"),
        "/srv/team\\repo"
    );
    assert_eq!(
        normalize_runtime_path_for_comparison("/srv/team/repo"),
        "/srv/team/repo"
    );
    assert!(!is_path_inside_or_equal(
        "/srv/team\\repo",
        "/srv/team/repo/file.ts"
    ));
    assert_eq!(
        relative_path_inside_root("/srv/repo", "/srv/repo/a\\b.txt"),
        Some("a\\b.txt".to_string())
    );
}

// it :25-30 "handles Windows drive roots and sibling drives case-insensitively"
#[test]
fn oracle_windows_drive_roots() {
    assert!(is_path_inside_or_equal(
        "C:\\Repo",
        "c:\\repo\\src\\index.ts"
    ));
    assert_eq!(
        relative_path_inside_root("C:\\Repo", "c:\\repo\\src\\index.ts"),
        Some("src/index.ts".to_string())
    );
    assert!(!is_path_inside_or_equal(
        "C:\\Repo",
        "D:\\Repo\\src\\index.ts"
    ));
    assert_eq!(
        relative_path_inside_root("C:\\", "c:\\repo\\src\\index.ts"),
        Some("repo/src/index.ts".to_string())
    );
}

// it :32-38 "handles UNC roots, trailing slashes, mixed separators, and case"
#[test]
fn oracle_unc_roots() {
    assert!(is_path_inside_or_equal(
        "\\\\Server\\Share\\Repo\\",
        "//server/share/repo/src"
    ));
    assert_eq!(
        relative_path_inside_root("\\\\Server\\Share\\Repo\\", "//server/share/repo/src"),
        Some("src".to_string())
    );
    assert!(!is_path_inside_or_equal(
        "\\\\Server\\Share\\Repo",
        "\\\\server\\share\\repo2"
    ));
}

// it :40-71 "treats WSL UNC aliases as the same case-sensitive filesystem"
#[test]
fn oracle_wsl_unc_aliases() {
    assert!(is_path_inside_or_equal(
        "\\\\wsl$\\Ubuntu\\home\\Alice\\repo",
        "\\\\wsl.localhost\\ubuntu\\home\\Alice\\repo\\src"
    ));
    assert_eq!(
        relative_path_inside_root(
            "\\\\wsl$\\Ubuntu\\home\\Alice\\repo",
            "\\\\wsl.localhost\\ubuntu\\home\\Alice\\repo\\Src"
        ),
        Some("Src".to_string())
    );
    // distro-below path is case-SENSITIVE: alice != Alice.
    assert!(!is_path_inside_or_equal(
        "\\\\wsl$\\Ubuntu\\home\\Alice\\repo",
        "\\\\wsl.localhost\\ubuntu\\home\\alice\\repo\\src"
    ));
    assert_eq!(
        relative_path_inside_root(
            "\\\\wsl$\\Ubuntu\\home\\Alice\\repo",
            "\\\\wsl.localhost\\ubuntu\\home\\alice\\repo\\src"
        ),
        None
    );
    // Embedded newline is preserved literally (C6: no trim; regex `[\s\S]`).
    assert_eq!(
        relative_path_inside_root(
            "\\\\wsl$\\Ubuntu\\home\\Alice\\repo",
            "\\\\wsl.localhost\\ubuntu\\home\\Alice\\repo\\line\nbreak"
        ),
        Some("line\nbreak".to_string())
    );
}

// it :73-79 "resolves POSIX relative paths without using the process cwd"
#[test]
fn oracle_resolve_posix() {
    assert_eq!(
        resolve_runtime_path("/repos/app/repo", "../worktrees/feature"),
        "/repos/app/worktrees/feature"
    );
    assert_eq!(
        resolve_runtime_path("/repos/app/repo", "/custom/worktrees"),
        "/custom/worktrees"
    );
    assert!(!is_runtime_path_absolute("../worktrees", None));
}

// it :81-87 "resolves Windows relative paths with Windows semantics"
#[test]
fn oracle_resolve_windows() {
    assert_eq!(
        resolve_runtime_path("C:\\Repos\\app\\repo", "..\\worktrees\\feature"),
        "C:/Repos/app/worktrees/feature"
    );
    assert_eq!(
        resolve_runtime_path("C:\\Repos\\app\\repo", "D:\\worktrees"),
        "D:/worktrees"
    );
    assert!(is_runtime_path_absolute(
        "/remote/worktrees",
        Some(Flavor::Windows)
    ));
}

// ===========================================================================
// C2 — two-predicate split (merging the two Windows predicates breaks one)
// ===========================================================================

#[test]
fn pin_c2_two_predicate_split() {
    // A POSIX path with a MID backslash: NOT abs-like (start-anchored), but IS
    // windows-flavor (contains-anywhere). Merging them diverges.
    assert!(!is_windows_absolute_path_like("/srv/team\\repo"));
    assert!(is_windows_path_flavor("/srv/team\\repo"));

    // The comparison path uses the abs-like predicate, so the backslash is
    // preserved (folding it would collapse `team\repo` into a separator).
    assert_eq!(
        normalize_runtime_path_for_comparison("/srv/team\\repo"),
        "/srv/team\\repo"
    );

    // The flavor predicate drives resolve: a mid-backslash relative target is
    // treated with Windows semantics.
    assert_eq!(resolve_runtime_path("C:\\base", "..\\sib"), "C:/sib");
}

// ===========================================================================
// C3 — sibling-prefix `/`-boundary defense
// ===========================================================================

#[test]
fn pin_c3_sibling_prefix_boundary() {
    // Bare sibling (no trailing segment): the `/` boundary is the whole defense.
    assert!(!is_path_inside_or_equal("/repo/app", "/repo/application"));
    // Equal → inside.
    assert!(is_path_inside_or_equal("/repo/app", "/repo/app"));
    // Real child → inside.
    assert!(is_path_inside_or_equal("/repo/app", "/repo/app/lib"));
    // And via the relative form: sibling is None, child is the suffix.
    assert_eq!(
        relative_path_inside_root("/repo/app", "/repo/application"),
        None
    );
    assert_eq!(
        relative_path_inside_root("/repo/app", "/repo/app/lib"),
        Some("lib".to_string())
    );
}

// ===========================================================================
// C4 — `..` NOT resolved in containment (preserved gap) + safe wrapper
// ===========================================================================

#[test]
fn pin_c4_dotdot_not_resolved_preserve() {
    // PRESERVED ORCA BEHAVIOR: a literal `..` segment is prefix-matched as-is,
    // so an obvious traversal is (wrongly, by intent) reported INSIDE. This is
    // WHY callers MUST pre-resolve both arguments before trusting containment.
    assert!(is_path_inside_or_equal(
        "/safe/root",
        "/safe/root/../outside"
    ));
}

#[test]
fn pin_c4_safe_wrapper_rejects_after_resolve() {
    // The safe wrapper: resolve the candidate first, THEN check containment.
    // `/safe/root/../outside` resolves to `/safe/outside` (the `..` pops
    // `root`), which is outside `/safe/root` → rejected.
    let resolved = resolve_runtime_path("/", "/safe/root/../outside");
    assert_eq!(resolved, "/safe/outside");
    assert!(!is_path_inside_or_equal("/safe/root", &resolved));
}

// ===========================================================================
// C5 — case-fold (Unicode to_lowercase) + UTF-16-safe suffix
// ===========================================================================

#[test]
fn pin_c5_windows_wholestring_fold() {
    // Windows abs-like → whole-string fold, so C:\Foo contains c:\foo\bar.
    assert!(is_path_inside_or_equal("C:\\Foo", "c:\\foo\\bar"));
    assert_eq!(
        relative_path_inside_root("C:\\Foo", "c:\\foo\\bar"),
        Some("bar".to_string())
    );
}

#[test]
fn pin_c5_non_ascii_fold_suffix_is_panic_free() {
    // `İ` (U+0130) folds via Unicode to_lowercase to `i̇` (U+0069 U+0307) — a
    // LENGTH-CHANGING fold. This exercises the UTF-16 suffix slice on a
    // case-preserving candidate whose byte layout diverges from the folded
    // prefix. Expectations:
    //   * It must NOT panic (a raw byte slice would split the multi-byte `é`).
    //   * It must reproduce JS `.slice(N)` exactly: N = UTF-16 units of the
    //     folded prefix "c:/i̇/" = 6, applied to "C:/İ/ébc" → "bc".
    //   * With to_ascii_lowercase (İ unchanged), N would be 5 and the result
    //     "ébc" — so this also pins the Unicode-fold choice.
    let out = relative_path_inside_root("C:\\İ", "C:\\İ\\ébc");
    assert_eq!(out, Some("bc".to_string()));

    // Containment on the same non-ASCII fold does not panic and holds.
    assert!(is_path_inside_or_equal("C:\\İ", "C:\\İ\\ébc"));
}

#[test]
fn pin_c5_wsl_distro_fold_preserves_linux_case() {
    // WSL folds the distro but PRESERVES the case-sensitive Linux path below it.
    // Distro Ubuntu≡ubuntu and alias wsl$≡wsl.localhost, but `Alice` must match
    // `Alice` (not `alice`).
    assert!(is_path_inside_or_equal(
        "\\\\wsl$\\Ubuntu\\home\\Alice",
        "\\\\wsl.localhost\\ubuntu\\home\\Alice\\repo"
    ));
    assert!(!is_path_inside_or_equal(
        "\\\\wsl$\\Ubuntu\\home\\Alice",
        "\\\\wsl.localhost\\ubuntu\\home\\alice\\repo"
    ));
    // The returned suffix keeps Linux-side casing verbatim.
    assert_eq!(
        relative_path_inside_root(
            "\\\\wsl$\\Ubuntu\\home\\Alice",
            "\\\\wsl.localhost\\ubuntu\\home\\Alice\\RePo"
        ),
        Some("RePo".to_string())
    );
}

// ===========================================================================
// Oracle-UNTESTED exports — direct pins
// ===========================================================================

#[test]
fn pin_get_runtime_path_basename() {
    assert_eq!(get_runtime_path_basename("/repo/app/"), "app");
    assert_eq!(get_runtime_path_basename("/repo/app"), "app");
    // Flavor-AGNOSTIC backslash split (asymmetric vs the comparison path):
    // POSIX `team\repo` yields `repo`.
    assert_eq!(get_runtime_path_basename("team\\repo"), "repo");
    assert_eq!(get_runtime_path_basename("C:\\Users\\foo"), "foo");
    // Empty / all-separator inputs.
    assert_eq!(get_runtime_path_basename(""), "");
    assert_eq!(get_runtime_path_basename("//"), "");
    // `//`-produced empty segments are skipped (findLast(Boolean)).
    assert_eq!(get_runtime_path_basename("a//b"), "b");
}

#[test]
fn pin_normalize_runtime_path_separators() {
    assert_eq!(
        normalize_runtime_path_separators("C:\\Users\\foo"),
        "C:/Users/foo"
    );
    // UNC double-slash restored after collapse.
    assert_eq!(
        normalize_runtime_path_separators("\\\\server\\share"),
        "//server/share"
    );
    assert_eq!(
        normalize_runtime_path_separators("//server//share"),
        "//server/share"
    );
    // Repeated-slash collapse on a plain path.
    assert_eq!(normalize_runtime_path_separators("a//b///c"), "a/b/c");
    // A non-UNC single-leading-slash path is NOT turned into UNC.
    assert_eq!(normalize_runtime_path_separators("/a/b"), "/a/b");
}

#[test]
fn pin_is_windows_absolute_path_like() {
    assert!(is_windows_absolute_path_like("C:\\"));
    assert!(is_windows_absolute_path_like("C:/"));
    assert!(is_windows_absolute_path_like("//srv/share"));
    assert!(is_windows_absolute_path_like("\\\\srv\\share"));
    // Bare drive with NO separator does not match.
    assert!(!is_windows_absolute_path_like("C:"));
    // POSIX absolute is not "windows abs-like".
    assert!(!is_windows_absolute_path_like("/usr/bin"));
    // A single leading backslash is not `\\`.
    assert!(!is_windows_absolute_path_like("\\srv"));
    // Mid backslash (relative) is not start-anchored.
    assert!(!is_windows_absolute_path_like("..\\x"));
}

#[test]
fn pin_normalize_runtime_path_dots() {
    assert_eq!(
        normalize_runtime_path_dots("/a/b/../c", Flavor::Posix),
        "/a/c"
    );
    assert_eq!(normalize_runtime_path_dots("/a/./b", Flavor::Posix), "/a/b");
    // Leading `..` preserved on a relative path.
    assert_eq!(normalize_runtime_path_dots("../a", Flavor::Posix), "../a");
    // Empty relative result → ".".
    assert_eq!(normalize_runtime_path_dots(".", Flavor::Posix), ".");
    assert_eq!(normalize_runtime_path_dots("a/..", Flavor::Posix), ".");
    // `..` above a root is dropped.
    assert_eq!(normalize_runtime_path_dots("/../a", Flavor::Posix), "/a");
    // Windows drive root, empty suffix preserved as `C:/`.
    assert_eq!(normalize_runtime_path_dots("C:\\", Flavor::Windows), "C:/");
    assert_eq!(
        normalize_runtime_path_dots("C:\\a\\..\\b", Flavor::Windows),
        "C:/b"
    );
}

// ===========================================================================
// Empty-root / relative-root behavior (caller-prohibited footguns) — pinned
// ===========================================================================

#[test]
fn pin_empty_and_relative_root_behavior() {
    // Two empty inputs normalize equal → "inside". Caller must not pass these.
    assert!(is_path_inside_or_equal("", ""));
    // DANGER (pinned, not endorsed): an empty root yields boundary "/", so it
    // "contains" every absolute path. Callers MUST reject empty/relative roots.
    assert!(is_path_inside_or_equal("", "/foo"));
    // A relative root also prefix-matches loosely; pinned for awareness.
    assert!(is_path_inside_or_equal("a", "a/b"));
    assert!(!is_path_inside_or_equal("a", "ab"));
}

#[test]
fn pin_is_runtime_path_absolute_windows_single_sep() {
    // Under explicit Windows flavor, a bare leading `\` or `/` is absolute.
    assert!(is_runtime_path_absolute("\\foo", Some(Flavor::Windows)));
    assert!(is_runtime_path_absolute("/foo", Some(Flavor::Windows)));
    // Drive needs a separator.
    assert!(!is_runtime_path_absolute("C:foo", Some(Flavor::Windows)));
    assert!(is_runtime_path_absolute("C:\\foo", Some(Flavor::Windows)));
    // POSIX: only leading `/`.
    assert!(!is_runtime_path_absolute("\\foo", Some(Flavor::Posix)));
}
