//! Orca-compatible Git/GitHub attribution for Suaegi-managed terminals.
//!
//! The setting must not mutate a user's global Git configuration. Like Orca,
//! Suaegi installs private `git`/`gh` shims and prepends them only to the PATH
//! of terminals it launches. Interactive/editor-driven commands pass through;
//! only commands with an explicit non-interactive message are attributed.

use std::io;
use std::path::{Path, PathBuf};

const SHIM_VERSION: &str = "1";
const COMMIT_TRAILER: &str = "Co-authored-by: Orca <help@stably.ai>";
const GH_FOOTER: &str = "Made with [Orca](https://github.com/stablyai/orca) 🐋";

fn attribution_root() -> PathBuf {
    let base = dirs::config_dir().unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".suaegi")
    });
    base.join("suaegi").join("orca-terminal-attribution")
}

fn write_executable(path: &Path, contents: &str) -> io::Result<()> {
    std::fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

fn ensure_shims_at(root: &Path) -> io::Result<PathBuf> {
    let posix = root.join("posix");
    let version = root.join("VERSION");
    let current = std::fs::read_to_string(&version)
        .ok()
        .map(|value| value.trim().to_string());
    let git = posix.join("git");
    let gh = posix.join("gh");
    if current.as_deref() == Some(SHIM_VERSION) && git.is_file() && gh.is_file() {
        return Ok(posix);
    }

    std::fs::create_dir_all(&posix)?;
    write_executable(&git, &git_wrapper())?;
    write_executable(&gh, &gh_wrapper())?;
    std::fs::write(version, format!("{SHIM_VERSION}\n"))?;
    Ok(posix)
}

fn is_attribution_path(entry: &str) -> bool {
    entry
        .replace('\\', "/")
        .to_ascii_lowercase()
        .contains("/orca-terminal-attribution/")
}

/// Add Orca's terminal-scoped attribution environment to a PTY spawn.
///
/// Installation failures are deliberately non-fatal: a terminal must still
/// open if its config directory is temporarily read-only.
pub fn inject(env: &mut Vec<(String, String)>, enabled: bool) {
    if !enabled {
        return;
    }
    #[cfg(unix)]
    {
        let Ok(shim_dir) = ensure_shims_at(&attribution_root()) else {
            return;
        };
        inject_with_shim_dir(env, &shim_dir);
    }
}

#[cfg(unix)]
fn inject_with_shim_dir(env: &mut Vec<(String, String)>, shim_dir: &Path) {
    let inherited_path = env
        .iter()
        .rev()
        .find(|(key, _)| key == "PATH")
        .map(|(_, value)| value.clone())
        .or_else(|| std::env::var("PATH").ok())
        .unwrap_or_default();
    let cleaned = inherited_path
        .split(':')
        .filter(|entry| !entry.is_empty() && !is_attribution_path(entry))
        .collect::<Vec<_>>()
        .join(":");
    let path = if cleaned.is_empty() {
        shim_dir.display().to_string()
    } else {
        format!("{}:{cleaned}", shim_dir.display())
    };

    env.retain(|(key, _)| {
        !matches!(
            key.as_str(),
            "PATH"
                | "ORCA_ENABLE_GIT_ATTRIBUTION"
                | "ORCA_GIT_COMMIT_TRAILER"
                | "ORCA_GH_PR_FOOTER"
                | "ORCA_GH_ISSUE_FOOTER"
                | "ORCA_ATTRIBUTION_SHIM_DIR"
        )
    });
    env.extend([
        ("PATH".to_string(), path),
        ("ORCA_ENABLE_GIT_ATTRIBUTION".to_string(), "1".to_string()),
        (
            "ORCA_GIT_COMMIT_TRAILER".to_string(),
            COMMIT_TRAILER.to_string(),
        ),
        ("ORCA_GH_PR_FOOTER".to_string(), GH_FOOTER.to_string()),
        ("ORCA_GH_ISSUE_FOOTER".to_string(), GH_FOOTER.to_string()),
        (
            "ORCA_ATTRIBUTION_SHIM_DIR".to_string(),
            shim_dir.display().to_string(),
        ),
    ]);
}

const POSIX_COMMON: &str = r####"#!/usr/bin/env bash
set -euo pipefail

clean_path() {
  local current_path="${PATH:-}"
  local script_dir
  script_dir="$(cd -- "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
  local cleaned=()
  local entry
  IFS=':' read -r -a entries <<<"$current_path"
  for entry in "${entries[@]}"; do
    case "$entry" in
      "$script_dir"|*/orca-terminal-attribution/posix|*/orca-terminal-attribution/win32|*\\orca-terminal-attribution\\posix|*\\orca-terminal-attribution\\win32)
        ;;
      *)
        cleaned+=("$entry")
        ;;
    esac
  done
  (IFS=':'; printf '%s' "${cleaned[*]:-}")
}
"####;

const POSIX_GIT_BODY: &str = r####"
real_path="$(clean_path)"
real_git="$(PATH="$real_path" command -v git || true)"
if [[ -z "$real_git" ]]; then
  echo "Orca attribution wrapper could not locate git on PATH." >&2
  exit 127
fi

is_commit_command() {
  while [[ $# -gt 0 ]]; do
    case "$1" in
      -c|--config|-C|--git-dir|--work-tree|--namespace) shift 2 ;;
      --config=*|--git-dir=*|--work-tree=*|--namespace=*) shift ;;
      commit) return 0 ;;
      -*) shift ;;
      *) return 1 ;;
    esac
  done
  return 1
}

if [[ "${ORCA_ENABLE_GIT_ATTRIBUTION:-0}" != "1" || "${ORCA_ATTRIBUTION_BYPASS:-0}" == "1" ]] || ! is_commit_command "$@"; then
  PATH="$real_path" exec "$real_git" "$@"
fi

for arg in "$@"; do
  [[ "$arg" == "--dry-run" ]] && PATH="$real_path" exec "$real_git" "$@"
done

trailer="${ORCA_GIT_COMMIT_TRAILER:-Co-authored-by: Orca <help@stably.ai>}"

has_explicit_commit_message() {
  local arg
  while [[ $# -gt 0 ]]; do
    arg="$1"
    case "$arg" in
      -m|--message|-F|--file|--message=*|--file=*|-[!-]*m|-m?*|-F?*) return 0 ;;
    esac
    shift
  done
  return 1
}

has_unsupported_commit_message_source() {
  local arg next_arg
  local saw_commit=0
  while [[ $# -gt 0 ]]; do
    arg="$1"
    if [[ $saw_commit -eq 0 ]]; then
      case "$arg" in
        -c|--config|-C|--git-dir|--work-tree|--namespace) shift 2; continue ;;
        --config=*|--git-dir=*|--work-tree=*|--namespace=*) shift; continue ;;
        commit) saw_commit=1; shift; continue ;;
      esac
    fi
    case "$arg" in
      -C|--reuse-message|-c|--reedit-message|--fixup|--squash) return 0 ;;
      -F|--file)
        shift
        next_arg="${1:-}"
        [[ -z "$next_arg" || ! -f "$next_arg" ]] && return 0
        ;;
      --file=*)
        next_arg="${arg#--file=}"
        [[ ! -f "$next_arg" ]] && return 0
        ;;
      -F?*)
        next_arg="${arg:2}"
        [[ ! -f "$next_arg" ]] && return 0
        ;;
    esac
    shift
  done
  return 1
}

message_already_has_trailer() {
  local arg next_arg
  while [[ $# -gt 0 ]]; do
    arg="$1"
    case "$arg" in
      -m|--message)
        shift
        next_arg="${1:-}"
        grep -Fqi "$trailer" <<<"$next_arg" && return 0
        ;;
      --message=*) grep -Fqi "$trailer" <<<"${arg#--message=}" && return 0 ;;
      -m?*) grep -Fqi "$trailer" <<<"${arg:2}" && return 0 ;;
      -[!-]*m)
        shift
        next_arg="${1:-}"
        grep -Fqi "$trailer" <<<"$next_arg" && return 0
        ;;
      -F|--file)
        shift
        next_arg="${1:-}"
        [[ -n "$next_arg" && -f "$next_arg" ]] && grep -Fqi "$trailer" "$next_arg" && return 0
        ;;
      --file=*)
        next_arg="${arg#--file=}"
        [[ -f "$next_arg" ]] && grep -Fqi "$trailer" "$next_arg" && return 0
        ;;
      -F?*)
        next_arg="${arg:2}"
        [[ -f "$next_arg" ]] && grep -Fqi "$trailer" "$next_arg" && return 0
        ;;
    esac
    shift
  done
  return 1
}

if ! has_explicit_commit_message "$@" || has_unsupported_commit_message_source "$@" || message_already_has_trailer "$@"; then
  PATH="$real_path" exec "$real_git" "$@"
fi

tmp_file=""
cleanup_commit_message() {
  [[ -n "$tmp_file" ]] && rm -f "$tmp_file"
}
trap cleanup_commit_message EXIT

attributed_args=()
replaced_file_message=0
while [[ $# -gt 0 ]]; do
  arg="$1"
  case "$arg" in
    -F|--file)
      if [[ $replaced_file_message -eq 0 ]]; then
        shift
        source_file="${1:-}"
        tmp_file="$(mktemp)"
        if [[ -n "$source_file" && -f "$source_file" ]]; then
          printf '%s\n\n%s\n' "$(cat "$source_file")" "$trailer" >"$tmp_file"
          attributed_args+=("$arg" "$tmp_file")
          replaced_file_message=1
        else
          attributed_args+=("$arg" "$source_file")
        fi
      else
        attributed_args+=("$arg")
      fi
      ;;
    --file=*)
      if [[ $replaced_file_message -eq 0 ]]; then
        source_file="${arg#--file=}"
        tmp_file="$(mktemp)"
        if [[ -f "$source_file" ]]; then
          printf '%s\n\n%s\n' "$(cat "$source_file")" "$trailer" >"$tmp_file"
          attributed_args+=("--file=$tmp_file")
          replaced_file_message=1
        else
          attributed_args+=("$arg")
        fi
      else
        attributed_args+=("$arg")
      fi
      ;;
    -F?*)
      if [[ $replaced_file_message -eq 0 ]]; then
        source_file="${arg:2}"
        tmp_file="$(mktemp)"
        if [[ -f "$source_file" ]]; then
          printf '%s\n\n%s\n' "$(cat "$source_file")" "$trailer" >"$tmp_file"
          attributed_args+=("-F$tmp_file")
          replaced_file_message=1
        else
          attributed_args+=("$arg")
        fi
      else
        attributed_args+=("$arg")
      fi
      ;;
    *) attributed_args+=("$arg") ;;
  esac
  shift
done

if [[ $replaced_file_message -eq 0 ]]; then
  attributed_args+=("-m" "$trailer")
fi

ORCA_ATTRIBUTION_BYPASS=1 PATH="$real_path" exec "$real_git" "${attributed_args[@]}"
"####;

const POSIX_GH_BODY: &str = r####"
real_path="$(clean_path)"
real_gh="$(PATH="$real_path" command -v gh || true)"
if [[ -z "$real_gh" ]]; then
  echo "Orca attribution wrapper could not locate gh on PATH." >&2
  exit 127
fi

github_api_path() {
  local kind="$1"
  local url="$2"
  if [[ "$kind" == "pr" && "$url" =~ ^https://github[.]com/([^/]+)/([^/]+)/pull/([0-9]+) ]]; then
    printf 'repos/%s/%s/pulls/%s' "${BASH_REMATCH[1]}" "${BASH_REMATCH[2]}" "${BASH_REMATCH[3]}"
    return 0
  fi
  if [[ "$kind" == "issue" && "$url" =~ ^https://github[.]com/([^/]+)/([^/]+)/issues/([0-9]+) ]]; then
    printf 'repos/%s/%s/issues/%s' "${BASH_REMATCH[1]}" "${BASH_REMATCH[2]}" "${BASH_REMATCH[3]}"
    return 0
  fi
  return 1
}

append_footer() {
  local kind="$1"
  local pattern="$2"
  local footer="$3"
  local stdout_capture="$4"
  local stderr_capture="$5"
  local url
  url="$(printf '%s\n%s\n' "$stdout_capture" "$stderr_capture" | grep -Eo "$pattern" | tail -n 1 || true)"
  [[ -z "$url" ]] && return 0
  local api_path
  api_path="$(github_api_path "$kind" "$url" || true)"
  [[ -z "$api_path" ]] && return 0
  local body
  body="$(PATH="$real_path" "$real_gh" api "$api_path" --jq '.body // ""' 2>/dev/null)" || return 0
  grep -Fqi "$footer" <<<"$body" && return 0
  local tmp_file
  tmp_file="$(mktemp)"
  if [[ -n "$body" ]]; then
    printf '%s\n\n%s\n' "$body" "$footer" >"$tmp_file"
  else
    printf '%s\n' "$footer" >"$tmp_file"
  fi
  PATH="$real_path" "$real_gh" api -X PATCH "$api_path" -F "body=@$tmp_file" >/dev/null || true
  rm -f "$tmp_file"
}

has_noninteractive_create_args() {
  local arg
  for arg in "$@"; do
    case "$arg" in
      --title|-t|--title=*|--body|-b|--body=*|--body-file|-F|--body-file=*|--fill|--fill-first|--fill-verbose|--template|-T|--template=*|--recover|--recover=*|--web) return 0 ;;
    esac
  done
  return 1
}

has_passthrough_create_args() {
  local arg
  for arg in "$@"; do
    case "$arg" in --help|-h|--version) return 0 ;; esac
  done
  return 1
}

if [[ "${ORCA_ENABLE_GIT_ATTRIBUTION:-0}" != "1" || "${ORCA_ATTRIBUTION_BYPASS:-0}" == "1" ]]; then
  PATH="$real_path" exec "$real_gh" "$@"
fi

kind=""
pattern=""
footer=""
if [[ "${1:-}" == "pr" && "${2:-}" == "create" ]]; then
  kind="pr"
  pattern='https://github.com/[^[:space:]]+/pull/[0-9]+'
  footer="${ORCA_GH_PR_FOOTER:-Made with [Orca](https://github.com/stablyai/orca) 🐋}"
elif [[ "${1:-}" == "issue" && "${2:-}" == "create" ]]; then
  kind="issue"
  pattern='https://github.com/[^[:space:]]+/issues/[0-9]+'
  footer="${ORCA_GH_ISSUE_FOOTER:-Made with [Orca](https://github.com/stablyai/orca) 🐋}"
else
  PATH="$real_path" exec "$real_gh" "$@"
fi

if has_passthrough_create_args "$@" || ! has_noninteractive_create_args "$@"; then
  PATH="$real_path" exec "$real_gh" "$@"
fi

stdout_file="$(mktemp)"
stderr_file="$(mktemp)"
cleanup_capture() { rm -f "$stdout_file" "$stderr_file"; }
trap cleanup_capture EXIT
if PATH="$real_path" "$real_gh" "$@" >"$stdout_file" 2>"$stderr_file"; then status=0; else status=$?; fi
stdout_capture="$(cat "$stdout_file")"
stderr_capture="$(cat "$stderr_file")"
cat "$stderr_file" >&2
cat "$stdout_file"
if [[ $status -eq 0 ]]; then
  append_footer "$kind" "$pattern" "$footer" "$stdout_capture" "$stderr_capture"
fi
cleanup_capture
trap - EXIT
exit $status
"####;

const fn joined_len(left: &str, right: &str) -> usize {
    left.len() + right.len()
}

// Rust cannot concatenate non-literal const strings with `concat!`; fixed
// arrays keep the source readable and are joined only during installation.
struct ScriptParts(&'static str, &'static str);

impl ScriptParts {
    fn render(self) -> String {
        let mut result = String::with_capacity(joined_len(self.0, self.1));
        result.push_str(self.0);
        result.push_str(self.1);
        result
    }
}

// Lazy values are unnecessary: shim installation is rare and these are only
// rendered when a missing/outdated version is encountered.
fn git_wrapper() -> String {
    ScriptParts(POSIX_COMMON, POSIX_GIT_BODY).render()
}

fn gh_wrapper() -> String {
    ScriptParts(POSIX_COMMON, POSIX_GH_BODY).render()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn attribution_paths_are_recognized_cross_platform() {
        assert!(is_attribution_path(
            "/tmp/suaegi/orca-terminal-attribution/posix"
        ));
        assert!(is_attribution_path(
            r"C:\tmp\orca-terminal-attribution\win32"
        ));
        assert!(!is_attribution_path("/usr/local/bin"));
    }

    #[cfg(unix)]
    #[test]
    fn shim_installation_is_executable_and_versioned() {
        let temp = tempfile::tempdir().unwrap();
        let dir = ensure_shims_at(temp.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(temp.path().join("VERSION")).unwrap(),
            "1\n"
        );
        for name in ["git", "gh"] {
            let path = dir.join(name);
            assert!(path.is_file());
            assert_ne!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o111,
                0
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn injection_replaces_stale_path_and_sets_orca_contract() {
        let temp = tempfile::tempdir().unwrap();
        let shim = temp.path().join("orca-terminal-attribution/posix");
        let mut env = vec![(
            "PATH".into(),
            "/old/orca-terminal-attribution/posix:/usr/bin".into(),
        )];
        inject_with_shim_dir(&mut env, &shim);
        let value = |key: &str| {
            env.iter()
                .find(|(candidate, _)| candidate == key)
                .map(|(_, value)| value.as_str())
        };
        assert_eq!(value("ORCA_ENABLE_GIT_ATTRIBUTION"), Some("1"));
        assert_eq!(value("ORCA_GIT_COMMIT_TRAILER"), Some(COMMIT_TRAILER));
        let expected_path = format!("{}:/usr/bin", shim.display());
        assert_eq!(value("PATH"), Some(expected_path.as_str()));
    }

    #[cfg(unix)]
    #[test]
    fn git_shim_attributes_only_noninteractive_commits() {
        use std::process::Command;

        let temp = tempfile::tempdir().unwrap();
        let dir = ensure_shims_at(temp.path().join("root").as_path()).unwrap();
        let fake_bin = temp.path().join("bin");
        std::fs::create_dir_all(&fake_bin).unwrap();
        let fake_git = fake_bin.join("git");
        write_executable(
            &fake_git,
            "#!/bin/sh\nfor arg in \"$@\"; do printf '<%s>\\n' \"$arg\"; done\n",
        )
        .unwrap();
        let path = format!("{}:{}:/usr/bin:/bin", dir.display(), fake_bin.display());

        let attributed = Command::new(dir.join("git"))
            .args(["commit", "-m", "hello"])
            .env("PATH", &path)
            .env("ORCA_ENABLE_GIT_ATTRIBUTION", "1")
            .env("ORCA_GIT_COMMIT_TRAILER", COMMIT_TRAILER)
            .output()
            .unwrap();
        let output = String::from_utf8(attributed.stdout).unwrap();
        let stderr = String::from_utf8(attributed.stderr).unwrap();
        assert!(
            output.contains("<hello>"),
            "status={:?}, stdout={output:?}, stderr={stderr:?}",
            attributed.status.code()
        );
        assert!(output.contains(&format!("<{COMMIT_TRAILER}>")));

        let interactive = Command::new(dir.join("git"))
            .arg("commit")
            .env("PATH", &path)
            .env("ORCA_ENABLE_GIT_ATTRIBUTION", "1")
            .output()
            .unwrap();
        let output = String::from_utf8(interactive.stdout).unwrap();
        assert_eq!(output, "<commit>\n");
    }

    #[test]
    fn render_helpers_include_both_script_sections() {
        let git = git_wrapper();
        assert!(git.starts_with("#!/usr/bin/env bash"));
        assert!(git.contains("real_git="));
        let gh = gh_wrapper();
        assert!(gh.starts_with("#!/usr/bin/env bash"));
        assert!(gh.contains("real_gh="));
    }
}
