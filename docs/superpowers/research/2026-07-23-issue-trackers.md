# 이슈 트래커 통합 조사: Jira + Linear

> 2026-07-23. Orca **실제 소스를 직접 읽어** 확인한 것만 적는다. 모든 주장에 `file:line`이 붙어 있다.
> Rust 코드는 쓰지 않았다 — 이 문서는 조사(RESEARCH) 단계 산출물이다.
>
> 대상 버전: Orca **v1.4.150-rc.0** (`b25c298`).
> Orca 클론 루트: `/private/tmp/claude-501/.../scratchpad/orca-src/`.
> 이하 모든 경로는 이 루트 기준. suaegi 경로는 `crates/...`로 표기해 구분한다.

---

## 0. 요약 — 이 조사가 확정한 것

- **Jira transport**: `net.fetch`(Electron) 위의 **REST 클라이언트**. Cloud=`/rest/api/3`(+ADF), Server/DC=`/rest/api/2`(+plain text). 인증은 요청마다 `Authorization` 헤더에 Basic(email:token) 또는 Bearer(PAT). (`src/main/jira/client.ts:72,338,369`) → suaegi에선 `github_http`의 `HttpTransport` 패턴을 그대로 미러링한 REST 클라이언트.
- **Linear transport**: `@linear/sdk`(GraphQL over HTTP)의 `LinearClient`. 읽기 리스트/검색/write-back은 전부 **raw GraphQL 쿼리 문자열**(`client.client.rawRequest`)로 직접 친다. 인증은 API key. (`src/main/linear/issues.ts:179-320,488`, `src/main/linear/linear-sdk.ts:7-14`) → Rust엔 `@linear/sdk`가 없으니 **손으로 짠 GraphQL-over-HTTP**가 유일한 길.
- **권장 마일스톤 순서**: **N1 = Linear 읽기+링크 → N2 = Jira 읽기 → N3 = Linear ForAgent write-back(에이전트가 자기 티켓에 쓰는 경로)**. 근거는 §4.
- **suaegi가 채워야 할 top 3 gap**: (1) GraphQL-over-HTTP 클라이언트(요청 빌더 + 에러 분류), (2) ADF↔Markdown 변환기(Jira Cloud), (3) worktree↔issue 링크 도메인 필드 + "현재 워크트리의 이슈" 해석 경로.

### 이 조사가 바꾼 설계 결정 3가지 (핵심)

1. **Orca의 "에이전트가 티켓에 write-back" 딥 통합은 Linear 전용이다.** Jira RPC 메서드는 전부 UI-facing(connect/list/search/create/update/comment/transitions/metadata)이고, `...ForAgent` 변형이나 `resolveCurrentIssue`가 **없다**(`src/main/runtime/rpc/methods/jira.ts:100-217`). Linear만 `linear.issueCreate`/`issueSetState`/`issueAddComment`/`resolveCurrentIssue` 등 에이전트 write 경로를 가진다(`src/main/runtime/rpc/methods/linear-agent-access.ts:158-248`). → suaegi도 write-back은 Linear부터.
2. **에이전트는 MCP가 아니라 `orca` CLI 셔임 + 내부 RPC로 티켓에 접근한다.** modelcontextprotocol 서버는 없다(grep 결과 0). Orca는 managed-PTY의 PATH 앞에 `orca` 셔임을 심고(`src/main/cli/linux-terminal-orca-cli-shim.ts:27-62`), 에이전트가 `orca linear …`를 실행하면 RPC(`defineMethod`)로 라우팅된다. → suaegi는 이미 이 CLI-in-PATH 축을 가질 것(forge). 같은 축을 재사용한다.
3. **Linear write는 낙관적 write가 아니라 writeId 멱등 + readback 확인이다.** 모든 `...ForAgent`는 duplicate_id/failed/network/**unconfirmed** 4-way 분류를 하고(`src/main/linear/issues.ts:96,567-597`), write 후 반드시 다시 읽어 확인한다(`confirmLinearWrite`, `issues.ts:652-658,1199-1208`). → suaegi의 forge 분류 규율(transient→Unavailable, 거짓 음성 금지)과 정확히 같은 철학. mutation-verifiable 코어가 여기 있다.

---

## 1. 서비스별 표

### 1.1 Jira

**Transport / Auth**

| 항목 | 사실 | 인용 |
|---|---|---|
| HTTP | `net.fetch`(Electron 네트워크 스택, Chromium proxy/session 따름). undici 아님. | `jira/client.ts:352-388` |
| 베이스 경로 | Cloud=`/rest/api/3`, Server/DC=`/rest/api/2`. `apiBasePath(site)`가 분기. | `jira/client.ts:72-74` |
| 인증 헤더 | Server+username空 → `Bearer <PAT>`; 그 외(Cloud/Server+username) → `Basic base64(email:token)`. 요청마다 헤더 생성. | `jira/client.ts:330-339` |
| UA 함정 | Atlassian XSRF 필터가 브라우저 UA의 POST/PUT을 403. 그래서 `User-Agent: 'Orca'` 강제. | `jira/client.ts:25-30,401,445` |
| 토큰 저장 | `~/.orca/jira-tokens/<base64url(siteId)>.enc`, `safeStorage.encryptString`, mode 0600. 복호 불가 시 `CredentialDecryptionError`. | `jira/client.ts:104-106,225-232,244-258` |
| siteId | `sha256(siteUrl\n email.toLowerCase())[:24]`. PAT(email 없음)는 viewer.accountId로 대체 키. | `jira/client.ts:288-293,548` |
| 멀티 사이트 | `jira-sites.json`에 `sites[]`, `activeSiteId`, `selectedSiteId('all'|id)`. `getClients(selection)`이 'all'이면 팬아웃. | `jira/client.ts:57-62,460-485` |
| 에러 분류 | `isAuthError` = 401만. 403은 권한 갭이지 크리덴셜 무효 아님(주석). 429/네트워크는 별도 취급 없이 로그 후 skip. | `jira/client.ts:632-636` |
| 동시성 | 프로세스 전역 세마포어 `MAX_CONCURRENT=4` (`acquire`/`release`). | `jira/client.ts:32-55` |

**연산 (UI/앱-facing; ForAgent 없음)** — 전부 `jira/issues.ts`, RPC는 `rpc/methods/jira.ts:100-217`

| 연산 | 구현 | 인용 |
|---|---|---|
| connect / disconnect / selectSite / status / testConnection | `/myself`로 검증, 사이트 파일 갱신 | `jira/client.ts:504-624` |
| searchIssues(JQL) | Cloud=`POST /search/jql`, Server=`POST /search`. body에 `jql,maxResults,fields`. | `jira/issues.ts:359-432` |
| listIssues(filter) | filter→JQL 프리셋(assigned/reported/done/기본) | `jira/issues.ts:346-386` |
| getIssue(key) | `GET /issue/{key}?fields=…` | `jira/issues.ts:434-463` |
| createIssue | `POST /issue`, body는 Cloud면 ADF, Server면 plain. customFields 지원 | `jira/issues.ts:465-509` |
| updateIssue | `PUT /issue/{key}`(summary/labels/priority) + assignee(`PUT .../assignee`, Cloud=accountId/Server=name) + transition(`POST .../transitions`) | `jira/issues.ts:511-567` |
| addIssueComment | `POST /issue/{key}/comment` (ADF/plain) | `jira/issues.ts:569-598` |
| getIssueComments | 페이지네이션 `GET .../comment` | `jira/issues.ts:610-639` |
| listProjects | Cloud=`/project/search`(paged), Server=`/project`(플랫 배열) | `jira/issues.ts:641-679` |
| listIssueTypes | `/issue/createmeta/{proj}/issuetypes` (paged) | `jira/issues.ts:681-712` |
| listCreateFields | `/issue/createmeta/{proj}/issuetypes/{type}` (paged) | `jira/issues.ts:714-761` |
| listPriorities | `/priority` | `jira/issues.ts:763-782` |
| listAssignableUsers | `/user/assignable/search` (Cloud=`query`/Server=`username`) | `jira/issues.ts:784-816` |
| listTransitions | `/issue/{key}/transitions` | `jira/issues.ts:818-847` |
| getProjectStatusOrder | `/rest/agile/1.0/board`+`/configuration` (보드 컬럼 순서) | `jira/issues.ts:849-921` |

**suaegi-secrets 매핑**: service = 예 `"suaegi-jira"`, account = **siteId**(sha256 prefix). 토큰 타입은 문자열 하나(PAT 또는 API token). email/authType 등 non-secret 메타는 `suaegi-core` JSON에 평문 저장(Orca가 `jira-sites.json`을 평문으로 두는 것과 동형 — 토큰만 keychain). `SecretRequest::new("suaegi-jira", &site_id)` + env fallback.

### 1.2 Linear

**Transport / Auth**

| 항목 | 사실 | 인용 |
|---|---|---|
| HTTP | `@linear/sdk`의 `LinearClient`. 지연 로딩(`createRequire`, ~2.6MB CJS). Rust엔 대응물 없음. | `linear/linear-sdk.ts:7-32` |
| 쿼리 방식 | 리스트/검색/write-back은 **raw GraphQL 문자열** `entry.client.client.rawRequest<Resp,Vars>(QUERY, vars)`. SDK 헬퍼(`createIssue`/`updateIssue`)도 일부 씀. | `issues.ts:488-495,826-829,1118` |
| 인증 | API key. `new LinearClient({ apiKey })`. 헤더 옵션(`public-file-urls-expire-in`)도 지원. | `linear/client.ts:511-566` |
| 토큰 저장 | 워크스페이스별 `~/.orca/linear-tokens/<base64url(id)>.enc`(+레거시 단일 `linear-token.enc`), `safeStorage`, 0600. | `linear/client.ts:98-103,323-343` |
| 워크스페이스 id | Linear org.id. 메타는 `linear-workspaces.json` 평문(status 렌더에 keychain 프롬프트 회피). | `linear/client.ts:48-59,442-454` |
| 멀티 워크스페이스 | `activeWorkspaceId`/`selectedWorkspaceId('all'|id)`, `getClients('all')` 팬아웃. 레거시 워크스페이스 마이그레이션 경로 존재. | `linear/client.ts:271-313,522-557` |
| 에러 분류 | `isAuthError` = `AuthenticationLinearError`. write는 4-way(`duplicate_id/failed/network/unconfirmed`), 읽기 팬아웃은 `auth/rate_limited/network/unknown`. | `issues.ts:567-597,874-897` |
| 동시성 | 전역 `MAX_CONCURRENT=4`. | `linear/client.ts:22-46` |

**연산** — 읽기/UI write는 `linear/issues.ts`, 에이전트 write는 같은 파일의 `...ForAgent` 변형, RPC는 `rpc/methods/linear-agent-access.ts:158-248`

| 그룹 | 연산 | 인용 |
|---|---|---|
| connect | connect/disconnect/selectWorkspace/status/testConnection (`viewer`+`organization`) | `linear/client.ts:577-696` |
| 읽기 | searchIssues(GraphQL `searchIssues(term)`, 관련도 순 유지) | `issues.ts:812-854` |
| 읽기 | listIssues(assigned/created/all/completed/open; 커서 페이지네이션, 워크스페이스별 50 상한; 'all'은 글로벌 정렬 병합) | `issues.ts:1059-1089,438-464,1011-1057` |
| 읽기 | getIssue(SDK `client.issue(id)`, children/project 포함) | `issues.ts:710-741` |
| 읽기 | getIssueComments(단일 GraphQL로 author까지 — N+1 회피) | `issues.ts:304-320,1498-1537` |
| UI write | createIssue / updateIssue / addIssueComment (SDK 헬퍼) | `issues.ts:1091-1156,1227-1292,1347-1389` |
| **ForAgent** | getIssue/Comment/Attachment**ByUuidForAgent** (write-id readback 조회) | `issues.ts:743-798` |
| **ForAgent** | createIssueForAgent / updateIssueForAgent / addIssueCommentForAgent / createIssueAttachment | `issues.ts:1158-1210,1294-1345,1391-1472` |
| relations | issue-relation-write.ts(관계 생성) | `linear/issue-relation-write.ts` (참조) |
| teams/projects | teams.ts(listTeams/labels/states/members + `...ForAgent`), projects.ts(create/get/list/customViews) | `linear/teams.ts:74`, `linear/projects.ts` |
| context | issue-context*.ts (한 이슈 + comments/children/attachments/relations/activity 팬아웃 번들) | `linear/issue-context.ts`, `issue-context-includes.ts` |

**suaegi-secrets 매핑**: service = `"suaegi-linear"`, account = **workspace id**(org.id). 토큰 타입은 API key 문자열. non-secret 워크스페이스 메타(name/urlKey/displayName)는 `suaegi-core` JSON. `SecretRequest::new("suaegi-linear", &workspace_id)`.

**공유 크리덴셜 규율(양쪽 공통)** — `integration-credential-file.ts`: 빈 파일=미저장, 복호 실패=`CredentialDecryptionError`(레거시 평문 폴백은 UTF-8+제어문자 없음일 때만). suaegi-secrets는 이미 keychain>env 우선순위 + `Resolved`로 이 셋(found/none/error)을 표현하니 재현 불필요. (`integration-credential-file.ts:10-80`)

---

## 2. 태스크 소스 모델 — 티켓이 에이전트를 굴리는 방식 (통합의 존재 이유)

**핵심 흐름 (Linear 기준, 구체):**

1. **워크트리에 이슈를 링크한다.** 도메인 타입 `Worktree`가 이슈 참조 슬롯을 가진다:
   `linkedLinearIssue: string | null`, `linkedLinearIssueWorkspaceId?`, `linkedLinearIssueOrganizationUrlKey?` (`src/shared/types.ts:479-481`). GitHub은 `linkedIssue:number`/`linkedPR`, GitLab/Bitbucket 등도 각자 슬롯 — provider 판별자 대신 필드를 분리했다(주석 `types.ts:482-489`). 링크는 워크트리 생성/업데이트 시 세팅(`orca-runtime.ts:16639,17216,17595`, RPC 스키마 `worktree-schemas.ts:76,189`).

2. **에이전트가 그 워크트리 안에서 뜬다.** managed-PTY로 뜬 에이전트(Claude/Codex)는 PATH 앞의 `orca` 셔임을 본다(`linux-terminal-orca-cli-shim.ts:20-62`). 에이전트는 스킬/dispatch 프리앰블 지시에 따라 `orca linear …`를 실행한다(MCP 서버 아님 — grep으로 modelcontextprotocol 0건 확인).

3. **"현재 이슈"가 워크트리에서 자동 해석된다.** `linear.resolveCurrentIssue` RPC(`linear-agent-access.ts:210-214`) → `getLinearCurrentIssueFromWorktree(worktree)`가 `linkedLinearIssue`(+workspaceId)를 읽어 identifier/workspace를 돌려준다. 링크 없으면 `linear_no_linked_issue` 에러 + nextSteps 힌트(`issue-context-current.ts:17-39`). 즉 **에이전트는 이슈 id를 몰라도 "내 워크트리의 티켓"에 작업할 수 있다.**

4. **에이전트가 티켓에 write-back 한다.** RPC 메서드가 `...ForAgent` 구현으로 라우팅:
   - `linear.issueSetState` / `issueUpdateTask` → `updateIssueForAgent` (상태 전이 = 워크플로우 이동)
   - `linear.issueAddComment` → `addIssueCommentForAgent` (진행상황 코멘트)
   - `linear.issueCreate` → `createIssueForAgent` (후속 이슈 생성)
   - `linear.issueAttachLink` → `createIssueAttachment` (PR/산출물 링크 첨부)
   - `linear.issueRelationWrite` → 관계 생성
   전부 `orca-runtime.ts:543-562`에서 import되어 런타임 메서드로 노출.

5. **write는 멱등 + 확인.** RPC가 `parseLinearWriteId(params.writeId)`로 클라이언트 제공 write-id를 파싱(`linear-agent-access.ts:163,234,240,246`). `createIssueForAgent`는 그 id로 생성 후 즉시 `getCreatedIssueRecord`로 readback, 실패하면 `unconfirmed`(성공했는지 모름 — 재시도 시 duplicate 잡힘). 재시도가 같은 write-id로 오면 `duplicate_id`로 멱등 처리(`issues.ts:545-553,1181-1210`). **네트워크 타임아웃을 "실패"로 단정하지 않는다** — suaegi forge의 "거짓 음성 금지"와 동일.

**Jira는 (2)까지만.** 워크트리에 Jira 이슈를 링크하는 슬롯이 도메인에 있고(코드베이스에 `linkedIssue`는 GitHub용; Jira 전용 링크 슬롯은 이 버전엔 얇음 — 확인 필요), UI에서 사람이 create/update/comment/transition을 하지만, **에이전트 자동 write-back RPC는 없다**(`rpc/methods/jira.ts`엔 ForAgent/resolveCurrent 부재). 이게 v1 스코프를 가르는 결정적 사실.

**suaegi가 필요로 하는 것 (구체):**
- 도메인: `suaegi-core::Worktree`에 `linked_linear_issue: Option<String>` + `linked_linear_issue_workspace_id: Option<String>` (그리고 Jira용 `linked_jira_issue: Option<String>` + site_id). 기존 `linked_github_pr: Option<u64>`(`crates/suaegi-core/src/domain.rs:61`)와 나란한 슬롯. **JSON 라운드트립 + "키 없음 = 링크 없음" 회귀 테스트**는 `linked_github_pr`가 이미 가진 패턴 그대로(`domain.rs:302-325`).
- 해석 경로: "현재 워크트리 → 링크된 이슈" 순수 함수(파싱 + 워크스페이스 폴백). mutation-verifiable.
- 런치 플로우: 이슈 리스트에서 "이 티켓으로 워크트리 생성" → 워크트리 생성 시 링크 필드 세팅 → 터미널 위젯이 에이전트를 띄움(이미 plan4/plan6 축). write-back은 CLI-in-PATH + suaegi RPC로 노출(forge와 같은 축).

---

## 3. suaegi가 없는 것 (gap)

1. **GraphQL-over-HTTP 클라이언트 (Linear).** Rust엔 `@linear/sdk`가 없다. 손으로: 쿼리 문자열 + `variables` JSON을 `POST https://api.linear.app/graphql`에 싣고, `data`/`errors`를 파싱. `github_http`의 `HttpTransport` 트레잇/`FakeTransport`/`HttpRequest`/`HttpResponse`(`crates/suaegi-forge/src/github_http/transport.rs:16-90`)를 그대로 재사용 가능 — GraphQL은 단일 POST 엔드포인트라 REST보다 오히려 단순. **주의**: GraphQL은 HTTP 200에도 `errors`를 담아 온다 → 분류가 status code만으로 안 됨. `AuthenticationLinearError`/rate-limit/network를 **응답 body의 error 형태**로도 판정해야 한다(`issues.ts:874-897`가 message 문자열 매칭으로 하는 그 일).
2. **ADF ↔ Markdown 변환 (Jira Cloud).** Cloud v3는 description/comment가 ADF(Atlassian Document Format) JSON. Orca는 `adfToMarkdownText`(읽기)와 `textToAdf`(쓰기, 단순 문단만)를 가진다(`jira/adf-markdown.ts:24-192`). Server/DC v2는 plain text라 변환 불필요. suaegi는 이 두 함수의 Rust 포팅 필요 — **순수 함수, mutation-verifiable 코어**. 범위: doc/paragraph/heading/bulletList/orderedList/codeBlock/blockquote/rule/hardBreak(읽기), 문단 분해(쓰기).
3. **worktree↔issue 링크 도메인 필드** (§2). 현재 `suaegi-core`엔 `linked_github_pr`만.
4. **멀티 사이트/워크스페이스 크리덴셜 키.** suaegi-secrets는 service+account 2-튜플이니 account = site_id/workspace_id로 자연 매핑되지만, "all" 팬아웃 시 **한 사이트 복호 실패가 나머지를 죽이지 않기**(`jira/client.ts:476-483`, `linear/client.ts:536-546`)를 앱 레이어에서 재현해야 한다. suaegi-secrets의 `Resolved`가 per-request라 이미 잘 맞는다.
5. **이슈 리스트/워크스페이스 UI (net-new iced).** §5 하단 참조.
6. **전역 동시성 리미터.** Orca는 서비스별 `MAX_CONCURRENT=4` 세마포어. suaegi는 async 런타임에 따라 `tokio::sync::Semaphore` 등. (있으면 좋음, v1 필수 아님 — 읽기 팬아웃이 커질 때.)

---

## 4. 마일스톤 분해 (권장 스테이징)

각 마일스톤은 **mutation-verifiable 코어**(파싱·분류·변환 — 순수 함수, `FakeTransport`로 테스트)와 **human-eyes**(실제 API 왕복 — 눈으로 확인)를 분리한다. 이 저장소는 공허한 회귀 테스트가 5번 나왔으니 mutation 검증이 필수다(메모리 참조).

**N1 — Linear 읽기 + 링크 (권장 시작점).**
- GraphQL-over-HTTP 클라이언트(§3.1) + `searchIssues`/`listIssues`/`getIssue`/`getIssueComments`.
- connect/status/testConnection(API key → viewer/organization), suaegi-secrets 저장.
- 워크트리 링크 도메인 필드 + "현재 이슈" 해석 순수 함수.
- **mutation 코어**: GraphQL 응답 파싱, 에러 분류(auth/rate/network/unknown), 커서 페이지네이션 로직, 링크 필드 JSON 라운드트립.
- **human-eyes**: 실제 Linear 워크스페이스로 리스트/검색이 뜨는지.
- 왜 먼저? GraphQL 클라이언트가 가장 큰 미지수 + write-back(N3)의 전제. 읽기만이라 파괴적이지 않다.

**N2 — Jira 읽기 (+ 선택적 UI write).**
- REST 클라이언트(`github_http` 미러) + Cloud/Server 분기(`apiBasePath`), Basic/Bearer 인증.
- searchIssues(JQL)/listIssues/getIssue/getIssueComments/listProjects.
- ADF→Markdown(읽기) 포팅.
- (선택) createIssue/updateIssue/addIssueComment + textToAdf(쓰기).
- **mutation 코어**: `authHeader` 분기, `apiBasePath` 분기, ADF↔Markdown, JQL 프리셋, 401-only auth 분류.
- **human-eyes**: Cloud와 Server/DC 양쪽(가능하면). UA 함정(`client.ts:25-30`) 실측.
- 왜 두 번째? REST는 GraphQL보다 익숙하고 forge 패턴 직결. Cloud/Server 분기가 표면적을 늘리니 Linear 뒤.

**N3 — Linear ForAgent write-back / 티켓으로 에이전트 런치.**
- write-id 멱등 + readback 확인(`createIssueForAgent`/`updateIssueForAgent`/`addIssueCommentForAgent`/attachment).
- 4-way write 분류(duplicate_id/failed/network/unconfirmed).
- CLI-in-PATH(또는 suaegi RPC)로 에이전트에 노출 + "현재 워크트리 이슈" 자동 해석.
- 이슈 리스트 → "이 티켓으로 워크트리 생성" 런치 플로우.
- **mutation 코어**: write 분류(§0.3), 멱등 처리, readback null 판정, 워크트리→이슈 해석.
- **human-eyes**: 에이전트가 실제로 자기 티켓 상태를 옮기고 코멘트를 다는지.
- 왜 마지막? 파괴적(실 티켓 변경) + N1/N2의 클라이언트에 의존.

**보류**: Jira ForAgent write-back(Orca에 없음), Linear projects/customViews/relations 깊이, activity 피드, attachments beyond link, ADF의 표/미디어/멘션 완전 변환.

---

## 5. 열린 질문 / 리스크

1. **Linear GraphQL 스키마 드리프트.** raw 쿼리 문자열은 스키마 변경에 깨진다(codegen 없음). Orca도 손으로 쿼리를 관리하니(위험을 감수) suaegi도 동일 — 쿼리를 한 곳(상수)에 모으고 통합 테스트를 실 API에 거는 게 유일한 방어. codegen(예: `graphql-client` 크레이트 + introspection)은 스키마 SDL이 필요하고 CI 복잡도를 올린다 → **v1은 손으로 짠 쿼리 권장**, 쿼리가 10개를 넘어가면 재검토.
2. **ADF 완전성.** `adf-markdown.ts`는 표/패널/미디어/멘션을 fallback text로 뭉갠다(`adf-markdown.ts:66-73,184`). 읽기 손실 허용 여부 = 제품 결정. 쓰기(`textToAdf`)는 문단만 — 리치 마크다운을 ADF로 못 올린다.
3. **워크트리 Jira 링크 슬롯.** 이 Orca 버전 도메인엔 Linear 링크 필드는 명시적이지만(`types.ts:479-481`) Jira 전용 링크 슬롯은 얇다(`linkedIssue`는 GitHub 숫자용). suaegi가 Jira 티켓을 워크트리에 링크하려면 **새 필드를 우리가 설계**해야 한다(`linked_jira_issue: Option<String>` + site_id) — Orca를 그대로 못 베낀다. 확인 필요 항목.
4. **에이전트 노출 축.** Orca는 `orca` CLI 셔임(Linux는 스크린리더 충돌까지 회피 `shim:20-26`). suaegi가 이미 forge용 CLI-in-PATH를 갖는지, 아니면 별도 RPC/MCP를 낼지 = plan6/plan7 축과의 정합성 결정. MCP로 가면 Orca와 갈라진다(장단 있음).
5. **XSRF/UA·프록시.** Jira는 브라우저 UA 403 함정(`client.ts:25-30`)과 프록시 재설정(`client.ts:357-365`)을 Electron `net.fetch`에 의존해 푼다. suaegi의 `reqwest` 기반 transport는 UA를 우리가 직접 세팅하면 되지만(쉬움), VPN 전환 후 stale keep-alive는 우리가 겪을 수 있다 — 커넥션 풀 정책 주시.
6. **rate limit.** Linear는 429를 body/message로 신호(`issues.ts:882`), Jira도 429 가능하나 Orca Jira는 명시적 429 처리가 없다(로그 후 skip). suaegi는 forge의 `is_rate_limited`(`crates/suaegi-forge/src/github_http/classify.rs:17`) 패턴을 양쪽에 적용 권장 — transient→Unavailable, 거짓 음성 금지.

---

### 부록: suaegi 재사용 자산 매핑

| 필요 | suaegi 기존 자산 | 인용 |
|---|---|---|
| 주입식 HTTP transport + Fake | `github_http::{HttpTransport, FakeTransport, HttpRequest, HttpResponse, TransportError}` | `crates/suaegi-forge/src/github_http/transport.rs:16-90` |
| transient→Unavailable 분류 규율 | `github_http::classify_*`, `provider::ForgeUnavailable` | `crates/suaegi-forge/src/github_http/classify.rs:17-67`, `provider.rs:49` |
| 토큰 저장 keychain>env + redaction | `suaegi_secrets::{SecretRequest, resolve, load, store, Secret::expose, Resolved, Source}` | `crates/suaegi-secrets/src/lib.rs:28-187`, `secret.rs:10-20` |
| 도메인 링크 필드 + JSON 라운드트립 테스트 패턴 | `suaegi_core::Worktree::linked_github_pr` | `crates/suaegi-core/src/domain.rs:61,302-325` |
