# Plan — terminal detectors 클러스터 (bell/partial-escape-tail/scrollback/zero-dim) 확정

조사: 이번엔 리드가 소스를 직접 정독(Orca @ v1.4.150-rc.0, 인용 file:line 아래). **에이전트 디스패치가 이 세션
내내 고장**(background·sync 6+회 tool_uses=0 + 주입) → 리서치/Codex 서브에이전트 없이 리드가 직접 정찰·이식·
mutation 검증([[subagent-output-untrusted]]). 5모듈 중 **4개 이식, kitty 1개는 Codex 회복까지 defer**(사유 §2).

## 0. 결정 (소스 정독 결과)

터미널 output 인제스트-경계 detector 5모듈(전부 무-import 독립). **이번 이식 4개** → `suaegi-term` 신규 top-level
모듈:
- `partial_escape_tail.rs` ← `terminal-partial-escape-tail.ts`(149L, test 86L): VT500 파서-상태 스캐너, 청크
  경계 걸친 미완성 escape 꼬리 추출.
- `bell_detector.rs` ← `terminal-bell-detector.ts`(116L, test 26L): OSC 내부 BEL을 무시하는 stateful BEL 감지.
- `scrollback_policy.rs` ← `terminal-scrollback-policy.ts`(72L, test 62L): 순수 수치 정책(row 정규화/클램프/
  레거시 byte→row 버킷).
- `zero_dimensions.rs` ← `terminal-zero-dimensions-diagnostic.ts`(11L, test 16L): 진단 메시지 생성/매칭.

## 1. 결정/트랩 (구현자 필독)

- **D1 — byte-native `&[u8]` 안전(partial-escape-tail, bell-detector).** 두 파서가 검사하는 유의미 바이트는
  **전부 ASCII**(ESC 0x1b, CAN 0x18, SUB 0x1a, BEL 0x07, `[`0x5b, `]`0x5d, `\`0x5c, `P/X/^/_` 0x50/58/5e/5f,
  0x20–0x2f, 0x30–0x7e, 0x40–0x7e, 0x7f). UTF-8은 self-synchronizing — ASCII 바이트는 멀티바이트 시퀀스 내부에
  절대 안 나타난다(continuation 0x80–0xBF, lead 0xC0–0xFF). 따라서 `charCodeAt(i)`→`stream[i]:u8` 직역이
  **동치 + panic-proof**(`&[u8]` 슬라이스는 경계 무관 안전). `>= 0x80` 바이트는 두 파서 모두 "기타 바이트"로
  pass-through(ground 유지/시퀀스 내 유지) — 코드포인트 스캔과 **최종 상태·꼬리 동일**. `partial-escape-tail.ts:62`
  `charCodeAt`, `:141` `stream.slice(start)` → byte range 반환(`&[u8]`), `start=i-1`(oscEsc/stringEsc, i≥1 보장).
- **D2 — kitty-keyboard-mode-tracker는 DEFER(0x9b C1-CSI 인코딩 모호성).** `terminal-kitty-keyboard-mode-tracker.ts:82`
  regex가 `\x9b`(8-bit CSI, U+009B)를 CSI로 매칭. JS(UTF-16 문자열)에선 단일 code unit이나 **0x9b는 UTF-8
  continuation 바이트 범위(0x80–0xBF)** — byte-native로 raw 0x9b 매칭 시 정상 멀티바이트 문자 중간을 CSI로
  **오매칭**. 반대로 `&str` 이식은 caller 디코드 계약(UTF-8 strict/lossy/latin1?)에 의존 — Orca/suaegi 양쪽 caller
  인코딩 계약 미확인. 이건 Codex 교차검증이 값을 더하는 지점(지난 마일스톤서 seq-counter 오류 적발). **디스패치
  회복 후 별도 마일스톤**(스택 push/pop/set·screen-swap 47/1047/1049·RIS·DECSTR 상태기계도 subtle). 무리한 solo
  이식 = faithfulness 리스크 → 명시적 defer.
- **D3 — JS `unknown`은 `Option<f64>`로 모델(scrollback-policy).** `isFiniteNumber`(`:16`)= `typeof number &&
  Number.isFinite`. Rust: `None`=비-숫자/undefined(`'25000'`·`undefined`·`'garbage'` 케이스), `Some(x)`=JS number
  (NaN/Inf는 `is_finite()`로 걸러짐). `clampRows`(`:20`)= `min(MAX, max(min, floor(value)))` → `value.floor()
  .max(min as f64).min(MAX as f64) as i64`(NaN은 호출 전 필터되므로 cast 안전). 반환: row/cap은 `i64`, snapshot은
  `Option<i64>`. **버킷 경계 연산자 정확 재현**(`legacyTerminalScrollbackBytesToRows`: `bytes <= 1MB`(≤)·이후
  `bytes < BUCKET_*`(<) — `:59,62,65,68`). `bytes<=0`은 default(`:56`).
- **D4 — zero-dim `×`는 U+00D7**(`:6`), `format!("...({cols}×{rows})...")`. cols/rows `u32`. 매처는 리터럴
  prefix `startsWith("Terminal has zero dimensions (")` (`:10`).
- **bell-detector 상태**: closure 3-bool(`pendingEscape`/`inOsc`/`pendingOscEscape`) → `struct BellDetector` +
  `&mut self` 메서드. fast-path(`:38-45`)의 `hints.containsOscIntroducer ?? data.includes('\x1b]')` →
  `contains_osc_introducer: Option<bool>` + `unwrap_or_else(|| find \x1b])`. `data.endsWith('\x1b')`→`last()==Some(0x1b)`.

## 2. 마일스톤

### M1 — terminal detectors 4모듈 (`suaegi-term` top-level, 단일 마일스톤)
- `partial_escape_tail.rs`: `ScanState` enum(8), `extract_partial_escape_tail(&[u8])->&[u8]`,
  `advance_partial_escape_tail(pending, chunk)->Vec<u8>`(cap 4096), `MAX_PARTIAL_ESCAPE_TAIL_LENGTH`,
  `state_after_esc_byte`(private).
- `bell_detector.rs`: `struct BellDetector`(Default), `chunk_contains_bell(&mut self, &[u8], Option<bool>)->bool`,
  `reset`.
- `scrollback_policy.rs`: 12 pub const + `normalize_desktop_terminal_scrollback_rows(Option<f64>)->i64`,
  `terminal_output_backlog_cap_chars(Option<f64>)->i64`, `normalize_desktop_terminal_snapshot_rows(Option<f64>)
  ->Option<i64>`, `legacy_terminal_scrollback_bytes_to_rows(Option<f64>)->i64`, `clamp_rows`(private).
- `zero_dimensions.rs`: `create_terminal_zero_dimensions_message(u32,u32)->String`,
  `is_terminal_zero_dimensions_diagnostic(&str)->bool`.

**오라클(4 테스트 전부 이식):** partial-escape-tail(clean/dangling CSI/dangling OSC/ESC-aborts-CSI/CAN·SUB-abort/
**fold-safety** `extract(extract(a)+b)==extract(a+b)`/advance-accumulate/cap-abandon); bell(ANSI-skip/split-OSC-
terminator-not-bell/split-non-OSC-ESC-then-BEL=real); scrollback(defaults·presets/normalize-no-string-coercion/
snapshot-zero-preserve/backlog-cap-scale/legacy-bucket-intent); zero-dim(round-trip/no-false-match).

**추가 핀:** D1 non-ASCII byte-boundary 무-panic(멀티바이트 payload가 escape 꼬리에 안 섞임); D3 `Some(NaN)`→default·
버킷 경계값(정확히 1MB/17.5M/37.5M/75M)·`-1`→0 클램프; fold-safety를 한글/이모지 포함 케이스로도.

*mutation:* `floor`→`ceil`/제거, 버킷 `<=`↔`<` 스왑, clamp min/max 스왑, bell `inOsc` 상태 반전, partial-tail
CAN/SUB abort 제거, `start=i-1`→`i`, oscEsc `\`(0x5c) 종결 제거, MAX cap 값.

## 3. Deferred (명시)
- **kitty-keyboard-mode-tracker**(D2) — 0x9b 인코딩 계약 미해결, Codex 교차검증 후 별도 마일스톤.
- detector 소비자 배선(bell→알림, partial-tail→snapshot producer, scrollback→설정 UI, zero-dim→렌더러) = 사람눈.

## 4. 순서
M1 단일(4모듈 + 오라클 4 + D1·D3·D4 핀). 불변식: byte-native ASCII-safe(D1), Option<f64> finite 모델(D3), U+00D7(D4),
매 회귀 mutation 검증, kitty defer(D2). 관련: [[mutation-verify-regression-tests]], [[suaegi-workflow]],
[[subagent-output-untrusted]], [[suaegi-rustfmt-no-convention]]
