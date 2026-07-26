# Plan — clipboard-text (M1 of 2; repo-icon은 별도 PR로 분리)

조사: Explore 정찰(clipboard-text 189L/155L + repo-icon 134L/130L 통독). 대상: `crates/suaegi-misc/src/clipboard_text.rs` (신규).
**새 의존 없음** — `suaegi-misc`의 무의존 헌장 유지.

## 0. 분할 결정 — `repo-icon`은 이번 PR에 넣지 않는다
정찰 권고를 채택한다. `repo-icon`은 `new URL`에 **3곳** 의존(`:22,:47,:71`)하고, 그 WHATWG 파싱은
**credential-confusion 방어**(`repo-icon.ts:51-52`)와 IDNA가 걸린 **보안 경계**라 손코딩이 위험하다.
→ **`url` 크레이트 도입 = `suaegi-misc` 무의존 헌장 변경**이 그 PR의 진짜 안건이다.
묶으면 무의존으로 깔끔히 끝나는 이 모듈이 헌장 결정의 인질이 된다. **M2에서 단독으로 다룬다**(§3).

## 1. 계약 결정

- **S1 (Q1) — `…WithYield` 4종은 **동기 함수 + 주입 콜백**으로 재형상화한다.**
  이 함수들은 장식이 아니라 실사용 경로다(`web-preload-api.ts:2479,2499`, `terminal-paste-coordinator.ts:104`).
  `suaegi-misc`는 무의존이라 async를 넣을 수 없다 → `yield_to_event_loop: &mut dyn FnMut()`를 받는 **동기** 함수로.
  이식 가능한 실제 내용은 **양보 케이던스 계산**이고 그건 그대로 살아난다(오라클 `test:66` "정확히 2회"가 검증).
  실제 이벤트루프 양보는 **호출자 책임**임을 모듈 doc에 명시. 선례: OSC 마일스톤의 C8(sink 클로저 주입).
- **S2 (Q2) — `max_bytes` 옵션은 `Option<f64>`로 받아 JS 수치 의미론을 충실 재현.**
  `Number.isFinite(v) && v > 0 ? floor(v) : fallback`(`:104-111`, `:113-120`).
  → `None`/`NaN`/`±Infinity`/`0`/음수 → **fallback**, `0.5` → `floor` → **0**(실효 한계 0 = 비어있지 않은 텍스트 전부 거절).
  **오라클이 이 두 함수를 import조차 안 한다**(전 분기 미검증) → **핀 필수**. 반환은 `u64`.
- **S3 (Q7) — 에러는 enum + `&str` 술어 **둘 다** 노출.**
  `isClipboardTextTooLargeError(error: unknown)`(`:166`,`:170`)는 **남의 에러**의 message에 `.includes`를 한다.
  타입 enum만 내놓으면 그 성질이 사라진다 → `ClipboardTextError` enum(선례 `stable_pane_id.rs:71-92`)
  **+ `is_clipboard_text_too_large_message(&str) -> bool`** / `..._write_..`. 오라클 `test:113-154`가 살아남는다.
- **S4 (T19) — 두 술어는 **서로소**다. `.includes` 부분문자열 매칭이며 **대소문자 구분**.**
  `'…too large for this paste target.'` vs `'…too large to copy safely.'` — 어느 쪽도 다른 쪽의 부분문자열이 아니다
  (오라클 `test:142-154`가 write 에러에 대해 read 술어가 **false**임을 못박음).
  ⚠ `remote_runtime_error.rs:87-90`은 **소문자화 후** 매칭한다 — **헬퍼 공유 금지**(여기는 case-sensitive).
- **S5 (T6/Q8) — 측정은 **UTF-8 바이트**, 조기중단 의미론은 증분 루프로 보존.**
  `getClipboardTextByteLength(text)` ≡ **`text.len()`**(잘 형성된 `&str`의 UTF-8 바이트 합).
  `isClipboardTextByteLengthOverLimit` ≡ **`text.len() > max_bytes`** — 원본의 `text.length > maxBytes`
  UTF-16 fast path는 **결과에 관측 불가**(임의 코드포인트에서 `utf8 ≥ utf16`이라 건전한 단축)이고
  JS spy 테스트만 그걸 봤다 → **Rust로 이식 불가, 주석으로 명시**.
  **단 `measure_*`는 증분 루프가 필요하다**: `stop_after_bytes` 초과 시 반환되는 `byte_length`가
  **초과분을 포함한 부분합**이기 때문(오라클 `test:33`이 `5`가 아니라 **`8`**로 못박음). `chars()` + `len_utf8()`.
  비교는 **엄격 `>`** — 정확히 한계값은 초과 아님(오라클 `test:39` `'éé'`/4 → false).
- **S6 — `stop_after_bytes`도 `Option<f64>` 의미론.** `Number.isFinite(v) && byteLength > (v ?? 0)`(`:29`).
  `None`/`NaN`/`Infinity` → **절대 멈추지 않음**, `0` → 첫 글자에서 즉시 중단, 음수 → 즉시 중단.
  (빈 문자열은 루프 미진입이라 `{0, false}`.)
- **S7 — yield 케이던스 축자.** `yield_after_code_units = max(1, opt ?? 262144)`(`:52-55`) — `??`라서 **`0` → 1**.
  `next_yield_at = index + cadence` — **`+=`가 아니라 현재 index 기준 재고정**(`:69-72`, astral에서 드리프트).
  검사는 **astral skip 이후** index로. 인덱스는 **UTF-16 unit** 기준이어야 오라클 "524289자 → 정확히 2회"가 맞는다
  → 루프에서 `ch.len_utf16()`으로 index를 전진시킬 것.
- **S8 (T5) — 이 모듈은 **어디서도 trim하지 않는다**.** `assert_*` 4종은 초과가 아니면 **입력을 verbatim 반환**
  (절단도 정규화도 없음). ⚠ 인접 모듈들이 `js_trim`을 쓰므로 교차 오염 주의 — **핀 필수**.
- **S9 — 에러 메시지에 **페이로드를 절대 넣지 않는다**.** 오라클 `test:113-124`가 `String(error)`에 payload 부재를
  명시 검증(metadata-only). 하우스 규율(`InvalidWriteId`)과도 일치.

## 2. 마일스톤 M1 (단일 PR)
`crates/suaegi-misc/src/clipboard_text.rs` + `lib.rs` 선언/re-export/모듈 목록 갱신. **의존 추가 없음.**
Export: 5 상수, `ClipboardTextByteLengthMeasurement`, `measure_clipboard_text_byte_length`,
`get_clipboard_text_byte_length`, `measure_..._with_yield`, `is_..._over_limit`, `is_..._over_limit_with_yield`,
`get_clipboard_text_read_max_bytes`, `get_clipboard_text_write_max_bytes`, `assert_*` 4종,
`is_clipboard_text_too_large_message` / `..._write_...`, `ClipboardTextError`.

**오라클(12케이스 전량):** `'a😀'`→5(UTF-8이지 UTF-16 아님); stop=5 → `{8,true}`(**부분합**);
`'éé'`/4 → false(**엄격 `>`**); 한계 내에서도 yield 발생; 524289 → yield **정확히 2회**;
`'é'×16`/31 → true(거절 **전에** yield); read/write 각자 메시지로 throw + 입력 verbatim 반환;
에러에 payload 부재 ×2; **두 술어 서로소**.
(spy 기반 fast-path 케이스 `test:82-93`은 Rust에서 관측 불가 — 이식 제외, 주석으로 근거 명시.)

**추가 핀(오라클 침묵):** S2 `get_*_max_bytes` 전 분기(`None`/`NaN`/`Infinity`/`0`/음수/`0.5`→0/정상값);
S6 `stop_after_bytes`의 `None`/`0`/음수/`NaN`; S7 cadence `0`→1; S8 **trim 안 함**(선행/후행 공백 보존,
U+FEFF 보존); S9 payload 부재; 기본 상수 16MiB ×2; `assert_*`가 정확히 한계값에서 통과.

*mutation:* S5 `>`→`>=`, 부분합 대신 중단 직전 합 반환, `len()`→`chars().count()`, S2 `>0` 가드 제거·`floor`→`ceil`,
S4 술어를 소문자화 매칭으로·두 메시지 통합, S7 `+=`로 변경·`max(1,..)` 제거, S8 trim 추가, S6 `isFinite` 가드 제거.

## 3. Deferred — `repo-icon`은 M2 별도 PR
**진짜 안건: `suaegi-misc` 무의존 헌장 변경 여부.** 정찰 Q4의 세 선택지 — (a) `url = "2"` 추가(헌장 변경),
(b) WHATWG host 파싱 손코딩(**보안 경계라 위험**), (c) 의존 허용 크레이트로 이관.
M2에서 단독 결정. 그 PR은 오라클이 얇아(**`faviconUrlFromWebsite` 커버리지 0**, 캡 경계 전부 미검증)
oracle-silent 핀을 대량으로 써야 한다: `/i` ASCII-only(U+212A) vs `toLowerCase()` full-Unicode(U+212A)가
**같은 파일에 공존**([[js-lowercase-two-mechanisms]]), emoji 16-unit 경계, src 409600, label 80 서로게이트 straddle,
`/.png` 거절, IDN 폴백, `source` 미trim, 빈 label 키 부재, `.host` vs `.hostname` 혼용.

## 4. 순서
단일 PR. 불변식: 동기+콜백 재형상화(S1), `Option<f64>` 수치 의미론(S2), enum+`&str` 술어 이중 노출(S3),
술어 서로소·case-sensitive(S4), 바이트 측정+부분합 보존(S5), stop 의미론(S6), 케이던스 축자(S7),
**trim 없음**(S8), payload 미포함(S9), 매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[suaegi-impl-model-sonnet]]
