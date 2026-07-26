# Plan — browser-screencast-protocol (신규 `suaegi-screencast` 크레이트, 단일 PR)

조사: Explore 정찰(`browser-screencast-protocol.ts` 143L + `.test.ts` 136L 통독, 실제 생산자
`browser-screencast-stream.ts`와 소비자 `BrowserPane.tsx` 실측, 형제 프로토콜 대조).

**이건 바이너리 와이어 프로토콜이다.** 최대 위험은 바이트 레이아웃과 엔디언이고,
**오라클이 엔디언을 전혀 고정하지 못한다**(§3 참조).

## 0. 배치 — 신규 leaf `suaegi-screencast`, 의존 1개

```toml
[dependencies]
serde_json = { workspace = true }   # 디코드 전용 — JSON.parse 충실도
```
- `suaegi-misc`/`suaegi-path`는 **의존 0 헌장** → `serde_json` 불가. `suaegi-browser-url`은 URL 전용이고
  M3(#90)에서 모듈 완료를 선언했다. `suaegi-term`은 PTY 크레이트라 역전.
- **디코드에 `serde_json`은 정당하다** — 공격자 영향 하의 바이트에 `JSON.parse`를 재현한다
  (문자열 이스케이프, `\uXXXX`, 중복 키, 숫자 문법). 손코딩은 리포가 경고하는 "hollow-test magnet"이다.
- ⚠ **`serde_json::Value`(BTreeMap)를 써도 여기서는 안전하다** — `suaegi-mcp`와 달리 이 모듈은
  **디코드에서 키 순서를 관측하지 않는다**(`O:79`가 `METADATA_KEYS`로 루프를 돌며 **키로 조회만** 한다).
  `Map::insert`의 last-wins도 `JSON.parse`의 중복 키 규칙과 일치한다.
- ⚠ **`preserve_order` 절대 금지** — 현재 11개 크레이트가 `serde_json`을 쓰므로 feature unification이
  전역 전파된다. 그리고 **이 모듈은 그걸 원할 이유가 전혀 없다**.
- **인코드는 `serde_json`을 쓰지 않는다**(F5). `regex`·`serde` derive·`thiserror` 없음(디코드는 total).

## 1. 계약 결정

- **F1 — ⚠ **엔디언이 전부 리틀엔디언이고, 오라클은 이를 하나도 고정하지 못한다**.**
  16바이트 헤더: `[0]` kind `0x62`, `[1]` version `1`, `[2]` opcode, `[3]` format,
  `[4..8]` seq **u32 LE**, `[8..12]` metadata 길이 **u32 LE**, `[12..16]` reserved **u32 LE, 반드시 0**.
  소스는 `setUint32`/`getUint32`에 **매번 `littleEndian = true`를 명시**한다(`O:96/97/98/122/123/124`).
  ⚠ **`DataView`의 기본값은 빅엔디언**이라 "인자가 없으니 BE겠지"라는 역방향 오독이 쉽고,
  와이어 프로토콜에서 `to_be_bytes()`("network byte order")를 집는 반사신경이 더 위험하다.
  **정찰 실측: 빅엔디언 포트는 오라클 8케이스를 전부 통과한다**(seq는 늘 42/1, 길이는 2/19라 전부 1바이트).
  → `u32::to_le_bytes`/`from_le_bytes`. **`seq = 0x01020304`로 바이트 정확 핀 필수.**
- **F2 — `[12..16]`은 reserved이지 **이미지 길이가 아니다**.** 이 프로토콜은 **메타데이터만 길이 접두**를 갖고
  **이미지는 끝까지**다(`O:141` `subarray(imageStart)`, 끝 인자 없음). 이미지 길이 검증을 **발명하지 말 것**.
  reserved가 0이 아니면 거부(`O:124-126`).
- **F3 — 디코드는 **뷰를 반환한다**(`subarray`), 복사가 아니다.**
  오라클 `T:132-134`가 `decoded.image.buffer === encoded.buffer`를 명시 검증하고,
  소비자 `BrowserPane.tsx:1364-1366`이 그 뷰에서 **명시적으로 복사해 나간다**.
  → **`fn decode(bytes: &[u8]) -> Option<Frame<'_>>`, `image: &'a [u8]`.**
  `Vec<u8>`은 모든 값 단언을 통과하면서 계약을 깨고 프레임마다 전체 복사를 추가한다.
  ⚠ 형제 `terminal-stream-protocol.ts:67`은 **진짜로 복사한다**(`slice`) — 두 프로토콜을 통일하지 말 것.
- **F4 — `METADATA_KEYS`는 **디코드 전용**이다.** 유일한 사용처가 `O:79`의 디코드 루프다.
  인코드는 호출자 객체를 **그대로** `JSON.stringify`한다(`O:57`, `O:89`) → 방출 키 순서 = 호출자의 삽입 순서.
  ⚠ 오라클 픽스처(`T:14-19`)의 순서는 `METADATA_KEYS`와 **다르다**.
  **실제 생산자(`browser-screencast-stream.ts:56-66`)는 `METADATA_KEYS` 순서와 정확히 일치**한다
  → Rust 구조체 필드를 `METADATA_KEYS` 순서로 선언하면 **생산 바이트 스트림을 정확히 재현**하고,
  오라클은 **의미상으로만** 재현한다(오라클은 바이트를 들여다보지 않으므로 무해). 주석으로 명시.
- **F5 — ⚠ 메타데이터 직렬화는 **손코딩**해야 한다. `serde_json`의 수치 포맷이 다르기 때문이다.**
  `serde_json`은 `ryu`로 f64를 찍는다: `1280.0` → **`"1280.0"`**(JS는 `"1280"`), `-0.0` → **`"-0.0"`**(JS는 `"0"`),
  그리고 **지수 표기로 전환하지 않는다**(JS는 1e21에서 전환).
  이 차이는 전부 **오프셋 8의 u32 길이 필드로 전파**되어 **거의 모든 실제 프레임에서 다른 프레임이 된다**.
  → 9개 `Option<f64>`를 `METADATA_KEYS` 순서로, `None`은 **키 자체를 생략**, 값은 ECMAScript
  `Number::toString`으로 찍고 `,`로 join, `{}`로 감싼다. 키는 고정 ASCII라 이스케이프 불필요.
  **수치 포맷터는 `suaegi-mcp/src/json.rs:266 format_ecmascript_float`를 복사**한다(그 함수는 `pub`이 아니다).
  선례: `suaegi-workname/Cargo.toml:22-24`가 같은 이유로 `js_ws` 술어를 복사했다.
  이 모듈은 f64뿐이라 `Float` 분기만 필요하다.
- **F6 — 부재는 **키 삭제**, 비유한(non-finite)은 **키 존재 + `null`**.**
  `JSON.stringify`는 `undefined` 값 키를 **생략**하지만 `NaN`/`±Infinity`는 **`null`로 방출**한다
  (오라클 `T:110`이 정확히 이걸 만든다). serde의 `skip_serializing_if` 없는 구조체는 부재 필드마다
  `"offsetTop":null`을 찍어 **9개 키가 더 붙고 길이가 달라진다**.
- **F7 — `TextDecoder`는 **lossy이고 BOM을 벗긴다**.**
  `O:62` `new TextDecoder()`는 `fatal: false` → 잘못된 UTF-8은 **U+FFFD로 치환되고 절대 throw하지 않는다**.
  그리고 `ignoreBOM: false` → 선두 U+FEFF(`EF BB BF`)를 **제거한 뒤** `JSON.parse`에 넘긴다.
  → **`String::from_utf8_lossy`**(`str::from_utf8`/`serde_json::from_slice` **금지**) **+ 선두 BOM 명시 제거**.
  도달 가능한 발산: `{"note":"\xFF"}`는 JS에선 파싱 성공, `from_utf8`은 `None`.
  ⚠ 반대 방향 하나는 승인된 발산으로 문서화: `JSON.parse`는 고립 서로게이트 이스케이프(`"\ud800"`)를
  받아들이지만 `serde_json`은 거부한다. 메타데이터 **값**에는 도달 불가(숫자만 살아남음)이나
  **미지 키**에는 도달 가능하다.
- **F8 — 거부 경로는 정확히 7종이고 **비교 연산자가 계약**이다.**
  ① `len < 16`(**엄격 `<`** — 정확히 16바이트는 **통과**) ② kind≠`0x62` **또는** version≠1(한 `if`)
  ③ opcode≠1 ④ format이 1도 2도 아님(`if (!format)` truthiness) ⑤ reserved≠0
  ⑥ `image_start > len`(**엄격 `>`** — `image_start == len`인 **빈 이미지는 통과**)
  ⑦ 메타데이터가 falsy/비객체/배열(`!raw || typeof !== 'object' || Array.isArray`).
  ⚠ ⑦의 하위: JSON 파스 실패·리터럴 `null`·`0`/`false`/`""`·숫자/문자열/`true`·배열.
  **`{}`는 truthy라 성공이다**(`O:133`) — `is_empty()` 가드 추가 금지. 오라클 4케이스가 이에 의존한다.
- **F9 — `seq` 인코딩은 **음수 클램프 → floor → mod 2³²**이고 Rust의 `as u32`는 **틀리다**.**
  `O:96` `Math.max(0, Math.floor(seq)) >>> 0`. Rust의 f64→정수 캐스트는 **saturate**한다
  (`5e9 as u32` = 4294967295) 반면 JS `>>> 0`는 **wrap**한다(= 705032704).
  → 유한성 확인 후 `(v.floor() as u64) % (1u64 << 32)`. `NaN`·`+∞` → 0, `-∞`·음수 → 0.
  디코드는 이걸 **되돌리지 않는다** — 원시 u32를 그대로 준다(`O:122`). seq 검증도 **없다**.
- **F10 — `16 + metadata_len`은 **checked 산술**로.** JS는 f64라 2⁵³까지 정확하지만 Rust에서
  `metadata_len = 0xFFFF_FFFF`면 32비트 타깃에서 오버플로한다 → debug 패닉 / release 랩어라운드로
  `image_start`가 15가 되어 ⑥을 통과하고 **쓰레기에서 그럴듯한 프레임을 잘라낸다**.
  `checked_add` 또는 u64 산술 후 `None`.
- **F11 — 인코드 시 `format`은 **catch-all**이다.** `formatToByte`는 `format === 'png' ? 2 : 1`(`O:43`) —
  `'png'`가 **아닌 모든 것**이 jpeg(1)로 나간다. Rust는 2-variant enum이라 자연히 재현되지만,
  디코드의 `byteToFormat`은 **1과 2만** 받고 나머지는 `null`(`O:47-52`)이라는 **비대칭**을 유지한다.
- **F12 — 디코드는 `bytes.byteOffset`을 존중한다**(`O:108`). 즉 0이 아닌 오프셋의 뷰를 넘겨도 정확하다.
  Rust `&[u8]`은 자연히 동일. 핀으로 고정.

## 2. 오라클 & 핀

**오라클 8케이스 전량:** `T:9-37` 왕복(seq 42, jpeg, 4키, 4바이트 이미지) / `T:39-41` 4바이트 입력 → null
(**이름과 달리 길이 가드에 걸린다**) / `T:43-58` 3케이스(version·opcode·format 손상) /
`T:60-75` 메타 길이 오버런 / `T:77-88` reserved≠0 / `T:90-100` 메타가 배열 / `T:102-121` 필터
(문자열·NaN·미지 키 탈락, 유한 숫자 보존) / `T:123-135` **이미지 앨리어싱**.

**추가 핀(오라클 침묵 — 여기가 이 PR의 실제 가치다):**
**F1 엔디언 바이트 정확**(seq `0x01020304` → `[04,03,02,01]`, 메타 길이도); F8① `len == 16` 정확히 통과;
F8⑥ `image_start == len`(**빈 이미지**) 통과; F8② **kind 바이트 단독 거부**(오라클이 한 번도 안 건드린다);
F8⑦ 파스 실패·`null`·`0`·`""`·`true`·숫자·문자열 각각; `metadata_len == 0` → 거부(간접 경로);
F5 `{deviceWidth: 1280.0}`의 메타 바이트가 정확히 `{"deviceWidth":1280}`(`1280.0` 아님)·`-0.0` → `0`·
`1e21` → `1e+21`; F6 부재 키 생략 vs `NaN` → `"pageScaleFactor":null`; F7 잘못된 UTF-8이 U+FFFD로 치환되어
**파싱 성공**·선두 BOM이 제거되어 파싱 성공; F9 seq 음수/소수/`NaN`/`∞`/`2**32`/`5e9`(**wrap 705032704**);
F10 `metadata_len = 0xFFFF_FFFF` → `None`(패닉 아님); F11 png 인코드(2) 및 디코드(2)—**오라클 커버리지 0**;
F12 비-0 오프셋 슬라이스; F4 생산자 키 순서로 바이트 정확.

*mutation:* F1 `to_be_bytes`, F2 reserved를 이미지 길이로 해석·reserved 검사 제거, F3 `Vec<u8>` 반환,
F5 `serde_json` 직렬화·`to_string()` 사용, F6 `null` 방출·비유한 생략, F7 `from_utf8`·BOM 미제거,
F8 `<`→`<=`·`>`→`>=`·`{}` 거부·kind 검사 제거·배열 검사 제거, F9 `as u32` saturate·클램프 제거·floor 제거,
F10 unchecked 산술, F11 디코드가 미지 format을 jpeg로.

## 3. 순서
단일 PR. 인코드/디코드가 헤더 레이아웃·format 맵·메타 코덱을 공유하고,
**오라클 8케이스 중 7개가 `encode`로 픽스처를 만든 뒤 손상시켜 `decode`에 먹인다** → 분할 불가.
작업 순서: 헤더 상수 + format 맵 → 수치 포맷터 복사 + 메타 직렬화 → `encode` → `decode`(빌린 슬라이스) →
오라클 8 → 침묵 핀.
불변식: **리틀엔디언**(F1), reserved≠이미지길이(F2), **뷰 반환**(F3), `METADATA_KEYS`는 디코드 전용(F4),
**손코딩 직렬화 + ECMAScript 수치**(F5), 부재/비유한 비대칭(F6), **lossy 디코드 + BOM 제거**(F7),
거부 7종의 비교 연산자(F8), seq wrap(F9), checked 산술(F10), format 비대칭(F11), 오프셋 존중(F12),
매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[mutation-survivor-triage]], [[suaegi-impl-model-sonnet]]
