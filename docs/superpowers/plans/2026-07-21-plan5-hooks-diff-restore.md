# Suaegi Plan 5: hook 서버 · diff 패널 · 레이아웃 복원 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** MVP를 닫는다. 각 pane이 에이전트가 **일하는 중인지 사람을 기다리는지 끝났는지** 보여주고, worktree의 변경을 diff로 볼 수 있고, 앱을 껐다 켜도 레이아웃이 살아 있다.

**Spec:** `docs/superpowers/specs/2026-07-20-suaegi-mvp-design.md` 항목 5·6·7
**조사:** `docs/superpowers/research/2026-07-21-plan5-hooks-diff-restore.md` — **구현 전 필독.**
아래 제약은 전부 그 문서에 근거가 있고, hook 동작은 **실제로 `claude`를 돌려 캡처**한 것이다.
**선행:** Plan 1~4 머지 완료.

---

## Global Constraints

### 이 플랜의 세 가지 절대 규칙

1. **사용자의 Claude 설정을 건드리지 않는다.** Orca는 `~/.claude/settings.json`을 직접 고치는데
   따라 하지 않는다.

   **주입은 `--settings`가 아니라 worktree의 `.claude/settings.local.json`이다** — 구현 중
   발견: **`suaegi-app`은 `claude`를 실행하지 않는다.** 모든 세션이 평범한 로그인 셸이고
   (`state.rs`가 `AgentKind::Custom, None`으로 스폰한다), 에이전트 선택 UI는 Plan 3·5 모두
   범위 밖이다. 그러니 `--settings`를 넘길 argv가 없고, 사용자가 프롬프트에서 `claude`를
   직접 쳐도 **훅이 등록되지 않아 배지가 영원히 `Unknown`이다.**

   대신 **suaegi가 만든 worktree 안에** 설정 파일을 쓴다. 그 디렉터리는 우리 것이지 사용자의
   저장소가 아니다 — 그래서 "사용자 저장소를 오염시키지 않는다"는 규칙과 충돌하지 않는다.
   사용자가 어떻게 `claude`를 띄우든(맨손, `--resume`, 별칭) 설정이 적용된다는 이점도 있다.

   **그 파일이 우리 diff 패널에 untracked로 뜨면 안 되는데, git ignore로 풀지 않는다.**
   `.git/info/exclude`는 **worktree별이 아니라 공통 git 디렉터리**에 있다(실측:
   `git rev-parse --git-common-dir`). worktree에서 거기 쓰면 **사용자 저장소 전체**(메인
   체크아웃과 다른 모든 worktree 포함)에 영구 ignore 규칙이 박히고, `git worktree remove`로도
   지워지지 않아 우리가 만드는 worktree마다 쌓인다. 사용자가 메인 체크아웃에 자기 `.claude/`를
   두고 있다면(Claude Code를 쓰는 사람이니 그럴 법하다) **그걸 조용히 숨긴다.**
   worktree 전용 `info/exclude`는 git이 아예 읽지 않는다(마커로 확인).

   → **우리 diff 패널에서 걸러낸다.** `compare.rs:176-185`가 `status --porcelain -z`로
   untracked를 모으는 지점에서 `.claude/settings.local.json`을 제외한다(픽스처 포함).
   사용자 저장소를 전혀 건드리지 않고, 정리할 것도 없고, 사용자의 git 설정에 영향받지 않는다.

   **미측정 위험 — Task 2가 확인한다.** 갓 만든 worktree는 신뢰되지 않은 디렉터리이고
   (연구 §7.1), 이제 우리는 **그 안의 프로젝트 스코프 파일**에 훅을 둔다. 신뢰하지 않은
   디렉터리의 훅을 자동 실행하는 것은 신뢰 프롬프트가 막으려는 바로 그것이라, Claude가
   신뢰 전에는 프로젝트 스코프 훅을 아예 안 읽을 가능성이 있다 — `--settings`(argv라
   호출자에게서 온 게 분명하다)에는 없던 게이트다. **추측하지 말고 §1.4와 같은 방식으로
   측정한다**: 신뢰 안 된 디렉터리에 `.claude/settings.local.json`을 두고 `claude`를 띄워,
   신뢰 전후로 훅이 발화하는지 본다.
2. **`CLAUDE_CONFIG_DIR`을 존중한다.** 이 기계에서 설정돼 있어 `~/.claude/settings.json`은 활성
   파일이 **아니다**. 하드코딩하면 엉뚱한 파일을 본다(Orca가 그렇게 한다).
3. **훅이 사용자의 에이전트를 멎게 하면 안 된다.** 훅은 턴을 블록한다. 따라서
   **모든 훅에 `"async": true`**(실측: 턴 지연 18.4s → 3.0s, 전달은 유지), 스크립트는
   존재 가드 → `curl --max-time 1.5` → **항상 exit 0** → 모든 경로에서 stdin 배출.

### 검증 규칙 (Plan 4에서 이어짐)

**헤드리스 위젯 테스트가 가능하다**(`impl Renderer for ()`, `tests/harness/`). Plan 4가 만든
하네스를 그대로 쓴다. 순수 함수로 뽑을 수 있는 것은 뽑아서 실제 값으로 표 테스트한다.

- **모든 테스트를 mutation 검증한다.** 그리고 **어느 단언이 mutant를 죽였는지 확인한다** —
  Plan 4에서 이름을 단 단언이 아니라 대조군에서 죽는 테스트가 나왔다.
- **"버그가 있었다면 이 단언이 움직였을까?"** 를 매번 묻는다. Plan 4의 공허한 테스트 넷이
  전부 이 질문에서 "아니오"였다.
- **"이 입력이 프로덕션에서 도달 가능한가?"** 도 묻는다. mutation이 구조적으로 못 잡는
  유일한 유형이 도달 불가능한 입력을 엄밀히 고정한 테스트다(Plan 4에서 실제로 나왔다).
- **mutation 배치는 공유 트리에서 돌리지 않는다.** `git worktree`나 스크래치 복사본에서.
  적용/복원 후 `touch`(mtime 해상도 때문에 가짜 생존이 난다).
- 심각도 판정에는 **두 번째 프로브**를 붙인다 — "얼마나 영구적이고 무엇이 푸는가".
- 검증할 수 없는 것을 "확인했다"고 쓰지 않는다. 사람 눈이 필요한 것은 PR 체크리스트로.

### 스레딩 (Plan 3·4에서 확립)

- **UI 스레드**: 원자값 읽기, 논블로킹 송신
- **`Task::perform`(tokio)**: `suaegi-git` 전부 — diff 포함
- **워커**: 블로킹(fsync, canonicalize, 스냅샷, 세션 drop, 선택 추출)
- 모든 비동기 결과는 **발신 맥락(`OpId`/대상 id)을 나르고**, 오래된 결과는 버린다

### 공통

- edition 2021, `rust-version = "1.94"`. 파일/모듈 이름에 `utils`/`helpers` 금지
- **`cargo fmt`를 크레이트 전체에 돌리지 않는다**(follow-ups #25 — 관례가 정해지지 않았다).
  건드린 파일만 `rustfmt <file>`
- **`git add -A`를 쓰지 않는다.** 동시에 도는 에이전트의 진행 중 파일을 쓸어담는다. 경로를 명시한다

---

## Task 0 — 계약 확정 (컴파일되는 산출물)

**파일**: `crates/suaegi-app/src/agent_status/contract.rs`.
Task 1~6은 이것이 `cargo check`를 통과한 뒤 시작한다.

### 0.1 `PaneKey` — worktree에 묶이고 재시작을 넘는다

```rust
/// **`WorktreeId`에서 파생한다.** `SessionId`는 실행마다 매기는 카운터라 재시작을
/// 못 넘고, 배열 인덱스는 pane이 닫히면 어긋난다. worktree id는 경로에서 나오므로
/// 앱을 껐다 켜도 같다 — 훅 상관관계와 레이아웃 복원이 **같은 키**를 쓴다.
pub struct PaneKey(WorktreeId);
```
한 worktree에 세션이 재시작돼도 키가 같다. **이것이 의도다** — 배지는 pane의 속성이지
세션 인스턴스의 속성이 아니다.

**그러나 키만으로는 부족하다.** 세션이 교체되면 **옛 Claude 프로세스의 훅이 늦게 도착해
새 세션의 배지를 덮을 수 있다**(훅은 async라 더 그렇다). worktree를 지웠다 같은 경로에
다시 만들어도 같다.

**Claude의 `session_id`로는 못 막는다.** "첫 이벤트의 id를 묶는다"는 규칙은 **그 첫 이벤트가
옛 세션의 늦은 훅일 때** 옛 세션을 새 세대에 묶어버린다. 우리가 발급한 값이어야 한다:

```rust
// 앱이 스폰마다 새로 만들어 env로 심는다 — SUAEGI_SPAWN_NONCE
pub struct SpawnNonce(u64);          // 단조 증가. 프로세스 전역

pub struct PaneBinding { key: PaneKey, expected: SpawnNonce }
```
훅 스크립트가 nonce를 헤더로 되돌려 보내고, **앱은 자기가 방금 심은 값과 다른 이벤트를 버린다.**
스폰 시점에 이미 알고 있는 값이라 "첫 이벤트를 믿는" 창이 없다. 세션 교체 시 nonce를 새로
발급하고 배지를 `Unknown`으로 리셋한다.
(Claude의 `session_id`도 나르되 진단·툴팁용이지 세대 판별에는 쓰지 않는다.)

### 0.2 훅 인입 타입

```rust
pub struct HookEvent {
    pub pane_key: PaneKey,
    pub spawn_nonce: SpawnNonce,         // 세대 판별. 우리가 발급한 값
    pub claude_session_id: String,       // 진단용. 판별에 쓰지 않는다
    pub event: HookEventName,
    pub tool_name: Option<String>,
    pub agent_id: Option<String>,        // Some = 서브에이전트, None = 리드
    pub background_tasks_empty: Option<bool>,
    pub prompt_is_task_notification: bool,
}
pub enum HookEventName {
    SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, PostToolUseFailure,
    PermissionRequest, Stop, StopFailure, SubagentStop, SessionEnd,
}
```
**`background_tasks_empty == None`은 `Stop`에서만 "비지 않음"으로 취급한다**(보수적).
**`StopFailure`는 이 필드를 아예 갖지 않으므로 이 규칙을 적용하지 않는다** — 적용하면
`StopFailure`가 영원히 `done`이 될 수 없어 pane이 계속 돈다(0.3 표 참고). 이벤트별로
갈리는 규칙이므로 `HookEventName`으로 분기한다.

### 0.3 배지 리듀서 — 전 조합 결정표

```rust
pub enum BadgeState { Working, Waiting, Done, Unknown }
/// **훅이 만들 수 있는 상태는 셋뿐이다.** `Unknown`은 훅에서 오지 않고 리듀서가 만든다 —
/// `Option<BadgeState>`로 두면 표에 정의되지 않은 행(`hook == Unknown`)이 생긴다.
pub enum HookState { Working, Waiting, Done }

/// **측정된 API 재시도 창(~210초)보다 길게 잡는다.** 오류 시 Claude는 빨리 실패하지 않고
/// 백오프로 재시도한다(관측: "attempt 7/10", `StopFailure`가 t+210s에 도착). 90초로 두면
/// 정상 재시도 중에 배지가 `Unknown`으로 튀는데, 그때 에이전트는 실제로 일하는 중이라
/// 오해를 부른다. 긴 침묵의 흔한 원인이 재시도라는 것이 이 값의 근거다.
pub const HOOK_STALE_AFTER: Duration = Duration::from_secs(240);
pub const NO_AGENT_CONFIRMATIONS: u8 = 3;   // 750ms 티어에서 ~2.25s

pub struct BadgeInput {
    pub presence: AgentPresence,
    pub hook: Option<(HookState, Instant)>,
    pub previous: BadgeState,            // `NoAgent` streak < 3에서 유지할 값
    pub no_agent_streak: u8,
    pub now: Instant,
}
pub fn reduce(input: &BadgeInput) -> BadgeState;
```

| presence | 훅 상태 | 결과 |
|---|---|---|
| `Exited{code}` | 무엇이든 | `Done` — **최우선** |
| `NoAgent`, streak < 3 | 무엇이든 | `previous` 유지 (셸 exec 중 포그라운드 전이) |
| `NoAgent`, streak ≥ 3 | 무엇이든 | `Done` |
| `Agent(_)` | `Waiting` (나이 무관) | `Waiting` — **절대 감쇠시키지 않는다** |
| `Agent(_)` | `Working`, 90초 이내 | `Working` |
| `Agent(_)` | `Working`, 90초 초과 | `Unknown` |
| `Agent(_)` | `Done`, 나이 무관 | `Done` |
| `Agent(_)` | 없음 | `Unknown` |
| `Unknown` | 있음 | 훅 그대로 (나이 규칙 동일) |
| `Unknown` | 없음 | `Unknown` — **`Done`을 합성하지 않는다** |

`Unknown`은 UI에서 `Working`과 **시각적으로 구별한다**(모른다 ≠ 바쁘다).

**`BadgeState`는 오류를 담지 않는다.** 0이 아닌 종료 코드의 스타일링은 **UI가
`AgentPresence::Exited{code}`를 직접 읽어서** 한다 — 리듀서 반환에 변형을 더하면 배지 상태와
프로세스 사실이 두 곳에서 관리된다. 리듀서는 "무슨 상태인가"만 답한다.
`no_agent_streak`은 `Agent(_)`나 `Exited`를 보면 0으로 리셋한다.

### 0.4 서버 → 앱 전달

**정책은 drop-newest다. drop-oldest가 아니다.** `futures::channel::mpsc`는 가득 차면
새 전송을 거절하거나 재우고, **보내는 쪽이 가장 오래된 것을 꺼낼 수 없다**
(`futures-channel-0.3.33/src/mpsc/mod.rs:378-385`). 용량도 `buffer + 송신자 수`이지
정확히 `buffer`가 아니다.

```rust
pub const HOOK_QUEUE: usize = 256;
// 서버 스레드에서: try_send가 실패하면 그 이벤트를 버린다(재시도·블로킹 없음)
```
훅 폭주 시 **가장 최근 것이 버려진다.** 무계로 두면 앱이 OOM 난다(Plan 2에서 같은 실수를 했다).

**대가를 정직하게 적는다** — "다음 이벤트나 폴링이 곧 고친다"는 **항상 참이 아니다.**
큐가 `Working`들로 차 있는 동안 마지막 `PermissionRequest`나 `Stop`이 버려지면:
폴링은 계속 `Agent`를 보고, 잃어버린 `Waiting`은 재구성할 수 없고, 잃어버린 `Done`은
**90초 뒤에야 `Unknown`**이 되며, 후속 훅이 아예 없을 수도 있다.
→ **버린 개수를 카운터로 노출한다.** pane별 최신 이벤트 합치기(coalescing)는 다음 개선이다.

**구독 배선** — `Subscription::run_with`는 `fn(&D) -> S`라 **`&D`에서 receiver를 옮길 수 없다**
(`iced_futures-0.14.0/src/subscription.rs:198-207`). 공유 홀더 + 안정된 identity로 간다:

```rust
#[derive(Clone)]
struct HookSub { id: u64, slot: Arc<Mutex<Option<mpsc::Receiver<HookEvent>>>> }
impl Hash for HookSub { /* id만 해시한다 — slot은 identity에 들어가지 않는다 */ }

fn build(d: &HookSub) -> Either<BoxStream<HookEvent>, Pending<HookEvent>> {
    match d.slot.lock().take() { Some(rx) => Either::Left(rx.boxed()), None => Either::Right(pending()) }
}
```
`Arc`여야 `&D`에서 꺼낼 수 있고, `Hash`가 불변 `id`만 보아야 **레시피 identity가 앱 수명 내내
같다**. **훅 구독은 조건부로 붙였다 뗐다 하지 않는다** — identity가 유지되는 동안만 첫 스트림이
살아 있으므로, iced가 레시피를 떨구면 receiver도 같이 사라지고 이후 빌더는 `pending`밖에
못 준다.
- **서버는 앱보다 먼저 뜬다.** 세션 스폰이 포트를 필요로 하므로 `boot()` 이전에 바인딩하고,
  실패하면 배지 없이 계속 간다(치명적이지 않다).

### 0.5 포트·토큰의 소유와 전달

`suaegi-app`이 서버를 소유하고, `suaegi-term`이 PTY를 띄운다. 크레이트 경계를 넘겨야 한다.

```rust
// suaegi-term::pty — 이미 있는 PtySpawn.env에 얹는다. 새 타입 없음.
// suaegi-app이 스폰 직전에 채운다:
//   SUAEGI_PANE_KEY    = encode_pane_key(&pane_key)   ← base64url(RFC4648, 패딩 없음)
//   SUAEGI_SPAWN_NONCE = nonce.to_string()             ← 세대 판별
//   SUAEGI_HOOK_PORT   = port.to_string()
//   SUAEGI_HOOK_TOKEN  = token        ← env로만. 설정 파일에 두면 worktree에 남는다
```
- **수명**: 서버는 앱 수명 내내 하나. 포트·토큰은 부팅 시 한 번 정해지고 바뀌지 않는다.
  세션 재시작은 같은 값을 다시 심는다.
- `suaegi-term`은 이 변수들의 **의미를 모른다** — 그냥 env다. 의존 방향이 유지된다.

### 0.6 앱 메시지

```rust
HookArrived(HookEvent),                                  // 상관관계는 PaneKey
BadgeTick,                                               // presence 폴링과 같은 티어
DiffRequested { worktree: WorktreeId, op: OpId },
DiffLoaded { worktree: WorktreeId, op: OpId, result: Result<CompareOutcome, String> },
FileDiffRequested { worktree: WorktreeId, path: String, op: OpId },
FileDiffLoaded { worktree: WorktreeId, path: String, op: OpId, result: Result<FileDiff, String> },
DiffCancelled { worktree: WorktreeId },
HydrationStep(HydrationStep),                            // 0.7
```
`HookArrived`는 **`OpId`를 갖지 않는다** — 요청에 대한 응답이 아니라 푸시다. 상관관계 키는
`PaneKey`뿐이다. `BadgeChanged`는 없다 — 배지는 리듀서에서 파생되지 전달되지 않는다.

### 0.7 하이드레이션 게이트 — 상태기계

```rust
pub enum HydrationStep { ReposListed(RepoId), SessionsResolved, LayoutBuilt }
pub struct Hydration { pending_repos: HashSet<RepoId>, sessions_resolved: bool, layout_built: bool }
impl Hydration { pub fn is_open(&self) -> bool; }   // 셋 다 끝나야 true
```
- **게이트가 닫혀 있는 동안 `persist()`는 아무것도 쓰지 않는다.** 부팅 중간 단계가 실패하면
  부분 변경된 상태가 디스크에 덮인다(Orca가 이걸로 사용자 탭을 날렸다).
- **저하된 완료도 완료다**: repo 조회가 실패해도 그 repo는 `pending`에서 빠진다 —
  게이트가 영원히 닫혀 있으면 사용자가 아무것도 저장할 수 없다.
- 게이트가 닫힌 동안의 사용자 편집은 **메모리에 남고 게이트가 열릴 때 저장된다**(거부하지 않는다).
- **`ReposListed`는 성공·실패 양쪽에서 정확히 한 번 발행된다.** `Authoritative`든 `Degraded`든
  그 repo를 `pending`에서 뺀다. **낡은 응답은 빼지 않는다** — 요청 시 발급한 `OpId`로 대조해
  현재 요청의 것만 반영한다(중복 응답이 카운터를 두 번 깎으면 게이트가 일찍 열린다).

## Task 1 — `suaegi-git` 확장 (크기 상한과 바이트 접근)

**Codex 지적**: "git 계층은 손대지 않는다"는 틀렸다. 아래가 전부 git 계층 변경이다.

- [ ] **`GitRunner`에 스트리밍 바이트 상한.** 지금은 `read_to_end`로 EOF까지 읽고 나서
  `from_utf8_lossy`한다 — **상한을 나중에 검사하면 이미 할당된 뒤다.** 읽는 도중에 세고,
  넘으면 `GitError::OutputTooLarge { limit }`를 낸다.

  **오케스트레이션을 명시한다** — 지금 구조에 그냥 끼울 수 없다. `runner.rs:96-128`이
  `try_join!(child.wait(), read_out, read_err)`인데, `child.wait()`가 `child`를 가변 대여하고
  있어 **리더가 그 안에서 `kill`을 부를 수 없다.** 그리고 리더가 그냥 에러를 반환하면
  `try_join!`이 나머지를 취소할 뿐 `wait`가 보장되지 않는다(`kill_on_drop`은 종료를 요청할 뿐
  수확이 아니다).
  → 리더는 **넘침을 바깥 상태기계에 보고만** 한다. 바깥이 조인된 future들을 떨궈 대여를 끝낸
  **뒤에** 프로세스 그룹을 죽이고, 양쪽 파이프를 배출하고, `child.wait()`를 await 한다.
  **stdout·stderr 양쪽에 대량 출력하는 자식으로 파이프 교착 테스트를 넣는다.**
  ```rust
  pub const MAX_DIFF_BYTES: usize = 6 * 1024 * 1024;   // 바이트다. 문자가 아니다
  ```
  **`String::len()`은 바이트를 센다** — 6M"자"라고 쓰면 구현자가 헷갈린다.
- [ ] **바이트를 돌려주는 프로브**. `rev`를 문자열로 두면 안 된다 — `git show <rev>:<path>`는
  **untracked 파일을 읽지 못한다**:
  ```rust
  pub enum FileSource { WorkingTree, Revision(String) }
  pub async fn file_head_bytes(runner, worktree, src: FileSource, path, cap: usize)
      -> Result<Vec<u8>, GitError>;
  ```
  `WorkingTree`는 **파일시스템에서 직접** 읽고, `Revision`은 `git show`로 읽는다.
  **앞 `cap`(=8192)바이트만** 읽는다.
  경로 검증은 **어휘적 포함만으로 부족하다** — 어휘적으로는 안에 있어도 심볼릭 링크가 밖을
  가리킬 수 있고, 열면 대상을 따라간다(git은 링크를 링크 내용으로 다룬다).
  `symlink_metadata`로 먼저 보고, **심볼릭 링크면 따라가지 않는다** — `read_link`의 대상
  바이트를 보거나 `NonRenderable`로 표시한다.
  바이너리 판정은 **앞 8192바이트에 NUL이 있는지**로 하고, `file_diff`의 lossy `String`으로는
  할 수 없다. 검사 대상 리비전:
  | 상태 | 검사 |
  |---|---|
  | `Added`/untracked | `WorkingTree` |
  | `Modified`/`Renamed`/`Copied` | `WorkingTree` |
  | `Deleted` | `Revision(merge_base)` |
  | `Other(c)` | **검사하지 않는다.** 타입 변경·미병합 등은 추측하지 말고 `NonRenderable(c)`로 |
- [ ] **타입 있는 비교 결과** — 지금은 전부 `GitError::Failed` 문자열이다:
  ```rust
  pub enum CompareOutcome { Ready(BranchCompare), NoMergeBase, UnbornHead, InvalidBase }

  /// **`file_diff`의 결과 타입도 여기서 정의한다**(구현 중 발견: 플랜 어디에도 없었다).
  /// 지금 `file_diff`는 patch `String`만 돌려주는데, 바이너리와 `Other(c)` 결과를 담을 곳이
  /// 없었다.
  pub enum FileDiff {
      Patch(String),
      Binary,                 // NUL 스니핑 결과
      TooLarge { limit: usize },
      NonRenderable(char),    // ChangeStatus::Other(c) — 추측하지 않는다
  }
  ```
  **`Message::DiffLoaded`/`FileDiffLoaded`도 Task 1이 추가한다** — 이 두 타입에 의존하므로
  Task 0에서는 쓸 수 없다(Task 0이 Task 1보다 먼저 끝나야 하는데 타입이 반대로 흐른다).
  **분류 순서를 못 박는다** — `merge-base`의 exit 1만으로는 셋을 가를 수 없다:
  1. `rev-parse --verify <base_ref>` 실패 → `InvalidBase`
  2. `rev-parse --verify HEAD` 실패 → `UnbornHead`
  3. `merge-base` 실패 → `NoMergeBase`
  4. 그 외 실패 → `GitError::Failed`(진짜 오류)
- [ ] **`-C`(복사 감지)를 켜려면 파서를 먼저 고쳐야 한다.** `--name-status -z`에서 `C`는
  `R`처럼 **경로 둘짜리 레코드**를 내는데, 지금 파서는 `R`만 두 경로로 읽고 `C`는
  `Other('C')`로 떨어뜨려 **경로를 하나만 소비한다 → 이후 모든 레코드가 밀린다.**
  `ChangeStatus::Copied { from: String }`를 추가하고 `R`과 같은 모양으로 파싱한다.
  **픽스처: 복사된 파일 뒤에 다른 변경을 하나 더 둔다**(밀림은 그래야 보인다).
- [ ] `-c core.quotePath=false`는 **`-z`만으로 비ASCII 이스케이프가 억제되는지 실측한 뒤**
  결정한다(한글 파일명) — 억제된다면 넣지 않는다.
- [ ] **취소**: `branch_compare`는 git을 **7번**(분류 프로브 2회 포함) 부르고 각각 30초라
  **최악 ~210초**다. 취소 확인 지점도 그만큼 늘어난다.
  ```rust
  pub struct CompareHandle { cancel: Arc<AtomicBool> }
  ```
  **두 층위를 구분해 정직하게 적는다:**
  - **값싼 쪽(이 플랜의 기본)**: 각 git 호출 **사이**에 확인해 남은 호출을 시작하지 않는다.
    이름을 `stop_after_current_call`로 지어 오해를 없앤다. **실행 중인 호출은 최대 30초 더 돈다.**
  - 진짜 즉시 취소가 필요하면 `GitRunner`에 취소 인지 메서드를 추가해 완료·타임아웃·취소를
    `select`로 경합시키고, **세 경로가 같은 kill-and-reap 루틴을 공유**해야 한다.
    타임아웃 kill 경로를 "재사용한다"는 말은 취소를 러너까지 내리지 않으면 **거짓이다.**

  취소는 **오류가 아니다** — 배너를 띄우지 않고 조용히 끝난다.

**테스트:** 상한 초과 시 (a) `OutputTooLarge`가 나오고 (b) **프로세스가 남지 않는지**.
NUL 스니핑을 상태별로. `NoMergeBase`가 `Failed`와 구별되는지(고아 브랜치를 만들어 실측).
취소가 남은 호출을 막는지(대조군: 취소 없으면 전부 실행).

## Task 2 — hook 서버

- [ ] **의존성 결정**(구현 중 발견: 워크스페이스에 HTTP도 base64도 없다):
  - **HTTP는 `std::net::TcpListener` 위에 직접 짠다.** 루프백 전용에 라우트 하나뿐이라
    hyper/axum은 과하고 공격 표면만 넓힌다. **엄격하게** 판정한다 — 기대한 모양과 정확히
    맞지 않으면 거절한다(관대한 파싱을 하지 않는다).
  - **base64는 `base64` 크레이트를 쓴다.** 손으로 짤 이유가 없고, 엄격 디코딩이 필요하다.
    `URL_SAFE_NO_PAD` 엔진.
- [ ] 루프백 전용 임시 포트, 헤더 토큰(없으면 403), 본문 1MB, slowloris 5s.
- [ ] `POST /hook/<source>` — `<source>`로 에이전트 종류를 구분(Codex 자리).
- [ ] **와이어 포맷을 못 박는다.** 스크립트와 서버가 같은 골든 픽스처를 쓴다.
  ```sh
  # 훅 스크립트가 내는 요청 (stdin의 JSON을 그대로 본문으로)
  curl -sS --max-time 1.5 -X POST \
    -H "X-Suaegi-Token: $SUAEGI_HOOK_TOKEN" \
    -H "X-Suaegi-Pane: $SUAEGI_PANE_KEY" \
    -H "X-Suaegi-Nonce: $SUAEGI_SPAWN_NONCE" \
    -H 'Content-Type: application/json' \
    --data-binary @- \
    "http://127.0.0.1:$SUAEGI_HOOK_PORT/hook/claude" >/dev/null 2>&1 || true
  ```
  - **본문은 Claude의 stdin JSON 그대로**(감싸지 않는다) — 페이로드가 바뀌어도 파서만 고치면 된다
  - **`pane_key`는 헤더로 온다**, 본문이 아니다. Claude가 우리 env를 본문에 넣어주지 않는다
  - **헤더 값은 base64url(RFC 4648 URL-safe, 패딩 없음)로 인코딩한다.** 패딩 유무를 정하지
    않으면 엄격 디코딩에서 양쪽이 어긋난다. 골든 픽스처에 인코딩된 값 하나를 박아둔다. `PaneKey`는 파일시스템 경로에서 나오는데, unix 경로는
    **개행과 임의 바이트를 담을 수 있다** — 날것으로 헤더에 넣으면 헤더 주입이다. 앱이
    스폰 시 `SUAEGI_PANE_KEY`에 **이미 인코딩된 값**을 심고, 서버는 엄격히 디코딩·검증한다.
    테스트: 공백, 한글, `%`, 따옴표, **개행**, 잘못된 인코딩
  - `<source>`는 경로 세그먼트(`/hook/claude`)
  - **요청 검증 표** (전부 서버는 계속 산다):

    | 조건 | 응답 |
    |---|---|
    | 토큰 없음 / 틀림 | 403 |
    | 메서드·경로·`<source>` 미지원 | 404 (메서드는 405) |
    | pane 헤더 없음 / 빈 값 / 디코딩 실패 | 400 |
    | nonce 헤더 없음 / 파싱 실패 | 400 |
    | 본문 1MB 초과 | 413 |
    | 수신 지연(slowloris 5s) | 408 |
    | JSON 파싱 실패 / `session_id` 없음 | 400 |
- [ ] **먼저 빠진 페이로드를 캡처한다.** 조사가 실측한 것은 `SessionStart`·`PreToolUse`·
  `Stop`·`PermissionRequest` 넷뿐이다. `UserPromptSubmit`(프롬프트 텍스트를 담은 **필드
  이름을 모른다** — `<task-notification>` 필터를 쓸 수 없다), `PostToolUseFailure`,
  `StopFailure`, `SubagentStop`, `SessionEnd`를 캡처해 조사 문서에 추가한 뒤 파서를 쓴다.
  **추측으로 필드 이름을 정하지 않는다.**

  **캡처 완료** — 확정된 것: `UserPromptSubmit`의 프롬프트 필드는 **`prompt`**.
  `PostToolUseFailure`는 `PostToolUse` **대신** 발화하며(둘이 같이 오지 않는다)
  `is_interrupt`로 사용자 중단과 진짜 도구 실패를 구분한다. `SessionEnd`는 `reason`을 나른다.
  `SubagentStop`의 유령(`agent_type: ""`)이 Bash만 쓴 턴에서 재확인됐다.
  `SessionStart`는 **대화형에서만** `model`을 나른다(print 모드엔 없다).
- [ ] **정규화는 순수 함수**: `parse_hook(pane: &str, nonce: &str, body: &[u8]) -> Result<HookEvent, ParseError>`.
  `iced`를 모른다. **픽스처는 조사 문서의 실측 캡처를 쓴다** — 골든 픽스처에 pane 헤더, nonce 헤더, 본문을 함께 담아 스크립트와 서버가 같은 것을 본다.
- [ ] 유계 채널로 `HookArrived` 발행(0.4).

**테스트:** 실측 페이로드 표 테스트. 토큰 없음 → 403, 본문 초과 → 거부. `agent_id` 유무로
리드/서브 구별. **채널이 가득 찼을 때 새 이벤트가 거절되고 기존 것이 남는지**(drop-newest다 — 대조군:
여유가 있으면 들어간다). 그리고 **버린 개수 카운터가 올라가는지.**

## Task 3 — 주입과 배지

- [ ] `SUAEGI_PANE_KEY`/`SPAWN_NONCE`/`HOOK_PORT`/`HOOK_TOKEN`을 `PtySpawn.env`에 심는다(0.5). pane 키는 **이미 인코딩해서** 심는다.
- [ ] **훅 스크립트**를 `dirs::config_dir()/suaegi/hooks/`에(macOS면
  `~/Library/Application Support/suaegi/hooks/`). `persistence_thread.rs:28-32`가 이미 그
  규칙을 쓴다 — `~/.suaegi`는 `config_dir()`이 없을 때의 폴백일 뿐이라, 거기 하드코딩하면
  주 개발 플랫폼에서 앱의 디스크 흔적이 두 곳으로 갈린다. 존재 가드 → `curl --max-time 1.5` →
  **항상 exit 0** → 모든 경로에서 stdin 배출. 경로는 POSIX 홑따옴표 이스케이프.
  **가드가 없으면 낡은 항목이 exit 127을 내 사용자 트랜스크립트에 모든 도구 호출마다 오류가 뜬다.**
- [ ] **worktree의 `.claude/settings.local.json`**에 쓴다(위 규칙 1). **모든 훅에 `"async": true`**. 등록: `SessionStart`,
  `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `PostToolUseFailure`, `PermissionRequest`,
  `Stop`, **`StopFailure`**, `SessionEnd`. `Notification`은 등록하지 않는다(6초 늦다).
- [ ] **이벤트 → 상태 매핑** (실측):
  - `UserPromptSubmit` → working. **`<task-notification>` 필터는 이 플랜에서 넣지 않는다.**

    이유 둘. (1) 배지 관점에서 **결과가 같다** — 서브에이전트 완료를 리드가 처리하는 중이므로
    `working`이 맞다. 필터의 원래 동기는 "사람이 치지도 않은 프롬프트로 보인다"였는데,
    이 플랜은 프롬프트 텍스트를 표시하지 않는다. (2) **접두사를 아직 실측하지 못했다** —
    비동기 Agent 실행이 리드의 `Stop` 이후에 끝나야 재현되는데 그 조건을 못 만들었다.
    측정 안 된 문자열로 `starts_with`를 쓰면 **필터가 영영 발화하지 않는다**(이번 플랜에서
    같은 모양의 버그를 이미 한 번 잡았다). 툴팁에 프롬프트를 띄우게 되면 그때 측정하고 넣는다.
  - `PreToolUse`/`PostToolUse`/`PostToolUseFailure` → working
  - `PermissionRequest` → **waiting**. `AskUserQuestion` 특수 처리 없음(실측: 자동 허용이 아니다)
  - `Stop` + `background_tasks_empty == Some(true)` → done. 그 외 → **working 유지**
  - **`StopFailure` → done, 무조건.** `background_tasks`를 보지 않는다.

    **실측: `StopFailure` 페이로드에 `background_tasks`가 아예 없다**(`session_crons`,
    `permission_mode`도 없다 — `Stop`보다 훨씬 얇다). 0.2의 보수적 규칙
    (`None` → "비지 않음")과 합치면 **`StopFailure`는 영원히 `done`이 될 수 없고 pane이
    계속 돈다** — `StopFailure`를 등록한 이유가 정확히 그 무한 스피너를 막는 것이었는데
    그 자체가 원인이 된다. 필드가 가끔 빠지는 게 아니라 **구조적으로 없으므로** 보수적
    기본값이 이 이벤트에는 틀린다.
    턴이 오류로 끝났고 리드는 새 사용자 입력 없이는 더 진행하지 않는다 — 이 UI에서
    그것이 `done`의 의미다.
  - `SubagentStop` → 무시(유령 이벤트가 온다 — 본 적 없는 id, `agent_type: ""`)
  - `SessionStart` → **바인딩만 하고 배지는 `Unknown`.** 아직 아무 일도 안 일어났다
  - `SessionEnd` → **무시한다.** 종료 판정은 presence 폴링의 `Exited`/`NoAgent`가 권위다
    (훅은 async라 프로세스 사망 시 아예 안 올 수도 있다 — 폴링만이 그걸 본다)
- [ ] `reduce`를 배지 UI에 배선. `last_assistant_message`는 **이번 범위 밖**(툴팁은 나중에).
- [ ] **신뢰 대화상자**: 이 플랜에서는 감지를 요구하지 않는다. 배지는 0.3 표의
  **`Agent(_)` + 훅 없음 → `Unknown`** 행으로 자연히 처리된다.

  **"`SessionStart` 이후 침묵"이라는 휴리스틱을 만들지 말 것** — 그런 상태는 존재할 수 없다.
  실측 결과 신뢰 전에는 **`SessionStart`조차 오지 않는다**(주입 방식과 무관하며 `--settings`
  대조군도 동일하다). 결과는 같지만 경로가 다르므로, 근거를 잘못 알고 있으면 절대 발화하지
  않을 감지 로직을 짜게 된다.

  **감지·사전 신뢰 심기는 follow-up으로 기록한다** — 신뢰 상태는
  `$CLAUDE_CONFIG_DIR/.claude.json`의 `projects[<경로>].hasTrustDialogAccepted`에 있고,
  **알려진 유일한 심기 방법이 Global Constraint #1을 위반한다**는 사실까지 함께 적는다.

**테스트:** `reduce` 전 조합 표 테스트(0.3의 표를 그대로). 특히 `Exited`가 이기는지,
`Waiting`이 감쇠하지 않는지, `Stop` + 비지 않은 background가 working을 유지하는지,
`<task-notification>` 필터, `no_agent_streak` 경계(2 vs 3). 주입 JSON 생성과 스크립트
이스케이프는 순수 함수 → 표 테스트(경로에 따옴표·공백).

## Task 4 — diff 패널 UI

- [ ] **셋째 영역**: `lib.rs`의 2열 `row!`에 **오른쪽 패널**로 붙인다.
  - 기본 **닫힘**. 사이드바의 worktree 행에 토글 버튼.
  - 열리면 고정 폭(리사이즈는 범위 밖 — pane_grid가 아니라 `row!`의 단순 분할이다).
  - 상태: `Closed | Loading | Ready(list) | Empty | Failed(msg) | NoMergeBase | TooLarge`.
  - 파일을 고르면 같은 패널 하단에 patch. **한 번에 한 파일.**
  - 닫으면 진행 중인 compare를 **취소**한다(Task 1의 핸들).
- [ ] `git_tasks.rs`에 `branch_compare`/`file_diff` 래퍼, `Task::perform`, **`OpId` 가드**.
- [ ] 렌더링은 patch 텍스트에 줄 선두(`+`/`-`/` `/`@`)로 색만. **Orca를 따라 하지 않는다** —
  Monaco가 없는 우리에겐 없는 diff 알고리즘을 새로 쓰는 셈이라 **더 비싸다.**
- [ ] base ref는 **repo의 기본 브랜치 하나로 고정**한다(선택 UI는 범위 밖).

**테스트:** 상태 전이(닫힘→로딩→준비/실패/과대). `OpId` staleness(오래된 결과가 새 것을
덮지 않는지, 대조군 포함). 닫기가 취소를 부르는지. **실제 픽셀은 검증 불가 — 명시한다.**

## Task 5 — 레이아웃 복원

- [ ] `PersistedPane`을 `SessionState`에 `#[serde(default)]`로 추가.
  **`SCHEMA_VERSION`을 올리지 않는다**(가드가 `>` 비교라 범프하면 구버전이 저장을 거부한다).
- [ ] 저장: `State::layout()`을 걸으며 잎을 `WorktreeId`로 치환.
  `ratio`는 **0.5에서 0.005 넘게 벗어날 때만 소수 3자리로**(float 잡음이 저장을 흔든다).
- [ ] **저장 트리거를 명시한다**: pane 열기/닫기, 분할 리사이즈, 포커스 변경, 복원 완료.
  maximize는 저장하지 않는다(범위 밖).

  ```rust
  pub const LAYOUT_SAVE_DEBOUNCE: Duration = Duration::from_millis(400);
  Message::LayoutPersistDue { generation: u64 }   // 최신 세대만 저장한다
  ```
  리사이즈마다 세대를 올리고 타이머를 건다. **하이드레이션 게이트가 닫힌 동안 타이머가
  터지면 저장하지 않고**, 게이트가 열리는 시점에 현재 스냅샷을 한 번 저장한다.

  **"드래그 종료 시"라는 이벤트는 iced에 없다.** `pane_grid::ResizeEvent`는 `split`과 `ratio`만
  나르고 단계 표시가 없다(`iced_widget-0.14.2/src/pane_grid.rs:1228-1238`) — `on_resize`는
  드래그 **중에도** 계속 발화한다. 그러니 **디바운스로 간다**: 리사이즈 메시지가 온 뒤
  400ms 동안 추가 메시지가 없으면 그때 저장한다. 있지도 않은 단계 이벤트를
  찾으라고 지시하지 않는다.
- [ ] **복원 배리어 — 실패한 시작의 정의**:
  잎마다 종단 결과가 하나 온다: `Started | Failed | WorktreeGone`.
  **재귀로 정의한다** — "형제가 부모 자리를 차지"만으로는 **양쪽이 다 실패한 분할**을 정의하지
  못한다(중첩 서브트리가 통째로 비는 경우도 같다):
  ```rust
  fn restore(node: &PersistedPane) -> Option<Configuration<SessionId>>
  //  Leaf, 시작 성공        -> Some(Pane)
  //  Leaf, Failed/Gone      -> None
  //  Split(Some(a), Some(b)) -> Some(Split)
  //  Split(Some(x), None)    -> Some(x)      // 형제 승격
  //  Split(None, Some(x))    -> Some(x)
  //  Split(None, None)       -> None          // 서브트리 전체 소멸
  ```
  루트가 `None`이면 빈 워크벤치(기본 상태). 재시도는 하지 않는다.
  **테스트: 직계 자식 둘 다 실패, 그리고 중첩 서브트리 둘 다 비는 경우.**
  **부분 복원을 허용한다.** 전부 실패해야 포기하는 게 아니고, 하나 실패했다고 전체를 버리지도
  않는다. 재시도는 하지 않는다(사용자가 다시 열면 된다).
- [ ] `active_worktree_id`를 **부팅 시 읽는다**(지금은 쓰기만 한다).
- [ ] **복원 전에 트리를 검증한다.** "worktree 하나에 pane 하나"는 대화형 경로에서만 성립하고,
  **디스크의 JSON은 손상·수동 편집·구버전 버그로 같은 `WorktreeId`를 여러 잎에 담을 수 있다.**
  그러면 `PaneKey`가 중복돼 훅 라우팅이 모호해진다. **순회 순서상 첫 등장만 남기고 이후 중복은
  접는다**(위 `restore`의 `None`과 같은 처리). 테스트로 고정한다.

**삭제 판정에 증거를 요구한다:**
- [ ] `WorktreesListed`가 **출처를 나른다**: `Authoritative(Vec<WorktreeEntry>) | Degraded`.
  `apply_worktree_listing`은 **`Authoritative`만 받아 정리한다.** 지금은 성공한 빈 목록과
  저하된 조회를 구별할 수 없고, **레이아웃 복원이 붙으면 실패한 스캔 한 번이 복원된 레이아웃
  전체를 지운다.**

**테스트:** 트리 왕복(중첩 분할·비대칭 ratio·단일 pane). 배리어: 잎 하나가 `Failed`일 때
나머지가 살아남고 트리가 접히는지(대조군: 전부 성공하면 원형 그대로). 게이트: 중간 단계
실패 시 저장이 **일어나지 않는지**(대조군: 열리면 저장된다). 삭제: `Degraded`가 pane을
지우지 **않는지**(대조군: `Authoritative`의 빈 목록은 지운다).

## Task 6 — worktree 메타데이터 (follow-ups #15)

- [ ] `AppState`에 `HashMap<WorktreeId, WorktreeMeta>`. (1) `from_load`에서 시드,
  (2) **생성 시점**(`WorktreeCreated`가 지금 `Ok(_created)`를 버린다)에 기록,
  (3) `persisted_snapshot`이 자리표시자 대신 읽는다.
- [ ] **`created_with_agent`는 `None`으로 둔다.** 채울 소스가 없다(에이전트 선택 UI가 범위 밖).
  **가짜로 채우지 않는다.**

---

## 범위 밖

- **Codex 배지.** 훅은 있고(이 기계에서 켜져 있다) 스키마도 같지만, **트러스트 해시**와
  **`CODEX_HOME` 미러링**이 필요해 Claude보다 훨씬 무겁다. 인제스트 계층을 에이전트 무관하게
  만들어 자리만 남기고, 구현은 다음으로.
- 스크롤백 영속화, SSH 원격, PTY 생존 데몬, OSC 프로토콜, i18n (스펙이 post-MVP로 명시)
- follow-ups #20(종료 시 UI 스레드 drop), #21·#27(unwind 안전성), #25(rustfmt 관례),
  #26(위젯 밖 마우스 clamp), #8(Windows `claude.exe`)
