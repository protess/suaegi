# Plan — terminal-line-height-settings + ui-language (`suaegi-misc`에 모듈 2개 추가, 단일 PR)

조사: Explore 정찰(소스 3개·오라클 3개 통독 + 소비자 전수 grep). 출처 `reference/orca/` = **v1.4.146-rc.0**.
소스 38L / 오라클 47L. 신규 의존 **0**, async 0, 문자열 연산 0, I/O 0.

## 0. ⚠ 내 배치 판단이 틀렸고 정찰이 고쳤다 — 신규 크레이트 아님
나는 "캡 상수 = 정책 → `suaegi-misc` 부적격"으로 잡았는데, **이미 반례가 크레이트 안에 있다**:
`suaegi-misc/src/markdown_toc_width.rs`가 **같은 `unknown → clamp` 모양**에 캡 4개(200/240/320/600),
`Option<f64>` 모델링, 무반올림 `f64` end-to-end까지 **판박이**다(직접 확인).
헌장 문구의 작동하는 절반은 "정책 0"이 아니라 **"크레이트 내부 import 0"**이고, 이 둘은 import가 0이다.
→ **`suaegi-misc`에 모듈 2개 추가.** `markdown_toc_width`를 형제 선례로 삼는다.

⚠ **`pi-overlay-ui-settings`는 이 PR에서 뺀다**(원래 3개 묶음이었다). 이유:
`JsValue`/`JsRecord`(삽입순서 보존 레코드) 기구가 통째로 필요해 다른 둘과 **공유 코드가 0**이고,
**생산 소비자가 0개**이며(리포 전수 grep — 테스트 import 하나뿐), 생산 경로는 오히려
`settings.json`을 **무수정 복사**한다고 단언한다(`plugin-overlay.test.ts:146-160`).
→ **버리지 않는다**(클론 완성이 목표다). 별도 PR에서 자체 leaf 크레이트 +
`JsValue`/`JsRecord` **3번째 로컬 복사**(모듈별 복제 = 이 리포 헌장; `suaegi-filedrop`·`suaegi-quickcmd`에 2벌 존재)로 이식하고,
"상류에서 이미 후퇴한 정책"임을 주석에 남긴다.

## 1. 계약 결정 — `terminal_line_height`

- **V1 — ⚠⚠ `Number.isFinite` 가드는 **`±Infinity` 때문에만** 존재하고, 오라클은 그걸 **못 잡는다**.**
  JS `Math.min/max`는 NaN을 **전파**하고 Rust `f64::min/max`는 NaN을 **흡수**한다(IEEE `minNum`).
  그래서 가드를 **통째로 빼고** `MAX.min(v.max(MIN))`만 옮긴 포트도 `NaN → 1`을 맞혀 **7/7 통과**한다.
  진짜 발산은 `+Infinity`: 정답 **1**, 가드 없는 포트 **3**.
  → 두 줄 다 이식하고 **`Infinity → 1`, `-Infinity → 1`을 직접 핀**한다.
  ⚠ `f64::clamp`는 **제3의 동작**(NaN self → NaN, 경계가 NaN이면 **패닉**) — 쓰지 말 것.
- **V2 — ⚠ `MIN`/`MAX`가 **값으로 고정되지 않는다**.** 오라클을 연립방정식으로 풀면
  **`MIN ∈ [0.85, 1]`, `MAX ∈ [3, 4]`면 전부 통과**한다(`[4, MAX]`가 기대값 쪽에 `MAX`를 심볼로 써서 자기충족).
  `MIN=0.9, MAX=3.5`인 포트가 7/7 green. → **리터럴 단언** + `normalize(3.5) == 3.0`, `normalize(0.95) == 1.0`.
- **V3 — 수치 강제 변환이 **한 곳도 없다**.** `Number()`·`parseFloat`·`parseInt` 전무 → `"2"`는 **1**이지 2가 아니다.
  ⚠ 그런데 `Number(true)/Number(null)/Number('')/Number([])`가 **전부 클램프 후 1**이라,
  강제 변환을 **추가한** 포트도 리뷰어가 짚어볼 네 입력에서 **정답과 일치**한다.
  분리 증인은 `"2"`·`"1.5"`·`[2]`·`Infinity`뿐.
  → **`Option<f64>` 모델링**(`markdown_toc_width` 선례와 동일: `None` = 비-숫자)으로 강제 변환을
  **구조적으로 불가능**하게 만들고, 그 선택을 doc에 명시한다.
  ⚠ 이 리포의 `Number('')===0`·`parseInt` 무예외 선례는 **여기 해당 없음** — 끌어오지 말 것.
- **V4 — 반올림·정수화가 **없다**. `f64` 그대로 통과**(`1.35` → `1.35` 고정).
  ⚠ 소비자가 이걸 **비트 단위로** 의존한다: `persistence.ts:3027-3028`이 정규화 결과를 `!==`로 비교해
  **파일 재기록 여부를 결정**한다 → 반올림·클램프 순서 변경은 **매 로드마다 스퓨리어스 재기록**을 만든다.
- **V5 — 폴백값이 클램프 바닥과 **같은 수(1)**다.** `undefined → 1`, `NaN → 1`, `0.85 → 1`이 전부 같은 값이라
  **"센티넬 반환"과 "바닥으로 클램프"를 구별하는 픽스처가 0개**다. V1이 안 보이는 이유가 이것.

## 2. 계약 결정 — `ui_language`

- **V6 — 정확 멤버십**(`Set.has` = SameValueZero, 원소가 전부 String이라 **정확 문자열 동등**).
  trim·lowercase·로케일 태그 분해·접두 매칭 **전부 없다**.
- **V7 — ⚠⚠ 오라클이 **훨씬 느슨한 매처와 구별을 못 한다**.**
  픽스처가 정확한 멤버 6개 + `'fr'` + `null`뿐인데, `'fr'`은 **trim/lowercase/`-` 분해 매처에서도 전부 실패**한다.
  → trim하는 포트, ASCII 소문자화하는 포트, `en-US`→`en` 분해하는 포트가 **8/8 통과**.
  ⚠ **형제 모듈 `ui-locale.ts:16-31`이 정확히 그 짓을 한다**(`.trim().toLowerCase().replace(/_/g,'-')` + primary-subtag 분해)
  → 현실적인 오염 경로다. **`ui-locale`은 이 PR 범위 밖**이고 그쪽 의미론을 **끌어오면 안 된다**.
  → 핀: `'EN'`·`'en-US'`·`'ko_KR'`·`' en '`·`''`·`None` **전부 `System`**.
- **V8 — 집합이 **닫혀 있다는 게 미고정**이다.** 일곱 번째 언어(`'de'`)를 넣어도 8/8 통과 →
  **원소 수와 정확한 내용을 핀**한다.
- **V9 — `'system'`은 로케일이 아니라 **센티넬**이다.** 소비자 둘이 `language === UI_LANGUAGE_SYSTEM`으로
  **분기**한다(`main-i18n.ts:83`, `ui-locale.ts:66`) → 베어 `&'static str`이 아니라
  **`enum UiLanguage { System, En, Zh, Ko, Ja, Es }` + `as_str()`**로 모델링한다.
  여섯 상수는 `as_str()`이 내는 값과 **동일 진실의 두 번 진술**이므로 상수도 그대로 `pub`으로 낸다(소비자 6곳이 인용).
- **V10 — 호출부의 `??`를 함수 안으로 **흡수하지 말 것**.**
  `web-preload-api.ts:3571`이 `normalizeUiLanguage(updates.x ?? base.x)`인데 `??`(nullish)라
  `''`와 `0`은 **base로 대체되지 않고** 함수에 도달해 `'system'`이 된다. 시그니처는 `Option<&str>`로 받고
  **폴백 결정은 호출자 몫**임을 doc에 명시.
- **V11 — 집합 **순서는 관측 불가****(순회·spread·직렬화·export 전무) → Rust `match` arm 순서 자유.
  순서가 관측되는 곳은 `renderer/i18n/supported-languages.ts:25-32`이고 **범위 밖**이다.

## 3. 크레이트 헤더
`suaegi-misc/src/lib.rs` 헤더가 "**fifteen** helpers @ **v1.4.150-rc.0**"라고 단언한다.
→ 실제 모듈 수를 **세어서** 고치고, 베이스라인이 **모듈마다 다르다**(기존 1.4.150 / 신규 **1.4.146**)는 걸
헤더에 명시한다. 기존 모듈의 1.4.150 라벨은 **그 시점 기준이라 건드리지 않는다**.
`Cargo.toml`의 `description`/헤더 주석도 같이 맞춘다.

## 4. 오라클 & 핀
**오라클 전량**: `terminal-line-height-settings.test.ts` 7케이스, `ui-language.test.ts` 8단언.
간접 오라클도 핀으로 옮긴다: `persistence.test.ts:592-604`(0.85 → 1 저장까지),
`ipc/settings.test.ts:349-365`, `constants.test.ts:48`(기본 uiLanguage = `'system'`).

**추가 핀(오라클 침묵 — 이 PR의 본체):**
**V1 `Infinity`/`-Infinity` → 1**(가드 없는 포트를 죽이는 **유일한** 증인);
**V2 `MIN`/`MAX` 리터럴** + `3.5 → 3.0` + `0.95 → 1.0`;
V3 `None → 1`(비-숫자 전부) + `Option<f64>` 선택 근거 주석; V4 `1.35` 비트 동일 통과 + `-0.0` 입력 → `1.0`;
**V7 `'EN'`·`'en-US'`·`'ko_KR'`·`' en '`·`''`·`None` → `System`**;
**V8 원소 수 6 + 정확 내용**; V9 각 variant ↔ `as_str()` 왕복; V10 `None`은 `System`(호출자 `??` 미흡수).

*mutation:* V1 `is_finite` 가드 제거·`f64::clamp`로 교체, V2 `MIN=0.9`·`MAX=3.5`,
V3 문자열 파싱 추가, V4 `round()` 추가·min/max 순서 교환, V6/V7 `trim()` 추가·`eq_ignore_ascii_case`로·
`split('-')` 추가, V8 일곱 번째 variant 추가, V9 `as_str` 값 교환, V10 `None`을 `En`으로.

## 5. 순서
단일 PR. 두 모듈은 서로 **공유 코드가 0**이지만 둘 다 의존 0·`suaegi-misc` 소속이라 한 diff가 맞다.
`pi-overlay-ui-settings`는 **후속 PR**(§0).
불변식: **`suaegi-misc`에 추가**(§0), `is_finite` 가드 유지(V1), 캡 리터럴(V2), 강제변환 없음 + `Option<f64>`(V3),
무반올림 비트 보존(V4), 정확 멤버십(V6), **느슨한 매처 금지**(V7), 닫힌 집합(V8), `enum` + 센티넬(V9),
`??` 미흡수(V10), 헤더 정정(§3), 매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[mutation-survivor-triage]], [[orca-source-location]],
[[suaegi-impl-model-sonnet]]
