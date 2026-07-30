//! VERBATIM port of the summary/inspection layer of Orca's
//! `src/shared/mcp-config.ts` (@ v1.4.150-rc.0), milestone M2b — the final
//! layer of this module.
//!
//! Ported: the [`McpServerTransport`]/[`McpServerStatus`]/[`McpServerSummary`]/
//! [`McpConfigInspection`] types (`O:15-34`), [`inspect_mcp_config_content`]
//! (`O:108-140`), and the private `extract_object_at_path` (`O:170-184`),
//! `summarize_mcp_server` (`O:186-241`), `read_command` (`O:243-251`),
//! `read_url` (`O:253-261`), and `resolve_transport` (`O:263-275`).
//!
//! Builds on M1 (`crate::McpConfigCandidate`, `src/lib.rs`) and M2a
//! (`crate::json::{parse_json, JsonValue}`, `crate::env_mask::mask_mcp_env`,
//! `src/json.rs` + `src/env_mask.rs`).

use crate::env_mask::mask_mcp_env;
use crate::json::{parse_json, JsonValue};
use crate::McpConfigCandidate;

// ---------------------------------------------------------------------------
// O:15-34 types
// ---------------------------------------------------------------------------

/// `O:15` — `'stdio' | 'http' | 'unknown'`.
///
/// # X2 — NOT modeled after the raw `type` field
/// This is the RESOLVED transport ([`resolve_transport`]'s return value), not
/// a direct reflection of the JSON `type` string. `resolve_transport` never
/// promotes the raw `type` value to this enum via an exhaustive mapping — it
/// compares exactly three string literals and otherwise falls through to
/// presence inference. Do not be misled by the enum shape into thinking
/// unknown `type` strings get their own variant; they all collapse through
/// the fallthrough path to one of these three.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpServerTransport {
    Stdio,
    Http,
    Unknown,
}

/// `O:16` — a single server entry's enabled/disabled/invalid state.
///
/// # X11 — a DIFFERENT type from [`McpConfigInspection`]'s `status`
/// Both are named "status" in the TS source and both happen to have 3
/// variants, but they are unrelated enums over unrelated domains: this one
/// describes one server entry; [`McpConfigInspection`]'s `status` describes
/// the whole file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpServerStatus {
    Enabled,
    Disabled,
    Invalid,
}

/// `O:18-26`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerSummary {
    pub name: String,
    pub transport: McpServerTransport,
    pub status: McpServerStatus,
    pub command: Option<String>,
    pub url: Option<String>,
    pub env: Option<Vec<(String, String)>>,
    pub issue: Option<String>,
}

/// `O:28-34`.
///
/// # X11 — `status` here is `missing | valid | invalid`
/// A DIFFERENT 3-variant enum from [`McpServerStatus`] (see its doc comment).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpConfigStatus {
    Missing,
    Valid,
    Invalid,
}

/// `O:28-34`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpConfigInspection {
    pub candidate: McpConfigCandidate,
    pub exists: bool,
    pub status: McpConfigStatus,
    pub servers: Vec<McpServerSummary>,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// X4 — falsy, not absent
// ---------------------------------------------------------------------------

/// `!url`/`!command` in JS is JS-falsy, which the empty string satisfies too
/// — it is NOT the same test as "is this field absent". Four call sites rely
/// on this: `O:213`, `O:223`, `O:268`, `O:271`. Never write `Option::is_none`
/// where the JS source writes a bare `!x` on a string-or-undefined value.
fn is_falsy(value: Option<&str>) -> bool {
    value.is_none_or(str::is_empty)
}

/// Plain linear lookup by key into a parsed object's entries. Not a `HashMap`
/// — these objects are always small (a handful of server-entry fields), and
/// keeping them as `Vec<(String, JsonValue)>` all the way through (rather
/// than re-indexing into a map at this layer) matches how `crate::json` and
/// `crate::env_mask` already carry order-preserving objects.
fn get_field<'a>(raw: &'a [(String, JsonValue)], key: &str) -> Option<&'a JsonValue> {
    raw.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

// ---------------------------------------------------------------------------
// O:170-184 extractObjectAtPath
// ---------------------------------------------------------------------------

/// `O:170-184`.
///
/// # X1 — plain `get`, no hardening, and a miss is `valid`
/// This is a bare walk through nested objects by key. It deliberately does
/// NOT special-case `__proto__`/`constructor`/`prototype`: Orca recon (base
/// M2 plan) confirmed by direct measurement that `JSON.parse` always
/// materializes `__proto__` as an own data property (never as a prototype
/// mutation), that a genuinely absent key reads back as `undefined` (not
/// `Object.prototype`'s own enumerable content, since `Object.entries` on the
/// eventual result only ever sees own properties), and that `constructor`/
/// `toString` are functions that fail the `typeof === 'object'`/non-array
/// guard below exactly like any other wrong-shaped value. A `Vec`-backed
/// lookup is output-identical to the JS `Record` indexing here; adding a
/// denylist would only make this port diverge from the source it is meant to
/// mirror. See [`inspect_mcp_config_content`] for the equally
/// counter-intuitive consequence: when this returns `None`, the caller's
/// status is `valid` with zero servers, never `invalid`.
fn extract_object_at_path<'a>(
    value: &'a JsonValue,
    path_segments: &[&str],
) -> Option<&'a Vec<(String, JsonValue)>> {
    let mut current: Option<&'a JsonValue> = Some(value);
    for segment in path_segments {
        current = match current {
            Some(JsonValue::Object(entries)) => {
                entries.iter().find(|(k, _)| k == segment).map(|(_, v)| v)
            }
            _ => None,
        };
    }
    match current {
        Some(JsonValue::Object(entries)) => Some(entries),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// O:243-251 readCommand / O:253-261 readUrl
// ---------------------------------------------------------------------------

/// `O:243-251`.
///
/// # X3 — asymmetric with [`read_url`]; `[0]` only, never a scan
/// A string `command` is returned as-is. An array `command` is read at index
/// `[0]` ONLY, and only if that element is itself a string — `[1, "npx"]`
/// does NOT get treated as "the first string element", it returns `None`
/// (matching `typeof raw.command[0] === 'string'` failing at index 0, full
/// stop — the source never scans past index 0). `[]` has no index 0, so also
/// `None`. `[""]` has a string at index 0, so it returns `Some(String::new())`
/// — an empty string is a valid (if falsy — see [`is_falsy`]) command value,
/// not a missing one.
fn read_command(raw: &[(String, JsonValue)]) -> Option<String> {
    match get_field(raw, "command") {
        Some(JsonValue::String(s)) => Some(s.clone()),
        Some(JsonValue::Array(items)) => match items.first() {
            Some(JsonValue::String(s)) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// `O:253-261`.
///
/// # X3 — asymmetric with [`read_command`]; no array form at all
/// `url` wins over `httpUrl` when both are strings. There is NO array-reading
/// branch here, unlike `read_command` — `{"url": ["x"]}` is `None`, not
/// `Some("x")`. Do not unify these two readers behind one helper.
fn read_url(raw: &[(String, JsonValue)]) -> Option<String> {
    if let Some(JsonValue::String(s)) = get_field(raw, "url") {
        return Some(s.clone());
    }
    if let Some(JsonValue::String(s)) = get_field(raw, "httpUrl") {
        return Some(s.clone());
    }
    None
}

// ---------------------------------------------------------------------------
// O:263-275 resolveTransport
// ---------------------------------------------------------------------------

/// `O:263-275`.
///
/// # X2 — tolerant fallthrough, `type` read as `Option<&str>`, `url`-first
/// `raw.type` is compared against exactly three string literals
/// (`"http"`/`"remote"` -> http, `"local"` -> stdio) using `===`, which in JS
/// is false for every non-string value (a number, `null`, an object, absent)
/// as well as for every OTHER string (`"sse"`, `"HTTP"`, `"stdio"`, ...) — all
/// of those fall through to presence inference instead of erroring or
/// defaulting to `unknown` outright. Modeling `type` as a Rust enum (even
/// with a catch-all variant) would make it tempting to branch on that
/// catch-all specifically; reading it as a plain `Option<&str>` and doing
/// three literal `==` checks keeps the fallthrough shape identical to the
/// source. `url` is checked in the FIRST `if`, before `command` is ever
/// consulted — so `{command, url}` resolves to `http` (url wins even though a
/// command is also present), and `{"type": "local", url}` ALSO resolves to
/// `http` (a present truthy `url` overrides an explicit `type: "local"`).
fn resolve_transport(
    raw: &[(String, JsonValue)],
    command: Option<&str>,
    url: Option<&str>,
) -> McpServerTransport {
    let type_value = match get_field(raw, "type") {
        Some(JsonValue::String(s)) => Some(s.as_str()),
        _ => None,
    };

    if type_value == Some("http") || type_value == Some("remote") || !is_falsy(url) {
        return McpServerTransport::Http;
    }
    if type_value == Some("local") || !is_falsy(command) {
        return McpServerTransport::Stdio;
    }
    McpServerTransport::Unknown
}

// ---------------------------------------------------------------------------
// O:186-241 summarizeMcpServer
// ---------------------------------------------------------------------------

/// `O:186-241`.
///
/// # X11 — the four verbatim issue strings; the first omits `env`
/// `"Server entry must be an object."` is returned for a non-object entry
/// (`null`, an array, or a primitive) — and, unlike every other return in
/// this function, that branch never calls [`mask_mcp_env`] at all (there is
/// no `raw` to read an `env` field from), so `env` is always `None` there,
/// not merely "masked to nothing". The other three issue strings —
/// `"Missing command or URL."`, `"Missing URL."`, `"Missing command."` — are
/// each attached to a summary that DOES carry `env` (computed at `O:201`,
/// consumed for every remaining branch including these).
///
/// # X6 — the invalid branches are decided before enabled/disabled is used
/// `enabled` is computed at `O:200`, but the three invalid-transport/target
/// checks (`O:203`, `O:213`, `O:223`) run first and return early; the
/// `enabled ? 'enabled' : 'disabled'` ternary (`O:236`) is only reached if
/// none of them fired. So an entry that LOOKS disabled (`"enabled": false`)
/// but is also missing its command/URL never reaches "disabled" — it comes
/// back `invalid`.
///
/// # X10 — nothing is trimmed, `args` is read by nothing
/// There is no `.trim()` anywhere in this function or its helpers, and no
/// code path reads an `args` field at all — `command: "  npx  "` is returned
/// byte-for-byte, and an `args` array/value of any shape is inert.
fn summarize_mcp_server(name: &str, entry: &JsonValue) -> McpServerSummary {
    let raw = match entry {
        JsonValue::Object(entries) => entries,
        _ => {
            return McpServerSummary {
                name: name.to_string(),
                transport: McpServerTransport::Unknown,
                status: McpServerStatus::Invalid,
                command: None,
                url: None,
                env: None,
                issue: Some("Server entry must be an object.".to_string()),
            };
        }
    };

    let command = read_command(raw);
    let url = read_url(raw);
    let transport = resolve_transport(raw, command.as_deref(), url.as_deref());
    // X5 — strict comparison, never a truthiness helper: `enabled: 0` /
    // `null` / `"false"` are all still enabled; `disabled: 1` / `"yes"` are
    // still enabled. Only the literal booleans `false`/`true` flip anything.
    let enabled = !matches!(get_field(raw, "enabled"), Some(JsonValue::Bool(false)))
        && !matches!(get_field(raw, "disabled"), Some(JsonValue::Bool(true)));
    let env = get_field(raw, "env").and_then(mask_mcp_env);

    if transport == McpServerTransport::Unknown {
        return McpServerSummary {
            name: name.to_string(),
            transport,
            status: McpServerStatus::Invalid,
            command: None,
            url: None,
            env,
            issue: Some("Missing command or URL.".to_string()),
        };
    }

    if transport == McpServerTransport::Http && is_falsy(url.as_deref()) {
        return McpServerSummary {
            name: name.to_string(),
            transport,
            status: McpServerStatus::Invalid,
            command: None,
            url: None,
            env,
            issue: Some("Missing URL.".to_string()),
        };
    }

    if transport == McpServerTransport::Stdio && is_falsy(command.as_deref()) {
        return McpServerSummary {
            name: name.to_string(),
            transport,
            status: McpServerStatus::Invalid,
            command: None,
            url: None,
            env,
            issue: Some("Missing command.".to_string()),
        };
    }

    McpServerSummary {
        name: name.to_string(),
        transport,
        status: if enabled {
            McpServerStatus::Enabled
        } else {
            McpServerStatus::Disabled
        },
        command,
        url,
        env,
        issue: None,
    }
}

// ---------------------------------------------------------------------------
// O:108-140 inspectMcpConfigContent
// ---------------------------------------------------------------------------

/// `O:108-140`.
///
/// # X7 — `content: Option<&str>`; `Some("")` is not missing
/// `O:112`'s check is `content === null`, i.e. "was there a read result at
/// all", not "is it non-empty". An empty string is a real (if degenerate)
/// read result and flows into `JSON.parse('')`, which fails, producing
/// `status: 'invalid'`. A real caller relies on this (`mcp-config-inspection`
/// passes `''` for an unreadable-as-text file) — collapsing `Some("")` into
/// the `None`/missing branch would silently reclassify those files.
///
/// # X8 — sanctioned divergence in the parse-error message
/// The oracle (`T:27`) requires `result.error` to contain the substring
/// `"JSON"`. `serde_json`'s own message text does not contain that substring
/// (e.g. `"EOF while parsing a value at line 1 column 1"`), so byte-for-byte
/// forwarding `error.to_string()` would fail the oracle outright. Faithfully
/// reproducing V8's own message instead is not an option either: V8's
/// `SyntaxError` for `JSON.parse` echoes back a chunk of the *original input*
/// (observed up to ~20 characters, e.g. parsing
/// `{"apiKey":"sk-live-SUPERSECRET-abcdef","x":@}` yields a message
/// containing `"...cdef","x":@}" is not valid JSON'` — meaning Orca's own
/// error message can leak fragments of the config file's contents to
/// whatever surface renders it, even though this test's own name
/// ('reports invalid JSON without exposing file contents') asserts the
/// opposite intent). This port takes the position that the STATED intent is
/// the one worth honoring: `format!("Invalid JSON at line {} column {}",
/// e.line(), e.column())` satisfies the oracle's `toContain('JSON')` check
/// AND never echoes any input byte, at the cost of not being a literal
/// transliteration of either runtime's message text. This string reaches the
/// user (rendered by `McpConfigFileRow.tsx`), so the divergence is
/// user-visible by design, not an accident.
///
/// # X9 — `serde_json` is stricter than `JSON.parse` in three known ways
/// Each of the following makes `serde_json::from_str` return `Err` for input
/// that `JSON.parse` accepts, flipping a file from `valid` to `invalid`
/// relative to Orca on that input alone: (1) a lone UTF-16 surrogate escape
/// such as `"\uD800"` with no matching low surrogate (`JSON.parse` allows an
/// unpaired surrogate in a JS string; `serde_json` requires valid UTF-8 and
/// rejects it); (2) an out-of-range number literal like `1e999` (`JSON.parse`
/// coerces this to the `Infinity` double per ECMA-262; `serde_json` has no
/// non-finite JSON representation and errors); (3) deeply nested arrays/
/// objects (V8 currently tolerates recursion past roughly 128 levels;
/// `serde_json`'s default recursion limit is 128 and errors beyond it). None
/// of these are exercised by the oracle; they are documented here, not
/// worked around, per the same accepted-divergence precedent as
/// `pull_request_generation.rs`.
pub fn inspect_mcp_config_content(
    candidate: McpConfigCandidate,
    content: Option<&str>,
) -> McpConfigInspection {
    let content = match content {
        None => {
            return McpConfigInspection {
                candidate,
                exists: false,
                status: McpConfigStatus::Missing,
                servers: Vec::new(),
                error: None,
            };
        }
        Some(content) => content,
    };

    let parsed = match parse_json(content) {
        Ok(parsed) => parsed,
        Err(error) => {
            return McpConfigInspection {
                candidate,
                exists: true,
                status: McpConfigStatus::Invalid,
                servers: Vec::new(),
                error: Some(format!(
                    "Invalid JSON at line {} column {}",
                    error.line(),
                    error.column()
                )),
            };
        }
    };

    let raw_servers = extract_object_at_path(&parsed, candidate.servers_path);
    let servers = match raw_servers {
        None => Vec::new(),
        Some(entries) => entries
            .iter()
            .map(|(name, entry)| summarize_mcp_server(name, entry))
            .collect(),
    };

    McpConfigInspection {
        candidate,
        exists: true,
        status: McpConfigStatus::Valid,
        servers,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MCP_CONFIG_CANDIDATES;

    fn workspace_candidate() -> McpConfigCandidate {
        MCP_CONFIG_CANDIDATES[0]
    }

    fn object_entries(json: &str) -> Vec<(String, JsonValue)> {
        match parse_json(json).expect("valid JSON") {
            JsonValue::Object(entries) => entries,
            other => panic!("expected an object, got {other:?}"),
        }
    }

    fn summarize(json: &str) -> McpServerSummary {
        let parsed = parse_json(json).expect("valid JSON");
        summarize_mcp_server("s", &parsed)
    }

    // -- Oracle test 1 (T:16-22) ----------------------------------------------

    #[test]
    fn oracle_reports_missing_configs() {
        let result = inspect_mcp_config_content(workspace_candidate(), None);
        assert!(!result.exists);
        assert_eq!(result.status, McpConfigStatus::Missing);
        assert_eq!(result.servers, Vec::new());
    }

    // -- Oracle test 2 (T:24-29) -----------------------------------------------

    #[test]
    fn oracle_reports_invalid_json_without_exposing_file_contents() {
        let result = inspect_mcp_config_content(workspace_candidate(), Some("{"));
        assert_eq!(result.status, McpConfigStatus::Invalid);
        assert!(result.error.as_deref().unwrap().contains("JSON"));
        assert_eq!(result.servers, Vec::new());
    }

    // -- Oracle test 3 (T:31-76) ------------------------------------------------

    #[test]
    fn oracle_summarizes_stdio_http_disabled_and_invalid_servers() {
        let content = r#"{
            "mcpServers": {
                "filesystem": {
                    "command": "npx",
                    "args": ["-y", "@modelcontextprotocol/server-filesystem"],
                    "env": { "NODE_ENV": "production", "API_TOKEN": "secret-token" }
                },
                "docs": { "type": "http", "url": "https://example.com/mcp" },
                "old": { "command": "node", "enabled": false },
                "broken": { "args": ["missing-command"] }
            }
        }"#;
        let result = inspect_mcp_config_content(workspace_candidate(), Some(content));

        assert_eq!(result.status, McpConfigStatus::Valid);
        assert_eq!(
            result.servers,
            vec![
                McpServerSummary {
                    name: "filesystem".to_string(),
                    transport: McpServerTransport::Stdio,
                    status: McpServerStatus::Enabled,
                    command: Some("npx".to_string()),
                    url: None,
                    env: Some(vec![
                        ("NODE_ENV".to_string(), "production".to_string()),
                        ("API_TOKEN".to_string(), "\u{2022}".repeat(8)),
                    ]),
                    issue: None,
                },
                McpServerSummary {
                    name: "docs".to_string(),
                    transport: McpServerTransport::Http,
                    status: McpServerStatus::Enabled,
                    command: None,
                    url: Some("https://example.com/mcp".to_string()),
                    env: None,
                    issue: None,
                },
                McpServerSummary {
                    name: "old".to_string(),
                    transport: McpServerTransport::Stdio,
                    status: McpServerStatus::Disabled,
                    command: Some("node".to_string()),
                    url: None,
                    env: None,
                    issue: None,
                },
                McpServerSummary {
                    name: "broken".to_string(),
                    transport: McpServerTransport::Unknown,
                    status: McpServerStatus::Invalid,
                    command: None,
                    url: None,
                    env: None,
                    issue: Some("Missing command or URL.".to_string()),
                },
            ]
        );
    }

    // -- Oracle test 4 (T:78-93) ------------------------------------------------

    #[test]
    fn oracle_supports_agent_specific_command_and_url_shapes_from_common_adapters() {
        let content = r#"{
            "mcpServers": {
                "opencodeLocal": { "type": "local", "command": ["uvx", "server"] },
                "geminiRemote": { "httpUrl": "https://example.com/sse" }
            }
        }"#;
        let result = inspect_mcp_config_content(workspace_candidate(), Some(content));

        assert_eq!(result.servers.len(), 2);
        assert_eq!(result.servers[0].name, "opencodeLocal");
        assert_eq!(result.servers[0].transport, McpServerTransport::Stdio);
        assert_eq!(result.servers[0].command.as_deref(), Some("uvx"));
        assert_eq!(result.servers[1].name, "geminiRemote");
        assert_eq!(result.servers[1].transport, McpServerTransport::Http);
        assert_eq!(
            result.servers[1].url.as_deref(),
            Some("https://example.com/sse")
        );
    }

    // -- Oracle test 5 (T:95-120) -----------------------------------------------

    #[test]
    fn oracle_marks_declared_transports_without_their_target_as_invalid() {
        let content = r#"{
            "mcpServers": {
                "remoteMissingUrl": { "type": "http" },
                "localMissingCommand": { "type": "local" }
            }
        }"#;
        let result = inspect_mcp_config_content(workspace_candidate(), Some(content));

        assert_eq!(
            result.servers,
            vec![
                McpServerSummary {
                    name: "remoteMissingUrl".to_string(),
                    transport: McpServerTransport::Http,
                    status: McpServerStatus::Invalid,
                    command: None,
                    url: None,
                    env: None,
                    issue: Some("Missing URL.".to_string()),
                },
                McpServerSummary {
                    name: "localMissingCommand".to_string(),
                    transport: McpServerTransport::Stdio,
                    status: McpServerStatus::Invalid,
                    command: None,
                    url: None,
                    env: None,
                    issue: Some("Missing command.".to_string()),
                },
            ]
        );
    }

    // -- Oracle test 7 (T:136-142) ------------------------------------------------

    #[test]
    fn oracle_keeps_starter_config_valid_and_empty() {
        let result =
            inspect_mcp_config_content(workspace_candidate(), Some(crate::MCP_STARTER_CONFIG));
        assert!(result.exists);
        assert_eq!(result.status, McpConfigStatus::Valid);
        assert_eq!(result.servers, Vec::new());
    }

    // -- X1: extractObjectAtPath is a plain get; a miss is `valid`, not `invalid` -

    #[test]
    fn x1_missing_mcp_servers_key_is_valid_with_zero_servers() {
        let result = inspect_mcp_config_content(workspace_candidate(), Some("{}"));
        assert_eq!(result.status, McpConfigStatus::Valid);
        assert_eq!(result.servers, Vec::new());
    }

    #[test]
    fn x1_null_mcp_servers_is_valid_with_zero_servers() {
        let result =
            inspect_mcp_config_content(workspace_candidate(), Some(r#"{"mcpServers": null}"#));
        assert_eq!(result.status, McpConfigStatus::Valid);
        assert_eq!(result.servers, Vec::new());
    }

    #[test]
    fn x1_array_mcp_servers_is_valid_with_zero_servers() {
        // Crux pin: an array is JS-truthy and `typeof array === 'object'`, so
        // the ONLY thing that excludes it is the explicit `Array.isArray`
        // check. Getting that check backwards (or dropping it) would make
        // this return the array's own indices as bogus "servers".
        let result = inspect_mcp_config_content(
            workspace_candidate(),
            Some(r#"{"mcpServers": ["a", "b"]}"#),
        );
        assert_eq!(result.status, McpConfigStatus::Valid);
        assert_eq!(result.servers, Vec::new());
    }

    #[test]
    fn x1_string_mcp_servers_is_valid_with_zero_servers() {
        let result =
            inspect_mcp_config_content(workspace_candidate(), Some(r#"{"mcpServers": "oops"}"#));
        assert_eq!(result.status, McpConfigStatus::Valid);
        assert_eq!(result.servers, Vec::new());
    }

    #[test]
    fn x1_dunder_proto_key_is_a_plain_data_key_not_denylisted() {
        // Present: `__proto__` sits inside `mcpServers` as an ordinary
        // server name and must be summarized like any other key — a
        // hardened lookup that special-cases this key would silently drop
        // it instead.
        let with_proto = inspect_mcp_config_content(
            workspace_candidate(),
            Some(r#"{"mcpServers": {"__proto__": {"command": "npx"}}}"#),
        );
        assert_eq!(with_proto.servers.len(), 1);
        assert_eq!(with_proto.servers[0].name, "__proto__");
        assert_eq!(with_proto.servers[0].transport, McpServerTransport::Stdio);
        assert_eq!(with_proto.servers[0].status, McpServerStatus::Enabled);
        assert_eq!(with_proto.servers[0].command.as_deref(), Some("npx"));

        // Absent: the exact same config without a `__proto__` key at all
        // behaves identically for the key it DOES have — no ambient
        // prototype content leaks in as a phantom extra server.
        let without_proto = inspect_mcp_config_content(
            workspace_candidate(),
            Some(r#"{"mcpServers": {"real": {"command": "npx"}}}"#),
        );
        assert_eq!(without_proto.servers.len(), 1);
        assert_eq!(without_proto.servers[0].name, "real");
    }

    // -- X2: tolerant fallthrough + url-first ------------------------------------

    #[test]
    fn x2_type_remote_resolves_to_http() {
        let entries = object_entries(r#"{"type": "remote"}"#);
        assert_eq!(
            resolve_transport(&entries, None, None),
            McpServerTransport::Http
        );
    }

    #[test]
    fn x2_unknown_type_falls_through_to_presence_inference() {
        let entries = object_entries(r#"{"type": "sse"}"#);
        // No command/url signal at all -> unknown, `"sse"` is not treated
        // specially.
        assert_eq!(
            resolve_transport(&entries, None, None),
            McpServerTransport::Unknown
        );
        // A command IS present -> falls through to stdio, `"sse"` still
        // ignored rather than forcing http or erroring.
        assert_eq!(
            resolve_transport(&entries, Some("npx"), None),
            McpServerTransport::Stdio
        );
    }

    #[test]
    fn x2_non_string_type_is_ignored() {
        let entries = object_entries(r#"{"type": 5}"#);
        assert_eq!(
            resolve_transport(&entries, None, None),
            McpServerTransport::Unknown
        );
        assert_eq!(
            resolve_transport(&entries, Some("npx"), None),
            McpServerTransport::Stdio
        );
    }

    #[test]
    fn x2_command_and_url_together_resolves_to_http() {
        // Crux pin: `url` is checked before `command` in resolveTransport,
        // so a command being present too does not push this to stdio.
        let summary = summarize(r#"{"command": "npx", "url": "https://example.com"}"#);
        assert_eq!(summary.transport, McpServerTransport::Http);
        assert_eq!(summary.status, McpServerStatus::Enabled);
        assert_eq!(summary.command.as_deref(), Some("npx"));
        assert_eq!(summary.url.as_deref(), Some("https://example.com"));
        assert_eq!(summary.issue, None);
    }

    #[test]
    fn x2_type_local_with_url_resolves_to_http() {
        // Crux pin: an explicit `type: "local"` is overridden by a present,
        // truthy `url` because `url` is checked in the FIRST `if`.
        let summary = summarize(r#"{"type": "local", "url": "https://example.com"}"#);
        assert_eq!(summary.transport, McpServerTransport::Http);
        assert_eq!(summary.status, McpServerStatus::Enabled);
        assert_eq!(summary.url.as_deref(), Some("https://example.com"));
    }

    /// Kills the surviving mutant that adds
    /// `Some(JsonValue::Bool(_)) => Some("local"),` (or an equivalent
    /// coercion for numbers/null/unrecognized strings) to `resolve_transport`'s
    /// `type_value` match. Every existing non-string-`type` case ALSO carries
    /// a `command` or `url` that independently forces the same transport, so
    /// the `type` read itself is never actually observed. Here `type` is the
    /// ONLY field present — no `command`, no `url` — so a mutant that treats
    /// a non-string (or unrecognized-string) `type` as `"local"` diverges:
    /// real `resolve_transport` ignores it entirely and falls all the way
    /// through to `Unknown`/`Invalid`/`"Missing command or URL."`, while a
    /// coercing mutant would land on `Stdio`/`Invalid`/`"Missing command."`.
    #[test]
    fn x2_a_non_string_type_is_ignored_entirely_not_coerced() {
        for entry in [
            r#"{"type": true}"#,
            r#"{"type": 42}"#,
            r#"{"type": null}"#,
            r#"{"type": "sse"}"#,
        ] {
            let summary = summarize(entry);
            assert_eq!(
                summary.transport,
                McpServerTransport::Unknown,
                "entry {entry} should resolve to Unknown"
            );
            assert_eq!(
                summary.status,
                McpServerStatus::Invalid,
                "entry {entry} should be Invalid"
            );
            assert_eq!(
                summary.issue,
                Some("Missing command or URL.".to_string()),
                "entry {entry} should carry the no-signal-at-all issue"
            );
        }
    }

    // -- X3: asymmetric readers ---------------------------------------------------

    #[test]
    fn x3_read_command_array_with_non_string_first_element_is_none() {
        let entries = object_entries(r#"{"command": [1, "npx"]}"#);
        assert_eq!(read_command(&entries), None);
    }

    #[test]
    fn x3_read_command_empty_array_is_none() {
        let entries = object_entries(r#"{"command": []}"#);
        assert_eq!(read_command(&entries), None);
    }

    #[test]
    fn x3_read_url_has_no_array_form() {
        let entries = object_entries(r#"{"url": ["https://example.com"]}"#);
        assert_eq!(read_url(&entries), None);
    }

    #[test]
    fn x3_read_url_prefers_url_over_http_url() {
        let entries = object_entries(r#"{"url": "a", "httpUrl": "b"}"#);
        assert_eq!(read_url(&entries).as_deref(), Some("a"));
    }

    /// Kills the surviving mutant that reorders `read_url` to check
    /// `httpUrl` before `url`. No existing case supplies both keys with
    /// distinct values, so that reordering was never actually exercised
    /// end-to-end through `summarize_mcp_server`. This pins all three
    /// directions: `url` wins when both are present strings; `httpUrl` is
    /// still live as a fallback when `url` is absent; and `url` being an
    /// ARRAY (which `read_url` has no array form for, per X3) does NOT match
    /// the `url` branch, so the read falls through to `httpUrl` instead of
    /// producing `None`.
    #[test]
    fn x3_url_wins_over_http_url_when_both_are_present() {
        let both = summarize(
            r#"{"url": "https://from-url.example", "httpUrl": "https://from-http-url.example"}"#,
        );
        assert_eq!(both.url, Some("https://from-url.example".to_string()));
        assert_eq!(both.transport, McpServerTransport::Http);

        let http_url_only = summarize(r#"{"httpUrl": "https://from-http-url.example"}"#);
        assert_eq!(
            http_url_only.url,
            Some("https://from-http-url.example".to_string())
        );

        let array_url_falls_through = summarize(
            r#"{"url": ["https://array.example"], "httpUrl": "https://from-http-url.example"}"#,
        );
        assert_eq!(
            array_url_falls_through.url,
            Some("https://from-http-url.example".to_string())
        );
    }

    // -- X4: falsy, not absent ----------------------------------------------------

    #[test]
    fn x4_empty_string_url_is_missing_url_not_an_enabled_empty_url() {
        let summary = summarize(r#"{"type": "http", "url": ""}"#);
        assert_eq!(
            summary,
            McpServerSummary {
                name: "s".to_string(),
                transport: McpServerTransport::Http,
                status: McpServerStatus::Invalid,
                command: None,
                url: None,
                env: None,
                issue: Some("Missing URL.".to_string()),
            }
        );
    }

    #[test]
    fn x4_empty_string_command_is_missing_command() {
        let summary = summarize(r#"{"type": "local", "command": ""}"#);
        assert_eq!(
            summary,
            McpServerSummary {
                name: "s".to_string(),
                transport: McpServerTransport::Stdio,
                status: McpServerStatus::Invalid,
                command: None,
                url: None,
                env: None,
                issue: Some("Missing command.".to_string()),
            }
        );
    }

    #[test]
    fn x4_command_array_with_lone_empty_string_element_is_still_falsy() {
        let summary = summarize(r#"{"type": "local", "command": [""]}"#);
        assert_eq!(summary.issue, Some("Missing command.".to_string()));
        assert_eq!(summary.status, McpServerStatus::Invalid);
    }

    // -- X5: strict enabled/disabled comparison -----------------------------------

    #[test]
    fn x5_enabled_zero_is_still_enabled() {
        let summary = summarize(r#"{"command": "npx", "enabled": 0}"#);
        assert_eq!(summary.status, McpServerStatus::Enabled);
    }

    #[test]
    fn x5_enabled_null_is_still_enabled() {
        let summary = summarize(r#"{"command": "npx", "enabled": null}"#);
        assert_eq!(summary.status, McpServerStatus::Enabled);
    }

    #[test]
    fn x5_enabled_string_false_is_still_enabled() {
        let summary = summarize(r#"{"command": "npx", "enabled": "false"}"#);
        assert_eq!(summary.status, McpServerStatus::Enabled);
    }

    #[test]
    fn x5_disabled_one_is_still_enabled() {
        let summary = summarize(r#"{"command": "npx", "disabled": 1}"#);
        assert_eq!(summary.status, McpServerStatus::Enabled);
    }

    #[test]
    fn x5_disabled_yes_is_still_enabled() {
        let summary = summarize(r#"{"command": "npx", "disabled": "yes"}"#);
        assert_eq!(summary.status, McpServerStatus::Enabled);
    }

    #[test]
    fn x5_disabled_true_is_disabled() {
        let summary = summarize(r#"{"command": "npx", "disabled": true}"#);
        assert_eq!(summary.status, McpServerStatus::Disabled);
    }

    // -- X6: invalid is decided before enabled/disabled is consumed --------------

    #[test]
    fn x6_enabled_false_with_no_transport_signal_is_invalid_not_disabled() {
        // The literal entry `{"enabled": false}` carries no `type`,
        // `command`, or `url` at all, so transport resolves to `unknown`
        // (O:274) and the function returns at O:203-210, well before the
        // `enabled ? 'enabled' : 'disabled'` ternary at O:236 is ever
        // reached — even though `enabled` was already computed at O:200.
        let summary = summarize(r#"{"enabled": false}"#);
        assert_eq!(summary.status, McpServerStatus::Invalid);
        assert_eq!(summary.issue, Some("Missing command or URL.".to_string()));
    }

    #[test]
    fn x6_enabled_false_with_stdio_type_and_no_command_is_invalid_missing_command() {
        // Same ordering point, but with transport pinned to `stdio` via an
        // explicit `type: "local"` so the invalid branch that fires is the
        // "Missing command." one: `{"enabled": false}` + no command must
        // still come back `invalid`, never `disabled`.
        let summary = summarize(r#"{"type": "local", "enabled": false}"#);
        assert_eq!(summary.status, McpServerStatus::Invalid);
        assert_eq!(summary.issue, Some("Missing command.".to_string()));
    }

    // -- X7: `Some("")` is not missing --------------------------------------------

    #[test]
    fn x7_some_empty_string_content_is_invalid_not_missing() {
        let result = inspect_mcp_config_content(workspace_candidate(), Some(""));
        assert!(result.exists);
        assert_eq!(result.status, McpConfigStatus::Invalid);
        assert!(result.error.is_some());
    }

    // -- X8: error message contains "JSON", never input bytes --------------------

    #[test]
    fn x8_invalid_json_error_contains_json_and_never_leaks_input_bytes() {
        let content = r#"{"apiKey":"sk-live-SUPERSECRET-abcdef","x":@}"#;
        let result = inspect_mcp_config_content(workspace_candidate(), Some(content));
        assert_eq!(result.status, McpConfigStatus::Invalid);
        let error = result.error.expect("invalid JSON produces an error");
        assert!(error.contains("JSON"));
        assert!(!error.contains("SUPERSECRET"));
        assert!(!error.contains("apiKey"));
        assert!(!error.contains("sk-live"));
    }

    // -- X10: no trimming, `args` is inert -----------------------------------------

    #[test]
    fn x10_args_field_is_never_read() {
        let summary = summarize(r#"{"command": "npx", "args": 12345}"#);
        assert_eq!(summary.transport, McpServerTransport::Stdio);
        assert_eq!(summary.status, McpServerStatus::Enabled);
        assert_eq!(summary.command.as_deref(), Some("npx"));
        assert_eq!(summary.issue, None);
    }

    #[test]
    fn x10_command_is_not_trimmed() {
        let summary = summarize(r#"{"command": "  npx  "}"#);
        assert_eq!(summary.command.as_deref(), Some("  npx  "));
        assert_eq!(summary.status, McpServerStatus::Enabled);
    }

    // -- X11: the four verbatim issue strings --------------------------------------

    #[test]
    fn x11_null_entry_is_server_entry_must_be_an_object() {
        let summary = summarize_mcp_server("n", &JsonValue::Null);
        assert_eq!(
            summary,
            McpServerSummary {
                name: "n".to_string(),
                transport: McpServerTransport::Unknown,
                status: McpServerStatus::Invalid,
                command: None,
                url: None,
                env: None,
                issue: Some("Server entry must be an object.".to_string()),
            }
        );
    }

    #[test]
    fn x11_array_entry_is_server_entry_must_be_an_object() {
        let summary = summarize("[1, 2]");
        assert_eq!(
            summary.issue,
            Some("Server entry must be an object.".to_string())
        );
        assert_eq!(summary.env, None);
    }

    #[test]
    fn x11_string_entry_is_server_entry_must_be_an_object() {
        let summary = summarize(r#""str""#);
        assert_eq!(
            summary.issue,
            Some("Server entry must be an object.".to_string())
        );
    }

    #[test]
    fn x11_number_entry_is_server_entry_must_be_an_object() {
        let summary = summarize("5");
        assert_eq!(
            summary.issue,
            Some("Server entry must be an object.".to_string())
        );
    }

    #[test]
    fn x11_bool_entry_is_server_entry_must_be_an_object() {
        let summary = summarize("true");
        assert_eq!(
            summary.issue,
            Some("Server entry must be an object.".to_string())
        );
    }

    #[test]
    fn x11_missing_command_or_url_issue_string_is_verbatim() {
        let summary = summarize("{}");
        assert_eq!(summary.issue, Some("Missing command or URL.".to_string()));
    }

    #[test]
    fn x11_missing_url_issue_string_is_verbatim() {
        let summary = summarize(r#"{"type": "http"}"#);
        assert_eq!(summary.issue, Some("Missing URL.".to_string()));
    }

    #[test]
    fn x11_missing_command_issue_string_is_verbatim() {
        let summary = summarize(r#"{"type": "local"}"#);
        assert_eq!(summary.issue, Some("Missing command.".to_string()));
    }
}
