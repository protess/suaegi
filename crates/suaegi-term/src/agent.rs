use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::pty::PtySpawn;

/// 훅 주입처럼 **Claude/Codex를 특수 취급**하는 경로에서만 쓰는 좁은 enum.
/// 나머지 31종은 [`AgentDef::id`] 문자열로 다룬다(Orca `TUI_AGENT_CONFIG` 미러).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentKind {
    Claude,
    Codex,
    /// 사용자가 직접 지정한 커맨드 (셸 경유 실행)
    Custom,
}

/// 런치 커맨드가 플랫폼별로 갈리는 에이전트용(Orca `launchCmdByPlatform`).
/// 33종 중 실제로 쓰는 행은 없지만(claude-agent-teams만 썼고 그건 제외됨) 모델을
/// 온전히 옮겨 두어 새 에이전트가 "테이블 한 줄"로 추가되게 한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Linux,
    MacOS,
    Windows,
}

/// 설치 감지를 게이팅하는 런타임(Orca `detectUnsupportedRuntimes`,
/// `NodeJS.Platform | 'wsl'`). 33종 모두 빈 목록이지만 게이트 자체는 살아 있다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runtime {
    Linux,
    MacOS,
    Windows,
    Wsl,
}

/// 상태 신호의 출처. v1에서 Claude만 훅 기반 정밀 신호를 쓰고, 나머지는 전부
/// OSC-title 백스톱을 쓴다(플랜 §0/§2.1 — 비-Claude 훅은 전역 홈을 건드려야 해서
/// 미룬다). 6a에서는 데이터일 뿐이고 소비는 6b가 한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusSource {
    Hooks,
    OscTitle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptInjection {
    /// 프롬프트를 argv 마지막 인자로. `separator`는 에이전트별 opt-in —
    /// claude/codex는 suaegi 실측, grok은 Orca 문서화로 `Some("--")`.
    Argv {
        separator: Option<&'static str>,
    },
    /// 프롬프트를 플래그 값으로 넘긴 뒤 제출/종료한다: `<flag> <prompt>`.
    /// flag-prompt → `--prompt <v>` (opencode, mimo-code; Orca `tui-agent-startup.ts:117`).
    Flag(&'static str),
    /// 프롬프트를 seed하되 제출/종료하지 않는 인터랙티브 플래그: `<flag> <prompt>`.
    /// flag-prompt-interactive → `--prompt-interactive <v>` (gemini, antigravity;
    /// `:155`); flag-interactive → `-i <v>` (copilot; `:167`).
    FlagInteractive(&'static str),
    /// 빈 TUI로 띄운 뒤 composer 준비 후 PTY에 써넣는다(18종). 6a는 프롬프트 없이
    /// 띄우기만 하고, 실제 주입은 6b.
    StdinAfterStart,
    /// hermes 고유 startup-query 계약(Orca `:263`). 6a는 빈 TUI, 주입은 6b.
    HermesQuery,
    None,
}

#[derive(Debug, Clone)]
pub struct AgentDef {
    /// Orca `TuiAgent` id — 저장된 사용자 선호/텔레메트리 키. (예: "claude", "kiro")
    pub id: &'static str,
    /// 사람이 읽는 이름(Orca `tui-agent-display-names.ts`).
    pub display_name: &'static str,
    /// 토큰화된 launchCmd의 첫 토큰.
    pub launch_program: &'static str,
    /// 나머지 고정 토큰(`kiro-cli chat --tui`의 `chat --tui` 등).
    pub launch_args: &'static [&'static str],
    /// 플랫폼별 런치 오버라이드(program, args). 비면 `launch_program`/`launch_args`.
    pub launch_by_platform: &'static [(Platform, &'static str, &'static [&'static str])],
    /// PATH에서 설치를 증명하는 바이너리(launch와 다를 수 있다: `kiro`→`kiro-cli`).
    pub detect_cmd: &'static str,
    /// 같은 에이전트를 가리키는 추가 PATH 이름(Orca `detectCmdAliases`).
    pub detect_aliases: &'static [&'static str],
    /// 설치로 치기 전에 함께 있어야 하는 커맨드(AND 게이트, Orca `detectRequiredCommands`).
    pub required_commands: &'static [&'static str],
    /// 이 런타임에서는 감지에서 제외(Orca `detectUnsupportedRuntimes`).
    pub unsupported_runtimes: &'static [Runtime],
    /// 프로세스 테이블 감지명(Orca `expectedProcess`).
    pub expected_process: &'static str,
    /// node 패키지 경로 마커 — 인터프리터 래핑 감지용(Orca
    /// `NODE_PACKAGE_SCRIPT_ENTRYPOINTS`). 예: codex→`@openai/codex`.
    pub package_marker: Option<&'static str>,
    pub prompt_injection: PromptInjection,
    pub status: StatusSource,
}

/// 지원 에이전트 선언 테이블. 새 에이전트 추가는 여기 한 항목이면 된다.
///
/// Orca `TUI_AGENT_CONFIG`(`src/shared/tui-agent-config.ts:46-296`)의 미러다.
/// **33행 = 34종 − `claude-agent-teams`**. 그 행은 Orca 전용 CLI shim(`orca
/// claude-teams`)에 shell-out하는데 suaegi 세계엔 `orca` 바이너리가 없어
/// 무의미하다(플랜 §2.1 / Codex B2). 의도적으로 제외한다.
static AGENT_DEFS: &[AgentDef] = &[
    AgentDef {
        id: "claude",
        display_name: "Claude",
        launch_program: "claude",
        launch_args: &[],
        launch_by_platform: &[],
        detect_cmd: "claude",
        detect_aliases: &[],
        required_commands: &[],
        unsupported_runtimes: &[],
        expected_process: "claude",
        // node 래퍼(`node .../@anthropic-ai/claude-code/cli.js`)로 실행될 때
        // 잡는다. 실측 확인된 suaegi 회귀 케이스라 마커로 정밀화해 보존한다.
        package_marker: Some("@anthropic-ai/claude-code"),
        // suaegi 실측: `-`로 시작하는 프롬프트가 플래그로 오인되지 않게 `--` 필요.
        prompt_injection: PromptInjection::Argv {
            separator: Some("--"),
        },
        status: StatusSource::Hooks,
    },
    AgentDef {
        id: "openclaude",
        display_name: "OpenClaude",
        launch_program: "openclaude",
        launch_args: &[],
        launch_by_platform: &[],
        detect_cmd: "openclaude",
        detect_aliases: &[],
        required_commands: &[],
        unsupported_runtimes: &[],
        expected_process: "openclaude",
        package_marker: None,
        // #F7: `--` 필요 여부 미검증 → 안전한 기본값 Some("--")(플랜 §2.2 Codex B1).
        prompt_injection: PromptInjection::Argv {
            separator: Some("--"),
        },
        status: StatusSource::OscTitle,
    },
    AgentDef {
        id: "codex",
        display_name: "Codex",
        launch_program: "codex",
        launch_args: &[],
        launch_by_platform: &[],
        detect_cmd: "codex",
        detect_aliases: &[],
        required_commands: &[],
        unsupported_runtimes: &[],
        expected_process: "codex",
        package_marker: Some("@openai/codex"),
        // suaegi 실측: codex(clap)도 `--` 뒤를 항상 위치 인자로 받는다.
        prompt_injection: PromptInjection::Argv {
            separator: Some("--"),
        },
        status: StatusSource::OscTitle,
    },
    AgentDef {
        id: "autohand",
        display_name: "Autohand Code",
        launch_program: "autohand",
        launch_args: &[],
        launch_by_platform: &[],
        detect_cmd: "autohand",
        detect_aliases: &[],
        required_commands: &[],
        unsupported_runtimes: &[],
        expected_process: "autohand",
        package_marker: None,
        prompt_injection: PromptInjection::StdinAfterStart,
        status: StatusSource::OscTitle,
    },
    AgentDef {
        id: "ante",
        display_name: "Ante",
        launch_program: "ante",
        launch_args: &[],
        launch_by_platform: &[],
        detect_cmd: "ante",
        detect_aliases: &[],
        required_commands: &[],
        unsupported_runtimes: &[],
        expected_process: "ante",
        package_marker: None,
        prompt_injection: PromptInjection::StdinAfterStart,
        status: StatusSource::OscTitle,
    },
    AgentDef {
        id: "opencode",
        display_name: "OpenCode",
        launch_program: "opencode",
        launch_args: &[],
        launch_by_platform: &[],
        detect_cmd: "opencode",
        detect_aliases: &[],
        required_commands: &[],
        unsupported_runtimes: &[],
        expected_process: "opencode",
        package_marker: None,
        prompt_injection: PromptInjection::Flag("--prompt"),
        status: StatusSource::OscTitle,
    },
    AgentDef {
        id: "mimo-code",
        display_name: "MiMo Code",
        launch_program: "mimo",
        launch_args: &[],
        launch_by_platform: &[],
        detect_cmd: "mimo",
        detect_aliases: &[],
        required_commands: &[],
        unsupported_runtimes: &[],
        expected_process: "mimo",
        package_marker: None,
        prompt_injection: PromptInjection::Flag("--prompt"),
        status: StatusSource::OscTitle,
    },
    AgentDef {
        id: "pi",
        display_name: "Pi",
        launch_program: "pi",
        launch_args: &[],
        launch_by_platform: &[],
        detect_cmd: "pi",
        detect_aliases: &[],
        required_commands: &[],
        unsupported_runtimes: &[],
        expected_process: "pi",
        package_marker: None,
        // #F7: 미검증 → 안전한 기본값 Some("--").
        prompt_injection: PromptInjection::Argv {
            separator: Some("--"),
        },
        status: StatusSource::OscTitle,
    },
    AgentDef {
        id: "omp",
        display_name: "OMP",
        launch_program: "omp",
        launch_args: &[],
        launch_by_platform: &[],
        detect_cmd: "omp",
        detect_aliases: &[],
        required_commands: &[],
        unsupported_runtimes: &[],
        expected_process: "omp",
        package_marker: None,
        // #F7: 미검증 → 안전한 기본값 Some("--").
        prompt_injection: PromptInjection::Argv {
            separator: Some("--"),
        },
        status: StatusSource::OscTitle,
    },
    AgentDef {
        id: "gemini",
        display_name: "Gemini",
        launch_program: "gemini",
        launch_args: &[],
        launch_by_platform: &[],
        detect_cmd: "gemini",
        detect_aliases: &[],
        required_commands: &[],
        unsupported_runtimes: &[],
        expected_process: "gemini",
        package_marker: Some("@google/gemini-cli"),
        prompt_injection: PromptInjection::FlagInteractive("--prompt-interactive"),
        status: StatusSource::OscTitle,
    },
    AgentDef {
        id: "antigravity",
        display_name: "Antigravity",
        launch_program: "agy",
        launch_args: &[],
        launch_by_platform: &[],
        detect_cmd: "agy",
        detect_aliases: &[],
        required_commands: &[],
        unsupported_runtimes: &[],
        expected_process: "agy",
        package_marker: None,
        prompt_injection: PromptInjection::FlagInteractive("--prompt-interactive"),
        status: StatusSource::OscTitle,
    },
    AgentDef {
        id: "aider",
        display_name: "Aider",
        launch_program: "aider",
        launch_args: &[],
        launch_by_platform: &[],
        detect_cmd: "aider",
        detect_aliases: &[],
        required_commands: &[],
        unsupported_runtimes: &[],
        expected_process: "aider",
        package_marker: None,
        prompt_injection: PromptInjection::StdinAfterStart,
        status: StatusSource::OscTitle,
    },
    AgentDef {
        id: "goose",
        display_name: "Goose",
        launch_program: "goose",
        launch_args: &[],
        launch_by_platform: &[],
        detect_cmd: "goose",
        detect_aliases: &[],
        required_commands: &[],
        unsupported_runtimes: &[],
        expected_process: "goose",
        package_marker: None,
        prompt_injection: PromptInjection::StdinAfterStart,
        status: StatusSource::OscTitle,
    },
    AgentDef {
        id: "amp",
        display_name: "Amp",
        launch_program: "amp",
        launch_args: &[],
        launch_by_platform: &[],
        detect_cmd: "amp",
        detect_aliases: &[],
        required_commands: &[],
        unsupported_runtimes: &[],
        expected_process: "amp",
        package_marker: None,
        prompt_injection: PromptInjection::StdinAfterStart,
        status: StatusSource::OscTitle,
    },
    AgentDef {
        id: "kilo",
        display_name: "Kilocode",
        launch_program: "kilo",
        launch_args: &[],
        launch_by_platform: &[],
        detect_cmd: "kilo",
        detect_aliases: &[],
        required_commands: &[],
        unsupported_runtimes: &[],
        expected_process: "kilo",
        package_marker: None,
        prompt_injection: PromptInjection::StdinAfterStart,
        status: StatusSource::OscTitle,
    },
    AgentDef {
        id: "kiro",
        display_name: "Kiro",
        // Kiro 설치기는 `kiro-cli`를 깐다(id는 저장 선호를 위해 "kiro" 유지).
        launch_program: "kiro-cli",
        // trust 플래그는 `chat` 서브커맨드에 붙는다.
        launch_args: &["chat", "--tui"],
        launch_by_platform: &[],
        detect_cmd: "kiro-cli",
        detect_aliases: &[],
        required_commands: &[],
        unsupported_runtimes: &[],
        expected_process: "kiro-cli",
        package_marker: None,
        prompt_injection: PromptInjection::StdinAfterStart,
        status: StatusSource::OscTitle,
    },
    AgentDef {
        id: "crush",
        display_name: "Charm",
        launch_program: "crush",
        launch_args: &[],
        launch_by_platform: &[],
        detect_cmd: "crush",
        detect_aliases: &[],
        required_commands: &[],
        unsupported_runtimes: &[],
        expected_process: "crush",
        package_marker: None,
        prompt_injection: PromptInjection::StdinAfterStart,
        status: StatusSource::OscTitle,
    },
    AgentDef {
        id: "aug",
        display_name: "Auggie",
        // @augmentcode/auggie는 `auggie` 바이너리를 깐다(id는 "aug" 유지).
        launch_program: "auggie",
        launch_args: &[],
        launch_by_platform: &[],
        detect_cmd: "auggie",
        detect_aliases: &[],
        required_commands: &[],
        unsupported_runtimes: &[],
        expected_process: "auggie",
        package_marker: None,
        prompt_injection: PromptInjection::StdinAfterStart,
        status: StatusSource::OscTitle,
    },
    AgentDef {
        id: "cline",
        display_name: "Cline",
        launch_program: "cline",
        launch_args: &[],
        launch_by_platform: &[],
        detect_cmd: "cline",
        detect_aliases: &[],
        required_commands: &[],
        unsupported_runtimes: &[],
        expected_process: "cline",
        package_marker: None,
        prompt_injection: PromptInjection::StdinAfterStart,
        status: StatusSource::OscTitle,
    },
    AgentDef {
        id: "codebuff",
        display_name: "Codebuff",
        launch_program: "codebuff",
        launch_args: &[],
        launch_by_platform: &[],
        detect_cmd: "codebuff",
        detect_aliases: &[],
        required_commands: &[],
        unsupported_runtimes: &[],
        expected_process: "codebuff",
        package_marker: None,
        prompt_injection: PromptInjection::StdinAfterStart,
        status: StatusSource::OscTitle,
    },
    AgentDef {
        id: "command-code",
        display_name: "Command Code",
        // 풀네임 — Windows 빌트인 `cmd.exe`와의 감지 충돌을 피한다(`cmd` 별칭 안 씀).
        launch_program: "command-code",
        // `--trust`로 첫 실행 trust 프롬프트를 건너뛴다(플래그, preflight 파일 아님).
        launch_args: &["--trust"],
        launch_by_platform: &[],
        detect_cmd: "command-code",
        detect_aliases: &[],
        required_commands: &[],
        unsupported_runtimes: &[],
        expected_process: "command-code",
        package_marker: None,
        // #F7: 미검증 → 안전한 기본값 Some("--").
        prompt_injection: PromptInjection::Argv {
            separator: Some("--"),
        },
        status: StatusSource::OscTitle,
    },
    AgentDef {
        id: "continue",
        display_name: "Continue",
        // Continue의 CLI 바이너리는 `cn`. `continue`는 셸 빌트인이라 감지 못한다.
        launch_program: "cn",
        launch_args: &[],
        launch_by_platform: &[],
        detect_cmd: "cn",
        detect_aliases: &[],
        required_commands: &[],
        unsupported_runtimes: &[],
        expected_process: "cn",
        package_marker: None,
        prompt_injection: PromptInjection::StdinAfterStart,
        status: StatusSource::OscTitle,
    },
    AgentDef {
        id: "cursor",
        display_name: "Cursor",
        launch_program: "cursor-agent",
        launch_args: &[],
        launch_by_platform: &[],
        detect_cmd: "cursor-agent",
        detect_aliases: &[],
        required_commands: &[],
        unsupported_runtimes: &[],
        expected_process: "cursor-agent",
        package_marker: None,
        // #F7: 미검증 → 안전한 기본값 Some("--").
        prompt_injection: PromptInjection::Argv {
            separator: Some("--"),
        },
        status: StatusSource::OscTitle,
    },
    AgentDef {
        id: "droid",
        display_name: "Droid",
        launch_program: "droid",
        launch_args: &[],
        launch_by_platform: &[],
        detect_cmd: "droid",
        detect_aliases: &[],
        required_commands: &[],
        unsupported_runtimes: &[],
        expected_process: "droid",
        package_marker: None,
        // #F7: 미검증 → 안전한 기본값 Some("--").
        prompt_injection: PromptInjection::Argv {
            separator: Some("--"),
        },
        status: StatusSource::OscTitle,
    },
    AgentDef {
        id: "kimi",
        display_name: "Kimi",
        launch_program: "kimi",
        launch_args: &[],
        launch_by_platform: &[],
        detect_cmd: "kimi",
        detect_aliases: &[],
        required_commands: &[],
        unsupported_runtimes: &[],
        expected_process: "kimi",
        package_marker: None,
        prompt_injection: PromptInjection::StdinAfterStart,
        status: StatusSource::OscTitle,
    },
    AgentDef {
        id: "mistral-vibe",
        display_name: "Mistral Vibe",
        // 설치기는 `vibe` 바이너리를 노출한다(패키지명은 mistral-vibe).
        launch_program: "vibe",
        launch_args: &[],
        launch_by_platform: &[],
        detect_cmd: "vibe",
        detect_aliases: &["mistral-vibe"],
        required_commands: &[],
        unsupported_runtimes: &[],
        expected_process: "vibe",
        package_marker: None,
        prompt_injection: PromptInjection::StdinAfterStart,
        status: StatusSource::OscTitle,
    },
    AgentDef {
        id: "qwen-code",
        display_name: "Qwen Code",
        // 패키지는 qwen-code지만 설치된 CLI 바이너리는 `qwen`.
        launch_program: "qwen",
        launch_args: &[],
        launch_by_platform: &[],
        detect_cmd: "qwen",
        detect_aliases: &[],
        required_commands: &[],
        unsupported_runtimes: &[],
        expected_process: "qwen",
        package_marker: None,
        prompt_injection: PromptInjection::StdinAfterStart,
        status: StatusSource::OscTitle,
    },
    AgentDef {
        id: "rovo",
        display_name: "Rovo Dev",
        launch_program: "rovo",
        launch_args: &[],
        launch_by_platform: &[],
        detect_cmd: "rovo",
        detect_aliases: &[],
        required_commands: &[],
        unsupported_runtimes: &[],
        expected_process: "rovo",
        package_marker: None,
        prompt_injection: PromptInjection::StdinAfterStart,
        status: StatusSource::OscTitle,
    },
    AgentDef {
        id: "hermes",
        display_name: "Hermes",
        launch_program: "hermes",
        // bare `hermes`는 REPL, `--tui`가 전체화면 에이전트 UI.
        launch_args: &["--tui"],
        launch_by_platform: &[],
        detect_cmd: "hermes",
        detect_aliases: &[],
        required_commands: &[],
        unsupported_runtimes: &[],
        expected_process: "hermes",
        package_marker: None,
        prompt_injection: PromptInjection::HermesQuery,
        status: StatusSource::OscTitle,
    },
    AgentDef {
        id: "openclaw",
        display_name: "OpenClaw",
        launch_program: "openclaw",
        launch_args: &[],
        launch_by_platform: &[],
        detect_cmd: "openclaw",
        detect_aliases: &[],
        required_commands: &[],
        unsupported_runtimes: &[],
        expected_process: "openclaw",
        package_marker: None,
        prompt_injection: PromptInjection::StdinAfterStart,
        status: StatusSource::OscTitle,
    },
    AgentDef {
        id: "copilot",
        display_name: "GitHub Copilot",
        launch_program: "copilot",
        launch_args: &[],
        launch_by_platform: &[],
        detect_cmd: "copilot",
        detect_aliases: &[],
        required_commands: &[],
        unsupported_runtimes: &[],
        expected_process: "copilot",
        package_marker: None,
        // bare `--prompt`는 완료 시 종료하므로 `-i`(interactive)로 seed한다.
        prompt_injection: PromptInjection::FlagInteractive("-i"),
        status: StatusSource::OscTitle,
    },
    AgentDef {
        id: "grok",
        display_name: "Grok",
        launch_program: "grok",
        launch_args: &[],
        launch_by_platform: &[],
        detect_cmd: "grok",
        detect_aliases: &[],
        required_commands: &[],
        unsupported_runtimes: &[],
        expected_process: "grok",
        package_marker: None,
        // Orca 문서화: grok은 `--` separator를 켠 유일한 에이전트(`:287`).
        prompt_injection: PromptInjection::Argv {
            separator: Some("--"),
        },
        status: StatusSource::OscTitle,
    },
    AgentDef {
        id: "devin",
        display_name: "Devin",
        launch_program: "devin",
        launch_args: &[],
        launch_by_platform: &[],
        detect_cmd: "devin",
        detect_aliases: &[],
        required_commands: &[],
        unsupported_runtimes: &[],
        expected_process: "devin",
        package_marker: None,
        // `devin -- <prompt>`는 즉시 자동 제출되므로 argv 프롬프트 없이 REPL을 연다.
        prompt_injection: PromptInjection::StdinAfterStart,
        status: StatusSource::OscTitle,
    },
];

/// 스크립트를 실행하는 node 계열 런처들 — 두 번째 토큰이 실제 진입점이다.
const NODE_LAUNCHERS: &[&str] = &["node", "bun", "deno"];
/// python 계열 런처들 — `-m <module>` 또는 스크립트 경로가 진입점이다.
const PYTHON_LAUNCHERS: &[&str] = &["python", "python3"];

/// node-pty가 런치 커맨드 대신 패키지 플랫폼 바이너리를 보고할 때의 접두
/// (Orca `agent-process-recognition.ts:88-95`). 예: `codex-aarch64-apple-darwin`.
const PACKAGED_BINARY_PREFIXES: &[(&str, &str)] = &[("codex-", "codex"), ("grok-", "grok")];

/// 세션이 상태 신호를 어디서 얻는가. `AgentKind`가 좁아(Claude/Codex/Custom)
/// `AgentDef` 조회 없이 결정한다. **Claude만 훅**이고, Codex와 Custom(사용자가
/// 직접 띄운 임의 CLI)은 OSC 타이틀이 유일한 신호다 — 우리가 무엇을 띄웠는지와
/// 무관하게 pane이 내보내는 타이틀에서 상태를 추론한다.
pub fn status_source_for(kind: AgentKind) -> StatusSource {
    match kind {
        AgentKind::Claude => StatusSource::Hooks,
        AgentKind::Codex | AgentKind::Custom => StatusSource::OscTitle,
    }
}

pub fn agent_def(kind: AgentKind) -> Option<&'static AgentDef> {
    let id = match kind {
        AgentKind::Claude => "claude",
        AgentKind::Codex => "codex",
        AgentKind::Custom => return None,
    };
    agent_def_by_id(id)
}

/// id로 테이블 행을 찾는다(비-Claude/Codex 에이전트의 정규 조회 경로).
pub fn agent_def_by_id(id: &str) -> Option<&'static AgentDef> {
    AGENT_DEFS.iter().find(|d| d.id == id)
}

/// 테이블 전체를 순회하려는 소비자(설치 감지, UI 목록)용.
pub fn agent_defs() -> &'static [AgentDef] {
    AGENT_DEFS
}

pub fn current_platform() -> Platform {
    if cfg!(windows) {
        Platform::Windows
    } else if cfg!(target_os = "macos") {
        Platform::MacOS
    } else {
        Platform::Linux
    }
}

pub fn current_runtime() -> Runtime {
    if cfg!(windows) {
        Runtime::Windows
    } else if cfg!(target_os = "macos") {
        Runtime::MacOS
    } else {
        Runtime::Linux
    }
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

/// 프롬프트를 에이전트별 주입 모드에 따라 argv에 반영한다. 6a에서
/// `StdinAfterStart`/`HermesQuery`/`None`은 아무것도 하지 않는다(빈 TUI로 뜬다).
fn apply_prompt_injection(def: &AgentDef, prompt: Option<&str>, args: &mut Vec<String>) {
    let Some(prompt) = prompt else {
        return;
    };
    match def.prompt_injection {
        PromptInjection::Argv { separator } => {
            // `--`로 이후 인자를 옵션 파싱에서 제외시킨다. 프롬프트가 `-`로
            // 시작하면(예: "-fix this") 이게 없을 때 CLI가 이를 알 수 없는
            // 플래그로 파싱해 시작 에러를 내거나 조용히 오동작한다. claude/codex는
            // suaegi 실측으로, grok은 Orca 문서화로 `--`가 필요함을 확인했다.
            if let Some(sep) = separator {
                args.push(sep.to_string());
            }
            args.push(prompt.to_string());
        }
        PromptInjection::Flag(flag) | PromptInjection::FlagInteractive(flag) => {
            args.push(flag.to_string());
            args.push(prompt.to_string());
        }
        PromptInjection::StdinAfterStart | PromptInjection::HermesQuery | PromptInjection::None => {
        }
    }
}

/// 테이블 행(에이전트)을 PTY 스폰 스펙으로. 플랫폼 오버라이드를 존중한다.
pub fn spawn_for_def(
    def: &AgentDef,
    prompt: Option<&str>,
    cwd: PathBuf,
    rows: u16,
    cols: u16,
) -> PtySpawn {
    let platform = current_platform();
    let (program, base_args) = def
        .launch_by_platform
        .iter()
        .find(|(p, _, _)| *p == platform)
        .map(|(_, prog, args)| (*prog, *args))
        .unwrap_or((def.launch_program, def.launch_args));
    let mut args: Vec<String> = base_args.iter().map(|s| s.to_string()).collect();
    apply_prompt_injection(def, prompt, &mut args);
    PtySpawn {
        program: program.to_string(),
        args,
        cwd: Some(cwd),
        env: Vec::new(),
        rows,
        cols,
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
    match agent_def(kind) {
        Some(def) => spawn_for_def(def, prompt, cwd, rows, cols),
        // Custom: 사용자 커맨드를 셸에 통째로 넘긴다 (파이프/리다이렉션 허용)
        None => {
            let (program, args) = match custom_command {
                Some(command) => {
                    let flag = if cfg!(windows) { "/C" } else { "-c" };
                    let (shell, _) = login_shell();
                    (shell, vec![flag.to_string(), command.to_string()])
                }
                None => login_shell(),
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
    }
}

// ── 설치 감지 (Orca `preflight.ts:104` `isCommandOnPath` 미러) ──────────────

/// PATH 조회를 추상화한다(테스트에서 합성 PATH를 주입할 수 있게).
pub trait CommandProbe {
    fn on_path(&self, cmd: &str) -> bool;
}

/// 실제 PATH를 스캔하는 프로브.
#[derive(Debug, Default, Clone, Copy)]
pub struct PathProbe;

impl CommandProbe for PathProbe {
    fn on_path(&self, cmd: &str) -> bool {
        let Some(path) = std::env::var_os("PATH") else {
            return false;
        };
        for dir in std::env::split_paths(&path) {
            if is_executable_file(&dir.join(cmd)) {
                return true;
            }
            #[cfg(windows)]
            {
                for ext in ["exe", "cmd", "bat"] {
                    if is_executable_file(&dir.join(format!("{cmd}.{ext}"))) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

#[cfg(unix)]
fn is_executable_file(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(meta) => meta.is_file() && meta.permissions().mode() & 0o111 != 0,
        Err(_) => false,
    }
}

#[cfg(not(unix))]
fn is_executable_file(path: &std::path::Path) -> bool {
    path.is_file()
}

/// 에이전트가 설치돼 있는지: `detect_cmd`(또는 별칭 중 하나)가 PATH에 있고
/// `required_commands`가 모두 있으며, 현재 런타임이 `unsupported_runtimes`에
/// 없어야 한다. 버전 고정/설치 디렉토리 폴백은 v1에서 뺀다(Orca도 버전 고정 없음).
pub fn detect_installed(def: &AgentDef, probe: &dyn CommandProbe, runtime: Runtime) -> bool {
    if def.unsupported_runtimes.contains(&runtime) {
        return false;
    }
    let primary_present = std::iter::once(def.detect_cmd)
        .chain(def.detect_aliases.iter().copied())
        .any(|cmd| probe.on_path(cmd));
    if !primary_present {
        return false;
    }
    def.required_commands.iter().all(|cmd| probe.on_path(cmd))
}

// ── 감지 (`match_agent`) ────────────────────────────────────────────────────

/// 토큰의 basename을 소문자로 정규화하고 Windows 실행 확장자를 벗긴다.
/// `C:\tools\codex\codex.exe` → `codex`.
fn normalized_basename(token: &str) -> String {
    let lowered = token.to_ascii_lowercase().replace('\\', "/");
    let base = lowered.rsplit('/').next().unwrap_or(&lowered);
    for ext in [".exe", ".cmd", ".bat", ".ps1"] {
        if let Some(stripped) = base.strip_suffix(ext) {
            return stripped.to_string();
        }
    }
    base.to_string()
}

/// 한 행이 프로세스/PATH 이름으로 나타날 수 있는 후보들(Orca
/// `agent-process-recognition.ts:65-69`: expectedProcess + detect(+aliases) +
/// launch 첫 토큰). 테이블 값은 모두 소문자다.
fn def_detect_names(def: &AgentDef) -> impl Iterator<Item = &'static str> {
    std::iter::once(def.expected_process)
        .chain(std::iter::once(def.detect_cmd))
        .chain(def.detect_aliases.iter().copied())
        .chain(std::iter::once(def.launch_program))
}

/// basename이 어떤 행의 감지 이름과 정확히 일치하는지.
fn basename_matches(token: &str) -> Option<&'static AgentDef> {
    let base = normalized_basename(token);
    AGENT_DEFS
        .iter()
        .find(|def| def_detect_names(def).any(|name| name == base))
}

/// 패키지 플랫폼 바이너리 접두(`codex-<arch>`, `grok-*`).
fn packaged_binary_prefix(token: &str) -> Option<&'static AgentDef> {
    let base = normalized_basename(token);
    PACKAGED_BINARY_PREFIXES
        .iter()
        .find(|(prefix, _)| base.starts_with(prefix))
        .and_then(|(_, id)| agent_def_by_id(id))
}

/// node 스크립트 진입점 경로가 어떤 행의 `package_marker`를 포함하는지.
/// 예: `.../@openai/codex/bin/codex.js` → codex.
fn package_marker_matches(token: &str) -> Option<&'static AgentDef> {
    let path = token.to_ascii_lowercase().replace('\\', "/");
    AGENT_DEFS.iter().find(|def| {
        def.package_marker
            .is_some_and(|marker| path.contains(&marker.to_ascii_lowercase()))
    })
}

/// python `-m <module>`의 모듈명 첫 세그먼트가 어떤 행의 감지 이름과 맞는지.
/// 예: `python -m aider` → aider.
fn python_module_matches(module: &str) -> Option<&'static AgentDef> {
    if module.starts_with('-') {
        return None;
    }
    let first_seg = module
        .split('.')
        .next()
        .unwrap_or(module)
        .to_ascii_lowercase();
    AGENT_DEFS
        .iter()
        .find(|def| def_detect_names(def).any(|name| name == first_seg))
}

fn is_launcher(basename: &str, launchers: &[&str]) -> bool {
    launchers.iter().any(|l| basename == *l)
}

/// 명령줄에서 에이전트를 식별한다. **실행 파일 자리만** 본다 —
/// `grep codex README.md`처럼 인자에 이름이 스쳐 지나가는 경우를 배제한다.
/// 첫 토큰이 스크립트 런처(node/python 등)면 진입점 토큰까지 본다.
pub fn match_agent(command_line: &str) -> Option<&'static AgentDef> {
    let mut tokens = command_line.split_whitespace();
    let first = tokens.next()?;

    // 1) 직접 실행: basename 정확 일치.
    if let Some(def) = basename_matches(first) {
        return Some(def);
    }
    // 2) 패키지 플랫폼 바이너리 접두(codex-<arch>, grok-*).
    if let Some(def) = packaged_binary_prefix(first) {
        return Some(def);
    }

    // 3) 인터프리터 래핑: 런처면 진입점 토큰을 검사한다.
    let first_base = normalized_basename(first);
    if is_launcher(&first_base, NODE_LAUNCHERS) {
        // 런처 플래그를 건너뛰고 첫 진입점(플래그 아님) 토큰만 본다 —
        // 프롬프트 텍스트("compare opencode vs orca")의 오탐을 막는다.
        if let Some(entry) = tokens.find(|t| !t.starts_with('-')) {
            if let Some(def) = basename_matches(entry) {
                return Some(def);
            }
            if let Some(def) = package_marker_matches(entry) {
                return Some(def);
            }
        }
    } else if is_launcher(&first_base, PYTHON_LAUNCHERS) {
        let rest: Vec<&str> = tokens.collect();
        if let Some(pos) = rest.iter().position(|t| *t == "-m") {
            if let Some(module) = rest.get(pos + 1) {
                if let Some(def) = python_module_matches(module) {
                    return Some(def);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ── 테이블 완전성 ──────────────────────────────────────────────────────

    #[test]
    fn table_has_the_thirty_three_usable_agents() {
        // claude-agent-teams(Orca 전용 shim)를 제외한 33종.
        assert_eq!(AGENT_DEFS.len(), 33, "expected 33 usable agents");
        // claude-agent-teams는 의도적으로 없다.
        assert!(agent_def_by_id("claude-agent-teams").is_none());
    }

    #[test]
    fn every_agent_has_complete_required_fields() {
        use std::collections::HashSet;
        let mut ids = HashSet::new();
        for def in AGENT_DEFS {
            assert!(!def.id.is_empty(), "id must not be empty");
            assert!(ids.insert(def.id), "duplicate id: {}", def.id);
            assert!(
                !def.display_name.is_empty(),
                "{}: display_name must not be empty",
                def.id
            );
            assert!(
                !def.launch_program.is_empty(),
                "{}: launch_program must not be empty",
                def.id
            );
            assert!(
                !def.detect_cmd.is_empty(),
                "{}: detect_cmd must not be empty",
                def.id
            );
            assert!(
                !def.expected_process.is_empty(),
                "{}: expected_process must not be empty",
                def.id
            );
        }
    }

    #[test]
    fn known_agents_have_definitions() {
        assert_eq!(agent_def(AgentKind::Claude).map(|d| d.id), Some("claude"));
        assert_eq!(agent_def(AgentKind::Codex).map(|d| d.id), Some("codex"));
        assert!(agent_def(AgentKind::Custom).is_none());
    }

    // ── argv separator (버그 수정 회귀) ────────────────────────────────────

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

    /// `-`로 시작하는 프롬프트가 claude에 의해 플래그로 오인되면 안 된다.
    /// `--` 분리자가 있으면 위치 인자로 전달된다(suaegi 실측).
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

    /// codex(clap)도 `--` 뒤를 위치 인자로 받는다 — separator를 켜 둔다.
    #[test]
    fn codex_keeps_the_dash_separator() {
        let spawn = build_spawn(
            AgentKind::Codex,
            None,
            Some("-x flagged prompt"),
            PathBuf::from("/tmp"),
            24,
            80,
        );
        assert_eq!(
            spawn.args,
            vec!["--".to_string(), "-x flagged prompt".to_string()],
        );
    }

    /// grok은 Orca가 `--` separator를 켠 유일한 에이전트다.
    #[test]
    fn grok_spawns_with_the_dash_separator() {
        let def = agent_def_by_id("grok").expect("grok row");
        let spawn = spawn_for_def(def, Some("help"), PathBuf::from("/tmp"), 24, 80);
        assert_eq!(spawn.program, "grok");
        assert_eq!(spawn.args, vec!["--".to_string(), "help".to_string()]);
    }

    /// separator가 실제로 갈라지는 지점: flag-prompt(opencode)는 `--` 없이
    /// `--prompt <p>`를 쓴다. argv(claude)의 `-- <p>`와 여기서 처음 갈라진다.
    #[test]
    fn flag_prompt_agent_diverges_from_argv_separator() {
        let opencode = agent_def_by_id("opencode").expect("opencode row");
        let spawn = spawn_for_def(opencode, Some("do it"), PathBuf::from("/tmp"), 24, 80);
        assert_eq!(spawn.program, "opencode");
        assert_eq!(
            spawn.args,
            vec!["--prompt".to_string(), "do it".to_string()],
            "flag-prompt must pass the prompt as a flag value, not an argv positional"
        );
    }

    /// flag-prompt-interactive(gemini)는 `--prompt-interactive <p>`.
    #[test]
    fn flag_prompt_interactive_uses_the_interactive_flag() {
        let gemini = agent_def_by_id("gemini").expect("gemini row");
        let spawn = spawn_for_def(gemini, Some("hello"), PathBuf::from("/tmp"), 24, 80);
        assert_eq!(
            spawn.args,
            vec!["--prompt-interactive".to_string(), "hello".to_string()],
        );
    }

    /// copilot의 flag-interactive 리터럴은 `-i`.
    #[test]
    fn copilot_uses_the_interactive_short_flag() {
        let copilot = agent_def_by_id("copilot").expect("copilot row");
        let spawn = spawn_for_def(copilot, Some("hello"), PathBuf::from("/tmp"), 24, 80);
        assert_eq!(spawn.args, vec!["-i".to_string(), "hello".to_string()]);
    }

    /// stdin-after-start 에이전트는 6a에서 프롬프트 없이 빈 TUI로 뜬다 —
    /// 고정 launch_args만 남는다.
    #[test]
    fn stdin_agent_launches_bare_in_6a() {
        let kiro = agent_def_by_id("kiro").expect("kiro row");
        let spawn = spawn_for_def(kiro, Some("write tests"), PathBuf::from("/tmp"), 24, 80);
        assert_eq!(spawn.program, "kiro-cli");
        assert_eq!(spawn.args, vec!["chat".to_string(), "--tui".to_string()]);
        assert!(
            !spawn.args.iter().any(|a| a == "write tests"),
            "6a must not inject the prompt for stdin-after-start agents"
        );
    }

    /// command-code는 고정 `--trust` 뒤에 argv 프롬프트를 `--`로 붙인다.
    #[test]
    fn command_code_keeps_fixed_arg_then_separated_prompt() {
        let cc = agent_def_by_id("command-code").expect("command-code row");
        let spawn = spawn_for_def(cc, Some("go"), PathBuf::from("/tmp"), 24, 80);
        assert_eq!(
            spawn.args,
            vec!["--trust".to_string(), "--".to_string(), "go".to_string()],
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

    // ── 감지 (`match_agent`) ───────────────────────────────────────────────

    #[test]
    fn matches_agent_from_a_command_line() {
        assert_eq!(match_agent("claude --resume").map(|d| d.id), Some("claude"));
        assert_eq!(
            match_agent("/usr/local/bin/codex").map(|d| d.id),
            Some("codex")
        );
        // claude는 node 스크립트로 실행될 수 있다 — 패키지 마커로 잡는다.
        assert_eq!(
            match_agent("node /opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/cli.js")
                .map(|d| d.id),
            Some("claude")
        );
        // Windows 경로 구분자 + `.exe` 확장자.
        assert_eq!(
            match_agent("C:\\tools\\codex\\codex.exe --help").map(|d| d.id),
            Some("codex")
        );
    }

    #[test]
    fn does_not_match_incidental_mentions() {
        assert_eq!(match_agent("/bin/zsh").map(|d| d.id), None);
        assert_eq!(match_agent("vim claude-notes.md").map(|d| d.id), None);
        assert_eq!(match_agent("grep codex README.md").map(|d| d.id), None);
        assert_eq!(match_agent("").map(|d| d.id), None);
    }

    /// 회귀: 실행 파일 토큰은 basename만 봐야 한다. 에이전트명과 같은 디렉토리
    /// 아래의 무관한 실행 파일을 에이전트로 오인하면 안 된다.
    #[test]
    fn does_not_match_an_executable_under_a_directory_named_like_an_agent() {
        assert_eq!(match_agent("~/code/codex/run.sh").map(|d| d.id), None);
        assert_eq!(
            match_agent("/home/claude/bin/backup.sh").map(|d| d.id),
            None
        );
        assert_eq!(
            match_agent("/home/claude-code/scripts/deploy.sh").map(|d| d.id),
            None
        );
    }

    /// node 패키지 마커로 인터프리터 래핑된 codex/gemini를 잡는다(Orca
    /// `NODE_PACKAGE_SCRIPT_ENTRYPOINTS`). 진입점 basename이 아니라 경로의
    /// 패키지 마커로 식별한다.
    #[test]
    fn node_package_marker_identifies_wrapped_clis() {
        assert_eq!(
            match_agent("node /usr/lib/node_modules/@openai/codex/bin/codex.js").map(|d| d.id),
            Some("codex")
        );
        assert_eq!(
            match_agent("node /usr/lib/node_modules/@google/gemini-cli/dist/index.js")
                .map(|d| d.id),
            Some("gemini")
        );
    }

    /// python `-m aider`를 잡는다(python 진입점 인식).
    #[test]
    fn python_module_identifies_aider() {
        assert_eq!(
            match_agent("python -m aider --model gpt-4").map(|d| d.id),
            Some("aider")
        );
        assert_eq!(
            match_agent("python3 -m aider.main").map(|d| d.id),
            Some("aider")
        );
    }

    /// node-pty가 보고하는 패키지 플랫폼 바이너리 접두를 잡는다.
    #[test]
    fn packaged_platform_binary_prefix_is_recognized() {
        assert_eq!(
            match_agent("/opt/codex/codex-aarch64-apple-darwin").map(|d| d.id),
            Some("codex")
        );
        assert_eq!(
            match_agent("/opt/grok/grok-x86_64-unknown-linux").map(|d| d.id),
            Some("grok")
        );
    }

    /// `continue`는 셸 빌트인이라 프로세스로 나타나지 않는다 — 감지는 `cn`.
    #[test]
    fn continue_is_detected_as_cn_not_the_builtin() {
        assert_eq!(match_agent("cn --resume").map(|d| d.id), Some("continue"));
        // 순진하게 `continue`를 이름으로 넣었다면 여기서 오탐이 났을 것이다.
        assert_eq!(match_agent("continue").map(|d| d.id), None);
    }

    /// `command-code`는 풀네임으로 감지 — Windows 빌트인 `cmd.exe`와 충돌 회피.
    #[test]
    fn command_code_does_not_collide_with_cmd_exe() {
        assert_eq!(
            match_agent("command-code --trust").map(|d| d.id),
            Some("command-code")
        );
        assert_eq!(match_agent("cmd.exe /C echo hi").map(|d| d.id), None);
    }

    /// mistral-vibe는 `vibe` 바이너리로 뜬다.
    #[test]
    fn mistral_vibe_detects_by_its_vibe_binary() {
        assert_eq!(match_agent("vibe").map(|d| d.id), Some("mistral-vibe"));
    }

    // ── 설치 감지 ──────────────────────────────────────────────────────────

    struct FakePath(&'static [&'static str]);
    impl CommandProbe for FakePath {
        fn on_path(&self, cmd: &str) -> bool {
            self.0.contains(&cmd)
        }
    }

    #[test]
    fn detect_installed_finds_agent_by_primary_command() {
        let claude = agent_def_by_id("claude").unwrap();
        assert!(detect_installed(
            claude,
            &FakePath(&["claude"]),
            Runtime::MacOS
        ));
        assert!(!detect_installed(
            claude,
            &FakePath(&["codex"]),
            Runtime::MacOS
        ));
    }

    /// mistral-vibe는 별칭 `mistral-vibe`로도 설치로 친다(detect_cmd는 `vibe`).
    #[test]
    fn detect_installed_honors_aliases() {
        let vibe = agent_def_by_id("mistral-vibe").unwrap();
        // primary 이름이 없어도 별칭이 있으면 설치로 친다.
        assert!(detect_installed(
            vibe,
            &FakePath(&["mistral-vibe"]),
            Runtime::Linux
        ));
        assert!(!detect_installed(
            vibe,
            &FakePath(&["something-else"]),
            Runtime::Linux
        ));
    }

    #[test]
    fn dimensions_are_carried_into_the_spawn() {
        let spawn = build_spawn(AgentKind::Codex, None, None, PathBuf::from("/tmp"), 40, 120);
        assert_eq!((spawn.rows, spawn.cols), (40, 120));
    }

    /// 두 설치 게이트(`unsupported_runtimes` 배제, `required_commands` AND)는 현재
    /// 33행이 모두 빈 값이라 프로덕션에선 도달 불가다(6a 리뷰 NOTE). 그러나 로직
    /// 자체는 미래 에이전트가 이 필드를 채우는 순간 도달 가능해지므로, 합성
    /// `AgentDef`로 **로직을 고정**한다 — 도달 불가 입력을 엄밀히 검증하는 것이 아니라,
    /// 실제 에이전트가 쓰게 될 코드 경로를 미리 못 박는 것이다.
    const GATE_DEF: AgentDef = AgentDef {
        id: "synthetic",
        display_name: "Synthetic",
        launch_program: "synth",
        launch_args: &[],
        launch_by_platform: &[],
        detect_cmd: "synth",
        detect_aliases: &[],
        // **두 개**여야 AND 게이트가 검증된다 — 하나면 `.all()`과 `.any()`가
        // 동일해 `.all()→.any()` 변형이 안 잡힌다(회귀 메모리 §1: 두 구현이
        // 처음 갈라지는 지점은 원소 ≥2에 하나만 존재할 때다).
        required_commands: &["helper", "extra"],
        unsupported_runtimes: &[Runtime::Windows],
        expected_process: "synth",
        package_marker: None,
        prompt_injection: PromptInjection::None,
        status: StatusSource::OscTitle,
    };

    /// 배제 런타임이면 primary·required가 모두 PATH에 있어도 설치로 치지 않는다.
    #[test]
    fn detect_installed_excludes_unsupported_runtimes() {
        // Windows는 배제 목록에 있다 — 모든 커맨드가 있어도 false.
        assert!(!detect_installed(
            &GATE_DEF,
            &FakePath(&["synth", "helper", "extra"]),
            Runtime::Windows
        ));
        // 지원 런타임에선 나머지 조건이 충족되면 true.
        assert!(detect_installed(
            &GATE_DEF,
            &FakePath(&["synth", "helper", "extra"]),
            Runtime::Linux
        ));
    }

    /// `required_commands`는 AND 게이트 — 하나라도 빠지면 설치 아님.
    #[test]
    fn detect_installed_requires_every_required_command() {
        // primary + required 하나(`helper`)만 있고 다른 하나(`extra`)가 없으면
        // false여야 한다. 이 케이스가 `.all()`(false)과 `.any()`(true)를 가른다 —
        // 이게 있어야 `.all()→.any()` 변형이 잡힌다.
        assert!(!detect_installed(
            &GATE_DEF,
            &FakePath(&["synth", "helper"]),
            Runtime::Linux
        ));
        // required가 하나도 없으면 당연히 false.
        assert!(!detect_installed(
            &GATE_DEF,
            &FakePath(&["synth"]),
            Runtime::Linux
        ));
        // primary + required 전부 있으면 true.
        assert!(detect_installed(
            &GATE_DEF,
            &FakePath(&["synth", "helper", "extra"]),
            Runtime::Linux
        ));
    }

    /// **Claude만 훅, 그 외는 OSC 타이틀.** 이 매핑이 6b-A 배선의 게이트다 —
    /// Claude가 OscTitle로 새면 타이틀 감지가 훅 상태를 덮고, Codex/Custom이 Hooks로
    /// 새면 그들의 유일한 상태 신호(타이틀)가 배선되지 않는다.
    #[test]
    fn status_source_routes_only_claude_to_hooks() {
        assert_eq!(status_source_for(AgentKind::Claude), StatusSource::Hooks);
        assert_eq!(status_source_for(AgentKind::Codex), StatusSource::OscTitle);
        assert_eq!(status_source_for(AgentKind::Custom), StatusSource::OscTitle);
    }
}
