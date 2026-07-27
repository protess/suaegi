# Plan — opencode-terminal-title + agent-title-decoration (`suaegi-misc`에 모듈 2개, 단일 PR)

조사: Explore 정찰(소스 33L·오라클 58L 통독 + 정규식을 **Node에서 실제 실행**해 검증 + 소비자 전수 grep).
출처 `reference/orca/` = **v1.4.146-rc.0**. 소스 임포트 0, 외부 의존 0.

## 0. 배치 — `suaegi-misc`, `regex` 금지
[[suaegi-misc-placement-rule]] 그대로: 외부 의존 0 → **모듈 2개 추가**.
`js_ws` 인트라 크레이트 import는 **확립된 예외**다(18개 중 **6개**가 이미 쓴다: `image_data_uri`,
`stable_pane_id`, `command_token_scanner`, `process_output_field_scanner`, `codex_auth_errors`,
`harness_injected_user_turns`; 크레이트 헤더가 명시적으로 허용).
**`regex` 금지** — Rust `regex`의 `\s`는 유니코드 `White_Space`라 W1의 발산 8개 중 6개를 그대로 밟는다.
클래스를 손으로 다 적을 거면 정규식이 사 주는 게 0이다. 두 패턴 다 lookahead·역참조 불필요.
⚠ `suaegi-app/src/agent_status/title.rs`에 **같은 글리프 상수 5개와 braille 술어가 이미 있다**(`:41-45`, `:88-89`).
그쪽은 `suaegi_term`을 끌고 오고 `suaegi-app`은 `suaegi-misc`를 의존하지 **않으므로** 거기 두면 의존 방향이 역전된다.
→ **글리프 집합이 두 크레이트에 중복된다. 모듈별 복제는 이 리포 헌장이므로 정상**이고,
누가 "중복 제거"로 크로스 크레이트 간선을 만들지 않게 **주석에 한 줄** 남긴다.

## 1. ⚠⚠ W1 — 중심 함정: `\s`/`\S`/`trim`은 **ECMAScript** 집합
사이트 **8곳**: `opencode:6`의 `[^|\s]+`·`\s*`×2·`\S`, `opencode:9`의 `.trim()`,
`decoration:8`의 `\s`(필수 1개)·후행 `\s*`, `decoration:11`의 `.trimStart()`.
ECMAScript는 **U+FEFF 포함 / U+0085 제외**, Rust는 **정확히 반대**(`js_ws.rs:9-12`).
정찰이 Node로 실행해 발산 witness를 뽑았다 — 대표 4개:
`"\u{FEFF}OC | x"` → JS **true** / 순진한 Rust false; `"\u{0085}OC | x"` → JS **false** / Rust true;
`".\u{FEFF}x"` → JS `"x"` / Rust 무변화; `"✳\u{0085}Pi"` → JS `"\u{0085}Pi"` / Rust `"Pi"`.
→ **`suaegi_misc::is_js_whitespace` + `js_trim`**.
⚠ **`js_trim_start`는 리포에 없다**(`js_ws.rs`엔 `is_js_whitespace`와 **양끝** `js_trim`뿐).
→ `decoration` 모듈에 **로컬 2줄**로 만든다(`suaegi-quickcmd/src/lib.rs:435-437`의 `js_trim_end` 선례와 동형).
`js_ws`로 승격하지 **않는다** — 그러면 quickcmd의 `js_trim_end`까지 리팩터해야 해 diff가 커진다.

## 2. ⚠⚠ W2 — 등가 변이와 **죽일 수 있는 변이**가 한 쌍으로 붙어 있다 (비대칭!)
`x.replace(/^P\s*/,'').trimStart()`에서 `\s*`와 `.trimStart()`는 **같은 공백 집합을 문자열 선두에서** 지운다.
- **후행 `\s*` 삭제 = 등가 변이.** 모든 입력에서 결과 동일(P가 먹은 뒤 남은 선두 공백을 `trimStart`가 똑같이 먹는다).
  → **어떤 픽스처로도 죽일 수 없다.** SURVIVED가 떠도 **공허한 핀이 아니다**([[mutation-survivor-triage]] 원인 ②).
- **`.trimStart()` 삭제 = 죽일 수 있다.** P가 **매치 실패**하면 `\s*`는 아예 안 돌아 선두 공백이 남는다.
  witness: `"  npm run dev"` → 정답 `"npm run dev"`, `trimStart` 없는 포트는 `"  npm run dev"`.
  ⚠ 리포 전체에 그런 픽스처가 **0개**다.
→ 구현은 **둘을 `js_trim_start` 한 번으로 융합**하고(§4 스케치), 위 비대칭을 주석에 적는다.
필수 핀: `"  npm run dev"` → `"npm run dev"`, 그리고 `" ✳ Pi"` → `"✳ Pi"`(W4).

## 3. 계약 결정 — `opencode_terminal_title`

- **W3 — ⚠⚠ 멀티플렉서 파이프는 **리터럴 `" | "`**, 마커 파이프는 `\s*`다. 비대칭이 진짜다**(Node 실행 확인).
  `tmux | OC | x` **true** / `tmux  |  OC | x` **false** / `tmux\t| OC | x` **false** / `tmux |  OC | x` **false**.
  반면 `OC  |  x` **true**, `OC\t|\tx` **true**.
  ⚠ 오라클 픽스처가 공백 1개짜리 하나뿐이라 `" | "`·`\s*\|\s*`·`\s?\|\s?`가 **구별 불가**.
  ⚠ 둘을 "일관되게" 통일하는 게 **가장 나올 법한 조용한 동작 변경**이다. → false witness 3개를 핀.
- **W4 — 접두 그룹은 **재시도가 필요**하다.** `OC | x`에서 선택적 그룹이 **먼저 매치되고**(`OC` + `" | "`)
  그 뒤 `OC` 리터럴이 `x`에서 실패 → **그룹을 건너뛴 재시도**로만 성공한다.
  → 손코딩은 **두 갈래를 다 시도**해야 한다(순서는 자유). 한 갈래만 있는 포트는 헤드라인 픽스처를 깬다.
- **W5 — `\S`는 **아무 비공백 1자**다.** `OC||x`·`OC | |`·`OC | ✳` 전부 true.
  ⚠ 모든 양성 픽스처의 파이프 뒤 첫 글자가 **알파벳**이라 `[A-Za-z0-9_]`로 좁힌 포트가 **오라클 전량 통과**.
- **W6 — `OC`는 **대소문자 구분**이고 마커 경계가 정확하다**: `oc | …` false(핀 있음), `OCX | x` false(핀 없음).
  bare `OC |`도 false(핀 있음 — 주석의 주장이 실제로 고정돼 있다).
- **W7 — `[^|\s]+`의 `+` vs `*`는 **관측 불가**다.** 빈 토큰이려면 trim된 문자열이 `" | "`로 시작해야 하는데
  `.trim()`이 그걸 불가능하게 만든다. → **핀 낭비하지 말고** 주석에 남겨 향후 SURVIVED 오판을 막는다(원인 ②).
- **W8 — `isMeaningfulOpenCodeTerminalTitle`은 **순수 별칭**이다**(`:12-14`, 로직 추가 0).
  ⚠ **둘 다 살아 있다**(각각 소비자 4곳/2곳). 함수 참조로 넘기는 곳은 **없다** → `#[inline]` 위임으로 충분.
  이름 둘을 유지한다(정체성 판정 vs 표시 제목 보존, 소비자 코드가 두 이름을 다 쓴다).
- **W9 — nullish 사다리**: `title?.trim() ?? ''`. `undefined`/`null`/`''`/`'   '`이 **전부 `''`로 수렴**한다
  → `?? ''` 자체는 관측 불가. `Option<&str>`로 받는다(JS가 null/undefined를 구별 못 하므로 접는 게 충실).

## 4. 계약 결정 — `agent_title_decoration`

- **W10 — ⚠ **선-트림이 없다**.** `^`는 **원문 인덱스 0**에 묶인다(`:11`의 트림은 replace **이후**).
  → `" ✳ Pi"` → **`"✳ Pi"`**(글리프가 **살아남는다**). 먼저 트림하는 포트는 Orca에 없는 버그를 "고친다". 커버리지 0.
- **W11 — never-empty는 **원문 그대로**를 돌려준다**(`:18`, 트림본이 아니다).
  `"✳ "` → `"✳ "`는 핀이 있지만, `"  "` → `"  "`와 `"\u{2800}"` → `"\u{2800}"`는 **커버리지 0**.
  ⚠ U+2800(braille blank)은 **보이지 않는 글자**라 never-empty를 밟는다.
- **W12 — 클래스 멤버 침식**: `[✳✦⏲◇✋⠀-⣿]` = U+2733, U+2726, U+23F2, U+25C7, U+270B + braille U+2800–U+28FF.
  ⚠ 자체 오라클이 검증하는 건 **✳와 ⠋(U+280B)뿐**이고 ✦는 mobile 테스트에서만 나온다 →
  **⏲·◇·✋는 리포 전체에서 커버리지 0**(지워도 전부 통과). braille **양 끝점 U+2800/U+28FF 미검증**,
  **바로 바깥 U+27FF/U+2900도 비매치로 미검증**(off-by-one 범위 포트가 통과).
  → 멤버 5개 + braille 3점(양 끝점 + 내부) + 바깥 2점을 **각각** 핀.
- **W13 — `[.*]`는 **리터럴 2개**이고 `\s`는 **정확히 1개 필수**다**(와일드카드도 수량자도 아니다).
  `".x"`·`"*y"` **무변화**, `"*  y"` → `"y"`. 오라클은 **ASCII 공백 1개**로만 검증한다 → 탭/NBSP/음성 케이스 핀.
- **W14 — replace는 `/g`가 없고 `^` 앵커라 **치환 1회**다.**
  `[glyphs]+`는 greedy → `"✳✦✳ x"` → `"x"`(**한 매치**). 그러나 `"✳. ✦"` → **`". ✦"`**
  (`\s*`가 `.`을 못 먹고 끝 — 두 번째 장식은 **살아남는다**). 안정될 때까지 **반복하는 포트는 발산**한다. 둘 다 미검증.
- **W15 — 교대 순서는 무관**하다(두 갈래의 첫 글자 집합이 서로소) → Rust에서 순서 자유.
- **W16 — `/u` 유무는 **양쪽 다 무해**하다**(정찰이 실행 검증: 글리프·astral·lone surrogate·braille 전부 동일 출력).
  패턴 최대 멤버가 U+28FF(BMP)이고 `opencode` 쪽은 전부 ASCII 리터럴 + 클래스뿐이다.
  ⚠ 불일치를 "고치지" 말고, 여기서 숨은 유니코드 요구를 **추론하지 말 것**.
  `decoration:7`의 `no-control-regex` eslint-disable은 **낡은 주석**이다(클래스에 제어문자 없음) — 데이터로 취급.

## 5. 오라클 & 핀
**오라클 전량**: `opencode-terminal-title.test.ts` 11케이스, `agent-title-decoration.test.ts` 6블록.
간접 오라클도 핀으로: `terminal-title-agent-type.test.ts:57-67`(`OC | ✦ Gemini CLI`, `OC|compact-session`),
`mobile-terminal-tab-agent.test.ts:80`(**리포 유일의 ✦ 커버리지**), `:88`.
⚠ `tab-title-tooltip.test.tsx:88-91`의 정규식 사본은 **목(mock)이지 오라클이 아니다** — 이식하지 말 것.

**추가 핀(오라클 침묵 — 이 PR의 본체):**
**W2 `"  npm run dev"` → `"npm run dev"`**(trimStart 삭제를 죽이는 유일한 witness) + `" ✳ Pi"` → `"✳ Pi"`;
**W3 false witness 3개**(`tmux  |  OC | x`, `tmux\t| OC | x`, `tmux |  OC | x`) + true 대조군(`OC  |  x`, `OC\t|\tx`);
W5 `OC||x`·`OC | |`·`OC | ✳`; W6 `OCX | x` + 접두 2개(`a | b | OC | x`); W9 `null`·`''`·`'   '`;
W1 U+FEFF/U+0085 **8사이트 전부**; W10 선-트림 없음; W11 `"  "`·`"\u{2800}"`·`". "`·`"✳✦✳ "`;
**W12 멤버 5개 + U+2800/U+280B/U+28FF + U+27FF/U+2900 비매치**; W13 `".x"`·`"*y"` 무변화 + `"*  y"` + 탭;
W14 `"✳✦✳ x"` → `"x"` **그리고** `"✳. ✦"` → `". ✦"`.

*mutation:* W1 `char::is_whitespace`/`str::trim`/`trim_start`로, W3 접두를 `\s*`로 통일,
W4 재시도 갈래 제거, W5 `\S`를 alnum으로, W6 대소문자 무시·`starts_with("OC")`만,
W10 선-트림 추가, W11 `js_trim_start(title)` 반환·`stripped` 반환, W12 멤버 1개씩 제거·범위 ±1,
W13 `\s`를 `\s*`로·`[.*]`를 와일드카드로, W14 안정될 때까지 반복.
**W2의 후행 `\s*` 삭제와 W7의 `+`→`*`는 mutation 대상에서 제외**(등가 — §2·W7 증명 참조).

## 6. 순서
단일 PR. 두 모듈은 공유 코드가 0이지만 둘 다 `suaegi-misc` 소속·터미널 제목 의미론·`js_ws` 의존이라 한 diff가 맞다.
`js_trim_start`는 `decoration` 쪽에만 필요하다(`opencode`는 양끝 `js_trim`).
크레이트 헤더의 모듈 수(현재 eighteen)와 모듈 목록, `Cargo.toml` 설명을 같이 고친다(신규 2개는 **v1.4.146-rc.0**).
`pi-overlay-ui-settings`는 **다음 PR**(정찰 권고대로 `suaegi-misc` + `JsValue`/`JsRecord` 로컬 3번째 사본;
`2026-07-27-ui-prefs-normalizers.md` §0의 "자체 leaf 크레이트" 스케치를 **이걸로 대체**한다).
불변식: `suaegi-misc` + `regex` 금지(§0), **ECMAScript 공백**(W1), **`\s*` 등가/`trimStart` 죽일 수 있음**(W2),
**리터럴 `" | "` 비대칭**(W3), 재시도 갈래(W4), `\S` 광의(W5), 대소문자 구분(W6), `+`/`*` 등가(W7),
별칭 유지(W8), `Option<&str>`(W9), **선-트림 없음**(W10), 원문 반환(W11), **멤버 개별 핀**(W12),
리터럴 2개 + 공백 1개(W13), **치환 1회**(W14), 순서 자유(W15), `/u` 무해(W16), 매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[mutation-survivor-triage]], [[suaegi-misc-placement-rule]],
[[orca-source-location]], [[suaegi-impl-model-sonnet]]
