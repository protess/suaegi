//! 외부 에디터 런처 — Orca `external-editor-launch.ts` 순수 포팅.
//!
//! command(에디터 설정) + 워크트리 경로 → spawn spec. 핵심(crux)은 argv 구성과
//! shell 이스케이프로, 악의적 editor-command 설정이나 shell 메타문자를 담은 경로가
//! 명령을 주입하지 못하게 막는다. `platform`/`file_exists`를 주입 파라미터로 받아
//! win32/macos/linux와 존재 여부를 실제 fs/플랫폼을 건드리지 않고 검증한다(Orca의
//! `options.platform`/`options.fileExists` 등가, `:158-164`).
//!
//! 불변식: 에디터 command는 파라미터로 들어온다(suaegi 자체 설정에서). suaegi는
//! 사용자 전역 config(`~/.config`)를 절대 읽거나 쓰지 않는다.

use std::io;

/// Orca `EXTERNAL_EDITOR_CLI_COMMAND` (`:7`) — command 미지정 시 기본값.
const EXTERNAL_EDITOR_CLI_COMMAND: &str = "code";

/// Orca `WINDOWS_CONSOLE_EDITORS` (`:8`) — win32에서 콘솔을 숨기지 않는 TUI 에디터.
const WINDOWS_CONSOLE_EDITORS: &[&str] = &["nvim", "vim"];

/// Orca `VSCODE_REMOTE_EDITORS` (`:9`) — WSL UNC 경로에 `--remote`를 붙일 대상.
const VSCODE_REMOTE_EDITORS: &[&str] = &["code", "code-insiders", "code - insiders"];

/// 주입되는 대상 플랫폼. Orca `NodeJS.Platform`의 관련 부분집합.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Macos,
    Linux,
    Win32,
}

impl Platform {
    /// 현재 컴파일 대상의 플랫폼. spawn 경로에서 기본값으로 쓴다.
    pub fn current() -> Self {
        if cfg!(target_os = "windows") {
            Platform::Win32
        } else if cfg!(target_os = "macos") {
            Platform::Macos
        } else {
            Platform::Linux
        }
    }
}

/// Orca `ExternalEditorLaunchSpec` (`:11-23`)의 Rust 등가.
///
/// `hide_windows_console`는 Orca에서 두 variant 모두에 존재한다(`:14,20`) —
/// nvim/vim의 shell 분기(`nvim --clean`)가 콘솔을 유지해야 하므로 Shell에도 둔다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchSpec {
    /// 직접 실행: `Command::new(program).args(args)`. argv는 shell 해석을 거치지
    /// 않으므로 경로 이스케이프가 필요 없다.
    Executable {
        program: String,
        args: Vec<String>,
        hide_windows_console: bool,
    },
    /// shell 경유: POSIX는 `/bin/sh -c "<cmd> <escaped-path>"`, win32는
    /// `cmd.exe /d /s /c "<cmd> <escaped-path>"`. 경로는 반드시 이스케이프된다.
    Shell {
        program: String,
        args: Vec<String>,
        hide_windows_console: bool,
    },
}

/// Orca `escapePosixPathForShell` (`:25-30`).
///
/// safe charset(`^[a-zA-Z0-9_./@:-]+$`)이면 그대로, 아니면 single-quote로 감싸고
/// 내부 `'`를 `'\''`로 치환한다. 이것이 argv 주입을 막는 유일한 가드다.
fn escape_posix_path_for_shell(path_value: &str) -> String {
    if is_posix_safe(path_value) {
        return path_value.to_string();
    }
    format!("'{}'", path_value.replace('\'', "'\\''"))
}

/// `^[a-zA-Z0-9_./@:-]+$` — 비어 있으면(`+`) 안전하지 않다.
fn is_posix_safe(s: &str) -> bool {
    !s.is_empty()
        && s.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'/' | b'@' | b':' | b'-')
        })
}

/// Orca `escapeWindowsPathForShell` (`:32-34`).
///
/// safe charset(`^[a-zA-Z0-9_./@:\\-]+$`, 백슬래시 포함)이면 그대로, 아니면
/// double-quote로 감싼다.
fn escape_windows_path_for_shell(path_value: &str) -> String {
    if is_windows_safe(path_value) {
        return path_value.to_string();
    }
    format!("\"{path_value}\"")
}

fn is_windows_safe(s: &str) -> bool {
    !s.is_empty()
        && s.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(b, b'_' | b'.' | b'/' | b'@' | b':' | b'\\' | b'-')
        })
}

/// Orca `escapePathForShell` (`:36-40`).
fn escape_path_for_shell(path_value: &str, platform: Platform) -> String {
    match platform {
        Platform::Win32 => escape_windows_path_for_shell(path_value),
        _ => escape_posix_path_for_shell(path_value),
    }
}

/// Orca `stripMatchingQuotes` (`:62-69`) — 앞뒤로 짝이 맞는 따옴표만 벗긴다.
fn strip_matching_quotes(value: &str) -> String {
    let trimmed = value.trim();
    let mut chars = trimmed.chars();
    if let Some(quote) = chars.next() {
        if (quote == '"' || quote == '\'') && trimmed.len() >= 2 && trimmed.ends_with(quote) {
            return trimmed[quote.len_utf8()..trimmed.len() - quote.len_utf8()].to_string();
        }
    }
    trimmed.to_string()
}

/// Orca `hasMatchingOuterQuotes` (`:71-75`).
fn has_matching_outer_quotes(value: &str) -> bool {
    let trimmed = value.trim();
    let mut chars = trimmed.chars();
    match chars.next() {
        Some(quote @ ('"' | '\'')) => trimmed.len() >= 2 && trimmed.ends_with(quote),
        _ => false,
    }
}

/// Orca `getLeadingShellCommandToken` (`:50-60`) — 따옴표로 감싼 첫 토큰 또는
/// 첫 공백-구분 토큰.
fn leading_shell_command_token(command: &str) -> String {
    let trimmed = command.trim();
    let mut chars = trimmed.chars();
    if let Some(quote @ ('"' | '\'')) = chars.next() {
        if let Some(rel) = trimmed[quote.len_utf8()..].find(quote) {
            // closingIndex > 0: 여는 따옴표 다음에서 찾은 위치.
            return trimmed[quote.len_utf8()..quote.len_utf8() + rel].to_string();
        }
    }
    trimmed.split_whitespace().next().unwrap_or("").to_string()
}

/// Orca `getLauncherBaseName` (`:42-48`).
///
/// shell 커맨드면 첫 토큰을, 아니면 따옴표를 벗긴 전체를 취해 basename 추출 후
/// `.cmd/.exe/.bat` 확장자를 벗기고 소문자로 만든다. 백슬래시가 있으면 win32
/// basename(백슬래시/슬래시 둘 다 구분자)을, 없으면 POSIX basename을 쓴다.
fn launcher_base_name(command: &str, shell_command: bool) -> String {
    let normalized = if shell_command {
        leading_shell_command_token(command)
    } else {
        strip_matching_quotes(command)
    };
    let name = if normalized.contains('\\') {
        win32_basename(&normalized)
    } else {
        posix_basename(&normalized)
    };
    strip_exe_suffix(&name).to_ascii_lowercase()
}

/// `.cmd`/`.exe`/`.bat` 접미사를 대소문자 무시하고 제거(Orca `:47`의 정규식).
fn strip_exe_suffix(name: &str) -> &str {
    for ext in ["cmd", "exe", "bat"] {
        if let Some(dot) = name.len().checked_sub(ext.len() + 1) {
            if name.as_bytes().get(dot) == Some(&b'.') && name[dot + 1..].eq_ignore_ascii_case(ext)
            {
                return &name[..dot];
            }
        }
    }
    name
}

/// POSIX basename: 마지막 `/` 뒤. Node `path.basename` 등가(뒤 슬래시 무시).
fn posix_basename(p: &str) -> String {
    basename_with(p, &['/'])
}

/// win32 basename: 마지막 `/` 또는 `\` 뒤. Node `path.win32.basename` 등가.
fn win32_basename(p: &str) -> String {
    basename_with(p, &['/', '\\'])
}

fn basename_with(p: &str, seps: &[char]) -> String {
    let trimmed = p.trim_end_matches(|c| seps.contains(&c));
    match trimmed.rfind(|c| seps.contains(&c)) {
        Some(idx) => trimmed[idx + 1..].to_string(),
        None => trimmed.to_string(),
    }
}

/// Orca `isWindowsExecutablePath` (`:77-79`) — win32 절대경로이고 실행 확장자.
fn is_windows_executable_path(command: &str) -> bool {
    win32_is_absolute(command) && has_windows_exec_ext(command)
}

fn has_windows_exec_ext(command: &str) -> bool {
    for ext in ["cmd", "exe", "bat", "com"] {
        if let Some(dot) = command.len().checked_sub(ext.len() + 1) {
            if command.as_bytes().get(dot) == Some(&b'.')
                && command[dot + 1..].eq_ignore_ascii_case(ext)
            {
                return true;
            }
        }
    }
    false
}

/// Node `path.posix.isAbsolute` 등가 — `/`로 시작.
fn posix_is_absolute(p: &str) -> bool {
    p.starts_with('/')
}

/// Node `path.win32.isAbsolute` 등가 — `\`/`/`로 시작(UNC 포함) 또는 `C:\`/`C:/`.
fn win32_is_absolute(p: &str) -> bool {
    let b = p.as_bytes();
    if b.first().is_some_and(|&c| c == b'/' || c == b'\\') {
        return true;
    }
    // 드라이브 문자 + 콜론 + 구분자.
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'/' || b[2] == b'\\')
}

/// Orca `isDirectExecutablePath` (`:81-101`).
fn is_direct_executable_path(
    command: &str,
    platform: Platform,
    file_exists: &impl Fn(&str) -> bool,
) -> bool {
    let unquoted = strip_matching_quotes(command);
    if !unquoted.contains(['\\', '/']) {
        return false;
    }
    let is_absolute = match platform {
        Platform::Win32 => win32_is_absolute(&unquoted),
        _ => posix_is_absolute(&unquoted),
    };
    if !is_absolute {
        return false;
    }
    // 공백이 없거나, 바깥 따옴표가 짝지어져 있으면 하나의 실행파일로 확정.
    if !unquoted.contains(char::is_whitespace) || has_matching_outer_quotes(command) {
        return true;
    }
    // Why(Orca `:98-100`): 따옴표 없는 POSIX 경로는 공백을 담을 수 있으나 인자를
    // 붙인 shell 커맨드도 그렇다. 존재하는 경로만 하나의 실행파일로 취급.
    match platform {
        Platform::Win32 => is_windows_executable_path(&unquoted),
        _ => file_exists(&unquoted),
    }
}

/// Orca `shouldShowWindowsConsole` (`:103-109`).
fn should_show_windows_console(command: &str, platform: Platform, shell_command: bool) -> bool {
    platform == Platform::Win32
        && WINDOWS_CONSOLE_EDITORS.contains(&launcher_base_name(command, shell_command).as_str())
}

/// Orca `isCompoundShellCommand` (`:132-134`) — 공백을 담으면 복합 커맨드.
fn is_compound_shell_command(command: &str) -> bool {
    command.contains(char::is_whitespace)
}

/// Orca WSL UNC 파싱 (`wsl-paths.ts` `parseWslUncPath`). `\\wsl.localhost\<distro>\<path>`
/// 또는 `\\wsl$\<distro>\<path>` → (distro, linuxPath). 경로 없으면 linuxPath = "/".
struct WslUncInfo {
    distro: String,
    linux_path: String,
}

fn parse_wsl_unc_path(path: &str) -> Option<WslUncInfo> {
    let normalized = path.replace('\\', "/");
    let rest = normalized.strip_prefix("//")?;
    // 첫 세그먼트는 wsl.localhost / wsl$ (대소문자 무시).
    let (share, after_share) = split_first_segment(rest);
    if !(share.eq_ignore_ascii_case("wsl.localhost") || share.eq_ignore_ascii_case("wsl$")) {
        return None;
    }
    let after_share = after_share?;
    // 두 번째 세그먼트는 distro(비어 있으면 매치 실패 — `[^/]+`).
    let (distro, after_distro) = split_first_segment(after_share);
    if distro.is_empty() {
        return None;
    }
    let linux_path = match after_distro {
        Some(rest) => format!("/{rest}"),
        None => "/".to_string(),
    };
    Some(WslUncInfo {
        distro: distro.to_string(),
        linux_path,
    })
}

/// `"a/b/c"` → `("a", Some("b/c"))`, `"a"` → `("a", None)`.
fn split_first_segment(s: &str) -> (&str, Option<&str>) {
    match s.find('/') {
        Some(idx) => (&s[..idx], Some(&s[idx + 1..])),
        None => (s, None),
    }
}

/// Orca `buildExecutableArgs` (`:111-130`).
fn build_executable_args(
    editor_command: &str,
    path_value: &str,
    platform: Platform,
) -> Vec<String> {
    let base = launcher_base_name(editor_command, false);
    if base == "cursor" {
        // Why(Orca `:117-120`): Cursor의 bare 폴더 실행은 마지막 활성 workbench로
        // 라우팅될 수 있다. --new-window로 "Open in Cursor"를 이 워크트리에 고정.
        return vec!["--new-window".to_string(), path_value.to_string()];
    }
    if platform == Platform::Win32 && VSCODE_REMOTE_EDITORS.contains(&base.as_str()) {
        if let Some(wsl) = parse_wsl_unc_path(path_value) {
            // Why(Orca `:122-127`): 아니면 VS Code가 WSL UNC 경로를 로컬 폴더로 취급.
            return vec![
                "--remote".to_string(),
                format!("wsl+{}", wsl.distro),
                wsl.linux_path,
            ];
        }
    }
    vec![path_value.to_string()]
}

/// Orca `buildShellLaunchSpec` (`:136-156`).
fn build_shell_launch_spec(command: &str, path_value: &str, platform: Platform) -> LaunchSpec {
    let shell_command = format!("{command} {}", escape_path_for_shell(path_value, platform));
    if platform == Platform::Win32 {
        return LaunchSpec::Shell {
            hide_windows_console: !should_show_windows_console(command, platform, true),
            program: cmd_exe_path(),
            args: vec![
                "/d".to_string(),
                "/s".to_string(),
                "/c".to_string(),
                shell_command,
            ],
        };
    }
    LaunchSpec::Shell {
        hide_windows_console: true,
        program: "/bin/sh".to_string(),
        args: vec!["-c".to_string(), shell_command],
    }
}

/// Orca `getCmdExePath` (`win32-utils.ts:39-41`)의 축약 — win32 spawn에서만 쓰인다.
/// 실제 win32 실행 시 `ComSpec`을 우선한다.
fn cmd_exe_path() -> String {
    std::env::var("ComSpec").unwrap_or_else(|_| {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
        format!("{root}\\System32\\cmd.exe")
    })
}

/// Orca `resolveExternalEditorLaunchSpec` (`:158-188`)의 포팅.
///
/// `command`가 None/빈문자열/공백뿐이면 기본 `code`로 대체(`:165`). 세 분기:
/// 1. 직접 실행파일 경로(`:167-175`) → Executable.
/// 2. 복합 shell 커맨드(공백 포함, `:177-179`) → Shell(이스케이프된 경로).
/// 3. 그 외 bare CLI(`:181-187`) → Executable.
///
/// deviation: Orca `:181`의 `resolveCliCommand`(GUI PATH 증강 탐색)는 여기서
/// identity다 — suaegi는 spawn 시점 `Command::new`의 OS PATH 조회에 맡긴다. Orca
/// 테스트가 `resolveCliCommand`를 identity로 mock하므로 패리티 테이블과 동일하게
/// 동작한다.
pub fn resolve_external_editor_launch_spec(
    command: Option<&str>,
    path_value: &str,
    platform: Platform,
    file_exists: impl Fn(&str) -> bool,
) -> LaunchSpec {
    let trimmed = match command.map(str::trim) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => EXTERNAL_EDITOR_CLI_COMMAND.to_string(),
    };

    // 분기 1: 직접 실행파일 경로(`:167-175`).
    if is_direct_executable_path(&trimmed, platform, &file_exists) {
        let editor_command = strip_matching_quotes(&trimmed);
        return LaunchSpec::Executable {
            hide_windows_console: !should_show_windows_console(&editor_command, platform, false),
            args: build_executable_args(&editor_command, path_value, platform),
            program: editor_command,
        };
    }

    // 분기 2: 복합 shell 커맨드(`:177-179`).
    if is_compound_shell_command(&trimmed) {
        return build_shell_launch_spec(&trimmed, path_value, platform);
    }

    // 분기 3: bare CLI(`:181-187`). resolveCliCommand = identity(위 deviation 참조).
    let editor_command = trimmed;
    LaunchSpec::Executable {
        hide_windows_console: !should_show_windows_console(&editor_command, platform, false),
        args: build_executable_args(&editor_command, path_value, platform),
        program: editor_command,
    }
}

/// spec을 detached로 spawn한다 — stdio 무시, 즉시 반환(UI 블록 없음).
///
/// Orca `shell.ts:64-71,92-95`의 `detached:true, stdio:'ignore'` + `unref` 등가.
/// argv 정확성은 순수 함수 테스트가 이미 증명하므로 여기는 얇게 유지한다.
pub fn spawn_external_editor(spec: &LaunchSpec) -> io::Result<()> {
    use std::process::{Command, Stdio};

    let (program, args) = match spec {
        LaunchSpec::Executable { program, args, .. } => (program, args),
        LaunchSpec::Shell { program, args, .. } => (program, args),
    };

    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    // unix에서 setsid로 자식을 우리 세션에서 분리한다(부모 종료와 무관하게 생존).
    // setsid는 세션만 분리할 뿐 부모-자식 reaping은 바꾸지 않으므로, 아래에서
    // 백그라운드 스레드가 wait해 좀비를 거둔다. Windows에서는 hide_windows_console에
    // 따라 콘솔을 숨기는 것이 이상적이나(CREATE_NO_WINDOW), macOS-first라 여기서는
    // 세션 분리만 이식한다.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // setsid: 자기 세션 리더로 만들어 부모 종료와 무관하게 산다.
        unsafe {
            cmd.pre_exec(|| {
                libc_setsid();
                Ok(())
            });
        }
    }

    let child = cmd.spawn()?;
    // 즉시 반환해 UI를 블록하지 않되, 백그라운드 스레드가 자식을 wait로 reap한다.
    // Child를 그냥 drop하면 wait가 일어나지 않아 `open -a`/`code`/`cursor` shim처럼
    // 곧장 종료하는 자식이 suaegi 수명 내내 좀비로 쌓인다 — 스레드가 그걸 막는다.
    std::thread::spawn(move || {
        let mut child = child;
        let _ = child.wait();
    });
    Ok(())
}

/// `setsid(2)` 직접 호출 — `libc` 의존을 피하려 syscall을 raw로 부른다.
///
/// deviation: 새 의존을 추가하지 않으려는 목적. 실패해도 무시(부모가 죽으면 자식이
/// 함께 죽을 수 있으나, 에디터 실행에 치명적이지 않다).
#[cfg(unix)]
fn libc_setsid() {
    extern "C" {
        fn setsid() -> i32;
    }
    unsafe {
        setsid();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 대부분의 테스트에서 파일은 존재하지 않는다고 본다.
    fn no_files(_: &str) -> bool {
        false
    }

    // ---- 기본 command fallback (Orca `:165`) ----

    #[test]
    fn none_command_falls_back_to_code() {
        let spec = resolve_external_editor_launch_spec(None, "/tmp/ws", Platform::Macos, no_files);
        assert_eq!(
            spec,
            LaunchSpec::Executable {
                program: "code".to_string(),
                args: vec!["/tmp/ws".to_string()],
                hide_windows_console: true,
            }
        );
    }

    #[test]
    fn empty_and_whitespace_command_fall_back_to_code() {
        for cmd in ["", "   ", "\t\n"] {
            let spec = resolve_external_editor_launch_spec(
                Some(cmd),
                "/tmp/ws",
                Platform::Macos,
                no_files,
            );
            match spec {
                LaunchSpec::Executable { program, .. } => {
                    assert_eq!(program, "code", "cmd={cmd:?}")
                }
                other => panic!("expected code executable, got {other:?}"),
            }
        }
    }

    // ---- Cursor --new-window (Orca `:117-120`, 테스트 `:20-30`) ----

    #[test]
    fn cursor_gets_new_window_flag() {
        let spec = resolve_external_editor_launch_spec(
            Some("cursor"),
            "/tmp/workspace",
            Platform::Macos,
            no_files,
        );
        assert_eq!(
            spec,
            LaunchSpec::Executable {
                program: "cursor".to_string(),
                args: vec!["--new-window".to_string(), "/tmp/workspace".to_string()],
                hide_windows_console: true,
            }
        );
    }

    // ---- 복합 macOS open 커맨드 + 이스케이프 (Orca 테스트 `:32-43`) ----

    #[test]
    fn compound_macos_open_command_escapes_path() {
        let spec = resolve_external_editor_launch_spec(
            Some("open -a \"Typora\""),
            "/tmp/note's.md",
            Platform::Macos,
            no_files,
        );
        assert_eq!(
            spec,
            LaunchSpec::Shell {
                program: "/bin/sh".to_string(),
                args: vec![
                    "-c".to_string(),
                    "open -a \"Typora\" '/tmp/note'\\''s.md'".to_string(),
                ],
                hide_windows_console: true,
            }
        );
    }

    // ---- 존재하는 POSIX 실행파일(공백 포함) → executable (Orca 테스트 `:45-58`) ----

    #[test]
    fn existing_posix_executable_with_spaces_is_executable() {
        let idea = "/Users/me/Library/Application Support/JetBrains/Toolbox/scripts/idea";
        let spec = resolve_external_editor_launch_spec(
            Some(idea),
            "/tmp/workspace",
            Platform::Macos,
            |c| c == idea,
        );
        assert_eq!(
            spec,
            LaunchSpec::Executable {
                program: idea.to_string(),
                args: vec!["/tmp/workspace".to_string()],
                hide_windows_console: true,
            }
        );
    }

    // ---- 절대 POSIX 커맨드 + 인자 → shell (Orca 테스트 `:60-72`) ----

    #[test]
    fn absolute_posix_command_with_args_is_shell() {
        let spec = resolve_external_editor_launch_spec(
            Some("/usr/local/bin/code --reuse-window"),
            "/tmp/workspace",
            Platform::Macos,
            no_files,
        );
        assert_eq!(
            spec,
            LaunchSpec::Shell {
                program: "/bin/sh".to_string(),
                args: vec![
                    "-c".to_string(),
                    "/usr/local/bin/code --reuse-window /tmp/workspace".to_string(),
                ],
                hide_windows_console: true,
            }
        );
    }

    // ---- stripMatchingQuotes: 따옴표 감싼 직접 경로 (Orca 테스트 `:111-124`) ----

    #[test]
    fn quoted_direct_path_strips_quotes_win32() {
        let spec = resolve_external_editor_launch_spec(
            Some("\"C:\\Program Files\\Neovim\\bin\\nvim.exe\""),
            "C:\\workspaces\\orca",
            Platform::Win32,
            no_files,
        );
        assert_eq!(
            spec,
            LaunchSpec::Executable {
                program: "C:\\Program Files\\Neovim\\bin\\nvim.exe".to_string(),
                args: vec!["C:\\workspaces\\orca".to_string()],
                hide_windows_console: false, // nvim → 콘솔 표시
            }
        );
    }

    #[test]
    fn unquoted_win32_executable_with_spaces_is_executable() {
        let spec = resolve_external_editor_launch_spec(
            Some("C:\\Program Files\\Neovim\\bin\\nvim.exe"),
            "C:\\workspaces\\orca",
            Platform::Win32,
            no_files,
        );
        assert_eq!(
            spec,
            LaunchSpec::Executable {
                program: "C:\\Program Files\\Neovim\\bin\\nvim.exe".to_string(),
                args: vec!["C:\\workspaces\\orca".to_string()],
                hide_windows_console: false,
            }
        );
    }

    // ---- nvim/vim → win32 콘솔 유지 (Orca 테스트 `:126-137`, shell 분기) ----

    #[test]
    fn nvim_shell_command_shows_windows_console() {
        let spec = resolve_external_editor_launch_spec(
            Some("nvim --clean"),
            "C:\\workspaces\\orca",
            Platform::Win32,
            no_files,
        );
        assert_eq!(
            spec,
            LaunchSpec::Shell {
                program: cmd_exe_path(),
                args: vec![
                    "/d".to_string(),
                    "/s".to_string(),
                    "/c".to_string(),
                    "nvim --clean C:\\workspaces\\orca".to_string(),
                ],
                hide_windows_console: false,
            }
        );
    }

    // ---- 복합 win32 커맨드 → cmd.exe (Orca 테스트 `:74-83`) ----

    #[test]
    fn compound_win32_command_runs_through_cmd_exe() {
        let spec = resolve_external_editor_launch_spec(
            Some("start \"\" notepad"),
            "C:\\note.md",
            Platform::Win32,
            no_files,
        );
        assert_eq!(
            spec,
            LaunchSpec::Shell {
                program: cmd_exe_path(),
                args: vec![
                    "/d".to_string(),
                    "/s".to_string(),
                    "/c".to_string(),
                    "start \"\" notepad C:\\note.md".to_string(),
                ],
                hide_windows_console: true,
            }
        );
    }

    #[test]
    fn win32_path_with_spaces_is_double_quoted_in_compound() {
        let spec = resolve_external_editor_launch_spec(
            Some("start \"\" notepad"),
            "C:\\my notes.md",
            Platform::Win32,
            no_files,
        );
        match spec {
            LaunchSpec::Shell { args, .. } => {
                assert_eq!(args[3], "start \"\" notepad \"C:\\my notes.md\"");
            }
            other => panic!("expected shell, got {other:?}"),
        }
    }

    // ---- win32 VSCode + WSL UNC → --remote (Orca 테스트 `:139-165,204-212`) ----

    #[test]
    fn vscode_wsl_unc_uses_remote() {
        for editor in ["code", "code-insiders"] {
            let spec = resolve_external_editor_launch_spec(
                Some(editor),
                "\\\\wsl.localhost\\Ubuntu\\home\\aliuq\\project",
                Platform::Win32,
                no_files,
            );
            match spec {
                LaunchSpec::Executable { args, .. } => assert_eq!(
                    args,
                    vec![
                        "--remote".to_string(),
                        "wsl+Ubuntu".to_string(),
                        "/home/aliuq/project".to_string()
                    ],
                    "editor={editor}"
                ),
                other => panic!("expected executable, got {other:?}"),
            }
        }
    }

    #[test]
    fn legacy_wsl_unc_and_distro_roots() {
        let cases = [
            (
                "\\\\wsl$\\Debian\\home\\ada\\project",
                "wsl+Debian",
                "/home/ada/project",
            ),
            ("\\\\wsl.localhost\\Ubuntu", "wsl+Ubuntu", "/"),
            ("\\\\wsl$\\Debian", "wsl+Debian", "/"),
        ];
        for (path, authority, linux) in cases {
            let spec =
                resolve_external_editor_launch_spec(Some("code"), path, Platform::Win32, no_files);
            match spec {
                LaunchSpec::Executable { args, .. } => assert_eq!(
                    args,
                    vec![
                        "--remote".to_string(),
                        authority.to_string(),
                        linux.to_string()
                    ],
                    "path={path}"
                ),
                other => panic!("expected executable, got {other:?}"),
            }
        }
    }

    #[test]
    fn wsl_distro_and_folder_spaces_preserved() {
        let spec = resolve_external_editor_launch_spec(
            Some("code"),
            "\\\\wsl.localhost\\Ubuntu Preview\\home\\Ada Lovelace\\project",
            Platform::Win32,
            no_files,
        );
        match spec {
            LaunchSpec::Executable { args, .. } => assert_eq!(
                args,
                vec![
                    "--remote".to_string(),
                    "wsl+Ubuntu Preview".to_string(),
                    "/home/Ada Lovelace/project".to_string()
                ]
            ),
            other => panic!("expected executable, got {other:?}"),
        }
    }

    #[test]
    fn direct_win32_vscode_launcher_recognized() {
        // 직접 win32 실행파일 경로(.exe/.cmd/.bat) → executable, basename이 code면 remote.
        for editor in [
            "C:\\Program Files\\Microsoft VS Code\\Code.exe",
            "C:\\Tools\\CODE.CMD",
            "C:\\Tools\\code.bat",
            "C:\\Tools\\code-insiders.cmd",
        ] {
            let spec = resolve_external_editor_launch_spec(
                Some(editor),
                "\\\\wsl.localhost\\Ubuntu\\home\\ada\\project",
                Platform::Win32,
                no_files,
            );
            match spec {
                LaunchSpec::Executable { args, .. } => assert_eq!(
                    args,
                    vec![
                        "--remote".to_string(),
                        "wsl+Ubuntu".to_string(),
                        "/home/ada/project".to_string()
                    ],
                    "editor={editor}"
                ),
                other => panic!("expected executable, got {other:?}"),
            }
        }
    }

    #[test]
    fn wsl_looking_paths_stay_local_on_posix() {
        for platform in [Platform::Macos, Platform::Linux] {
            let path = "\\\\wsl.localhost\\Ubuntu\\home\\ada\\project";
            let spec = resolve_external_editor_launch_spec(Some("code"), path, platform, no_files);
            match spec {
                LaunchSpec::Executable { args, .. } => {
                    assert_eq!(args, vec![path.to_string()], "platform={platform:?}")
                }
                other => panic!("expected executable, got {other:?}"),
            }
        }
    }

    #[test]
    fn non_wsl_win32_paths_stay_local() {
        for path in ["C:\\workspaces\\orca", "\\\\server\\share\\project"] {
            let spec =
                resolve_external_editor_launch_spec(Some("code"), path, Platform::Win32, no_files);
            match spec {
                LaunchSpec::Executable { args, .. } => {
                    assert_eq!(args, vec![path.to_string()], "path={path}")
                }
                other => panic!("expected executable, got {other:?}"),
            }
        }
    }

    #[test]
    fn other_editors_do_not_get_vscode_remote() {
        let path = "\\\\wsl.localhost\\Ubuntu\\home\\ada\\project";
        // cursor.exe → --new-window (remote 아님)
        let spec = resolve_external_editor_launch_spec(
            Some("C:\\Tools\\cursor.exe"),
            path,
            Platform::Win32,
            no_files,
        );
        match spec {
            LaunchSpec::Executable { args, .. } => {
                assert_eq!(args, vec!["--new-window".to_string(), path.to_string()])
            }
            other => panic!("expected executable, got {other:?}"),
        }
        // codium.exe → 그냥 경로
        let spec = resolve_external_editor_launch_spec(
            Some("C:\\Tools\\codium.exe"),
            path,
            Platform::Win32,
            no_files,
        );
        match spec {
            LaunchSpec::Executable { args, .. } => assert_eq!(args, vec![path.to_string()]),
            other => panic!("expected executable, got {other:?}"),
        }
    }

    #[test]
    fn compound_vscode_command_not_rewritten() {
        let path = "\\\\wsl.localhost\\Ubuntu\\home\\ada\\project";
        let spec = resolve_external_editor_launch_spec(
            Some("code --reuse-window"),
            path,
            Platform::Win32,
            no_files,
        );
        assert_eq!(
            spec,
            LaunchSpec::Shell {
                program: cmd_exe_path(),
                args: vec![
                    "/d".to_string(),
                    "/s".to_string(),
                    "/c".to_string(),
                    format!("code --reuse-window {path}"),
                ],
                hide_windows_console: true,
            }
        );
    }

    // ==== THE crux: argv 주입 이스케이프 (mutation-verify) ====
    //
    // shell 분기가 만드는 `/bin/sh -c "<editor> <path>"` 문자열에서, shell
    // 메타문자를 담은 워크트리 경로는 반드시 single-quote로 감싸져 하나의 인자가
    // 되어야 한다. escape_posix_path_for_shell을 무력화(경로를 그대로 반환하거나
    // single-quote wrap을 건너뜀)하면 아래 세 테스트가 FAIL한다.

    /// `;`로 명령을 연쇄하려는 경로 → 통째로 single-quote 안에 갇혀야 한다.
    #[test]
    fn injection_semicolon_path_is_single_quoted() {
        let spec = resolve_external_editor_launch_spec(
            Some("code --wait"), // 복합 → shell 분기
            "/tmp/x; touch /tmp/PWNED",
            Platform::Macos,
            no_files,
        );
        match spec {
            LaunchSpec::Shell { args, .. } => {
                // 정확한 이스케이프 결과를 고정. `;`가 따옴표 밖에 노출되면 안 된다.
                assert_eq!(args[1], "code --wait '/tmp/x; touch /tmp/PWNED'");
                // 방어적: 경로 전체가 single-quote 쌍 안에 갇혀야 한다(주입 토큰이
                // shell에 raw로 노출되지 않음).
                assert!(args[1].ends_with("'/tmp/x; touch /tmp/PWNED'"));
            }
            other => panic!("expected shell, got {other:?}"),
        }
    }

    /// `$(...)` 명령치환 → single-quote 안에서는 shell이 확장하지 않는다.
    #[test]
    fn injection_command_substitution_is_single_quoted() {
        let spec = resolve_external_editor_launch_spec(
            Some("code --wait"),
            "/tmp/$(whoami)",
            Platform::Macos,
            no_files,
        );
        match spec {
            LaunchSpec::Shell { args, .. } => {
                assert_eq!(args[1], "code --wait '/tmp/$(whoami)'");
            }
            other => panic!("expected shell, got {other:?}"),
        }
    }

    /// 내부 single-quote가 있는 경로 → `'\''`로 안전하게 닫고 다시 연다.
    #[test]
    fn injection_embedded_single_quote_is_escaped() {
        let spec = resolve_external_editor_launch_spec(
            Some("code --wait"),
            "/tmp/a b'c",
            Platform::Macos,
            no_files,
        );
        match spec {
            LaunchSpec::Shell { args, .. } => {
                // 공백 → 감싸야 함, 내부 ' → '\'' 로.
                assert_eq!(args[1], "code --wait '/tmp/a b'\\''c'");
            }
            other => panic!("expected shell, got {other:?}"),
        }
    }

    /// safe-charset 경로는 불필요하게 감싸지 않는다(Orca 패리티 — 동작 동일).
    /// escape가 "항상 감싸도록" 변형되면 이 테스트가 FAIL → 무해한 변형도 잡힌다.
    #[test]
    fn safe_charset_path_is_not_quoted() {
        let spec = resolve_external_editor_launch_spec(
            Some("code --wait"),
            "/tmp/clean-path_1.2/file",
            Platform::Macos,
            no_files,
        );
        match spec {
            LaunchSpec::Shell { args, .. } => {
                assert_eq!(args[1], "code --wait /tmp/clean-path_1.2/file");
            }
            other => panic!("expected shell, got {other:?}"),
        }
    }

    /// executable 분기: 공백 담은 직접 경로는 별개 argv 원소로 — shell 분할이
    /// 없으므로 이스케이프 불필요(Rust Command::args는 shell 해석 안 함).
    #[test]
    fn executable_branch_passes_path_as_single_arg() {
        let idea = "/Applications/My Editor.app/Contents/MacOS/editor";
        let spec = resolve_external_editor_launch_spec(
            Some(idea),
            "/tmp/a b; touch c", // 메타문자 있어도 argv 원소 하나로
            Platform::Macos,
            |c| c == idea,
        );
        match spec {
            LaunchSpec::Executable { program, args, .. } => {
                assert_eq!(program, idea);
                assert_eq!(args, vec!["/tmp/a b; touch c".to_string()]);
            }
            other => panic!("expected executable, got {other:?}"),
        }
    }

    // ---- unit: escape 함수 직접 ----

    #[test]
    fn escape_posix_safe_passthrough() {
        assert_eq!(
            escape_posix_path_for_shell("/a/b_c.d@e:f-g"),
            "/a/b_c.d@e:f-g"
        );
    }

    #[test]
    fn escape_posix_wraps_unsafe() {
        assert_eq!(escape_posix_path_for_shell("a b"), "'a b'");
        assert_eq!(escape_posix_path_for_shell("a'b"), "'a'\\''b'");
        assert_eq!(escape_posix_path_for_shell("$(x)"), "'$(x)'");
    }

    #[test]
    fn strip_matching_quotes_behavior() {
        assert_eq!(strip_matching_quotes("\"abc\""), "abc");
        assert_eq!(strip_matching_quotes("'abc'"), "abc");
        assert_eq!(strip_matching_quotes("abc"), "abc");
        assert_eq!(strip_matching_quotes("\"abc"), "\"abc"); // 짝 안 맞음
        assert_eq!(strip_matching_quotes("  \"a b\"  "), "a b");
    }

    // ---- spawn: 실제로 detached 실행되는지 (mutation: args 누락 감지) ----

    /// spec의 program/args가 실제 spawn에 그대로 전달되는지 — 마커 파일로 확인.
    /// 이 테스트는 argv 배선(spawn이 args를 넘기는지)을 고정한다.
    #[cfg(unix)]
    #[test]
    fn spawn_actually_runs_with_args() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("ran");
        let spec = LaunchSpec::Executable {
            program: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), format!("touch {}", marker.display())],
            hide_windows_console: true,
        };
        spawn_external_editor(&spec).expect("spawn must succeed");
        // detached child라 wait하지 않는다 — 마커가 나타날 때까지 짧게 폴링.
        let mut ok = false;
        for _ in 0..200 {
            if marker.exists() {
                ok = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(ok, "detached child must have run with its args");
    }
}
