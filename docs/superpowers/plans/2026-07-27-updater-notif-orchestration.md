# Plan — updater-windows-signature-check + agent-notification-id + orchestration-task-summary (`suaegi-misc` 모듈 3개, 단일 PR)

조사: Explore 정찰(소스 3개·오라클 3개 통독 + 소비자 추적 + **0x80–0x10FFFF 전수 스캔**으로 케이스 폴딩 검증).
출처 `reference/orca/` = **v1.4.146-rc.0**. 소스 73L / 오라클 152L. 세 모듈 다 import 0, 외부 의존 0.

## 0. 배치 — 셋 다 `suaegi-misc`, 단일 PR
[[suaegi-misc-placement-rule]]: 셋 다 런타임 import 0·외부 의존 0. `TASK_SPEC_BRIEF_LENGTH = 160`은
**실격 사유 아님**. `orchestration_task_summary`만 `js_ws`가 필요하고 그건 **공인된 예외**.
`[dependencies]`는 **계속 빈다**: `encode_uri_component`(20L, `suaegi-forge/src/repo_icon.rs:272-292`)와
`utf16_slice_prefix`(10L, `repo_icon.rs:188-200`)를 **모듈 로컬로 복사**한다(모듈별 복제 = 이 리포 헌장).
**`regex` 불필요**: 서명 모듈은 정규식이 **아예 없고**(`contains`), `\s+`는 선형 스캔 한 번이다.
헤더 모듈 수(현재 twenty-nine)·목록·`Cargo.toml` 설명을 **셋 다** 반영해 한 번에 고친다
(쪼개면 공유 파일 2개의 doc churn만 3배가 되고 격리 이득은 0).

## 1. ⚠⚠ N1 — 선례 반전: 여기선 `to_lowercase()`가 맞고 `to_ascii_lowercase`가 틀리다
이 모듈은 **정규식이 없다**. `String.prototype.toLowerCase()` + `.includes()`다.
`.toLowerCase()`는 **풀 유니코드** Default Case Conversion이고, Rust `str::to_lowercase`도 그렇다 → **일치**.
[[js-lowercase-two-mechanisms]]가 말하는 두 메커니즘 중 **이쪽**이다.
⚠ `codex_auth_errors`/`worktree_submodule_removal`의 `to_ascii_lowercase` 처방은
**소스가 `/…/i` 정규식(`/u` 없음)이기 때문**이지 일반 규칙이 아니다. 정찰이 양쪽을 실증했다:
`/k/i.test('K')` → **false**, 그런데 `'K'.toLowerCase()` → **`"k"`**.
→ **`to_lowercase()`를 쓴다.** 이 반전을 헤더에 **크게** 적는다(후속 포트가 인용할 지점이다).

⚠ 단, **이 두 문구에서는 둘이 관측상 등가**다(정찰 전수 스캔):
ASCII 문자로 접히는 비-ASCII는 `U+212A`→`k`와 `U+0130`→`i`+U+0307 **딱 둘**인데,
두 문구에 **`k`가 없고**(`get-authenticodesignature`, `not signed by the application owner`),
`U+0130`은 결합문자를 끼워 넣어 **양쪽 다 매치를 깬다**.
→ **`to_ascii_lowercase` 변이는 등가**다(원인 ②: 문서화, mutation 대상 아님).
문구에 `k`가 하나 생기는 순간 조용히 발산하므로 `to_lowercase()`가 결합을 없앤다.
⚠ `to_lowercase()`는 **길이 변경 가능**(`İ`→2자, 최종 시그마) — 제자리 소문자화/길이 보존 가정 금지.

## 2. ⚠⚠ N2 — 보안: veto가 **미고정**이고, 지우면 공급망 다운그레이드
`isWindowsSignatureCheckUnavailableFailure`는 3-leaf다(`:9-13`):
① `'not signed by the application owner'` 포함 → **`false`**(veto, **먼저** 평가) ②
`'get-authenticodesignature'` 포함 → `true` ③ 그 외 → `false`.
`isWindowsSignatureMismatchFailure`는 단일 leaf(`:22`, veto 없음, 독립적으로 재소문자화).

⚠ **오라클의 veto 테스트(`test:28-35`)가 veto를 고정하지 못한다** — 픽스처에
`get-authenticodesignature`가 **없어서** ②만으로도 이미 `false`다.
→ **`:10-12`를 통째로 지운 구현이 오라클 전량 통과**한다. `test:55-63`도 단일 문구라 못 잡는다.

**결과(소비자 추적):** `UpdateCard.tsx:388-407`의 mismatch는 `variant:'security'`,
**재시도 버튼 없음**, `releaseUrlForVersion(null)`(주석 `:401-402`: 직접 링크는 퍼블리셔 검사 우회 유도).
`:408-423`의 check-unavailable은 **`Retry Download` 기본 액션** + **거부된 그 버전의 릴리스 링크**.
→ **진짜 mismatch를 unavailable로 오분류하면**, 퍼블리셔 서명이 실패한 인스톨러가
안심시키는 문구와 함께 **클릭 한 번 거리**가 된다. 역방향 오분류는 가용성 손실뿐이다.
→ **비대칭이므로 veto는 최대한 공격적으로 유지**한다.
→ **핀: 두 문구를 다 포함하는 메시지**(`'Get-AuthenticodeSignature failed: not signed by the application owner'`)
→ unavailable **`false`**, mismatch **`true`**.

- **N3 — 이 리포 **일곱 번째** 중복 메커니즘**: 모듈 내 veto와 호출자의 mismatch-우선 순서
  (`UpdateCard.tsx:366`/`:388`)가 **각각 독립적으로** 같은 UI를 만든다 → veto 삭제가 안 보인다.
- **N4 — mismatch 픽스처가 **전부 소문자**다**(`test:49,:56,:66`) → **소문자화 없는 `contains`도 통과**.
  → 핀: 대문자 문구.
- **N5 — 비앵커 부분문자열, trim 없음, ANSI 스트립 없음**, 빈 문자열 → 양쪽 `false`.

## 3. 계약 결정 — `agent_notification_id`

- **N6 — ⚠⚠ `encodeURIComponent`는 **하중을 받는다**(장식이 아니다).**
  실제 입력이 **둘 다 `:`를 포함**한다: `paneKey`는 `tabId:leafUuid`, `worktreeId`는 `repoId::path`.
  인코딩이 `:`→`%3A`, `%`→`%25`로 escape하므로 4-세그먼트 분해가 **모호하지 않고 사상이 단사**다.
  → **`format!("agent:{w}:{p}:{t}")`로 이식하면 진짜 충돌 버그**다
  (`w='a', p='b:c'` ≡ `w='a:b', p='c'`). 오라클은 **못 본다**.
  ⚠ `ephemeral_setup_terminal_worktree_id`처럼 "상류 위험 보존"하는 케이스가 **아니다** — 여기엔 보존할 위험이 없다.
  unreserved 집합 `A-Za-z0-9 -_.!~*'()`는 `repo_icon.rs:272-292`와 동일.
- **N7 — ⚠⚠ 오라클이 **id 리터럴을 단 한 번도 단언하지 않는다** → 세 모듈 중 **가장 미제약**이다.**
  통과하는 틀린 구현: 인코딩 없음 / 구분자 `|` + 필드 순서 뒤바꿈 / **타임스탬프만** / 반올림 잘못 / `NaN`에 `Some`.
  오라클이 강제하는 건 "`stateStartedAt`의 함수임"과 "필드 하나라도 없으면 `None`"뿐.
  → **리터럴 id 핀**을 직접 쓴다.
- **N8 — `Math.trunc` + `String(number)` → `Option<f64>`**(`i64` 아님 — `1e+21` 표기 때문).
  `stateStartedAt === 0`은 **수락**되어 `…:0`이 된다(truthiness가 그 앞에서 멈춘다).
  `worktreeId`/`paneKey`의 `''`는 **truthiness로 거부**(타입 검사 아님) → `.filter(|s| !s.is_empty())`.
- **N9 — `encodeURIComponent`는 고립 서로게이트에서 **`URIError` throw**한다.**
  Rust `&str`엔 담을 수 없어 **구조적으로 도달 불가** → `Result`로 이식하지 말고 **주석으로 기록**.
- **N10 — 결정성 테스트(`test:5-13`)는 **공허**하다**(모든 순수 함수가 통과). 커버리지로 세지 말 것.

## 4. 계약 결정 — `orchestration_task_summary`

- **N11 — 3단계**: ① `\s+`→`' '` + `trim`을 **모든 태스크에** 적용(`:7`, 잘리지 않아도) ②
  `spec.length > 160`(**strict `>`**) ③ 잘릴 때만 절단 + `…`(**U+2026 1개**, `...` 아님. **1 UTF-16 단위 / 3 UTF-8 바이트**).
  원소 **개수는 줄지 않는다**(`.map` 1:1).
- **N12 — ⚠ 160은 **상한이지 불변식이 아니다**.** 159가 **두 경로로** 도달한다:
  서로게이트 드롭, 그리고 절단면의 `trimEnd`. 출력 길이를 160으로 단언하는 핀은 **틀린다**.
- **N13 — ⚠ 절단 단위는 **UTF-16 코드 단위**다**(`.length`/`.slice`). Rust `&s[..159]`는 **패닉**한다
  (`'é'.repeat(200)`이면 단위 159 = 바이트 318, 문자 중간).
  → **`utf16_slice_prefix` 스냅다운**(로컬 복사).
  ⚠ **중요**: 스냅다운은 TS의 `slice(0,159)` + high-surrogate 드롭(`:24`)과
  **Rust로 표현 가능한 모든 입력에서 정확히 등가**다(정찰 케이스 분석).
  유일한 발산은 **고립 low surrogate**인데 `&str`이 담을 수 없다.
  → **`:24`의 서로게이트 검사를 `char`에 대해 재구현하지 말 것 — Rust에선 죽은 분기다.** 등가 근거를 주석에.
- **N14 — `\s+`·`trim`·`trimEnd`는 전부 ECMAScript 공백**(정찰 실증: `"a"+U+FEFF+"b"` → `"a b"` **접힘**,
  `"a"+U+0085+"b"` → **불변**). → `is_js_whitespace` 스캔 + `js_trim` + **로컬 `js_trim_end`**
  (`js_ws`에 없다; `suaegi-quickcmd/src/lib.rs:435-437` 선례대로 2줄 로컬).
- **N15 — 제네릭 `T`는 `{spec}`으로만 읽히고 `...task`로 통과된다.**
  Rust에선 `serde_json::Value` 없이 표현 불가이고 이 크레이트는 serde를 **거부**한다 →
  **변환과 통과를 분리**한다:
  ```rust
  pub struct AbbreviatedSpec { pub spec: String, pub spec_truncated: bool }
  pub fn abbreviate_orchestration_task_spec(spec: &str) -> AbbreviatedSpec
  pub fn abbreviate_orchestration_tasks<T>(tasks: &[T], get_spec: impl Fn(&T) -> &str) -> Vec<AbbreviatedSpec>
  ```
  `...task` 스프레드는 **호출자 책임**(`worktree_submodule_removal`이 `String(error)`에 한 처리와 동형).
- **N16 — ⚠ 절단 픽스처가 **한계에서 멀다****(290·200·10 단위) → **159/160/161에 아무것도 없다**.
  `>` vs `>=`도, 상수 `160` vs `159` vs `161`도 **전부 안 보인다**. → 세 지점 핀 + **상수 리터럴 핀**.
- **N17 — ⚠ 전 코퍼스가 사실상 ASCII**라 바이트/단위 구분이 숨는다(😀 하나 제외).
  → **`'é'.repeat(200)`** 핀(단위 159 = 바이트 318): 바이트 슬라이싱 포트는 패닉, 순진한 `chars().take`는 우연히 맞는다.
- **N18 — 서로게이트 픽스처(`test:34-36`)가 **길이를 단언하지 않는다**** →
  "160으로 되메움", "2단위 드롭", 정답 스냅다운이 **구별되지 않는다**. → **길이 159를 직접 핀**.
- **N19 — 소비자 미묘함 기록만**: `cli/handlers/orchestration.ts:583-584`가 `.some(t => t.spec_truncated === undefined)`로
  가드해서 **혼합 응답**에선 이미 잘린 행의 플래그를 `false`로 뒤집는다. 이식 모듈의 버그는 아니므로 **주석만**.

## 5. 오라클 & 핀
**오라클 전량**: 서명 68L, notification-id 46L, task-summary 38L.

**추가 핀(오라클 침묵 — 이 PR의 본체):**
**N2 두 문구 동시 포함 메시지**(veto 삭제를 죽이는 유일한 핀); **N4 대문자 문구**; N5 빈 문자열·무관 메시지;
N1 `U+212A`·`U+0130` 현행 동작 회귀 가드(등가지만 문구 변경 시 파수꾼);
**N6/N7 리터럴 id 4종**(`:` 포함 필드로 충돌 부재 증명 — `w='a',p='b:c'` vs `w='a:b',p='c'`);
N8 `stateStartedAt=0` 수락 + `''` 거부 + 소수/거대값 `Math.trunc`;
**N16 159/160/161 경계 + 상수 리터럴**; **N17 `'é'.repeat(200)`**; **N18 서로게이트 케이스 길이 159**;
N12 `trimEnd`로 159가 되는 케이스; N11 잘리지 않는 태스크도 공백 정규화됨; N14 U+FEFF 접힘 / U+0085 유지.

*mutation:* N1 `to_ascii_lowercase`로(**등가 — 대상 아님**), **N2 veto 삭제**, N4 소문자화 제거,
N6 인코딩 생략·구분자 변경·필드 순서, N7 타임스탬프만 반환, N8 `0`을 거부·`i64`로·`''` 허용,
N11 공백 정규화를 절단 시에만, N12 출력 길이를 160으로 강제, **N13 바이트 슬라이싱·`chars().take`**,
N14 `char::is_whitespace`·`str::trim`, N16 `>=`로·상수 159/161, N18 서로게이트 드롭 생략·2단위 드롭.

## 6. 순서
단일 PR. 셋이 서로 상호작용하지 않고 같은 baseline·같은 크레이트다.
쪼갠다면 **서명 모듈을 단독으로 먼저** 보낸다(보안 의미론 + N1 선례 반전을 후속 포트가 인용한다).
불변식: `suaegi-misc` 3모듈 + 로컬 헬퍼 복사(§0), **`to_lowercase()` 선례 반전**(N1),
**veto 유지 + 두 문구 핀**(N2), 대문자 핀(N4), **인코딩은 하중**(N6), 리터럴 id 핀(N7),
`Option<f64>` + `0` 수락(N8), `URIError` 기록만(N9), UTF-16 스냅다운 + 죽은 분기 미구현(N13),
ECMAScript 공백 3곳(N14), 클로저 접근자(N15), **경계 3점 + 상수 리터럴**(N16),
`é` 핀(N17), 서로게이트 길이 핀(N18), 매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[mutation-survivor-triage]], [[mutation-harness-mtime-trap]],
[[js-lowercase-two-mechanisms]], [[suaegi-misc-placement-rule]], [[orca-source-location]],
[[suaegi-impl-model-sonnet]]
