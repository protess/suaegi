# Plan — hosted-review M2: 큐 분류기 (`hosted-review-queue.ts`)

조사: `docs/superpowers/research/2026-07-25-hosted-review.md`. M1(어휘)은 PR #75로 머지됨.
리드가 소스 `:14-134` **전문 재확인 완료**. 이 문서가 구현 계약.
대상: `crates/suaegi-forge/src/hosted_review_queue.rs` (신규). M1의 `hosted_review::*` 어휘 소비.

## 0. 범위
`hosted-review-queue.ts`의 **`hostedReviewIdentityKey`를 제외한 전부**(M1에서 이미 이식):
private `hasRequestedReviewerSignal`(`:18-31`), `isAgentAuthored`(`:33-48`), `getQueueState`(`:50-66`);
public `reviewNeedsResponse`(`:68-90`), `reviewReadyToMerge`(`:92-121`), `classifyHostedReview`(`:123-134`);
type `HostedReviewClassificationOptions`(`:9-12`).
**범위 밖:** github/gitlab 노멀라이저(M3/M4), `pr_actions.rs` 수정, 정렬(호출자 소유).

## 1. 계약 결정

- **H1 (Q1) — `Date.parse`는 strict RFC3339로 이식. `chrono` 의존 추가.**
  `queue.ts:88` `Date.parse(summary.updatedAt)` → `chrono::DateTime::parse_from_rfc3339(s).ok().map(|dt| dt.timestamp_millis())`
  → `Option<i64>`(ms). 파싱 실패 → `None` → `false`(JS `NaN` → `Number.isFinite` false → 같은 결과).
  `suaegi-forge/Cargo.toml`에 `chrono = { workspace = true }` 추가(워크스페이스 기존, `suaegi-automation` 선례 —
  `occurrence.rs:77` `timestamp_millis()`).
  **문서화할 좁은 발산:** ECMAScript는 오프셋 없는 date-time을 **local**, date-only를 **UTC**로도 받지만 RFC3339는
  거부한다. GitHub/GitLab API는 `Z` 접미 RFC3339만 내보내므로 **실입력에서 관측 불가**. 주석으로 남길 것.
- **H2 — 타임스탬프 비교는 반드시 `i64` 공간에서. `parsed as u64` 금지(cardinal sin).**
  `queue.ts:89` `updatedAt > summary.lastViewedAt`. M1의 필드는 `last_viewed_at: Option<u64>`(epoch ms)이고
  `Date.parse`는 **부호 있는** 값(1970 이전이면 음수)이다.
  → `parsed_ms > last_viewed_at as i64` 형태로 **i64에서 비교**. `parsed_ms as u64 > last_viewed_at`로 쓰면
  음수가 거대한 u64로 wrap해 **거짓 true**가 된다. **strict `>`**(동일 타임스탬프는 false, T13).
  **핀:** `updated_at`이 1969년(음수 epoch) + `last_viewed_at: Some(0)` → **false**(wrap 없음 증명).
- **H3 (Q3) — T9/T10 비대칭은 의도된 안전 자세다. 축자 보존 + 주석.**
  `:76` `(threadSummary?.unresolvedCount ?? 0) > 0` — 스레드 정보 **부재/null → 0** → "응답 불필요"(nag 안 함).
  `:117` `threadSummary?.unresolvedCount !== 0` — 부재/null → **머지 차단**.
  즉 **"모르면 시끄럽게 굴지 않되, 되돌리기 어려운 머지는 막는다."** 두 함수가 같은 필드를 반대로 읽는 게 정상.
  Rust(M1 `Option<u32>`): `:76`은 `.unwrap_or(0) > 0`, `:117`은 `!= Some(0)`.
  **핀 양방향:** `thread_summary: None` → `needs_response` **false** AND `ready_to_merge` **false**(테스트 `:96` 잠금).
- **H4 — `void viewer`(`:72`) 인자를 시그니처에 유지.** `review_needs_response(summary, _viewer: Option<&HostedReviewUser>)`.
  Orca가 의도적으로 남긴 미사용 인자(`classifyHostedReview:131`이 넘긴다) — 드롭하면 호출부 시그니처가 갈린다.
  `#[allow(unused_variables)]` 또는 `_` 접두. 주석으로 "미사용은 의도(upstream 시그니처 보존)" 명시.
- **H5 (Q6) — 미지/부재 `reviewDecision`은 머지 게이트를 **통과**한다(관대). 축자 보존.**
  `:108-113`은 `'review_required'`/`'changes_requested'`만 차단 → `'approved'`/null/부재 **전부 통과**.
  이는 크레이트의 지배적 규율("미지는 보수적으로", `pr_actions.rs:267,281`)과 **반대**다 →
  **주석으로 divergence를 명시**하되 동작은 바꾸지 말 것. 같은 맥락: `checksStatus`가 `'neutral'`이면 **통과**(`:114`).
- **H6 (Q12) — `includes('bot')` 과잉매칭 유지.** `:47` `author.ends_with("[bot]") || author.contains("bot")`.
  후자가 전자를 완전히 포섭하는 **죽은 조건**이지만 축자 이식(주석). `"abbot"`/`"robot"`/`"botvinnik"`이 agent로
  분류되는 건 **의도된 과잉매칭**. **핀:** `"robot"` → agent.
- **H7 (T3) — 모든 소문자화는 full Unicode `str::to_lowercase()`.** `:29,30,40,44,54,55` 6곳.
  `to_ascii_lowercase`/`eq_ignore_ascii_case` **금지**([[js-lowercase-two-mechanisms]] — 이 모듈은 `.toLowerCase()`이지 regex `/i`가 아니다).
- **H8 — `getQueueState` 우선순위가 계약:** mine(`:56`) → requested(`:59`) → agent(`:62`) → teammate(`:65`).
  작성자==뷰어면 **동시에 요청된 리뷰어여도 `mine`**. 그리고 `classifyHostedReview`의 `requested: true`는
  `state: Mine`과 **공존 가능**(`:130`이 독립 계산). **핀:** viewer==author + requested 포함 → `state=Mine, requested=true`.
- **H9 — 빈 문자열 login은 "부재"와 동일.** `:22` `!viewer?.login`(T1), `:41` `!author`(T5),
  `:54-56` `?? null` 후 truthiness 게이트(T7). → `Option<String>`에서 `None`과 `Some("")` **둘 다** 탈락.
- **H10 — `reviewReadyToMerge` 8단 게이트 순서 그대로**(`:93,96,99,102-107,108-113,114,117,120`).
  게이트4는 **`provider == Github`일 때만**(T15 — gitlab BLOCKED는 통과, 테스트 `:108-123`이 잠금).
  `mergeStateStatus`는 **대문자 정확 일치**(`BEHIND`/`BLOCKED`), M1에서 `Option<String>` 원문 보관.
  `state != Open`이면 차단이라 **`Draft`도 여기서 탈락**(반면 `reviewNeedsResponse`는 `Draft`를 **허용** `:73`).

## 2. 마일스톤 M2 (단일 PR)
`crates/suaegi-forge/src/hosted_review_queue.rs` 신규 + `lib.rs` 선언/re-export + `Cargo.toml`에 `chrono`.
- `HostedReviewClassificationOptions { viewer: Option<HostedReviewUser>, agent_author_logins: Option<Vec<String>> }`
- private: `has_requested_reviewer_signal`, `is_agent_authored`, `get_queue_state`
- public: `review_needs_response`(H1/H2/H3/H4), `review_ready_to_merge`(H3/H5/H10), `classify_hosted_review`

**오라클(`queue.test.ts` 23케이스 전량, `:26-42` identity-key는 M1에서 이식 완료이므로 제외):**
큐 상태 4(mine/requested/agent/teammate) + needs-response 5(`:71,72,73,74-81,85`) + ready-to-merge 13(`:91-101,105,109-122`).
픽스처 `baseSummary()`(`:10-23`)는 **`lastViewedAt`/`draft`/`reviewDecision`/`mergeStateStatus`/`requestedReviewerLogins`
전부 부재**이고 `ready_to_merge == true`임에 주의.

**추가 핀(오라클 침묵):** H2 음수 epoch 무-wrap; H3 `thread_summary: None` 양방향; H6 `"robot"`→agent;
H7 non-ASCII 로그인 폴딩(`"Ä"` vs `"ä"`); H8 mine+requested 공존; H9 `Some("")` 로그인 3곳;
H5 `reviewDecision: None`이 머지 통과 + `checksStatus: Neutral` 통과; T4 `is_bot: Some(true)`가 로그인보다 우선;
T11 `last_viewed_at: Some(0)`은 조기탈출 아님(비교로 진행); H1 파싱 실패(`""`, `"not-a-date"`) → false.

*mutation:* `?? 0`→`!= Some(0)`(T9/T10 뒤바꿈), `>`→`>=`(T13), `as u64` 캐스팅, `to_lowercase`→`to_ascii_lowercase`,
우선순위 순서 스왑, github 게이트 조건 제거(T15), `contains("bot")` 제거, `Draft` 허용/차단 뒤바꿈, 게이트 하나 제거.

## 3. Deferred
- **M3** github 노멀라이저 + `pr_actions.rs::PrComment` 확장(thread_id/is_resolved).
- **M4** gitlab 노멀라이저 + `url` 크레이트(T25/T27/T28).
- 정렬·큐 렌더링 순서(Q5, 호출자 소유) = 사람눈.

## 4. 순서
단일 PR. 불변식: strict RFC3339 + 실패는 false(H1), **i64 비교**(H2), T9/T10 비대칭 보존(H3), 미사용 viewer 유지(H4),
관대한 미지 처리(H5), `contains("bot")` 유지(H6), full-Unicode 폴딩(H7), 우선순위(H8), `""`=부재(H9), 8단 게이트(H10),
매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[suaegi-impl-model-sonnet]], [[js-lowercase-two-mechanisms]]
