# Plan — hosted-review M1: 어휘 + identity key

조사: `docs/superpowers/research/2026-07-25-hosted-review.md`. 이 문서가 구현 계약.
클러스터 4단 분할 중 **M1**(신규 파일만, 새 의존성 0, 기존 타입 **수정 금지** → blast radius 0).
대상: `crates/suaegi-forge/src/hosted_review.rs` (신규).

## 0. 범위
- `hosted-review.ts`(215L) 전체 어휘: enum 10종 + struct 12종 + `isPositiveHostedReviewNumber`(`:14-16`).
- `hostedReviewIdentityKey`(`hosted-review-queue.ts:14-16`) + 그 오라클(`queue.test.ts:26-42`).
- **범위 밖(M2–M4):** queue 분류기 4함수, github/gitlab 노멀라이저, `pr_actions.rs` 수정, `url` 크레이트.

## 1. 계약 결정

- **G1 (Q4) — 3-way `?: T | null`은 `Option<T>`로 붕괴한다. 리드가 read-path 전수 확인 완료: 이 클러스터의
  모든 소비 지점에서 `null`과 `undefined`가 동일하게 동작한다.**
  근거(각 필드의 유일한 읽기 지점): `mergeStateStatus` → `=== 'BEHIND'`/`'BLOCKED'`(`queue.ts:104`, 둘 다 불일치);
  `reviewDecision` → `=== 'review_required'`/`'changes_requested'`(`:108-113`, 둘 다 불일치);
  `requestedReviewerLogins` → `!requested || length===0`(`:26`, 둘 다 접힘);
  `unresolvedCount` → `?? 0`(`:76`)과 `!== 0`(`:117`) — 둘 다 nullish 연산이라 absent/null 동일.
  → **`Option<Option<T>>` 금지**(과설계). **단, 모듈 doc에 "이 붕괴는 이 클러스터 read-path 기준으로만 무손실이며,
  JSON 라운드트립이나 null-구분 소비자가 생기면 재검토"를 명시**할 것.
  ⚠ `HostedReviewCreationEligibility.review: HostedReviewSummary | null`(`:131`)은 **필수-nullable**(부재 개념 없음) →
  `Option<HostedReviewSummary>`이되 `#[serde(...)]` 없이 항상 존재하는 필드로 둔다(주석).
- **G2 (Q13) — `is_positive_hosted_review_number(value: Option<f64>) -> bool`.** `None`=비-숫자(JS `typeof !== 'number'`)
  → false. `Some(v)` → `v.is_finite() && v.fract() == 0.0 && v > 0.0`. (`Number.isInteger`는 NaN/±∞ 거부 = `is_finite`+`fract`.)
  **핀:** `None`→false, `Some(0.0)`→false, `Some(-1.0)`→false, `Some(1.5)`→false, `Some(f64::NAN)`→false,
  `Some(f64::INFINITY)`→false, `Some(1.0)`→true, `Some(42.0)`→true.
  **문서화:** JS는 `1e21`도 true지만 그건 `u64::MAX`(≈1.8e19) 초과 — 실제 리뷰 번호 도메인 밖(주석만, 동작 변경 금지).
- **G3 — `hosted_review_identity_key` 축자.** `format!("{provider}::{host}::{owner}::{repo}::{number}")`에서
  **host/owner/repo만 `str::to_lowercase()`(full Unicode, [[js-lowercase-two-mechanisms]]), provider는 소문자화 안 함**
  (이미 리터럴), **trim 없음**(이 클러스터 `.trim()` 0건), 구분자 **`::`**(2문자), **host는 항상 포함 —
  `github.com`을 절대 생략하지 않는다**.
  **⚠ `suaegi_forge::github_identity::github_repo_identity_key` 재사용 절대 금지** — 포맷이 다르고 github.com을
  생략해 GHES 키 충돌을 만든다(오라클 `queue.test.ts:26-42`가 dotcom≠GHE를 잠금). 두 키가 공존한다는 사실을
  양쪽 모듈 doc에 상호 주석.
- **G4 — 번호는 `u64`.** `HostedReviewIdentity.number`/`HostedReviewInfo.number`/`HostedReviewSummary.number`.
  키 보간 `{number}`는 u64 10진수 = JS `${42}`와 일치.
- **G5 — enum 10종 + types.ts 4종 전량 축자**(조사 §2 목록). 문자열 표현은 소스 리터럴 그대로
  (`'azure-devops'` 하이픈, `'needs-response'`/`'ready-to-merge'` 하이픈, `PRMergeableState`/`PRReviewDecision`는 **대문자**).
  `dataCompleteness::Full`은 **생성 경로 없는 데드 변형**이지만 타입에 존재하므로 **정의하고 주석**(T24).
  파생: fieldless enum은 `Clone, Copy, Debug, PartialEq, Eq`; struct는 `Clone, Debug, PartialEq, Eq`.
  **serde 파생은 M1에서 넣지 않는다**(와이어 소비자 없음). 나중에 추가할 때 G1 붕괴 결정을 반드시 재검토하라고 주석.
- **G6 — 기존 타입 재사용 금지(조사 §3의 9곳).** M1은 `hosted_review.rs` 안에 **자체 vocabulary를 새로 정의**한다.
  특히 `MergeabilityState`(4-state, blocked를 mergeability 축에 접음), `ChecksSummary`(카운트라 `neutral` 표현 불가),
  `PrReviewState`(개별 리뷰 6-state), `AnyForge`를 **끌어오지 말 것**. `pr_actions.rs`/`provider.rs`/`github_identity.rs`
  **수정 금지**(M3에서 `PrComment` 확장 예정).
- **G7 — null 문법 차이 보존.** `HostedReviewUser.login: string | null`은 **필수-nullable** → `Option<String>`(항상 존재하는
  필드), `isBot?: boolean`은 **옵셔널-non-null** → `Option<bool>`. Rust 타입은 둘 다 `Option`이지만 **의미가 다르다는
  주석**을 남긴다(T4/T5가 여기 의존).

## 2. 마일스톤 M1 (단일 PR)
`crates/suaegi-forge/src/hosted_review.rs` 신규 + `lib.rs`에 `pub mod hosted_review;`(알파벳) + re-export.
- enum: `HostedReviewProvider`(6), `HostedReviewState`(4), `CreateHostedReviewErrorCode`(8),
  `HostedReviewCreationBlockedReason`(11), `HostedReviewCreationNextAction`(6), `HostedReviewLookupOutcome`(3),
  `HostedReviewDecision`(3), `HostedReviewThreadDataCompleteness`(2), `HostedReviewQueueKey`(6),
  `HostedReviewQueueState`(4), `PrState`(4), `CheckStatus`(4), `PrMergeableState`(3), `PrReviewDecisionAggregate`(3).
  (blocked-reason/next-action의 `null`은 `Option<…>`로 표현 = G1.)
- struct: `HostedReviewInfo`, `HostedReviewForBranchArgs`, `HostedReviewSummary`, `CreateHostedReviewInput`,
  `CreateHostedReviewArgs`, `CreateHostedReviewResult`, `HostedReviewCreationEligibility`, `HostedReviewIdentity`,
  `HostedReviewUser`, `HostedReviewThreadSummary`, `HostedReviewQueueSummary`, `HostedReviewQueueClassification`.
- fn: `is_positive_hosted_review_number`(G2), `hosted_review_identity_key`(G3).

**오라클:** `queue.test.ts:26-42` — github.com#7 키 ≠ github.acme.internal#7 키(**GHES 충돌 방지**).

**추가 핀(오라클 침묵):** G3 full-Unicode 소문자화(`İ`/`GHE.ÉXAMPLE` 호스트), provider는 소문자화 **안 함**,
**trim 안 함**(선행/후행 공백이 키에 남음), `::` 구분자 정확, github.com도 키에 **포함**됨(생략 아님),
동일 입력 → 동일 키(결정성); G2 8케이스(위).

*mutation:* host 소문자화 제거, `to_lowercase`→`to_ascii_lowercase`(É), github.com 생략 추가, 구분자 `::`→`:`,
number를 키에서 누락, provider를 소문자화, trim 추가, G2의 `fract()==0` 제거·`> 0`→`>= 0`·`is_finite` 제거.

## 3. Deferred
- **M2** queue 분류기(비대칭 null T9/T10·`Date.parse`·`includes('bot')`) — Q1/Q3/Q6/Q12 결정 필요.
  **⚠ M2 주의 — `last_viewed_at: Option<u64>` 캐스팅 함정:** M1은 epoch-ms를 `u64`로 뒀다. M2의
  `reviewNeedsResponse`(`queue.ts:88-89`)는 이걸 **`Date.parse` 결과와 비교**하는데 그 값은 **부호 있는**
  값이다(1970 이전 날짜면 음수). 반드시 **`parsed_ms > last_viewed_at as i64`** 형태로 i64 공간에서 비교할 것 —
  `parsed_ms as u64 > last_viewed_at`로 쓰면 음수가 거대한 u64로 wrap해 **거짓 true**가 된다(cardinal sin:
  transient≠false-negative의 사촌). M2에서 이 캐스팅을 mutation으로 반드시 검증하거나, 그때 필드를 `i64`로 바꿀 것.
- **M3** github 노멀라이저 + `PrComment` 확장(thread_id/is_resolved, T16) — 기존 타입 수정.
- **M4** gitlab 노멀라이저 + `url` 크레이트(T25 포트 포함 host, T27/T28) — 새 의존성.
- 소비자 배선(큐 뷰·정렬 Q5) = 사람눈.

## 4. 순서
단일 PR. 불변식: 3-way→`Option<T>` 붕괴 + 재검토 주석(G1), 술어 시그니처(G2), identity key 축자 + 기존 키 재사용
금지(G3), `u64`(G4), enum 축자 + 데드 변형 보존(G5), 기존 타입 재사용/수정 금지(G6), null 문법 주석(G7),
매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[suaegi-impl-model-sonnet]], [[js-lowercase-two-mechanisms]], [[suaegi-workflow]]
