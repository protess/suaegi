# Plan — terminal-view-attributes (단일 PR)

조사: Explore 정찰(188L 소스 + 119L 오라클 전문 정독). 대상: `crates/suaegi-term/src/view_attributes.rs` (신규).
소스 import 0개. **새 의존: `serde_json`**(워크스페이스 기존, suaegi-term엔 없음 — R2 근거).

## 0. 핵심 발견 — 기존 CSS 모듈과 **재사용 금지**
`reply_query/osc_color_reply.rs`의 `css_color_to_osc_rgb`는 **CSS 문법**, 이 모듈은 **XParseColor(xterm.js) 문법**이다.
**`#RGB`에서 결과가 정면으로 어긋난다:** `#abc` → XParseColor `[0xa0,0xb0,0xc0]`(`c << 4`) vs CSS `[0xaa,0xbb,0xcc]`(자릿수 복제).
또 CSS는 `rgb:h/h/h` 계열 전부 미지원 + `js_trim` 적용(이 모듈은 **trim 없음**).
→ **파싱 헬퍼 재사용 절대 금지.** 재사용 시 (a) `rgb:` 문법 소실 (b) `#abc` 값이 조용히 변경 (c) 없어야 할 trim 발생.

## 1. 계약 결정

- **R1 — 정규식 대신 손코딩.** 소스는 `X_RGB_SPEC_RE`(4-way alternation) + `X_HASH_SPEC_RE`를 쓰지만 JS `\d`=ASCII,
  Rust `\d`=Unicode Nd라 그대로 옮기면 `٣` 같은 문자가 통과한다. 4개 분기는 **자릿수로 상호배타**라 `len` 분기 +
  `is_ascii_hexdigit()` 검사로 자명하게 손코딩된다. **하우스 선례**: `osc_color_reply.rs:141-255`가 같은 이유로
  정규식을 전부 손코딩했다. (lookaround는 원본에 **0개**라 regex 자체는 가능하지만 손코딩이 더 안전·일관.)
- **R2 — `validate_terminal_view_attributes`를 이식한다(입력 `&serde_json::Value`).** 이 함수는 Electron IPC가
  무타입이라 존재하지만, **오라클 10 케이스가 실제 검증 규칙**(채널 범위 0..=255, 정수성, 팔레트 길이 256,
  enum 멤버십, `cursorBlink`의 **`typeof` bool** 검사 → 숫자 `1`은 truthy여도 **거부**)을 잠근다. 그 규칙을 버리면
  오라클 절반이 죽는다. → `serde_json = { workspace = true }` 추가.
- **R3 — 두 경로의 스케일링 비대칭을 **그대로** 보존한다.**
  - `rgb:` 경로: 자릿수 n∈{1,2,3,4} → base∈{15,255,4095,65535}, `round(v / base * 255)` (**나눗셈+반올림**).
  - `#` 경로: `adv=len/3`∈{1,2,3,4} → `c<<4` / `c` / `c>>4` / `c>>8` (**시프트=절단**).
  → **`rgb:f/f/f` = 255 이지만 `#fff` = 240**. 직관에 반하지만 xterm.js의 실제 비대칭이다. **통일하지 말 것.**
  ⚠ 정확한 `.5` tie는 수학적으로 존재하지 않는다(base 4095/65535에서 좌변 짝수·우변 홀수라 불가) →
  `f64` 경로와 정수 경로가 전 입력에서 일치. `f64::round()`(half-away-from-zero) 사용, 값이 항상 ≥0이라
  JS `Math.round`(half-toward-+∞)와 동일. 선례 주석: `osc_color_reply.rs:257-262`.
- **R4 — 세 채널의 자릿수는 반드시 동일.** 각 alternation이 자체 앵커+고정폭이라 **`rgb:f/ff/fff`는 거부**된다
  (실제 X11 XParseColor는 혼합폭을 허용하지만 이 모듈은 **xterm.js를 미러링**). **오라클에 없음 → 핀 필수.**
- **R5 — `#` 경로의 길이 게이트는 정규식과 **분리**돼 있다.** `^[0-9a-f]+$` 통과 **AND** `len ∈ {3,6,9,12}`.
  그래서 `#abcd`(길이 4)는 hex로는 유효하지만 **거부**(오라클 `test:40`). 두 조건을 합치지 말 것.
- **R6 — `"0"` 채널 truthiness 함정.** 소스 `:55`/`:57-59`는 **미참여 캡처가 `undefined`(falsy)**임을 이용해 분기하는데,
  참여한 캡처 `"0"`은 **truthy**다(빈 문자열이 아니므로). 캡처를 먼저 숫자로 바꿔 `!= 0`으로 분기하면
  `rgb:0/8/f`가 깨진다(오라클 `test:22`가 잠금). Rust는 `Option<&str>`로 자연히 안전 — **숫자 변환 후 분기 금지**.
- **R7 — trim 없음.** 이 모듈은 입력을 **전혀 trim하지 않는다** → `" rgb:f/f/f"`는 **거부**.
  ⚠ 인접 `osc_color_reply.rs:98`은 `js_trim`을 쓴다 — **교차 오염 주의**. 실수로 끼워넣으면 문법이 넓어진다. 핀 필수.
- **R8 — 대소문자는 `to_lowercase()`(full Unicode)로 축자 이식.** 소스 `:50`은 `String.prototype.toLowerCase()`이고
  정규식에 `/i` **없다**([[js-lowercase-two-mechanisms]] — 두 메커니즘 혼동 금지).
  a-f/0-9로 폴딩되는 non-ASCII 문자는 존재하지 않아 accept/reject 결과는 `to_ascii_lowercase`와 동일하지만,
  **계약을 문자 그대로** 유지한다. 입력 타입은 **`&str`**(`&[u8]`로 가면 이 계약을 포기해야 함).
- **R9 — 출력은 byte-exact 18바이트 `rgb:xxxx/xxxx/xxxx`, 전부 소문자, 채널당 4자리.**
  `{:02x}` → 그 2자리를 **두 번 이어붙임**. `[0,8,255]` → `"rgb:0000/0808/ffff"`(0 채널이 `0000`).
  **포맷터는 4줄 복제한다(공유 금지).** `osc_color_reply.rs:136-139` `rgb_channel_to_word`가 바이트 동일하지만,
  공유하면 **CSS 모듈 리팩터가 XParseColor 답장 바이트를 흔든다**. byte-identity가 하드 요구사항이므로
  모듈 독립성을 택하고, 양쪽에 "바이트 동일 유지 필요, 그러나 의도적으로 독립" 상호 주석을 남긴다.
  ⚠ 같은 파일의 `byte_hex_to_word`(`:129-133`)는 **원본 대소문자를 보존**해 `#ABC`→`rgb:AAAA/...`를 만든다 —
  **절대 쓰지 말 것**(대문자 답장은 byte-identity 위반).
- **R10 — 타입 설계.** `TerminalViewRgb = [u8; 3]`. `TerminalViewCursorStyle`은 **독립 3변형 enum**
  (`Bar`/`Block`/`Underline`) — alacritty `CursorShape`(5변형, `grid.rs:11`)에 매핑하지 말 것(집합이 다르고
  `HollowBlock`/`Hidden`의 행선지가 오라클에 없다). 검증 후 구조체의 `ansi`는 **`[TerminalViewRgb; 256]`**
  (길이 검증은 validator 안에 살아 있으므로 오라클 `test:78` "16개 팔레트 거부"가 그대로 유효).
  `format_x_color_rgb_spec`은 `[u8;3]`을 받아 JS의 `value>255` 8자리 출력 경로를 **구조적으로 배제**(의도적 강화, 주석).

## 2. 마일스톤 (단일 PR)
`crates/suaegi-term/src/view_attributes.rs` + `lib.rs` 선언/re-export + `Cargo.toml`에 `serde_json`.
**위치는 크레이트 루트** — `reply_query/mod.rs:1-12`가 그 모듈을 "byte-native 스캐너" 묶음으로 규정하는데
이 모듈은 스캐너가 아니라 **payload 계약 + color-spec 코덱**이다.
Export: `TerminalViewRgb`, `TERMINAL_VIEW_ANSI_COLOR_COUNT`, `TerminalViewCursorStyle`, `TerminalViewAttributes`,
`parse_x_color_spec`, `format_x_color_rgb_spec`, `terminal_view_attributes_equal`, `validate_terminal_view_attributes`.

**오라클(34케이스 전량):** parse accept 10(`rgb:` 4폭 + 대문자 `RGB:` + `#` 4폭 + `rgb:0/8/f`);
parse reject 6(`""`/`red`/채널부족/비-hex/`#abcd`/`rgbi:`); format 2; validate accept 1 + reject 10;
equal 동일 1 + 차이 감지 7(**`ansi[200]`** 포함 — 256 꼬리까지 비교함을 잠금).

**추가 핀(오라클 침묵):** R4 혼합폭 `rgb:f/ff/fff` 거부; R7 선행 공백 `" rgb:f/f/f"` 거부(**`js_trim` 대비**);
`"#"`/`"rgb:"` 단독 거부; `#abcdefghi`(길이 9 비-hex) 거부; R3 `#fff`=240 vs `rgb:f/f/f`=255 **대비 핀**;
R6 `rgb:0/0/0` 전 채널 0; R9 `[0,0,0]`→`"rgb:0000/0000/0000"`; R8 `RGB:FF/00/80` 대문자 경로;
CSS 문법이 **거부**됨(`rgb(1,2,3)`, `#abc`가 CSS와 다른 값) — 두 모듈 분리 증명.

*mutation:* R3 `#` 시프트를 자릿수 복제로(=CSS와 통일), `rgb:` 반올림 제거, R4 폭 일치 검사 제거,
R5 길이 게이트 제거, R6 `"0"`을 falsy 취급, R7 trim 추가, R8 `to_ascii_lowercase`(관측 동일 → 예상 SURVIVE, 문서화),
R9 `{:x}`(패딩 없음)·doubling 제거·대문자, R10 ansi 길이 검사 제거, equal에서 `ansi` 비교 생략.

## 3. Deferred
- **`terminal-view-attribute-responder.ts`**(OSC 4/10/11/12/104/110-112 핸들러 + WCAG relative luminance DSR ?996n) —
  xterm-headless parser 등록 API에 묶여 있어 alacritty `grid.rs` 경로와의 접합 설계가 선행돼야 한다. 별도 PR.
- 소비자 배선(renderer→main push) = 사람눈.

## 4. 순서
단일 PR. 불변식: 손코딩 파싱(R1), validator 이식(R2), **스케일링 비대칭 보존**(R3), 폭 일치(R4), 길이 게이트 분리(R5),
`"0"` truthiness(R6), **trim 없음**(R7), full-Unicode lower + `&str`(R8), byte-exact 출력 + 포맷터 복제(R9),
타입 설계(R10), 매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[js-lowercase-two-mechanisms]], [[suaegi-impl-model-sonnet]]
