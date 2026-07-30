//! VERBATIM port of Orca's `src/shared/setup-agent-sequencing.ts` (257 lines)
//! @ v1.4.146-rc.0. Line citations below (`SAS:N`) refer to that source file.
//!
//! Ported: `SAS:7-8` [`DEFAULT_WAIT_TIMEOUT_SECONDS`, private constant] +
//! [`SETUP_AGENT_SEQUENCE_STARTUP_COMMAND_ENV`], `SAS:10-14`
//! [`SequencedSetupAgentCommands`], `SAS:16-22`
//! [`resolve_setup_agent_sequence_launch_command`], `SAS:24-30`
//! [`create_setup_agent_sequence_nonce`], `SAS:32-68`
//! [`create_sequenced_setup_agent_commands`], `SAS:70-85`
//! (`buildPosixSetupCommand`, private), `SAS:87-123`
//! (`buildPosixStartupCommand`, private), `SAS:125-133`
//! (`buildPosixStartupSuccessCommand`, private), `SAS:135-137`
//! (`hasLeadingPosixEnvAssignment`, private), `SAS:139-166`
//! (`hasUnquotedPosixCommandSeparator`, private), `SAS:168-179`
//! (`buildWindowsSetupCommand`, private), `SAS:181-231`
//! (`buildWindowsStartupCommand`, private), `SAS:233-235` (`wrapCmd`,
//! private), `SAS:237-246` (`quotePosixArg`/`quoteWindowsArg`, private —
//! duplicated from `setup_runner_command.rs`, see that module's docs),
//! `SAS:248-250` (`escapeCmdSetValue`, private), `SAS:252-257`
//! [`get_setup_agent_sequence_shell_for_tests`].
//!
//! SKIPPED oracle case: `setup-agent-sequencing.test.ts:30-36` ("defaults
//! agent startup to immediate unless the wait policy is explicit") exercises
//! `DEFAULT_SETUP_AGENT_STARTUP_POLICY` / `getDefaultRepoHookSettings` /
//! `shouldWaitForSetupBeforeAgentStartup`, all from the *unported*
//! `setup-agent-startup-policy.ts` (and `constants.ts`) modules — not this
//! one. Not ported here.
//!
//! NOT PORTED (documented deviation): `setup-agent-sequencing.test.ts:368-377`
//! ("prefers `crypto.randomUUID` when available") stubs `globalThis.crypto`,
//! a browser/Node host API with no Rust std equivalent. This crate is
//! forbidden from depending on `rand` (an OS-RNG dependency), so
//! [`create_setup_agent_sequence_nonce`] only reproduces the JS *fallback*
//! branch's shape (`SAS:29`); every oracle/pin test that needs a
//! deterministic nonce supplies one explicitly via `nonce: Some(...)`,
//! matching every actual call site the oracle exercises.
//!
//! # K4 — ⚠⚠ `escapeCmdSetValue` has ZERO oracle coverage and is the one
//! construction in this crate that can inject
//! `escape_cmd_set_value` (`SAS:248-250`) does `"` -> `""`, then escapes each
//! of `%`, `!`, `^` as `^` + itself. Its output is embedded inside `set
//! "VAR=…"` (`SAS:170-171`, `SAS:224-225`), and the whole line is then
//! re-quoted by [`wrap_cmd`] (`SAS:233-235`), which doubles `"` a SECOND
//! time. If `markerPath` (which is `runnerScriptPathForShell` + `.` + nonce +
//! `.done`, `SAS:43`) ever contained a `"` — e.g.
//! `C:\a"&calc&".done` — the two rounds of doubling do not compose back into
//! balanced quoting, and `&calc&` can end up OUTSIDE any quoted region,
//! executable by cmd.exe. This is UNREACHABLE in production: `"` is illegal
//! in NTFS filenames, and the nonce is caller-generated (a UUID in
//! practice) — which is presumably why upstream never fixed it. Ported
//! VERBATIM, not corrected. The exact injectable output is pinned below at
//! [`k4_quote_bearing_marker_path_breaks_cmd_quote_parity_in_the_full_setup_command`].
//! Separately: `^%` is cargo-cult — `^` does not escape `%` in cmd.exe — but
//! `^!` IS meaningful because [`wrap_cmd`]'s caller always runs under `/v:on`
//! (delayed expansion). Both are reproduced verbatim regardless.
//!
//! # K5 — the startup command is interpolated BARE on the POSIX branch, by
//! design
//! [`build_posix_startup_success_command`]'s `exec ${startupCommand}`
//! (`SAS:132`) is deliberately unquoted: `startupCommand` IS the user's own
//! shell command line and must word-split/glob/expand exactly as if the user
//! had typed it directly. The two heuristics in
//! [`has_unquoted_posix_command_separator`] /
//! [`has_leading_posix_env_assignment`] are the only guard steering
//! multi-command or env-prefixed input to the `eval "$(quote_posix_arg
//! startupCommand)"` branch instead (`SAS:130`, which itself re-parses the
//! quoted text via `eval` — also by design).
//!
//! # K6 — the wait timeout is only `max(1, floor(x))`
//! `SAS:96`/`SAS:186`. Modeled as `i64` here, which makes `floor` a no-op;
//! the JS `NaN`/`Infinity` pass-through (`Math.floor(NaN) === NaN`,
//! `Math.max(1, NaN) === NaN`, literally embedding the text `NaN` into the
//! generated shell script) is unreachable with an integer model and is not
//! reproduced.
//!
//! # K7 — a nonce containing `:` breaks the `IFS=:` marker-read split
//! `SAS:106`: `IFS=: read -r seen status < marker`. If `nonce` itself
//! contains `:`, the written line `nonce:status` (`SAS:79`) splits into MORE
//! than two fields, so `$seen` can never re-equal the (unsplit) nonce value
//! compared against at `SAS:107` — the startup gate then always times out.
//! Textually safe (no injection), but a real semantic hazard; ported
//! verbatim, not guarded against. Pinned at
//! [`k7_nonce_containing_colon_breaks_the_ifs_read_split`].
//!
//! # K10 — the launch-command env hint wins, but a whitespace-only value
//! falls back
//! `SAS:20`: `env[ENV]?.trim() || fallbackCommand`. The trim MUST be
//! ECMAScript whitespace (`suaegi_misc::js_trim`), never `str::trim` — see
//! the K10 pins below for U+FEFF (JS whitespace, trims away) vs U+0085 (NOT
//! JS whitespace, survives) in both directions.
//!
//! # K11 — `get_setup_agent_sequence_shell_for_tests` is a test-only export
//! `SAS:252-257`. Kept `pub` here (the oracle calls it directly,
//! `setup-agent-sequencing.test.ts:144`) — this is NOT part of the module's
//! production surface, only a test-observation seam, mirrored faithfully.
//!
//! # K12 — do NOT reuse `suaegi_misc::powershell_argument`'s helpers here
//! Those two functions port a DIFFERENT Orca module
//! (`powershell-native-argument.ts`) with different semantics (`'` doubling
//! and backslash-run handling for native PowerShell argv). Neither
//! `quote_windows_arg` (CommandLineToArgvW `"`->`""`) nor
//! `escape_cmd_set_value` (cmd.exe `set` value escaping) can be expressed
//! with them.
//!
//! # K13 — do NOT add a `js_trim_start` to `suaegi-misc`
//! [`has_leading_posix_env_assignment`] needs `.trimStart()` semantics
//! (`SAS:136`); rather than extend a crate five-plus other leaves depend on,
//! it is hand-rolled locally as [`js_trim_start`], built on the existing
//! `suaegi_misc::is_js_whitespace` predicate.

use std::collections::HashMap;

use suaegi_misc::{is_js_whitespace, js_trim};

use crate::setup_runner_command::{
    resolve_setup_runner_command, SetupRunnerCommandPlatform, SetupRunnerCommandShell,
};

/// `SAS:7`, private upstream — the default wait timeout (2 hours in
/// seconds).
const DEFAULT_WAIT_TIMEOUT_SECONDS: i64 = 2 * 60 * 60;

/// `SAS:8`.
pub const SETUP_AGENT_SEQUENCE_STARTUP_COMMAND_ENV: &str = "ORCA_SEQUENCED_STARTUP_COMMAND";

/// `SequencedSetupAgentCommands` (`SAS:10-14`). `startup_env` always holds
/// exactly one entry in practice (mirroring the TS object literal shape at
/// `SAS:50-52`/`SAS:64-66`); modeled as a map (not a single `Option<String>`)
/// to keep the field shape an honest `Record<string, string>` mirror.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SequencedSetupAgentCommands {
    pub setup_command: String,
    pub startup_command: String,
    pub startup_env: Option<HashMap<String, String>>,
}

/// `resolveSetupAgentSequenceLaunchCommand` (`SAS:16-22`). K10: `env`'s
/// looked-up value is trimmed with ECMAScript whitespace semantics
/// ([`js_trim`]), and JS `||` falls back on an EMPTY string too (not just a
/// missing key) — modeled here as `HashMap` absence (missing key) OR
/// present-but-trims-to-empty both falling through to `fallback_command`.
pub fn resolve_setup_agent_sequence_launch_command(
    env: &HashMap<String, String>,
    fallback_command: Option<&str>,
) -> Option<String> {
    let sequenced_startup = env
        .get(SETUP_AGENT_SEQUENCE_STARTUP_COMMAND_ENV)
        .map(|value| js_trim(value))
        .filter(|trimmed| !trimmed.is_empty());

    match sequenced_startup {
        Some(value) => Some(value.to_string()),
        None => fallback_command.map(|command| command.to_string()),
    }
}

/// `createSetupAgentSequenceNonce` (`SAS:24-30`). Only the JS *fallback*
/// branch is reproduced (see module docs for why `crypto.randomUUID`'s
/// preference has no faithful equivalent here): a millisecond timestamp and
/// a pseudo-random suffix, each base36-encoded and joined with `-`, matching
/// the fallback's shape (`SAS:29`,
/// `` `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}` ``).
/// The pseudo-random suffix is sourced from `std::collections::hash_map`'s
/// OS-seeded `RandomState` (std, not the forbidden `rand` crate) hashed
/// against a volatile per-call value — never asserted exactly by any test in
/// this crate (every oracle/pin case supplies an explicit `nonce`).
pub fn create_setup_agent_sequence_nonce() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::time::{SystemTime, UNIX_EPOCH};

    let millis: u128 = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);

    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u128(millis);
    let volatile = Box::new(0u8);
    hasher.write_usize(volatile.as_ref() as *const u8 as usize);
    let random_bits = hasher.finish();

    format!("{}-{}", to_base36(millis), to_base36(random_bits as u128))
}

/// Base-36 encoder (`0-9a-z`), mirroring `Number.prototype.toString(36)` for
/// non-negative integers. Hand-rolled: Rust has no built-in radix-36
/// formatter.
fn to_base36(mut n: u128) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".to_string();
    }
    let mut buf = Vec::new();
    while n > 0 {
        buf.push(DIGITS[(n % 36) as usize]);
        n /= 36;
    }
    buf.reverse();
    String::from_utf8(buf).expect("base36 digits are ASCII")
}

/// Argument bundle for [`create_sequenced_setup_agent_commands`], mirroring
/// the TS `args` object parameter (`SAS:32-38`).
pub struct CreateSequencedSetupAgentCommandsArgs {
    pub runner_script_path: String,
    pub startup_command: String,
    pub platform: SetupRunnerCommandPlatform,
    pub nonce: Option<String>,
    pub wait_timeout_seconds: Option<i64>,
}

/// `createSequencedSetupAgentCommands` (`SAS:32-68`).
pub fn create_sequenced_setup_agent_commands(
    args: CreateSequencedSetupAgentCommandsArgs,
) -> SequencedSetupAgentCommands {
    let nonce = args.nonce.unwrap_or_else(create_setup_agent_sequence_nonce);
    let resolution = resolve_setup_runner_command(&args.runner_script_path, args.platform);
    // Why: overlapping gated launches of the same setup runner must not race
    // on a shared completion marker.
    let marker_path = format!("{}.{}.done", resolution.runner_script_path_for_shell, nonce);
    let wait_timeout_seconds = args
        .wait_timeout_seconds
        .unwrap_or(DEFAULT_WAIT_TIMEOUT_SECONDS);

    let mut startup_env = HashMap::new();
    startup_env.insert(
        SETUP_AGENT_SEQUENCE_STARTUP_COMMAND_ENV.to_string(),
        args.startup_command.clone(),
    );

    if resolution.shell == SetupRunnerCommandShell::Windows {
        SequencedSetupAgentCommands {
            setup_command: build_windows_setup_command(&resolution.command, &marker_path, &nonce),
            startup_command: build_windows_startup_command(
                &marker_path,
                &nonce,
                wait_timeout_seconds,
            ),
            startup_env: Some(startup_env),
        }
    } else {
        SequencedSetupAgentCommands {
            setup_command: build_posix_setup_command(&resolution.command, &marker_path, &nonce),
            startup_command: build_posix_startup_command(
                &args.startup_command,
                &marker_path,
                &nonce,
                wait_timeout_seconds,
            ),
            startup_env: Some(startup_env),
        }
    }
}

/// `buildPosixSetupCommand` (`SAS:70-85`).
fn build_posix_setup_command(setup_command: &str, marker_path: &str, nonce: &str) -> String {
    let marker = quote_posix_arg(marker_path);
    let tmp = quote_posix_arg(&format!("{marker_path}.tmp"));
    let nonce_value = quote_posix_arg(nonce);

    let script = [
        format!("rm -f {marker} {tmp} 2>/dev/null"),
        format!("( {setup_command} )"),
        "status=$?".to_string(),
        format!("printf '%s:%s\\n' {nonce_value} \"$status\" > {tmp}"),
        format!("mv -f {tmp} {marker}"),
        "exit \"$status\"".to_string(),
    ]
    .join("; ");

    format!("bash -lc {}", quote_posix_arg(&script))
}

/// `buildPosixStartupCommand` (`SAS:87-123`).
fn build_posix_startup_command(
    startup_command: &str,
    marker_path: &str,
    nonce: &str,
    wait_timeout_seconds: i64,
) -> String {
    let marker = quote_posix_arg(marker_path);
    let tmp = quote_posix_arg(&format!("{marker_path}.tmp"));
    let nonce_value = quote_posix_arg(nonce);
    // K6: `Math.max(1, Math.floor(x))`; `i64` makes `floor` a no-op.
    let timeout = wait_timeout_seconds.max(1);
    let startup_success_command = build_posix_startup_success_command(startup_command);

    // `SAS:109`: `if [ -n "${ENV:-}" ]; then eval "$ENV"; ...` — built with
    // plain string pushes (rather than a `format!` with brace-escaping) to
    // keep the literal `${...}`/`$VAR` shell syntax unambiguous.
    let mut env_gate = String::new();
    env_gate.push_str("if [ \"$status\" = \"0\" ]; then if [ -n \"${");
    env_gate.push_str(SETUP_AGENT_SEQUENCE_STARTUP_COMMAND_ENV);
    env_gate.push_str(":-}\" ]; then eval \"$");
    env_gate.push_str(SETUP_AGENT_SEQUENCE_STARTUP_COMMAND_ENV);
    env_gate.push_str("\"; exit \"$?\"; else ");
    env_gate.push_str(&startup_success_command);
    env_gate.push_str("; fi; fi;");

    // Why: the PTY launch path feeds this command through an interactive
    // shell, so keeping the wrapper on one line avoids visible `quote>`
    // continuation prompts while still preserving valid `while`/`if` shell
    // syntax.
    let script = [
        format!("deadline=$((SECONDS + {timeout}));"),
        "echo \"Waiting for setup to finish before starting agent...\" >&2;".to_string(),
        "while :; do".to_string(),
        format!("if [ -f {marker} ]; then"),
        format!("IFS=: read -r seen status < {marker} || true;"),
        format!("if [ \"$seen\" = {nonce_value} ]; then"),
        format!("rm -f {marker} {tmp} 2>/dev/null;"),
        env_gate,
        "echo \"Setup failed; skipping agent startup.\" >&2;".to_string(),
        "exit \"${status:-1}\";".to_string(),
        "fi;".to_string(),
        "fi;".to_string(),
        "if [ \"$SECONDS\" -ge \"$deadline\" ]; then".to_string(),
        "echo \"Timed out waiting for setup before starting agent.\" >&2;".to_string(),
        "exit 124;".to_string(),
        "fi;".to_string(),
        "sleep 1;".to_string(),
        "done".to_string(),
    ]
    .join(" ");

    format!("bash -lc {}", quote_posix_arg(&script))
}

/// `buildPosixStartupSuccessCommand` (`SAS:125-133`). K5: the `exec`
/// interpolation is deliberately bare — see module docs.
fn build_posix_startup_success_command(startup_command: &str) -> String {
    if has_unquoted_posix_command_separator(startup_command)
        || has_leading_posix_env_assignment(startup_command)
    {
        format!("eval {}; exit \"$?\"", quote_posix_arg(startup_command))
    } else {
        format!("exec {startup_command}")
    }
}

/// `hasLeadingPosixEnvAssignment` (`SAS:135-137`):
/// `/^[A-Za-z_][A-Za-z0-9_]*=/.test(command.trimStart())`. K13: `trimStart`
/// uses ECMAScript whitespace via the local [`js_trim_start`], not
/// `str::trim_start`.
fn has_leading_posix_env_assignment(command: &str) -> bool {
    let trimmed = js_trim_start(command);
    let bytes = trimmed.as_bytes();
    if bytes.is_empty() || !(bytes[0].is_ascii_alphabetic() || bytes[0] == b'_') {
        return false;
    }
    for &b in &bytes[1..] {
        if b == b'=' {
            return true;
        }
        if !(b.is_ascii_alphanumeric() || b == b'_') {
            return false;
        }
    }
    false
}

/// K13: local ECMAScript leading-only trim (`.trimStart()`), built on
/// `suaegi_misc::is_js_whitespace` rather than adding a new export to
/// `suaegi-misc` itself.
fn js_trim_start(s: &str) -> &str {
    s.trim_start_matches(is_js_whitespace)
}

/// `hasUnquotedPosixCommandSeparator` (`SAS:139-166`): a hand-rolled
/// single/double-quote-aware scan for an unquoted `;`, `&`, `|`, or newline.
fn has_unquoted_posix_command_separator(command: &str) -> bool {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for ch in command.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            continue;
        }
        if matches!(ch, ';' | '&' | '|' | '\n' | '\r') {
            return true;
        }
    }
    false
}

/// `buildWindowsSetupCommand` (`SAS:168-179`).
fn build_windows_setup_command(setup_command: &str, marker_path: &str, nonce: &str) -> String {
    wrap_cmd(&[
        format!(
            "set \"ORCA_SETUP_MARKER={}\"",
            escape_cmd_set_value(marker_path)
        ),
        format!("set \"ORCA_SETUP_NONCE={}\"", escape_cmd_set_value(nonce)),
        "del /f /q \"!ORCA_SETUP_MARKER!\" \"!ORCA_SETUP_MARKER!.tmp\" 2>nul".to_string(),
        format!("call {setup_command}"),
        "set \"ORCA_SETUP_STATUS=!ERRORLEVEL!\"".to_string(),
        "> \"!ORCA_SETUP_MARKER!.tmp\" echo !ORCA_SETUP_NONCE!:!ORCA_SETUP_STATUS!".to_string(),
        "move /y \"!ORCA_SETUP_MARKER!.tmp\" \"!ORCA_SETUP_MARKER!\" >nul".to_string(),
        "exit /b !ORCA_SETUP_STATUS!".to_string(),
    ])
}

/// `buildWindowsStartupCommand` (`SAS:181-231`).
fn build_windows_startup_command(
    marker_path: &str,
    nonce: &str,
    wait_timeout_seconds: i64,
) -> String {
    // K6: same clamp as the POSIX side.
    let timeout = wait_timeout_seconds.max(1);
    // Why: native Windows setup runners launch through cmd.exe, but
    // PowerShell gives us safe bounded file polling/parsing without a
    // fragile batch label loop.
    let script = [
        "$marker = $env:ORCA_SETUP_MARKER".to_string(),
        "$tmp = $marker + \".tmp\"".to_string(),
        "$nonce = $env:ORCA_SETUP_NONCE".to_string(),
        format!("$deadline = (Get-Date).AddSeconds({timeout})"),
        "while ($true) {".to_string(),
        "  if (Test-Path -LiteralPath $marker) {".to_string(),
        "    $content = Get-Content -LiteralPath $marker -TotalCount 1".to_string(),
        "    if ($content -match \"^([0-9A-Za-z_-]+):([0-9]+)$\" -and $Matches[1] -eq $nonce) {"
            .to_string(),
        "      $setupStatus = [int]$Matches[2]".to_string(),
        "      Remove-Item -LiteralPath $marker, $tmp -Force -ErrorAction SilentlyContinue"
            .to_string(),
        "      if ($setupStatus -ne 0) {".to_string(),
        "        [Console]::Error.WriteLine(\"Setup failed; skipping agent startup.\")".to_string(),
        "        exit $setupStatus".to_string(),
        "      }".to_string(),
        format!("      $startup = $env:{SETUP_AGENT_SEQUENCE_STARTUP_COMMAND_ENV}"),
        "      if ([string]::IsNullOrWhiteSpace($startup)) {".to_string(),
        "        [Console]::Error.WriteLine(\"Missing sequenced startup command.\")".to_string(),
        "        exit 1".to_string(),
        "      }".to_string(),
        "      Invoke-Expression $startup".to_string(),
        "      if ($global:LASTEXITCODE -ne $null) { exit $global:LASTEXITCODE }".to_string(),
        "      if (-not $?) { exit 1 }".to_string(),
        "      exit 0".to_string(),
        "    }".to_string(),
        "  }".to_string(),
        "  if ((Get-Date) -ge $deadline) {".to_string(),
        "    [Console]::Error.WriteLine(\"Timed out waiting for setup before starting agent.\")"
            .to_string(),
        "    exit 124".to_string(),
        "  }".to_string(),
        "  Start-Sleep -Seconds 1".to_string(),
        "}".to_string(),
    ]
    .join("; ");

    wrap_cmd(&[
        format!(
            "set \"ORCA_SETUP_MARKER={}\"",
            escape_cmd_set_value(marker_path)
        ),
        format!("set \"ORCA_SETUP_NONCE={}\"", escape_cmd_set_value(nonce)),
        "echo Waiting for setup to finish before starting agent... 1>&2".to_string(),
        format!(
            "powershell.exe -NoProfile -ExecutionPolicy Bypass -Command {}",
            quote_windows_arg(&script)
        ),
        "set \"ORCA_SETUP_STATUS=!ERRORLEVEL!\"".to_string(),
        "exit /b !ORCA_SETUP_STATUS!".to_string(),
    ])
}

/// `wrapCmd` (`SAS:233-235`).
fn wrap_cmd(parts: &[String]) -> String {
    format!(
        "cmd.exe /d /s /v:on /c {}",
        quote_windows_arg(&parts.join(" & "))
    )
}

/// `quotePosixArg` (`SAS:237-242`). K2. Duplicated verbatim from
/// `setup_runner_command.rs` — see that module's doc comment on its own
/// copy for the duplication rationale.
fn quote_posix_arg(value: &str) -> String {
    if is_bare_safe_posix_arg(value) {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// `/^[A-Za-z0-9_./:-]+$/` (K2), duplicated per-module.
fn is_bare_safe_posix_arg(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'/' | b':' | b'-'))
}

/// `quoteWindowsArg` (`SAS:244-246`). K3. Duplicated verbatim from
/// `setup_runner_command.rs`.
fn quote_windows_arg(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

/// `escapeCmdSetValue` (`SAS:248-250`). K4: see module docs — ZERO oracle
/// coverage, the one construction in this crate that can inject. Ported
/// VERBATIM, not fixed.
fn escape_cmd_set_value(value: &str) -> String {
    let doubled = value.replace('"', "\"\"");
    let mut out = String::with_capacity(doubled.len());
    for ch in doubled.chars() {
        if matches!(ch, '%' | '!' | '^') {
            out.push('^');
        }
        out.push(ch);
    }
    out
}

/// `getSetupAgentSequenceShellForTests` (`SAS:252-257`). K11: kept `pub` —
/// exists purely for test observation (the oracle calls it directly); not
/// part of this module's production surface.
pub fn get_setup_agent_sequence_shell_for_tests(
    runner_script_path: &str,
    platform: SetupRunnerCommandPlatform,
) -> SetupRunnerCommandShell {
    resolve_setup_runner_command(runner_script_path, platform).shell
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(
        runner_script_path: &str,
        startup_command: &str,
        platform: SetupRunnerCommandPlatform,
        nonce: &str,
        wait_timeout_seconds: Option<i64>,
    ) -> CreateSequencedSetupAgentCommandsArgs {
        CreateSequencedSetupAgentCommandsArgs {
            runner_script_path: runner_script_path.to_string(),
            startup_command: startup_command.to_string(),
            platform,
            nonce: Some(nonce.to_string()),
            wait_timeout_seconds,
        }
    }

    // -----------------------------------------------------------------
    // Oracle: `setup-agent-sequencing.test.ts`
    // (SKIPPED: `:30-36` — exercises the unported `setup-agent-startup-policy`
    // module, not this one.)
    // -----------------------------------------------------------------

    #[test]
    fn oracle_uses_the_original_sequenced_startup_command_as_the_launch_hint_when_present() {
        let mut env = HashMap::new();
        env.insert(
            SETUP_AGENT_SEQUENCE_STARTUP_COMMAND_ENV.to_string(),
            "omp --resume".to_string(),
        );
        assert_eq!(
            resolve_setup_agent_sequence_launch_command(&env, Some("powershell wait-wrapper")),
            Some("omp --resume".to_string())
        );

        let mut whitespace_env = HashMap::new();
        whitespace_env.insert(
            SETUP_AGENT_SEQUENCE_STARTUP_COMMAND_ENV.to_string(),
            "   ".to_string(),
        );
        assert_eq!(
            resolve_setup_agent_sequence_launch_command(
                &whitespace_env,
                Some("powershell wait-wrapper")
            ),
            Some("powershell wait-wrapper".to_string())
        );
    }

    #[test]
    fn oracle_wraps_posix_setup_and_startup_commands_with_a_matching_nonce_marker() {
        let result = create_sequenced_setup_agent_commands(args(
            "/repo/.git/orca/setup-runner.sh",
            "codex 'fix bug'",
            SetupRunnerCommandPlatform::Posix,
            "nonce-123",
            Some(9),
        ));

        assert!(result.setup_command.starts_with("bash -lc "));
        assert!(result
            .setup_command
            .contains("bash /repo/.git/orca/setup-runner.sh"));
        assert!(result.setup_command.contains("printf"));
        assert!(result.setup_command.contains("nonce-123 \"$status\""));
        assert!(result
            .setup_command
            .contains("mv -f /repo/.git/orca/setup-runner.sh.nonce-123.done.tmp"));
        assert!(result.startup_command.starts_with("bash -lc "));
        assert!(result.startup_command.contains("deadline=$((SECONDS + 9))"));
        assert!(!result.startup_command.contains("date +%s"));
        assert!(result
            .startup_command
            .contains("Waiting for setup to finish before starting agent..."));
        assert!(result.startup_command.contains("[ \"$seen\" = nonce-123 ]"));
        assert!(result.startup_command.contains(
            "rm -f /repo/.git/orca/setup-runner.sh.nonce-123.done /repo/.git/orca/setup-runner.sh.nonce-123.done.tmp"
        ));
        assert!(result.startup_command.contains("exec codex"));
        assert!(result.startup_command.contains("fix bug"));
        assert_eq!(
            result
                .startup_env
                .unwrap()
                .get(SETUP_AGENT_SEQUENCE_STARTUP_COMMAND_ENV),
            Some(&"codex 'fix bug'".to_string())
        );
    }

    #[test]
    fn oracle_uses_launch_specific_marker_paths_for_overlapping_setup_gates() {
        let first = create_sequenced_setup_agent_commands(args(
            "/repo/.git/orca/setup-runner.sh",
            "claude",
            SetupRunnerCommandPlatform::Posix,
            "first-launch",
            None,
        ));
        let second = create_sequenced_setup_agent_commands(args(
            "/repo/.git/orca/setup-runner.sh",
            "codex",
            SetupRunnerCommandPlatform::Posix,
            "second-launch",
            None,
        ));

        assert!(first
            .setup_command
            .contains("/repo/.git/orca/setup-runner.sh.first-launch.done"));
        assert!(first
            .startup_command
            .contains("/repo/.git/orca/setup-runner.sh.first-launch.done"));
        assert!(second
            .setup_command
            .contains("/repo/.git/orca/setup-runner.sh.second-launch.done"));
        assert!(second
            .startup_command
            .contains("/repo/.git/orca/setup-runner.sh.second-launch.done"));
        assert!(!first
            .setup_command
            .contains("/repo/.git/orca/setup-runner.sh.second-launch.done"));
        assert!(!second
            .setup_command
            .contains("/repo/.git/orca/setup-runner.sh.first-launch.done"));
    }

    #[test]
    fn oracle_keeps_simple_posix_startup_commands_eligible_for_exec_when_quoted_text_has_separators(
    ) {
        let result = create_sequenced_setup_agent_commands(args(
            "/repo/.git/orca/setup-runner.sh",
            "codex 'fix this; then test'",
            SetupRunnerCommandPlatform::Posix,
            "nonce-quoted",
            Some(9),
        ));

        assert!(result
            .startup_command
            .contains("exec codex '\\''fix this; then test'\\''"));
        assert!(!result.startup_command.contains("eval codex"));
    }

    #[test]
    fn oracle_preserves_posix_inline_environment_assignment_startup_commands() {
        let result = create_sequenced_setup_agent_commands(args(
            "/repo/.git/orca/setup-runner.sh",
            "FOO=bar claude",
            SetupRunnerCommandPlatform::Posix,
            "nonce-env",
            Some(9),
        ));

        assert!(result.startup_command.contains("FOO=bar claude"));
        assert!(result.startup_command.contains("exit \"$?\""));
        assert!(!result.startup_command.contains("exec FOO=bar claude"));
    }

    #[test]
    fn oracle_uses_the_converted_linux_marker_path_for_wsl_unc_runners_on_windows() {
        let result = create_sequenced_setup_agent_commands(args(
            "\\\\wsl.localhost\\Ubuntu\\home\\jin\\repo\\.git\\worktrees\\feature\\orca\\setup-runner.sh",
            "claude",
            SetupRunnerCommandPlatform::Windows,
            "nonce-wsl",
            None,
        ));

        assert_eq!(
            get_setup_agent_sequence_shell_for_tests(
                "\\\\wsl.localhost\\Ubuntu\\home\\jin\\repo\\.git\\worktrees\\feature\\orca\\setup-runner.sh",
                SetupRunnerCommandPlatform::Windows
            ),
            SetupRunnerCommandShell::Posix
        );
        assert!(result
            .setup_command
            .contains("bash /home/jin/repo/.git/worktrees/feature/orca/setup-runner.sh"));
        assert!(result
            .setup_command
            .contains("/home/jin/repo/.git/worktrees/feature/orca/setup-runner.sh.nonce-wsl.done"));
        assert!(!result.setup_command.contains("wsl.localhost"));
    }

    #[test]
    fn oracle_keeps_remote_posix_runners_in_bash_even_from_a_windows_client() {
        let result = create_sequenced_setup_agent_commands(args(
            "/remote/repo/.git/worktrees/feature/orca/setup-runner.sh",
            "claude",
            SetupRunnerCommandPlatform::Windows,
            "nonce-remote",
            None,
        ));

        assert!(result
            .setup_command
            .contains("bash /remote/repo/.git/worktrees/feature/orca/setup-runner.sh"));
        assert!(result
            .startup_command
            .contains("[ \"$seen\" = nonce-remote ]"));
    }

    #[test]
    fn oracle_wraps_native_windows_runners_in_a_cmd_pinned_setup_and_startup_gate() {
        let result = create_sequenced_setup_agent_commands(args(
            "C:\\repo\\.git\\orca\\setup-runner.cmd",
            "codex --model gpt-5 'fix !PATH! & test'",
            SetupRunnerCommandPlatform::Windows,
            "nonce-win",
            Some(3),
        ));

        assert!(result.setup_command.contains("cmd.exe /d /s /v:on /c"));
        assert!(result
            .setup_command
            .contains("cmd.exe /c \"\"C:\\repo\\.git\\orca\\setup-runner.cmd\"\""));
        assert!(result
            .setup_command
            .contains("echo !ORCA_SETUP_NONCE!:!ORCA_SETUP_STATUS!"));
        assert_eq!(result.startup_command.matches("powershell.exe").count(), 1);
        assert!(result
            .startup_command
            .contains("powershell.exe -NoProfile -ExecutionPolicy Bypass"));
        assert!(result.startup_command.contains("AddSeconds(3)"));
        assert!(result.startup_command.contains("!ORCA_SETUP_STATUS!"));
        assert!(result
            .startup_command
            .contains("Timed out waiting for setup before starting agent."));
        assert!(result
            .startup_command
            .contains("Setup failed; skipping agent startup."));
        assert!(result.startup_command.contains(
            "Remove-Item -LiteralPath $marker, $tmp -Force -ErrorAction SilentlyContinue"
        ));
        assert!(!result.startup_command.contains("%ERRORLEVEL%"));
        assert!(!result.startup_command.contains(" & ) else"));
        assert!(!result
            .startup_command
            .contains("if \"\"!ORCA_SETUP_STATUS!\"\"==\"\"124\"\""));
        assert!(!result
            .startup_command
            .contains("if not \"\"!ORCA_SETUP_STATUS!\"\"==\"\"0\"\""));
        assert!(!result.startup_command.contains(&format!(
            "call !{SETUP_AGENT_SEQUENCE_STARTUP_COMMAND_ENV}!"
        )));
        assert!(result.startup_command.contains("Invoke-Expression"));
        assert!(!result.startup_command.contains("fix !PATH! & test"));
        let mut expected_env = HashMap::new();
        expected_env.insert(
            SETUP_AGENT_SEQUENCE_STARTUP_COMMAND_ENV.to_string(),
            "codex --model gpt-5 'fix !PATH! & test'".to_string(),
        );
        assert_eq!(result.startup_env, Some(expected_env));
    }

    // -----------------------------------------------------------------
    // Oracle: subprocess-spawning cases (`it.skipIf(process.platform ===
    // 'win32')`, `:203-365`). Ported using `std::process` only (no extra
    // dependency); gated on `#[cfg(unix)]` as the faithful equivalent of the
    // oracle's own win32 skip.
    // -----------------------------------------------------------------

    #[cfg(unix)]
    mod subprocess_oracle {
        use super::*;
        use std::fs;
        use std::io::Read;
        use std::process::{Command, Stdio};
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::Duration;

        fn make_temp_dir() -> std::path::PathBuf {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "suaegi-setupseq-{}-{}-{}",
                std::process::id(),
                n,
                create_setup_agent_sequence_nonce()
            ));
            fs::create_dir_all(&dir).expect("create temp dir");
            dir
        }

        fn write_executable(path: &std::path::Path, contents: &str) {
            fs::write(path, contents).expect("write script");
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(path, perms).unwrap();
        }

        fn read_if_exists(path: &std::path::Path) -> String {
            fs::read_to_string(path).unwrap_or_default()
        }

        /// Mirrors the oracle test file's local `quoteSh` helper.
        fn quote_sh(value: &str) -> String {
            if is_bare_safe_posix_arg(value) {
                value.to_string()
            } else {
                format!("'{}'", value.replace('\'', "'\\''"))
            }
        }

        struct ChildResult {
            code: Option<i32>,
            stderr: String,
        }

        fn wait_for_exit(mut child: std::process::Child) -> ChildResult {
            let status = child.wait().expect("wait for child");
            let mut stderr = String::new();
            if let Some(mut s) = child.stderr.take() {
                let _ = s.read_to_string(&mut stderr);
            }
            ChildResult {
                code: status.code(),
                stderr,
            }
        }

        fn spawn_bash(command: &str) -> std::process::Child {
            Command::new("bash")
                .arg("-lc")
                .arg(command)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn bash")
        }

        #[test]
        fn oracle_ignores_stale_markers_until_the_matching_setup_run_finishes_even_when_startup_launches_first(
        ) {
            let dir = make_temp_dir();
            let runner_script_path = dir.join("setup-runner.sh");
            let startup_script_path = dir.join("startup.sh");
            let log_path = dir.join("sequence.log");
            let marker_path = format!("{}.fresh-sequence.done", runner_script_path.display());

            write_executable(
                &runner_script_path,
                &format!(
                    "#!/bin/sh\nprintf 'setup-start\\n' >> {log}\nsleep 1\nprintf 'setup-done\\n' >> {log}\n",
                    log = quote_sh(&log_path.display().to_string())
                ),
            );
            write_executable(
                &startup_script_path,
                &format!(
                    "#!/bin/sh\nprintf 'agent-start\\n' >> {log}\n",
                    log = quote_sh(&log_path.display().to_string())
                ),
            );
            fs::write(&marker_path, "stale:0\n").unwrap();

            let commands = create_sequenced_setup_agent_commands(args(
                &runner_script_path.display().to_string(),
                &format!(
                    "bash {}",
                    quote_sh(&startup_script_path.display().to_string())
                ),
                SetupRunnerCommandPlatform::Posix,
                "fresh-sequence",
                Some(5),
            ));

            let startup_child = spawn_bash(&commands.startup_command);
            std::thread::sleep(Duration::from_millis(250));
            assert_eq!(read_if_exists(&log_path), "");
            assert_eq!(fs::read_to_string(&marker_path).unwrap(), "stale:0\n");

            let setup_status = Command::new("bash")
                .arg("-lc")
                .arg(&commands.setup_command)
                .status()
                .expect("run setup");
            assert_eq!(setup_status.code(), Some(0));

            let startup_result = wait_for_exit(startup_child);
            assert_eq!(startup_result.code, Some(0));

            assert_eq!(
                fs::read_to_string(&log_path).unwrap(),
                "setup-start\nsetup-done\nagent-start\n"
            );
            assert_eq!(read_if_exists(std::path::Path::new(&marker_path)), "");
            assert_eq!(
                read_if_exists(std::path::Path::new(&format!("{marker_path}.tmp"))),
                ""
            );

            let _ = fs::remove_dir_all(&dir);
        }

        #[test]
        fn oracle_runs_compound_posix_startup_cleanup_commands_after_setup_succeeds() {
            let dir = make_temp_dir();
            let runner_script_path = dir.join("setup-runner.sh");
            let log_path = dir.join("sequence.log");

            write_executable(
                &runner_script_path,
                &format!(
                    "#!/bin/sh\nprintf 'setup-done\\n' >> {log}\n",
                    log = quote_sh(&log_path.display().to_string())
                ),
            );

            let commands = create_sequenced_setup_agent_commands(args(
                &runner_script_path.display().to_string(),
                &format!(
                    "printf 'agent-start\\n' >> {log}; printf 'cleanup\\n' >> {log}",
                    log = quote_sh(&log_path.display().to_string())
                ),
                SetupRunnerCommandPlatform::Posix,
                "compound-sequence",
                Some(5),
            ));

            let setup_child = spawn_bash(&commands.setup_command);
            let startup_result = wait_for_exit(spawn_bash(&commands.startup_command));
            let setup_result = wait_for_exit(setup_child);

            assert_eq!(setup_result.code, Some(0));
            assert_eq!(startup_result.code, Some(0));
            assert_eq!(
                fs::read_to_string(&log_path).unwrap(),
                "setup-done\nagent-start\ncleanup\n"
            );
            assert!(commands.startup_command.contains("eval"));
            assert!(!commands.startup_command.contains("exec printf"));

            let _ = fs::remove_dir_all(&dir);
        }

        #[test]
        fn oracle_prefers_the_env_provided_startup_command_after_setup_succeeds() {
            let dir = make_temp_dir();
            let runner_script_path = dir.join("setup-runner.sh");
            let startup_script_path = dir.join("startup.sh");
            let log_path = dir.join("sequence.log");

            write_executable(
                &runner_script_path,
                &format!(
                    "#!/bin/sh\nprintf 'setup-done\\n' >> {log}\n",
                    log = quote_sh(&log_path.display().to_string())
                ),
            );
            write_executable(
                &startup_script_path,
                &format!(
                    "#!/bin/sh\nif [ \"$FOO\" = \"bar\" ]; then\n  printf 'env-start\\n' >> {log}\nfi\n",
                    log = quote_sh(&log_path.display().to_string())
                ),
            );

            let commands = create_sequenced_setup_agent_commands(args(
                &runner_script_path.display().to_string(),
                &format!(
                    "printf 'inline-start\\n' >> {log}",
                    log = quote_sh(&log_path.display().to_string())
                ),
                SetupRunnerCommandPlatform::Posix,
                "env-sequence",
                Some(5),
            ));

            let setup_child = spawn_bash(&commands.setup_command);

            let override_command = format!(
                "FOO=bar bash {}; printf 'env-cleanup\\n' >> {}",
                quote_sh(&startup_script_path.display().to_string()),
                quote_sh(&log_path.display().to_string())
            );
            let startup_child = Command::new("bash")
                .arg("-lc")
                .arg(&commands.startup_command)
                .env(SETUP_AGENT_SEQUENCE_STARTUP_COMMAND_ENV, override_command)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn startup with env override");

            let startup_result = wait_for_exit(startup_child);
            let setup_result = wait_for_exit(setup_child);

            assert_eq!(setup_result.code, Some(0));
            assert_eq!(startup_result.code, Some(0));
            assert_eq!(
                fs::read_to_string(&log_path).unwrap(),
                "setup-done\nenv-start\nenv-cleanup\n"
            );

            let _ = fs::remove_dir_all(&dir);
        }

        #[test]
        fn oracle_times_out_instead_of_hanging_forever_when_setup_never_writes_a_matching_marker() {
            let dir = make_temp_dir();
            let runner_script_path = dir.join("setup-runner.sh");
            write_executable(&runner_script_path, "#!/bin/sh\nexit 0\n");

            let commands = create_sequenced_setup_agent_commands(args(
                &runner_script_path.display().to_string(),
                "printf ready",
                SetupRunnerCommandPlatform::Posix,
                "timeout-sequence",
                Some(1),
            ));

            let startup_result = wait_for_exit(spawn_bash(&commands.startup_command));

            assert_eq!(startup_result.code, Some(124));
            assert!(startup_result
                .stderr
                .contains("Timed out waiting for setup before starting agent."));

            let _ = fs::remove_dir_all(&dir);
        }
    }

    // -----------------------------------------------------------------
    // K1: windows platform + WSL UNC path -> posix shell (also pinned in
    // `setup_runner_command.rs`; repeated here through this module's own
    // test-observation export, K11).
    // -----------------------------------------------------------------

    #[test]
    fn k1_windows_platform_with_wsl_unc_path_yields_posix_shell() {
        assert_eq!(
            get_setup_agent_sequence_shell_for_tests(
                "\\\\wsl.localhost\\Ubuntu\\home\\jin\\repo\\setup-runner.sh",
                SetupRunnerCommandPlatform::Windows
            ),
            SetupRunnerCommandShell::Posix
        );
    }

    // -----------------------------------------------------------------
    // K2: bare/quoted boundary + `'` -> `'\''`, on THIS module's own copy.
    // -----------------------------------------------------------------

    #[test]
    fn k2_bare_safe_characters_stay_unquoted() {
        assert_eq!(quote_posix_arg("a-b:c/d.e_1"), "a-b:c/d.e_1");
    }

    #[test]
    fn k2_space_or_single_quote_forces_quoting() {
        assert_eq!(quote_posix_arg("a b"), "'a b'");
        assert_eq!(quote_posix_arg("it's"), "'it'\\''s'");
    }

    // -----------------------------------------------------------------
    // K3: a `"`-bearing value through this module's own `quote_windows_arg`.
    // -----------------------------------------------------------------

    #[test]
    fn k3_quote_windows_arg_doubles_embedded_quotes_breaking_cmd_parity() {
        assert_eq!(
            quote_windows_arg("C:\\a\"&calc&\".cmd"),
            "\"C:\\a\"\"&calc&\"\".cmd\""
        );
    }

    // -----------------------------------------------------------------
    // K4: `escape_cmd_set_value` for `"`, `%`, `!`, `^` individually, plus
    // the full-command injectable snapshot.
    // -----------------------------------------------------------------

    #[test]
    fn k4_escape_cmd_set_value_doubles_embedded_quotes() {
        assert_eq!(escape_cmd_set_value("a\"b"), "a\"\"b");
    }

    #[test]
    fn k4_escape_cmd_set_value_carets_percent_cargo_cult() {
        // K4: `^` does NOT actually escape `%` in cmd.exe — cargo-cult,
        // reproduced verbatim regardless.
        assert_eq!(escape_cmd_set_value("50%"), "50^%");
    }

    #[test]
    fn k4_escape_cmd_set_value_carets_bang_meaningfully_under_delayed_expansion() {
        // K4: `^!` IS meaningful — `wrap_cmd`'s caller always runs under
        // `/v:on` (delayed expansion enabled).
        assert_eq!(escape_cmd_set_value("go!"), "go^!");
    }

    #[test]
    fn k4_escape_cmd_set_value_carets_caret() {
        assert_eq!(escape_cmd_set_value("a^b"), "a^^b");
    }

    #[test]
    fn k4_quote_bearing_marker_path_breaks_cmd_quote_parity_in_the_full_setup_command() {
        // K4: DELIBERATELY PRESERVED UPSTREAM HAZARD. `"` is illegal in NTFS
        // filenames and the nonce is a UUID in practice, so this exact input
        // is unreachable in production — but if `markerPath` ever contained
        // a `"`, `escape_cmd_set_value`'s doubling composes badly with
        // `wrap_cmd`'s outer re-quoting: the `&calc&` here ends up in an
        // UNQUOTED region of the final cmd.exe command line, executable.
        let marker_path = "C:\\a\"&calc&\".done";
        let command = build_windows_setup_command(
            "cmd.exe /c \"C:\\repo\\setup-runner.cmd\"",
            marker_path,
            "nonce-1",
        );
        assert_eq!(
            command,
            "cmd.exe /d /s /v:on /c \"set \"\"ORCA_SETUP_MARKER=C:\\a\"\"\"\"&calc&\"\"\"\".done\"\" & set \"\"ORCA_SETUP_NONCE=nonce-1\"\" & del /f /q \"\"!ORCA_SETUP_MARKER!\"\" \"\"!ORCA_SETUP_MARKER!.tmp\"\" 2>nul & call cmd.exe /c \"\"C:\\repo\\setup-runner.cmd\"\" & set \"\"ORCA_SETUP_STATUS=!ERRORLEVEL!\"\" & > \"\"!ORCA_SETUP_MARKER!.tmp\"\" echo !ORCA_SETUP_NONCE!:!ORCA_SETUP_STATUS! & move /y \"\"!ORCA_SETUP_MARKER!.tmp\"\" \"\"!ORCA_SETUP_MARKER!\"\" >nul & exit /b !ORCA_SETUP_STATUS!\""
        );
        // The hazard, made explicit: `escape_cmd_set_value` already doubled
        // the marker path's `"` to `""` (2 chars); `wrap_cmd`'s outer
        // `quote_windows_arg` doubles EVERY `"` in the whole joined line a
        // SECOND time, so each of those becomes `""""` (4 chars) — a
        // visibly different, unbalanced-looking run compared to the single
        // `""` used everywhere a `"` was NOT already escaper-doubled (e.g.
        // around `!ORCA_SETUP_MARKER!` itself). This 4-quote run is exactly
        // where `&calc&` sits.
        assert!(command.contains("C:\\a\"\"\"\"&calc&\"\"\"\".done"));
    }

    // -----------------------------------------------------------------
    // K6: timeout 0, negative, and 1.
    // -----------------------------------------------------------------

    #[test]
    fn k6_wait_timeout_floors_and_clamps_to_a_minimum_of_one() {
        for (input, expected) in [(0i64, 1i64), (-5, 1), (1, 1), (2, 2)] {
            let result = create_sequenced_setup_agent_commands(args(
                "/repo/setup-runner.sh",
                "echo hi",
                SetupRunnerCommandPlatform::Posix,
                "n",
                Some(input),
            ));
            assert!(
                result
                    .startup_command
                    .contains(&format!("deadline=$((SECONDS + {expected}))")),
                "input {input} expected clamp to {expected}, got: {}",
                result.startup_command
            );
        }
    }

    // -----------------------------------------------------------------
    // K7: a nonce containing `:`.
    // -----------------------------------------------------------------

    #[test]
    fn k7_nonce_containing_colon_breaks_the_ifs_read_split() {
        // `:` is in K2's bare-safe char class, so the nonce stays unquoted
        // in the comparison — textually safe. Semantically hazardous: the
        // marker line `nonce:status` (written with THIS nonce) will itself
        // split into more than two `IFS=:` fields when read back, so
        // `$seen` (only the text before the FIRST `:`) can never again equal
        // the full colon-bearing nonce compared against here — the startup
        // gate always times out.
        let result = create_sequenced_setup_agent_commands(args(
            "/repo/setup-runner.sh",
            "claude",
            SetupRunnerCommandPlatform::Posix,
            "nonce:with:colons",
            Some(5),
        ));
        assert!(result
            .startup_command
            .contains("[ \"$seen\" = nonce:with:colons ]"));
        // The setup script's own `printf '%s:%s\n' ...` format string uses
        // literal single quotes, and the ENTIRE script is then wrapped by
        // `quote_posix_arg` a second time — so those inner quotes come out
        // as the `'\''` escape sequence, not bare `'`.
        assert!(result
            .setup_command
            .contains("printf '\\''%s:%s\\n'\\'' nonce:with:colons \"$status\""));
    }

    // -----------------------------------------------------------------
    // K10: whitespace-only env value falls back; U+FEFF vs U+0085 in both
    // directions.
    // -----------------------------------------------------------------

    #[test]
    fn k10_whitespace_only_env_value_falls_back_to_the_launch_command() {
        let mut env = HashMap::new();
        env.insert(
            SETUP_AGENT_SEQUENCE_STARTUP_COMMAND_ENV.to_string(),
            "   ".to_string(),
        );
        assert_eq!(
            resolve_setup_agent_sequence_launch_command(&env, Some("powershell wait-wrapper")),
            Some("powershell wait-wrapper".to_string())
        );
    }

    #[test]
    fn k10_feff_is_js_whitespace_and_trims_away_to_an_empty_fallback() {
        let mut env = HashMap::new();
        env.insert(
            SETUP_AGENT_SEQUENCE_STARTUP_COMMAND_ENV.to_string(),
            "\u{FEFF}".to_string(),
        );
        assert_eq!(
            resolve_setup_agent_sequence_launch_command(&env, Some("fallback")),
            Some("fallback".to_string())
        );
    }

    #[test]
    fn k10_u0085_is_not_js_whitespace_and_survives_as_the_launch_hint() {
        let mut env = HashMap::new();
        env.insert(
            SETUP_AGENT_SEQUENCE_STARTUP_COMMAND_ENV.to_string(),
            "\u{0085}".to_string(),
        );
        assert_eq!(
            resolve_setup_agent_sequence_launch_command(&env, Some("fallback")),
            Some("\u{0085}".to_string())
        );
    }

    #[test]
    fn k10_missing_env_key_falls_back() {
        let env = HashMap::new();
        assert_eq!(
            resolve_setup_agent_sequence_launch_command(&env, Some("fallback")),
            Some("fallback".to_string())
        );
        assert_eq!(
            resolve_setup_agent_sequence_launch_command(&env, None),
            None
        );
    }
}
