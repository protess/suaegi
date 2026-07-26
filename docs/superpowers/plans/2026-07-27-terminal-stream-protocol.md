# Plan — terminal-stream-protocol (신규 `suaegi-termstream` 크레이트, 단일 PR)

조사: Explore 정찰(소스 109L + 오라클 157L 통독, **이미 이식된 형제** `suaegi-screencast` 대조).
출처 `reference/orca/` = **v1.4.146-rc.0**.

⚠ 내 리서치 프롬프트가 "opcode 7개"라고 했는데 **틀렸다**. `shared/`판은 **15개**고,
7개짜리는 `mobile/src/transport/terminal-stream-protocol.ts`의 **별도 중복 구현**이다(R11).

## 0. ⚠⚠ 오라클이 **12가지 틀린 구현을 전부 통과시킨다**
정찰이 표로 정리했다. 이 PR의 가치는 사실상 **전부 직접 쓰는 핀**에 있다:
빅엔디언 / seq 워드 뒤바꿈 / seq를 `u64::to_le_bytes`로 / seq를 단일 u32로 /
복사 대신 뷰 반환 / 바이트 3을 검증 / kind 바이트를 검증하거나 안 하거나 /
streamId를 seq처럼 클램프 / serde_json 키 정렬 / ryu 수치 포맷 / 엄격 UTF-8 / BOM 미제거.

## 1. 배치 — 신규 leaf `suaegi-termstream`
```toml
[dependencies]
serde_json = { workspace = true }   # 디코드 전용
```
`suaegi-screencast`를 확장하지 **않는다** — 두 프로토콜은 **공유 코드가 0**이다
(kind·검증 개수·페이로드 소유권·바이트 3/12 의미·JSON 코덱 모양 전부 다름).
`suaegi-term`은 PTY 크레이트라 역전. `preserve_order` **금지**(워크스페이스 전역 전파).

## 2. 계약 결정 — **형제와 다른 것부터**

- **R1 — ⚠⚠ 페이로드는 **복사**다(`slice`), 뷰가 아니다.** 스크린캐스트는 `subarray`(뷰)였다.
  → **`payload: Vec<u8>`, 반환 타입에 라이프타임 **없음****.
  ⚠ `suaegi-screencast`의 `&'a [u8]` 시그니처를 **가져오면 안 된다**.
  ⚠ 오라클에 `.buffer` 단언이 **0개**라 뷰 반환도 7/7 통과한다.
  생산 코드가 이 복사에 의존한다: `remote-runtime-terminal-multiplexer.ts:530`이
  `frame.payload`를 배열에 쌓아 **이벤트 루프 턴을 넘겨** `:534`에서 합친다 —
  재사용되는 WebSocket 버퍼 위의 뷰라면 조용히 오염된다.
  → 핀: 디코드 후 **원본 버퍼를 변형**하고 payload가 그대로인지 단언.
- **R2 — ⚠⚠ `seq`는 **64비트를 두 u32로 쪼갠 것**이고 **HIGH가 더 낮은 오프셋(8)**에 있다.**
  각 워드는 내부적으로 LE지만 **워드끼리는 BE 배치**다 →
  `u64::to_le_bytes`도 `to_be_bytes`도 **아니다**.
  오라클의 seq가 전부 < 2³²이라 **high 워드가 항상 0** → 워드를 뒤바꿔도, `u64::to_le_bytes`로 해도,
  high를 아예 버려도 **7/7 통과**한다.
  → 핀: `seq = 0x0000_0005_0000_0006` → `[8..12] == [5,0,0,0]` **그리고** `[12..16] == [6,0,0,0]`,
  더해서 `[8..16] != seq.to_le_bytes()`라는 **부정 단언**.
- **R3 — 엔디언은 여섯 사이트 전부 명시적 LE**다. `DataView` 기본값은 **빅**엔디언이고
  와이어 프로토콜 반사신경은 `to_be_bytes`("network byte order")다.
  오라클 값이 전부 < 256이라 **빅엔디언 포트가 7/7 통과**한다.
  → 핀: `streamId = 0x01020304` → `[4..8] == [4,3,2,1]`.
- **R4 — 거부 경로는 **정확히 셋**이다**(길이 < 16, kind∨version 불일치, 미지 opcode).
  ⚠ **스크린캐스트의 검사를 가져오지 말 것**:
  ① `[12..16]`은 **reserved가 아니라 seq LOW 워드**다 — reserved 검사를 넣으면 대부분의 프레임이 거부된다.
  ② **길이 필드가 없다** → `checked_add`도 `image_start > len`도 **대응물이 없다**.
  ③ **바이트 3은 pad**로 쓰기만 하고 **읽지 않는다** — 검증을 넣으면 **오라클은 통과하면서**
     제3자 프레임을 거부한다(encode가 항상 0을 쓰므로).
  → 핀 셋: `bytes[3] = 0xFF`도 **디코드됨**, seq low 워드 `0xFFFFFFFF`도 **디코드됨**,
    길이 필드 없이 1바이트 페이로드도 **디코드됨**.
- **R5 — opcode는 **15개**이고 검증은 **집합 멤버십**이다**(15항 `||` 체인).
  값이 우연히 연속 1..=15라 범위 검사가 오늘은 동치지만, 소스 주석이 향후 비연속 추가를 예고한다
  → **`TryFrom<u8>` + 15-arm match**.
  ⚠ 오라클은 미지 opcode를 **99 하나**로만 검증한다 — 경계 `0`과 `16`이 어둡다.
  6개 opcode(3,4,5,6,14,15)는 **왕복 자체가 없다**(15는 생산 소비자가 있다).
- **R6 — `streamId`와 `seq`는 **수치 의미론이 다르다**(네 줄 간격).**
  `seq`: `max(0, floor(x))` → 클램프 있음. `streamId`: **생 `ToUint32`** — 0 방향 절단, mod 2³² 랩,
  `NaN`/`±Inf` → 0, **음수 클램프 없음** → `-1`은 `0xFFFF_FFFF`로 나간다. 오라클 커버리지 0.
  ⚠ Rust `f64 as u32`는 **포화**하고 JS `>>> 0`은 **랩**한다 → `% 4_294_967_296.0` 관용구를 **두 워드 다**.
- **R7 — `decodeTerminalStreamJson`에는 **형태 검사가 없다**.** 스크린캐스트의
  `!raw || typeof !== 'object' || Array.isArray` **대응물이 없다** — `"[1,2]"`·`"42"`·`"true"` 전부 성공한다.
  스크린캐스트의 `Value::Object` 매치를 가져오면 **없는 거부를 추가**하게 된다.
  ⚠ `JSON.parse("null")`의 `null`과 실패 sentinel이 **구별 불가**하다 —
  실제 소비자 4곳이 둘을 동일 취급하므로 `Option`으로 접는 게 충실하다. **주석으로 명시.**
- **R8 — 텍스트 코덱이 **새로 생겼다****(스크린캐스트엔 없음). `Output` 프레임의 **핫 경로**다.
  `TextDecoder` 기본값: `fatal: false` → **U+FFFD 치환, throw 안 함**;
  `ignoreBOM: false` → **선두 BOM이 먹힌다**.
  → `String::from_utf8_lossy` + `strip_prefix('\u{FEFF}')`를 **JSON·텍스트 두 곳 다**.
  `str::from_utf8`(Err 반환)도 그냥 `from_utf8_lossy`(BOM 유지)도 **둘 다 틀리다**.
  핀: `[EF BB BF 'a']` → `"a"`, `[0xFF]` → `"\u{FFFD}"`.
- **R9 — `seq` 재조합은 **f64**다**(`high * 2³² + low`). `u64` 반환은 **소스보다 정확해져서** 발산한다
  → 양쪽 다 `f64`로 모델링(스크린캐스트의 32비트 논거와 동일).
- **R10 — 인코드 JSON의 바이트 충실도 두 가지**(오라클이 파싱값만 `toEqual`해서 **둘 다 안 보임**):
  `serde_json::Value`는 **키를 재정렬**한다(BTreeMap) — `JSON.stringify`는 삽입 순서;
  `ryu`는 `120.0`을 `"120.0"`으로, `-0.0`을 `"-0.0"`으로 찍고 1e21에서 지수로 안 바꾼다.
  → `format_ecmascript_float`를 **또 복사**한다(`suaegi-mcp/src/json.rs:266` → `suaegi-screencast` → 여기;
  모듈별 복제가 이 리포의 확립된 헌장).
  ⚠ 다만 **여기선 길이 필드가 없어** 포맷 차이가 프레이밍을 깨지는 **않는다**(스크린캐스트와 다른 점).
- **R11 — `mobile/`에 **7개 opcode짜리 중복 구현**이 있다.** 프레이밍은 바이트 동일하지만
  opcode 7~11·13~15를 **거부**한다. **`shared/`판(15개)을 이식**하고 모바일 부분집합은
  화해시킬 불일치가 아니라 **호환성 사실**로 기록한다.

## 3. 오라클 & 핀
**오라클 7케이스 전량**. **추가 핀이 이 PR의 본체다** — §0의 12가지를 각각 죽인다:
R1 복사(원본 변형 후 불변); R2 워드 배치 3종 단언(부정 단언 포함); R3 `streamId` 바이트 정확;
R4 세 가지 "그런 검사 없음"; R5 **15개 전부 왕복** + opcode `0`·`16` 거부 + 0/15바이트 → `None`
+ 정확히 16바이트 → `Some`(빈 페이로드); R6 `streamId`의 음수·소수·`NaN`·≥2³², `seq`와 대비;
R7 배열/숫자/불리언 JSON이 **성공**함 + `"null"`의 모호성; R8 BOM·불량 UTF-8 양쪽 코덱;
R10 키 순서 + `120`/`-0`/`1e21` 바이트; kind 바이트 손상 거부.

*mutation:* R1 `&[u8]` 반환, R2 워드 교환·`u64::to_le_bytes`·high 버림, R3 `to_be_bytes`,
R4 reserved 검사·길이 검사·바이트 3 검사 **각각 추가**, R5 범위 검사로·arm 제거,
R6 `as u32` 포화·streamId에 클램프 추가, R7 형태 검사 추가, R8 `from_utf8`·BOM 미제거,
R10 `serde_json` 직렬화·`to_string()`.

## 4. 순서
단일 PR. 오라클 7케이스 중 6개가 payload를 `encodeTerminalStreamJson`/`Text`로 만들고
7번째는 encode 출력을 손상시켜 decode에 먹인다 → 프레임 코덱과 페이로드 코덱, encode와 decode
**모두 분리 불가**.
불변식: **복사 반환**(R1), **seq 워드 배치**(R2), LE(R3), **거부 3개뿐**(R4), 15-arm(R5),
두 수치 규칙 분리(R6), 형태 검사 없음(R7), lossy+BOM 양쪽(R8), f64 재조합(R9),
ECMAScript 수치 + 키 순서(R10), 모바일 중복은 기록만(R11), 매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[mutation-survivor-triage]], [[orca-source-location]],
[[suaegi-impl-model-sonnet]]
