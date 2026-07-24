//! Path normalization — verbatim port of `normalizeRelativePath`
//! (`src/shared/text-search.ts:33-35`).

/// Normalize a relative path so results are cross-platform stable and never
/// break a caller's `join(root_path, rel_path)`.
///
/// Two steps, matching Orca's `path.replace(/[\\/]+/g, '/').replace(/^\/+/, '')`
/// exactly (`text-search.ts:34`):
/// 1. Collapse every run of `\` or `/` (in any mix) to a single `/`.
/// 2. Strip ALL leading `/`.
///
/// **Manual string ops only — `std::path` is deliberately avoided**: it is
/// platform-dependent (separator, drive letters, UNC handling) and would diverge
/// from the JS oracle, which treats both slashes identically on every OS.
pub fn normalize_relative_path(path: &str) -> String {
    // Step 1: collapse runs of `\`/`/` into a single `/`.
    let mut collapsed = String::with_capacity(path.len());
    let mut prev_was_sep = false;
    for ch in path.chars() {
        if ch == '\\' || ch == '/' {
            if !prev_was_sep {
                collapsed.push('/');
            }
            prev_was_sep = true;
        } else {
            collapsed.push(ch);
            prev_was_sep = false;
        }
    }

    // Step 2: strip ALL leading `/`.
    collapsed.trim_start_matches('/').to_string()
}

// ─── search-root relative/join (pathFlavor) ──────────────────────────
//
// Ports `pathFlavor` (`text-search.ts:37-42`) + `relativeToSearchRoot`
// (`:44-46`) + `joinSearchRoot` (`:48-50`). Orca dispatches to Node's
// `path.win32` / `path.posix` so the relative/join math never depends on the
// host OS. Rust's `std::path` IS host-fixed, so — like `normalize_relative_path`
// — these are manual string ops.
//
// Domain: rg emits absolute paths *under* the search target (= `root_path`), and
// git-grep emits paths that are rejoined onto the root. Both callers pass the
// result of `relative_to_search_root` through [`normalize_relative_path`], which
// collapses separators and strips leading slashes — so these helpers only need
// to strip / reattach the root prefix; the downstream normalize forgives leading
// separators and `\` vs `/`. When `abs` is NOT under `root` (never happens for
// rg in practice) `relative_to_search_root` falls back to returning `abs`
// unchanged (normalize still yields a sane relative-ish path) rather than
// reproducing Node's `../..` ascent — a documented, unexercised divergence.

/// Path dialect selected by the shape of `root_path`, mirroring `pathFlavor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flavor {
    Posix,
    Win32,
}

/// `pathFlavor(rootPath)` (`text-search.ts:37-42`): a leading drive letter
/// (`C:\`, `C:/`) or a UNC prefix (`\\`) selects win32, else posix.
fn path_flavor(root: &str) -> Flavor {
    let b = root.as_bytes();
    let drive = b.len() >= 3
        && b[0].is_ascii_alphabetic()
        && b[1] == b':'
        && (b[2] == b'\\' || b[2] == b'/');
    if drive || root.starts_with("\\\\") {
        Flavor::Win32
    } else {
        Flavor::Posix
    }
}

fn is_sep(c: char) -> bool {
    c == '/' || c == '\\'
}

/// `relativeToSearchRoot(rootPath, absPath)` (`text-search.ts:44-46`).
///
/// Strips the `root` prefix from `abs` and returns the tail (with its leading
/// separator intact — the caller's [`normalize_relative_path`] removes it). The
/// win32 flavor matches the prefix ASCII-case-insensitively (Node's win32
/// semantics); non-ASCII bytes are compared exactly. Uses `str::get(..n)` so a
/// non-char-boundary prefix length can NEVER panic — it just misses and falls
/// through to the whole-`abs` fallback.
pub(crate) fn relative_to_search_root(root: &str, abs: &str) -> String {
    let flavor = path_flavor(root);
    let root_trim = root.trim_end_matches(is_sep);
    if let Some(head) = abs.get(..root_trim.len()) {
        let prefix_matches = match flavor {
            Flavor::Posix => head == root_trim,
            Flavor::Win32 => head.eq_ignore_ascii_case(root_trim),
        };
        if prefix_matches {
            // `get(..n)` returned `Some`, so `root_trim.len()` is a char boundary.
            let tail = &abs[root_trim.len()..];
            if tail.is_empty() {
                return String::new();
            }
            if tail.starts_with(is_sep) {
                return tail.to_string();
            }
        }
    }
    // Not under `root` (unexercised for rg); normalize downstream yields a sane
    // path. See module note above.
    abs.to_string()
}

/// `joinSearchRoot(rootPath, relPath)` (`text-search.ts:48-50`).
///
/// Joins with the flavor's separator, collapsing the one boundary separator so
/// `join("/root", "src/a.ts")` → `/root/src/a.ts`. Used only to reconstruct the
/// git-grep `filePath` (and the accumulator map key); it is never sliced.
pub(crate) fn join_search_root(root: &str, rel: &str) -> String {
    let sep = match path_flavor(root) {
        Flavor::Posix => '/',
        Flavor::Win32 => '\\',
    };
    let root_trim = root.trim_end_matches(is_sep);
    let rel_trim = rel.trim_start_matches(is_sep);
    if rel_trim.is_empty() {
        root_trim.to_string()
    } else {
        format!("{root_trim}{sep}{rel_trim}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Oracle case 1 (`text-search.test.ts:21`): collapses mixed separators and
    /// strips leading slashes.
    #[test]
    fn oracle_collapses_and_strips() {
        assert_eq!(normalize_relative_path("a\\b\\c"), "a/b/c");
        assert_eq!(normalize_relative_path("/a/b"), "a/b");
        // Prompt-specified extra: mixed backslash/forward-slash leading run.
        assert_eq!(normalize_relative_path("\\/a\\/b"), "a/b");
        // Mutation target (c): dropping the leading-`/` strip yields `/a/b`.
        assert_eq!(normalize_relative_path("///a//b"), "a/b");
    }

    /// A path with no separators is returned unchanged (no spurious prefixing).
    #[test]
    fn passthrough_no_separators() {
        assert_eq!(normalize_relative_path("file.ts"), "file.ts");
        assert_eq!(normalize_relative_path(""), "");
    }

    /// Interior runs collapse but a single interior separator is preserved —
    /// pins that collapsing does not eat non-separator characters.
    #[test]
    fn interior_single_separator_preserved() {
        assert_eq!(normalize_relative_path("src/a/b.ts"), "src/a/b.ts");
    }

    /// posix root: rg-style abs under a posix root strips to the tail (leading
    /// `/` kept, removed by the caller's normalize). Oracle case 8 path shape.
    #[test]
    fn relative_posix_under_root() {
        assert_eq!(relative_to_search_root("/root", "/root/src/a.ts"), "/src/a.ts");
        assert_eq!(
            normalize_relative_path(&relative_to_search_root("/root", "/root/src/a.ts")),
            "src/a.ts"
        );
    }

    /// win32/UNC root (oracle case 15 shape): the `\\wsl$\...` prefix is stripped
    /// ASCII-case-insensitively, leaving the rg-emitted `/a.ts` tail.
    #[test]
    fn relative_win32_unc_under_root() {
        let root = "\\\\wsl$\\Ubuntu\\home\\u\\repo";
        let abs = "\\\\wsl$\\Ubuntu\\home\\u\\repo/a.ts";
        assert_eq!(relative_to_search_root(root, abs), "/a.ts");
        assert_eq!(
            normalize_relative_path(&relative_to_search_root(root, abs)),
            "a.ts"
        );
        // Case-insensitive prefix match (win32).
        let abs_mixed = "\\\\WSL$\\ubuntu\\home\\u\\repo/a.ts";
        assert_eq!(relative_to_search_root(root, abs_mixed), "/a.ts");
    }

    /// A non-char-boundary root length must not panic: a multibyte `abs` that
    /// does not share the root's byte prefix falls through to the whole-`abs`
    /// fallback (via `str::get`), never a slice panic.
    #[test]
    fn relative_non_boundary_prefix_never_panics() {
        // root len 5 bytes; abs has a 2-byte `é` straddling byte 5.
        let out = relative_to_search_root("/root", "/roété/x");
        // Not under root → fallback returns abs unchanged.
        assert_eq!(out, "/roété/x");
    }

    /// A path that is not under the root falls back to the whole path.
    #[test]
    fn relative_not_under_root_fallback() {
        assert_eq!(relative_to_search_root("/root", "/other/x"), "/other/x");
        // Sibling that shares a textual prefix but not a path segment: `/rootier`
        // does not start with `/root` + separator, so it is NOT treated as under.
        assert_eq!(relative_to_search_root("/root", "/rootier"), "/rootier");
    }

    /// posix + win32 join reattach the root with the flavor separator, collapsing
    /// the boundary. Used for the git-grep `filePath` / map key.
    #[test]
    fn join_reattaches_root() {
        assert_eq!(join_search_root("/root", "src/a.ts"), "/root/src/a.ts");
        assert_eq!(join_search_root("/root/", "/src/a.ts"), "/root/src/a.ts");
        assert_eq!(
            join_search_root("C:\\repo", "src\\a.ts"),
            "C:\\repo\\src\\a.ts"
        );
    }
}
