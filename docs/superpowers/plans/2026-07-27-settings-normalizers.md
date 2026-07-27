# Plan — terminal-fonts + hook-command-source-policy + source-control-group-order (`suaegi-misc` 모듈 3개, 단일 PR)

조사: Explore 정찰(소스 3개·오라클 3개 통독 + 소비자·persistence 경로 추적 + 선행 `terminal_line_height` 포트 대조).
출처 `reference/orca/` = **v1.4.146-rc.0**. 소스 68L / 오라클 74L.
`hook-command-source-policy.ts`는 **내가 직접 통독해 F9/F10을 재확인**했다(API 모양을 결정하는 주장이라).

## 0. 배치 — 셋 다 `suaegi-misc`, 단일 PR
[[suaegi-misc-placement-rule]]: `terminal-fonts`는 import **0**, 나머지 둘은 **`import type`**(런타임 소거).
`js_ws` 불필요(공백 처리 없음), 외부 의존 0 → `[dependencies]` **계속 빈다**.
두 `import type`은 **모듈 로컬 Rust `enum`**이 된다(`ui_language.rs`·`usage_percentage.rs`가 이미 `types.ts`
문자열 유니온에 하는 처리) — **공유 `types` 모듈도, 크로스 모듈 import도 만들지 않는다**.
셋 다 "설정 정규화"라 **리뷰 프레임이 동일**하다("오라클이 무엇을 고정하지 **못하는가**") → 한 PR.
⚠ 다만 위험이 **두 모듈에 나뉘어 있다**(수치 + 보안) → 각 모듈 doc 헤더에 **⚠P1 표식**을 단다.

## 1. 계약 결정 — `terminal_fonts`

- **F1 — ⚠⚠ 최고 위험: `TERMINAL_FONT_WEIGHT_STEP`은 **아무 데도 쓰이지 않는다**.**
  본문은 `Math.min(900, Math.max(100, Math.round(x)))`가 전부다(`:14-17`) — `x / STEP * STEP`도, `%`도, 스냅도 **없다**.
  → **`normalize(550)`은 `550`이다.** `500`도 `600`도 아니다.
  ⚠ **오라클 입력 5개가 전부 100의 배수**(`10`·`1200`·`500`·`800`·`undefined`)라
  **스텝 스냅 구현이 5/5 통과**한다. 그리고 `export const ..._STEP = 100`이 클램프 함수 **한 줄 위**에 있어
  포터가 쓰기 딱 좋다. → **핀: `550` → `550`, `449` → `449`.**
  ⚠ **생산에서 도달한다**(학술적이지 않다): `persistence.ts`에 `terminalFontWeight` 새니타이저가 **없고**,
  Ghostty 임포터가 `[100,900]`의 **임의 유한값**을 반올림 없이 그대로 쓰며,
  `NumberField`의 커밋 경로도 클램프만 하고 **스텝 스냅을 하지 않는다** → `550`이 그대로 저장돼 매 렌더에 도달한다.
- **F2 — ⚠⚠ `Number.isFinite` 가드가 **보이지 않는다**, 그리고 `terminal_line_height`보다 **더 위험하다**.**
  가드는 `DEFAULT`(**500**)를 반환한다(`:10-12`) — `MIN`이 아니다.
  가드 없는 Rust `MAX.min(v.round().max(MIN))`은 `NaN`→**100**, `+∞`→**900**, `-∞`→**100**.
  `v.round().clamp(100.,900.)`은 `NaN`→**NaN**(그리고 경계가 NaN이면 **패닉**).
  ⚠ 오라클의 폴백 픽스처가 **`undefined` 하나뿐**이라 `Option<f64>` 포트는 **`None` 팔에서 답하고
  `is_finite`를 아예 실행하지 않는다**.
  ⚠ `terminal_line_height`에선 가드 없는 NaN 결과가 **우연히 폴백과 같았지만**(둘 다 1),
  여기선 **100 vs 500로 다르다** → NaN 핀이 **최대로 판별력 있는데 존재하지 않는다**.
  → **핀 3개 필수: `NaN`·`+∞`·`-∞` → 전부 `500`.** `f64::clamp` **금지**.
- **F3 — `Math.round`가 **한 번도 목격되지 않는다**.** 오라클 입력 5개가 **전부 정수**라
  **반올림을 통째로 빼도 5/5 통과**한다(= `terminal_line_height` 본문 그대로).
  → 핀: `550.6`→`551`, `550.4`→`550`, `550.5`→`551`.
- **F4 — ⚠ JS `Math.round`는 **half toward +∞**, Rust `f64::round`는 **half away from zero**인데
  **여기선 완전히 등가**다.** 두 모드가 갈리는 건 **음수 half-integer뿐**이고(`-0.5`→`-0` vs `-1`),
  모든 음수 결과는 `< 100`이라 `max(100, …)`가 **둘 다 정확히 100으로 접는다**.
  bold 체인도 `n+200 ∈ [300,1100]`으로 전부 양수다. → **판별 입력이 존재하지 않는다.**
  **등가로 문서화하고 mutation 대상에서 뺀다**(원인 ②). 선례: `usage_percentage.rs:7-9`가 같은 논거를 기록.
- **F5 — ⚠ bold 체인의 **바닥과 델타가 서로를 가린다**(중복 메커니즘, 아홉 번째).**
  `fontWeightBold = min(900, max(700, n + 200))`(`:28-31`).
  오라클 픽스처 `resolve(500)` → bold `700`인데 **`500+200`이 바닥 `700`과 정확히 일치**한다.
  → 2/2를 통과하는 틀린 구현: **바닥 제거**(`min(900, n+200)`), **계단 함수**(`n>=700 ? 900 : 700`),
  **델타 `d ∈ [100,200]` 아무거나**(`+100`·`+150` 통과), **바닥 `B ≤ 700` 아무거나**(`0` 포함).
  → **`resolve(600)` → `{600, 800}`가 이 포트 전체에서 가장 레버리지 높은 핀 하나**다
  (계단 함수와 `+100`을 동시에 죽인다). 더해 `resolve(100)` → `{100, 700}`로 바닥을 고정.
  ⚠ 리터럴 `200`은 이름 없는 매직 넘버이고 `..._BOLD = 700`은 **모듈 private**이다.
- **F6 — `resolve`가 **유효하지 않은 입력을 한 번도 안 본다****(픽스처가 `500`·`800`).
  → `fontWeight` 필드에서 **정규화를 건너뛴 구현이 통과**한다.
  핀: `resolve(None)` → `{500, 700}`, `resolve(Some(1200))` → `{900, 900}`.
- **F7 — `DEFAULT`(500)와 `STEP`(100)은 **심볼 참조뿐**이다**(`MIN`/`MAX`는 `10`→`100`, `1200`→`900`으로
  **정확히 고정돼 있다**). → **DEFAULT·STEP 리터럴 핀**.
- **F8 — `Option<f64>`로 모델링**해 문자열 강제 변환을 **구조적으로 불가능**하게 만든다
  (`terminal_line_height.rs:8-11` 선례). `'500'`은 **폴백 경유로 우연히 500**이지만 `'800'`은 **500**이다.
  `u16`이 아니라 **`f64` 반환** — 반올림/클램프 산술이 관측 가능하게 남는다.

## 2. 계약 결정 — `hook_command_source_policy` ⚠ 보안

- **F9 — ⚠⚠ `'local-only'` 팔을 지우면 **green으로 통과하면서 명시적 옵트아웃을 뒤집는다**.**
  오라클(`test:13`)은 `resolve(undefined, {hasLocalScript:true})` → `'local-only'`로
  **`:21`의 기본 분기**만 탄다 — **`'local-only'` 문자열을 넘기는 픽스처가 하나도 없다**.
  → `:17`의 체인에서 `policy === 'local-only'`를 빼면 `resolve('local-only', …)`는 `:21`(`undefined` 아님)을
  지나 `:25`의 **`'shared-only'`**로 떨어진다.
  **결과**: 사용자가 "이 저장소의 커밋된 스크립트를 실행하지 말라"고 명시했는데 **실행하게 된다**
  (체크아웃된 repo의 `orca.yaml` `scripts.setup`·`defaultTabs[].command` 임의 실행, `main/hooks.ts:210-218,305-311`).
  → **핀: `resolve(Some("local-only"), false)`·`resolve(Some("local-only"), true)` → 둘 다 `LocalOnly`,
  그리고 `normalize(Some("local-only"))` → `LocalOnly`.**
- **F10 — ⚠ `undefined`와 `null`이 **다르다**(내가 소스에서 직접 확인).**
  `:21`은 `policy === undefined && hasLocalScript` — **엄격히 `undefined`**다.
  → `resolve(null, hasLocalScript=true)` = **`'shared-only'`**, `resolve(undefined, true)` = **`'local-only'`**.
  ⚠ 순진한 `Option<&str>`는 둘을 **접어버려** `null`에서 틀린다. 도달 가능하다(`new-workspace.ts:166`이
  `commandSourcePolicy?: unknown`이고 영속 JSON을 왕복한다).
  → **3-상태 입력**: `Option<Option<&str>>`(`None`=undefined, `Some(None)`=null/비문자열, `Some(Some(s))`=문자열).
- **F11 — `normalize`의 **유일한 픽스처가 폴백값을 기대한다**** → **상수 함수가 1/1 통과**한다.
  → 멤버 3개 각각 직접 핀(`native_chat_agent_support` 선례).
- **F12 — `'shared-only'` 팔은 **진짜 죽은 코드**다**(폴백과 값이 같아 두 체인에서 다 빼도 **전 입력 동작 동일**).
  → **핀으로 구별 불가**. **verbatim 유지 + doc 주석**으로 "단순화 패스가 지우지 못하게" 막는다(원인 ②).
- **F13 — trim/대소문자 음성 픽스처가 **없다**** → 과잉 정규화 포트가 **양쪽 오라클 100% 통과**한다.
  이 모듈에선 그게 **느슨해지는 방향**이다(`' run-both '`가 커밋된 스크립트를 돌리기 시작).
  → 핀: `'Shared-Only'`·`' local-only '` → 전부 폴백.

## 3. 계약 결정 — `source_control_group_order`
- **F14 — 폴백이 멤버 #1과 같다** → 체인에서 `'changes-first'`를 빼도 **전 입력 동작 동일**(F12와 같은 부류).
  → 등가로 문서화, 팔은 유지.
- **F15 — 멤버 3개 각각 + `DEFAULT` 리터럴 + 대소문자/trim 음성 핀.**

## 4. 오라클 & 핀
**오라클 전량**: `terminal-fonts.test.ts` 28L, `hook-command-source-policy.test.ts` 26L,
`source-control-group-order.test.ts` 20L.

**추가 핀(오라클 침묵 — 이 PR의 본체):**
**F1 `550`→`550`, `449`→`449`**(스텝 스냅 살해); **F2 `NaN`/`±∞` → `500`** ×3;
F3 `550.6`/`550.4`/`550.5`; **F5 `resolve(600)`→`{600,800}` + `resolve(100)`→`{100,700}`**;
F6 `resolve(None)`/`resolve(Some(1200))`; F7 `DEFAULT`·`STEP` 리터럴;
**F9 `local-only` 3종**; **F10 `Some(None)`(null) vs `None`(undefined) × `has_local_script`**;
F11 정책 멤버 3개; F13/F15 대소문자·trim 음성.

*mutation:* F1 스텝 스냅 도입, F2 가드 제거·`f64::clamp`로·폴백을 `MIN`으로, F3 반올림 제거,
**F5 바닥 제거·계단 함수로·델타 `+100`으로**, F6 `fontWeight`에서 정규화 생략, F7 상수 값 변경,
**F9 `'local-only'` 팔 제거**, F10 `Option<&str>`로 접기, F11 상수 함수로, F13/F15 `trim`/소문자화 추가.
**F4(반올림 모드)와 F12/F14(죽은 팔)는 mutation 대상 아님** — 등가 증명은 §1·§2·§3.

## 5. 순서
단일 PR. 헤더 모듈 수(현재 thirty-five)·목록·`Cargo.toml` 설명 반영(신규 3개는 **v1.4.146-rc.0**).
불변식: 셋 다 `suaegi-misc` + 로컬 `enum`(§0), **STEP 미사용**(F1), **가드 유지 + NaN 3핀**(F2),
반올림 유지(F3), 반올림 모드는 등가(F4), **bold 바닥·델타 개별 핀**(F5), `resolve`도 정규화(F6),
상수 리터럴(F7), `Option<f64>`(F8), **`local-only` 팔 보존**(F9), **3-상태 입력**(F10),
멤버 개별 핀(F11), 죽은 팔 유지(F12/F14), 과잉 정규화 금지(F13/F15), 매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[mutation-survivor-triage]], [[mutation-harness-mtime-trap]],
[[suaegi-misc-placement-rule]], [[orca-source-location]], [[suaegi-impl-model-sonnet]]
