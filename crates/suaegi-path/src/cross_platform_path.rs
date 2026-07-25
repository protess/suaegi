//! Verbatim port of Orca `src/shared/cross-platform-path.ts` @ v1.4.150-rc.0.
//!
//! Line citations (`:N`) below refer to that source file. Where the plan's
//! Codex decisions (C1–C6) constrain a choice, the relevant `C_` tag is noted.
//! Quirks are DELIBERATELY preserved (POSIX backslash-as-filename-char, the
//! unresolved-`..` containment gap, the case-fold slice) — this is a security
//! port, not a cleanup.

/// Path flavor. Models the JS `'posix' | 'windows'` string union used to pick
/// separator/drive/UNC semantics without touching `std::path` (C1).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Flavor {
    Posix,
    Windows,
}

// ---------------------------------------------------------------------------
// Hand-rolled pattern predicates (C1: no `regex` crate).
// All operate on ASCII bytes; the drive/separator sentinels are all < 0x80 so
// byte indexing never lands inside a multi-byte UTF-8 sequence.
// ---------------------------------------------------------------------------

/// `/^[A-Za-z]:[\\/]/` — a drive letter, a colon, then a `\` or `/` separator.
/// Matches `C:\` and `C:/`; does NOT match a bare `C:` (no trailing separator).
fn has_drive_separator_prefix(value: &str) -> bool {
    let b = value.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
}

/// `/^[A-Za-z]:\/$/` (and the semantically identical `/^[a-z]:\/$/i`) — a bare
/// drive root such as `c:/`. Source uses two spellings across `:66`/`:82`/`:95`;
/// they mean the same thing, so a single helper is faithful.
fn is_drive_root(value: &str) -> bool {
    let b = value.as_bytes();
    b.len() == 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'/'
}

/// Collapse runs of `/` into a single `/` (`value.replace(/\/+/g, '/')`).
fn collapse_slashes(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut prev_slash = false;
    for c in value.chars() {
        if c == '/' {
            if !prev_slash {
                out.push('/');
            }
            prev_slash = true;
        } else {
            out.push(c);
            prev_slash = false;
        }
    }
    out
}

/// Case-insensitive (ASCII) prefix strip. `prefix` MUST be ASCII (all callers
/// pass ASCII WSL alias literals), which keeps the split a valid char boundary.
fn strip_prefix_ascii_ci<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    if value.len() >= prefix.len()
        && value.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
    {
        Some(&value[prefix.len()..])
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// :1-3 isWindowsAbsolutePathLike  (C2: START-anchored predicate)
// ---------------------------------------------------------------------------

/// `:1-3` — "does this path *look* like a Windows absolute / UNC path?" This is
/// the **start-anchored** Windows predicate (C2): drive+separator, OR a leading
/// `\\` (TWO backslashes), OR a leading `//`. A backslash in the *middle* of a
/// POSIX path does NOT count here (that is what preserves POSIX backslashes as
/// filename characters). Do NOT merge this with [`is_windows_path_flavor`].
pub fn is_windows_absolute_path_like(value: &str) -> bool {
    has_drive_separator_prefix(value) || value.starts_with("\\\\") || value.starts_with("//")
}

// ---------------------------------------------------------------------------
// :5-11 normalizeRuntimePathSeparators
// ---------------------------------------------------------------------------

/// `:5-11` — unify Windows separators (`\` -> `/`), collapse `/+` -> `/`, and
/// restore the leading UNC `//` when the ORIGINAL began with `\\` or `//`.
pub fn normalize_runtime_path_separators(value: &str) -> String {
    // `value.replace(/\\/g, '/')` then `.replace(/\/+/g, '/')` (:6).
    let replaced: String = value
        .chars()
        .map(|c| if c == '\\' { '/' } else { c })
        .collect();
    let normalized = collapse_slashes(&replaced);
    // :7-8 — UNC detection is on the ORIGINAL value, not the collapsed one.
    if value.starts_with("\\\\") || value.starts_with("//") {
        format!("//{}", normalized.trim_start_matches('/'))
    } else {
        normalized
    }
}

// ---------------------------------------------------------------------------
// :94-99 trimRuntimePathTrailingSlash
// ---------------------------------------------------------------------------

/// `:94-99` — strip trailing `/`, but preserve the filesystem root `/` and a
/// bare drive root (`C:/`) which legitimately end in a slash.
fn trim_runtime_path_trailing_slash(value: &str) -> String {
    if value == "/" || is_drive_root(value) {
        return value.to_string();
    }
    value.trim_end_matches('/').to_string()
}

// ---------------------------------------------------------------------------
// :20-26 WSL UNC alias fold — hand-rolled from
//   /^\/\/(?:wsl\.localhost|wsl\$)\/([^/]+)(\/[\s\S]*)?$/i
// ---------------------------------------------------------------------------

/// Matches a separator-normalized WSL UNC path. Returns `(distro, rest)` where
/// `rest` includes its leading `/` (or is empty). `[\s\S]` means the rest may
/// contain newlines — preserved verbatim (C6: no trim).
fn match_wsl_unc(normalized: &str) -> Option<(String, String)> {
    let after = normalized.strip_prefix("//")?;
    // Alias is matched case-insensitively (the regex `i` flag); the literals
    // `wsl.localhost` / `wsl$` are ASCII, so ASCII-CI is faithful.
    let after_alias = strip_prefix_ascii_ci(after, "wsl.localhost/")
        .or_else(|| strip_prefix_ascii_ci(after, "wsl$/"))?;
    // `([^/]+)` distro then optional `(\/[\s\S]*)`.
    let (distro, rest) = match after_alias.find('/') {
        Some(idx) => (&after_alias[..idx], &after_alias[idx..]),
        None => (after_alias, ""),
    };
    if distro.is_empty() {
        return None;
    }
    Some((distro.to_string(), rest.to_string()))
}

// ---------------------------------------------------------------------------
// :13-27 normalizeRuntimePathForComparison  (C5: Unicode fold)
// ---------------------------------------------------------------------------

/// `:13-27` — the containment comparison key. Normalizes separators (Windows
/// only; a POSIX path keeps its backslashes per `:15-16`), then case-folds:
/// WSL folds only the distro (`:20-24`), any other Windows-abs-like path folds
/// the WHOLE string (`:26`), and a POSIX path is left case-sensitive.
///
/// C5: uses Unicode [`str::to_lowercase`], NOT `to_ascii_lowercase` — JS
/// `toLowerCase` is Unicode (e.g. `İ` -> `i̇`, a length change).
///
/// # Unicode-table version skew (security-reviewed, deny-safe)
/// Rust's `to_lowercase` tables are pinned to the compiling rustc; JS
/// `toLowerCase` uses V8/ICU. A security-review fold sweep over U+0020–U+2FFFF
/// found ~28 rare epigraphic codepoints (e.g. U+A7D2, U+16EA0–U+16EB8) where JS
/// folds and Rust does not. **The divergence is uniformly deny-safe**: across
/// that whole range there are zero codepoints in the *escape* direction (Rust
/// folding two paths JS keeps distinct), so this fold is only ever *more*
/// restrictive than Orca's — never a containment escape. It can shift with the
/// rustc version; that only ever tightens containment, never loosens it.
pub fn normalize_runtime_path_for_comparison(value: &str) -> String {
    let is_windows_path = is_windows_absolute_path_like(value);
    // :17-19
    let normalized = trim_runtime_path_trailing_slash(&if is_windows_path {
        normalize_runtime_path_separators(value)
    } else {
        // POSIX: collapse `/+` only; backslash is a valid filename char (:15-16).
        collapse_slashes(value)
    });
    // :20-24
    if let Some((distro, rest)) = match_wsl_unc(&normalized) {
        return format!("//wsl/{}{}", distro.to_lowercase(), rest);
    }
    // :26
    if is_windows_path {
        normalized.to_lowercase()
    } else {
        normalized
    }
}

// ---------------------------------------------------------------------------
// :101-103 isWindowsPathFlavor  (C2: contains-`\`-ANYWHERE predicate)
// ---------------------------------------------------------------------------

/// `:101-103` — the **contains-anywhere** Windows predicate (C2): drive+sep, OR
/// a backslash ANYWHERE, OR a leading `//`. Differs from
/// [`is_windows_absolute_path_like`] (which needs a LEADING `\\`): `..\ws`
/// (relative, mid backslash) is flavor=windows but not abs-like, and
/// `/srv/team\repo` (POSIX, mid backslash) is flavor=windows yet NOT abs-like
/// (so comparison preserves its backslash). Merging the two is an escape.
fn is_windows_path_flavor(value: &str) -> bool {
    has_drive_separator_prefix(value) || value.contains('\\') || value.starts_with("//")
}

// ---------------------------------------------------------------------------
// :29-37 isRuntimePathAbsolute
// ---------------------------------------------------------------------------

/// `:29-37` — is `value` absolute under `flavor`? `flavor = None` reproduces the
/// JS default arg (`isWindowsPathFlavor(value) ? 'windows' : 'posix'`, `:31`).
/// Under Windows, a bare leading `\` or `/` also counts (`:34`).
pub fn is_runtime_path_absolute(value: &str, flavor: Option<Flavor>) -> bool {
    let flavor = flavor.unwrap_or_else(|| {
        if is_windows_path_flavor(value) {
            Flavor::Windows
        } else {
            Flavor::Posix
        }
    });
    match flavor {
        // :33-34
        Flavor::Windows => {
            has_drive_separator_prefix(value) || value.starts_with('\\') || value.starts_with('/')
        }
        // :36
        Flavor::Posix => value.starts_with('/'),
    }
}

// ---------------------------------------------------------------------------
// :39-49 resolveRuntimePath
// ---------------------------------------------------------------------------

/// `:39-49` — lexically resolve `target_path` against `base_path`, resolving
/// `.`/`..` (via [`normalize_runtime_path_dots`]). Never touches the process
/// cwd. An absolute target wins outright (`:42-43`). Case is PRESERVED (only
/// comparison folds); output separators are `/`.
pub fn resolve_runtime_path(base_path: &str, target_path: &str) -> String {
    // :40-41 — EITHER operand being Windows-flavor makes the whole op Windows.
    let flavor = if is_windows_path_flavor(base_path) || is_windows_path_flavor(target_path) {
        Flavor::Windows
    } else {
        Flavor::Posix
    };
    if is_runtime_path_absolute(target_path, Some(flavor)) {
        return normalize_runtime_path_dots(target_path, flavor);
    }
    // :45-48
    let combined = format!(
        "{}/{}",
        trim_runtime_path_trailing_slash(&normalize_runtime_path_separators(base_path)),
        target_path
    );
    normalize_runtime_path_dots(&combined, flavor)
}

// ---------------------------------------------------------------------------
// :51-57 getRuntimePathBasename
// ---------------------------------------------------------------------------

/// `:51-57` — the last non-empty segment. NOTE the deliberate asymmetry
/// (`:56`): this ALWAYS splits on BOTH `\` and `/`, regardless of flavor —
/// unlike the comparison path, which preserves POSIX backslashes. So POSIX
/// `team\repo` yields `repo` here. Ported exactly.
pub fn get_runtime_path_basename(value: &str) -> String {
    // :52 — `value.replace(/[\\/]+$/g, '')`.
    let trimmed = value.trim_end_matches(['\\', '/']);
    if trimmed.is_empty() {
        return String::new();
    }
    // :56 — `.split(/[\\/]/).findLast(Boolean) ?? ''`.
    trimmed
        .split(['\\', '/'])
        .rfind(|s| !s.is_empty())
        .unwrap_or("")
        .to_string()
}

// ---------------------------------------------------------------------------
// :59-68 isPathInsideOrEqual  (C3 boundary defense; C4 unresolved-`..` gap)
// ---------------------------------------------------------------------------

/// `:59-68` — is `candidate_path` inside (or equal to) `root_path`?
///
/// Containment is a normalized-string **prefix match**, not a relative-path
/// computation. Both sides are folded via [`normalize_runtime_path_for_comparison`];
/// equal ⇒ inside; otherwise a `/` boundary is appended to the root (with the
/// FS-root `/` and drive-root `c:/` special-cased) before `starts_with` (C3).
/// That `/` boundary is the ONLY thing separating a real child from a sibling
/// whose name merely *starts with* the root (`/repo/app` vs `/repo/application`).
///
/// # Caller contract (C4 — SECURITY)
/// This is PURELY LEXICAL and does NOT resolve `..`. A literal `..` segment is
/// prefix-matched as-is, so `is_path_inside_or_equal("/safe/root",
/// "/safe/root/../outside")` returns `true` (a preserved Orca behavior).
/// Callers MUST pre-resolve BOTH arguments with [`resolve_runtime_path`] before
/// relying on this for a security boundary.
pub fn is_path_inside_or_equal(root_path: &str, candidate_path: &str) -> bool {
    let root = normalize_runtime_path_for_comparison(root_path);
    let candidate = normalize_runtime_path_for_comparison(candidate_path);
    // :62-64 — "OrEqual".
    if candidate == root {
        return true;
    }
    // :65-66
    let root_with_boundary = if root == "/" || is_drive_root(&root) {
        root
    } else {
        format!("{}/", root.trim_end_matches('/'))
    };
    // :67
    candidate.starts_with(&root_with_boundary)
}

// ---------------------------------------------------------------------------
// :70-92 relativePathInsideRoot  (C3 boundary + C5 UTF-16 suffix)
// ---------------------------------------------------------------------------

/// `:70-92` — the case-preserving relative path of `candidate_path` under
/// `root_path`, or `None` when the candidate is OUTSIDE (note: `None`, NOT a
/// `..`-prefixed relative like Node's `path.relative`). Equal ⇒ `Some("")`.
///
/// Same lexical prefix-match and `..`-gap caller contract as
/// [`is_path_inside_or_equal`].
///
/// # UTF-16 suffix (C5)
/// The suffix (`:89-91`) reproduces JS `String.prototype.slice(N)`, where `N`
/// is the UTF-16 code-unit length of the comparison prefix, applied to either
/// the comparison candidate (WSL branch) or the case-preserving normalized
/// candidate (non-WSL branch) — ported exactly. For the common ASCII path the
/// fold is length-preserving so `N` aligns; for a length-changing Unicode fold
/// the slice may be misaligned, EXACTLY as JS is, but it is byte-safe and
/// PANIC-FREE (a split surrogate decodes lossily to U+FFFD — see
/// [`js_slice_from_utf16`]).
pub fn relative_path_inside_root(root_path: &str, candidate_path: &str) -> Option<String> {
    // :71-75 — case-PRESERVING candidate (the returned suffix source).
    let normalized_candidate =
        trim_runtime_path_trailing_slash(&if is_windows_absolute_path_like(candidate_path) {
            normalize_runtime_path_separators(candidate_path)
        } else {
            collapse_slashes(candidate_path)
        });
    // :76-77 — folded comparison keys.
    let comparison_root = normalize_runtime_path_for_comparison(root_path);
    let comparison_candidate = normalize_runtime_path_for_comparison(candidate_path);

    // :79-81
    if comparison_candidate == comparison_root {
        return Some(String::new());
    }
    // :82-83
    let is_root = comparison_root == "/" || is_drive_root(&comparison_root);
    let comparison_prefix = if is_root {
        comparison_root.clone()
    } else {
        format!("{comparison_root}/")
    };
    // :84-86
    if !comparison_candidate.starts_with(&comparison_prefix) {
        return None;
    }
    // :89-91 — N = UTF-16 code-unit length of the comparison prefix.
    let n = comparison_prefix.encode_utf16().count();
    if comparison_root.starts_with("//wsl/") {
        Some(js_slice_from_utf16(&comparison_candidate, n))
    } else {
        Some(js_slice_from_utf16(&normalized_candidate, n))
    }
}

/// Reproduce JS `str.slice(start_unit)`: drop the first `start_unit` UTF-16
/// code units and decode the remainder. PANIC-FREE — if `start_unit` splits a
/// surrogate pair (unreachable for real paths), the orphaned surrogate decodes
/// lossily to U+FFFD rather than panicking or corrupting silently. This is the
/// C5 requirement: never a raw byte slice (which would panic mid-char).
fn js_slice_from_utf16(value: &str, start_unit: usize) -> String {
    char::decode_utf16(value.encode_utf16().skip(start_unit))
        .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

// ---------------------------------------------------------------------------
// :105-128 normalizeRuntimePathDots
// ---------------------------------------------------------------------------

/// `:105-128` — resolve `.`/`..` lexically. Under a rooted path, `..` above the
/// root is dropped (`:118`); under a relative path, leading `..` is preserved
/// (`:116-117`). An empty relative result is `.` (`:124-125`).
fn normalize_runtime_path_dots(value: &str, flavor: Flavor) -> String {
    let normalized = normalize_runtime_path_separators(value);
    let (root, rest) = split_runtime_path_root(&normalized, flavor);
    let mut segments: Vec<&str> = Vec::new();
    for segment in rest.split('/') {
        // :110-112
        if segment.is_empty() || segment == "." {
            continue;
        }
        // :113-120
        if segment == ".." {
            if !segments.is_empty() && *segments.last().unwrap() != ".." {
                segments.pop();
            } else if root.is_empty() {
                segments.push(segment);
            }
            continue;
        }
        // :121
        segments.push(segment);
    }
    // :123
    let suffix = segments.join("/");
    // :124-125
    if root.is_empty() {
        return if suffix.is_empty() {
            ".".to_string()
        } else {
            suffix
        };
    }
    // :127
    if suffix.is_empty() {
        trim_runtime_path_trailing_slash(&root)
    } else {
        format!("{root}{suffix}")
    }
}

// ---------------------------------------------------------------------------
// :130-155 splitRuntimePathRoot
// ---------------------------------------------------------------------------

/// `:130-155` — split a separator-normalized path into `(root, rest)`. Windows
/// recognizes a drive root (`C:/`), a UNC server/share root (`//srv/share/`),
/// and a bare `/`; a Windows path matching none FALLS THROUGH to the POSIX
/// arm (`:151`), so e.g. `C:foo` (no separator) becomes a relative path.
fn split_runtime_path_root(value: &str, flavor: Flavor) -> (String, String) {
    if flavor == Flavor::Windows {
        // :135-137 — drive `/^([A-Za-z]:)(?:\/|$)/`.
        if let Some((drive, matched_len)) = match_drive(value) {
            return (format!("{drive}/"), value[matched_len..].to_string());
        }
        // :139-146 — UNC `//server/share/...`.
        if let Some(after) = value.strip_prefix("//") {
            let parts: Vec<&str> = after.split('/').collect();
            if parts.len() >= 2 && !parts[0].is_empty() && !parts[1].is_empty() {
                let root = format!("//{}/{}/", parts[0], parts[1]);
                let rest = parts[2..].join("/");
                return (root, rest);
            }
            return ("//".to_string(), after.to_string());
        }
        // :147-148
        if let Some(rest) = value.strip_prefix('/') {
            return ("/".to_string(), rest.to_string());
        }
    }
    // :151-153 (POSIX arm / Windows fall-through)
    if let Some(rest) = value.strip_prefix('/') {
        return ("/".to_string(), rest.to_string());
    }
    // :154
    (String::new(), value.to_string())
}

/// `/^([A-Za-z]:)(?:\/|$)/` — returns `(drive, matched_len)` where `drive` is
/// the two-char `X:` and `matched_len` is the full match length (2 at
/// end-of-string, 3 when a `/` follows). `value` is already separator-
/// normalized, so only `/` (never `\`) appears after the colon.
fn match_drive(value: &str) -> Option<(String, usize)> {
    let b = value.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        let drive = value[..2].to_string();
        if b.len() == 2 {
            Some((drive, 2))
        } else if b[2] == b'/' {
            Some((drive, 3))
        } else {
            None
        }
    } else {
        None
    }
}

#[cfg(test)]
mod tests;
