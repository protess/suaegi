# Suaegi Plan 4: 터미널 커스텀 위젯 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 워크벤치의 터미널이 **진짜 터미널**이 된다. 색과 커서가 보이고, 타이핑이 셸에 들어가고, 포커스가 오가고, pane 크기에 맞춰 리사이즈되고, 마우스로 선택·스크롤하고, 마우스를 쓰는 TUI(vim, htop)가 동작한다.

**Architecture:** `iced_core::Widget`를 직접 구현하는 커스텀 위젯.
**위젯은 `TerminalSession`을 절대 만지지 않는다.** 스냅샷을 읽어 그리고, 입력을 `TermCommand`로 번역해 `shell.publish`한다. 세션을 만지는 것은 앱의 `update`뿐이다. 이 경계가 테스트 가능성의 근거다.

**Tech Stack:** iced 0.14 (features: tokio, canvas, advanced, lazy), alacritty_terminal 0.25.1, 기존 4개 크레이트.

**Spec:** `docs/superpowers/specs/2026-07-20-suaegi-mvp-design.md`
**조사:** `docs/superpowers/research/2026-07-21-plan4-terminal-widget.md` — **구현 전 필독.** 아래 제약은 전부 그 문서에 `file:line` 근거가 있다.
**선행:** Plan 1~3 머지 완료. `follow-ups.md` 22번(pty_test 플레이키)은 이 플랜 **시작 전에** 별도로 처리한다.

---

## Global Constraints

### 이 플랜이 뒤집는 Plan 3의 전제

1. **`scrollable`을 제거한다**(`workbench.rs:75`). 터미널이 스크롤백을 직접 소유한다.

   **다만 원래 적었던 근거는 우리 구성에서 성립하지 않는다**(구현 중 mutation으로 확인).
   "첫 스크롤 후 트랜잭션이 열려 휠이 자식에게 전달되지 않는다"는 기제 자체는 실재하지만
   (`pane_grid_behavior.rs`가 5000px 프로브로 고정해 뒀다), **트랜잭션은 실제 스크롤이
   일어나야 열린다.** 터미널 위젯은 `Length::Fill`이라 콘텐츠가 뷰포트와 정확히 같고,
   넘치는 게 없으니 스크롤이 일어나지 않고, 따라서 트랜잭션도 열리지 않는다 —
   `scrollable`을 다시 씌워도 휠은 매번 통과한다(M35가 살아남아 이걸 드러냈다).

   그래도 제거가 옳다: **죽은 레이어**이고, 콘텐츠가 넘치기 시작하는 순간 삼키기 시작한다.
   단 "이 테스트가 제거를 지킨다"고 말할 수는 없다 — 지키지 못한다.
2. **`.spacing(2).on_resize(8, ..)` → `.spacing(4).on_resize(0, ..)`.** 본문 침범량은 `leeway/2`이고 `spacing`과 무관하다. `leeway = 0`이면 리사이즈 밴드가 거터와 정확히 일치해 침범이 0이 된다(실측 확인).
3. **`TitleBar`는 하중을 받는 부재다.** 본문 드래그가 pane을 옮기지 못하게 막는 **유일한** 기제다. `on_drag`를 타이틀바 없는 `Content`와 짝지으면 안 된다. 생성부에 이유를 주석으로 남긴다.

### 절대 규칙

- **`capture_event()`로 조상·형제를 막을 수 있다고 가정하지 않는다.** 캡처는 단락이 아니라 플래그다. 호출은 하되 **설계가 그것 없이도 옳아야 한다.**
- **포커스 게이팅과 bounds 필터링은 우리 책임이다.** `Widget::update`는 포커스와 무관하게 모든 키 이벤트를 받고, `pane_grid`는 커서가 근처에도 없는 pane에까지 이벤트를 뿌린다.
- **위젯은 세션을 만지지 않는다.** 위젯이 `TerminalSession`을 부르는 순간 이 플랜의 테스트 전략이 무너진다.
- **`write()`의 `bool`을 무시하지 않는다.** `false`는 "PTY에 못 썼다"가 아니라 **"큐(상한 256)에 못 넣었다 = 입력 유실"**이다.
- **`Text`에 `Default`가 없다.** 9개 필드 전부 명시.
- **`Modifiers::control()`을 쓴다.** `command()`는 macOS에서 Cmd다.
- edition 2021, `rust-version = "1.94"`. 파일/모듈 이름에 `utils`/`helpers` 금지.

### 검증 규칙

**헤드리스 위젯 테스트가 가능하다.** `impl Renderer for ()` + `Shell::new` + `clipboard` 페이크 + `Tree::new`로 창·GPU 없이 `Widget::update`에 합성 이벤트를 흘린다. 합성 OS 클릭이 아니므로 금지 규칙에 걸리지 않는다.

**그러나 null 렌더러의 한계가 분명하다** (Codex 교차검증에서 확인):

```
impl text::Paragraph for () {           // iced_core/src/renderer/null.rs
    fn with_text(_: Text<&str>) -> Self {}
    fn compare(&self, _: Text<()>) -> Difference { Difference::None }   // 항상 None
    // min_bounds() 등도 무의미한 값
}
```

→ **텍스트 측정·셰이핑·캐시 무효화는 이 하네스로 검증할 수 없다.**
→ 대응: **측정에 의존하는 계산을 순수 함수로 뽑고 `CellMetrics`를 인자로 받는다.**
그 순수 함수를 실제 값으로 테스트하고, null 하네스는 **이벤트/메시지 배선에만** 쓴다.
이 분리는 선택이 아니라 **테스트 가능성의 전제**다.

- **`TermCommand`는 `PartialEq`를 파생할 수 없다.** `alacritty_terminal::grid::Scroll`이
  `Debug, Copy, Clone`만 파생한다(`grid/mod.rs:72`). 헤드리스 테스트가 발행 커맨드를 비교할 때는
  `matches!`와 필드 분해를 쓴다. **`assert_eq!`로 커맨드 목록을 통째로 비교하려다 막히면
  래퍼 타입을 만들지 말고** 이 방식을 쓴다 — Task 0 구현 중 발견.
- **모든 테스트는 mutation 검증한다.** 구현(또는 기대값)을 바꿔 테스트가 **실제로 FAIL하는 것**을 확인하고 보고한다. 이 저장소에서 공허한 테스트가 5번 나왔다.
- **대조군 없이 "아무 일도 안 일어났다"를 단언하지 않는다.** 비교 대상을 같이 단언한다.
- **시계에 의존하는 것을 mutation 검증했다고 주장하지 않는다.** `mouse::Click::new`는 내부에서 `Instant::now()`를 읽고 300ms/6px로 분류한다(`iced_core/src/mouse/click.rs:43-90`). 하네스가 시계를 주입할 수 없다 → **우리 소유의 순수 분류기를 만들고 그것을 테스트한다**(Task 6).
- 검증할 수 없는 것을 "확인했다"고 쓰지 않는다. 사람 눈이 필요한 것은 그렇다고 적고 follow-ups로 넘긴다.

### 성능은 실측 후 결정 (추측 금지)

~~행당 `with_spans`가 이론적으로 최선~~ → **측정했고, 틀렸다. 셀당 `fill_text`를 쓴다.**

| 경로 | 24×80 | 50×200 |
|---|---|---|
| A. 행당 `with_spans`(재구축) | 1.02ms | **4.70ms** |
| B. 셀당 `fill_text`(캐시됨) | 55µs | **261µs** |

**A가 18배 느리다.** 200×50에서 4.70ms면 pane 하나가 16ms 예산의 28%를 먹는다.

이유가 두 겹이다. (1) `with_spans`가 `Shaping::Advanced`를 강제한다
(`iced_graphics/src/text/paragraph.rs:161`) — 매 행 매 프레임 전체 리치텍스트 셰이핑.
(2) **A는 캐시할 수 없고 이건 구조적이다**: `Widget::draw`가 `&State`(불변)를 받아
만든 `Paragraph`를 되쓸 수 없고, 동결된 위젯 상태에 paragraph 캐시 필드가 없다.

그리고 **B는 "셀당 셰이핑"이 아니다.** `fill_text`는 `Text::Cached`를 레이어에 넣고
prepare가 내용을 해시해 셰이핑된 버퍼를 재사용한다(`iced_graphics/src/text/cache.rs:29-42`).
터미널 셀은 문자 하나라 히트율이 사실상 100%이고, B의 실제 비용은 String 할당 + 해시 +
맵 조회다. 벤치가 프레임 간에 진짜 `Cache`를 물려 이걸 쟀다 — B를 "셀당 `with_text`"로
쟀다면 캐시가 항상 빗나가는 세계의 숫자가 나왔을 것이다.

측정 못 한 것: GPU 업로드, 아틀라스, draw call 제출(창·GPU 없음). A의 draw call 수
이점은 여기 안 잡힌다 — 다만 B의 55µs/261µs면 그게 뒤집을 여지가 없다.

---

## Task 0 — 계층과 계약 확정 (컴파일되는 산출물)

**산출물은 산문이 아니라 컴파일되는 타입 선언이다.** 아래 결정은 이미 내려져 있다 —
구현자가 고를 것이 남아 있으면 그건 Task 0의 실패다.

### 0.1 계층 규칙 (이 플랜에서 가장 중요한 절)

의존 방향은 `suaegi-app → suaegi-term` **단방향**이다(저장소 규칙). 따라서:

| 무엇 | 어디 | 왜 |
|------|------|-----|
| 터미널 프로토콜 타입(`KeyInput`, `MouseIntent`, `ViewportHit`, `TermKey`, `Mods`) | **`suaegi-term`** | 인코더가 이걸 받는다. app에 두면 순환 의존이다 |
| 순수 인코더(`encode_*`, `route_mouse`) | **`suaegi-term::encode`** | `(입력, TermMode) → 바이트`. 표 테스트가 여기 있다 |
| term 락을 잡는 intent 메서드 | **`TerminalGrid`** | `FairMutex<Term>`가 `TerminalGrid`의 **비공개** 필드다(`grid.rs:140-145`). 세션은 `Arc<TerminalGrid>`만 들고 있어 직접 못 잡는다 |
| 쓰기 큐 | **`TerminalSession`** | 큐를 grid에 넣으면 락 중첩이 생긴다. **grid가 바이트를 돌려주고 session이 큐에 넣는다** |
| iced 이벤트 → 프로토콜 타입 변환 | **`suaegi-app`** | `suaegi-term`은 iced에 의존하지 않는다 |
| 위젯, `TermCommand` | **`suaegi-app`** | |

- [ ] **`suaegi-term`에 iced 의존을 추가하지 않는다.** 프로토콜 타입을 **구체적으로** 정의한다 —
  이름만 정하면 `encode_key`를 컴파일할 수 없다:
  ```rust
  // suaegi-term/src/input_types.rs
  pub enum TermKey {
      Char(char),                     // 유니코드 스칼라가 **정확히 하나**일 때만. 아니면 Unknown
      Named(NamedKey),                // 아래 목록이 전부다
      Unknown,                        // 매핑 없음 — 인코더가 None을 돌려준다
  }
  pub enum NamedKey {
      Enter, Tab, Space, Backspace, Escape, Delete, Insert,
      ArrowUp, ArrowDown, ArrowLeft, ArrowRight,
      Home, End, PageUp, PageDown,
      F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12,
  }
  pub enum KeyLocation { Standard, Numpad, Left, Right }   // APP_KEYPAD 분기에 필요
  pub struct Mods { pub shift: bool, pub ctrl: bool, pub alt: bool, pub logo: bool }

  pub struct KeyInput {
      pub key: TermKey,               // iced의 `key`에서
      pub physical_latin: Option<char>, // iced의 `key.to_latin(physical_key)` 결과 — 제어 조회 전용
      pub location: KeyLocation,
      pub mods: Mods,
      pub text: Option<String>,       // iced의 SmolStr을 소유 String으로
      pub repeat: bool,
  }
  pub enum TermMouseButton { Left, Middle, Right }

  // --- 마우스 ---
  pub struct ViewportHit { pub row: usize, pub col: usize, pub side: Side }  // Side는 alacritty 것
  pub enum ClickKind { Single, Double, Triple }
  pub enum MouseAction {
      Press(TermMouseButton),
      Release(TermMouseButton),
      Motion,
      Wheel { lines: i32 },        // 부호 있음. 양수 = 위로. 위젯이 누산해 정수 줄로 만든 값
  }
  pub struct MouseIntent {
      pub action: MouseAction,
      pub hit: ViewportHit,
      pub held: Option<TermMouseButton>,
      pub mods: Mods,
      pub click: ClickKind,
      pub force_local: bool,       // Shift 오버라이드. 모드와 무관하므로 위젯이 판단
  }
  pub enum MouseRoute { Report, LocalSelect(SelectionType), LocalScroll, AltScreenArrows, Ignore }

  // grid 내부 — press에서 래치, release에서 해제
  struct PointerLatch { button: TermMouseButton, route: MouseRoute }
  ```
  **`modified_key`는 나르지 않는다** — `text`가 이미 수식자 적용 결과를 담고, 제어 조회는
  `physical_latin`이 담당한다. 셋 다 나르면 어느 것이 권위인지가 흐려진다(`iced_term`이
  캐시된 수식자와 이벤트 수식자를 섞어 쓰다 버그를 만든 것과 같은 종류의 실수다).
  **`Unknown`이 있는 이유**: `NamedKey` 목록에 없는 키(미디어 키 등)를 조용히 다른 키로
  오인하지 않기 위해서다. 인코더는 `Unknown`에 대해 `None`을 돌려준다.
  **다중 스칼라 규칙**: iced의 논리 키는 문자열이다. 스칼라가 0개이거나 2개 이상이면
  `TermKey::Unknown`으로 두되 **`text`는 보존한다** — 조합 문자는 3번 우선순위(`text`)로
  흘러가 정상 입력된다. 변환 테스트에 조합/다중 스칼라 케이스를 넣는다.
- [ ] **iced → 프로토콜 변환은 앱에 있고 그 자체가 표 테스트 대상이다**
  (명명 키 매핑 전수, `to_latin` 폴백, 수식자 비트, `Location::Numpad`).

### 0.2 `TermCommand` (위젯 → 앱)

```rust
pub enum TermCommand {
    Key(KeyInput),                       // suaegi-term 타입
    Paste(String),
    Mouse(MouseIntent),                  // 라우팅 판단은 세션이
    Resize { rows: u16, cols: u16, seq: u64 },
    Scroll(Scroll),                      // 위젯이 로컬 스크롤로 확정한 경우만
    CopySelection { to: CopyTargets },
}
```
`CopyTargets`는 **`suaegi-term`이 소유**한다(iced-free). `CopyRequest`가 그걸 담고
`CopyRequest`는 `suaegi-term` 타입이므로, app 쪽에 두면 역방향 의존이 생긴다:
```rust
// suaegi-term
pub struct CopyTargets { pub standard: bool, pub primary: bool }
```

**`Select*` 변형이 없다.** 선택은 마우스 처리의 결과이지 별도 커맨드가 아니다 — 아래 0.4.
**epoch가 위젯 커맨드에 없다.** 위젯은 epoch를 알 수 없다(세션이 할당한다).
**`InputDropped`가 없다.** 유실은 앱이 실행한 **뒤에** 아는 것이므로 위젯이 발행할 수 없다 —
앱 레벨 메시지/상태 전이다.

- [ ] **라우팅**: 위젯이 `SessionId`를 들고 `(SessionId, TermCommand)`를 발행한다.

### 0.3 인코딩 위치 규칙 (한 문장)

*터미널 프로토콜 인코딩과 그에 딸린 모든 판단은 `TerminalGrid`가 **term 락을 쥔 채** 한다.*

근거 — **어떤 모드 캐시도 correctness에 쓸 수 없다.** 스냅샷은 비동기라 낡고,
**원자 미러도 안 된다**: `feed()`가 읽기 청크 **전체**(최대 64KiB)를 락 쥔 채 처리하므로
(`grid.rs:166-170`) 청크 중간에 `BRACKETED_PASTE`가 켜져도 미러는 청크가 끝나야 갱신되고,
갱신 직전에 리더가 스케줄 아웃되면 창이 더 벌어진다. 그 창에서 인코딩하면
**개행이 든 붙여넣기가 그대로 실행된다** — 이 플랜의 하드 요구사항이 막으려는 실패다.
같은 문제가 `APP_CURSOR`·`APP_KEYPAD`·`LINE_FEED_NEW_LINE`·마우스 라우팅에 전부 걸린다.

→ **원자 모드 미러를 만들지 않는다.** 만들면 누군가 반드시 correctness 경로에 쓴다.
→ **스냅샷의 `mode`는 렌더링 전용이다.** 입력 경로가 읽으면 리뷰에서 반려한다.

### 0.4 마우스 — 라우팅·변환·변경을 한 락 안에서

**이것이 4·5라운드에서 두 번 재발한 레이스의 근본 수정이다.** 라우팅만 락 안에서 하고
결과를 앱에 돌려줘 앱이 다시 `Select*`를 부르면, **두 번째 락에서 `display_offset`이 이미
달라져 있을 수 있다** — 좌표 변환과 변경이 갈리는 바로 그 문제가 마우스 경로로 옮겨온 것이다.

```rust
// TerminalGrid — 한 번의 락 안에서 모드 읽기 → 라우팅 → 좌표 변환 → 변경/인코딩까지 끝낸다
pub fn handle_mouse(&self, intent: &MouseIntent) -> Result<GridMouseResult, MouseEncodeError>;

pub struct GridMouseResult {                // grid → session (내부)
    pub bytes: Option<Vec<u8>>,             // 아직 큐에 안 들어갔다
    pub redraw: bool,
    pub copy: Option<CopyRequest>,
}

pub struct MouseResult {                    // session → app (공개)
    pub write: WriteOutcome,                // 큐잉 결과. bytes를 그대로 노출하지 않는다
    pub redraw: bool,
    pub copy: Option<CopyRequest>,
}
pub struct CopyRequest { pub epoch: u64, pub to: CopyTargets }
```
**두 타입을 나누는 이유**: grid가 바이트를 돌려주고 session이 큐에 넣는데, 공개 타입이
`bytes`를 그대로 들고 있으면 **큐에 실제로 들어갔는지를 앱이 알 수 없다.** 그러면 마우스
입력 유실이 "보이는 피드백" 규칙을 빠져나간다.

- [ ] **포인터 라우팅 상태는 `TerminalGrid`가 소유한다.** press 시점에 로컬/리포트를 **래치**하고
  release까지 유지한다. 드래그 도중 모드가 바뀌어도 한 제스처가 반으로 갈리지 않는다.
  **위젯은 라우팅을 모른다** — `MouseResult`는 위젯의 `update`가 끝난 뒤 앱에 반환되므로
  위젯이 그걸 보고 자기 상태를 유지할 방법이 없다(5라운드 지적). 위젯은 **원시 사실만** 든다:
  눌린 버튼, 커서 위치, 클릭 분류.
- [ ] **휠은 래치에 참여하지 않는다.** 래치는 **눌린 포인터 버튼의 press/motion/release에만**
  적용한다. 휠은 드래그 중이라도 **매번 라이브 모드로 독립 판정**한다 — 드래그 중 휠을 굴리는
  TUI(예: 선택 중 스크롤)가 래치 때문에 리포트를 못 받으면 안 된다. 테스트로 고정한다.
- [ ] **순수 라우팅 함수**: `route_mouse(&MouseIntent, TermMode) -> Result<MouseRoute, MouseEncodeError>`
  (`MouseRoute::{Report, LocalSelect, LocalScroll, AltScreenArrows, Ignore}`). 락을 쥔 핸들러가
  이 결과를 실행한다. **모드×Shift×액션 전 조합을 표 테스트한다.**
- [ ] `MouseEncodeError`는 **억제와 다르게 취급한다** — 로그를 남기고 디버그 빌드에서 단언한다.
  조용히 버리면 상태기계 버그가 정상 억제로 위장된다. 통합 테스트로 이 구분을 고정한다.

### 0.5 선택 epoch — `suaegi-term` 내부에 가둔다

비동기 추출이 **나중에 시작된 선택을 덮어쓰는** 것을 막는다. 시퀀스만으로는 부족하다 —
오래된 워커가 락을 잡고 변경까지 해버리면 완료 시점 가드는 클립보드 출력만 막는다.

- [ ] **epoch는 "제스처 ID"가 아니라 "선택 버전"이다.** 제스처 ID로는 부족하다 —
  **alacritty 자신이 선택을 바꾼다**: `feed`(출력), 스크롤, 리사이즈 중에 `Term::selection`을
  회전·변경·삭제한다(`term/mod.rs:682-686, 733, 752, 778, 1657, 1773-1811, 1847`).
  제스처마다만 올리면, `CopyRequest` 생성과 추출 사이에 셸 출력이 선택을 바꿔도 epoch가 같아
  **다른 내용을 복사하거나 빈 내용을 복사한다.**
  → `GridState`에 `last_seen_selection: Option<SelectionRange>`를 두고, **락을 쥐는 모든 연산
  (`feed`, `scroll`, `resize`, `handle_mouse`)의 끝에서** 헬퍼를 부른다:
  ```rust
  fn bump_if_selection_changed(state: &mut GridState);   // 범위가 달라졌으면 epoch += 1
  ```
  선택을 **변경할 수 있는 모든 경로가 한 곳을 지나므로** 빠뜨리기 어렵다.
  **범위 비교만으로는 부족하다**: 범위가 그대로여도 **그 안의 셀 내용을 출력이 덮어쓸 수 있다**.
  따라서 `feed` 뒤에는 **선택이 존재하기만 하면 범위 변화와 무관하게 무조건 버전을 올린다**
  (보수적). 범위 동등 비교는 마우스 갱신과 구조적 변화(회전·클리핑·삭제)에만 쓴다.
  테스트: 범위가 그대로인 채 출력이 선택 안의 텍스트를 바꾼 뒤 추출하면 `None`이어야 한다.
  **위젯도 앱도 epoch를 할당하지 않는다.**
- [ ] **드래그 중 명시적 복사**: `request_copy`는 **로컬 포인터 래치가 살아 있으면 `None`**을
  돌려준다. 아직 만드는 중인 선택을 복사하면, 워커가 추출하기 전에 다음 move가 범위를 바꿔
  **단축키를 누른 시점보다 나중의 범위**가 복사된다. 만들기가 끝난 뒤에 복사하게 한다.
- [ ] `handle_mouse`가 복사를 요청할 때 `CopyRequest`에 **현재 epoch를 실어 돌려준다.**
- [ ] 앱은 그 `CopyRequest`를 워커로 보내고, 워커는
  `extract_selection(epoch) -> Option<String>`을 부른다. 이 메서드가 **락을 잡은 뒤 변경 전에**
  epoch를 비교하고, 불일치면 **아무것도 하지 않고 `None`**을 돌려준다.
- [ ] **명시적 복사(`TermCommand::CopySelection`)의 epoch 획득 경로**: 앱은 현재 epoch를 모르고,
  따로 노출해 읽게 하면 read-then-use 레이스가 다시 생긴다. 동기 API를 둔다:
  ```rust
  pub fn request_copy(&self, to: CopyTargets) -> Option<CopyRequest>;   // 락 안에서 현재 epoch를 읽는다
  ```
  선택이 없으면 `None`. 앱은 받은 `CopyRequest`를 드래그 완료 때와 **똑같이** 워커로 보낸다.
  → 복사 경로가 하나로 합쳐진다.

### 0.6 세션 API (인코딩은 grid에, 큐잉은 session에)

```rust
pub enum WriteOutcome { Queued, Dropped, Suppressed }   // Suppressed = 모드상 보낼 것 없음

impl TerminalSession {
    pub fn send_key(&self, input: &KeyInput) -> WriteOutcome;
    pub fn send_paste(&self, text: &str) -> WriteOutcome;
    pub fn send_mouse(&self, intent: &MouseIntent) -> Result<MouseResult, MouseEncodeError>;
    pub fn report_focus(&self, focused: bool) -> WriteOutcome;
    pub fn extract_selection(&self, epoch: u64) -> Option<String>;
    pub fn request_copy(&self, to: CopyTargets) -> Option<CopyRequest>;
}
```
**앱은 `TerminalGrid`에 접근할 수 없다** — grid 메서드는 전부 세션 래퍼를 통해서만 닿는다.
`request_copy`가 세션에 없으면 `TermCommand::CopySelection`을 실행할 경로가 아예 없다.

각각 **grid의 intent 메서드를 불러 바이트를 받고**(락은 그 안에서 잡고 놓는다),
**그 다음** 쓰기 큐에 넣는다. **락 중첩이 없다.**
`WriteOutcome`이 세 결과를 구분하는 이유: `bool`이면 "모드 꺼짐"과 "큐가 차서 유실"이 같은
`false`로 뭉개져 유실 피드백 규칙을 어긴다.

- [ ] **`redraw`면 generation을 올린다.** `send_mouse`는 `GridMouseResult.redraw`가 `true`일 때
  grid 호출 **뒤에** generation을 bump해야 한다. 스냅샷 스케줄링이 generation으로 돌아가므로
  (Plan 3), 다시 그리라고만 하면 **옛 스냅샷을 옛 선택으로 다시 그린다.** 선택 변경을 화면에
  반영하려면 **새 스냅샷을 찍어야** 한다.

### 0.7 클립보드 소유권 (한 문장)

*읽기는 위젯이, 클립보드 쓰기는 앱이, 프로토콜 인코딩(bracketed paste)은 `suaegi-term`이 한다.*

- **붙여넣기**: 위젯의 `update`는 `&mut dyn Clipboard`를 이미 받는다. 위젯이 읽어 **원문 그대로**
  `TermCommand::Paste(String)`으로 내보낸다. `send_paste`가 락 안에서 `encode_paste`를 부른다.
- **복사**: 앱이 `extract_selection`으로 받은 텍스트를 `CopyTargets`대로 쓴다.
- 기본값: 명시적 복사는 `{ standard: true, primary: true }`, 드래그 완료는
  `{ standard: false, primary: true }`(X11/Wayland 중클릭 관례).

### 0.8 스레딩 정책 표

| 커맨드 | 실행 | 근거 / 규칙 |
|--------|------|-------------|
| `Key`/`Paste`/`Mouse` | UI 스레드 직접 | grid가 짧은 term 락으로 인코딩 후 `try_send`. **아래 지연 벤치로 확인한다** |
| `Resize` | **워커** | 블로킹(resize_lock + pty + grid). **합치기**: 세션당 최신 `seq`만 실행 |
| `Scroll` | UI 스레드 직접 | 짧은 락. 워커로 보내면 순서가 뒤집혀 스크롤이 튄다 |
| `extract_selection` | **워커(세션당 직렬)** | `selection_to_string()`이 선택 범위 전체를 훑는다(`term/mod.rs:529`). epoch 가드가 stale을 막는다 |
| 스냅샷 | 워커 | Plan 3 그대로 |

- [ ] **입력 지연 벤치(Task 1에 둔다).** "락이 짧다"는 **보장이 아니다** — `feed()`가 최대 64KiB
  청크를 락 쥔 채 파싱하므로 UI 스레드의 `send_key`가 그 뒤에 밀릴 수 있다. 리더가 최대 크기
  청크를 계속 먹이는 동안 UI가 intent 메서드를 반복 호출하는 벤치를 만들어 상한을 정한다.
  경합이 눈에 보이면 intent 처리를 세션당 직렬 워커로 옮긴다.
  **렌더 벤치나 추출 벤치로 이 결정을 대신하지 않는다** — 재는 것이 다르다.

### 0.9 포커스 리포팅

`Focusable::focus/unfocus`는 `Shell`도 메시지 채널도 받지 않아 바이트를 발행할 수 없다
(`iced_core/src/widget/operation/focusable.rs:7-16`).
- [ ] **앱이 포커스 전환을 소유한다.** 위젯의 `Focusable`은 렌더링·게이팅용이며 권위가 아니다.
- [ ] `report_focus`가 **락 안에서 진짜 `FOCUS_IN_OUT`을 읽는다**(캐시 금지). 반환 `WriteOutcome`.
- [ ] **순서**: 이전 세션에 focus-out을 먼저, 그 다음 새 세션에 focus-in.

### 0.10 키 인코딩 우선순위 표

같은 키가 여러 갈래에 걸린다(Ctrl+C = 복사냐 ETX냐).
1. **앱 단축키** — macOS `Cmd+C/V`, 그 외 `Ctrl+Shift+C/V`. **`repeat == true`면 건너뛴다**
2. **명명 키 / 제어 인코딩** — `encode_key`
3. **`KeyPressed.text`** — IME·데드키. 여기까지 왔으면 그대로 쓴다
4. **`Key::to_latin(physical_key)`** — **제어 조회에만**(비US 레이아웃에서 `Ctrl+[`를 찾기 위해서지
   문자를 삽입하기 위해서가 아니다)
- **`Alt+Ctrl+letter`**: 먼저 Ctrl로 제어 바이트를 만들고 그 앞에 `ESC`를 붙인다.
- 1번(모드 무관)은 **위젯**이, 2~4번(모드 필요)은 **`suaegi-term`**이 한다.

### 0.11 스냅샷 선택 영역의 표현

**뷰포트로 잘라낸 범위**를 쓴다.
```rust
pub struct ViewportSelection { pub start: (usize, usize), pub end: (usize, usize), pub is_block: bool }
```
렌더러가 매 셀마다 교차 판정을 하는 것보다 **스냅샷을 만드는 쪽이 락 안에서 한 번 잘라내는** 편이
싸고 렌더러를 단순하게 유지한다.

**잘라내기 규칙 (양 끝 포함)** — 선형과 블록이 다르다:
- **선형**: 위로 넘치면 시작을 `(0, 0)`, 아래로 넘치면 끝을 `(rows-1, cols-1)`로. 잘리지 않은 끝의
  열은 **보존한다.**
- **블록**: **행만 자르고 열 범위는 보존**한다(직사각형이 정의이므로 열을 경계로 밀면 모양이 망가진다).
  열은 `0..cols-1`로 클램프만.
- **`None` 조건**: 교차 후 남는 행이 없을 때(`end.line < 뷰포트 상단` 또는 `start.line > 하단`).

**테스트는 네 방향 교차(위만/아래만/양쪽/안쪽)와 블록 선택을 전부 덮는다.**

**산출물 검증:** `cargo check`가 통과한다. 그리고 **Task 1~7이 이 문서만 보고 시작 가능한지**
체크리스트로 확인한다. "여기서 정해야 할 걸 아직 안 정했다"가 하나라도 나오면 Task 0으로 되돌아온다.

## Task 1 — `suaegi-term` 배선

- [ ] **프로토콜 타입**(iced-free): `TermKey`, `Mods`, `TermMouseButton`, `KeyInput`, `MouseIntent`,
  `ViewportHit`, `MouseResult`, `CopyRequest`, `WriteOutcome`, `MouseEncodeError`.
- [ ] **`encode` 모듈**(`crates/suaegi-term/src/encode.rs`) — 순수 함수만:
  ```rust
  pub fn encode_key(input: &KeyInput, mode: TermMode) -> Option<Vec<u8>>;
  pub fn encode_paste(text: &str, mode: TermMode) -> Vec<u8>;
  pub fn encode_mouse(route: &MouseRoute, intent: &MouseIntent, mode: TermMode) -> Option<Vec<u8>>;
  pub fn encode_focus(focused: bool, mode: TermMode) -> Option<Vec<u8>>;
  pub fn route_mouse(intent: &MouseIntent, mode: TermMode) -> Result<MouseRoute, MouseEncodeError>;
  ```
  **`encode_mouse`도 `mode`를 받는다**: `MouseRoute::Report`는 와이어 포맷(SGR/X10/UTF8)을
  나르지 않고, `AltScreenArrows`는 `APP_CURSOR`를 봐야 한다. `handle_mouse`가 락을 쥔 채
  부르므로 비용이 없다. (`MouseRoute`를 넓히지 않는다 — 구현 중 발견.)
  **표 테스트가 전부 여기 있다** — 이 플랜에서 테스트 밀도가 가장 높아야 할 곳.

  **키 인코딩 규칙은 Task 4에 있다**(모드가 필요해 코드만 여기 산다). **마우스 규칙은 여기다**:
  - 버튼 코드: 좌 0, 중 1, 우 2, 레거시 릴리스 3, 휠 64/65. 드래그(버튼 눌린 채 이동) +32
  - 수식자 비트: shift +4, alt +8, ctrl +16
  - **모드 구분**: `MOUSE_REPORT_CLICK`(press/release만), `MOUSE_DRAG`(버튼 눌린 채 이동 추가),
    `MOUSE_MOTION`(버튼 없는 이동까지). **합성 플래그 `MOUSE_MODE` 하나로 판단하지 않는다**
  - 프로토콜: `SGR_MOUSE` → `ESC [ < b ; x ; y M|m`(선호), 아니면 X10 `ESC [ M …`, `UTF8_MOUSE` 변형
  - **좌표는 1-based 뷰포트 기준**(`iced_term`은 버퍼 좌표를 써서 스크롤백에서 음수 줄을 보낸다)
  - **레거시 오버플로 — 좌표계를 헷갈리지 말 것**(구현 중 실제로 걸린 지점이다).
    **와이어 값 = `33 + coord`**(마우스 좌표는 와이어에서 1-based다. `32 + coord`가 아니다).
    **임계값은 와이어가 아니라 좌표에 걸린다**: `UTF8_MOUSE`일 때 `coord >= 95`부터 2바이트
    UTF-8(`(0xC0 + wire/64, 0x80 + (wire & 63))`). `coord = 95` → 와이어 `128` → `0xC2 0x80`으로
    **정확한 UTF-8이고 overlong이 아니다**(임계값을 와이어 95로 읽으면 overlong이 나온다).
    상한은 **좌표 기준 배타적**: 비UTF8 `coord >= 223`, UTF8 `coord >= 2015`에서 **`None`**
    (에러가 아니다) — 각각 와이어 `255`, `2047 = U+07FF`에 대응한다.
    `SGR_MOUSE`는 십진 문자열이라 한계가 없다.
    (근거: `iced_term-0.8.0/src/backend.rs:330-352`. 클로저가 `pos`를 `32 + 1 + pos`로 재바인딩해
    읽기 어렵게 되어 있다.)
  - 드래그 중 눌린 버튼을 추적한다(레거시 릴리스 코드를 만들려면 필요)

  **두 가지 추가 결정**(구현 중 발견, 계획이 침묵했던 곳):
  - **`mods.logo`가 켜져 있으면 `encode_key`는 `None`이다.** macOS의 Cmd, 그 외의 Super는
    터미널 입력이 아니다. 억제하지 않으면 앱의 `classify_shortcut`이 거절한 `Cmd+W` 같은
    조합이 `text` 갈래로 흘러 셸에 `w`를 보낸다.
  - **비-bracketed 붙여넣기는 개행을 정규화한다**: `\r\n` → `\r`, 그 다음 `\n` → `\r`.
    비-bracketed 모드에서 앱은 붙여넣기와 타이핑을 구별할 수 없고 Enter가 실제로 내는 것은
    `\r`다 — `\n`을 보내면 여러 줄 붙여넣기가 타이핑과 다르게 동작한다.
    (alacritty_terminal은 파서라 이걸 하지 않는다. 프론트엔드 몫이고 우리가 프론트엔드다.)
- [ ] **`TerminalGrid`의 intent 메서드**(락을 안에서 잡고 놓는다, 바이트를 돌려준다):
  `encode_key_locked`, `encode_paste_locked`, `handle_mouse`, `encode_focus_locked`,
  `extract_selection(epoch)`, `request_copy(to)`. **쓰기 큐를 만지지 않는다.**
- [ ] **`bump_if_selection_changed`를 락을 쥐는 모든 연산 끝에 배치한다**(`feed`/`scroll`/`resize`/`handle_mouse`).
- [ ] **`FairMutex`의 페이로드를 상태 구조체로 바꾼다.** 지금은 `Term`만 감싸고 있어
  (`grid.rs:140-145`) 포인터 라우팅 상태와 선택 epoch를 `&self`로 바꿀 수 없다. 두 번째 뮤텍스를
  쓰면 "한 락 안에서"라는 불변식이 깨지고 락 순서를 증명해야 한다. 페이로드를 바꾸는 쪽이
  불변식과 일치한다:
  ```rust
  struct GridState {
      term: Term<GridEventProxy>,          // 현재 grid.rs가 쓰는 실제 타입 이름
      pointer: Option<PointerLatch>,       // press에서 래치, release에서 해제
      selection_epoch: u64,                // 0에서 시작
      last_seen_selection: Option<SelectionRange>,   // Term 생성 직후 값으로 초기화(보통 None)
  }
  // FairMutex<GridState>
  ```
  기존 파서/스냅샷 경로를 `state.term`으로 옮긴다 — **기계적이지만 넓은 변경**이므로
  이 항목만으로 커밋을 하나 쓴다.
- [ ] `TerminalSession` 래퍼(0.6): grid를 부른 뒤 큐에 넣는다.
- [ ] `TerminalSnapshot`에 `mode: TermMode`(렌더링 전용), `selection: Option<ViewportSelection>`,
  `SnapshotCursor`에 `blinking: bool`. **전부 `grid.rs:183-193`의 같은 락 안에서.**
- [ ] `scroll_display`를 `Scroll` 전체를 받게 넓힌다.
- [ ] **깨지는 호출부 둘 다 갱신**: `session_store.rs:120`, `state.rs:1077`.
  착수 전에 `grep -rn 'TerminalSnapshot *{' crates/`를 다시 돌린다.
- [ ] **generation의 의미를 문서화한다.** generation은 `TerminalSession`에 있고 그리드 락 **밖에서**
  올라간다(`grid.rs:254` 반환 후 `session.rs:363`). 스냅샷이 **새 상태를 보면서 옛 generation을
  볼 수 있다** — 정확한 버전이 아니라 "다시 찍어라"는 eventual 신호다. `session_store`의
  generation 가드가 정확한 대응을 전제하지 않는지 확인하고, 전제한다면 고친다.
- [ ] **건드리지 말 것**: `GridSize::history_size()`가 0인 것은 의도된 것이다.
- [ ] **입력 지연 벤치**(0.8) — 리더가 최대 청크를 먹이는 동안 intent 메서드 지연을 잰다.
- [ ] **추출 지연 벤치** — 최대 크기 스크롤백 선택으로 `extract_selection`을 잰다.

**테스트:** 인코더 표 테스트(아래 Task 4·6에 상세). **bracketed paste는 종료자 주입 케이스 필수**
(`\x1b[201~`가 든 텍스트 → 제거 확인). 선택 왕복과 **`display_offset != 0`일 때의 보정**,
**뷰포트 위/아래 경계를 넘는 선택**(네 방향 + 블록). `extract_selection`은 읽기 전용이므로 "변경하지 않는다"는 단언은 공허하다 — 대신
**불일치 epoch는 `None`, 일치 epoch는 기대한 텍스트, 그리고 그 뒤에도 선택이 여전히 살아 있음**을
단언한다.
mutation: 보정 제거, epoch 비교 제거 시 각각 FAIL해야 한다.

## Task 2 — 헤드리스 테스트 하네스

- [ ] `pane_grid_behavior.rs`의 `Harness`를 재사용 가능한 모듈로 뽑는다.
- [ ] 제공: 레이아웃 + 합성 `Event` 주입, 발행 메시지 수집, `Shell` 상태 관찰, **읽기 가능한 클립보드 페이크**(`clipboard::Null`은 항상 `None`이라 붙여넣기 테스트에 부족하다).
- [ ] `pane_grid_behavior.rs`를 새 하네스로 옮기고 **6개 테스트가 계속 통과**하는지 확인.
- [ ] **하네스 문서에 한계를 명시한다**: `()` 렌더러는 텍스트를 측정하지 않고 `Paragraph::compare`가 항상 `Difference::None`이다. 측정 의존 로직은 여기서 테스트하지 말 것.

## Task 3 — 위젯 뼈대 + 순수 레이아웃 계산

- [ ] **모듈을 먼저 가른다** — Task 4와 5가 병렬로 돌 수 있으려면 파일이 갈려 있어야 한다:
  ```
  crates/suaegi-app/src/terminal/
    contract.rs   (Task 0)   mod.rs / state.rs (Task 3, 여기서 동결)
    input.rs      (Task 4)   render.rs, palette.rs (Task 5)   mouse.rs (Task 6)
  ```
  **Task 3이 끝나는 시점에 위젯 상태 구조체와 그 접근자를 동결한다.** Task 4·5는 그 필드를
  읽고 쓸 뿐 정의를 바꾸지 않는다. `Widget` 트레이트 impl은 `mod.rs`에 두되 각 메서드 본문은
  해당 모듈의 함수에 위임해, 두 태스크가 같은 줄을 건드리지 않게 한다.
- [ ] **`CellMetrics`는 유효성을 보장하는 타입으로 만든다.**
  ```rust
  pub struct CellMetrics { width: f32, height: f32 }   // 필드 비공개
  impl CellMetrics {
      /// width/height가 유한하고 > 0일 때만 Some.
      pub fn new(width: f32, height: f32) -> Option<Self>;
  }
  ```
  이렇게 하면 아래 두 순수 함수가 "메트릭이 이상한 경우"를 다시 방어하지 않아도 된다.
- [ ] **순수 함수 시그니처를 못 박는다** — 여기가 이 플랜에서 테스트 밀도가 가장 높아야 할 곳이다:
  ```rust
  /// bounds가 유한하지 않거나, 계산 결과가 0이거나 u16::MAX를 넘으면 None.
  /// f32 → 정수 캐스팅은 saturating이므로 캐스팅 전에 범위를 검사한다.
  pub fn grid_size(bounds: Size, m: CellMetrics) -> Option<GridSize>;

  /// pos는 위젯 bounds 기준 상대 좌표(Cursor::position_in의 결과).
  /// 유한하지 않거나 음수면 None. 계산된 row/col이 그리드 밖이면 None.
  pub fn hit_test(pos: Point, m: CellMetrics, size: GridSize) -> Option<ViewportHit>;
  ```
  **둘 다 `Option`이다.** 셀을 항상 돌려주면 오른쪽/아래 모서리에서 `col == cols`가 나오고,
  음수/NaN은 캐스팅에서 조용히 0이 된다.
- [ ] `Widget` 구현: `size`(Fill/Fill), `layout`(`Node::new(limits.max())`), `tag`/`state`, `mouse_interaction`(`Interaction::Text`).
- [ ] **위젯 상태(여기서 동결)** — Task 6이 쓸 **원시 사실만** 둔다. **선택 상태기계를 두지 않는다**
  (라우팅을 모르므로 유지할 수 없다 — Task 0.4):
  포커스, `CellMetrics` 캐시, `last_bounds`, `last_emitted: Option<GridSize>`,
  `held: Option<TermMouseButton>`, `last_click: Option<LastClick>`,
  `cursor_pos: Option<Point>`, `scroll_acc: f32`, `mods: Mods`.
- [ ] **측정**(순수 함수 밖): `Paragraph::with_text`로 `"MMMMMMMMMM"` → `min_bounds().width / 10.0`. **줄 높이는 측정하지 말고 `LineHeight::to_absolute(size)`** — cosmic-text에 그대로 들어가는 권위 있는 값. **f32로 유지**하고 `WindowSize`를 만들 때만 반올림(`iced_term`은 u16으로 잘라 열당 ~1px 드리프트를 만든다).
- [ ] **리사이즈**: 캐시를 **두 개** 둔다 — `last_bounds`(측정한 크기)와 `last_emitted: Option<GridSize>`
  (실제로 발행한 그리드 크기). `grid_size`가 `None`을 주면(pane이 0으로 접힘) **`last_emitted`를
  `None`으로 무효화한다.** 하나만 두면 pane이 접혔다 원래 크기로 돌아올 때 "같은 크기"로 보여
  리사이즈가 발행되지 않고 PTY가 낡은 크기에 남는다. `Some`이고 `last_emitted`와 다를 때만 발행.
  (`session_store.rs`의 고정 스폰 보정은 **Task 7**이다 — 스폰 시점엔 레이아웃이 없어
  실제 크기를 알 수 없다. `DEFAULT_ROWS`/`COLS`는 부트스트랩 기본값으로 남고,
  `last_emitted`가 `None`에서 시작하므로 첫 유효 레이아웃이 반드시 보정을 발행한다.)

**테스트:** `grid_size`/`hit_test`를 **실제 메트릭 값으로** 표 테스트(경계: 딱 떨어지는 크기, 나머지가 남는 크기, 0행/0열, 음수/NaN 방어). 헤드리스로는 "크기 변화 → 1회 발행, 재진입 → 발행 없음"만. **측정 자체와 캐시 무효화는 검증 불가** — 코드 주석과 보고서에 명시.

## Task 4 — 포커스 + 키 입력

Task 3에만 의존한다. **Task 5(렌더링)와 병렬 가능.**

- [ ] `operation::Focusable` 구현 + `operate()` 노출. 위젯은 `Option<widget::Id>`를 갖는다.
- [ ] **포커스 권위는 앱에 있다**(Task 0). `pane_grid`의 `on_click` → 앱 메시지 → `operation::focus(id)` + `FOCUS_IN_OUT` 바이트 발행.
- [ ] `update` 진입부에서 포커스·bounds를 직접 확인. **`ModifiersChanged`는 언포커스여도 받는다**(`iced_term`은 여기서 수식자 캐시가 상한다).
- [ ] **iced → 프로토콜 변환**(`terminal/input.rs`). `suaegi-term`은 iced를 모르므로 앱이 번역한다.
  **`modified_key`는 옮기지 않는다**(Task 0.1). `text`가 수식자 적용·IME 결과를 이미 담고,
  제어·단축키 조회는 `physical_latin`이 담당한다 — 셋을 다 나르면 어느 것이 권위인지 흐려진다.
  ```rust
  fn to_key_input(key, physical_key, location, modifiers, text, repeat) -> KeyInput;
  //                    ^^^^^^^^^^^^ 여기서 key.to_latin(physical_key) → physical_latin
  ```
  **이 변환 자체가 표 테스트 대상이다**(명명 키 매핑, `to_latin` 폴백, 수식자 비트).
- [ ] **단축키 분류는 위젯에**(모드와 무관하므로):
  ```rust
  pub enum Platform { Mac, Other }   // 인자로 받는다 — cfg!면 한쪽 플랫폼 테스트가 아예 안 돈다
  pub fn classify_shortcut(input: &KeyInput, p: Platform) -> Option<Shortcut>;  // Copy | Paste
  ```
  걸리면 `TermCommand::Key`를 발행하지 않는다. **분류와 인코딩을 나눈다** — 섞으면 `Ctrl+C`가
  복사인지 ETX인지가 함수 안에 숨는다.
- [ ] **`encode_key`는 Task 1(`suaegi-term::encode`)에 있다.** 모드가 필요하기 때문이다.
  아래 규칙은 그 구현의 명세이며 **표 테스트도 거기 있다**:
  - `APP_CURSOR`: 화살표/Home/End가 `ESC O A` vs `ESC [ A`
  - `APP_KEYPAD`: 키패드 `ESC O ...`
  - **수식자 파라미터는 계산한다**: `1 + shift*1 + alt*2 + ctrl*4` → `ESC [ 1 ; <mod> <final>`
  - Ctrl+letter → `0x01..0x1A`, Ctrl+`[ \ ] ^ _`, Ctrl+Space → NUL. **`Ctrl+U`는 `\x15`**(`iced_term`은 `\x51`='Q'로 깨져 있다)
  - Alt+문자 → `ESC` 프리픽스. Backspace `0x7F`, Alt+Backspace `ESC \x7F`, Shift+Tab `ESC [ Z`
  - Enter `\r`, **`LINE_FEED_NEW_LINE`이면 `\r\n`**
  - Insert/Delete/PgUp/PgDn `ESC [ n~`, F1-F4 `ESC O P/Q/R/S`, F5-F12 `ESC [ n~`
  - **수식자를 정확 일치로 비교하지 않는다** — CapsLock 하나에 조용히 무시되면 안 된다
- [ ] 붙여넣기: 위젯이 클립보드를 **읽어 원문 그대로** `TermCommand::Paste(String)`으로 낸다.
  **감싸기는 Task 1의 `encode_paste`가 한다**(bracketed paste는 라이브 모드가 필요하다).
- [ ] 키 입력 시 `Scroll::Bottom`.
- [ ] **입력 유실 피드백은 앱 레벨 메시지다**(커맨드가 아니다). 앱이 `WriteOutcome::Dropped`를
  보고 상태를 전이시킨다. `Suppressed`는 유실이 아니므로 피드백을 내지 않는다.

**테스트(이 태스크 몫):** iced→프로토콜 변환의 표 테스트. `classify_shortcut`을 **양쪽 플랫폼
인자로** — `cfg!`였다면 한쪽이 아예 안 돌았을 것이다. 포커스 게이팅은 **대조군과 함께**
(언포커스에서 커맨드 없음 + 포커스에서 커맨드 있음 + 언포커스에서도 `ModifiersChanged` 반영).
`repeat == true`일 때 단축키가 재발동하지 않는지.
(`encode_key`/`encode_paste`의 표 테스트는 Task 1에 있다.)

## Task 5 — 팔레트 + 렌더링

Task 3에만 의존한다. **Task 4와 병렬 가능.**

- [ ] **팔레트를 한 번만 만든다.** `[iced::Color; 256]` + fg/bg/cursor. **모든 `NamedColor` 변형을 열거해 표로 못 박는다**(0-15, `Foreground`/`Background`/`Cursor`, Dim 8종, `BrightForeground`, `DimForeground`). Indexed: 0-15 명명색, 16-231 큐브(`16 + 36r + 6g + b`, 성분 `0 or r*40+55`), 232-255 회색(`i*10+8`).
  **팔레트는 고정 내장값이다** — OSC 4/10/11 동적 변경은 Plan 5다. 따라서 **실패 가능한 생성자가 필요 없다**(`iced_term`처럼 hex 문자열을 들고 있지 않으므로). 색은 `iced::Color` 상수로 직접 쓴다.
- [ ] **셀 스타일 결정을 드로우에서 분리**한다. 반환이 `(fg, bg)`면 `HIDDEN`(글자만 숨김)을
  표현할 수 없어 플래그 처리가 두 곳으로 갈린다:
  ```rust
  pub struct ResolvedCell { pub fg: Color, pub bg: Color, pub draw_glyph: bool, pub underline: Option<UnderlineKind>, pub strikeout: bool, pub bold: bool, pub italic: bool }

  pub fn resolve_cell(cell: &SnapshotCell, p: &Palette, selected: bool, under_cursor: bool) -> ResolvedCell;
  ```
  `INVERSE`·`DIM`·`HIDDEN`·선택·커서 반전의 **합성 순서**를 함수 doc에 못 박는다
  (예: DIM 감쇠 → INVERSE 교환 → 선택 교환 → 커서 교환, 교환은 짝수 번이면 상쇄).
- [ ] **3패스 드로우**: (1) 배경 전부 → (2) 커서 → (3) 텍스트. `iced_term`은 배경 런을 커서보다 늦게 flush해 커서를 덮는다.
- [ ] **배경과 글리프를 분리해 정의한다**(Codex 지적): **모든 보이는 슬롯이 자기 배경을 받는다**(spacer 포함). spacer는 **글리프만** 억제한다. wide 글리프는 두 칸 중앙에 놓고, **마지막 열에 걸리면 클리핑**한다. 이렇게 하면 줄바꿈 경계의 `LEADING_WIDE_CHAR_SPACER`가 배경을 잃지 않는다.
- [ ] 배경은 같은 색 수평 런으로 배칭.
- [ ] `Flags`: `BOLD`, `ITALIC`, `DIM`/`DIM_BOLD`, `INVERSE`, `HIDDEN`(글자만), `STRIKEOUT`, 밑줄 5종.
- [ ] `combining` 문자를 기준 문자에 붙인다.
- [ ] **커서**: 5종 모두. `Hidden`이면 안 그린다. **언포커스면 `HollowBlock`** — alacritty가 해주지 않고 렌더러 책임이다. 커서 아래 글자는 **모드와 무관하게** fg/bg 교환(`iced_term`의 `APP_CURSOR` 게이팅은 버그).
- [ ] `with_layer(bounds, ..)`로 클리핑. `Quad.snap` 명시(기본값이 `crisp` feature에 달려 있다).
- [ ] **벤치**: 행당 `with_spans` vs 셀당 `fill_text`를 실제로 재고 문서에 남긴다. `follow-ups.md` 6번과 damage 도입 여부도 여기서 판단.

**테스트:** 팔레트 변환과 `resolve_cell`은 순수 함수 → 표 테스트. **`ResolvedCell` 전체를 단언한다**(`draw_glyph`와 장식 포함)(큐브/회색 경계, 세 갈래, `INVERSE`+선택 동시 등 합성). wide char의 **배경 슬롯과 글리프 억제를 따로** 단언. **실제 픽셀은 검증 불가** — 명시한다.

## Task 6 — 마우스 (위젯 쪽)

Task 0의 계약과 Task 3의 `hit_test`, Task 1의 `route_mouse`/`handle_mouse`에 의존한다.

**범위 주의:** 라우팅(선택이냐 리포트냐 스크롤이냐)과 선택 변경은 **Task 1이 락 안에서** 한다.
이 태스크는 **iced 이벤트를 `MouseIntent`로 만드는 것까지**다. 위젯은 라우팅 결과를 모른다 —
`MouseResult`는 위젯의 `update`가 끝난 뒤 앱에 반환되므로 위젯이 그걸 보고 상태를 유지할 수 없다.

- [ ] **위젯이 드는 상태는 원시 사실뿐**: 눌린 버튼, 마지막 클릭 시각·위치, 커서 위치,
  스크롤 픽셀 누산기, 수식자. **선택 상태기계를 위젯에 두지 않는다.**
- [ ] **우리 소유의 클릭 분류기**(순수 함수):
  ```rust
  pub struct LastClick { pub button: TermMouseButton, pub at: Instant, pub pos: Point, pub kind: ClickKind }

  pub fn classify_click(
      prev: Option<LastClick>,
      button: TermMouseButton, now: Instant, pos: Point,
  ) -> ClickKind;
  ```
  **`kind`를 반드시 들고 다녀야 한다.** 분류는 "직전 클릭이 있었나"가 아니라
  **"직전 클릭의 종류를 한 단계 전진"**이다. 이전 종류가 없으면 2번째 클릭이 Double,
  3번째도 Double이 되어 **`Triple`이 영원히 도달 불가능**해지고 `SelectionType::Lines`
  (트리플 클릭 줄 선택)가 조용히 죽는다. iced도 같은 벽에 부딪혀 `Click`에 `kind`를
  넣어 푼다(`iced_core/src/mouse/click.rs:9-14`, `previous.kind.next()` at `:53`).
  (구현 중 발견 — Task 3의 동결 목록이 이 필드를 좁게 잡고 있었다.)
  **버튼을 반드시 받는다.** 없으면 좌클릭 직후 같은 자리 우클릭이 더블클릭으로 분류된다
  (iced의 분류기도 버튼 일치를 요구한다 — `iced_core/src/mouse/click.rs:50-53`).
  **`mouse::Click::new`에 의존하지 않는다** — 내부에서 `Instant::now()`를 읽어 시계를 주입할 수
  없고, 그러면 mutation 검증을 할 수 없다. 시각은 위젯 상태에 들고 다닌다.
- [ ] `Cursor::position_in(bounds)` → `hit_test`(Task 3) → `ViewportHit`.
  **`Cursor::Levitating`은 `position()`이 `None`이라 조용히 실패한다** — 위에 오버레이가 있다는
  뜻이므로 무시가 맞지만 의도적임을 주석으로 남긴다.
- [ ] **`force_local`(Shift 오버라이드)만 위젯이 판단한다** — 모드와 무관하기 때문이다.
  앱이 마우스 모드를 쥐고 있어도 Shift를 누르면 사용자가 선택할 수 있어야 한다.
- [ ] **held 전이 표를 못 박는다.** `route_mouse`의 불변식 검사와 어긋나면 `MouseEncodeError`가
  정상 입력에서 튄다:

  | 이벤트 | intent의 `held` | 위젯 상태 갱신 시점 |
  |--------|-----------------|---------------------|
  | Press(b) | `Some(b)` | intent를 만들기 **전에** `held = Some(b)` |
  | Motion | 현재 `held` 그대로(`None`일 수 있다) | 갱신 없음 |
  | Wheel | 현재 `held` 그대로 | 갱신 없음. **래치에 참여하지 않는다**(0.4) |
  | Release(b) | `Some(b)` — 놓인 버튼을 싣는다 | intent를 만든 **뒤에** `held = None` |

  `held`가 눌린 적 없는 버튼의 Release를 받으면(창 밖에서 눌렸다 들어온 경우 등)
  intent를 만들지 않고 버린다.
- [ ] **스크롤**: `Lines`와 `Pixels` **양쪽** 처리(휠=Lines, 트랙패드=Pixels). Pixels는 나머지 누산기:
  ```
  acc += lines_from(delta);   // 부호 주의 — 아래
  let lines = acc.trunc();  acc %= 1.0;
  ```
  **부호는 `+`다.** 초안의 `acc -= y`는 방향이 뒤집혀 있었다(구현 중 발견). 셋이 모두
  "양수 = 위로"로 이미 고정돼 있기 때문이다: `MouseAction::Wheel { lines }`의 문서
  (`input_types.rs:132`), iced의 `y`(`scrollable`이 `-Vector::new(x, y)`를 아래 방향
  오프셋에 더한다 — `scrollable.rs:873, 1799`), 그리고 `Scroll::Delta` 양수가
  `display_offset`을 올린다는 실측. `-=`면 모든 스크롤이 반대로 간다.

  **줄 단위로 누산한다**(픽셀 단위가 아니라). 동결된 `scroll_acc`가 `f32` 하나인데
  휠은 `Lines`, 트랙패드는 `Pixels`로 온다 — 진입 시 줄로 정규화해 누산하면 세션 중
  장치가 바뀌어도 남은 나머지가 엉뚱한 스케일로 재해석되지 않는다.
  누산 결과가 0줄이면 커맨드를 발행하지 않는다. **줄 수를 `MouseIntent`에 실어 보내고**,
  그것이 로컬 스크롤인지 alt-screen 화살표인지 리포트인지는 Task 1이 정한다.
- [ ] 앱은 `MouseResult.copy`가 `Some`이면 워커로 `extract_selection(epoch)`을 돌린다.
  **워커 완료 메시지를 정의한다**(명시적 복사와 드래그 완료가 같은 경로를 쓴다):
  ```rust
  Message::SelectionExtracted { id: SessionId, targets: CopyTargets, text: Option<String> }
  ```
  `text == None`은 **조용한 취소**다(epoch 불일치 또는 선택 없음) — 오류를 띄우지 않는다.
  `Some`이면 요청된 Standard/Primary에 **정확히 그것만** 쓴다.
- [ ] `MouseEncodeError`(= `handle_mouse`의 `Err`)는 로그 + 디버그 단언. **억제와 구분한다.**

**테스트:** 클릭 분류기를 **주입한 시각으로** 표 테스트(임계 직전/직후, 거리 초과) — 시계를
인자로 받았기 때문에 mutation 검증이 가능하다. `hit_test` 경계. Lines/Pixels 양쪽과
**나머지 누산 보존**(작은 델타 여러 번 → 정확히 1줄, 0줄일 때 미발행). Shift 오버라이드가
`force_local`을 세우는지. `MouseEncodeError`가 `Ok(None)`과 다르게 처리되는지 통합 테스트로 고정.
(마우스 **리포트 바이트**와 **라우팅**의 표 테스트는 Task 1에 있다 — 모드×Shift×액션 전 조합,
1-based 뷰포트 좌표, **스크롤백이 있을 때의 좌표 회귀**, X10/UTF8 오버플로 경계.)

## Task 7 — 워크벤치 통합

- [ ] `workbench.rs`: `scrollable` 제거, `.spacing(4).on_resize(0, ..)`, `text()` → 새 위젯.
- [ ] `TitleBar` 생성부에 **왜 하중 부재인지** 주석.
- [ ] **`DEFAULT_ROWS`/`COLS` 보정**(Task 3에서 넘어옴): 고정 스폰은 부트스트랩 기본값으로
  두고, 위젯이 첫 레이아웃에서 발행하는 `Resize`가 실제 크기로 고친다. 세션 스폰 시점에
  크기를 알아내려 하지 않는다 — 그때는 레이아웃이 존재하지 않는다.
- [ ] `TermCommand` → 세션 배선을 **Task 0.8의 스레딩 정책 표대로**. 리사이즈 합치기와 seq 가드, `MouseResult` 후속 처리(copy 워커 + redraw) 포함.
- [ ] 포커스 전환 + `FOCUS_IN_OUT` 바이트(Task 0의 경로).
- [ ] 입력 유실 피드백 UI — 앱 레벨 메시지/상태 전이(`WriteOutcome::Dropped`)에서 온다. `Suppressed`는 유실이 아니다.

**검증:** 헤드리스로 되는 것은 전부. **사람 눈이 필요한 것**(실제 셸 타이핑, vim/htop 마우스, CJK 정렬, 색 정확도, 리사이즈 체감)은 `follow-ups.md` 19번 방식대로 **확인하지 못했다고 명시**하고 사람이 확인할 목록을 PR에 적는다.

---

## 범위 밖 (Plan 5)

- 에이전트 상태 3색(working/waiting/done) — hook 서버
- diff 패널, 세션 레이아웃 복원
- 하이퍼링크(OSC 8) — **연다면 스킴 allowlist가 필수다.** `iced_term`은 터미널이 준 임의 URL을 `open::that`에 넘기고 정규식이 `file://`·`ssh:`를 허용한다
- OSC 4/10/11 동적 팔레트(`RenderableContent.colors`, 락 안에서 복사 필요)
- 검색, vi 모드, kitty 키보드 프로토콜(`Config::kitty_keyboard`가 기본 false)
