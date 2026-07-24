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
}
