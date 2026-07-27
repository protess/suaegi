# Plan — github-work-items-query-bounds + github-project-ref-input (`suaegi-misc` 모듈 2개, 단일 PR)

조사: Explore 정찰(소스 3개·오라클 3개 통독 + 소비자 전수 grep + 렌더러 중복 shim 확인).
출처 `reference/orca/` = **v1.4.146-rc.0**. 소스 29L / 오라클 73L.
3모듈 배치의 **PR 1/2** — `gitlab-projects`는 결정 클러스터가 **완전히 분리**돼 다음 PR
(클럭 시그니처 / JS `slice` 의미론 / 신규 공개 구조체. 공유 코드도 공유 함정도 0).

## 0. 배치 — 둘 다 `suaegi-misc`, **신규 크레이트 아님**
⚠ 처음엔 `clipboard-text` import 때문에 `suaegi-filedrop`/`suaegi-terminput` 선례대로
**별도 leaf가 필요하다고 봤는데, K1을 적용하면 그 전제가 사라진다**:
인라인하면 두 모듈 다 `text.len()` 비교로 접혀 **의존할 로직이 남지 않는다**
(`get_clipboard_text_byte_length`는 `clipboard_text.rs:38-39`에서 **`text.len()`과 항등**).
filedrop/terminput이 접기를 거부한 건 정책 때문이 아니라 **cadence 기계 ~40L을 진짜로 재사용**하기 때문이다.
여기서 "재사용"할 건 언어 프리미티브 하나뿐이다.
유일한 진짜 인트라 크레이트 필요는 `js_ws::is_js_whitespace`(K3)이고 그건 **공인된 예외**다.
**29L·export 7개짜리로 크레이트를 세우지 않는다** — 최소 기존 leaf가 `suaegi-terminput` 109L/10 export다.
⚠ `suaegi-terminput`에 합치는 것도 **거부**한다: 이 워크스페이스의 leaf는 전부
**1크레이트=1소스모듈**이고 이름이 소스와 대응한다. GitHub 도메인 모듈을 PTY 크레이트에 넣으면
그 불변식이 깨지고 `GITHUB_PROJECT_REF_INPUT_MAX_BYTES`가 자기 집에서 grep되지 않는다.
"바이트 캡이 있다"는 공통점은 Orca 모듈 ~20개에 해당해 **크레이트 편성 기준으로 너무 약하다**.

## 1. 계약 결정

- **K1 — ⚠⚠ 술어는 **f64 전 정의역에서** `text.len() as f64 > max_bytes` **하나로 정확히 접힌다**.**
  두 모듈 다 `isClipboardTextByteLengthOverLimit`를 **직접** 호출하므로
  `resolveClipboardTextMaxBytes`의 S2 정규화(`clipboard_text.rs:74-79`)는 **여기 적용되지 않는다**.
  델리게이트는 `text.length > m || measure(text,{stopAfterBytes:m}).exceededLimit`인데,
  모든 코드포인트에서 `utf8_len ≥ utf16_len`이므로 `u > m ⟹ b > m`이고 유한 `m`에선 OR ≡ `b > m`.
  비유한 팔도 직접 확인했다: `m=NaN` → 양쪽 false ✓; `m=+∞` → 양쪽 false ✓;
  `m=-∞`·`m=-1` → `0 > -1`이 **`""`에서도 true** ✓; `m=2.5` → `b > 2.5` ✓.
  → **인라인 한 줄**. `u64` 시그니처 헬퍼를 호출하지 **않는다**.
  ⚠ 표현 불가 1건: JS는 **고립 서로게이트를 3바이트로 센다**(`clipboard-text.ts:181-183`). Rust `&str`엔 담기지 않는다 → 주석.
- **K2 — ⚠ `max_bytes`는 **`Option<f64>`**다. `u64`는 계약을 **반대 방향으로 뒤집는다**.**
  JS `NaN` = **캡 해제**(8GB 쿼리도 통과) ↔ Rust `NaN as u64` = `0` = **전부 거부**.
  JS `-1` = **`""`까지 전부 거부** ↔ Rust `-1.0 as u64` = `0`. 두 방향 다 최대로 틀린다.
  ⚠ 그런데 **생산 호출 14곳·테스트 12곳이 전부 인자 1개**라 `u64` 포트는 **100% green**이다.
  → 시그니처가 계약이므로 `Option<f64>`로 간다(terminput U7과 같은 논거).
- **K3 — `/\S/.test(input)`는 ECMAScript `\S`**(앵커 없음, `/u` 없음) = "비공백 문자가 **어디든 1개 이상**".
  `""` → `false`. → `input.chars().any(|c| !is_js_whitespace(c))`.
  ⚠ 오라클의 공백은 **전부 U+0020**이라 `char::is_whitespace` 포트도 `trim().is_empty()` 포트도 **통과**한다.
  결정적 발산 2개: `"\u{FEFF}"` → TS **false** / 순진한 Rust true; `"\u{0085}"` → TS **true** / 순진한 Rust false.
  ⚠ `trim().is_empty()`는 **이중으로 틀리다**(공백 집합 + "전부 공백" vs "하나라도 비공백"은
  이 술어에선 동치지만 집합이 다르다) → `js_trim` 기반 구현도 금지, **`any` + `is_js_whitespace`로**.
- **K4 — ⚠⚠ `hasBounded…`의 `&&`에서 **캡 항이 오라클상 죽어 있다**.**
  유일한 초과 픽스처(`test:38`)가 `' '.repeat(MAX+1)` — **초과이면서 동시에 전부 공백**이라
  두 항이 **각각 독립적으로** `false`를 낸다. 커버리지 행렬의 빈 칸은 **"초과 AND 비공백"** 하나뿐이고
  그게 캡 항이 **결정하는 유일한 칸**이다.
  → **`!is_too_large(input)` 항을 통째로 지워도 `test:40-43` 4/4 통과**. 이 리포 **여섯 번째** 중복 메커니즘 사례.
  → 핀: `"x".repeat(2049)` → **`false`**.
  ⚠ 정황이 더 나쁘다: `hasBounded…`의 생산 소비자는 **제출 버튼 disabled 게이트 1곳뿐**이고
  실제 거부는 다른 곳에서 일어나므로 **사용자 눈에 보이는 증상도 없다**.
- **K5 — `&&`의 **항 순서는 등가**다**(두 피연산자가 순수·전역·부작용 0, 예외 없음).
  교환해도 모든 입력에서 동일 → **mutation 대상이 아니라 등가로 문서화**한다([[mutation-survivor-triage]] 원인 ②).
- **K6 — ⚠ 상수 3개가 **전부 심볼 참조뿐**이다.**
  `8*1024`, `2*1024`, 그리고 에러 문자열 `'Project reference is too large to resolve.'` —
  **리포 전체 grep에서 문자열 리터럴이 정의부에만 있다**(테스트가 import조차 안 한다).
  게다가 `query-bounds.test:25`는 `query.length < MAX`라는 **자기참조 검사**라 고정력이 0이다.
  → **셋 다 리터럴 `assert_eq!`**.
- **K7 — ⚠ 경계 픽스처가 **전부 ASCII**라 UTF-8 바이트 ≡ UTF-16 단위 ≡ `.length`가 **일치**한다.**
  다바이트 픽스처(`:23`, `:31`)는 캡을 **4바이트 초과**한 지점이지 **캡 위가 아니다**.
  → 폭별 off-by-one(2바이트를 3으로 세기 등) 포트가 살아남는다.
  → 핀: `"é".repeat(1024)` = **정확히 2048B** → `false`, `+ "x"` → `true`. 8KiB 캡도 동일. **3바이트 문자 추가.**
- **K8 — `get_github_project_ref_input_byte_length`는 픽스처가 **하나뿐**이다**(`'\u{e9}'` → 2).
  `text.len()`·`chars().map(len_utf8).sum()`·`encode_utf16().count()+1`·2바이트 전용 표까지 **전부 통과**한다.
  → 핀: `""`→0, `"가"`→3, `"😀"`→4, `"a가😀"`→8.
  ⚠ 이 export는 **생산 소비자가 0개**다 → `pi_overlay_ui_settings` 선례대로 기록하고 배선하지 않는다.
- **K9 — trim이 **없다**(S8).** 모듈은 절대 트림하지 않는다.
  ⚠ 호출자들이 **서로 다르게** 트림한다: `client.ts:1351`은 트림 후 캡, `github.ts:2740`은 원문 캡;
  `ProjectPicker.tsx:338`은 원문, `project-view.ts:1703`은 트림본. → **상류의 비대칭이므로 기록만** 하고 고치지 않는다.
- **K10 — 에러 문자열은 **메타데이터 전용**이어야 한다(S9).** IPC `validation_error` 페이로드로 나가고
  상류 테스트가 **초과 입력(비밀 포함)을 되울리지 않음**을 단언한다 → 페이로드 미포함을 핀.
- **K11 — 렌더러의 `slices/github-work-items-query-bounds.ts`는 **순수 재export shim**이다.
  Rust에선 no-op → **`shared/` 쪽만 이식**한다. 그쪽 중복 테스트도 신규 커버리지가 0이다.

## 2. 오라클 & 핀
**오라클 전량**: `github-work-items-query-bounds.test.ts` 3케이스, `github-project-ref-input.test.ts` 5케이스.
렌더러 중복 테스트는 **신규 커버리지 0**이라 이식하지 않는다(K11).

**추가 핀(오라클 침묵 — 이 PR의 본체):**
**K1/K2 `max_bytes` 6팔**(`None`·`NaN`·`+∞`·`-1`·`0`·`2.5`) **두 술어 각각**;
**K3 U+FEFF → `false`, U+0085 → `true`** + U+00A0·U+2028·U+3000 → `false`;
**K4 `"x".repeat(2049)` → `false`**(캡 항이 결정하는 유일한 칸);
**K6 상수 3개 리터럴**(에러 문자열 포함); **K7 `"é".repeat(1024)` 정확히 캡 → `false`, +1자 → `true`** (양쪽 캡);
**K8 바이트 길이 4종**; K10 에러에 입력 미포함; `hasBounded("")` → `false`.

*mutation:* K1 `u64` 헬퍼 호출로·`>=`로, K2 `max_bytes`를 `u64`로, K3 `char::is_whitespace`로·`trim().is_empty()`로,
**K4 캡 항 삭제**, K6 상수 값 변경 각각, K7 폭별 off-by-one(2바이트를 3으로), K8 `encode_utf16().count()`로,
K9 트림 추가, K10 에러에 입력 삽입.
**K5 항 순서 교환은 mutation 대상 아님**(등가 — §1 증명).

## 3. 순서
단일 PR. 두 모듈은 **하나의 결정 클러스터**다(수치 계약 K1/K2 + `\S` K3 + 상수 리터럴 K6).
크레이트 헤더 모듈 수(현재 twenty-six)·목록·`Cargo.toml` 설명을 같이 고친다(신규 2개는 **v1.4.146-rc.0**).
불변식: **`suaegi-misc`, 신규 크레이트 없음**(§0), **len 비교로 인라인**(K1), **`Option<f64>`**(K2),
**`is_js_whitespace`**(K3), **캡 항 직접 핀**(K4), 항 순서는 등가(K5), **상수 리터럴**(K6),
**다바이트 정확 경계**(K7), 바이트 길이 4종 + 미배선(K8), 트림 없음 + 상류 비대칭 기록(K9),
메타데이터 전용 에러(K10), shim 미이식(K11), 매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[mutation-survivor-triage]], [[suaegi-misc-placement-rule]],
[[orca-source-location]], [[suaegi-impl-model-sonnet]]
