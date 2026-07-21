//! 훅 주입 — worktree의 `.claude/settings.local.json`과 훅 스크립트 생성.
//!
//! **사용자의 Claude 설정을 건드리지 않는다.** Orca는 `~/.claude/settings.json`을
//! 직접 고치는데 따라 하지 않는다. 대신 **suaegi가 만든 worktree 안에** 설정을 쓴다 —
//! 그 디렉터리는 우리 것이지 사용자의 저장소가 아니다.
//!
//! **왜 `--settings`가 아닌가**: `suaegi-app`은 `claude`를 실행하지 않는다. 모든 세션이
//! 평범한 로그인 셸이라(`state.rs`가 `AgentKind::Custom, None`으로 스폰한다) 넘길 argv가
//! 없다. 사용자가 프롬프트에서 `claude`를 어떻게 띄우든(맨손, `--resume`, 별칭) 적용되는
//! 것도 이쪽의 이점이다.
//!
//! **`.git/info/exclude`를 건드리지 않는다.** 그 파일은 worktree가 아니라 **공용 git
//! 디렉터리**에 살아서, worktree에서 쓰면 사용자의 저장소 전체에 영구적인 무시 규칙이
//! 생기고 `git worktree remove` 후에도 남는다. 우리가 만든 `.claude/`를 diff에서 빼는
//! 일은 diff 패널이 자기 수집 단계에서 한다.
//!
//! ## 실측 (2.1.216, 이 파일을 쓰기 전에 확인)
//!
//! 아래 셋은 어디에도 기록돼 있지 않아 직접 돌려 확인했다:
//! 1. worktree의 `.claude/settings.local.json`이 **실제로 적용된다** — `SessionStart`,
//!    `PreToolUse`, `PostToolUse`, `Stop`이 전부 발화했다.
//! 2. 도구 이벤트에 **`matcher`가 필요 없다** — 없이도 `PreToolUse`/`PostToolUse`가
//!    발화한다. (필요한데 빠뜨렸다면 조용히 영영 발화하지 않았을 것이다.)
//! 3. 앱이 심은 **env가 훅 프로세스까지 도달한다** — `SUAEGI_PANE_KEY`/`SUAEGI_SPAWN_NONCE`를
//!    훅 스크립트에서 그대로 읽었다. 이 설계 전체가 여기에 걸려 있다.

use std::io;
use std::path::{Path, PathBuf};

use super::contract::{PaneKey, SpawnNonce};
use super::parse::encode_pane_key;

/// 등록하는 훅 이벤트. **`Notification`은 없다** — `PermissionRequest`보다 6초 늦게
/// 오므로 배지 신호로 쓸 값이 없다(실측).
///
/// `SessionEnd`는 등록하되 리듀서는 **무시한다**(`contract::hook_outcome` 참고):
/// 종료 판정은 presence 폴링이 권위다. 등록해 두는 것은 진단용이고 비용이 없다.
pub const REGISTERED_EVENTS: [&str; 9] = [
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "Stop",
    "StopFailure",
    "SessionEnd",
];

/// 훅 스크립트 본문.
///
/// 세 가지가 **전부 필수다**(조사 §1.5): 훅은 턴을 블록하므로 suaegi가 죽었거나
/// 느려도 사용자의 에이전트가 멎으면 안 된다.
/// 1. **stdin을 언제나 먼저 비운다** — 어느 경로로 빠져나가든.
/// 2. `curl --max-time 1.5` — 서버가 멈춰도 1.5초 안에 포기한다.
/// 3. **항상 `exit 0`** — 0이 아닌 종료는 stderr 첫 줄이 사용자 트랜스크립트에 뜬다.
///
/// `curl` 존재까지 확인하는 이유는 없을 때 셸이 "command not found"를 내고 **stdin이
/// 읽히지 않은 채** 끝나기 때문이다. 그래서 본문을 변수로 먼저 빨아들인다.
pub const HOOK_SCRIPT: &str = r#"#!/bin/sh
# suaegi 훅 — fire-and-forget 관찰자. 이 파일은 suaegi가 생성한다.
# **항상 exit 0으로 끝난다.** 훅은 턴을 블록하므로 여기서 실패하면 사용자의
# 에이전트가 멎는다.

# stdin은 어느 경로로 빠져나가든 **먼저** 비운다.
body=$(cat 2>/dev/null)

[ -n "$SUAEGI_HOOK_PORT" ] || exit 0
[ -n "$SUAEGI_HOOK_TOKEN" ] || exit 0
[ -n "$SUAEGI_PANE_KEY" ] || exit 0
command -v curl >/dev/null 2>&1 || exit 0

printf '%s' "$body" | curl -sS --max-time 1.5 -X POST \
  -H "X-Suaegi-Token: $SUAEGI_HOOK_TOKEN" \
  -H "X-Suaegi-Pane: $SUAEGI_PANE_KEY" \
  -H "X-Suaegi-Nonce: $SUAEGI_SPAWN_NONCE" \
  -H 'Content-Type: application/json' \
  --data-binary @- \
  "http://127.0.0.1:$SUAEGI_HOOK_PORT/hook/claude" >/dev/null 2>&1

exit 0
"#;

/// POSIX 홑따옴표 이스케이프. 홑따옴표 안에서는 홑따옴표만이 특별하므로
/// `'` → `'\''`(닫고, 이스케이프된 따옴표, 다시 열기) 하나로 끝난다.
///
/// 경로에 공백·따옴표·`$`·백틱이 있어도 안전해야 한다 — 사용자의 홈 디렉터리
/// 이름은 우리가 고르지 않는다.
pub fn posix_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// settings에 들어갈 훅 커맨드 한 줄.
///
/// **존재 가드가 핵심이다.** 스크립트가 지워졌는데 settings에 항목이 남아 있으면
/// 셸이 exit 127을 내고, 그것이 **모든 도구 호출마다** 사용자 트랜스크립트에
/// 오류로 뜬다. 가드가 없는 쪽의 실패는 조용하지 않다.
///
/// 없는 경로에서도 `cat >/dev/null`로 **stdin을 비운다** — 훅 커맨드가 stdin을
/// 남기면 파이프가 어떻게 정리되는지는 우리 통제 밖이다.
pub fn hook_command(script: &Path) -> String {
    let quoted = posix_single_quote(&script.to_string_lossy());
    format!("if [ -x {quoted} ]; then {quoted}; else cat >/dev/null 2>&1; fi; exit 0")
}

/// worktree에 쓸 `.claude/settings.local.json` 내용.
///
/// **모든 훅에 `"async": true`** — 실측으로 턴 지연이 18.4s에서 3.0s로 떨어지고
/// 전달은 유지된다. 대가는 훅이 결정(거부/수정)을 돌려줄 수 없다는 것인데, 우리
/// 훅은 fire-and-forget 관찰자라 비용이 0이다. **나중에 누가 같은 설정에 차단형
/// 결정 훅을 더하면 조용히 무시된다** — 그래서 여기 적어 둔다.
///
/// `matcher`를 넣지 않는 것은 실측 결과다(위 모듈 주석 2번).
pub fn settings_json(script: &Path) -> String {
    let command = hook_command(script);
    let hooks: serde_json::Map<String, serde_json::Value> = REGISTERED_EVENTS
        .iter()
        .map(|event| {
            (
                (*event).to_string(),
                serde_json::json!([{
                    "hooks": [{
                        "type": "command",
                        "command": command,
                        "async": true,
                    }]
                }]),
            )
        })
        .collect();
    // `to_string_pretty`인 것은 사용자가 열어볼 수 있는 파일이기 때문이다.
    serde_json::to_string_pretty(&serde_json::json!({ "hooks": hooks }))
        .expect("a map of strings and bools always serializes")
}

/// 훅 스크립트가 사는 곳. `dirs::config_dir()`를 쓰는 이유는
/// `persistence_thread.rs`가 이미 그 규칙을 쓰기 때문이다 — `~/.suaegi`에
/// 하드코딩하면 주 개발 플랫폼(macOS)에서 앱의 디스크 흔적이 두 곳으로 갈린다.
pub fn hook_script_path() -> PathBuf {
    let base = dirs::config_dir().unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".suaegi")
    });
    base.join("suaegi").join("hooks").join("suaegi-hook.sh")
}

/// 훅 스크립트를 설치한다(있으면 덮어쓴다 — 내용이 바뀌었을 수 있다).
/// 실행 비트를 세우지 않으면 `hook_command`의 `-x` 가드가 영영 거짓이 되어
/// 훅이 조용히 하나도 발화하지 않는다.
pub fn install_hook_script(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, HOOK_SCRIPT)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

/// worktree에 설정 파일을 쓴다. worktree 생성 직후에 부른다.
pub fn write_worktree_settings(worktree: &Path, script: &Path) -> io::Result<()> {
    let dir = worktree.join(".claude");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("settings.local.json"), settings_json(script))
}

/// 세션 스폰에 얹을 환경 변수.
///
/// **토큰은 env로만 간다.** `--settings`에 인라인하면 argv라 `ps`에 보인다.
/// pane 키는 **이미 인코딩해서** 심는다 — unix 경로는 개행과 임의 바이트를 담을 수
/// 있어 날것으로 헤더에 실으면 헤더 주입이다.
///
/// `suaegi-term`은 이 변수들의 **의미를 모른다** — 그냥 env다. 의존 방향이 유지된다.
pub fn spawn_env(
    pane_key: &PaneKey,
    nonce: SpawnNonce,
    port: u16,
    token: &str,
) -> Vec<(String, String)> {
    vec![
        ("SUAEGI_PANE_KEY".to_string(), encode_pane_key(pane_key)),
        ("SUAEGI_SPAWN_NONCE".to_string(), nonce.0.to_string()),
        ("SUAEGI_HOOK_PORT".to_string(), port.to_string()),
        ("SUAEGI_HOOK_TOKEN".to_string(), token.to_string()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use suaegi_core::domain::WorktreeId;

    // ---- POSIX 이스케이프: 경로 이름은 우리가 고르지 않는다 ----

    #[test]
    fn single_quoting_survives_the_characters_a_home_directory_can_contain() {
        assert_eq!(posix_single_quote("/tmp/plain"), "'/tmp/plain'");
        assert_eq!(posix_single_quote("/tmp/with space"), "'/tmp/with space'");
        assert_eq!(
            posix_single_quote("/tmp/it's"),
            r"'/tmp/it'\''s'",
            "an embedded single quote must close, escape, and reopen — anything else \
             ends the quoting early and the rest of the path becomes shell code"
        );
        // 홑따옴표 안에서는 이것들이 전부 리터럴이다.
        assert_eq!(posix_single_quote("/tmp/$HOME"), "'/tmp/$HOME'");
        assert_eq!(posix_single_quote("/tmp/`id`"), "'/tmp/`id`'");
        assert_eq!(posix_single_quote("/tmp/a\"b"), "'/tmp/a\"b'");
    }

    /// 이스케이프가 실제로 **셸에서** 옳은지 확인한다. 문자열 비교만으로는
    /// 규칙을 잘못 알고 있어도 통과한다.
    #[test]
    fn the_quoted_path_round_trips_through_a_real_shell() {
        for raw in [
            "/tmp/plain",
            "/tmp/with space",
            "/tmp/it's",
            "/tmp/$HOME",
            "/tmp/`id`",
            "/tmp/a\"b",
            "/tmp/semi;colon",
        ] {
            let out = std::process::Command::new("sh")
                .arg("-c")
                .arg(format!("printf '%s' {}", posix_single_quote(raw)))
                .output()
                .expect("sh must run");
            assert_eq!(
                String::from_utf8_lossy(&out.stdout),
                raw,
                "the shell must see exactly the original path for {raw:?}"
            );
        }
    }

    // ---- 훅 커맨드: 가드와 stdin 배출 ----

    #[test]
    fn the_hook_command_guards_on_the_script_existing() {
        let cmd = hook_command(Path::new("/tmp/suaegi-hook.sh"));
        assert!(
            cmd.contains("[ -x '/tmp/suaegi-hook.sh' ]"),
            "without an existence guard a stale settings entry exits 127 on EVERY tool \
             call and the user sees an error in their transcript each time; got {cmd}"
        );
        assert!(
            cmd.contains("cat >/dev/null"),
            "the miss path must still drain stdin; got {cmd}"
        );
        assert!(cmd.contains("exit 0"), "the hook must never fail the turn");
    }

    /// 스크립트가 **없을 때** 커맨드가 실제로 성공하고 stdin을 비우는지 —
    /// 진짜 셸로 확인한다.
    #[test]
    fn a_missing_script_still_exits_zero_and_consumes_stdin() {
        use std::io::Write;
        use std::process::{Command, Stdio};

        let cmd = hook_command(Path::new("/tmp/suaegi-definitely-not-here-xyz.sh"));
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("sh must run");
        // 훅이 받는 것과 같은 모양의 본문을 밀어 넣는다.
        child
            .stdin
            .as_mut()
            .expect("piped")
            .write_all(br#"{"session_id":"s","hook_event_name":"Stop"}"#)
            .expect("the command must consume stdin rather than break the pipe");
        let status = child.wait().expect("sh must finish");
        assert!(
            status.success(),
            "a missing hook script must not fail the user's turn; status = {status:?}"
        );
    }

    /// 스크립트가 **있을 때** 실행되는지. 위 테스트만으로는 가드가 항상 거짓이어도
    /// 통과한다.
    #[test]
    fn an_installed_script_is_actually_executed() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("ran");
        let script = dir.path().join("hook.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\ncat >/dev/null 2>&1\ntouch {}\nexit 0\n",
                marker.display()
            ),
        )
        .unwrap();
        install_permissions(&script);

        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(hook_command(&script))
            .stdin(std::process::Stdio::null())
            .status()
            .expect("sh must run");
        assert!(status.success());
        assert!(
            marker.exists(),
            "control: when the script IS present the guard must let it run — otherwise \
             the guard is simply always false and no hook ever fires"
        );
    }

    #[cfg(unix)]
    fn install_permissions(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    #[cfg(not(unix))]
    fn install_permissions(_path: &Path) {}

    // ---- settings JSON ----

    #[test]
    fn every_registered_event_is_present_and_async() {
        let json: serde_json::Value =
            serde_json::from_str(&settings_json(Path::new("/tmp/h.sh"))).expect("valid JSON");
        let hooks = json["hooks"].as_object().expect("a hooks object");

        assert_eq!(
            hooks.len(),
            REGISTERED_EVENTS.len(),
            "exactly the registered events, no more"
        );
        for event in REGISTERED_EVENTS {
            // 모양은 `{"<Event>": [ {"hooks": [ {...} ]} ]}` — 바깥이 matcher 그룹의
            // 배열이고 그 안에 훅 배열이 있다. 실측한 구조 그대로다.
            let entry = &hooks[event][0]["hooks"][0];
            assert_eq!(entry["type"], "command", "{event} must be a command hook");
            assert_eq!(
                entry["async"], true,
                "{event} must be async — a synchronous hook blocks the user's turn \
                 (measured: 18.4s vs 3.0s)"
            );
        }
    }

    /// `Notification`은 `PermissionRequest`보다 6초 늦으므로 배지 신호로 쓸 값이 없다.
    /// 등록하면 그 지연을 그대로 사는 데다 이벤트만 늘어난다.
    #[test]
    fn notification_is_deliberately_not_registered() {
        let json: serde_json::Value =
            serde_json::from_str(&settings_json(Path::new("/tmp/h.sh"))).unwrap();
        assert!(
            json["hooks"].get("Notification").is_none(),
            "Notification arrives 6s after PermissionRequest and carries no signal we do \
             not already have"
        );
        // 대조군: 실제로 신호가 있는 이벤트는 등록돼 있다.
        assert!(json["hooks"].get("PermissionRequest").is_some());
        assert!(
            json["hooks"].get("StopFailure").is_some(),
            "StopFailure is what prevents the infinite spinner on an API error"
        );
    }

    #[test]
    fn the_settings_file_lands_inside_the_worktree() {
        let dir = tempfile::tempdir().unwrap();
        write_worktree_settings(dir.path(), Path::new("/tmp/h.sh")).unwrap();
        let written = dir.path().join(".claude").join("settings.local.json");
        assert!(
            written.exists(),
            "settings must be written into the worktree"
        );
        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&written).unwrap()).unwrap();
        assert!(parsed["hooks"]["SessionStart"].is_array());
    }

    // ---- 스크립트 설치 ----

    #[test]
    fn the_installed_script_is_executable_and_is_a_posix_shell_script() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("suaegi-hook.sh");
        install_hook_script(&path).expect("install must create parent dirs");

        assert!(path.exists());
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .starts_with("#!/bin/sh"),
            "the script must carry a shebang — it is invoked as a program by the guard"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(
                mode & 0o111,
                0o111,
                "without the executable bit the `-x` guard is false forever and not one \
                 hook ever fires — silently"
            );
        }
    }

    /// 스크립트가 **항상 exit 0**이고 stdin을 비우는지, 진짜로 돌려 확인한다.
    /// 서버가 없는 상태(포트가 죽어 있음)가 가장 흔한 실패 모드다.
    #[test]
    fn the_script_exits_zero_even_with_no_server_listening() {
        use std::io::Write;
        use std::process::{Command, Stdio};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("suaegi-hook.sh");
        install_hook_script(&path).unwrap();

        // 포트 1은 아무도 듣지 않는다. curl이 즉시 거절당한다.
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(hook_command(&path))
            .env("SUAEGI_HOOK_PORT", "1")
            .env("SUAEGI_HOOK_TOKEN", "t")
            .env("SUAEGI_PANE_KEY", "cGFuZQ")
            .env("SUAEGI_SPAWN_NONCE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("sh must run");
        child
            .stdin
            .as_mut()
            .expect("piped")
            .write_all(br#"{"session_id":"s","hook_event_name":"Stop"}"#)
            .expect("the script must consume stdin");
        assert!(
            child.wait().expect("sh must finish").success(),
            "if suaegi is not listening the user's agent must carry on regardless — a \
             non-zero exit puts an error in their transcript on every single tool call"
        );
    }

    /// env가 하나라도 비면 조용히 빠져나간다 — 그 경우에도 stdin은 비운다.
    #[test]
    fn the_script_is_inert_when_the_environment_is_not_planted() {
        use std::io::Write;
        use std::process::{Command, Stdio};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("suaegi-hook.sh");
        install_hook_script(&path).unwrap();

        let mut child = Command::new("sh")
            .arg("-c")
            .arg(hook_command(&path))
            .env_remove("SUAEGI_HOOK_PORT")
            .env_remove("SUAEGI_HOOK_TOKEN")
            .env_remove("SUAEGI_PANE_KEY")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("sh must run");
        child
            .stdin
            .as_mut()
            .expect("piped")
            .write_all(br#"{"hook_event_name":"Stop"}"#)
            .expect("stdin must be drained even on the inert path");
        assert!(child.wait().expect("sh must finish").success());
    }

    // ---- env 심기 ----

    #[test]
    fn the_spawn_environment_carries_an_encoded_pane_key_and_the_token() {
        // 개행이 든 경로 — 날것으로 헤더에 실으면 헤더 주입이다.
        let key = PaneKey(WorktreeId("/tmp/ws/a\nb c".into()));
        let env = spawn_env(&key, SpawnNonce(7), 51234, "secret-token");
        let get = |k: &str| {
            env.iter()
                .find(|(name, _)| name == k)
                .map(|(_, v)| v.clone())
                .unwrap_or_else(|| panic!("{k} must be planted"))
        };

        let encoded = get("SUAEGI_PANE_KEY");
        assert!(
            !encoded.contains('\n') && !encoded.contains('=') && !encoded.contains(' '),
            "the pane key must be planted ALREADY encoded (base64url, unpadded) — a raw \
             unix path can contain newlines and would be header injection; got {encoded:?}"
        );
        // **진짜 디코더로 왕복시킨다.** base64를 여기서 다시 디코딩하면 서버가
        // 실제로 쓰는 경로와 다른 규칙을 검사하게 된다.
        let body = br#"{"session_id":"s","hook_event_name":"Stop","background_tasks":[]}"#;
        let event = crate::agent_status::parse::parse_hook(&encoded, "7", body)
            .expect("what we plant must be exactly what the server accepts");
        assert_eq!(
            event.pane_key, key,
            "and it must decode back to exactly the original key"
        );
        assert_eq!(event.spawn_nonce, SpawnNonce(7));
        assert_eq!(get("SUAEGI_SPAWN_NONCE"), "7");
        assert_eq!(get("SUAEGI_HOOK_PORT"), "51234");
        assert_eq!(
            get("SUAEGI_HOOK_TOKEN"),
            "secret-token",
            "the token goes in the environment, never in argv where ps would show it"
        );
    }
}
