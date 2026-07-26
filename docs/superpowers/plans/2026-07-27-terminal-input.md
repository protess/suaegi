# Plan — terminal-input (신규 `suaegi-terminput` 크레이트, 단일 PR)

조사: Explore 정찰(소스 109L + 오라클 134L 통독) + **내가 소스 109L 전량과
`clipboard-text.ts`의 `measureClipboardTextByteLength`/`isClipboardTextByteLengthOverLimitWithYield`를
직접 재확인**(정찰 주장 전부 일치). 출처 `reference/orca/` = **v1.4.146-rc.0**.

의존 `clipboard-text`는 **이미 이식됨**(`suaegi-misc/src/clipboard_text.rs`, 747L).
정찰이 현 벤더링본과 라인 대조 → **의미 발산 0**. 헤더는 `v1.4.146-rc.0`으로 단다
(`suaegi-misc` 쪽 `1.4.150` 라벨은 그 크레이트 시점 기준이라 건드리지 않는다).

## 0. ⚠ 이 오라클이 통과시키는 **틀린 구현 목록**
13케이스 전량이 다음을 **못 잡는다**. 이 PR의 가치는 대부분 **직접 쓰는 핀**에 있다:
상수 3개 값 전부 틀려도 통과(전부 **심볼 참조**, `MAX_BYTES`는 **import조차 안 됨**) /
`''` → `['']` / 청크 경계에서 2·3바이트 문자 오처리(픽스처가 **astral 전용**) /
deferred를 `-> bool`로 접기(13개 중 12개 통과) / `TI:83` 폴백 arm 전부 /
assert의 **성공 경로**(verbatim 반환·무예외) / 술어들의 `>` vs `>=` / `with_yield`의 `true` 반환.

## 1. 배치 — 신규 leaf `suaegi-terminput`
```toml
[dependencies]
suaegi-misc = { path = "../suaegi-misc" }   # measure_clipboard_text_byte_length(_with_yield),
                                            # CLIPBOARD_TEXT_MEASURE_YIELD_CODE_UNITS
```
`suaegi-misc`에 **접지 않는다** — 이틀 전 `suaegi-filedrop`이 **같은 상황**(유일 import가
`clipboard_text`)에서 내린 결론을 그대로 따른다: 그 크레이트 헌장은 "정책 0·**크레이트 내부 import 0**"이고
`clipboard_text`는 **747L 정책 모듈**이라 의존하면 내부 위상이 역전된다
(`docs/superpowers/plans/2026-07-27-native-file-drop.md:10-15`).
게다가 이 모듈은 자체 정책 캡 2개(`TI:7-8`)와 사용자 노출 에러 문자열(`TI:9-10`)을 갖는다.
`suaegi-term`은 PTY 런타임(`portable-pty`/`alacritty_terminal`)이라 순수 헬퍼를 묻으면
격리 mutation 테스트 성질이 죽는다.
**금지**: `tokio`/`futures`(S1로 async를 걷어낸다), `unicode-segmentation`(**코드포인트** 원자성이지
grapheme이 아니다), `regex`, `serde`, `thiserror`(무필드 에러 → `Display` 수기, 선례 `clipboard_text.rs:116-133`).

## 2. 계약 결정

- **U1 — ⚠⚠ 청크 경계는 **코드포인트 원자적**이고, 예산은 바이트인데 커서는 UTF-16이다.**
  `codeUnitLength = codePoint > 0xffff ? 2 : 1`(`TI:95`)로만 전진하고 절단은 **그 커서**로만 한다
  (`TI:99`,`:107`) → **다중바이트 문자·서로게이트 쌍 내부로 경계가 절대 안 들어간다**.
  → Rust는 `char_indices()`로 **바이트 오프셋**을 들고 기록된 시작점에서만 `&text[a..b]`.
  ⚠ "예산이 바이트니까 `&text[..max]`" 라는 **당연해 보이는 단순화는 패닉**한다(비-char-boundary).
  ⚠ 반대로 **grapheme으로 격상하지 말 것**: 결합문자·ZWJ·CRLF·ANSI 이스케이프는 **설계상 쪼개진다**.
- **U2 — ⚠ 청크는 `maxChunkBytes`를 **초과할 수 있다**.**
  `currentBytes > 0 &&` 가드(`TI:98`) 때문에 예산보다 큰 코드포인트는 **통째로 admit**된다
  (최대 `max(max_chunk_bytes, 4)` 바이트). 생산 소비자가 이미 이걸 전제한다
  (`remote-runtime-pty-batching.ts:89`의 `>=`). **"고치지" 말 것.** 오라클 `T:61-66`이 고정.
- **U3 — deferred의 **반환 *모양*이 계약**이다**(`TI:56-67`, `boolean | Promise<boolean>`).
  `-> bool`로 접으면 13케이스 중 **12개가 통과**하면서 계약이 사라진다(`T:127`만 죽는다).
  → **2-variant enum**이 정확한 대응물이다(TS union이 2항이므로 3항은 과잉):
  ```rust
  pub enum TerminalInputTooLargeDecision { Immediate(bool), Deferred(bool) }
  ```
  `TI:60` → `Immediate(true)`(스캔 0), `TI:63` → `Deferred(_)`(**유일하게 콜백이 도는 arm**),
  `TI:66` → `Immediate(_)`. 생산 소비가 이 분기를 구조적으로 읽는다
  (`pty-transport.ts:633-634`, `pty-input-write-queue.ts:133`).
- **U4 — ⚠⚠ `text.length`(UTF-16)는 **두 함수에선 관측 가능, 한 함수에선 불가능**하다. 균일 적용 금지.**
  - `TI:44`(`isTerminalInputTooLarge`): **불가능** — 모든 코드포인트에서 `utf8_len ≥ utf16_len`이라
    `utf16 > max ⟹ utf8 > max`이고, 나머지 항이 정확히 `utf8 > max`다.
    → `text.len() as f64 > max_bytes` **하나로 정확**(S5, `clipboard_text.rs:38-49`).
    `NaN`/`Infinity`/음수까지 직접 대조 확인: 양쪽 항이 항상 같은 답.
  - `TI:60`/`TI:63`(deferred): **가능** — 반환 *모양*을 고른다. **글자 그대로 `encode_utf16().count()`**.
    ⚠ 결정적 반례: `"é".repeat(200_000)`은 200 000 UTF-16 단위(≤262144 → TS는 **동기** arm `TI:66`)인데
    400 000 바이트(바이트 기반 포트는 **yield** arm). **오라클의 유일한 픽스처(`'é'×262145`)는
    두 지표 모두 초과라 이 둘을 구별 못 한다.** → 직접 핀.
- **U5 — 수치 계약은 전부 `Option<f64>`**(S2, `clipboard_text.rs:25-28`).
  `maxBytes`(`TI:31,41,51,58`)는 **정규화가 0회**다 → `NaN`이면 **아무것도 too large 아님**,
  `-1`이면 **`""`조차 too large**(`0 > -1`). `u64` 파라미터는 이 arm들을 **조용히 삭제**한다.
- **U6 — ⚠ `TI:83`은 형제(`CT:108-110`)와 달리 **`Math.floor`가 없다**.**
  `Number.isFinite(x) && x > 0 ? x : 1` → `NaN`/`±Inf`/`0`/음수 → **1**, 그런데 `2.5`는 **2.5로 남는다**.
  `normalized_max: f64`로 유지하고 `(current_bytes + character_bytes) as f64 > normalized_max`로 비교.
  floor를 넣고 싶으면(정수 LHS라 동치) **동치 증명을 주석에** 남길 것. 클립보드 리졸버 복붙 금지.
- **U7 — ⚠ `isClipboardTextByteLengthOverLimitWithYield`의 Rust 시그니처는 `max_bytes: u64`라
  TS `number` 도메인을 **표현 못 한다**.** 그대로 호출하면 U5의 `NaN`/음수/소수 arm이 **조용히 증발**하고
  오라클은 그런 값을 안 넘겨서 **아무도 못 잡는다**.
  → `CT:92-101`(5줄)을 **이 모듈에 인라인**한다: `text.encode_utf16().count() as f64 > max_bytes` 후
  `measure_clipboard_text_byte_length_with_yield(...)`(**이건 재사용** — cadence 로직은 중복 안 함).
  `suaegi-misc` 공개 API는 **건드리지 않는다**(그쪽 핀까지 흔든다).
- **U8 — async → 동기 + 주입 콜백**(S1, `clipboard_text.rs:17-24`). `Promise`는 이식 대상이 아니고
  cadence(262144 UTF-16 단위)만 이식한다. `&mut dyn FnMut()`.
  ⚠ TS 기본 yield는 `setTimeout(resolve,0)` = **매크로태스크**(오라클 `T:102` vs `T:106`이 증명).
  Rust엔 대응 기본값이 **없다** → 호출자 책임임을 doc에 명시.
- **U9 — 게으름은 생산 의미론**이지 최적화가 아니다. `pty-transport.ts:637-649`가 제너레이터를
  **중도 폐기**하고 `pty-input-write-queue.ts:97`이 drain 사이에 **재개**한다.
  → 명명 lazy 이터레이터 `TerminalInputChunks<'a>`(`Item = &'a str`), 선례
  `suaegi-misc/src/process_output_field_scanner.rs:52-82`. `split_*`은 `.collect()`(`TI:73`이 문자 그대로 그렇다).
  ⚠ `Vec` 포트도 `.into_iter()`로 감싸면 오라클을 통과한다 — **호출부가 잡는 계약**이라 직접 핀.
- **U10 — 빈 입력은 **0 청크**다**(`TI:80-82`). `['']`이 아니다. 오라클 커버리지 **0**.
- **U11 — 에러는 **페이로드 무관**하다**(`TI:34`). 무필드 에러 + `Display`가 상수 그대로.
  `assert`는 성공 시 입력을 **verbatim 반환**(`TI:36`) — trim·truncate **금지**(S8).
  ⚠ 오라클은 **성공 경로를 한 번도 안 탄다** → `js_trim` 사고나 "맞춰 자르기"가 **무검출**.
- **U12 — 비교는 **전 사이트 strict `>`**, `>=`가 **한 곳도 없다**.**
  → 캡과 **정확히 같으면 통과**(16384바이트 = 1청크, 16MiB = not too large). 술어 쪽 경계는 미고정.
- **U13 — `getTerminalInputByteLength`는 순수 위임**(`TI:13`, 조기중단 없음) = `text.len()`(S5 항등).
- **U14 — 사설 `getUtf8ByteLengthForCodePoint`(`TI:16-27`)는 `CT:174-185`와 **바이트 동일 중복**이다.**
  Rust에선 둘 다 `char::len_utf8()`. **공유 헬퍼로 추출하지 말 것**(모듈별 복제가 이 리포 헌장).

### ⚠ 등가 변이 2건 — mutation 스윕에서 **"공허한 핀"으로 오독하지 말 것**
내가 소스에서 직접 증명했다. `[[mutation-survivor-triage]]`의 **원인 ②/③**에 해당하며,
SURVIVED가 떠도 **테스트를 추가하는 게 정답이 아니다**. 주석으로 근거를 남긴다.
- **E1 — 빠른 경로(`TI:84-88`) 삭제는 등가다.** 총 바이트 ≤ `normalizedMax`면 루프의 가드
  `currentBytes + characterBytes > normalizedMax`가 **한 번도 참이 될 수 없고**(부분합 ≤ 총합 ≤ max)
  꼬리가 `text.slice(0)` = `text`를 낸다 → **출력 동일**. 차이는 **스캔 비용뿐**이고
  그걸 잡는 `T:47-55`는 `codePointAt` **스파이 테스트라 Rust 대응물이 없다**.
  → 구조 충실성 때문에 **남기되**, "출력 등가, 비용만 다름"을 주석에 적고 등가성 자체를 핀으로 고정.
- **E2 — 꼬리 가드 `currentStart < text.length`(`TI:106`)는 **비-empty 입력에서 항상 참**이다.**
  초기 `currentStart = 0 < len`(`TI:80`이 empty를 이미 걸렀다), 중간 방출 시 `currentStart = index`인데
  그 방출은 `index`의 코드포인트를 **소비하기 전**에 일어나므로 `index < len`. → 가드는 **사실상 죽은 코드**.
  삭제해도 안 죽는 건 **바깥 가드가 이미 결정**했기 때문이다(원인 ③, 무력 변이). 재타깃할 것.

## 3. 오라클 & 핀
**오라클 13케이스 전량**(`terminal-input.test.ts:21-133`).
단 `T:47-55`와 `T:83-91`은 `codePointAt` **스파이 기반**이라 Rust 대응물이 없다 →
의도만 옮기고 **왜 생략했는지 주석**으로 남긴다(선례 `clipboard_text.rs:378-381`).

**추가 핀(오라클 침묵 — 이 PR의 본체):**
**상수 3개 리터럴 값**(16384 / 16777216 / 정확한 메시지 — §0의 L7 함정);
U10 `''` → **0 청크**(두 splitter 다); U6 폴백 `0`·`-1`·`NaN`·`Infinity`·`2.5`;
U11 assert **성공 경로**(캡과 정확히 같을 때 무예외 + **verbatim 반환**) + 에러에 페이로드 **부재**;
U12 세 술어의 `>` 경계(정확히 캡이면 `false`); U5 `NaN` → `false`, `-1` → `""`도 `true`;
**U4 라우팅 반례 `"é".repeat(200_000)` → `Immediate`**(바이트 기반 포트를 죽이는 유일한 핀);
U3 세 arm 각각 + Deferred arm에서만 콜백이 돌았음; U1 **2바이트·3바이트 문자가 경계에 정확히 걸릴 때**
(픽스처가 astral 전용이라 4바이트만 특별대우하는 포트가 13/13 통과한다) + 혼합 폭 페이로드의 `join("") == input`;
U1 결합문자/ZWJ가 **쪼개짐**(grapheme 유지 안 함을 명시적으로 고정);
U2 예산 초과 청크; E1 빠른경로 등가성; U9 `split(x,n) == iterate(x,n).collect()` 항등;
U13 `''`와 2·3바이트 문자의 바이트 길이; `with_yield`가 **`true`를 반환**하는 모양.

*mutation:* U1 바이트 인덱스 슬라이스(패닉 확인)·grapheme 격상, U2 초과 문자 강제 분할,
U3 `-> bool`로 접기·arm 교환, U4 `TI:63`을 `text.len()`으로·`TI:44`를 `encode_utf16()`으로,
U5 `u64` 파라미터로, U6 `floor` 추가·폴백 상수를 `0`으로, U7 dep의 `u64` 시그니처 직접 호출,
U10 `['']` 반환, U11 `js_trim` 삽입·에러에 길이 삽입, U12 `>=`로 **전 사이트**,
U13 `encode_utf16().count()`로, U14 공유 헬퍼로 추출.
**E1·E2는 mutation 대상에서 제외**(등가/무력 — 위 증명 참조).

## 4. 순서
단일 PR. 109L·10 export·오라클 13케이스·신규 서드파티 0·이미 이식된 단일 의존.
`clipboard-text`가 `repo-icon`을 쪼갠 것 같은 **헌장 변경 요구가 없고**, 남은 판단
(크레이트 배치·deferred 반환 모양·수치 타입)은 전부 **첫 줄 쓰기 전에 확정되어야 하는** 것들이라 seam이 없다.
범위: `crates/suaegi-terminput/{Cargo.toml,src/lib.rs}` + 워크스페이스 `members` 한 줄.
불변식: 신규 leaf(§1), **코드포인트 원자 절단 + 바이트 오프셋 슬라이싱**(U1), 초과 청크 허용(U2),
**2-variant enum**(U3), UTF-16을 deferred에서만 글자 그대로(U4), `Option<f64>`(U5), floor 없음(U6),
`CT:92-101` 인라인(U7), 주입 콜백(U8), **lazy 이터레이터**(U9), 빈 입력 0청크(U10),
verbatim + 페이로드 없는 에러(U11), strict `>`(U12), 위임(U13), 헬퍼 복제(U14),
**E1/E2는 등가로 문서화**, 매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[mutation-survivor-triage]], [[orca-source-location]],
[[suaegi-impl-model-sonnet]]
