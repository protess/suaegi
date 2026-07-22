# Plan 6 — 에이전트 레지스트리 확장 (claude/codex/custom → 34종)

조사: `docs/superpowers/research/2026-07-22-plan6-agent-registry.md`.
Orca 원본 `stablyai/orca @ v1.4.150-rc.0`. 모든 Orca 인용은 그 조사 문서가
`file:line`으로 고정했다. 이 계획은 그 위에서 **무엇을 만들고, 무엇을 의도적으로
미루는가**를 정한다.

## 0. 목표와 비목표

**목표**: suaegi가 Orca의 34종 코딩 에이전트 CLI를 **선언적 테이블 한 곳**으로
띄우고 감지하고 상태를 표시한다. 새 에이전트 추가가 "테이블 한 줄"이라는 현재
불변식(`agent.rs:32`)을 34종에서도 유지한다.

**비목표 (이번 플랜에서 명시적으로 안 한다)**:
- **전역 사용자 설정 쓰기 금지.** Orca는 비-Claude 훅 설정을 `~/.codex`,
  `~/.cursor`, `~/.gemini`, `~/.factory`, `~/.kimi-code`, `~/.commandcode`에 쓴다
  (조사 §1 C열, §4.1). 이는 suaegi의 **의도된 불변식**(`inject.rs:1-15`:
  "사용자의 Claude 설정을 건드리지 않는다")을 정면으로 위반한다. `.git/info/exclude`
  사건과 같은 부류다. **따라서 비-Claude 에이전트는 v1에서 훅을 심지 않고
  OSC-title 상태만 쓴다.** 에이전트별 per-worktree 설정이 실제로 먹는지는 Claude에
  했던 것처럼 실측이 필요하며 follow-up으로 남긴다(§7 #F1).
- **preflight-trust 파일 쓰기 금지** (cursor/copilot/codex가 `~/.cursor` 등에
  trust 아티팩트를 쓴다, 조사 §4.4). 같은 불변식 위반. 미룬다(§7 #F2). **결과의
  정확한 서술** [Codex S2 정정]: cursor/codex는 `argv`, copilot은 `flag-interactive`
  라 **stdin-after-start가 아니다** — 프롬프트가 스폰 시점에 argv/flag로 이미
  박힌다(준비 게이트 이전). 첫 실행 trust 대화상자가 뜨면 그 프롬프트의 운명은
  (무시/재큐/대화상자에서 멈춤) **미검증**이다. "준비 신호가 삼킴을 막는다"는 앞선
  논리는 이 세 에이전트엔 오적용이었다. #F2에서 에이전트별로 확인한다(trust 승인
  후 CLI가 진행하는가, argv 프롬프트가 유실되는가).
- draft-prompt(`--prefill`/env prefill)와 아이콘/브랜딩은 UX 부가라 v1에서 뺀다
  (§7 #F3, #F4).

## 1. 두 마일스톤

이 플랜은 **두 PR**로 나눈다. 각각 독립적으로 머지 가능하고 사용자 가치가 있다.

- **6a — 레지스트리·모델·실행** (상태는 아직 Claude만): 34종을 선언적 테이블로,
  프롬프트 주입 argv/flag 모드, `--` 구분자 버그 수정, detect/launch 분리, 감지
  정교화(인터프리터 래핑·패키지 바이너리), 설치 감지. **끝나면 34종이 올바르게
  뜬다.** stdin 에이전트는 프롬프트 없이(빈 TUI로) 뜨는 것까지.
- **6b — 상태·stdin 주입**: OSC-title 범용 상태 감지기(비-Claude 33종의 상태
  신호) + stdin-after-start 준비-게이트 프롬프트 주입(19종). **끝나면 stdin
  에이전트도 프롬프트가 들어가고, 모든 에이전트가 working/idle 상태를 보인다.**

## 2. 모델 (6a)

현재 `AgentDef`(`agent.rs:22-49`)는 `Claude|Codex` 2행 + `PromptInjection::{Argv,None}`.
이걸 다음으로 확장한다.

### 2.1 `AgentKind` → id-키 테이블

34-변형 enum 대신 **`&'static str` id로 키잉하는 정적 테이블**(Orca의
`TUI_AGENT_CONFIG` 미러). "한 줄 추가" 속성을 지킨다. `AgentKind`는 남기되
`Claude`/`Codex`를 특수 취급하는 곳(훅 주입)만 참조하고, 나머지는 id 문자열로 다룬다.

```rust
pub struct AgentDef {
    pub id: &'static str,                 // "claude", "cursor", "kiro" ... (TuiAgent)
    pub display_name: &'static str,       // tui-agent-display-names.ts
    pub launch_program: &'static str,     // 토큰화된 launchCmd의 첫 토큰
    pub launch_args: &'static [&'static str], // 나머지 토큰 (kiro-cli의 `chat --tui` 등)
    pub launch_by_platform: &'static [(Platform, &'static str, &'static [&'static str])], // launchCmdByPlatform
    pub detect_cmd: &'static str,         // PATH에서 설치 증명하는 바이너리 (launch와 다를 수 있음)
    pub detect_aliases: &'static [&'static str], // detectCmdAliases
    pub required_commands: &'static [&'static str], // detectRequiredCommands (AND 게이트)
    pub unsupported_runtimes: &'static [Runtime],   // detectUnsupportedRuntimes
    pub expected_process: &'static str,   // 프로세스 테이블 감지명
    pub package_marker: Option<&'static str>, // node 패키지 경로 마커 (§2.3)
    pub prompt_injection: PromptInjection,
    pub status: StatusSource,             // Hooks(Claude) | OscTitle (그 외 전부, v1)
}
```

`AGENT_DEFS`는 조사 §1 표의 행을 옮긴다. **launchCmd 문자열의 program/args 경계는
작성 시점에 쪼갠다**(Orca는 런타임 토큰화, 우리는 선언 시 분리). 예:
`kiro-cli chat --tui` → program `kiro-cli`, args `["chat","--tui"]`.

**34행이 아니라 33 사용 가능 + 1 N/A** [Codex B2]. `claude-agent-teams`
(`tui-agent-config.ts:55-70`)는 **Orca 전용**이다 — `detectCmd: 'orca'`,
`launchCmd: 'orca claude-teams'`로 **Orca 자신의 CLI shim**에 shell-out한다.
suaegi 세계엔 `orca` 바이너리가 없다. 이 행을 "그대로" 옮기면 PATH에 아무것도
안 잡히거나(무해하지만 무의미) 구현자가 가상의 래퍼를 만든다(footgun). **테이블에서
제외하거나 명시적으로 N/A로 표시**하고, §6 완전성 테스트가 이 무의미한 행을 강제하지
않게 한다.

### 2.2 `PromptInjection` 2 → 6 변형

```rust
pub enum PromptInjection {
    /// 프롬프트를 argv 마지막 인자로. separator는 **에이전트별 opt-in** —
    /// Orca에서 grok만 `--`를 켠다(조사 §1 grok, §3 마지막 항목).
    Argv { separator: Option<&'static str> },
    /// 프롬프트를 플래그 값으로: `<flag> <prompt>`. [Codex N1로 확정]
    /// flag-prompt → `--prompt <v>` (opencode, mimo-code;
    /// `tui-agent-startup.ts:117`). `-p` 아님.
    Flag(&'static str),
    /// 프롬프트를 seed하되 제출/종료하지 않는 인터랙티브 플래그. [Codex N1로 확정]
    /// flag-prompt-interactive → `--prompt-interactive <v>` (gemini, antigravity;
    /// `:155`); flag-interactive → `-i <v>` (copilot; `:167`, 리터럴은 `-i`만).
    /// bare `--prompt`는 headless로 종료되므로 이 모드가 필요하다.
    FlagInteractive(&'static str),
    /// 빈 TUI로 띄운 뒤 composer 준비 후 PTY에 써넣는다(19종). §6b에서 구현.
    StdinAfterStart,
    /// hermes 고유 startup-query 계약(조사 §1 hermes). §6b 또는 follow-up.
    HermesQuery,
    None,
}
```

**separator를 per-agent로 (6a) — 단, Orca 테이블을 맹목 복사하지 않는다.**
[Codex B1이 잡은 것] 현재 `build_spawn`(`agent.rs:82-93`)은 모든 argv 에이전트에
`--`를 붙이는데, **이건 버그가 아니라 suaegi의 실측 결과다** — 그 자리 주석이
claude와 codex **둘 다** `--`를 필요로 함을 실측 확인했다고 적고 있고(codex는 에러
메시지가 직접 `-- -x`를 제안), Claude용 회귀 테스트도 있다(`agent.rs:209-224`
`prompt_starting_with_a_dash_is_separated_from_flags`). **따라서 claude/codex에서
`--`를 빼면 안 된다 — 이미 고치고 검증한 것을 되돌리는 것이다.**

모델을 blanket → per-agent `Argv { separator }`로 바꾸는 것은 여전히 옳다(grok의
opt-in을 표현하고, `--`를 원치 않는 미래 에이전트를 구분하려면 필요). 하지만
**기본값 정책**은:
- claude = `Some("--")`, codex = `Some("--")` — **suaegi 실측**(`agent.rs:82-93`).
- grok = `Some("--")` — Orca 문서화(`tui-agent-config.ts:287`).
- **그 외 새 argv 에이전트**(openclaude, command-code, cursor, droid)는 **미검증**
  → `--`가 필요한지 **에이전트별로 실측**하기 전까지 `None`으로 기본하지 **말고**,
  Orca 테이블의 부재를 근거로 삼지도 않는다. Orca가 그들에게 separator를 안 붙이는
  것이 "그 CLI가 bare dash를 견딘다"는 증거가 아니다 — Orca는 프롬프트를 셸용으로
  single-quote만 할 뿐 대상 CLI의 argv 파서는 그대로다(`tui-agent-startup-shell.ts:77-85`).
  실측 전까지는 안전하게 `Some("--")`로 두거나 follow-up(#F7)에서 확인.

**회귀 테스트로 고정**: claude/codex/grok argv 스폰에 `--`가 **있어야** 한다
(mutation: separator를 `None`으로 뒤집으면 기존 dash-prompt 테스트가 실패).
per-agent 필드가 실제로 갈라지는지는, separator가 다른 두 에이전트가 처음 갈라지는
지점에서 단언한다(회귀 메모리 §1 — 최단 시퀀스가 아니라 갈라지는 지점).

### 2.3 감지 정교화 (`match_agent`)

조사 §3의 오작동 표면을 닫는다:
- **인터프리터 래핑**: `codex → @openai/codex`, `gemini → @google/gemini-cli`
  같은 node 패키지 경로 마커(`package_marker`)로 세그먼트 매칭
  (Orca `agent-process-recognition.ts:51-54`). 현재 `SCRIPT_LAUNCHERS`+
  `segment_matches`는 바 디렉토리 세그먼트만 봐 오인 가능. python `-m module`
  인식(aider)도 추가.
- **패키지 플랫폼 바이너리**: `codex-<arch>`, `grok-*` 접두 매칭
  (Orca `:88-95`). 현재 `codex.exe`만 있고 `codex-aarch64-...`는 놓친다.
- **`continue`는 셸 빌트인** → detect는 `cn`. **`command-code`**는 `cmd.exe`
  충돌 회피용 풀네임. 테이블이 이미 이렇게 잡혀 있으니 그대로 옮기고 회귀 테스트로
  못 박는다.

### 2.4 설치 감지 (신규)

`detect_cmd`(+aliases + required_commands)를 PATH에서 찾는다
(Orca `preflight.ts:104` `isCommandOnPath`). known-install-dir 폴백과 버전 고정은
v1에서 뺀다(Orca도 버전 고정 없음). `unsupported_runtimes`로 게이팅.

## 3. 상태 (6b) — OSC-title 범용 감지기

Orca는 OSC 터미널 타이틀 상태기를 돌린다. [Codex N2/Q3로 확정] 실제 위치는
`src/shared/agent-title-status.ts:137-203`의 **단일 함수**(에이전트별 분기 아님).

**단, "범용"이 아니다 — 이게 S1의 핵심.** [Codex S1] 이 함수의 idle/permission
판정은 **닫힌 이름 집합**에 걸려 있다: `AGENT_NAMES`(`agent-name-token-match.ts:16-30`,
13종) + `DROID/HERMES/AGY` 정규식 3종 = **34종 중 16종만** 타이틀에 자기 이름이
나타난다. 그 16종 밖(autohand, ante, goose, amp, kilo, kiro, crush, aug, cline,
codebuff, continue, kimi, mistral-vibe, qwen-code, rovo)은 타이틀에 braille 스피너가
뜨면 "working"은 잡아도(`:172-178`에서 이름 플래그가 하나도 안 맞으면 idle/working
키워드 검사 전에 `return null`) **idle은 타이틀만으로 절대 못 낸다.**

- 따라서 **§1의 "모든 에이전트가 working/idle" 약속을 축소한다**: 이름/스피너가
  타이틀에 나타나는 에이전트만 6b에서 상태를 얻는다. 나머지 ~15종의 완전한 idle
  커버리지는 **에이전트별 follow-up**(#F1/#F6와 같은 층 — 각 CLI가 OSC 타이틀에
  이름을 넣는지 자체가 미검증).
- 그리드 배관은 **이미 있다** [Codex N3]: `grid.rs:102-168`이 `Event::Title`/
  `Event::ResetTitle`을 바운드 deque(`TITLE_CHANGES_CAPACITY = 256`)에 쌓고
  `take_title_changes()`(`grid.rs:657`)로 노출한다. `suaegi-app`이 아직 소비하지
  않을 뿐 — §3은 터미널 층에서 막히지 않고 **앱 층 배선만** 하면 된다.
- `detect_agent_status_from_title`을 순수 함수로 이식하고 이름 집합 게이팅까지
  옮긴다. `StatusSource::OscTitle`인 에이전트는 이 경로가 권위, `Hooks`(Claude)는
  기존 유지. 배지 리듀서(`contract.rs`)는 소스만 갈아끼우고 4값은 그대로.
- **순수 함수 mutation 검증**: 실제 Orca 타이틀 샘플 픽스처(빈/합성 금지 —
  회귀 메모리 §5). 이름 집합에 없는 에이전트가 idle을 못 내는 것도 **의도된 동작**
  으로 단언한다.

## 4. stdin-after-start 주입 (6b)

19종이 빈 TUI로 뜬 뒤 프롬프트를 PTY에 써야 한다. suaegi엔 `PtySession::write`
(`pty.rs:241`)가 **이미 있다** — 없는 건 "composer 준비" 타이밍 게이트다.

- **준비 게이트는 bare "N ms 잠잠"이 아니다** [Codex S3]. Orca의 실제 기본값은
  `render-quiet-after-bracketed-paste`(`tui-agent-config.ts:12-15`)이고,
  `draft-paste-ready-scanner.ts:60-93`에 따르면 **DECSET bracketed-paste enable
  시퀀스 `\x1b[?2004h`를 스트림에서 본 뒤에야** quiet 타이머를 무장한다(1500ms,
  `BRACKETED_PASTE_QUIET_MS`; 8000ms hard timeout, `DRAFT_PASTE_READY_TIMEOUT_MS`).
  이 전제조건이 **TUI 프레임워크가 터미널을 잡기 전 splash/spinner 단계에서 오발**을
  막는다. bare quiet-since-spawn 게이트는 그 가드가 없어 composer가 생기기도 전에
  발화해 프롬프트를 splash에 삼킨다 — 정확히 이 실패 모드를 피해야 한다.
- **v1 게이트**: `\x1b[?2004h`를 기다린 뒤 1500ms quiet window를 무장, 8000ms hard
  timeout. Orca의 두 상수(1500/8000ms)를 출발점으로 쓴다 — 임의값이 아니다(1500ms는
  opencode의 bracketed-paste-enable → composer mount ~1.5-2s 간격을 덮는다).
  그리드에 `\x1b[?2004h` 관측 신호가 있는지 구현 전 확인(없으면 파서에서 노출).
- 프롬프트는 bracketed-paste로 감싸 `PtySession::write`(`pty.rs:241`)로 쓴다.
- 키 입력 순서 보존: 주입은 스폰 직후 1회, 사용자 입력 전에만. 타임아웃하면
  **조용히 포기**하고 사용자가 직접 치게 둔다(오작동보다 미주입이 낫다).

## 5. 태스크 분해

**6a**:
1. `AgentDef`/`PromptInjection`/`StatusSource` 모델 확장 + `AGENT_DEFS` 34행
   (조사 §1 표 그대로). 순수 데이터, 테이블 완전성 테스트(`satisfies` 대응 —
   모든 id가 display_name·detect를 가진다).
2. `--` 구분자 버그 수정 → 에이전트별 separator. 회귀 mutation 검증.
3. `match_agent` 감지 정교화(패키지 마커·python -m·플랫폼 바이너리 접두). 오작동
   케이스별 회귀 테스트.
4. 설치 감지(`detect_cmd` PATH 조회 + unsupported_runtimes 게이팅).
5. 앱 UI 배선: worktree 생성 시 에이전트 선택을 3종에서 34종으로(사이드바 드롭다운/
   목록). 설치 안 된 에이전트 비활성 표시.

**6b**:
6. OSC-title 상태 순수 함수(Orca 매핑 이식) + 그리드 타이틀 → 배지 소스 배선.
7. stdin-after-start 준비-게이트 주입 경로 + 타임아웃 포기.
8. 6b 에이전트들(stdin 19종) end-to-end: 뜨고 → 프롬프트 주입 → OSC 상태.

## 6. 테스트/mutation 전략

- **테이블 완전성**: 34 id 각각이 필수 필드를 갖는지 컴파일/런타임에 강제(빠뜨린 채
  조용히 기본값 금지 — 회귀 메모리 §3 "미러링 코드에 테스트 없음" 방지).
- **separator 수정**: claude=`--` 없음 / grok=`--` 있음, 양방향 mutation.
- **감지**: 각 오작동 표면(node 패키지, python -m, `codex-<arch>`, `continue`
  vs 빌트인, `command-code` vs `cmd.exe`)마다 "이 입력이 프로덕션에서 도달
  가능한가"를 물어 도달 가능한 것만 고정(회귀 메모리 §5).
- **OSC 상태**: 실제 Orca 타이틀 샘플 픽스처. "버그가 있었다면 이 단언이
  움직였을까" 통과해야 함.
- 모든 회귀 테스트는 mutation으로 검증(구현자 계약에 명시, 리뷰어는 직접 되돌려봄 —
  워크플로 메모리).

## 7. Follow-ups (이 플랜이 의도적으로 미룬 것)

- **#F1 에이전트별 per-worktree 설정 주입** — codex/cursor/gemini/droid/kimi/
  command-code가 project-local 설정을 먹는지 **에이전트별 실측**(Claude에 한 것처럼).
  먹으면 훅 기반 정밀 상태를 얻는다. 안 먹으면 OSC-title이 최선. **전역 홈 쓰기는
  하지 않는다** — 불변식.
- **#F2 preflight-trust** — cursor/copilot/codex 첫 실행 trust. 전역 쓰기 없이
  우회할 방법(에이전트별 플래그?)이 있는지 조사. 없으면 사용자가 첫 실행에 수동
  승인.
- **#F3 draft-prompt** (`--prefill`/env prefill), **#F4 아이콘/브랜딩**.
- **#F5 hermes-query** 정밀 계약(§2.2), **#F6 draftPasteReadySignal** 에이전트별
  정교화, **#F7 새 argv 에이전트의 `--` 필요 여부 실측**(openclaude/command-code/
  cursor/droid, Codex B1).

## 8. 미해결 — Codex 교차검증으로 해소 완료

- **#Q1 해소** [Codex N1]: `tui-agent-startup.ts:100-174` `buildAgentStartupPlan`.
  flag-prompt → `--prompt <v>`(`:117`), flag-prompt-interactive →
  `--prompt-interactive <v>`(`:155`), flag-interactive → `-i <v>`(`:167`). `-p` 아님.
- **#Q3 해소** [Codex N2/S1]: 단일 함수(`agent-title-status.ts:137-203`), 단
  16종 이름 집합 게이팅 — §3에 반영.
- **#Q2** stdin 준비 게이트는 **bracketed-paste-enable 대기 + 1500ms quiet +
  8000ms timeout**으로 확정(§4, Codex S3). render-quiet 단독은 오발하므로 안 쓴다.
- **#Q4** 확정: 데이터는 6a에 **33행**(claude-agent-teams 제외), **동작**은 stdin만
  6b. 6a 단독으로 33종이 뜨는 가치가 있다(Codex N6).

## 9. Codex 교차검증 판정: IMPLEMENTABLE-AFTER-FIXES → 위 수정으로 반영 완료

B1(claude/codex의 `--` 유지, 새 argv는 미검증), B2(claude-agent-teams N/A,
34→33+1), S1(6b 상태 약속 축소), S2(trust 논리 정정), S3(준비 게이트),
N1/Q1·N2/Q3(리터럴 플래그·상태 매핑 확정) 전부 계획에 반영했다. 구현 착수 가능.
