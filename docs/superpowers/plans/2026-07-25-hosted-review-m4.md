# Plan — hosted-review M4: GitLab 노멀라이저 (클러스터 완료)

조사: `docs/superpowers/research/2026-07-25-hosted-review.md`. M1(#75)·M2(#76)·M3(#77) 머지 완료.
리드가 소스 `:1-125` **전문 재확인** + **`url` 크레이트 시맨틱을 워크스페이스에서 실측**(아래 K1).
대상: `crates/suaegi-forge/src/hosted_review_gitlab.rs` (신규). **`url` 의존 추가**(forge에 신규, 워크스페이스 기존).

## 0. 범위
`parseGitLabIdentity`(`:19-41`), `unresolvedThreadCount`(`:43-55`), `deriveChecksStatus`(`:57-82`),
`hostedReviewSummaryFromGitLabInfo`(`:84-125`), args 타입(`:4-11`). 이걸로 **클러스터 4/4 완료**.

## 1. 계약 결정

- **K1 (T25) — WHATWG `URL.host`는 `host_str()` + `port()` 재조립. 리드가 워크스페이스에서 실측 검증 완료.**
  JS `URL.host`는 **포트를 포함**하지만 Rust `Url::host_str()`는 **제외**한다. 실측 결과(`url` 2.5.8):
  - `https://gitlab.com:8443/...` → `host_str()="gitlab.com"`, `port()=Some(8443)` → 재조립 **`"gitlab.com:8443"`** ✓
  - `https://gitlab.com:443/...` → `port()=**None**`(스킴 기본 포트) → `"gitlab.com"` ✓ (WHATWG도 기본 포트 생략)
  - `http://gitlab.com:80/...` → 동일하게 `"gitlab.com"` ✓
  → `match (host_str(), port()) { (Some(h), Some(p)) => format!("{h}:{p}"), (Some(h), None) => h.into(), _ => String::new() }`.
  **`port_or_known_default()` 금지**(기본 포트를 붙여버려 WHATWG와 어긋난다).
- **K2 (T26) — path는 퍼센트 디코딩하지 않는다(실측: `Url::path()`가 `%2F`를 리터럴 보존 ✓).**
  `path().split('/')`에서 **빈 세그먼트 제거**(`filter(Boolean)` = `!s.is_empty()`).
- **K3 (T27) — `-` 마커는 첫 번째 위치. `marker_index == 0`이면 project 세그먼트가 **빈 배열**이 되어 `< 2` 분기로
  떨어지고 owner/repo 둘 다 `'unknown'`이 되지만 **host는 파싱된 값이 유지된다**.** (K4의 catch 폴백과 다르다!)
  `segments.iter().position(|s| *s == "-")` → `Some(i)` → `&segments[..i]`, `None` → 전체.
- **K4 (T28) — `Url::parse` 실패 → `{host:"gitlab.com", owner:"unknown", repo:"unknown"}`.**
  실측: 상대 URL(`/relative/path`)과 비-URL(`not a url`)은 `Err` → JS `new URL` throw와 일치 ✓.
  **주의: 여기서는 host도 `gitlab.com`으로 리셋**(K3과 대조 — K3은 host 유지).
- **K5 — 두 분기 축자.** `>= 2`: `owner = project[..len-1].join("/")`(**다중 세그먼트를 `/`로 조인**),
  `repo = project.last()`(`at(-1) ?? 'unknown'`은 이 분기에서 도달 불가한 방어 — `unwrap_or("unknown")`로 이식).
  `< 2`: `owner = project.get(0).unwrap_or("unknown")`, `repo = project.get(1).unwrap_or("unknown")`
  (**길이<2이므로 repo는 항상 `"unknown"`**). 두 분기 모두 `host`는 `parsed_host` 비었으면 `"gitlab.com"`(`||`, T25).
- **K6 (T31) — GitLab failure set은 `{failure, timed_out}` **단 2개**. M3의 4개짜리 함수를 재사용하지 말 것.**
  이 모듈에 **자체 `derive_checks_status`를 둔다**(중복 허용). 나머지(빈-checks 패스스루·pending 3조건·success·
  neutral fallthrough)는 M3과 동일하지만 **failure set만 다르다** — 조사가 "합치면 통일해버리는 실수를 유발한다"고
  경고한 지점이다. 두 파일에 **상호 참조 주석**(M3은 4개, M4는 2개, 통합하려면 failure set 파라미터화 필수).
  **핀: `cancelled`와 `action_required`가 GitLab에서는 failure가 **아님**(→ pending/neutral 경로).**
- **K7 (T32) — `requested_reviewer_logins`를 설정하지 않는다(`None`).** 소스 `:89-124`에 해당 필드가 **없다**.
  결과적으로 GitLab MR은 M2의 `hasRequestedReviewerSignal`에서 항상 false → **`Requested`로 분류될 수 없다**.
  축자 유지 + 주석으로 "upstream 갭 가능성" 명시. **핀: requested reviewer가 있어도 `Requested`가 안 나옴.**
- **K8 — `unresolved_thread_count`는 M3과 문자 그대로 동일 → M3의 것을 `pub(crate)`로 열어 재사용.**
  M3 파일에 대한 수정은 **가시성 한 단어**(`fn` → `pub(crate) fn`)로 제한한다. provider별 분기가 전혀 없는
  순수 함수라 K6의 함정과 무관. `HostedReviewCommentInput`도 M3에서 재사용(이미 `pub`).
- **K9 — summary 필드 매핑:** identity는 **URL에서 파생**(github은 인자로 받음), `checks_status`는
  `derive_checks_status(args.review.status, args.checks)` — 입력 필드명이 **`status`**(github은 `checks_status`, M3 J12 참조).
  author truthiness(`:100`, `Some("")`→None), reviewDecision 매핑 3+None(`:107-114`), threadSummary(`:115-121`),
  `last_viewed_at`(`:122`), `draft = state == Draft`(`:123`)는 M3과 동일 규칙.

## 2. 마일스톤 M4 (단일 PR, 클러스터 완료)
`crates/suaegi-forge/src/hosted_review_gitlab.rs` 신규 + `lib.rs` + `Cargo.toml`에 `url = { workspace = true }`
+ **M3의 `unresolved_thread_count`를 `pub(crate)`로**(K8, 한 단어).
- private: `parse_gitlab_identity`(K1–K5), `derive_checks_status`(K6, GitLab 전용).
- public: `hosted_review_summary_from_gitlab_info`, `HostedReviewFromGitLabInfoArgs`, `GitLabReviewInfo`(입력 부분집합).

**오라클(`hosted-review-gitlab.test.ts` 2케이스):** 중첩 그룹 URL → `{gitlab, gitlab.acme.internal, "group/subgroup",
"orca", 12}` + checksStatus 패스스루 + threadSummary None(`:17-29`); comments/checks → `{unresolvedCount:1, partial}` +
`failure`(`:31-71`).

**추가 핀(오라클 침묵 — 이 모듈은 커버리지가 가장 얇다):** K1 포트 포함 host(`:8443`) + 기본 포트 생략(`:443`);
K2 `%2F` 리터럴 보존 + 선행/중복 슬래시; K3 `marker_index==0`(→unknown/unknown, **host 유지**); K4 파싱 실패
(상대 URL·비-URL → **host도 gitlab.com**); K5 세그먼트 1개/0개·다중 그룹 owner 조인; K6 **`cancelled`/`action_required`가
failure가 아님**(GitHub과 대조) + `timed_out`은 failure + 빈 checks 패스스루 + neutral fallthrough;
K7 requested reviewer 무시; K9 author `Some("")`→None·draft 파생·`status` 필드명.

*mutation:* K1 `port_or_known_default()`로 교체·포트 누락, K2 퍼센트 디코딩 추가·빈 세그먼트 유지, K3 `position`→
`rposition`·marker 0 특수처리 추가, K4 폴백 host를 파싱값으로, K5 owner 조인 제거·분기 경계 `>=2`→`>2`,
K6 failure set에 `cancelled` 추가(= M3과 통일하는 실수), K7 requested 필드 채우기, K8 재사용 대신 다른 로직.

## 3. Deferred (클러스터 완료 후 남는 것)
- 소비자 배선(큐 뷰·정렬 Q5·`HostedReviewCreationEligibility` 확장) = 사람눈.
- 공유 `PrComment`에 스레드 필드 추가(M3 J1) = 필요해지면 별도 마이그레이션.
- `derive_checks_status` 통합(failure set 파라미터화) = 지금은 **의도적으로 중복 유지**.

## 4. 순서
단일 PR. 불변식: host_str+port 재조립(K1), 퍼센트 보존(K2), marker 0은 host 유지(K3), 파싱 실패는 host도 리셋(K4),
두 분기 축자(K5), **GitLab failure set 2개 유지**(K6), requested 미설정(K7), M3 함수 재사용(K8), 필드 매핑(K9),
매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[suaegi-impl-model-sonnet]]
