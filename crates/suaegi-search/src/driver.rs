//! M4 — the IMPURE process drivers. This is the half of Orca's content-search
//! backend that actually spawns a subprocess: it runs `rg` (or falls back to
//! `git grep`) and streams the child's stdout line-by-line through the pure
//! M1–M3 parsers ([`ingest_rg_json_line`] / [`ingest_git_grep_line`]).
//!
//! Ports the ORCHESTRATION of Orca `src/main/ipc/filesystem.ts:927-1022` (rg
//! spawn/stream/timeout/kill/backend-select) and `filesystem-search-git.ts:12-97`
//! (git-grep driver + rg-availability fallback), mirroring the process management
//! of suaegi-git's `quick_open.rs` (tokio `Command`, streamed stdout, explicit
//! kill/reap, and the **transient≠empty** discipline).
//!
//! # The cardinal invariant: transient ≠ empty
//! A failed or timed-out search must NEVER read as a *complete* empty result.
//! Concretely:
//! - **spawn failure** (rg/git binary missing, bad cwd) → [`SearchError::Spawn`],
//!   never `Ok(empty)`.
//! - **mid-stream IO error** → [`SearchError::Io`], never `Ok(empty)`.
//! - **rg exit 2 / git-grep exit >1 / signal-kill** → [`SearchError::Exit`].
//! - **rg present but its RUN fails** → HARD error; we do NOT second-chance
//!   fall back to git-grep (that would mask a real failure — quick_open fix 5).
//!
//! Timeout and cap are the *only* non-error truncations, and both surface as an
//! `Ok(SearchResult)` with `truncated == true` — a flagged partial result, never
//! a silent complete-looking empty. **Decision (plan C5):** a timeout is a
//! `truncated`-`Ok`, not an `Err` — the partial matches gathered so far are real
//! and the flag makes the incompleteness honest; erroring would throw them away.
//!
//! rg exit 1 ("no matches") and git-grep exit 1 ("no matches") are SUCCESS-empty
//! (`Ok` with `truncated == false`), NOT errors.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader};
use tokio::process::{Child, Command};

#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;

use crate::constants::{DEFAULT_SEARCH_MAX_RESULTS, SEARCH_TIMEOUT_MS};
use crate::git_grep_args::build_git_grep_args;
use crate::ingest::{finalize, ingest_git_grep_line, ingest_rg_json_line, Ingest};
use crate::rg_args::build_rg_args;
use crate::submatch::build_submatch_regex;
use crate::types::{create_accumulator, SearchAccumulator, SearchOptions, SearchResult};

/// rg availability probe timeout (`rg --version`), matching quick_open's
/// `RG_AVAILABILITY_TIMEOUT_MS`.
const RG_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Upper bound on reaping a killed child (matches suaegi-git's `REAP_TIMEOUT`).
/// If the kill doesn't take (an unkillable grandchild), we drop the `Child`;
/// `kill_on_drop(true)` lets tokio's reaper finish rather than hang forever.
const REAP_TIMEOUT: Duration = Duration::from_secs(5);

/// The failure taxonomy for a content search. The variants keep the
/// **transient ≠ empty** contract legible: any of these means "the search did
/// not complete", and a caller can never mistake one for an empty result.
#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    /// Failed to spawn the backend process (binary not found, bad cwd, …). This
    /// is the backend-unavailable / hard-transient case: NEVER an empty `Ok`.
    #[error("failed to spawn {program}: {source}")]
    Spawn {
        program: &'static str,
        source: std::io::Error,
    },
    /// A mid-stream IO error (reading stdout, waiting on the child). Transient —
    /// the partial buffer is discarded and this is returned, never `Ok(empty)`.
    #[error("{program} io error: {detail}")]
    Io {
        program: &'static str,
        detail: String,
    },
    /// The backend exited with an error status: rg exit 2, git-grep exit >1, or
    /// a signal-kill. (Exit 1 = "no matches" is NOT here — it is success-empty.)
    #[error("{program} exited with error ({code:?}): {detail}")]
    Exit {
        program: &'static str,
        code: Option<i32>,
        detail: String,
    },
}

/// **Public async entry point.** Run a content search: probe `rg` availability
/// once upfront, then drive `rg` (preferred) or `git grep` (rg-absent fallback),
/// streaming output through the M1–M3 parsers.
///
/// Returns `Ok(SearchResult)` on completion, cap-truncation, or timeout (the
/// last two flagged `truncated == true`); `Err(SearchError)` on any transient
/// failure. See the module docs for the transient≠empty contract.
pub async fn run_search(opts: &SearchOptions) -> Result<SearchResult, SearchError> {
    let rg = rg_available("rg", Path::new(&opts.root_path)).await;
    run_search_impl(
        opts,
        "rg",
        "git",
        rg,
        Duration::from_millis(SEARCH_TIMEOUT_MS),
    )
    .await
}

/// Cascade core with the backend-selection decision + program/timeout injected
/// (so tests can pin each branch without depending on the ambient `rg`/`git`).
///
/// **rg present → rg driver ONLY.** If the rg run fails it is a HARD error; we do
/// NOT fall back to git-grep (quick_open fix 5 — a second-chance fallback masks
/// real failures). git-grep is reached only when rg is *absent* upfront.
async fn run_search_impl(
    opts: &SearchOptions,
    rg_program: &str,
    git_program: &str,
    rg_available: bool,
    timeout: Duration,
) -> Result<SearchResult, SearchError> {
    // max_results clamp (filesystem.ts:927-930): at least 1, at most the default
    // cap; an unset request defaults to the cap. `clamp(1, DEFAULT)` == Orca's
    // `max(1, min(req, DEFAULT))` since `1 <= DEFAULT`.
    let max_results = opts
        .max_results
        .unwrap_or(DEFAULT_SEARCH_MAX_RESULTS)
        .clamp(1, DEFAULT_SEARCH_MAX_RESULTS);

    if rg_available {
        run_rg_search(opts, rg_program, timeout, max_results).await
    } else {
        run_git_grep_search(opts, git_program, timeout, max_results).await
    }
}

/// rg driver: spawn `rg` with [`build_rg_args`] in `root_path`, stream each
/// stdout line through [`ingest_rg_json_line`].
async fn run_rg_search(
    opts: &SearchOptions,
    program: &str,
    timeout: Duration,
    max_results: usize,
) -> Result<SearchResult, SearchError> {
    let root = opts.root_path.as_str();
    // target == root_path (Orca): rg emits absolute paths that ingest maps back
    // to root-relative. cwd is also root_path (harmless; mirrors Orca).
    let args = build_rg_args(&opts.query, root, opts);
    run_streaming(
        "rg",
        program,
        &args,
        Path::new(root),
        timeout,
        |line, acc| ingest_rg_json_line(line, root, acc, max_results, None),
    )
    .await
}

/// git-grep driver: spawn `git` directly (NOT via suaegi-git's `GitRunner`,
/// which buffers the entire output into a capped `Vec` — that would sacrifice
/// line-by-line streaming and the cap→kill early stop). Streams each stdout line
/// through [`ingest_git_grep_line`] with a best-effort submatch locator.
async fn run_git_grep_search(
    opts: &SearchOptions,
    program: &str,
    timeout: Duration,
    max_results: usize,
) -> Result<SearchResult, SearchError> {
    let root = opts.root_path.as_str();
    let args = build_git_grep_args(&opts.query, opts);
    // git grep reports only the first hit per line; the locator re-scans for all
    // occurrence columns. Compile failure → None → whole-line highlight (C2/C3).
    let re = build_submatch_regex(&opts.query, opts);
    run_streaming(
        "git",
        program,
        &args,
        Path::new(root),
        timeout,
        |line, acc| ingest_git_grep_line(line, root, re.as_ref(), acc, max_results),
    )
    .await
}

/// What ended the read loop.
enum ReadOutcome {
    /// The child closed stdout cleanly — inspect its exit code next.
    Eof,
    /// An ingest returned [`Ingest::Stop`] (total cap hit); `acc.truncated` is
    /// already set. Kill the child and finalize.
    Stopped,
}

/// The shared spawn/stream/timeout/kill core for both backends. Reads stdout
/// line-by-line, feeding each line to `ingest_line`; drains stderr concurrently
/// to avoid a full-pipe deadlock; enforces `timeout` over the whole read.
///
/// Outcome mapping (the transient≠empty contract lives here):
/// - **timeout elapsed** → set `acc.truncated = true` **BEFORE** killing (C5),
///   then kill/reap and `Ok(finalize)`. Never `Err`, never a complete-looking
///   empty: the flag makes the partial honest.
/// - **cap reached (`Stop`)** → `acc.truncated` already set by the ingest; kill
///   (to stop the child) and `Ok(finalize)`.
/// - **mid-stream IO error** → kill and `Err(Io)` (discard the partial buffer).
/// - **clean EOF** → inspect the exit code: 0/1 = success (1 = "no matches",
///   success-empty), signal-kill or any other code = `Err(Exit)`.
async fn run_streaming<F>(
    program: &'static str,
    spawn_program: &str,
    args: &[String],
    cwd: &Path,
    timeout: Duration,
    mut ingest_line: F,
) -> Result<SearchResult, SearchError>
where
    F: FnMut(&str, &mut SearchAccumulator) -> Ingest,
{
    let mut cmd = Command::new(spawn_program);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);

    // Spawn failure = backend-unavailable / hard-transient: Err, never Ok(empty).
    let mut child = cmd.spawn().map_err(|source| SearchError::Spawn {
        program,
        source,
    })?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");

    // Drain stderr on its own task so a chatty child (e.g. many permission
    // errors) can't fill the stderr pipe and block on write while we read stdout.
    let stderr_drain = tokio::spawn(async move { drain_reader(stderr).await });

    let mut acc = create_accumulator();

    let read = tokio::time::timeout(timeout, async {
        let mut lines = BufReader::new(stdout).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    if ingest_line(&line, &mut acc) == Ingest::Stop {
                        return Ok(ReadOutcome::Stopped);
                    }
                }
                Ok(None) => return Ok(ReadOutcome::Eof),
                Err(e) => {
                    return Err(SearchError::Io {
                        program,
                        detail: format!("read failed: {e}"),
                    })
                }
            }
        }
    })
    .await;

    // We're done reading; the drainer can stop (child's stderr closes on kill/exit).
    stderr_drain.abort();

    match read {
        // Timeout: mark truncated BEFORE the kill (C5), then reap and finalize.
        // A timeout is a truncated-Ok (plan decision), never an Err, and never a
        // complete-looking empty — the flag carries the incompleteness.
        Err(_elapsed) => {
            acc.truncated = true;
            kill_child(&mut child);
            reap(&mut child).await;
            Ok(finalize(&acc))
        }
        // Mid-stream IO error: discard the partial buffer, kill, hard-error.
        Ok(Err(e)) => {
            kill_child(&mut child);
            reap(&mut child).await;
            Err(e)
        }
        // Cap reached: `acc.truncated` was set inside the ingest. Kill the child
        // (we're no longer reading) and finalize the partial-but-flagged result.
        Ok(Ok(ReadOutcome::Stopped)) => {
            kill_child(&mut child);
            reap(&mut child).await;
            Ok(finalize(&acc))
        }
        // Clean EOF: the exit code decides success vs. transient error.
        Ok(Ok(ReadOutcome::Eof)) => {
            match tokio::time::timeout(REAP_TIMEOUT, child.wait()).await {
                Ok(Ok(status)) => {
                    // A signal-kill (timeout/OOM/external kill) with clean EOF is
                    // still a failure, not a success-empty.
                    #[cfg(unix)]
                    if let Some(sig) = status.signal() {
                        return Err(SearchError::Exit {
                            program,
                            code: None,
                            detail: format!("killed by signal {sig}"),
                        });
                    }
                    let code = status.code().unwrap_or(-1);
                    // 0 = matches, 1 = no matches (SUCCESS-empty). Anything else
                    // (rg 2, git-grep >1) is a real error.
                    if code == 0 || code == 1 {
                        Ok(finalize(&acc))
                    } else {
                        Err(SearchError::Exit {
                            program,
                            code: Some(code),
                            detail: format!("exited with code {code}"),
                        })
                    }
                }
                Ok(Err(e)) => Err(SearchError::Io {
                    program,
                    detail: format!("wait failed: {e}"),
                }),
                // The child closed stdout but won't reap — kill and hard-error
                // (transient), rather than pretend the (possibly partial) read is
                // a complete result.
                Err(_) => {
                    kill_child(&mut child);
                    reap(&mut child).await;
                    Err(SearchError::Io {
                        program,
                        detail: "wait timed out after EOF".to_string(),
                    })
                }
            }
        }
    }
}

/// Probe `rg` availability once, upfront (quick_open discipline): spawn
/// `rg --version` with a bounded timeout. spawn ENOENT (rg absent), a non-zero
/// exit, or a timeout all → `false` (→ git-grep fallback). This is the ONLY
/// thing that routes to git-grep; a failed rg *run* never does.
async fn rg_available(program: &str, cwd: &Path) -> bool {
    let mut cmd = Command::new(program);
    cmd.arg("--version")
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return false,
    };
    match tokio::time::timeout(RG_PROBE_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) => status.success(),
        Ok(Err(_)) => false,
        Err(_) => {
            kill_child(&mut child);
            reap(&mut child).await;
            false
        }
    }
}

// ─── shared spawn helpers (mirrors suaegi-git's runner/quick_open) ─────

/// Unix: SIGKILL the whole process **group** (any grandchildren the backend
/// spawned) then request kill on the child. Same discipline as suaegi-git's
/// `kill_process_tree`.
fn kill_child(child: &mut Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        // SAFETY: `kill(2)` with a negative pid targets the process group; this
        // is a plain libc call with no memory effects.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    let _ = child.start_kill();
}

/// Reap a killed child with an upper bound. If the kill didn't take (an
/// unkillable grandchild) we drop the `Child`; `kill_on_drop(true)` lets tokio's
/// reaper finish rather than hang past our own timeout.
async fn reap(child: &mut Child) {
    let _ = tokio::time::timeout(REAP_TIMEOUT, child.wait()).await;
}

/// Discard bytes to EOF (stderr drain — never buffered, so no cap needed).
async fn drain_reader<R: AsyncRead + Unpin>(mut reader: R) {
    let mut buf = [0u8; 8192];
    while let Ok(n) = reader.read(&mut buf).await {
        if n == 0 {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ─── fixtures ──────────────────────────────────────────────────

    /// Build a `SearchOptions` for a literal (fixed-string) search.
    fn opts_for(root: &Path, query: &str) -> SearchOptions {
        SearchOptions {
            query: query.to_string(),
            root_path: root.to_string_lossy().to_string(),
            ..Default::default()
        }
    }

    /// A hanging program: ignores its args and sleeps well past any test
    /// timeout, so the driver's timeout branch fires deterministically.
    fn slow_script(dir: &Path) -> PathBuf {
        let p = dir.join("slow.sh");
        std::fs::write(&p, "#!/bin/sh\nsleep 30\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        p
    }

    /// Init a real git repo in `dir` and commit `files` (name, contents).
    fn git_repo(dir: &Path, files: &[(&str, &str)]) {
        let run = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?} failed");
        };
        run(&["init", "-q"]);
        for (name, contents) in files {
            std::fs::write(dir.join(name), contents).unwrap();
        }
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", "init"]);
    }

    const TEST_TIMEOUT: Duration = Duration::from_secs(15);

    // ─── rg driver ─────────────────────────────────────────────────

    /// rg happy path: real `rg` finds every expected match with the correct
    /// relative path and total. Exercises the full public `run_search` entry
    /// (probe + rg driver + stream).
    #[tokio::test]
    async fn rg_happy_path_finds_matches() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello\nworld hello\nno match\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "hello there\n").unwrap();

        let res = run_search(&opts_for(dir.path(), "hello")).await.unwrap();

        assert_eq!(res.total_matches, 3, "two in a.txt + one in b.txt");
        assert!(!res.truncated);
        let mut rels: Vec<_> = res.files.iter().map(|f| f.relative_path.clone()).collect();
        rels.sort();
        assert_eq!(rels, vec!["a.txt", "b.txt"]);
    }

    /// THE cardinal crux — transient ≠ empty. A spawn failure (non-existent rg
    /// binary) must be an `Err(Spawn)`, NEVER `Ok(empty)`. Load-bearing: a driver
    /// that swallowed the spawn error into an empty result would pass every other
    /// test but fail this one.
    #[tokio::test]
    async fn transient_not_empty_rg_spawn_failure() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();

        // rg reported available, but the run's binary can't spawn → hard error.
        let err = run_search_impl(
            &opts_for(dir.path(), "hello"),
            "/nonexistent/definitely-not-rg",
            "git",
            true,
            TEST_TIMEOUT,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, SearchError::Spawn { program: "rg", .. }),
            "expected Spawn, got {err:?}"
        );
    }

    /// No matches = success-empty: rg exits 1, which is `Ok` with no files and
    /// `truncated == false` — NOT an error, NOT truncated.
    #[tokio::test]
    async fn no_matches_is_success_empty() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "nothing to see\n").unwrap();

        let res = run_search(&opts_for(dir.path(), "zzz-absent-zzz"))
            .await
            .unwrap();

        assert!(res.files.is_empty());
        assert_eq!(res.total_matches, 0);
        assert!(!res.truncated, "no-matches is complete, not truncated");
    }

    /// Cap → truncated Ok: more matches than `max_results` → `Ok`, `truncated`,
    /// and `total_matches == max_results` exactly (the child is killed early).
    #[tokio::test]
    async fn cap_produces_truncated_ok() {
        let dir = tempfile::tempdir().unwrap();
        let mut body = String::new();
        for _ in 0..20 {
            body.push_str("needle\n");
        }
        std::fs::write(dir.path().join("a.txt"), body).unwrap();

        let mut opts = opts_for(dir.path(), "needle");
        opts.max_results = Some(3);

        let res = run_search(&opts).await.unwrap();
        assert!(res.truncated, "hitting the cap must flag truncated");
        assert_eq!(res.total_matches, 3, "stops exactly at the cap");
    }

    /// Timeout → truncated Ok (plan C5): an injected hanging command trips the
    /// wall-clock timeout. The result is `Ok`, `truncated == true`, finalized —
    /// NOT an error, NOT a complete-looking empty.
    #[tokio::test]
    async fn timeout_produces_truncated_ok() {
        let dir = tempfile::tempdir().unwrap();
        let slow = slow_script(dir.path());

        let res = run_search_impl(
            &opts_for(dir.path(), "hello"),
            slow.to_str().unwrap(),
            "git",
            true, // rg "available" → drive the (slow) rg program
            Duration::from_millis(200),
        )
        .await
        .expect("timeout is a truncated-Ok, not an Err");

        assert!(res.truncated, "a timeout must flag truncated");
        assert!(res.files.is_empty());
        assert_eq!(res.total_matches, 0);
    }

    // ─── git-grep driver ───────────────────────────────────────────

    /// git-grep happy path: rg reported absent → git-grep drives and finds the
    /// matches in a real git repo.
    #[tokio::test]
    async fn git_grep_happy_path_finds_matches() {
        let dir = tempfile::tempdir().unwrap();
        git_repo(dir.path(), &[("a.txt", "alpha\nbeta alpha\n")]);

        let res = run_search_impl(
            &opts_for(dir.path(), "alpha"),
            "rg",
            "git",
            false, // rg absent → git-grep
            TEST_TIMEOUT,
        )
        .await
        .unwrap();

        assert_eq!(res.total_matches, 2);
        assert_eq!(res.files.len(), 1);
        assert_eq!(res.files[0].relative_path, "a.txt");
    }

    /// git-grep no matches = success-empty (exit 1), not an error.
    #[tokio::test]
    async fn git_grep_no_matches_is_success_empty() {
        let dir = tempfile::tempdir().unwrap();
        git_repo(dir.path(), &[("a.txt", "alpha\n")]);

        let res = run_search_impl(
            &opts_for(dir.path(), "zzz-absent-zzz"),
            "rg",
            "git",
            false,
            TEST_TIMEOUT,
        )
        .await
        .unwrap();

        assert!(res.files.is_empty());
        assert_eq!(res.total_matches, 0);
        assert!(!res.truncated);
    }

    /// git-grep transient ≠ empty: a non-existent git binary → `Err(Spawn)`,
    /// never `Ok(empty)`.
    #[tokio::test]
    async fn git_grep_transient_not_empty_spawn_failure() {
        let dir = tempfile::tempdir().unwrap();
        git_repo(dir.path(), &[("a.txt", "alpha\n")]);

        let err = run_search_impl(
            &opts_for(dir.path(), "alpha"),
            "rg",
            "/nonexistent/definitely-not-git",
            false, // rg absent → git-grep path (the bogus binary)
            TEST_TIMEOUT,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, SearchError::Spawn { program: "git", .. }),
            "expected Spawn, got {err:?}"
        );
    }

    // ─── backend selection ─────────────────────────────────────────

    /// Backend selection — rg absent routes to git-grep. The rg program is bogus,
    /// so if the dispatcher wrongly used rg it would Spawn-error; instead it uses
    /// git-grep and succeeds.
    #[tokio::test]
    async fn backend_selection_rg_absent_uses_git_grep() {
        let dir = tempfile::tempdir().unwrap();
        git_repo(dir.path(), &[("a.txt", "gamma\n")]);

        let res = run_search_impl(
            &opts_for(dir.path(), "gamma"),
            "/nonexistent/definitely-not-rg", // would fail if used
            "git",
            false,
            TEST_TIMEOUT,
        )
        .await
        .unwrap();
        assert_eq!(res.total_matches, 1);
    }

    /// Backend selection — rg present but its run FAILS → hard error, NO
    /// second-chance git-grep fallback (quick_open fix 5). The rg program is
    /// bogus and rg is "available"; even though a real git-grep on this repo
    /// would succeed, the dispatcher must NOT reach it.
    #[tokio::test]
    async fn backend_selection_rg_present_failed_hard_errors() {
        let dir = tempfile::tempdir().unwrap();
        git_repo(dir.path(), &[("a.txt", "delta\n")]);

        let err = run_search_impl(
            &opts_for(dir.path(), "delta"),
            "/nonexistent/definitely-not-rg",
            "git", // a real git that WOULD find "delta" — must not be used
            true,
            TEST_TIMEOUT,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, SearchError::Spawn { program: "rg", .. }),
            "rg-present-failed must be a hard rg error, got {err:?}"
        );
    }
}
