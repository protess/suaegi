# Plan 5 조사: hook 서버 · diff 패널 · 레이아웃 복원

> 2026-07-21. 조사 에이전트 둘이 **소스를 직접 읽고, 실제로 실행해** 확인한 것만 적는다.
> 코드 주장에는 `file:line`, 실측에는 캡처한 출력이 붙어 있다.
>
> **문서보다 로컬 설치가 이긴다.** 문서 조사가 낸 주장 중 하나는 실측으로 반증됐다
> (`CLAUDE_CONFIG_DIR`이 "존재하지 않는다" — 이 기계에서 설정돼 동작 중이다).
> 이 문서의 hook 페이로드는 전부 실제로 `claude`를 돌려 캡처한 것이다.
>
> 로컬 버전: Claude Code **2.1.216**, Codex CLI **0.144.3**, macOS.

---

## 0. 요약 — 이 조사가 바꾼 결정

1. **주입은 `--settings` 인라인 JSON.** 사용자 설정과 **병합**된다(실측). 사용자의 전역 설정을
   전혀 건드리지 않는다. Orca는 `~/.claude/settings.json`을 직접 고치는데 **따라 하지 않는다.**
2. **상관관계는 환경변수 상속.** pane 키를 PTY env에 심으면 에이전트가, 다시 훅 서브프로세스가
   상속한다. PID 매칭이나 pane당 포트가 아니다.
3. **상태는 셋이 아니라 넷** — `working / blocked / waiting / done`.
4. **`StopFailure`를 반드시 등록한다.** 없으면 API 오류 뒤 pane이 **영원히 `working`**에 멈춘다.
5. **`AskUserQuestion`은 도구 이름으로 특수 처리**해야 한다. 자동 허용이라 사람을 기다리는 동안
   `PermissionRequest`가 아니라 `PreToolUse`를 낸다.
6. **Codex도 훅이 있다** — 배지가 Claude 전용일 필요가 없다. 단 트러스트 해시 + `CODEX_HOME`
   미러링이 필요해 Claude보다 훨씬 무겁다.
7. **diff 패널은 예상보다 작다.** `suaegi-git/src/compare.rs`가 이미 완성·테스트돼 있고 앱 배선만
   없다. **Orca의 설계(양쪽 blob을 Monaco에 던지기)를 따라 하면 안 된다** — 우리에겐 더 비싸다.
8. **레이아웃 트리는 왕복 가능하다.** `pane_grid`의 `Node`(읽기)/`Configuration`(쓰기)로.
   **스키마 버전을 올릴 필요 없다.**

---

## 1. Claude Code hook 프로토콜 (실측)

### 1.1 이 기계의 활성 설정은 `~/.claude/settings.json`이 아니다

`CLAUDE_CONFIG_DIR=/Users/james/.config/claude-musinsa`가 설정돼 있다. 활성 파일은
`~/.config/claude-musinsa/settings.json`이고 `~/.claude/settings.json`과 훅 목록이 다르다.

> **프로젝트 제약**: suaegi가 Claude 설정을 읽거나 쓰는 코드는 **반드시 `CLAUDE_CONFIG_DIR`을
> 존중해야 한다.** Orca는 `homedir()/.claude`를 하드코딩해서 이 기계에서는 엉뚱한 파일을 본다.

### 1.2 주입 — `--settings`가 병합된다 (실측)

```
claude -p --settings '{"hooks":{...}}' --permission-mode bypassPermissions --model haiku '...'
```

사용자의 활성 설정에 이미 PreToolUse/PostToolUse/Stop 훅이 **있는 상태에서**, 주입한 훅 4개
(SessionStart, UserPromptSubmit, PreToolUse, Stop)가 **전부 발화**했다. 덮어쓰기가 아니라 병합이다.

우선순위(문서): managed policy > `--settings` > `.claude/settings.local.json` >
`.claude/settings.json` > user settings. `hooks`를 포함한 배열은 **모든 스코프에서 병합**된다.

| 방법 | 판정 |
|---|---|
| **`--settings` 인라인 JSON** | **채택.** 파일 흔적 없음, 세션 단위, 병합. 단 argv가 `ps`에 보이므로 **토큰은 env로** |
| `--settings /path/file.json` | 대안. 토큰을 0600 파일에 둘 수 있다 |
| `CLAUDE_CONFIG_DIR` 리다이렉트 | **금지.** 사용자의 설정·플러그인·모델 선호·인증을 통째로 갈아치운다 |
| 사용자 전역 설정 편집 (Orca 방식) | **금지** |
| worktree의 `.claude/settings.local.json` | **금지.** 사용자 저장소를 오염시킨다 |

### 1.3 실제 페이로드 (캡처, 문서 요약 아님)

stdin으로 **JSON 객체 하나**가 온다(개행 구분 아님).

```json
// SessionStart — source가 있고 permission_mode/prompt_id는 없다
{"session_id":"2b763cb6-...","transcript_path":"<CLAUDE_CONFIG_DIR>/projects/<slug>/<sid>.jsonl",
 "cwd":"...","hook_event_name":"SessionStart","source":"startup"}

// PreToolUse — tool_use_id를 나른다
{"session_id":"...","prompt_id":"...","permission_mode":"bypassPermissions",
 "hook_event_name":"PreToolUse","tool_name":"Bash",
 "tool_input":{"command":"echo hi","description":"..."},"tool_use_id":"toolu_01Lsso..."}

// Stop — 가장 풍부하다. 문서 요약이 아래 셋을 빠뜨렸다
{"session_id":"...","prompt_id":"...","permission_mode":"default","hook_event_name":"Stop",
 "stop_hook_active":false,"last_assistant_message":"Done. The command printed `hi`.",
 "background_tasks":[],"session_crons":[]}
```

- 모든 이벤트 공통: `session_id`, `transcript_path`, `cwd`, `hook_event_name`
- 첫 턴 이후: `prompt_id`, `permission_mode`
- **관측되지 않음**(문서는 있다고 함): `effort`, `agent_id`, `agent_type`
- `last_assistant_message`는 배지 툴팁에 공짜로 쓸 수 있다
- **`background_tasks`가 중요하다** — 백그라운드 작업이 도는 중에 `done`으로 넘기지 않으려면 필요

### 1.4 상태 매핑

| 이벤트 | 상태 |
|---|---|
| `UserPromptSubmit`, `PostToolUse`, `PostToolUseFailure`, `PreToolUse`(일반 도구) | working |
| `PermissionRequest`, `PreToolUse` where `tool_name == AskUserQuestion` | **waiting** |
| `Stop`, `StopFailure` | done |
| `SubagentStart`/`SubagentStop`/`TeammateIdle` | working을 수정, done을 만들지는 않음 |

**`StopFailure`가 필수인 이유** (Orca `hook-settings.ts:30-63` 주석): API/모델 오류 시 Claude가
정상 `Stop` 훅을 건너뛴다. 이게 없으면 **pane이 영원히 도는 스피너로 남는다.**

**`AskUserQuestion` 특수 처리가 필수인 이유**: 이 도구는 자동 허용이라 사람을 기다리는 동안
`PermissionRequest`가 아니라 `PreToolUse`를 낸다. Orca는 `Notification` 훅을 **일부러 등록하지
않고** 이 방식을 쓴다(`agent-hook-listener.ts:2600-2607`).

**구조적으로 관측 불가능한 것**: "에이전트가 끝났다"와 "사람 입력을 기다리며 놀고 있다"는
`Stop` 이후 같은 상태다. 둘의 구분은 프로토콜 사실이 아니라 **UI 정책 결정**이다.

### 1.5 실패·타임아웃 — 설계에 직접 영향

- 기본 타임아웃 **600s**(`UserPromptSubmit` 30s, `MessageDisplay` 10s). 훅별 `"timeout"` 재정의 가능
- exit **0** = 성공, stdout을 제어 JSON으로 파싱. exit **2** = 차단, stderr가 차단 사유.
  그 외 = 비차단, stderr 첫 줄이 트랜스크립트에 뜬다
- **훅은 턴을 블록한다**(PreToolUse, UserPromptSubmit, Stop, PermissionRequest, PreCompact 등).
  같은 이벤트의 여러 훅은 **병렬** 실행

> **따라서 훅 명령은 빠르고 fire-and-forget이어야 하고, 항상 exit 0이어야 하고, stdin을 비워야
> 한다. suaegi의 HTTP 서버가 멈췄다고 사용자의 에이전트가 멎으면 안 된다.**

`"async": true`가 스키마에 있고 블로킹을 아예 피할 수 있다 — **미검증, 스파이크 대상.**

---

## 2. 상관관계 — Orca의 기제 (그리고 우리가 쓸 것)

`POST http://127.0.0.1:$PORT/hook/<source>`, 임시 포트, **루프백 전용**, 헤더 토큰(없으면 403),
본문은 form-urlencoded. 상한: 1MB, slowloris 5s (`shared/agent-hook-listener.ts:68,78`).

**pane 식별은 환경변수 상속으로 한다** — PID 매칭도, pane당 포트도 아니다:

1. PTY 스폰 시 자식 env에 `ORCA_PANE_KEY` 설정 (`main/ipc/pty.ts:3995`)
2. 에이전트 프로세스가 상속 → 훅 서브프로세스가 다시 상속
3. 훅 스크립트가 그대로 되돌려 보낸다 (`installer-utils.ts:210,233`)

**PID 재사용·서브에이전트·셸 re-exec를 전부 견딘다** — PID 매칭은 셋 다 못 견딘다.
키는 **안정된 UUID**여야 한다(렌더러 로컬 인덱스 금지 — `stable-pane-id.ts:1-3`).

**훅 명령 하드닝** (`installer-utils.ts:130-148`) — 그대로 베낀다:

```sh
if [ -f 'PATH' ] && [ -r 'PATH' ] && [ -x 'PATH' ]; then /bin/sh 'PATH'; else <stdin drain>; fi
```

이유: 스크립트가 사라진 낡은 훅 항목이 exit 127을 내면 **모든 도구 호출마다** 사용자
트랜스크립트에 오류가 뜬다. 가드가 있으면 조용한 no-op이 된다. curl은 `--max-time 1.5`.
Windows에서는 `%SystemRoot%\System32\curl.exe`로 한정한다(저장소 로컬 `curl.exe` 탈취 방지).

**Orca가 하는데 우리가 안 할 것**: 사용자 전역 설정 편집, `CLAUDE_CONFIG_DIR` 무시.
**MVP에 필요 없는 것**: pane 이동에 따르는 `transferPaneAuthority`/alias 테이블(우리에겐 pane
이동 기능이 없다). 다만 paneKey가 불변이라고 가정하지는 말 것.

---

## 3. Codex — 훅이 있다

**로컬 확인**: `~/.codex/config.toml:47`의 `[features]`에 **`hooks = true`**, 그리고
`~/.codex/hooks.json`이 이미 쓰이고 있다. 스키마는 **Claude와 같은 모양**이다.

Orca가 등록하는 6개(`main/codex/hook-service.ts:75-107`): SessionStart, UserPromptSubmit,
PreToolUse, PermissionRequest, PostToolUse, Stop — working/waiting/done 기계에 충분하다.

**두 가지 복잡성**:
1. **훅 트러스트.** Codex는 훅을 트러스트 승인해야 실행한다. (sourcePath, eventLabel, groupIndex,
   handlerIndex, command, timeout, matcher)에 대한 해시를 `config.toml`에 쓴다. **명령 바이트가
   바뀌면 해시를 다시 만들어야 한다** — 신뢰받지 않은 훅은 그냥 안 돈다
2. **`CODEX_HOME` 리다이렉트.** Codex엔 `--settings` 대응물이 없어서, Orca는 자기 소유
   CODEX_HOME으로 리다이렉트하고 사용자 설정을 **미러링**한다. 미러는 드리프트 유지보수 부담이다

**권고**: Claude를 `--settings`로 먼저, **인제스트 계층을 에이전트 무관하게 설계**(Orca의
`/hook/<source>` 라우트 모양), Codex는 그 다음. 불가능하다고 적으면 틀린 것이다.

---

## 4. 화해 — 훅과 폴링은 다른 질문에 답한다

`presence.rs`는 **PTY 포그라운드에 에이전트 프로세스가 있나**만 답한다. 헤더 주석이 경계를
이미 옳게 적어뒀다. `presence_poll.rs`는 750ms(활성)/2s(유휴) 티어링.

**훅만 줄 수 있는 것**: working vs waiting(권한 프롬프트에 막힌 `claude`와 추론 중인 `claude`는
`ps`에서 바이트 단위로 동일하다), 턴 경계, 의미 페이로드, 지연(<100ms vs 750ms~2s — 한 틱 안에
시작하고 끝나는 턴은 폴링에 아예 안 보인다), `background_tasks`, 세션 정체성.

**폴링만 줄 수 있는 것 — 지우지 말 것**: 프로세스 사망(크래시한 에이전트는 `Stop`을 **안** 낸다 →
없으면 pane이 영원히 `working`), suaegi 밖에서 시작된 에이전트, 훅이 꺼졌거나 잘못 설정된 경우,
콜드 스타트, **Windows**(포그라운드 pgid 개념이 없어 `presence`가 항상 `Unknown`이므로
거기서는 훅이 유일한 소스다 — 둘이 정확히 상보적이다).

**리듀서 규칙** (우선순위 순):

1. **`Exited{code}`가 전부를 이긴다** → done(코드가 0이 아니면 오류 배지).
   **영구히 멈춘 `working` 배지를 막는 유일한 규칙이다**
2. `NoAgent` + 낡은 훅 상태 → 유휴로 정리
3. `Agent(_)` + 신선한 훅 상태 → **훅이 무조건 이긴다.** 750ms 폴링이 50ms 된 푸시를 덮지 않는다
4. `Agent(_)` + 훅 상태 없음 → "에이전트는 있는데 상태 모름". **훅으로 확인된 `working`과
   시각적으로 구별한다** — 사용자가 "모른다"와 "바쁘다"를 구별할 수 있어야 한다
5. `Agent(_)` + 오래된 훅 상태 → **`waiting`을 조용히 `working`으로 감쇠시키지 않는다.**
   답 없는 AskUserQuestion은 몇 시간이고 정당하게 `waiting`이다. `working`만 오래되면 의심한다
6. `Unknown` presence → 훅 단독. **`Unknown`에서 `done`을 합성하지 않는다**

**유일한 진짜 충돌**: 훅은 `working`, 폴링은 `NoAgent`(`Stop`을 잃음 — 크래시/SIGKILL/StopFailure도
실패). 폴링 쪽으로 해소하되 **N번 연속 확인 후에만** — `presence.rs`가 셸이 exec하는 동안
포그라운드를 잠깐 쥐는 전이를 이미 문서화해뒀다. 한 틱에 반응하면 배지가 깜빡인다.

---

## 5. diff 패널

### 5.1 `compare.rs`는 완성돼 있고, 앱 배선만 없다

`crates/suaegi-git/src/compare.rs` 239줄. `branch_compare`(`:58`), `file_diff`(`:194`,
**git의 unified patch 원문을 그대로 반환**), `working_tree_dirty`(`:231`). 테스트 7개.

**워크스페이스 전체에서 비-테스트 호출부가 `lib.rs`의 `pub mod compare;` 하나뿐이다.**
`git_tasks.rs`는 probe/list/add/remove만 감싼다. `lib.rs:28-30`의 `view()`는 2열 `row!`라
셋째 영역이 아예 없다.

→ **Plan 5의 diff 작업은 전부 앱 쪽이다.** git 계층은 손댈 게 없다.

### 5.2 Orca를 따라 하지 않는다

**Orca에는 diff 알고리즘이 없다.** git에서 파일 목록만 받고, 파일마다 **양쪽 blob 전문**을
`git show`로 가져와 Monaco의 diff 에디터에 던진다. Electron이 Monaco를 얹어주니 공짜인 것이다.

우리에겐 Monaco가 없으므로 그 설계는 **더 비싸다** — 필요 없는 diff 알고리즘을 써야 한다.
`file_diff`가 이미 patch를 주므로, **선두 문자(`+`/`-`/` `/`@`)로 줄마다 색만 입히면 된다.**
스펙 항목 6이 "신택스 하이라이트 없음도 허용"이라고 명시한다.

### 5.3 그래도 베낄 것

- **`-c core.quotePath=false`** — 비ASCII 경로가 8진 이스케이프되지 않는다. 우리는 안 넘긴다.
  **미검증**: `-z`만으로도 억제되는지. **한글 파일명으로 실제 테스트할 것**
- **`-M -C`** (리네임 + **복사** 감지). 우리는 `-M`만 — 복사가 `Added`로 보인다
- **명시적 상태 enum**: `ready | invalid-base | unborn-head | no-merge-base | loading | error`.
  우리는 전부 `GitError::Failed` 문자열로 뭉갠다. **"이 브랜치는 main과 공통 조상이 없다"는
  정당한 상태이지 오류가 아니다.** C절에서 가장 값진 차용
- **크기 상한**: 120k줄/측, 6M자 합계, blob 읽기 10MB. **문자 수를 먼저 검사**한다(`.length`라
  싸다). **우리는 어느 계층에도 상한이 없다** — `run_full`이 stdout을 EOF까지 읽어
  `from_utf8_lossy`로 넘기므로 수 MB diff가 그대로 iced `text()`에 들어간다
- **바이너리 판정**: 앞 8192바이트의 NUL 스니핑. 우리의 `additions == None`은 바이너리와
  untracked를 구별하지 못하므로 필요하다

### 5.4 에러·스레드

- `DEFAULT_TIMEOUT = 30s`가 호출마다 걸리고 `branch_compare`는 git을 **5번** 부른다 →
  **최악 ~150초**. 타임아웃 시 프로세스 그룹을 SIGKILL한다(훅/LFS/자격증명 헬퍼 자식까지)
- `merge-base`는 **공통 조상이 없으면 exit 1** → 빈 결과가 아니라 `GitError::Failed`
- 배치: Plan 3 규칙대로 **`Task::perform`(tokio)**. `canonicalize` 같은 블로킹 syscall을 같은
  `Task::perform`에 넣지 않는다. 결과에 `OpId`를 실어 stale을 버린다

---

## 6. 레이아웃 복원

### 6.1 지금 상태

**쓰긴 쓰는데 안 읽는다.** `persisted_snapshot`(`state.rs:374-404`)이 `session.active_worktree_id`를
쓰지만 `from_load`(`:343-368`)는 repos/settings/worktrees 셋만 읽는다. 전수 grep으로 확인:
필드 선언, 테스트 픽스처, 그리고 그 쓰기 — 끝. **`panes`/`focused_pane`은 아예 영속화되지 않는다.**

### 6.2 스키마 — 필드 추가는 공짜, 버전 범프는 하드 브레이크

가드가 `schema_version > SCHEMA_VERSION`에서만 발동한다(`persistence.rs:95-106`). 새 앱이
찍는 값은 여전히 `1`이므로 구버전이 열어도 모르는 키를 무시하고 계속 돈다(`deny_unknown_fields`
없음). 반대로 **버전을 올리면 구버전이 가드에 걸려 저장을 아예 거부한다.**

→ **`#[serde(default)]` 필드를 추가하고 버전은 올리지 않는다.**

### 6.3 `pane_grid` 트리는 왕복 가능하다

`Serialize`는 없지만 공개 API로 완전히 왕복한다(vendored 확인):

- **읽기**: `State::layout() -> &Node`(`state.rs:93`), `Node`의 필드가 **전부 공개**
  (`node.rs:9-30`), `State::get(Pane)`으로 잎의 값
- **쓰기**: `State::with_configuration(Configuration<T>)`(`state.rs:50`), `Configuration`도
  필드 전부 공개(`configuration.rs:6-26`)
- **`Pane`/`Split`의 내부 `usize`는 비공개다** — 직렬화하지 말고 트리를 걸으며 **우리 값으로
  치환**한다. 복원 후 pane id는 달라지지만 무관하다(우리는 `SessionId`로 키를 잡는다)
- 잎에는 `WorktreeId`를 넣는다 — `SessionId`는 실행마다 매기는 카운터라 재시작을 못 넘는다
- `State::layout()`은 maximize 치환 없는 **진짜 트리**를 준다. maximize 상태는
  `State::maximized()`로 따로 읽는다

Orca의 하드코딩 트리(`types.ts:1023-1050`)와 **구조가 동일하다** — 두 설계가 수렴했다.
Orca에서 베낄 디테일: **ratio는 0.5에서 0.005 넘게 벗어날 때만, 소수 3자리로** 저장한다.
float 잡음이 저장을 계속 흔드는 걸 막는다(우리 `Store::save`가 내용 해시로 스킵하므로 유의미).

### 6.4 진짜 설계 결정과 진짜 위험

**결정**: 세션은 **비동기로** 시작하고(`WorktreeSelected` → `SessionStarted`),
`pane_grid::State`는 빈 채로 만들 수 없다. 그래서 복원된 레이아웃은 **모든 세션이 시작을
보고할 때까지 실체화할 수 없다.** 트리를 들고 있다가 한 번에 짓는 쪽이 Orca의
"완전히 성공하기 전엔 쓰지 않는다" 게이트와도 맞는다.

**위험 1 — 부분 복원이 좋은 파일을 덮어쓴다.** Orca는 모든 부팅 단계가 성공한 뒤에야
저장을 푼다(`App.tsx:1106`). 주석이 이유를 적어뒀다: 플래그를 일찍 켜고 뒤 단계가 던지면
"부분 변경된 스토어를 디스크에 직렬화한다 — 이 PR이 고치는 바로 그 데이터 손실"(이슈 #1158).
**우리에겐 대응물이 전혀 없다** — `boot()`가 끝나는 순간부터 `persist()`가 호출 가능하다.

**위험 2 — 삭제 판정에 증거가 필요하다.** `apply_worktree_listing`(`state.rs:430-451`)이
**어떤 목록이 오든** 사라진 worktree의 세션을 정리한다. 빈 목록은 "삭제됨"이 아니라 "저하된
하이드레이션"일 수 있다. Orca는 저장소가 알려져 있고 **실제로 worktree가 로드됐을 때만**
권위로 친다(`terminals.ts:3167-3182`, 같은 이슈 #1158). **레이아웃 복원이 붙으면 폭발 반경이
훨씬 커진다** — 실패한 스캔 한 번이 복원된 레이아웃 전체를 지운다.

### 6.5 worktree 메타데이터 (follow-ups #15)

`persisted_snapshot`이 매 저장마다 `created_with_agent: None`, `created_at_unix_ms: 0`으로
합성한다(`state.rs:388-389`). 앱은 실제 타임스탬프를 계산하는데(`:989-992`) 세션 시작에만
쓰고 영속화 경로에 닿지 않는다.

들어가야 할 곳: `AppState`에 `HashMap<WorktreeId, WorktreeMeta>`를 두고 (1) `from_load`에서
시드, (2) **worktree 생성 시점**(`WorktreeCreated`, `state.rs:872-884`가 지금 `Ok(_created)`를
통째로 버린다)에 기록, (3) `persisted_snapshot`이 자리표시자 대신 그걸 읽는다.

`created_with_agent`는 **채울 소스가 아직 없다** — `WorktreeSelected`가
`AgentKind::Custom, None`을 하드코딩한다(에이전트 선택 UI가 범위 밖이라). **가짜로 채우지 말고
`None`으로 둔다.**

---

## 7. 미검증 — 추측으로 메우지 말 것

1. **`PermissionRequest`와 `Notification`이 발화하는 걸 관측하지 못했다.** print 모드가 권한을
   자동 해결한다. **`waiting` 상태 전체가 여기 달렸다 — 대화형 PTY 테스트가 최우선.**
2. **`AskUserQuestion`의 정확한 도구 이름 문자열.** 매칭에 쓸 값이라 추측 불가
3. **서브에이전트 이벤트의 `agent_id`** — 문서는 있다고 하나 단일 턴 테스트로는 미확인
4. **Codex 페이로드 모양** — 설정 스키마와 이벤트 이름은 확인했지만 실제 페이로드는 미캡처.
   Claude와 바이트 단위로 같다고 가정하지 말 것
5. **`"async": true`** — 턴 블로킹을 피할 수 있는지 미검증. fire-and-forget curl보다 나을 수 있다
6. **`-z`만으로 비ASCII 경로 이스케이프가 억제되는지** — 한글 파일명으로 실측할 것
7. 이 문서의 Orca 줄 번호 중 일부는 위임 읽기에서 왔다 — Rust 인용보다 한 단계 덜 단단하다
