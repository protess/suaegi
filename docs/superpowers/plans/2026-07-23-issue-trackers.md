# Plan — 이슈트래커 통합 (Jira + Linear)

조사: `docs/superpowers/research/2026-07-23-issue-trackers.md` (Orca @ v1.4.150-rc.0,
모든 인용 file:line 고정). suaegi-secrets(키체인+env-var 폴백)가 선결로 완성됐으므로
토큰 기반 통합이 가능해졌다.

## 0. 결정 (조사 권장 채택)

- **순서**: N1 Linear 읽기+링크 → N2 Jira 읽기 → N3 Linear ForAgent write-back +
  티켓으로 에이전트 런치. 근거: GraphQL 클라이언트가 최대 미지수이자 write-back의
  전제라 먼저, 읽기라 비파괴; Jira REST는 forge 인접이라 다음; write-back은 파괴적이라
  마지막.
- **Linear transport**: Rust `@linear/sdk`가 없다 → **hand-rolled GraphQL-over-HTTP**
  (api.linear.app/graphql로 단일 POST, forge의 `HttpTransport`/`FakeTransport` 재사용).
- **write-back은 Linear 전용**(조사 §0.1): Jira는 UI-facing만, `...ForAgent`/
  `resolveCurrentIssue`가 없다. N3는 Linear만.
- **에이전트 접근은 MCP 아님**(조사 §0.2): `orca`류 CLI 셔임 + 내부 RPC. suaegi도
  forge에서 이미 CLI-in-PATH 축을 가지므로 같은 축 재사용(N3에서).
- **write 규율은 forge와 동일**: transient→Unavailable, **거짓 음성 절대 금지**.
  Linear write는 writeId 멱등 + readback 확인 + 4-way 분류(duplicate_id/failed/
  network/**unconfirmed**).

## 1. N1 상세 — Linear 읽기 + 워크트리 링크 (즉시 구현 대상)

### 1.1 GraphQL-over-HTTP 클라이언트 (`crates/suaegi-tracker/src/linear/`, §Q5 레이어링)
`suaegi-http`(추출 예정, §Q5)의 `HttpTransport`/`FakeTransport` 재사용. Linear는 단일
엔드포인트 POST(`https://api.linear.app/graphql`), 본문 `{query, variables}`,
**`Authorization: <raw API key>` (Bearer 아님)**. 토큰 `suaegi-secrets`(service=
"suaegi-linear", account=workspace).

**[Codex 메타-발견] 분류 규칙의 ground truth는 Orca가 아니라 `@linear/sdk` 소스다.**
Orca는 GraphQL 파싱을 닫힌 `@linear/sdk`에 위임하고, 자기 코드는 `error.message`
substring 매칭만 한다(`linear/issues.ts:857-891` — **이것을 미러하지 말 것**, 열등하다).
실제 계약(`@linear/sdk` `graphql-client.ts` `rawRequest`, `error.ts` `parseLinearError`):

- **성공 = `response.ok && !result.errors && result.data`** (셋 다). 하나라도 아니면 실패.
- **HTTP 200 + `errors` 존재 → 성공 아님.** `errors[0].extensions.type`(문자열 enum:
  `"authentication error"`, `"ratelimited"`, `"forbidden"`, `"network error"`,
  `"internal error"`, `"invalid input"`, `"user error"`, ...)로 분류 → `Unavailable`
  (auth/rate/network/other). **errors 배열 전체가 아니라 `errors[0]`만** 본다.
  extensions.type 없으면 HTTP 상태로 폴백(403→Forbidden, 429→Rate, 4xx→Auth,
  500→Internal, 5xx→Network).
- **엣지 (plan이 놓쳤던 것, Codex):** ① `HTTP 200 + errors 없음 + data:null` → 성공
  아님(Unknown) → `Unavailable`, **절대 None/empty 아님**. ② `errors: []`(빈 배열,
  존재) → JS에선 truthy라 실패로 감 — Rust에선 `errors`가 `Some(비어있음)`이면 성공
  아님으로 처리(주석으로 "키 부재≠성공" 함정 명시).
- **절대 빈 결과(None/empty)로 읽지 않는다** — 캐시-오염 방지 crux. 사용자 문자열은
  `errors[0].extensions.userPresentableMessage`만(raw `.message`는 쿼리 내부 누출 위험).
- HTTP 비-2xx → `Unavailable(classify)`(401/403 auth, 429 rate, 5xx/timeout network).

인용은 Orca가 아니라 `github.com/linear/linear` `packages/sdk/src/{error,graphql-client}.ts`.

### 1.2 오퍼레이션 (읽기)
- `test_connection`: [Codex] `viewer { id }`만으론 워크스페이스 레코드를 못 채운다 →
  **`query { viewer { id displayName email organization { id name urlKey } } }`**.
  org name/urlKey/email이 연결-계정 UI와 딥링크에 필요.
- `list_issues(filter)`: `issues(...)` 쿼리, **bounded full traversal**(조사 §Q3/Codex):
  Orca `readIssueConnectionPages`(`issues.ts:438-462`)처럼 `pageInfo{hasNextPage,endCursor}`로
  limit까지 페이지를 돈다(전 페이지 아님, 첫 페이지-only 아님). **stuck-cursor 가드**
  (`next_cursor == after`면 중단 — 무한루프 방지, Codex가 복사 권장). `hasMore`를
  UI "더 보기"로 노출(무성 절단 금지, 회귀 메모리).
- `search_issues(query)`: [Codex NOTE] Orca는 `searchIssues`를 **단일 호출**로 하고
  커서 페이지를 안 한다(`issues.ts:812-854`). 현재 스키마가 `searchIssues`에 `pageInfo`를
  주는지 확인 전까지 커서 페이지네이션을 여기 적용하지 않는다 — 단일 호출로 시작.
- `get_issue(id)`: `issue(id:...)`. [Codex NOTE] Linear의 "not found"는 빈 결과가
  아니라 GraphQL 에러다 — 그 `extensions.type`(문서화된 값 불명, human-eyes로 실측
  기록) → None으로 매핑할지 Unavailable로 둘지 결정. 그 전까진 Unavailable(안전).
- `get_issue_comments(id)`: 코멘트. transient=Unavailable≠빈 목록.
- 매핑 공통: found/none/unavailable, transient은 never None(§1.1 crux 재사용).

### 1.3 도메인 링크 (`suaegi-core::Worktree`)
Orca는 provider별 슬롯 분리(`types.ts:479-489`) — **세 필드**(Codex): suaegi도
`linked_github_pr`와 나란히:
```rust
pub linked_linear_issue: Option<String>,                    // 이슈 identifier(예: ENG-123)
pub linked_linear_issue_workspace_id: Option<String>,       // 다중 워크스페이스 구분
pub linked_linear_issue_organization_url_key: Option<String>, // linear.app/{urlKey}/... 딥링크·재연결 식별
```
**둘 다 `#[serde(default)]`** — 옛 data.json 로드(Plan 6 follow-up 교훈, forge #14가
같은 함정 겪음). "현재 워크트리 이슈" 해석은 **순수 함수**(`worktree → Option<링크>`):
링크 없으면 None(에이전트에겐 "링크된 이슈 없음" 힌트, N3에서).

### 1.4 테스트/mutation (N1)
- **FakeTransport에 실제 Linear GraphQL JSON 모양** — 성공, 커서 페이지, **200-with-errors**,
  401. `crates/suaegi-forge/src/github_http`의 fake 하네스 미러.
- **mutation 코어**: (a) **200-with-errors를 빈 결과로 뭉개면** 조회가 None/empty로
  읽히는 회귀 → 그걸 잡는 테스트(캐시 오염 방지, forge와 같은 crux). (b) 에러 분류
  (auth/rate/network) 각 축. (c) 커서 페이지네이션(hasNextPage 처리 — 한 페이지만
  읽고 멈추는 off-by-one). (d) 링크 필드 serde(default) 라운드트립 + 옛-shape 로드.
  (e) raw GraphQL error 문자열이 UI 라벨로 안 샘.
- **human-eyes**: 실제 Linear 워크스페이스로 리스트/검색이 뜨는지.

## 2. N2 — Jira 읽기 (REST, github_http 미러)
- Cloud(`/rest/api/3`+ADF) vs Server/DC(`/rest/api/2`+plain) **분기**(`apiBasePath`),
  Basic(email:token)/Bearer(PAT) 인증. 토큰 suaegi-secrets(service="suaegi-jira",
  account=site).
- searchIssues(JQL)/listIssues/getIssue/getIssueComments/listProjects.
- **ADF→Markdown 변환(순수 함수, mutation-verifiable)** — Orca `adf-markdown.ts` 포팅.
  표/미디어/멘션 완전 변환은 보류(문단/리스트/코드/링크 우선).
- mutation 코어: authHeader 분기, apiBasePath 분기, ADF↔Markdown, 401-only auth 분류.

## 3. N3 — Linear ForAgent write-back + 티켓 런치 (파괴적, 마지막)
- writeId 멱등 + readback 확인 + 4-way 분류(조사 §0.3, §2.5). createIssueForAgent/
  updateIssueForAgent(상태 전이)/addIssueCommentForAgent/attachLink.
- CLI-in-PATH(또는 suaegi RPC)로 에이전트에 `orca linear …` 등가 노출 + `resolveCurrentIssue`
  (워크트리→링크 이슈 자동 유도, 1.3의 순수 함수 재사용).
- 이슈 리스트 → "이 티켓으로 워크트리 생성" 런치 플로우.
- mutation 코어: write 4-way 분류, 멱등, readback null 판정.
- **human-eyes**: 에이전트가 실제로 자기 티켓 상태를 옮기고 코멘트를 다는지.

## 4. UI (net-new iced, 각 마일스톤 후속)
연결(API key/토큰) 다이얼로그, 이슈 리스트/검색, 프로젝트 피커, 워크트리↔이슈 링크
표시. N1은 최소(연결 + 이슈 리스트 + 링크). 시크릿 입력은 화면에 마스킹.

## 5. 보류 / follow-up
Jira ForAgent write-back(Orca에 없음), Linear projects/customViews/relations 깊이,
activity 피드, ADF 표/미디어/멘션 완전 변환, 다중 워크스페이스 UI 선택.

## 6. 미해결 — Codex 교차검증 반영 (판정: IMPLEMENTABLE-AFTER-FIXES → 반영 완료)
- **#Q1 해소**(§1.1): `@linear/sdk` `graphql-client.ts`/`error.ts`의 실제 계약 —
  성공=`ok && !errors && data`, 분류=`errors[0].extensions.type` enum, 엣지(data:null,
  빈 errors 배열) 명시. Orca가 아니라 SDK 인용. Orca의 message-substring 매칭은 미러 금지.
- **#Q2 해소**(§1.1): raw API key(Bearer 아님), SDK `client.ts` 확인.
- **#Q3 해소**(§1.2): bounded full traversal(limit까지)+stuck-cursor 가드. searchIssues는
  단일 호출로 시작(커서 지원 확인 전).
- **#Q4** 워크트리 Jira 링크 슬롯 — N2 착수 시 `linked_jira_issue` 자체 설계(Orca에 얇음).
- **#Q5 결정**(Codex 권장): **`HttpTransport`/`FakeTransport`/`ReqwestTransport`/
  `HttpRequest`/`HttpResponse`/`TransportError`를 새 `suaegi-http` 크레이트로 추출**하고,
  `suaegi-forge`와 새 `suaegi-tracker`가 둘 다 의존한다. 이슈트래커가 "PR forge"에
  의존하는 레이어링 냄새를 피한다. forge의 기존 import를 `suaegi-http`로 재지정
  (약간의 churn, 영구적 스멜 회피). `suaegi-secrets`가 세운 leaf-크레이트 선례와 같은 모양.
  → **N1은 이 추출을 첫 태스크로 포함**한다.

## 7. 마일스톤 순서 (확정)
N0(추출) `suaegi-http` 크레이트 → N1 Linear 읽기+링크 → N2 Jira 읽기 → N3 Linear
ForAgent write-back+런치. N0는 N1의 첫 태스크로 흡수한다.
