//! VERBATIM port of the path/candidate layer of Orca's
//! `src/shared/mcp-config.ts` (@ v1.4.150-rc.0), milestone M1.
//!
//! Ported: the `McpConfigFormat` / `McpConfigCandidate` / `McpConfigDirectoryEntry`
//! types, the `MCP_CONFIG_CANDIDATES` / `MCP_STARTER_CONFIG` constants, the
//! `getMcpConfigParentDirs` / `getMcpConfigCandidateParentDir` /
//! `selectExistingMcpConfigCandidates` / `canInspectLocalMcpConfigRoot`
//! functions, and the private `getRelativeParentDir` / `getRelativeBasename`
//! helpers.
//!
//! M2a (`src/json.rs` + `src/env_mask.rs`) has since landed: the
//! order-preserving JSON value type, `js_string_of`/ECMAScript number
//! formatting, the sensitive-pattern predicates, and `maskMcpEnv`.
//!
//! M2b (`src/inspection.rs`) has since landed too, completing the module:
//! `inspectMcpConfigContent`, `summarizeMcpServer`,
//! `readCommand`/`readUrl`/`resolveTransport`, `extractObjectAtPath`, and the
//! `McpServerTransport`/`McpServerStatus`/`McpServerSummary`/
//! `McpConfigInspection` types.

use std::collections::HashMap;

mod env_mask;
mod inspection;
mod json;

pub use env_mask::{mask_mcp_env, MASKED_ENV_VALUE};
pub use inspection::{
    inspect_mcp_config_content, McpConfigInspection, McpConfigStatus, McpServerStatus,
    McpServerSummary, McpServerTransport,
};
pub use json::{parse_json, js_string_of, JsonNumber, JsonValue};

// ---------------------------------------------------------------------------
// O:1 McpConfigFormat
// ---------------------------------------------------------------------------

/// `O:1` — `'workspace' | 'cursor' | 'claude'`. NOT a unique key: `claude`
/// appears twice in [`MCP_CONFIG_CANDIDATES`] (see its doc comment).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpConfigFormat {
    Workspace,
    Cursor,
    Claude,
}

// ---------------------------------------------------------------------------
// O:3-8 McpConfigCandidate
// ---------------------------------------------------------------------------

/// `O:3-8`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpConfigCandidate {
    pub format: McpConfigFormat,
    pub label: &'static str,
    pub relative_path: &'static str,
    pub servers_path: &'static [&'static str],
}

// ---------------------------------------------------------------------------
// O:10-13 McpConfigDirectoryEntry
// ---------------------------------------------------------------------------

/// `O:10-13`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpConfigDirectoryEntry {
    pub name: String,
    pub is_directory: bool,
}

// ---------------------------------------------------------------------------
// O:36-61 MCP_CONFIG_CANDIDATES
// ---------------------------------------------------------------------------

/// `O:36-61` — order is contractual (consumed by [`get_mcp_config_parent_dirs`]
/// for insertion-order dedup, and mirrored by the oracle). `claude` appears
/// TWICE (index 2 `.claude.json`, index 3 `.claude/mcp.json`); identity is
/// `relative_path`, never `format`. All four share `servers_path`
/// `["mcpServers"]` — the three "formats" are relative-path differences, not
/// schema differences.
pub const MCP_CONFIG_CANDIDATES: [McpConfigCandidate; 4] = [
    McpConfigCandidate {
        format: McpConfigFormat::Workspace,
        label: "Workspace",
        relative_path: ".mcp.json",
        servers_path: &["mcpServers"],
    },
    McpConfigCandidate {
        format: McpConfigFormat::Cursor,
        label: "Cursor",
        relative_path: ".cursor/mcp.json",
        servers_path: &["mcpServers"],
    },
    McpConfigCandidate {
        format: McpConfigFormat::Claude,
        label: "Claude",
        relative_path: ".claude.json",
        servers_path: &["mcpServers"],
    },
    McpConfigCandidate {
        format: McpConfigFormat::Claude,
        label: "Claude workspace",
        relative_path: ".claude/mcp.json",
        servers_path: &["mcpServers"],
    },
];

// ---------------------------------------------------------------------------
// O:63-66 MCP_STARTER_CONFIG
// ---------------------------------------------------------------------------

/// `O:63-66` — exact bytes including the trailing newline. Validity of this
/// content against `inspectMcpConfigContent` is an M2 oracle concern
/// (`T:136-142`); M1 only exposes the constant.
pub const MCP_STARTER_CONFIG: &str = "{\n  \"mcpServers\": {}\n}\n";

// ---------------------------------------------------------------------------
// O:158-162 getRelativeParentDir / O:164-168 getRelativeBasename
// ---------------------------------------------------------------------------

/// `O:158-162` — hand-rolled, NOT `std::path` and NOT `suaegi-path`'s
/// `get_runtime_path_basename`/normalizer (contract decision V1): replace
/// EVERY `\` with `/` (a plain full replacement, not separator collapsing),
/// then find the LAST `/`. No trailing-slash trimming, no `.`/`..`
/// resolution, no `/+` collapsing. Slicing is always on the single-byte ASCII
/// `/` boundary, so this never cuts a multi-byte `char` in half.
fn relative_parent_dir(relative_path: &str) -> String {
    let normalized = replace_backslashes(relative_path);
    match normalized.rfind('/') {
        None => String::new(),
        Some(separator_index) => normalized[..separator_index].to_string(),
    }
}

/// `O:164-168` — same preamble as [`relative_parent_dir`]. If there is no
/// separator, the basename is the WHOLE replaced string (not the original
/// value, though they only ever differ by `\`->`/`). This deliberately
/// differs from `suaegi-path::get_runtime_path_basename`, which trims a
/// trailing separator first: for `"a/"`, that helper would say `"a"`; this
/// one says `""`, because Orca never trims here (contract decision V1/V5).
fn relative_basename(relative_path: &str) -> String {
    let normalized = replace_backslashes(relative_path);
    match normalized.rfind('/') {
        None => normalized,
        Some(separator_index) => normalized[separator_index + 1..].to_string(),
    }
}

/// `O:159`/`O:165` — `relativePath.replace(/\\/g, '/')`: replace every `\`
/// with `/`. A plain char-for-char substitution (not a regex collapse of
/// runs), so `\\\\` becomes `//`, not `/`.
fn replace_backslashes(value: &str) -> String {
    value
        .chars()
        .map(|c| if c == '\\' { '/' } else { c })
        .collect()
}

// ---------------------------------------------------------------------------
// O:68-78 getMcpConfigParentDirs
// ---------------------------------------------------------------------------

/// `O:68-78` — `Array.from(new Set(candidates.map(...).filter(...)))`: the
/// JS `Set` preserves FIRST-INSERTION order. Contract decision V3 forbids a
/// sorted set here — for [`MCP_CONFIG_CANDIDATES`] this must yield
/// `[".cursor", ".claude"]`, which is NOT alphabetical order.
pub fn get_mcp_config_parent_dirs(candidates: &[McpConfigCandidate]) -> Vec<String> {
    let mut parent_dirs: Vec<String> = Vec::new();
    for candidate in candidates {
        let parent_dir = relative_parent_dir(candidate.relative_path);
        if parent_dir.is_empty() {
            continue;
        }
        if !parent_dirs.contains(&parent_dir) {
            parent_dirs.push(parent_dir);
        }
    }
    parent_dirs
}

// ---------------------------------------------------------------------------
// O:80-82 getMcpConfigCandidateParentDir
// ---------------------------------------------------------------------------

/// `O:80-82`.
pub fn get_mcp_config_candidate_parent_dir(candidate: &McpConfigCandidate) -> String {
    relative_parent_dir(candidate.relative_path)
}

// ---------------------------------------------------------------------------
// O:84-94 selectExistingMcpConfigCandidates
// ---------------------------------------------------------------------------

/// `O:84-94` — preserves candidate order and returns ALL existing candidates
/// (contract decision V6): no single-winner selection, no merge, no
/// override. A missing parent-dir key is treated as an empty entry list.
/// Matching is exact and case-sensitive: `entry.name == basename &&
/// !entry.is_directory`. Entry order within a directory is irrelevant to the
/// result.
pub fn select_existing_mcp_config_candidates(
    entries_by_relative_dir: &HashMap<String, Vec<McpConfigDirectoryEntry>>,
    candidates: &[McpConfigCandidate],
) -> Vec<McpConfigCandidate> {
    let empty_entries: Vec<McpConfigDirectoryEntry> = Vec::new();
    candidates
        .iter()
        .filter(|candidate| {
            let parent_dir = relative_parent_dir(candidate.relative_path);
            let basename = relative_basename(candidate.relative_path);
            let entries = entries_by_relative_dir
                .get(&parent_dir)
                .unwrap_or(&empty_entries);
            entries
                .iter()
                .any(|entry| entry.name == basename && !entry.is_directory)
        })
        .copied()
        .collect()
}

// ---------------------------------------------------------------------------
// O:96-101 canInspectLocalMcpConfigRoot
// ---------------------------------------------------------------------------

/// `O:96-101` — hand-rolled, no `regex` crate (contract decision V4). On a
/// Windows host, always inspectable (short-circuit `true`). Otherwise NOT
/// inspectable only when `root_path` starts with a drive prefix or a UNC
/// prefix; both checks are anchored at the string start only (no `/m`
/// equivalent — there is nothing to anchor against here since we only test a
/// prefix).
pub fn can_inspect_local_mcp_config_root(root_path: &str, is_windows_host: bool) -> bool {
    if is_windows_host {
        return true;
    }
    !(matches_drive_prefix(root_path) || matches_unc_prefix(root_path))
}

/// `O:100` first alternative: `[A-Za-z]:[\\/]` — one ASCII letter, then `:`,
/// then exactly one separator (`\` or `/`). Nothing is required after that:
/// `"C:/repo"` matches (starts with `C:/`), but `"C:"` alone (only 2 bytes)
/// does not — it stays inspectable.
fn matches_drive_prefix(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && is_separator_byte(bytes[2])
}

/// `O:100` second alternative: `[\\/]{2}[^\\/]+[\\/][^\\/]+` — EXACTLY two
/// separators, then one-or-more non-separators (the "server" segment), then
/// one separator, then one-or-more non-separators (the "share" segment).
/// `{2}` is a fixed count with no backtracking: if a third separator
/// immediately follows the first two (e.g. `"///a/b"`), the following
/// `[^\\/]+` fails to match at that position and the whole alternative fails.
/// `"\\\\server"` (two separators, then a server segment, then nothing else)
/// also fails: there is no closing separator + share segment.
fn matches_unc_prefix(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() < 2 || !is_separator_byte(bytes[0]) || !is_separator_byte(bytes[1]) {
        return false;
    }

    let mut index = 2;
    let server_start = index;
    while index < bytes.len() && !is_separator_byte(bytes[index]) {
        index += 1;
    }
    if index == server_start {
        return false; // `[^\\/]+` needs at least one non-separator byte.
    }
    if index >= bytes.len() || !is_separator_byte(bytes[index]) {
        return false; // needs exactly one separator here.
    }
    index += 1;

    let share_start = index;
    while index < bytes.len() && !is_separator_byte(bytes[index]) {
        index += 1;
    }
    index != share_start // `[^\\/]+` needs at least one non-separator byte.
}

fn is_separator_byte(byte: u8) -> bool {
    byte == b'\\' || byte == b'/'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries_map(
        pairs: Vec<(&str, Vec<(&str, bool)>)>,
    ) -> HashMap<String, Vec<McpConfigDirectoryEntry>> {
        pairs
            .into_iter()
            .map(|(dir, entries)| {
                (
                    dir.to_string(),
                    entries
                        .into_iter()
                        .map(|(name, is_directory)| McpConfigDirectoryEntry {
                            name: name.to_string(),
                            is_directory,
                        })
                        .collect(),
                )
            })
            .collect()
    }

    // -- Oracle test 8 (T:144-165) -------------------------------------------

    #[test]
    fn oracle_plans_directory_discovery_before_reading_candidate_files() {
        assert_eq!(
            get_mcp_config_parent_dirs(&MCP_CONFIG_CANDIDATES),
            vec![".cursor".to_string(), ".claude".to_string()]
        );

        let parent_dirs: Vec<String> = MCP_CONFIG_CANDIDATES
            .iter()
            .map(get_mcp_config_candidate_parent_dir)
            .collect();
        assert_eq!(
            parent_dirs,
            vec![
                String::new(),
                ".cursor".to_string(),
                String::new(),
                ".claude".to_string(),
            ]
        );

        let entries_by_relative_dir = entries_map(vec![
            (
                "",
                vec![(".mcp.json", false), (".cursor", true), (".claude", false)],
            ),
            (".cursor", vec![("mcp.json", false)]),
        ]);

        let labels: Vec<&str> =
            select_existing_mcp_config_candidates(&entries_by_relative_dir, &MCP_CONFIG_CANDIDATES)
                .iter()
                .map(|candidate| candidate.label)
                .collect();
        assert_eq!(labels, vec!["Workspace", "Cursor"]);
    }

    // -- Oracle test 9 (T:167-178) --------------------------------------------

    #[test]
    fn oracle_rejects_windows_only_local_roots_on_non_windows_hosts() {
        assert!(!can_inspect_local_mcp_config_root("C:\\repo", false));
        assert!(!can_inspect_local_mcp_config_root(
            "\\\\wsl.localhost\\Ubuntu\\home\\me\\repo",
            false
        ));
        assert!(!can_inspect_local_mcp_config_root(
            "//wsl.localhost/Ubuntu/home/me/repo",
            false
        ));
        assert!(can_inspect_local_mcp_config_root("/Users/me/repo", false));
        assert!(can_inspect_local_mcp_config_root(
            "\\\\wsl.localhost\\Ubuntu\\home\\me\\repo",
            true
        ));
        assert!(can_inspect_local_mcp_config_root(
            "//wsl.localhost/Ubuntu/home/me/repo",
            true
        ));
    }

    // -- V1: hand-rolled parent/basename split (no std::path, no suaegi-path) -

    #[test]
    fn v1_trailing_slash_basename_is_empty_not_trimmed() {
        // Crux pin: suaegi-path's `get_runtime_path_basename("a/")` would
        // trim the trailing separator first and return "a". This module
        // never trims, so the basename of everything after the last `/` is
        // the empty string.
        assert_eq!(relative_basename("a/"), "");
        assert_eq!(relative_parent_dir("a/"), "a");
    }

    #[test]
    fn v1_no_separator_basename_is_whole_string() {
        assert_eq!(relative_basename("plain.json"), "plain.json");
        assert_eq!(relative_parent_dir("plain.json"), "");
    }

    #[test]
    fn v1_backslash_only_path_is_split_after_replacement() {
        assert_eq!(relative_parent_dir("a\\b"), "a");
        assert_eq!(relative_basename("a\\b"), "b");
        // Every backslash is replaced, not just the last one.
        assert_eq!(relative_parent_dir("a\\\\b"), "a/");
        assert_eq!(relative_basename("a\\\\b"), "b");
    }

    // -- V2: `claude` appears twice, keyed by relative_path not format -------

    #[test]
    fn v2_claude_format_appears_twice_with_distinct_relative_paths() {
        let claude_entries: Vec<&McpConfigCandidate> = MCP_CONFIG_CANDIDATES
            .iter()
            .filter(|candidate| candidate.format == McpConfigFormat::Claude)
            .collect();
        assert_eq!(claude_entries.len(), 2);
        assert_eq!(claude_entries[0].relative_path, ".claude.json");
        assert_eq!(claude_entries[1].relative_path, ".claude/mcp.json");
        assert_ne!(
            claude_entries[0].relative_path,
            claude_entries[1].relative_path
        );
    }

    // -- V3: insertion-order dedup, explicitly NOT sorted --------------------

    #[test]
    fn v3_parent_dirs_preserve_insertion_order_not_sorted() {
        let result = get_mcp_config_parent_dirs(&MCP_CONFIG_CANDIDATES);
        assert_eq!(result, vec![".cursor".to_string(), ".claude".to_string()]);
        // A sorted (e.g. BTreeSet-backed) implementation would produce
        // [".claude", ".cursor"] instead, since 'c'+'l' < 'c'+'u'. Assert the
        // two orders differ so this test fails if dedup is ever swapped for
        // a sorted set.
        let sorted = {
            let mut sorted = result.clone();
            sorted.sort();
            sorted
        };
        assert_ne!(result, sorted);
    }

    // -- V4: hand-rolled Windows-root prefix, no regex crate -----------------

    #[test]
    fn v4_bare_drive_letter_without_separator_is_inspectable() {
        assert!(can_inspect_local_mcp_config_root("C:", false));
    }

    #[test]
    fn v4_unc_without_share_segment_is_inspectable() {
        assert!(can_inspect_local_mcp_config_root("\\\\server", false));
    }

    #[test]
    fn v4_unc_with_empty_share_segment_is_inspectable() {
        // Crux pin: two separators, then a server segment, then one
        // separator, then NOTHING. `[^\\/]+` needs one-or-more non-separator
        // bytes for the share segment, so an empty share does not satisfy
        // the UNC alternative, and the path stays inspectable. Contrast with
        // a non-empty share segment (`"//server/share"`), which IS a UNC
        // match and is therefore not inspectable.
        assert!(can_inspect_local_mcp_config_root("\\\\server\\", false));
        assert!(can_inspect_local_mcp_config_root("//server/", false));
        assert!(!can_inspect_local_mcp_config_root("//server/share", false));
    }

    #[test]
    fn v4_triple_separator_does_not_backtrack_into_unc_match() {
        assert!(can_inspect_local_mcp_config_root("///a/b", false));
    }

    #[test]
    fn v4_forward_slash_drive_prefix_is_not_inspectable() {
        assert!(!can_inspect_local_mcp_config_root("C:/repo", false));
    }

    #[test]
    fn v4_windows_host_short_circuits_to_inspectable_regardless_of_path() {
        assert!(can_inspect_local_mcp_config_root("C:\\repo", true));
        assert!(can_inspect_local_mcp_config_root("not even a path", true));
    }

    // -- V5: no trimming anywhere in this module ------------------------------

    #[test]
    fn v5_space_padded_relative_path_is_not_trimmed() {
        assert_eq!(relative_parent_dir("  dir  /  file.json  "), "  dir  ");
        assert_eq!(relative_basename("  dir  /  file.json  "), "  file.json  ");
    }

    // -- V6: entry order irrelevant; is_directory excludes a name match ------

    #[test]
    fn v6_entry_order_within_a_directory_does_not_change_the_result() {
        let forward = entries_map(vec![(
            "",
            vec![
                (".mcp.json", false),
                (".claude.json", false),
                (".cursor", true),
            ],
        )]);
        let reversed = entries_map(vec![(
            "",
            vec![
                (".cursor", true),
                (".claude.json", false),
                (".mcp.json", false),
            ],
        )]);

        let labels = |map: &HashMap<String, Vec<McpConfigDirectoryEntry>>| -> Vec<&str> {
            select_existing_mcp_config_candidates(map, &MCP_CONFIG_CANDIDATES)
                .iter()
                .map(|candidate| candidate.label)
                .collect()
        };

        assert_eq!(labels(&forward), labels(&reversed));
        assert_eq!(labels(&forward), vec!["Workspace", "Claude"]);
    }

    #[test]
    fn v6_is_directory_true_does_not_match_even_with_exact_name() {
        let entries_by_relative_dir = entries_map(vec![("", vec![(".mcp.json", true)])]);
        let result =
            select_existing_mcp_config_candidates(&entries_by_relative_dir, &MCP_CONFIG_CANDIDATES);
        assert!(result.is_empty());
    }

    // -- V7: MCP_STARTER_CONFIG byte-exact contents ---------------------------

    #[test]
    fn v7_starter_config_is_byte_exact() {
        // Pins the two-space indent and the trailing newline together, so
        // dropping either one fails this test.
        assert_eq!(MCP_STARTER_CONFIG, "{\n  \"mcpServers\": {}\n}\n");
        assert!(MCP_STARTER_CONFIG.ends_with("}\n"));
    }
}
