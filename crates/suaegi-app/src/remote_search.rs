//! Bounded Quick Open and content search for SSH recipe workspaces.

use serde_json::json;
use suaegi_search::{SearchOptions, SearchResult};

use crate::ephemeral_vm::RecipeSshTarget;

const SEARCH_OUTPUT_LIMIT: u64 = 16 * 1024 * 1024;
const FILE_LIST_OUTPUT_LIMIT: u64 = 16 * 1024 * 1024;

const FILE_LIST_SCRIPT: &str = r#"
import json, os
root = os.getcwd()
blocked = {".git", ".next", ".nuxt", ".cache", ".stably", ".vscode", ".idea", ".yarn", ".pnpm-store", ".terraform", ".docker", ".husky", ".npm", ".npm-global", ".gvfs", "node_modules"}
files = []
for directory, dirs, names in os.walk(root, followlinks=False):
    dirs[:] = sorted(d for d in dirs if d not in blocked and not os.path.islink(os.path.join(directory, d)))
    rel_dir = os.path.relpath(directory, root)
    if rel_dir == ".local/share" or rel_dir.startswith(".local/share/"):
        dirs[:] = []
        continue
    for name in sorted(names):
        path = os.path.join(directory, name)
        if os.path.islink(path) or not os.path.isfile(path):
            continue
        rel = os.path.relpath(path, root).replace(os.sep, "/")
        files.append(rel)
        if len(files) > 200000:
            raise SystemExit("workspace contains more than 200000 files")
print(json.dumps(files, ensure_ascii=False, separators=(",", ":")))
"#;

const SEARCH_SCRIPT: &str = r#"
import fnmatch, json, os, re, sys
options = json.load(sys.stdin)
root = os.getcwd()
query = options["query"]
flags = 0 if options["caseSensitive"] else re.IGNORECASE
pattern = query if options["useRegex"] else re.escape(query)
if options["wholeWord"]:
    pattern = r"\b(?:" + pattern + r")\b"
try:
    matcher = re.compile(pattern, flags)
except re.error as error:
    raise SystemExit("Invalid search regular expression: " + str(error))
blocked = {".git", ".next", ".nuxt", ".cache", ".stably", ".vscode", ".idea", ".yarn", ".pnpm-store", ".terraform", ".docker", ".husky", ".npm", ".npm-global", ".gvfs", "node_modules"}
include = options["include"]
exclude = options["exclude"]
limit = options["maxResults"]
files = []
total = 0
truncated = False
marker = "…"
marker_bytes = len(marker.encode("utf-8"))

def allowed(rel):
    if include and not any(fnmatch.fnmatch(rel, item) for item in include):
        return False
    if exclude and any(fnmatch.fnmatch(rel, item) for item in exclude):
        return False
    return True

def clamp(text, start, length):
    raw = text.encode("utf-8")
    if len(raw) <= 500:
        return text, start + 1, length, None, None
    shown = min(length, 500)
    left = (500 - shown) // 2
    window_start = max(0, min(len(raw), start - left))
    window_end = min(len(raw), window_start + 500)
    window_start = max(0, window_end - 500)
    while window_start > 0 and (raw[window_start] & 0xC0) == 0x80:
        window_start -= 1
    while window_end < len(raw) and (raw[window_end] & 0xC0) == 0x80:
        window_end += 1
    snippet = raw[window_start:window_end].decode("utf-8")
    display = start - window_start + 1
    if window_start:
        snippet = marker + snippet
        display += marker_bytes
    if window_end < len(raw):
        snippet += marker
    return snippet, start + 1, length, display, shown

for directory, dirs, names in os.walk(root, followlinks=False):
    dirs[:] = sorted(d for d in dirs if d not in blocked and not os.path.islink(os.path.join(directory, d)))
    rel_dir = os.path.relpath(directory, root).replace(os.sep, "/")
    if rel_dir == ".local/share" or rel_dir.startswith(".local/share/"):
        dirs[:] = []
        continue
    for name in sorted(names):
        path = os.path.join(directory, name)
        if os.path.islink(path) or not os.path.isfile(path):
            continue
        rel = os.path.relpath(path, root).replace(os.sep, "/")
        if not allowed(rel):
            continue
        try:
            stat = os.stat(path)
            if stat.st_size > 5 * 1024 * 1024:
                continue
            data = open(path, "rb").read()
            if b"\0" in data[:8192]:
                continue
            text = data.decode("utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        matches = []
        per_file = 0
        for line_number, line in enumerate(text.splitlines(), 1):
            for found in matcher.finditer(line):
                start = len(line[:found.start()].encode("utf-8"))
                length = len(line[found.start():found.end()].encode("utf-8"))
                snippet, column, match_length, display_column, display_length = clamp(line, start, length)
                item = {"line": line_number, "column": column, "matchLength": match_length, "lineContent": snippet}
                if display_column is not None:
                    item["displayColumn"] = display_column
                    item["displayMatchLength"] = display_length
                matches.append(item)
                total += 1
                per_file += 1
                if total >= limit:
                    truncated = True
                    break
                if per_file >= 100:
                    break
            if total >= limit or per_file >= 100:
                break
        if matches:
            files.append({"filePath": path, "relativePath": rel, "matches": matches, "matchCount": len(matches)})
        if total >= limit:
            break
    if total >= limit:
        break
print(json.dumps({"files": files, "totalMatches": total, "truncated": truncated}, ensure_ascii=False, separators=(",", ":")))
"#;

fn shell_quote(value: &str) -> Result<String, String> {
    if value.contains('\0') {
        return Err("Remote search command contains a NUL byte.".to_string());
    }
    Ok(format!("'{}'", value.replace('\'', "'\"'\"'")))
}

fn python_command(script: &str) -> Result<String, String> {
    Ok(format!("python3 -c {}", shell_quote(script)?))
}

pub async fn list_files(
    target: RecipeSshTarget,
    project_root: String,
) -> Result<Vec<String>, String> {
    let output = crate::ssh::run_recipe_remote_command(
        target,
        project_root,
        python_command(FILE_LIST_SCRIPT)?,
        None,
        FILE_LIST_OUTPUT_LIMIT,
    )
    .await?;
    serde_json::from_str(&output).map_err(|_| "Remote Quick Open response is invalid.".to_string())
}

pub async fn search(
    target: RecipeSshTarget,
    project_root: String,
    options: SearchOptions,
) -> Result<SearchResult, String> {
    let max_results = options
        .max_results
        .unwrap_or(suaegi_search::DEFAULT_SEARCH_MAX_RESULTS)
        .clamp(1, suaegi_search::DEFAULT_SEARCH_MAX_RESULTS);
    let payload = serde_json::to_vec(&json!({
        "query": options.query,
        "caseSensitive": options.case_sensitive.unwrap_or(false),
        "wholeWord": options.whole_word.unwrap_or(false),
        "useRegex": options.use_regex.unwrap_or(false),
        "include": options.include_pattern.as_deref().map(suaegi_search::split_search_glob_patterns).unwrap_or_default(),
        "exclude": options.exclude_pattern.as_deref().map(suaegi_search::split_search_glob_patterns).unwrap_or_default(),
        "maxResults": max_results,
    }))
    .map_err(|error| format!("Could not encode remote search request: {error}"))?;
    let output = crate::ssh::run_recipe_remote_command(
        target,
        project_root,
        python_command(SEARCH_SCRIPT)?,
        Some(payload),
        SEARCH_OUTPUT_LIMIT,
    )
    .await?;
    let result: SearchResult = serde_json::from_str(&output)
        .map_err(|_| "Remote content-search response is invalid.".to_string())?;
    Ok(suaegi_search::normalize_search_result(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_are_quoted_as_one_argument() {
        let command = python_command("print(\"it's safe\")").unwrap();
        assert!(command.starts_with("python3 -c '"));
        assert!(command.contains("'\"'\"'"));
        assert!(python_command("bad\0script").is_err());
    }
}
