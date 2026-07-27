# Plan — emulator-touch-frame (`suaegi-misc` 모듈 1개, 단일 PR)

조사: `2026-07-27-native-chat.md`와 **같은 Explore 정찰**(3모듈 일괄) + 선행 바이너리 포트 2건의 함정 목록 대조.
출처 `reference/orca/` = **v1.4.146-rc.0**. 소스 18L / 오라클 15L. import 0, 외부 의존 0.
PR 2/2 — native-chat 2모듈은 #114로 머지. **이 모듈만 따로 보내는 이유**: 위험 표면이
**바이트 충실도**뿐이라 리뷰어 주의를 여기에 집중시켜야 한다(사소한 모듈에 묻는 게
앞선 바이너리 포트 2건에서 함정을 흘린 방식이다).

## 0. ⚠⚠ 내 전제가 틀렸다 — 이건 고정 레이아웃 바이너리가 **아니다**
나는 `DataView`/엔디언 함정을 예상했는데 **정규식도 `DataView`도 없다**. 실제 구조:

| offset | width | 내용 |
|---|---|---|
| 0 | 1 byte | 태그 리터럴 `0x03` (`:15`) |
| 1 | `n` bytes | `JSON.stringify(touch)`의 **UTF-8** (`:13,:16`) |

총 길이 **가변**(`1 + n`). 분기 0개, 루프 0개, throw 경로 0개.
→ **`suaegi-screencast`/`suaegi-termstream`의 반사신경 3가지가 전부 오발한다**(T11):
① `to_uint32`/wrap 헬퍼 — **정수 강제 변환이 아예 없다**;
② `to_le_bytes`/`to_be_bytes` — 와이어에 나가는 정수가 **1바이트 태그 하나뿐**이라 엔디언 개념이 없다;
③ `serde_json` — **디코더가 이 리포에 없다**(외부 `serve-sim@^0.1.40` 프로세스가 푼다). **인코드 전용**이다.
⚠ 좌표는 부호/폭이 있는 정수가 아니라 **IEEE-754 f64를 JSON 십진 텍스트로** 낸다.

## 1. ⚠⚠ E1 — 헤드라인: ECMAScript 수치 포맷, 그리고 `0.25/0.75`가 그걸 가린다
`JSON.stringify`는 ECMAScript `Number::toString`을 쓴다. Rust `ryu`/`serde_json`과 **다르다**:

| 입력 | JS | Rust `ryu` |
|---|---|---|
| `1` | `1` | **`1.0`** |
| `0` | `0` | **`0.0`** |
| `-0` | `0` | **`-0.0`** |
| `3`(edge) | `3` | **`3.0`** |
| `NaN`/`±∞` | **`null`** | 에러 |
| `1e21` | `1e+21` | `1e21` |

⚠ **유일한 오라클 픽스처가 `x: 0.25, y: 0.75`**(`test:6`)인데 둘 다 **짧은 십진 표기를 갖는 정확한 이진 분수**라
두 포맷터가 **바이트 동일**하게 찍는다 → `serde_json::to_vec` 포트가 **100% green이면서 바이트 발산**한다.
게다가 오라클이 **파싱 후 `toEqual`**(`test:9-13`)이라 `"2.5e-1"`을 내도 통과한다.

**발산이 생산에서 실제로 도달한다**(가설이 아니다):
`clampUnit`(`emulator-screen-gesture.ts:45-47`)이 끝점에서 **정확히 `0`/`1`**을 반환 → **화면 가장자리 터치마다**;
`HID_EDGE_BOTTOM = 3`(`:42`)이 **유일하게 생산되는 edge 값**이고 정수 → **엣지 스와이프마다**.
받는 쪽은 **제3자 바이너리**(`serve-sim`)라 관용도를 이 리포에서 **검증할 수 없다**.
→ **`format_ecmascript_float`를 모듈 private으로 복사**(4번째 복사; `suaegi-mcp/src/json.rs:266`,
`suaegi-screencast/src/lib.rs:455-501`, `suaegi-termstream/src/lib.rs:488` 선례) + **비유한 → `null`**
(`termstream:375-380` 방식). 크로스 크레이트 재사용 금지(전부 private이고 모듈별 복제가 헌장).
→ **핀은 파싱값이 아니라 정확한 바이트**로: `x:1.0`, `x:0.0`, `x:-0.0`, `edge:3.0`, `x:NaN`, `x:1e21`.

## 2. ⚠⚠ E2 — 키 순서는 **호출자가 정하고** 오라클은 순서맹이다
`JSON.stringify`는 **삽입 순서를 보존**하는데, 생산이 **두 가지 순서**를 쓴다:
- **`x, y, type[, edge]`** — 렌더러/라이브 터치 경로(`emulator-screen-gesture.ts:62`,
  `emulator-device-frame.tsx:133,150,313,315`의 `{...point, type}`)
- **`type, x, y[, edge]`** — CLI 경로(`cli/handlers/emulator.ts:112`)

타입 선언(`:4-7`)과 **오라클 픽스처 둘 다** 세 번째 프레이밍(`type, x, y`)이다.
**Rust 구조체는 순서가 하나뿐이라 둘 다에 충실할 수 없다.**
→ **명시적 결정: 타입 선언 순서 `type, x, y[, edge]`를 채택**한다.
근거: 타입 선언·CLI 경로·오라클 픽스처 **셋이 일치**하고, 렌더러 경로는 스프레드 부산물이다.
→ **바이트 정확 핀**으로 고정하고, **렌더러가 다른 순서를 낸다는 사실을 doc에 기록**한다.
⚠ JSON 객체 키 순서는 **적합한 파서에겐 의미 없다** → 이건 **바이트 수준 발산이지 동작 발산이 아니다**.
그래도 기록하는 이유는 수신자가 제3자 바이너리라 관용도를 모르기 때문이다.
선례: `termstream` R10이 필드 순서를 **표현의 일부**로 다뤘다(`termstream/src/lib.rs:110-122`).

## 3. 계약 결정

- **E3 — 태그 리터럴이 **모듈 오라클 안에선 항진명제**다.** `test:8`이
  `expect(frame[0]).toBe(SERVE_SIM_TOUCH_MESSAGE_TAG)` — **상수를 `0x99`로 바꿔도 green**.
  리터럴 `0x03`은 **범위 밖 다른 테스트**(`emulator-gesture-sender.test.ts:19`)에만 있다.
  → **`assert_eq!(SERVE_SIM_TOUCH_MESSAGE_TAG, 0x03)` + 프레임 레벨 `frame[0] == 0x03`** 둘 다.
  (형제 `emulator-keyboard-frame`의 `0x06`도 같은 결함 — 기록만.)
- **E4 — `edge`는 **픽스처가 리포 전체에 0개**다**(`:7`, optional).
  `undefined`면 **키 자체가 생략**된다(`JSON.stringify` 의미론).
  → Rust `Option<f64>`, `None`이면 **키 미출력**. 존재/생략/정수 포맷(E1) **셋 다** 핀.
- **E5 — 3개 variant 중 **`'begin'`과 `'end'`가 모듈 오라클에서 어둡다****(`test:6`에 `'move'`뿐).
  둘 다 생산에서 상시 발생한다(`emulator-device-frame.tsx:133,185,230,313`).
  → **문자열로 직렬화**된다(정수 매핑 **없음** — 판별자 정수를 도입하지 말 것). 3종 각각 바이트 핀.
- **E6 — 검증/클램핑을 **추가하지 말 것**.** 범위 개념이 이 계층에 없다.
  클램핑은 상류(`emulator-screen-gesture.ts:45-47`), 검증은 CLI(`cli/handlers/emulator.ts:57-61,95-102`)에 있다.
- **E7 — JSON 문자열 이스케이프 기계가 **필요 없다**.** 페이로드가 **닫힌 shape**이고
  문자열 필드가 **3원소 ASCII 집합**뿐이다(`:1`) → `\uXXXX`·제어문자·서로게이트 처리 **전부 불필요**.
  임의 문자열이 들어올 경로가 없다. 이 사실을 doc에 적어 후속 편집자가 필드를 늘릴 때 알아채게 한다.
- **E8 — 디코더를 만들지 않는다.** 리포에 디코더가 없고 외부 프로세스가 푼다. `encode`만 export.

## 4. 배치 — `suaegi-misc`
[[suaegi-misc-placement-rule]]: import 0, 외부 의존 0. **`serde_json` 불필요**(디코드가 없다) →
신규 leaf 크레이트 정당화가 **사라진다**(기존 프로토콜 크레이트 2개는 **디코드 때문에** serde_json을 갖는다).
`suaegi-screencast`/`suaegi-termstream`에 **합치지 않는다** — 그쪽 1크레이트=1소스모듈 헌장을 깨고
**인코드 경로에서 쓰면 안 되는 `serde_json`을 스코프로 끌어들여** E1 함정을 초대한다.
`suaegi-misc`엔 이미 프로토콜 모듈이 있다(`protocol_compat`) — 크레이트 경계는 **주제가 아니라 의존성**이다.
⚠ **기록**: `emulator-keyboard-frame.ts`(129L, 태그 `0x06`)가 **구조적 쌍둥이**다.
그것까지 이식하면 **그때 `suaegi-servesim` leaf 승격을 재검토**한다(둘이면 크레이트가 값을 한다).

## 5. 오라클 & 핀
**오라클 전량**(`emulator-touch-frame.test.ts` 15L, 케이스 1개). ⚠ 이 오라클은 **거의 아무것도 고정하지 못한다**
(파싱 후 비교 + 심볼 태그 + `move`만 + `edge` 없음 + `0.25/0.75`).

**추가 핀 = 이 PR의 사실상 전부. 전부 `assert_eq!(frame, b"...")` 바이트 단언으로 쓴다:**
**E1 수치 6종**(`1`·`0`·`-0`·`3`·`NaN`→`null`·`1e21`) — **파싱 금지, 바이트 비교**;
**E2 키 순서 정확 바이트**; **E3 태그 리터럴 + `frame[0]`**; **E4 `edge` 존재/생략/정수**;
**E5 `begin`/`move`/`end` 3종**; 프레임 길이 `1 + n`; UTF-8 페이로드가 오프셋 1에서 시작;
비-ASCII가 페이로드에 **불가능**함을 타입으로 보장(E7).

*mutation:* E1 `serde_json::to_vec`으로·`ryu`로·비유한을 에러로, E2 필드 순서 교환(`x,y,type`),
E3 태그 값 변경, E4 `None`일 때 `null` 출력·키 항상 출력, E5 variant 문자열 변경·정수 판별자 도입,
E6 클램핑 추가, E8 디코더 추가(범위 밖), 태그를 오프셋 1로.

## 6. 순서
단일 PR, 모듈 하나. 헤더 모듈 수(현재 thirty-four)·목록·`Cargo.toml` 설명 반영(**v1.4.146-rc.0**).
불변식: **고정 레이아웃 아님·이웃 기계 반입 금지**(§0), **ECMAScript 수치 포맷 + 로컬 복사**(E1),
**키 순서 명시 결정 + 바이트 핀**(E2), 태그 리터럴(E3), `edge` 생략 의미론(E4), 3 variant(E5),
검증 미추가(E6), 이스케이프 불필요 기록(E7), 인코드 전용(E8), `suaegi-misc`(§4),
**모든 핀을 바이트 단언으로**, 매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[mutation-survivor-triage]], [[mutation-harness-mtime-trap]],
[[suaegi-misc-placement-rule]], [[orca-source-location]], [[suaegi-impl-model-sonnet]]
