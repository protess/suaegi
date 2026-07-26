# Plan — nested-repo-telemetry (신규 `suaegi-nested-telemetry` 크레이트, 의존 0, 단일 PR)

조사: Explore 정찰(소스 233L + 오라클 224L 통독, 하류 zod 스키마와 생산 호출자까지 확인).
출처 `reference/orca/` = **v1.4.146-rc.0**.

## 0. 배치 — 신규 leaf, `[dependencies]` **빈다**
import가 **전부 type-only**라 런타임 의존이 0이다. `.trim()` 0회, 정규식 0개, 문자열 길이 0개
→ **`suaegi-misc`조차 필요 없다**(최근 leaf 중 처음).
`suaegi-misc` 헌장("작고 자족적인 순수 헬퍼, 정책 없음")에 안 맞는다 — 여기엔 **4-way/3-way 결정 트리를 가진
페이로드 빌더 3개 + 교차 프로세스 스키마 계약인 어휘 6개**가 있다.
`serde`/`serde_json` **금지**(M4), `rand`/`uuid` **금지**(M9, setupseq 선례).

## 1. 계약 결정

- **M1 — ⚠⚠ `Infinity`는 **0**으로 캡되지 500이 아니다. 그리고 **캡이 버킷보다 먼저** 돈다.**
  `O:78-80`이 `Number.isFinite` 게이트로 `±Infinity`·`NaN`을 **`0`**으로 만들고,
  `O:92`가 그 캡을 **먼저** 적용한다 → `bucket(Infinity) == "0"`, `bucket(-5) == "0"`.
  버킷을 먼저 돌리거나 `Infinity`를 캡으로 포화시키면 둘 다 `"16+"`가 된다.
  ⚠ **오라클은 `Infinity`를 어디에도 넘기지 않고**, `bucket()`에 음수·`NaN`·소수를 넘기지도 않는다.
- **M2 — ⚠⚠ 버킷 사다리는 전부 `<=`이고, 오라클은 **상단 경계만** 검증한다.**
  `O:93` `== 0`, `O:96` `== 1`, `O:99` `<= 3`, `O:102` `<= 7`, `O:105` `<= 15`, `O:108` fall-through.
  경계값은 **항상 아래쪽 버킷**에 들어간다. 오라클이 고정하는 건 `0,1,3,7,15,16`(상단)뿐 —
  **하단 경계 `4`·`8`과 `500`·`501`은 버킷 단언이 하나도 없다**. `0..=16` 전 표를 핀으로.
- **M3 — ⚠⚠ 카운트를 `usize`/`u32`로 모델링하면 분기 넷이 통째로 죽는다.**
  네 함수가 전부 `number`를 받는다. 부호 없는 정수로 받으면 `O:78-80`·`O:85-87`이 도달 불가가 되고
  `cap(-1)`을 표현할 수 없으며 `shouldEmit`의 `> 0`이 무의미해진다.
  → **경계는 `f64`**, 정수 타입은 **출력 페이로드 필드에만**. 선례: filedrop L6 / `clipboard_text` S2·S6.
- **M4 — ⚠⚠ `failedCount ?? selectedCount`(`O:210`)의 `??` vs `||`가 **오라클에 완전히 비가시**다.**
  `failedCount === 0`이면 `??`는 `0`을 유지하고 `||`는 `selectedCount`로 대체해
  **`outcome`이 `success` → `partial_failure`로 뒤집히고** `failed_count`/버킷도 바뀐다.
  오라클은 `failedCount: 0`을 **두 번** 만들면서(`T:108`, `T:189`) `failed_count`도 `outcome`도
  **한 번도 단언하지 않는다**. **이 모듈 최대의 조용한 발산 위험.**
  → `failed_count: Option<f64>`로 모델링하고 `unwrap_or(selected_count)`. "0이면 부재" 관용구는 `||` 버그를 재현한다.
- **M5 — ⚠⚠ 오라클의 결과-페이로드 단언은 `toMatchObject`(부분 일치)라 **버킷 필드 5개가 전부 미고정**이다.**
  `T:144`. **버킷 필드를 아예 방출하지 않는 구현도 통과한다.** 다섯 개 전부 명시 핀.
  (`toEqual`인 건 scan·action 페이로드 둘뿐 — `T:56`, `T:79`.)
- **M6 — `outcome`은 `accepted === 0`을 **먼저** 검사한다**(`O:213`).
  `{imported:0, alreadyKnown:0, failed:0}` → **`failed`**이지 `success`가 아니다.
  순서를 바꿔도 오라클은 못 잡는다(`'failed'`와 `'success'`가 **한 번도 단언되지 않는다**). 3×3 진리표 핀.
- **M7 — `all_selected`는 `normalize`(캡 없음, floor 있음)를 쓰고 **캡 이전** 값으로 계산된다**(`O:191`, `O:231`).
  `rawFound > 0` 가드 때문에 **0/0은 `false`**다. `T:92-115`(600/500)가 raw vs capped는 가르지만
  **normalize vs 그냥 통과는 못 가른다** → `foundCount: 3.5, selectedCount: 3`이면 JS는 둘 다 floor해 **`true`**,
  순진한 비교는 `false`. 커버리지 0.
- **M8 — ⚠⚠ 어휘 6개(리터럴 20개)는 **오라클과 접촉이 0**이다.**
  `O:9`, `:12`, `:15-20`, `:23-28`, `:31`, `:34`를 테스트가 **import조차 하지 않는다**.
  `'partial-failure'`·`'no-nested-repos'`·`'openAsFolder'`로 써도 **100% 초록**이고,
  하류 `z.enum`(`telemetry-events.ts:890-896`)과 count↔bucket `superRefine`(`:901-919`)에 대해 100% 고장이다.
  **20개 리터럴 전부 명시 핀.** ⚠ 순서는 load-bearing이 **아니다**(zod는 집합 멤버십) — filedrop L7과 다르다.
  한 번도 생산·단언되지 않는 것들: `open_as_folder`, `back`, `git_repo`, `no_nested_repos`, `scan_failed`,
  `success`, `failed`.
- **M9 — `MAX = 500`은 `T:38`이 아니라 `T:111`이 고정한다.**
  `T:38`은 **심볼릭**이라 `MAX ≤ 999`면 뭐든 통과한다. 리터럴 `.toBe(500)`은 **다른 함수를 테스트하는**
  `T:111-112`에 있다. 직접 `assert_eq!(MAX, 500)` 핀 추가.
  ⚠ 생산에선 상류가 이미 500으로 clamp하므로(`nested-repo-discovery.ts:71-74`) 이 캡은 **오라클만이 지킨다**.
- **M10 — 조건부 spread(`O:162`)는 **키 자체를 생략**한다.** `scan`이 없으면 `selected_path_kind` 키가 없다
  (하류가 `.strict()` + `.optional()`이라 부재와 `undefined`가 계약상 다르다).
  ⚠ **`scan: null` 테스트가 아예 없어** `'scan_failed'` 분기(`O:150`)가 통째로 미커버인데
  **생산에선 살아 있는 경로**다(`useAddRepoNestedImportFlow.ts:211-219`).
- **M11 — `shouldEmit`의 세 conjunct는 전부 **truthiness**다**(`O:116`).
  `attemptId: ''` → **false**(빈 문자열도 falsy), `null` → false; `isBusy`는 `!isBusy`라 `false`·부재 둘 다 통과;
  `selectedCount`는 **정규화 안 된 raw**라 `0.5` → true, `NaN` → false.
  오라클은 `isBusy: true`와 부재만 본다 → `Option::is_some()`만 쓰면 초록이면서 틀린다.
- **M12 — UUID 생성기는 오라클 정규식(`/^[0-9a-f-]{36}$/`)보다 **와이어 계약이 훨씬 강하다**.**
  하류가 `z.string().uuid()`(`telemetry-events.ts:899`)다. 오라클 정규식은 대시 위치·그룹 크기·version·variant를
  **하나도 안 본다**(`'-'×36`도 통과). 그리고 Node ≥19에선 `randomUUID()` 조기 반환만 실행돼
  **fallback 전체가 CI에서 죽은 코드**다.
  → version nibble(`O:135`)·variant(`O:136`)·8-4-4-4-12(`O:138`)를 이식하고 **직접 핀**:
  길이 36, 대시 인덱스 8/13/18/23, 나머지 소문자 hex, `s[14]=='4'`, `s[19] ∈ {8,9,a,b}`.
  `rand`/`uuid` 크레이트 대신 **setupseq 선례**(`std::collections::hash_map::RandomState` + `SystemTime`)를 쓰고,
  `globalThis.crypto` 선호 분기는 Rust 대응물이 없음을 문서화.

## 2. 오라클 & 핀
**오라클 전량**(`T:34-223`).

**추가 핀(오라클 침묵 — 이 PR의 실제 가치):**
M1 `Infinity`/`-Infinity`/`NaN`/음수/소수 → `"0"`(캡 선행 증명); M2 `0..=16` 전 표 + `500`·`501`;
M3 `f64` 경계의 음수·비정수; **M4 `failedCount: Some(0)`이 `success`를 유지**(`||` 버그 킬);
**M5 결과 페이로드의 버킷 5개 전부**; M6 outcome 3×3 진리표(`failed`·`success` 포함);
M7 `3.5/3` → `all_selected == true`·`0/0` → `false`; **M8 리터럴 20개 전부**;
M9 `assert_eq!(MAX, 500)`; M10 `scan: None` → `scan_failed` + `selected_path_kind` **키 부재**·
`truncated`/`timedOut` true; M11 `attemptId: Some("")`·`None`·`isBusy: Some(false)`·`0.5`·`NaN`;
M12 UUID 형태 전량; 그리고 정찰이 짚은 나머지 무커버: `git_repo`/`no_nested_repos` 분기,
`all_selected == true`, `open_as_folder`/`back` 액션, `result: None`, `imported+alreadyKnown > 500`.

*mutation:* M1 버킷 선행·`Infinity`를 MAX로, M2 `<=`→`<`·범위 이동, M3 `u32` 파라미터,
**M4 `??`→`||`**, M5 버킷 필드 제거, M6 outcome 순서 교환, M7 캡 이후 값으로 계산·`floor` 제거,
M8 각 리터럴 변경, M9 MAX 값 변경, M10 키를 `null`로 방출, M11 `is_some()`만·`is_busy == Some(true)`,
M12 version/variant nibble 제거.

## 3. 순서
단일 PR. 빌더 3개가 전부 `cap`/`bucket`을 호출하고 오라클 최대 테스트가 두 빌더를 가로지른다 → seam 없음.
불변식: 의존 0(§0), **캡 선행 + `Infinity`→0**(M1), `<=` 사다리 전 표(M2), `f64` 경계(M3),
**`??` 의미론**(M4), 버킷 필드 전부 방출(M5), outcome 순서(M6), pre-cap `normalize`(M7),
**와이어 리터럴 20개**(M8), `MAX` 직접 핀(M9), 키 생략(M10), truthiness 3종(M11), UUID 형태(M12),
매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[mutation-survivor-triage]], [[orca-source-location]],
[[suaegi-impl-model-sonnet]]
