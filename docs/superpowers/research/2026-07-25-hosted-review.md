# 조사 — hosted-review 클러스터 (Orca @ v1.4.150-rc.0)

Explore 정찰(4소스 + 3오라클 + `types.ts` 관련부 정독), 리드 검수·저장. 인용은 `shared/<module>.ts:line`.
대상 크레이트 **`suaegi-forge`**. 전 4모듈 **pure**(상태·I/O·`Date.now()` 없음).

## 0. 전역 negative facts (먼저 확정)
| trap class | 존재 |
|---|---|
| `.sort()`/비교자/정렬 안정성 | **0건** — 순서는 호출자(범위 밖) 소유 |
| `.localeCompare` / `parseInt` / `Number()` / `.trim()` / `.toUpperCase()` / `new Date()` / `.find(` | **0건** |
| 정규식 `/i` | **0건** — 폴딩은 전부 `String.prototype.toLowerCase()` = **full Unicode**([[js-lowercase-two-mechanisms]] → `to_ascii_lowercase` 금지) |
| `Date.parse` | **1건** `queue.ts:88` (유일한 날짜 연산) |
| `\|\|` falsy 폴백 | 2건 `gitlab.ts:27,33` |
| `??` nullish | 6건 queue `:54,55,76`, github `:71`, gitlab `:29,34,35` |

## 1. 트랩 표 (구현 계약)
| # | 위치 | 트랩 |
|---|---|---|
| T1 | `queue.ts:24` | `!viewer?.login` — 부재/null/`""` **전부** "뷰어 없음" |
| T2 | `queue.ts:26` | `!requested \|\| length===0` — 부재/null/`[]` 하나로 접힘 |
| T3 | `queue.ts:15,29,30,40,44,54,55` | `.toLowerCase()` **full Unicode** (`İ`→`i`+U+0307) |
| T4 | `queue.ts:37` | `author?.isBot` — `Some(true)`만 true |
| T5 | `queue.ts:40-43` | `login?.toLowerCase()` falsy(`""` 포함) → false |
| T6 | `queue.ts:47` | `endsWith('[bot]') \|\| includes('bot')` — 후자가 전자를 **포섭**(죽은 조건), `"abbot"`/`"robot"` **과잉매칭 = 의도, 축자 보존** |
| T7 | `queue.ts:54-56` | `??`는 `""`를 통과시키고 그 다음 `if` truthiness가 거른다 |
| T8 | `queue.ts:72` | `void viewer` — 2번째 인자 **미사용**(시그니처 보존용) |
| **T9** | `queue.ts:76` | `(threadSummary?.unresolvedCount ?? 0) > 0` — 부재/null → **0**(응답 불필요) |
| **T10** | `queue.ts:117` | `threadSummary?.unresolvedCount !== 0` — 부재/null → **차단**. **T9와 정반대 = 비대칭**(테스트 `:96`이 잠금) |
| T11 | `queue.ts:85` | `lastViewedAt === undefined` **strict** — `Some(0)`은 통과해 비교로 감 |
| T12 | `queue.ts:88` | `Date.parse` — ECMAScript 포맷(오프셋 없으면 local, date-only는 UTC). `chrono` RFC3339는 둘 다 거부 |
| T13 | `queue.ts:89` | `Number.isFinite && >` **strict** — 동일 타임스탬프는 false |
| T14 | `queue.ts:104` | `mergeStateStatus === 'BEHIND'\|'BLOCKED'` **대문자 정확 일치**, 타입은 자유 `string\|null` |
| T15 | `queue.ts:103` | BEHIND/BLOCKED 게이트는 **github 전용**(테스트 `:108-123`이 gitlab BLOCKED→true 잠금) |
| T16 | `github.ts:23`/`gitlab.ts:49` | `!threadId \|\| isResolved !== false` — `threadId:""`는 skip, **명시적 `false`만** 미해결 |
| **T17** | `github.ts:18-20`/`gitlab.ts:44-46` | `comments === undefined` **strict** → `null` → threadSummary 전체 생략. `[]` → `{0}`. **absent≠empty**(테스트 `github:105-122`) |
| T18 | `github.ts:52`/`gitlab.ts:72` | `conclusion === null` — `PRCheckDetail.conclusion`은 optional이 아니라 `\|null` |
| T19 | `github.ts:71` | `host ?? 'github.com'` — `""`는 `""`로 유지(`\|\|`가 아님) |
| T20 | `github.ts:79`/`gitlab.ts:100` | `authorLogin ? {…} : null` **truthiness** — `""`→author null |
| T21 | `github.ts:82-84,117-121`/`gitlab.ts:103-105` | 조건부 spread `x!==undefined` — **null은 키를 남긴다**(3-way 보존) |
| T22 | `github.ts:122-126` | `pr.headSha ? …` — **여기만 truthiness**(`""` 드롭). T21과 같은 파일에서 가드가 다름 |
| T23 | `github.ts:86-93`/`gitlab.ts:107-114` | reviewDecision 삼항 — 미지 토큰/null → `undefined` → ready-to-merge **통과(관대)**. 크레이트 규율(보수)과 반대 |
| T24 | `github.ts:99`/`gitlab.ts:120` | `dataCompleteness:'partial'` 고정 — `'full'`은 **생성 경로 없는 데드 변형** |
| T25 | `gitlab.ts:27,33` | `parsed.host \|\| 'gitlab.com'` — WHATWG `URL.host`는 **포트 포함**(`hostname`은 미포함) |
| T26 | `gitlab.ts:22` | `.split('/').filter(Boolean)`, pathname은 **퍼센트 디코딩 안 됨** |
| T27 | `gitlab.ts:23-24` | `indexOf('-')`가 0이면 projectSegments=[] → owner/repo 둘 다 `'unknown'`(host는 유지) |
| T28 | `gitlab.ts:20,37` | `new URL` throw → catch → `{gitlab.com, unknown, unknown}` |
| T29 | `hosted-review.ts:15` | `Number.isInteger && > 0` — `1e21` 통과하나 `u64::MAX` 초과 |
| T31 | `github.ts:38-46` vs `gitlab.ts:64-66` | **failure set이 다르다**: github `{failure,timed_out,cancelled,action_required}`, gitlab `{failure,timed_out}` — 공유 함수로 만들면 **파라미터화 필수** |
| T32 | `gitlab.ts:89-124` | `requestedReviewerLogins`를 **설정 안 함** → GitLab MR은 `'requested'`로 분류 불가(github `:101`엔 있음) |

## 2. enum 변형 전량 (축자)
`hosted-review.ts`: `HostedReviewProvider`(`:3-9`) github/gitlab/bitbucket/azure-devops/gitea/unsupported **6**;
`HostedReviewState`(`:11`) open/closed/merged/draft **4**; `CreateHostedReviewErrorCode`(`:77-85`) **8**;
`HostedReviewCreationBlockedReason`(`:96-111`) **11+null**; `HostedReviewCreationNextAction`(`:113-120`) **6+null**;
`HostedReviewLookupOutcome`(`:128`) found/not_found/unavailable **3**; `HostedReviewDecision`(`:175`)
approved/changes_requested/review_required **3+null**(필드는 optional → **4-way**); `dataCompleteness`(`:179`) full/partial **2**;
`HostedReviewQueueKey`(`:199-206`) mine/requested/agent/teammate/needs-response/ready-to-merge **6**;
`HostedReviewQueueState`(`:207`) mine/requested/agent/teammate **4**.
`types.ts`: `PRState`(`:1144`) **4**; `CheckStatus`(`:1146`) pending/success/failure/neutral **4**;
`PRMergeableState`(`:1148`) MERGEABLE/CONFLICTING/UNKNOWN **3 대문자**; `PRReviewDecision`(`:1149`) **3 대문자**;
`PRCheckDetail.status`(`:1329`) queued/in_progress/completed **3**; `.conclusion`(`:1330-1342`) **8+null non-optional**.

**미지 값 fallthrough 5곳:** ①reviewDecision 미지→`undefined`→머지 게이트 통과(관대) ②모든 check가
failure/pending/success 아니면(`skipped`/`neutral`)→`'neutral'`→**통과** ③`mergeStateStatus` BEHIND/BLOCKED 아닌
모든 값→통과 ④큐 분류 미매치→`'teammate'` ⑤gitlab 세그먼트 부족/URL 실패→`'unknown'`.

## 3. ⚠️ 기존 `suaegi-forge` 타입 재사용 시 **조용한 의미 변화 9곳**
큐 분류·스레드 요약·뷰어/봇 개념은 forge에 **하나도 없다**(신규). 그러나 인접 타입 재사용은 위험:
1. **`MergeabilityState`(4, `pr_actions.rs:166`) ← `PRMergeableState`(3)** — Rust는 blocked를 mergeability 축에 접었고
   Orca는 `mergeStateStatus` **별도 축**(+github 전용). 재사용하면 "MERGEABLE인데 BEHIND"와 "CONFLICTING" 구분 소멸.
2. **`ChecksSummary`(카운트, `provider.rs:83`) ← `CheckStatus`(라벨)** — `'neutral'` 표현 불가 → 통과해야 할 PR 차단.
3. **`PrReviewState`(6 개별, `pr_actions.rs:261`) ← `PRReviewDecision`(3 집계)** — Commented/Pending이 머지 게이트 통과.
4. **`PrComment.is_resolved: bool` ← `isResolved?: boolean`** — 부재(=해결)와 `false`(=미해결) 붕괴(T16).
5. `MergeabilityFields.merge_state_status: String`(serde default `""`) ← 3-way.
6. **`github_repo_identity_key`(`github_identity.rs:42`) 재사용 절대 금지** — 포맷이 다르고 **github.com을 생략**해
   GHES 키 충돌(테스트 `queue.test.ts:26-42`가 dotcom≠GHE를 잠금). Orca는 host **항상 포함**, trim 없음.
7. **`parse_gitlab_remote`(`gitlab/parse.rs:266`)** — 입력 도메인이 **git remote URL**(웹 MR URL 아님) + host 게이팅 추가.
8. `AnyForge`(`any.rs:31`) — `Github`/`GithubHttp` 둘 다 provider `'github'`(백엔드 디스패치 ≠ 데이터 라벨).
9. `Option<T>` ← `T|null|absent` 3-way(§Q4).

## 4. 권장 마일스톤 분할 (커트 기준 = 새 의존성 / 기존 타입 수정)
- **M1** 어휘 + identity key — 신규 파일만, 의존성 0, blast radius 0. 3-way 규약을 여기서 못 박으면 M2–M4가 기계적.
- **M2** 큐 분류기(`queue.ts` 4함수 + 오라클 23) — 의미론적 위험 집중(비대칭 null·날짜·폴딩). 여전히 신규 파일만.
- **M3** GitHub 노멀라이저 — **기존 `pr_actions.rs::PrComment` 확장**(thread_id/is_resolved) = 유일한 기존 타입 수정, 소비처 4곳.
- **M4** GitLab 노멀라이저 — **유일하게 새 외부 의존성(`url`)** 요구, 오라클 가장 얇음(2 it).
- **M3+M4 합본 금지** — failure set 차이(T31)와 `requestedReviewerLogins` 누락(T32)을 "통일해버리는" 실수를 유발.

## 5. 교차검증 열린 질문(요약)
Q1 `Date.parse` 계약 범위(RFC3339 가정 가능?) / Q2 `lastViewedAt` ms epoch·`Some(0)` 유효 / Q3 T9↔T10 비대칭이
의도인가 + `void viewer` 처리 / Q4 3-way 표현((a)`Option<Option<T>>` (b)`Option<T>` 붕괴 (c)전용 enum) /
Q5 정렬 소유권 / Q6 미지 변형 관대(축자) vs 크레이트 규율(보수) / Q7 failure set 차이 의도성 / Q8 gitlab
`requestedReviewerLogins` 누락 / Q9 `url` 크레이트 추가 vs 수제(+`URL.host` 포트 포함) / Q10 mergeability 타입 이중화
승인 / Q11 identity key 이중화 승인 / Q12 `includes('bot')` 과잉매칭 유지 / Q13 `isPositiveHostedReviewNumber` 입출력 타입 /
Q14 `baseRefName` 갭(축자 = 복사 안 함).
