use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::pty::PtySpawn;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentKind {
    Claude,
    Codex,
    /// 사용자가 직접 지정한 커맨드 (셸 경유 실행)
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptInjection {
    /// 프롬프트를 argv 마지막 인자로
    Argv,
    None,
}

#[derive(Debug, Clone)]
pub struct AgentDef {
    pub kind: AgentKind,
    pub launch_program: &'static str,
    pub launch_args: &'static [&'static str],
    /// 명령줄의 경로 세그먼트와 정확히 일치시킬 이름들
    pub process_names: &'static [&'static str],
    pub prompt_injection: PromptInjection,
}

/// 지원 에이전트 선언 테이블. 새 에이전트 추가는 여기 한 항목이면 된다.
static AGENT_DEFS: &[AgentDef] = &[
    AgentDef {
        kind: AgentKind::Claude,
        launch_program: "claude",
        launch_args: &[],
        // claude-code: node 래퍼로 실행될 때 경로에 나타나는 패키지 디렉토리명
        process_names: &["claude", "claude-code"],
        prompt_injection: PromptInjection::Argv,
    },
    AgentDef {
        kind: AgentKind::Codex,
        launch_program: "codex",
        launch_args: &[],
        process_names: &["codex", "codex.exe"],
        prompt_injection: PromptInjection::Argv,
    },
];

/// 스크립트를 실행하는 런처들 — 이 경우 두 번째 토큰이 실제 프로그램이다
const SCRIPT_LAUNCHERS: &[&str] = &["node", "python", "python3", "bun", "deno"];

pub fn agent_def(kind: AgentKind) -> Option<&'static AgentDef> {
    AGENT_DEFS.iter().find(|d| d.kind == kind)
}

fn login_shell() -> (String, Vec<String>) {
    #[cfg(not(windows))]
    {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        (shell, vec!["-l".to_string()])
    }
    #[cfg(windows)]
    {
        let shell = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string());
        (shell, Vec::new())
    }
}

pub fn build_spawn(
    kind: AgentKind,
    custom_command: Option<&str>,
    prompt: Option<&str>,
    cwd: PathBuf,
    rows: u16,
    cols: u16,
) -> PtySpawn {
    let (program, args) = match agent_def(kind) {
        Some(def) => {
            let mut args: Vec<String> = def.launch_args.iter().map(|s| s.to_string()).collect();
            if let (PromptInjection::Argv, Some(prompt)) = (def.prompt_injection, prompt) {
                // `--`로 이후 인자를 옵션 파싱에서 제외시킨다. 프롬프트가 `-`로
                // 시작하면(예: "-fix this") 이게 없을 때 claude/codex가 이를
                // 알 수 없는 플래그로 파싱해 시작 에러를 내거나 조용히
                // 오동작한다. claude와 codex 둘 다 clap 기반 파서를 쓰며 `--`
                // 뒤는 항상 위치 인자로 취급함을 실측으로 확인했다(둘 다
                // "unknown option"/"unexpected argument" 대신 값으로 받아
                // 프롬프트로 넘어감) — codex는 에러 메시지에서 직접
                // `-- -x` 형태를 제안하기도 한다.
                args.push("--".to_string());
                args.push(prompt.to_string());
            }
            (def.launch_program.to_string(), args)
        }
        // Custom: 사용자 커맨드를 셸에 통째로 넘긴다 (파이프/리다이렉션 허용)
        None => match custom_command {
            Some(command) => {
                let flag = if cfg!(windows) { "/C" } else { "-c" };
                let (shell, _) = login_shell();
                (shell, vec![flag.to_string(), command.to_string()])
            }
            None => login_shell(),
        },
    };

    PtySpawn {
        program,
        args,
        cwd: Some(cwd),
        env: Vec::new(),
        rows,
        cols,
    }
}

/// 토큰을 경로 구분자로 분해한 세그먼트들. 소문자 변환본을 소유하게 해
/// 호출부에서 임시 String의 수명 문제 없이 쓸 수 있게 한다.
fn segments(token: &str) -> Vec<String> {
    token
        .split(['/', '\\'])
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

fn segment_matches(token: &str) -> Option<AgentKind> {
    let lowered = token.to_ascii_lowercase();
    for segment in segments(&lowered) {
        for def in AGENT_DEFS {
            if def.process_names.iter().any(|name| *name == segment) {
                return Some(def.kind);
            }
        }
    }
    None
}

/// 명령줄에서 에이전트를 식별한다. **실행 파일 자리만** 본다 —
/// `grep codex README.md`처럼 인자에 이름이 스쳐 지나가는 경우를 배제하기 위함.
/// 첫 토큰이 스크립트 런처(node 등)면 두 번째 토큰까지 본다.
pub fn match_agent(command_line: &str) -> Option<AgentKind> {
    let mut tokens = command_line.split_whitespace();
    let first = tokens.next()?;
    if let Some(kind) = segment_matches(first) {
        return Some(kind);
    }
    let first_base = segments(&first.to_ascii_lowercase())
        .last()
        .cloned()
        .unwrap_or_default();
    let launcher = SCRIPT_LAUNCHERS
        .iter()
        .any(|l| first_base == *l || first_base == format!("{l}.exe"));
    if launcher {
        if let Some(second) = tokens.next() {
            return segment_matches(second);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn known_agents_have_definitions() {
        assert!(agent_def(AgentKind::Claude).is_some());
        assert!(agent_def(AgentKind::Codex).is_some());
        assert!(agent_def(AgentKind::Custom).is_none());
    }

    #[test]
    fn argv_injection_puts_prompt_in_args() {
        let spawn = build_spawn(
            AgentKind::Claude,
            None,
            Some("fix the bug"),
            PathBuf::from("/tmp"),
            24,
            80,
        );
        assert_eq!(spawn.program, "claude");
        assert!(spawn.args.iter().any(|a| a == "fix the bug"));
        assert_eq!(spawn.cwd, Some(PathBuf::from("/tmp")));
    }

    /// `-`로 시작하는 프롬프트가 claude/codex에 의해 플래그로 오인되면 안
    /// 된다. `--` 분리자가 있으면 위치 인자로 전달된다(실측 확인 — 두 CLI
    /// 모두 clap 기반이며 `--` 뒤는 언제나 값으로 처리한다).
    #[test]
    fn prompt_starting_with_a_dash_is_separated_from_flags() {
        let spawn = build_spawn(
            AgentKind::Claude,
            None,
            Some("-i am not a flag"),
            PathBuf::from("/tmp"),
            24,
            80,
        );
        assert_eq!(
            spawn.args,
            vec!["--".to_string(), "-i am not a flag".to_string()],
            "expected a `--` separator immediately before the prompt"
        );
    }

    #[test]
    fn no_prompt_leaves_args_at_defaults() {
        let spawn = build_spawn(AgentKind::Claude, None, None, PathBuf::from("/tmp"), 24, 80);
        assert!(spawn.args.is_empty());
    }

    #[test]
    fn custom_agent_runs_through_a_shell() {
        let spawn = build_spawn(
            AgentKind::Custom,
            Some("my-agent --flag"),
            None,
            PathBuf::from("/tmp"),
            24,
            80,
        );
        assert!(spawn.args.iter().any(|a| a.contains("my-agent --flag")));
    }

    #[test]
    fn custom_without_command_launches_a_login_shell() {
        let spawn = build_spawn(AgentKind::Custom, None, None, PathBuf::from("/tmp"), 24, 80);
        assert!(!spawn.program.is_empty());
        #[cfg(unix)]
        assert!(
            spawn.args.iter().any(|a| a == "-l"),
            "expected a login shell"
        );
    }

    #[test]
    fn matches_agent_from_a_command_line() {
        assert_eq!(match_agent("claude --resume"), Some(AgentKind::Claude));
        assert_eq!(match_agent("/usr/local/bin/codex"), Some(AgentKind::Codex));
        // claude는 node 스크립트로 실행될 수 있다 — 경로 세그먼트로 잡는다
        assert_eq!(
            match_agent("node /opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/cli.js"),
            Some(AgentKind::Claude)
        );
        // Windows 경로 구분자 (명령줄은 공백 기준으로 토큰화되므로 공백 없는 경로로 검증)
        assert_eq!(
            match_agent("C:\\tools\\codex\\codex.exe --help"),
            Some(AgentKind::Codex)
        );
    }

    #[test]
    fn does_not_match_incidental_mentions() {
        assert_eq!(match_agent("/bin/zsh"), None);
        assert_eq!(match_agent("vim claude-notes.md"), None);
        assert_eq!(match_agent("grep codex README.md"), None);
        assert_eq!(match_agent(""), None);
    }

    #[test]
    fn dimensions_are_carried_into_the_spawn() {
        let spawn = build_spawn(AgentKind::Codex, None, None, PathBuf::from("/tmp"), 40, 120);
        assert_eq!((spawn.rows, spawn.cols), (40, 120));
    }
}
