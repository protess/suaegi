# Plan 4 조사: 터미널 커스텀 위젯

> 2026-07-21. 네 개의 조사 에이전트가 **vendored 소스를 직접 읽어** 확인한 것만 적는다.
> 모든 주장에 `file:line`이 붙어 있다. 기억에서 쓴 문장은 이 문서에 없다.
>
> 대상 버전: iced 0.14.0 / iced_core 0.14.0 / iced_widget 0.14.2 / iced_graphics 0.14.0 /
> alacritty_terminal 0.25.1 / vte 0.15.0.
> vendored 루트: `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`

---

## 0. 요약 — 이 조사가 바꾼 결정 6가지

1. **`shell.capture_event()`는 `pane_grid`를 막지 못한다.** 캡처는 단락이 아니라 플래그다.
   설계가 성립하는 이유는 캡처가 아니라 **드래그가 타이틀바에서만 시작된다**는 제약 하나뿐이다.
   → `TitleBar`는 장식이 아니라 **하중을 받는 부재**다.
2. **워크벤치의 `scrollable` 래퍼는 제거한다.** 한 번 스크롤하면 1.5초 동안 휠 이벤트를
   자식에게 전달하지 않는다. 터미널이 스크롤백과 마우스 리포팅을 직접 소유한다.
3. **`TermMode` 한 필드가 병목이다.** 키 인코딩·붙여넣기·마우스 리포팅·alt-screen 스크롤이
   전부 여기에 달렸는데, `snapshot()`이 이미 계산해놓고 버리고 있다. 추가 락 0.
4. **키 인코딩 테이블은 우리가 쓴다.** alacritty_terminal에 인코더가 없다(4중 확인). 단
   **kitty 프로토콜은 꺼져 있으므로 레거시 xterm 인코딩만** 필요하다.
5. **렌더링은 셀당 `fill_text`가 아니라 행당 `Paragraph::with_spans`.** 단 `with_spans`는
   `Shaping::Advanced`를 강제하므로 **비용을 실측한 뒤** 확정한다.
6. **`Text`는 `Default`가 없다.** `iced_term`의 `..Default::default()`를 그대로 베끼면 깨진다.

---

## 1. iced 0.14 — 기억과 다른 것들

Plan 3의 같은 절과 이어진다. 각 항목은 컴파일 실패 1회 또는 런타임 오작동 1건을 뜻한다.

| # | 사실 | 출처 |
|---|------|------|
| 1 | `Widget::on_event`는 없다. `update`가 대신이고 **`()`를 반환**한다. 소비는 `shell.capture_event()` | `iced_core/src/widget.rs:112-123` |
| 2 | `update`/`layout`/`operate`가 **`&mut self`**를 받는다 | `widget.rs:56,100,112` |
| 3 | `event`는 **`&Event`**(참조)로 온다 | `widget.rs:114` |
| 4 | **`text::Text`에 `Default` impl이 없다.** 9개 필드 전부 명시 | `iced_core/src/text.rs:20-47` (grep 무결과) |
| 5 | 캡처는 형제를 막지 않는다. `row`/`column`/`container`에 `is_event_captured` 확인이 **없다**. `stack`만 확인한다 | `iced_widget/src/stack.rs:262` |
| 6 | `mouse::Cursor::Levitating`이 새로 생겼고 `position()`이 `None`을 준다 → `is_over`/`position_in`이 조용히 실패 | `iced_core/src/mouse/cursor.rs:5-22` |
| 7 | `Quad`에 `snap: bool`이 생겼고 기본값이 `cfg!(feature = "crisp")` | `iced_core/src/renderer.rs:79-102` |
| 8 | `Paragraph::with_spans`는 `Text.shaping`을 **무시**하고 `Advanced`를 강제한다. `with_text`는 존중한다 | `iced_graphics/src/text/paragraph.rs:161` vs `:89` |
| 9 | `fill_text`는 `Text<String, Font>`를 **값으로** 받는다 — 호출당 String 할당 | `iced_core/src/text.rs:368-374` |
| 10 | 클리핑 API가 따로 없다. `with_layer(bounds, ..)`가 클리핑이다 | `iced_core/src/renderer.rs:22-28` |
| 11 | **`Modifiers::control()`**이지 `ctrl()`이 아니다. `command()`는 macOS에서 Cmd라 Ctrl 제어문자 경로에 쓰면 틀린다 | `iced_core/src/keyboard/modifiers.rs` |
| 12 | `KeyPressed`에 `repeat: bool`이 있다 | `iced_core/src/keyboard/event.rs:12` |
| 13 | `Widget::overlay`에 `viewport`/`translation` 인자가 추가됐다 | `widget.rs:140-149` |
| 14 | `mouse::Interaction`의 기본값은 `Idle`이 아니라 `None` | `iced_core/src/mouse/interaction.rs:6-32` |

### 1.1 `Widget` 트레이트 (필수/제공)

필수는 `size`, `layout`, `draw` 셋뿐. 나머지는 전부 기본 구현이 있다.
`tag`/`state`로 위젯 내부 상태를 선언하고 `tree.state.downcast_ref::<T>()`로 꺼낸다 —
**`State::None`에 downcast하면 패닉**한다(`tree.rs:238`). `Tag::of::<T>()`가 어긋나면
`Tree::diff`가 서브트리를 통째로 재생성하며 **상태를 조용히 리셋**한다(`tree.rs:57-68`).

### 1.2 포커스 — 필터링은 우리 책임

```rust
pub trait Focusable {          // iced_core/src/widget/operation/focusable.rs:7-16
    fn is_focused(&self) -> bool;
    fn focus(&mut self);
    fn unfocus(&mut self);
}
```

`operate()`에서 `operation.focusable(self.id.as_ref(), layout.bounds(), state)`로 노출한다
(`iced_widget/src/text_input.rs:681-692` 패턴).

**확인됨: `Widget::update`는 포커스와 무관하게 모든 키보드 이벤트를 받는다.**
런타임은 루트에 전부 뿌리고(`iced_runtime/src/user_interface.rs:320-329`) 컨테이너는
포커스 검사 없이 모든 자식에게 전달한다(`row.rs:261-271`). `text_input`도 손으로 게이팅한다
(`text_input.rs:901-910`). → 우리도 `update` 진입부에서 포커스와 `is_event_captured`를
직접 확인해야 이중 처리를 피한다.

앱 레벨 포커스: `iced::widget::operation::focus(id) -> Task<T>`
(`iced_runtime/src/widget/operation.rs:64-67`). **매칭되지 않는 focusable을 전부 unfocus**
시키므로 상호배타가 공짜다(`focusable.rs:45-47`).

### 1.3 렌더링 — 행당 1 draw call

`fill_text`는 셀 루프에서 쓰면 안 된다(값 전달 = 글리프당 String 할당 + 매 프레임 재셰이핑).
권장 순서:

1. **`Paragraph::with_spans` + 행당 `fill_paragraph` 1회.** `Span`에 `color: Option<Color>`가
   있어 셰이핑에 구워지고(`iced_graphics/src/text/paragraph.rs:152-157`),
   `Highlight { background, border }`로 **셀 배경도 별도 quad 없이** 나온다
   (`iced_core/src/text.rs:406-411`). `Paragraph` 내부는 `Arc`라 캐싱이 싸다.
   **대가**: `Advanced` 셰이핑 강제(위 표 8번). 실측 대상.
2. 캐시된 런마다 `fill_paragraph`.
3. `fill_text` — 최악.

`fill_text`/`fill_paragraph`는 **`color`를 `Text`와 별개 인자로** 받는다. `Text`에 색 필드가 없다.

셀 메트릭: 전용 헬퍼가 없다. `Paragraph::with_text`로 `"M"`(또는 `"MMMMMMMMMM"` 후 /10)을
셰이핑하고 `min_bounds()`를 읽는다. **줄 높이는 측정보다 `LineHeight::to_absolute(size)`가
권위 있다** — cosmic-text에 그대로 들어가는 값이기 때문(`paragraph.rs:71-76`).
`layout()`에서 한 번 재고 `tree.state`에 캐시, 폰트/크기 변경 시에만 재측정
(`Paragraph::compare -> Difference`가 그 판정용이다).

### 1.4 입력 타입

- `KeyPressed { key, modified_key, physical_key, location, modifiers, text, repeat }` — 이 순서.
  `KeyReleased`에는 `text`도 `repeat`도 없다.
- `Key::to_latin(physical_key) -> Option<char>` — **비US 레이아웃에서 견고한 해석 경로**.
  `text_input`이 쓴다(`text_input.rs:913`).
- `ScrollDelta::{Lines{x,y}, Pixels{x,y}}` — 둘 다 명명 필드. **양쪽 다 처리해야 한다**
  (트랙패드=Pixels, 휠=Lines). 양수 y가 위로.
- `Cursor::position_in(bounds)` — 셀 히트테스트용 상대 좌표.
- `Clipboard::read(&self, Kind)` / `write(&mut self, Kind, String)`. `Kind::{Standard, Primary}`.
  Primary는 macOS/Windows에서 no-op — **쓸 때는 양쪽에, 읽을 때는 Standard**.

---

## 2. `pane_grid` × 마우스 — 가장 깨지기 쉬운 가정

> **스파이크로 검증 완료.** `crates/suaegi-app/tests/pane_grid_behavior.rs`의 6개 테스트가
> 아래 주장을 런타임에서 확인했고, 6개 전부 mutation 검증을 통과했다(M1~M6).
> 검증 등급이 항목마다 다르므로 §2.4를 반드시 같이 읽을 것.

### 2.0 헤드리스 위젯 테스트가 가능하다 — 이 저장소 전체에 해당

`iced_core`가 **feature gate 없이** `impl Renderer for ()`를 제공한다
(`iced_core/src/renderer/null.rs:10`). 따라서

```
Shell::new(&mut Vec<Message>) + iced::advanced::clipboard::Null + &() 렌더러
  + Tree::new(&element) + Widget::layout → Layout::new(&node)
```

만으로 창·GPU·OS 입력 없이 위젯 트리에 **합성 `Event::Mouse`를 흘려보낼 수 있다.**
합성 OS 클릭이 아니다 — 금지 규칙에 걸리지 않는다.

이것은 `docs/follow-ups.md` 19·20번이 "이 저장소 하네스로는 의미 있게 검증할 수 없다"며
미뤄둔 항목들의 전제를 바꾼다. Plan 4의 위젯 로직(키 인코딩, 포커스 게이팅, 선택 상태기계,
히트테스트)은 **전부 회귀 테스트 가능하다.** 계획은 이를 전제로 짠다.

(`iced_tester`가 feature `tester` 뒤에 있지만 필요 없었고 추가하지 않았다.)

### 2.1 확인된 것

- **자식이 먼저 받는다.** `pane_grid::update`가 자식 루프를 먼저 돌고(`pane_grid.rs:509-528`)
  그 다음 자기 `match event`를 실행한다(`:530`).
- **`pane_grid`는 `is_event_captured`를 어디서도 확인하지 않는다**(grep 무결과). 대조군:
  `scrollable`은 확인한다(`scrollable.rs:842-844`). → **캡처로 pane_grid를 막을 수 없다.**
- **드래그는 타이틀바의 pick area에서만 시작된다.** `can_be_dragged_at`이 타이틀바가 없으면
  무조건 `false`(`pane_grid/content.rs:413-426`). pick area는 타이틀바에서 **title 요소와
  controls의 bounds를 뺀 나머지**(`title_bar.rs:238-268`) — 지금 워크벤치에선 6px 패딩과
  title↔버튼 사이 틈뿐이다. 타이틀 텍스트를 잡으면 안 끌린다(UX 흠집, 정확성 문제는 아님).
- **picked 상태에선 본문 `update`를 건너뛴다**(`content.rs:272-283`) — 드래그 중 터미널이 조용해진다.
- **분할 히트밴드는 `spacing + leeway` 폭이고 분할선에 중앙정렬**된다(`axis.rs:76-98`).
  현재 `.spacing(2).on_resize(8, ..)` ⇒ **10px 밴드가 2px 거터 위에 놓여 양쪽 본문을 4px씩 침범**한다.
- **`pane_grid`는 `WheelScrolled`를 무시한다**(`_ => {}` at `:685`).
- **`scrollable`은 다르다.** 첫 스크롤 후 `last_scrolled`로 트랜잭션을 열고, 그동안 휠을
  **자식에게 전달하지 않는다**(`scrollable.rs:786-791`). 트랜잭션의 존재와 삼킴은 런타임에서
  확인했다. **지속시간은 확인하지 못했다** — `Instant::now()` 기반이라 시계를 주입할 수 없다.
  그리고 `1500ms` 하나가 아니다: 같은 영역에 `Duration::from_millis(100)`을 쓰는 다른 갈래가
  있다(`:609` vs `:611`). "1500ms 트랜잭션"은 타임아웃 로직의 **부분적 서술**이다.
- `on_click`은 분할선이 아닌 본문 press마다 발행되며 자식 캡처가 막지 못한다(`:1069-1072`).
  → "본문 아무 데나 누르면 그 pane에 포커스"가 공짜로 따라온다. 원하는 동작이다.

### 2.2 따라야 할 규칙

1. **`TitleBar`를 유지하고 하중 부재로 취급한다.** 본문 드래그가 pane을 옮기지 못하게 막는
   유일한 기제다. `on_drag`를 타이틀바 없는 `Content`와 짝지으면 안 된다.
   워크벤치의 `TitleBar` 생성부에 그 이유를 주석으로 남긴다.
2. **`capture_event()`로 pane_grid를 막을 수 있다고 가정하지 않는다.** 호출은 계속 한다
   (조상을 막고 의도를 문서화한다) — 다만 설계가 그것 없이도 옳아야 한다.
3. **`scrollable` 래퍼를 제거한다**(`workbench.rs:75`).
4. **`.spacing(4).on_resize(0, ..)`로 바꾼다 — 침범이 0이 된다.**
   밴드는 `spacing + leeway`를 분할선 중앙에 놓고, 본문은 중앙에서 `spacing/2`부터 시작한다.
   따라서 **본문 침범량 = `(spacing+leeway)/2 - spacing/2` = `leeway/2`이고 `spacing`과 무관하다.**
   `leeway = 0`이면 밴드가 거터와 정확히 일치해 침범이 사라진다. 스파이크의 M1이 이를 실측으로
   보여준다(`spacing 2` + `leeway 0` ⇒ 밴드 399..401, 본문은 401부터).
   `spacing`은 **잡기 좋은 폭**을 위해 올린다 — 4px면 충분하고, 침범 없이 4px 타깃을 얻는다.
   (스파이크 보고서는 "`spacing > 0`인 한 침범을 없앨 수 없다"고 결론지었으나 이는 자기
   데이터와 어긋난다. 침범은 `leeway`만의 함수다.)
5. 터미널 선택은 자체 press→move→release 상태기계를 갖고, 같은 `CursorMoved` 스트림에서
   pane_grid가 동시에 분할을 리사이즈하는 상황을 견뎌야 한다.

### 2.3 잔여 충돌 — 규칙 4로 해소된다

**증상**: leeway 밴드 안에서 누르면 분할 리사이즈가 시작되는데, 터미널도 그 press와 이후 모든
`CursorMoved`를 **먼저** 받으므로 텍스트 선택을 동시에 시작한다. 터미널은 `is_event_captured`로
방어할 수 없다 — 먼저 실행되기 때문이다. 즉 분할선 근처 클릭이 터미널 커서를 옮기면서 동시에
분할을 끄는 것으로 보인다.

**해소**: 규칙 4(`leeway = 0`)로 겹침 영역 자체가 사라진다. 밴드가 거터와 일치하고 거터에는
본문이 없으므로, 리사이즈를 시작하는 press는 어떤 터미널 본문에도 속하지 않는다.
`leeway > 0`을 남기기로 한다면 대신 (b) 터미널이 분할 인접 모서리 `leeway/2` 이내의 press를
무시하거나, (c) `.on_resize`를 빼고 리사이즈를 키보드/커맨드로만 노출한다.

### 2.4 검증 등급 — 항목마다 다르다

| 주장 | 등급 | 근거 |
|------|------|------|
| C1 자식이 먼저 받는다 | **CONFIRMED (런타임)** | pane_grid는 grid bounds 위 press에서 무조건 캡처한다. 자식이 `captured=false`를 본다는 것이 곧 자식이 먼저 돌았다는 증명. M2가 이 단언을 죽인다 |
| C2 캡처가 pane_grid를 막지 못한다 | **CONFIRMED, 단 한 단계 약함** | §아래 |
| C3 본문 드래그는 pane을 집지 않는다 | **CONFIRMED (런타임)** | M3(시작점을 타이틀바로)이 `[Picked, Canceled]`를 내며 테스트를 죽인다 — 본문/타이틀바를 실제로 가른다 |
| C4 picked 중 본문 update 생략 | **CONFIRMED (런타임)** | 대조군 있음: 집히지 않은 pane은 같은 시퀀스에서 계속 이벤트를 받는다. 이게 없으면 "아무 이벤트도 안 왔다"와 구별 불가 |
| C5 밴드 = spacing+leeway, 중앙정렬 | **CONFIRMED (기제 mutation)** | M1이 `leeway 8→0`으로 밴드를 줄이자 테스트가 FAIL |
| C6a pane_grid는 휠을 무시 | **CONFIRMED (런타임)** | |
| C6b scrollable이 둘째 휠을 삼킨다 | **CONFIRMED (런타임)** | M5(`Scrollable→Bare`)가 죽인다 |
| C6c 트랜잭션이 1500ms | **UNVERIFIED** | `Instant::now()` 기반, 시계 주입 불가. 존재와 삼킴만 확인 |

**C2의 구조적 한계 (묻지 말 것)**: `a_child_capturing_...`에서 M6은 *sanity* 단언이 죽이지,
하중을 받는 단언("Resized가 여전히 발행된다")을 죽이지 못한다. 캡처 플래그를 뒤집어도 그
단언은 죽을 수 없다 — "플래그가 아무 차이도 안 만든다"가 바로 C2의 주장이기 때문이다.
우리 테스트 코드의 어떤 mutation도 "pane_grid가 캡처를 무시한다"와 "우리 테스트가 캡처를
못 본다"를 가를 수 없다. iced를 mutation할 수 없기 때문이다.
완화 근거 둘: (i) 테스트가 자기대조적이다 — `capture=true`/`false` 양쪽을 돌려 발행 메시지
목록이 **같음**을 단언한다. pane_grid가 캡처를 존중한다면 이 등식이 깨진다. (ii) scrollable
휠 테스트가 **이 하네스로 억제를 관측할 수 있음**을 보인다(M5). 따라서 하네스가 억제 일반에
눈먼 것은 아니다.
→ CONFIRMED로 취급하되, C1/C3/C4/C5보다 한 단계 약하다는 사실을 계획에 남긴다.

### 2.5 부수 발견

**pane_grid는 커서가 근처에도 없는 pane에까지 모든 이벤트를 뿌린다.** 로그에 `over=false`
항목이 남는다. 자식이 스스로 bounds로 걸러야 한다 — 우리 위젯의 `update` 진입부 책임이다.

---

## 3. `suaegi-term` 표면 — 있는 것과 없는 것

### 3.1 이미 있는 것 (Plan 3이 버리고 있을 뿐)

```rust
// crates/suaegi-term/src/grid.rs:29-56
pub struct SnapshotCell {
    pub c: char,
    pub combining: Vec<char>,   // zero-width 결합 문자
    pub fg: Color,              // alacritty_terminal::vte::ansi::Color
    pub bg: Color,
    pub flags: Flags,           // alacritty_terminal::term::cell::Flags
}
pub struct SnapshotCursor { pub row: usize, pub col: usize, pub shape: CursorShape } // 뷰포트 좌표
pub struct TerminalSnapshot {
    pub rows: Vec<Vec<SnapshotCell>>,
    pub size: GridSize,
    pub cursor: Option<SnapshotCursor>,   // 뷰포트 안에 있을 때만 Some
    pub display_offset: usize,
    pub history_size: usize,
}
```

`lib.rs`에 re-export가 없다 — 소비자는 alacritty_terminal에 직접 의존한다(suaegi-app은 이미 그렇다).
커서 **가시성은 따로 읽을 필요가 없다**: `RenderableCursor::new`가 `!SHOW_CURSOR`일 때
`CursorShape::Hidden`을 넣는다(`alacritty term/mod.rs:2373-2388`).

### 3.2 `TerminalSession` 공개 API (`session.rs`)

| 메서드 | 성질 | 비고 |
|--------|------|------|
| `start(SessionSpec) -> Result<Self>` | 블로킹(fork/exec) | |
| `snapshot() -> TerminalSnapshot` | **블로킹** — `FairMutex<Term>` + 뷰포트 전체 복사 | 이미 워커 스레드로 뺐다 |
| `write(Vec<u8>) -> bool` | **논블로킹** (`try_send`) | **bool은 "큐에 들어갔다"이지 "썼다"가 아니다.** false = 입력 유실(큐 상한 256) |
| `resize(u16,u16) -> Result<()>` | 블로킹 | `rows==0 \|\| cols==0`이면 **아무것도 안 하고 Ok** |
| `scroll_display(i32)` | 짧은 블로킹 | `Scroll::Delta` 하드코딩 |
| `generation/exit_code/is_running` | **원자적**, lock-free | `exit_code`가 `running=false`보다 **먼저** 저장된다(순서 계약) |
| `take_title_changes() -> Vec<TitleChange>` | 짧은 블로킹 | 상한 256, 오래된 것부터 버림 |
| `kill() -> Result<KillOutcome>` | | |
| `Drop` | unix에서 **최대 2초** | 이후 detach |

### 3.3 없는 것 → 어디에 배선하나

| 없는 것 | alacritty API | 배선 위치 | 비용 |
|---------|---------------|-----------|------|
| **`TermMode`** | `RenderableContent.mode` (`term/mod.rs:2399`) | `grid.rs:189-193`에서 `content.mode`를 집어 스냅샷 필드로 | **추가 락 0** — 이미 계산해놓고 버린다 |
| 선택 영역(렌더용) | `RenderableContent.selection` (`:2395`) | 같은 블록 | **그리드 좌표 → `display_offset` 보정 필수**(행·커서에 이미 하는 것과 동일) |
| 선택 변경 | `Term::selection`이 **공개 필드**(`:275`), `Selection::new/update` | `TerminalGrid`에 start/update/clear 추가, generation bump | |
| 선택 텍스트 | `Term::selection_to_string()` (`:529`) | `TerminalGrid::selection_text()` | |
| 좌표 변환 | `viewport_to_point` (`:131`) | **락 안에서** 변환하는 메서드로. 위젯 쪽에서 스냅샷의 offset을 쓰면 그 사이 스크롤 시 **레이스** | |
| 커서 블링크 | `Term::cursor_style().blinking` (`:942`) | `SnapshotCursor`에 필드 추가, `grid.rs:183`의 같은 락 안에서 | |
| `Scroll` 전체 | `Scroll::{Delta,PageUp,PageDown,Top,Bottom}` | `grid.rs:254`의 하드코딩을 넓힌다 | shift+PgUp/PgDn, "키 누르면 맨 아래로"에 필요 |
| 팔레트(OSC 4/10/11) | `RenderableContent.colors` (`:2398`) | 빌림이라 **락 안에서 복사**해야 한다 | 동적 색 변경을 지원할 때만 |
| damage 추적 | `Term::damage()` (`:458`) | 더티 행만 복사 | **소비자가 하나뿐이어야 한다**(상태 있음). follow-ups 6번과 연결 |

**필드 추가 시 깨지는 호출부는 둘이다** (조사 1차에서 `blank_snapshot()` 하나라고 적었던 것은
**틀렸다** — Codex 교차검증이 잡았고 `grep -rn 'TerminalSnapshot *{' crates/`로 확인했다):

- `crates/suaegi-app/src/session_store.rs:120` — `blank_snapshot()`
- `crates/suaegi-app/src/state.rs:1077` — `snapshot_with_text()` (테스트 헬퍼)

구조체를 바꾸기 전에 그 grep을 다시 돌린다.

**함정**: `GridSize::history_size()`가 0이다(`total_lines() == screen_lines()`). 무해하다 —
`GridSize`는 `Term::new`/`resize`에만 가고, 스냅샷의 `history_size`는 `term.grid()`에서 읽는다.
**"고치지" 말 것.** `Term::resize`가 `screen_lines()`/`columns()`를 읽는다.

### 3.4 현재 앱 상태

- 렌더 경로 전체가 `snapshot_text(id)` → `text()` 위젯 하나(`workbench.rs:74`).
- **키 입력 경로가 없다.** `TerminalSession::write`를 부르는 곳이 저장소에 하나도 없다
  (grep은 테스트의 무관한 `Hasher::write`만 잡는다). 입력 쪽은 백지에서 시작한다.

---

## 4. alacritty_terminal 0.25.1 참조

### 4.1 `TermMode` (`term/mod.rs:53-88`)

`SHOW_CURSOR`(1), `APP_CURSOR`, `APP_KEYPAD`, `MOUSE_REPORT_CLICK`, `BRACKETED_PASTE`,
`SGR_MOUSE`, `MOUSE_MOTION`, `LINE_WRAP`, `LINE_FEED_NEW_LINE`, `ORIGIN`, `INSERT`,
`FOCUS_IN_OUT`, `ALT_SCREEN`, `MOUSE_DRAG`, `UTF8_MOUSE`, `ALTERNATE_SCROLL`, `VI`,
`URGENCY_HINTS`, `DISAMBIGUATE_ESC_CODES`, `REPORT_EVENT_TYPES`, `REPORT_ALTERNATE_KEYS`,
`REPORT_ALL_KEYS_AS_ESC`, `REPORT_ASSOCIATED_TEXT`.
합성: `MOUSE_MODE = MOUSE_REPORT_CLICK|MOUSE_MOTION|MOUSE_DRAG`, `KITTY_KEYBOARD_PROTOCOL = ...`.

**kitty 프로토콜은 꺼져 있다.** `Config::kitty_keyboard`가 기본 `false`이고 `TerminalGrid::new`
(`grid.rs:150`)는 `scrolling_history`만 덮어쓴다. → **레거시 xterm 인코딩만 구현하면 된다.**

### 4.2 `Flags` (`term/cell.rs:12-37`, u16)

`INVERSE, BOLD, ITALIC, BOLD_ITALIC, UNDERLINE, WRAPLINE, WIDE_CHAR, WIDE_CHAR_SPACER, DIM,
DIM_BOLD, HIDDEN, STRIKEOUT, LEADING_WIDE_CHAR_SPACER, DOUBLE_UNDERLINE, UNDERCURL,
DOTTED_UNDERLINE, DASHED_UNDERLINE, ALL_UNDERLINES`.

**렌더러 계약**: `WIDE_CHAR_SPACER`와 `LEADING_WIDE_CHAR_SPACER` 셀은 **건너뛴다**
(앞선 wide char의 자리를 중복 표현). `WIDE_CHAR`는 **두 칸**을 차지한다.
`iced_term`은 이걸 통째로 빠뜨렸다 — CJK가 한 칸에 뭉갠다.

### 4.3 색 (`vte/src/ansi.rs:1013-1128`)

`Color::{Named(NamedColor), Spec(Rgb), Indexed(u8)}`.
`NamedColor`: 0..7 기본, 8..15 bright, `Foreground=256/Background=257/Cursor=258`,
`DimBlack=259..DimWhite=266`, `BrightForeground=267`, `DimForeground=268`.
`Indexed(u8)`는 표준 xterm 256색: 0-15 명명색, **16-231 = 6×6×6 큐브(`16 + 36r + 6g + b`)**,
232-255 = 24단계 회색. 세 갈래 모두에 대한 팔레트 테이블을 우리가 공급해야 한다.
런타임 OSC-4 덮어쓰기는 `Term::colors()`에 있다.

### 4.4 선택 / 스크롤 / 커서

```rust
pub struct SelectionRange { pub start: Point, pub end: Point, pub is_block: bool }  // selection.rs:33
pub enum SelectionType { Simple, Block, Semantic, Lines }                            // :93
Selection::new(ty, location, side)  // :125
Selection::update(&mut self, point, side)  // :133
Term::selection_to_string(&self) -> Option<String>  // term/mod.rs:529
pub enum Scroll { Delta(i32), PageUp, PageDown, Top, Bottom }  // grid/mod.rs:73-79
pub enum CursorShape { Block, Underline, Beam, HollowBlock, Hidden }  // vte ansi.rs:828-844
```

`Point.line`은 **`Line(pub i32)`이고 스크롤백에서 음수**다(`index.rs:136`).
의미 경계 문자 기본값: `,│`|:"' ()[]{}<>\t` (`term/mod.rs:45`).

**`HollowBlock`은 alacritty가 적용해주지 않는다.** `Term::is_focused`는 공개 필드이고
`RenderableCursor`는 그걸 보지 않는다 — **렌더러가 직접** 언포커스 시 hollow로 그려야 한다.

### 4.5 키 인코딩 — 없다 (4중 확인)

(a) `lib.rs` 모듈 목록에 `input`/`keyboard`가 없다. (b) grep은 **인바운드**만 잡는다
(CSI `>u`/`<u` 파싱, keypad 모드 **설정**). (c) vte 0.15는 파서일 뿐 — `keyboard` 히트는 전부
`Handler` 트레이트 스텁. (d) 레지스트리에 `alacritty` 바이너리 크레이트 **자체가 없다**.
인코더는 `alacritty/src/input/keyboard.rs`의 `build_sequence`에 있고 라이브러리 의존이 아니다.

→ **우리가 쓴다.** `TermMode`로 분기해야 하는 것들:
APP_CURSOR(화살표 `ESC O A` vs `ESC [ A`), APP_KEYPAD, 수식자 파라미터(`ESC [ 1 ; <mod> <final>`),
Alt→ESC 프리픽스, Ctrl+letter→`0x01..0x1A`, Ctrl+`[ \ ] ^ _`, Ctrl+Space→NUL,
Home/End/PgUp/PgDn/Insert/Delete/F1-F12, Backspace `0x7F`,
Enter `\r`(LINE_FEED_NEW_LINE이면 `\r\n`),
**BRACKETED_PASTE → `ESC [ 200~ … ESC [ 201~`로 감싸고 페이로드에서 종료자를 제거**,
FOCUS_IN_OUT → `ESC [ I` / `ESC [ O`,
마우스 SGR `ESC [ < b ; x ; y M|m` (선호) / 레거시 X10 / UTF8 변형,
ALT_SCREEN+ALTERNATE_SCROLL → 휠이 스크롤 대신 화살표 반복.

**수식자 파라미터는 열거하지 말고 계산한다**: `1 + shift*1 + alt*2 + ctrl*4`.
`iced_term`은 ~100줄로 손으로 열거했다.

---

## 5. `iced_term` 0.8.0 — 참조 구현에서 가져올 것과 버릴 것

**타겟 버전이 iced 0.14.0 + alacritty_terminal 0.25.1로 우리와 같다.** 시그니처를 그대로 쓸 수 있다.
단 **의존하지는 않는다** — PTY/세션 계층이 우리 것과 충돌한다.

### 5.1 가져올 것

- **`Command` enum 경계**: 위젯은 순수하게 남아 커맨드를 발행하고, 백엔드가 `Term` 뮤텍스를 소유한다.
  우리가 원하는 모양과 일치한다.
- **평범한 타이핑은 `KeyPressed.text`, 제어·명명 키만 바인딩 테이블.** IME/데드키 정합성에 옳은 분리.
- `generate_bindings!` 매크로 — 테이블이 읽힌다.
- `iced_core::mouse::Click::new(pos, button, previous)` — 더블/트리플 클릭 타이밍을 대신 해준다.
- **픽셀 스크롤 나머지 누산기** — 트랙패드에 맞는 패턴:
  ```rust
  state.scroll_pixels -= y;
  let lines = (state.scroll_pixels / cell_height).trunc();
  state.scroll_pixels %= cell_height;   // 나머지 보존
  ```
- `viewport_to_point` 사용(직접 굴리지 않기).
- 배경 런 배칭 구조(단 flush 순서는 고쳐서).
- 리사이즈: `update`에서 `layout.bounds().size()`를 캐시와 비교해 변했을 때만 커맨드 발행.

### 5.2 버릴 것 — 실제 버그

| # | 버그 | 위치 |
|---|------|------|
| 1 | **`Ctrl+U`가 `\x51`('Q')** — `\x15`여야 한다. kill-line이 깨져 있다 | `bindings.rs:231,299` |
| 2 | **BRACKETED_PASTE 미구현** — 클립보드를 날것으로 쓴다. 개행 든 텍스트를 붙여넣으면 **실행된다**. 보안 이슈 | 전역 |
| 3 | 커서 아래 글자 반전을 `APP_CURSOR`로 게이팅 — APP_CURSOR는 커서 **키** 모드다. 일반 모드에서 글자가 안 보인다 | `view.rs:577` |
| 4 | 배경 런이 커서보다 **나중에** flush돼 커서를 덮는다 | `view.rs` |
| 5 | 마우스 리포트가 **버퍼 좌표**를 쓴다 — 스크롤백에서 음수 줄을 보낸다 | `view.rs:219`→`backend.rs:396` |
| 6 | macOS에서 `COMMAND`와 `CTRL` 혼용 → `Ctrl+화살표`가 아무것도 안 낸다 | `bindings.rs:190-193` |
| 7 | 수식자 출처 불일치(캐시된 값 vs 이벤트 값). 언포커스 중 `ModifiersChanged`를 버려서 캐시가 상한다 | `view.rs:334` vs `:348` |
| 8 | 수식자 **정확 일치** 비교 — CapsLock 하나만 껴도 조용히 무시된다 | `bindings.rs:121` |
| 9 | 휠 스크롤이 포커스 게이팅 밖에 있다 | `view.rs:158` |
| 10 | `cursor.position().unwrap()` | `view.rs:646` |

**미구현**: wide/CJK, `APP_KEYPAD`, Alt+문자 메타 프리픽스, 커서 모양·블링크,
언포커스 hollow 커서, 휠→SGR 리포팅, 마우스 모드 중 Shift로 로컬 선택 강제, 스크롤백 자체.

**성능 안티패턴**: 프레임마다 셀당 hex 문자열 파싱(`theme.rs:100`), 글리프당
`char::to_string()` 할당(`view.rs:595`), 동기화마다 그리드 **deep clone**(`backend.rs:506`),
hover마다 `RegexSearch` clone(`backend.rs:249`).

**견고성**: 팔레트 hex 오류·링크 열기 실패·구독 채널 닫힘에서 **`panic!`**.
그리고 터미널이 준 임의 URL을 `open::that`에 넘긴다(정규식이 `file://`, `ssh:`를 허용한다).

**주의**: `TermSize`를 `alacritty_terminal::term::test::TermSize`에서 가져온다 —
**테스트 모듈 타입을 프로덕션에서** 쓴다(`backend.rs:13`). 우리는 자체 `Dimensions` 구현을 쓴다
(이미 `GridSize`가 있다).

---

## 6. 실측이 필요한 것 (추측 금지)

1. `Paragraph::with_spans`의 `Advanced` 셰이핑 강제 비용 — 행당 1 draw call의 이득과 상계되는가.
2. 스냅샷 셀 복사 비용(`follow-ups.md` 6번). damage 추적을 도입할 값이 있는지는 **렌더
   벤치마크를 보고** 판단한다.
3. ~~`pane_grid` 스파이크~~ — 완료(§2). 남은 미검증 항목은 C6c(트랜잭션 지속시간)뿐이고,
   규칙 3(`scrollable` 제거)을 따르면 그 값이 무엇이든 우리에게 영향이 없다.
