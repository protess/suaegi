# Plan — hosted-review-refs + base-ref-search-result + worktree-base-ref (`suaegi-misc` 모듈 3개, 단일 PR)

조사: Explore 정찰(소스 5개·오라클 5개 통독 + 소비자 전수 grep + `suaegi-git` 실사).
출처 `reference/orca/` = **v1.4.146-rc.0**. 소스 57L / 오라클 96L.
배치 5개 중 **3개가 이 PR**, 나머지 2개(`worktree-submodule-removal`, `ephemeral-…-worktree-id`)는 **다음 PR**.
배치 내부 의존 **0**(유일한 import가 `base-ref-search-result:1`의 **type-only**라 런타임 간선 없음).

## 0. 배치 — 셋 다 `suaegi-misc`, **`suaegi-git` 아님**
[[suaegi-misc-placement-rule]] 적용: 런타임 import 0, 외부 의존 0 → `suaegi-misc`.
⚠ 경쟁 후보 `suaegi-git`을 실사한 결과 **결정적으로 부적합**하다:
의존이 `tokio`·`tempfile`·`serde_json`·`regex`·`url`·`libc`·**`suaegi-misc` 자신**이고,
21모듈 **10,528L**의 I/O 크레이트다(`runner.rs`가 git 바이너리를 spawn, `fs.rs`/`write_ops.rs`가 파일시스템).
거기 두면 의존 위치가 **엄격히 악화**되는데 얻는 게 없다 — `suaegi-git`은 지금도 `suaegi-misc`를 **소비할 수 있다**.
또한 그쪽 `refname.rs`는 `GitError`에 **결합**돼 있고 `worktree.rs`는 `GitRunner` 호출 덩어리라,
순수 `worktree_base_ref.rs`를 옆에 두면 누가 프로브를 러너에 배선해 **주입 콜백 계약을 파괴**할 유인이 생긴다.
mutation 격리성(= `suaegi-misc`가 의존 0을 유지하는 이유)도 무너진다.
**향후 신규 leaf `suaegi-gitref` 트리거 2개**(그때 가서 옮긴다, 지금은 아님):
① git-ref 모듈이 6개째 큐에 들어올 때(`mergeBaseRefSearchResultGroups`·`isFullGitObjectId`가 이미 보인다),
② `worktree_base_ref`가 주입이 아닌 실제 git 프로브를 갖게 될 때.

## 1. 계약 결정 — `hosted_review_refs`

- **G1 — ⚠⚠ 정규식 셋 다 `g` 플래그가 **없고** `^` 앵커다 → **각각 최대 1회 치환**.**
  Rust `String::replace`는 **전역**이고 `Regex::replace`도 **비앵커**다 → **`strip_prefix`로만** 쓴다.
  ⚠ 오라클 4픽스처가 **전부 접두사가 위치 0에 정확히 1회, 정확한 소문자**라
  전역·비앵커 포트가 **4/4 통과**한다. 발산 입력: `feature/refs/heads/x`, `release/origin/patch`,
  `origin/origin/main`, `my-origin/main`. → **네 개 다 핀.**
  실제 브랜치명 `release/origin-sync`가 생산에서 조용히 망가지는 종류다.
- **G2 — `refs/remotes/[^/]+/`는 **두 단계 스캔**이다**: `refs/remotes/` 제거 → **다음 `/`까지** 제거.
  ⚠ `[^/]+`는 **1자 이상**을 요구한다 → `refs/remotes//x`는 **매치 실패**(세그먼트가 비었다) → **무변화**.
  `refs/remotes/origin`(후행 `/` 없음)도 **무변화**. 둘 다 커버리지 0 → 핀.
- **G3 — `.trim()`은 ECMAScript 공백**(`:3`, 유일 발생) → **`suaegi_misc::js_trim`**.
  오라클이 ASCII 공백만 써서 **발산이 전혀 고정돼 있지 않다** → `str::trim` 포트가 그냥 통과.
  핀: `"\u{FEFF}refs/heads/x\u{FEFF}"` → `"x"`, `"\u{85}main"` → **`"\u{85}main"`**(NEL은 JS 공백이 아니다).
- **G4 — ⚠ `normalizeHostedReviewBaseRef`의 **2단계 순서가 미고정**이다**(`:9` head 정규화 → `:10` `^(origin|upstream)/`).
  두 base 픽스처 모두 순서를 바꿔도 통과한다. → 핀 입력 **`origin/refs/heads/x`**:
  정답 **`refs/heads/x`**(head 정규화가 먼저 돌지만 `origin/`으로 시작해 아무것도 못 벗기고, 그 다음 `origin/`이 벗겨진다),
  순서를 바꾼 포트는 **`x`**. 이 한 입력이 유일한 분리 증인이다.
- **G5 — `normalizeHostedReviewBaseRef`가 head 버전에 **위임**한다**(`:9`) — 중복 구현 금지.

## 2. 계약 결정 — `base_ref_search_result`

- **G6 — ⚠ `startsWith(p) && length > p.length` 가드는 **`refName === p`일 때만** 동작을 바꾼다**(중복 메커니즘 쌍).
  Rust `strip_prefix`가 `Some("")`을 주는 지점이 정확히 이 갈림길이다.
  픽스처가 그 입력을 **주지 않아** 가드 유무 둘 다 green.
  → 핀: `derive("origin/") == "origin/"`, `derive("upstream/") == "upstream/"`(**빈 문자열 아님**).
- **G7 — 접두사 목록은 `["origin/", "upstream/"]` **순서대로 첫 매치**다**(`:3`, 모듈 private).
  ⚠ 상수 자체가 심볼로만 쓰여 값이 틀려도 통과 → **리터럴 핀**.
- **G8 — ⚠⚠ `legacyBaseRefSearchResult`가 derive를 **실제로 호출하는지 미고정**이다.**
  유일 픽스처(`test:11-14`)가 **항등 케이스**라 `{ref_name: r, local_branch_name: r}`로 배선을 **생략한 포트가 통과**한다.
  ⚠ 게다가 **생산 소비자를 가진 건 이쪽뿐**이고(`web-preload-api.ts:1450`, `runtime-repo-client.ts:76`),
  제대로 검증되는 `deriveLegacyLocalBranchName`은 **소비자가 0개**다. → 핀: `legacy("origin/main")`이
  `{ref_name:"origin/main", local_branch_name:"main"}`.
- **G9 — 타입은 필드 2개 전부 **非옵션 `String`**`(types.ts:418-421`):
  `struct BaseRefSearchResult { ref_name: String, local_branch_name: String }` + `PartialEq`(오라클이 `toEqual`).
  **`serde`는 지금 넣지 않는다** — 이 타입이 IPC 페이로드이긴 하지만(`runtime-types.ts:713`)
  이 모듈은 직렬화하지 않는다. `suaegi-workspace-cleanup` 선례대로 **헤더에 기록만** 하고
  실제로 프로세스 경계를 넘을 때 optional feature로 추가한다.

## 3. 계약 결정 — `worktree_base_ref`

- **G10 — ⚠⚠ **단락(short-circuit)이 완전히 미고정**이다.**
  전 후보를 프로브한 뒤 첫 성공을 고르는 구현이 **오라클 전량 통과**한다:
  `test:33`/`:43`은 `toHaveBeenCalledWith`(호출 **존재**만 검사, 횟수·순서 무검사)이고,
  유일한 정확 배열 단언 `test:53-56`은 **첫 후보가 실패하는** 픽스처라 단락/전수가 **동일한 호출 배열**을 낸다.
  → Rust에서 `filter().next()`나 `join_all` 포트가 **git 서브프로세스를 2배**로 쓰면서 통과한다.
  **핀: `origin/main` 케이스에서 호출 횟수가 정확히 1.**
- **G11 — ⚠⚠ 후보 **순서**도 사실상 미고정이다.** `baseRef`에 `/`가 있으면
  `["refs/remotes/"+b, "refs/heads/"+b]`로 **remotes가 먼저**다(`:15`, 모듈 존재 이유 `:11-13`).
  그런데 `test:36-44`(`origin/main`)는 역순 구현도 통과시킨다(heads 먼저 → false → remotes → true → 같은 반환값,
  `toHaveBeenCalledWith`도 만족). 순서는 **`test:53-56` 하나에만** 걸려 있다. → **정확 호출 배열을 직접 핀.**
- **G12 — 분기 셋**: ① `baseRef.startsWith("refs/")` → **콜백 0회**, 입력 그대로 반환(`:7-8`);
  ② `/` 포함 → 후보 2개; ③ `/` 없음 → 후보 **1개**(`refs/heads/`만, `:16`).
  아무것도 없으면 **`baseRef` 그대로**(`:24`). `main/worktree-create-base-prefetch.ts:33-36`이 `!==`로 비교한다.
- **G13 — S1 주입 콜백**(`clipboard_text.rs:17-24` 선례를 인자·반환값 있는 형태로 확장):
  ```rust
  pub fn resolve_worktree_add_base_ref(
      base_ref: &str,
      ref_exists: &mut dyn FnMut(&str) -> bool,
  ) -> String
  ```
  `&mut dyn FnMut`이어야 오라클의 **호출 기록 스파이가 `Vec<String>`을 소유**할 수 있다(G10/G11 핀의 전제).
  ⚠ **의도적 발산 1건**: TS는 콜백 rejection을 **전파**한다(`try/catch` 없음). `bool` 형태는 그 계약을 **버린다**.
  생산 호출부 **6곳이 전부** 프로브를 `try/catch → false`로 감싸므로 `Result` 형태는 **실사용자가 0명**이고
  모든 호출자에 타입 파라미터를 전염시킨다. → **`bool`로 가되 헤더에 명시적으로 기록**한다(S1이 하는 방식 그대로).
- **G14 — `for…of` + `await`는 **순차**다**(`Promise.all`도 `.some`도 아니다). 동기 포트에선 평범한 `for` + 조기 `return`.

## 4. 오라클 & 핀
**오라클 전량**: `hosted-review-refs.test.ts` 4케이스, `base-ref-search-result.test.ts` 2케이스,
`worktree-base-ref.test.ts` 5케이스.
⚠ `worktree-base-ref.test.ts:15-26`의 두 케이스(`refs/pull/123/head`, `refs/merge-requests/456/head`)는
`:5-13`과 **바이트 동일 경로**(둘 다 `refs/` 조기 반환)라 **분기 커버리지 0**이다 —
provider 호환 의도 기록용이니 **이식은 하되 커버리지로 세지 말 것**.

**추가 핀(오라클 침묵 — 이 PR의 본체):**
**G1 전역/비앵커 발산 4입력**; G2 `refs/remotes//x`·`refs/remotes/origin` 무변화;
**G3 U+FEFF/U+0085 양방향**; **G4 `origin/refs/heads/x` → `refs/heads/x`**;
**G6 `derive("origin/")`·`derive("upstream/")`**; G7 접두사 리터럴 2개;
**G8 `legacy("origin/main")`의 `local_branch_name == "main"`**;
**G10 `origin/main`에서 호출 횟수 == 1**; **G11 슬래시 포함 입력의 정확 호출 배열**;
G12 세 분기 각각 + 미존재 시 입력 그대로 + `refs/` 케이스의 **콜백 0회**.

*mutation:* G1 `str::replace`로·`Regex::replace_all`로, G2 `[^/]+`를 `[^/]*`로·다음 `/` 미제거,
G3 `str::trim`으로, G4 두 단계 순서 교환, G5 head 로직 중복 구현, G6 가드 제거(`strip_prefix` 직결),
G7 접두사 값 변경·순서 교환, G8 derive 호출 생략, G10 전수 프로브로, G11 후보 순서 반전,
G12 `refs/` 조기 반환 제거·슬래시 없을 때도 후보 2개.

## 5. 순서
단일 PR. 셋을 **함께 리뷰해야 G1의 마스킹이 보인다** — 따로 보면 각각 한 줄짜리로 읽힌다.
크레이트 헤더 모듈 수(현재 twenty-one)·목록·`Cargo.toml` 설명을 같이 고친다(신규 3개는 **v1.4.146-rc.0**).
불변식: `suaegi-misc`(§0), **앵커 1회 치환**(G1), `[^/]+` 1자 이상(G2), `js_trim`(G3),
**2단계 순서**(G4), 위임(G5), **`length >` 가드**(G6), 접두사 리터럴(G7), **derive 배선**(G8),
非옵션 2필드 + serde 보류(G9), **단락**(G10), **후보 순서**(G11), 3분기(G12),
**S1 주입 콜백 + rejection 발산 기록**(G13), 순차 루프(G14), 매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[mutation-survivor-triage]], [[suaegi-misc-placement-rule]],
[[orca-source-location]], [[suaegi-impl-model-sonnet]]
