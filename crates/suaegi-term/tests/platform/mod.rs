//! 3 OS에서 동일하게 동작하는 테스트용 커맨드. Windows에서도 테스트 스위트가
//! 컴파일·통과해야 하므로 셸 문법을 직접 쓰지 않고 이 헬퍼를 경유한다.

// 이 헬퍼들은 여러 테스트 바이너리(각각 별도로 컴파일됨)에서 공유된다 — 바이너리마다
// 쓰는 부분집합이 달라서, --all-targets로 보면 항상 어느 한 바이너리에서는 일부가
// 미사용으로 보인다.
#![allow(dead_code)]

pub fn shell_command(script: &str) -> (String, Vec<String>) {
    #[cfg(unix)]
    {
        ("sh".to_string(), vec!["-c".to_string(), script.to_string()])
    }
    #[cfg(windows)]
    {
        (
            "cmd".to_string(),
            vec!["/C".to_string(), script.to_string()],
        )
    }
}

pub fn echo(text: &str) -> (String, Vec<String>) {
    #[cfg(unix)]
    {
        shell_command(&format!("printf '%s\\n' '{text}'"))
    }
    #[cfg(windows)]
    {
        shell_command(&format!("echo {text}"))
    }
}

pub fn sleep_seconds(secs: u32) -> (String, Vec<String>) {
    #[cfg(unix)]
    {
        ("sleep".to_string(), vec![secs.to_string()])
    }
    #[cfg(windows)]
    {
        // timeout은 리다이렉트된 stdin에서 실패하므로 ping 루프를 쓴다
        shell_command(&format!("ping -n {} 127.0.0.1 > nul", secs + 1))
    }
}

pub fn print_env(var: &str) -> (String, Vec<String>) {
    #[cfg(unix)]
    {
        shell_command(&format!("printf '%s\\n' \"${var}\""))
    }
    #[cfg(windows)]
    {
        shell_command(&format!("echo %{var}%"))
    }
}

pub fn print_cwd() -> (String, Vec<String>) {
    #[cfg(unix)]
    {
        ("pwd".to_string(), Vec::new())
    }
    #[cfg(windows)]
    {
        shell_command("cd")
    }
}

pub fn exit_with(code: i32) -> (String, Vec<String>) {
    shell_command(&format!("exit {code}"))
}

/// stdin을 그대로 되돌려주는 프로세스 (에코 검증용)
pub fn echo_stdin() -> (String, Vec<String>) {
    #[cfg(unix)]
    {
        ("cat".to_string(), Vec::new())
    }
    #[cfg(windows)]
    {
        shell_command("more")
    }
}
