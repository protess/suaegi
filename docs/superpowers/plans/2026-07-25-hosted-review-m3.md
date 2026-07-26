# Plan — hosted-review M3: GitHub 노멀라이저 (`hosted-review-github.ts`)

조사: `docs/superpowers/research/2026-07-25-hosted-review.md`. M1(#75)·M2(#76) 머지 완료.
리드가 소스 `:1-128` **전문 재확인**. 이 문서가 구현 계약.
대상: `crates/suaegi-forge/src/hosted_review_github.rs` (신규). M1 어휘 소비.

## 0. 범위
`hostedReviewSummaryFromGitHubPRInfo`(`:64-105`), `hostedReviewInfoFromGitHubPRInfo`(`:107-128`),
private `unresolvedThreadCount`(`:17-29`), `deriveChecksStatus`(`:31-62`), args 타입(`:4-15`).
**범위 밖:** gitlab 노멀라이저(M4), 소비자 배선.

## 1. 계약 결정

- **J1 — ⚠️ 조사 권고(“기존 `pr_actions::PrComment` 확장”)를 **채택하지 않는다**. 대신 이 모듈에 **로컬 입력 타입**을 둔다.**
  `HostedReviewCommentInput { thread_id: Option<String>, is_resolved: Option<bool> }`.
  **근거(리드 실측):** 기존 `PrComment`(`pr_actions.rs:299`)는 `{author, body, created_at, url}`로 스레드 필드가 **없고**,
  구조체 리터럴 생성 지점이 **3곳**(`pr_actions.rs:308` From, `gitlab/forge.rs:483`, `github_http/forge.rs:529`)이다.
  두 필드를 공유 타입에 추가하면 gitlab/github_http 생성자가 `is_resolved: None`을 넣게 되는데, **T16에 따르면
  `Some(false)`만 “미해결”**이므로 그 두 provider의 코멘트는 **항상 해결됨으로 집계**된다 → 나중에 배선되면
  **조용히 unresolvedCount=0**(가장 나쁜 실패 모드). 로컬 타입은 blast radius 0이고 함수가 실제로 읽는 2필드와 정확히 일치.
  **문서화:** 공유 `PrComment`에 스레드 정보가 필요해지면 **별도 마이그레이션**으로 다룰 것(주석 + 이 plan 인용).
- **J2 (T17) — `comments`의 부재≠빈 배열.** `comments: Option<&[HostedReviewCommentInput]>`:
  `None` → `unresolved_count = None` → **`thread_summary` 자체를 `None`**(“모름”);
  `Some(&[])` → `Some(0)` → `thread_summary = Some{unresolved_count: Some(0), data_completeness: Partial}`(“로드됐고 없음”).
  오라클 `github.test.ts:105-122`가 이 구분을 명시적으로 잠근다 — **이 클러스터에서 가장 load-bearing한 핀**.
- **J3 (T16) — skip 규칙 축자.** `if !comment.threadId || comment.isResolved !== false { continue }`:
  `thread_id`가 `None` **또는 `Some("")`**(빈 문자열 falsy) → skip; `is_resolved != Some(false)` → skip
  (**부재/`Some(true)` 둘 다 “해결됨” 취급**). 남은 것의 `thread_id`를 `HashSet<&str>`에 넣고 **size** 반환
  (문자열 정확 일치 dedupe, 케이스 폴딩 **없음**, 순서 무관).
- **J4 (T31) — GitHub failure set 4개 축자:** `failure`/`timed_out`/`cancelled`/`action_required`
  (`:38-46`, `action_required` 근거 주석 `:43-45` 함께 이식). 우선순위 **failure > pending > success > neutral**.
  `checks`가 `None`이거나 **빈 배열**이면 `pr.checks_status`를 **그대로 통과**(`:35-37`).
  pending 조건(`:50-52`): `status != Completed || conclusion.is_none() || conclusion == Some(Pending)`.
  마지막 fallthrough는 `Neutral`(`:61`) — **M2에서 `Neutral`은 ready-to-merge를 통과**함에 유의.
  ⚠ **M4의 GitLab판은 failure set이 `{failure, timed_out}`뿐**(T31) — 공유 함수로 묶으려면 **failure set을
  파라미터화**할 것. M3에서는 GitHub 전용으로 두고 M4에서 결정한다(**M3+M4 합본 금지**).
- **J5 (T19) — `host ?? 'github.com'`.** `??`이므로 `Some("")`는 **`""` 그대로**(‘github.com’으로 대체 안 됨).
  `host.unwrap_or("github.com")` — `.filter(|s| !s.is_empty())` **붙이지 말 것**.
- **J6 (T20) — author는 truthiness.** `authorLogin ? {login, isBot} : null` → `Some("")`이면 **author `None`**.
  `author_login.filter(|s| !s.is_empty()).map(|l| HostedReviewUser { login: Some(l), is_bot: author_is_bot })`.
  (`is_bot`은 `None`이어도 user 객체는 생성됨.)
- **J7 (G1) — 조건부 spread `x !== undefined`는 M1의 `Option<T>` 붕괴와 자연스럽게 일치**(null/부재 동일 취급).
  `merge_state_status`, `review_decision`, `auto_merge_enabled`, `auto_merge_allowed`, `merge_queue_required`,
  `requested_reviewer_logins`, `last_viewed_at`은 **그대로 통과**.
- **J8 (T22) — 단, `headSha`/`confirmedContainedHeadOid`/`conflictSummary`는 **truthiness 가드**(`:122-126`).**
  같은 파일 안에서 T21(`!== undefined`)과 **가드가 다르다** → 이 3개만 `.filter(|s| !s.is_empty())`(문자열) /
  `Option` 그대로(conflictSummary는 객체라 `Some`이면 truthy). **핀: `head_sha: Some("")` → 결과에서 `None`.**
- **J9 (T23) — reviewDecision 매핑:** `APPROVED`→`Approved`, `CHANGES_REQUESTED`→`ChangesRequested`,
  `REVIEW_REQUIRED`→`ReviewRequired`, **그 외(부재/null/미지) 전부 `None`**. Rust는 닫힌 enum이라 “미지 토큰”은
  발생 불가하나 **`None`이 M2의 머지 게이트를 통과(관대)**함을 주석으로 연결(H5).
- **J10 (T24) — `data_completeness`는 항상 `Partial`.** `Full`은 이 클러스터에 생성 경로 없음(M1에서 데드 변형으로 정의됨).
- **J11 — `draft`는 파생:** `draft: Some(pr.state == PrState::Draft)`(`:103`). 별도 입력 필드 아님.
- **J12 — 필드 리네임:** `hostedReviewInfoFromGitHubPRInfo`는 `pr.checksStatus` → **`status`**(`:114`). 오라클 `:124-136`이 잠금.
- **J13 (T14) — `baseRefName`은 복사하지 않는다(축자).** `HostedReviewInfo`에 필드가 선언돼 있지만
  `:107-128`이 `pr.baseRefName`을 **안 채운다**. **upstream 갭이므로 “고치지 말 것”** — 주석으로 명시.

## 2. 마일스톤 M3 (단일 PR)
`crates/suaegi-forge/src/hosted_review_github.rs` 신규 + `lib.rs` 선언/re-export. **새 의존성 없음.**
- 입력 타입: `HostedReviewCommentInput`(J1), `PrCheckDetail { status: CheckRunStatus, conclusion: Option<CheckConclusion> }`,
  `CheckRunStatus`(queued/in_progress/completed), `CheckConclusion`(8변형), `GitHubPrInfo`(필요 필드 부분집합),
  `HostedReviewFromGitHubPrInfoArgs`.
- private: `unresolved_thread_count`(J2/J3), `derive_checks_status`(J4).
- public: `hosted_review_summary_from_github_pr_info`, `hosted_review_info_from_github_pr_info`.

**오라클(`hosted-review-github.test.ts` 6케이스 전량):** host 오버라이드+checks 패스스루+threadSummary None(`:20-37`);
Set dedupe(t1 2건→1)+isResolved 제외+checks가 pr값 덮어씀(`:39-81`); `cancelled`→failure(`:83-92`);
`action_required`→failure(`:94-103`); **부재 vs `[]`**(`:105-122`); info 리네임 6필드(`:124-136`).

**추가 핀(오라클 침묵):** J5 host 부재→`github.com` + `Some("")`→`""`; J6 author `Some("")`→None,
`is_bot` 전달; J8 `head_sha: Some("")` 드롭; J9 매핑 3+None; J11 draft 파생(state Draft→true, Open→false);
J3 `thread_id: Some("")` skip·`is_resolved: None` skip(=해결 취급)·`Some(true)` skip; J4 `timed_out`→failure·
빈 checks 패스스루·pending 3조건 각각·success·**neutral fallthrough**; J13 `base_ref_name` 미복사.

*mutation:* J2 `None`↔`Some(0)` 뒤바꿈, J3 skip 조건 반전(`!= Some(false)`→`== Some(true)`), dedupe 제거(Vec len),
J4 failure set에서 `cancelled`/`action_required` 제거·우선순위 스왑·빈-checks 패스스루 제거, J5 `??`→`||`(빈 문자열 대체),
J6 truthiness 제거, J8 빈 문자열 가드 제거, J9 매핑 1개 누락, J11 draft 상수화, J12 리네임 되돌림.

## 3. Deferred
- **M4** gitlab 노멀라이저 + `url` 크레이트(T25/T27/T28) + failure-set 파라미터화 결정(J4).
- 공유 `PrComment`에 스레드 필드 추가(J1) = 필요해지면 별도 마이그레이션.
- 소비자 배선 = 사람눈.

## 4. 순서
단일 PR. 불변식: 로컬 입력 타입(J1), absent≠empty(J2), skip 규칙(J3), GitHub failure set(J4), `??` 시맨틱(J5),
author truthiness(J6), Option 통과(J7), headSha 계열만 빈-문자열 드롭(J8), 매핑(J9), Partial 고정(J10),
draft 파생(J11), status 리네임(J12), baseRefName 미복사(J13), 매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[suaegi-impl-model-sonnet]]
