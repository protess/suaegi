# Plan 7 — 소스컨트롤 · PR UI 통합 (GitHub 우선)

조사: `docs/superpowers/research/2026-07-22-plan7-source-control.md` (Orca @ v1.4.150-rc.0,
모든 인용 file:line 고정). 이 계획은 그 위에서 무엇을 만들고 무엇을 미루는지 정한다.

## 0. 결정 (James)

**GitHub과 통신을 `ForgeProvider` 트레잇 뒤에 두고, gh shell-out과 HTTP 내장을 둘 다
구현한다 — 단 단계화한다.** gh는 시크릿 저장이 필요 없어 먼저 가치(7a-1), HTTP 내장은
OS 키체인 subsystem을 먼저 지어야 하므로 다음(7a-2). 선택은 상황별(gh 설치·인증돼
있으면 gh, 아니면 저장된 토큰으로 HTTP).

**왜 트레잇을 N=1에서도 쓰나** (Codex가 "때이른 추상화"로 걸 수 있으니 명시): Orca의
`ForgeProvider`(`forge-provider.ts:60-72`)가 이 서브시스템에서 가장 이식 가치가 큰
아이디어다. 트레잇 하나 뒤에 gh/HTTP 두 transport와 이후 7c(GitLab)·embedded forge가
전부 들어간다. 지금 안 두면 7a-2에서 gh impl을 뜯어고쳐야 한다. **의식적으로** N=1
트레잇을 받아들인다(조사 §4.8).

## 1. 트레잇 설계 (7a-1에서 도입, 두 impl용으로)

Rust `ForgeProvider`. **3-상태 모델의 실제 출처는 `github/client.ts:2908`
`getPRForBranchOutcome` + `shared/types.ts:1228-1242`**(found/no-pr/upstream-error)이지
`forge-provider.ts:60-72`가 아니다 — 후자는 null-or-throw로 접히고, `pr-refresh-coordinator.ts`가
소비하는 3-상태는 한 층 아래에 산다(Codex N1). `ReviewLookup`을 트레잇에서 직접 돌려주는
것이 JS의 throw보다 **더 충실한** Rust 번역이다.

```rust
#[async_trait]
pub trait ForgeProvider {
    /// **worktree 경로**로 repo 좌표 해석(Orca `resolveRepository(context{repoPath,...})`,
    /// `forge-provider.ts:63`). URL 문자열이 아니다 [Codex B1] — gh impl은 그 cwd에서
    /// `gh repo view`로 owner/repo·호스트를 자체 해석하고(`github/client.ts:1593-1603`),
    /// 미래 HTTP impl은 `git remote get-url origin`을 읽어 파싱한다. URL을 인자로 강제하면
    /// gh가 무시하거나 suaegi가 GitHub URL 파서(ssh/https/GHES)를 새로 짜야 해 — gh가
    /// 이미 하는 걸 재구현하게 된다(조사 §0가 피하려던 것).
    async fn resolve_repository(&self, worktree: &Path) -> Result<Option<RepoCoords>, ForgeError>;
    async fn review_for_branch(&self, repo: &RepoCoords, branch: &str) -> ReviewLookup;
    async fn review_by_number(&self, repo: &RepoCoords, number: u64) -> ReviewLookup;
    /// 생성 지원 여부(Bitbucket은 false — 조사 §1). 7a는 GitHub만이라 항상 true.
    fn supports_review_creation(&self) -> bool;
    async fn create_review(&self, input: CreateReviewInput) -> Result<Review, ForgeError>;
}
```

**상태 모델 found/none/unavailable, 단 `Unavailable`은 분류된 에러를 든다** [Codex S1].
raw `String`은 (1) stderr를 UI에 누출하고 (2) UI가 상태별 대응("gh auth login" vs "레이트
리밋 재시도")을 못 하고 (3) §5의 mutation 테스트를 공허하게 만든다(아무 문자열이나 생존).
Orca는 `pr-refresh-error-classification.ts:20-122`에서 순서 있는 분류기로 raw stderr가
UI에 닿지 않게 한다.

```rust
pub enum ReviewLookup {
    Found(Review),                 // PR 있음
    None,                          // PR 없음 (확정)
    Unavailable(ForgeUnavailable), // 조회 실패 — **None과 구별**. 일시 오류가 알려진 PR을
                                   // 지우면 안 된다(MVP의 Authoritative/Degraded 규율).
}
pub enum ForgeUnavailable {
    NotInstalled,      // gh 없음
    NotAuthenticated,  // gh auth 안 됨 → "gh auth login" 안내
    RateLimited,       // 레이트 리밋
    Network,           // 네트워크
    Other(String),     // 분류 밖 — 여기만 메시지, 그것도 정제된 것
}
```

`Review`: number·state(open/merged/closed/draft)·title·url + CI 체크 요약(passing/failing/
pending — 7a 의식적 단순화, Orca의 REST→GraphQL→`gh pr checks` 3단 폴백은 안 함;
merge-queue/auto-merge 신호는 7b, Codex N2). `RepoCoords`: owner/repo(+host, GHES).
`CreateReviewInput`은 Orca `CreateHostedReviewInput` 모양(`shared/hosted-review.ts:60-67`):
`worktree_path, base, head, title, body, use_template`(Codex N3).

## 2. 마일스톤

- **7a-1 — GitHub PR 생성 + 상태 via `gh`** (이 플랜의 MVP). 트레잇 + gh impl.
- **7a-2 — HTTP GitHub impl + 시크릿 저장 subsystem**. 같은 트레잇, 저장된 토큰.
- **7b — PR 상호작용**: merge/auto-merge(파괴적 확인), 리뷰·코멘트, PR 패널.
- **7c — GitLab via `glab`** (선택, 트레잇 재사용).
- **별도 플랜 — Jira/Linear** (이슈트래커, embedded — 7a-2의 시크릿 저장이 선결이므로
  그 뒤 해금).
- **미계획 — Gitea/Bitbucket/Azure** (embedded, env-var PAT, 저빈도).

## 3. 7a-1 상세 (gh)

### 3.1 `GhRunner` (suaegi-git 또는 새 크레이트)
`GitRunner`(`crates/suaegi-git/src/runner.rs:60`, `:182` `Command::new("git")`)를 그대로
미러 — `Command::new("gh")`, 같은 `run_with_timeout`(`runner.rs:143`) 규율(조사 §4.4:
멈춘 gh가 UI를 막으면 안 됨). env 규율: **`LC_ALL=C`**(GitRunner가 이미 set — §3.3의
stderr 분류가 영어 로케일에 의존하므로 gh에도 반드시 이어져야 한다), `stdin(Stdio::null())`,
그리고 **`GH_PROMPT_DISABLED=1`** [Codex S4] — Orca가 write 계열에 명시적으로 건다
(`client.ts:3994` 등). stdin null이 대부분 막지만 `pr create`가 정확히 그 write op라 belt-
and-suspenders로 넣는다.

**읽기는 `gh ... --json <fields>` 구조화 출력을 파싱**하고 사람 텍스트를 긁지 않는다 —
**단 두 예외를 명시한다**: (a) `gh pr create`는 `--json`이 없다(§3.3 B2), (b) "PR 없음"은
구조화 데이터가 아니라 비-0 exit + stderr다(§3.3 S3). `--json` 필드는 보수적으로 골라
스키마 결합을 줄이고, **최소 gh 버전을 preflight에서 고정**한다(Orca식 다중 폴백은 안 짊어짐,
조사 §4.3 — 단 create의 URL 파싱은 버전 문제가 아니라 영구 한계라 예외).

### 3.2 Preflight
- `gh` 미설치 / `gh auth status` 미인증을 구분해 감지하고 **"`gh auth login`을 실행하라"**
  메시지를 낸다(Orca `client.ts:1682` 미러). 실패를 불투명하게 던지지 않는다.
- 엔터프라이즈(GHES)는 **gh가 로그인된 호스트를 그대로 상속**한다(조사 §4.2, Orca
  `forge-provider.ts:128-129`). suaegi가 호스트를 따로 설정하지 않는다.

### 3.3 오퍼레이션 (7a는 생성 + 상태 **읽기**만, merge/리뷰는 7b — 조사 §4.6)
- **create PR**: `gh pr create` — title/body/base/draft, 선택적 repo PR 템플릿
  (조사: `pull-request-template.ts`, `client.ts:1867`). head는 worktree의 브랜치, target은
  그 브랜치를 소유한 origin(`client.ts:1832`). **`gh pr create`는 `--json` 출력이 없다**
  [Codex B2] — 어느 gh 버전도 없다(버전 스큐가 아니라 영구 CLI 한계). Orca도
  `JSON.parse`가 던지면 **출력된 PR URL을 정규식 파싱**한다(`client.ts:~1856`
  `parseCreatePRPayload`). 따라서 create는 `https?://<host>/<owner>/<repo>/pull/(\d+)`로
  PR 번호를 복구한다 — **§3.1의 "텍스트 안 긁는다" 규칙에 대한 의도된 예외**로 명시한다.
- **PR-for-branch 상태**: `gh pr view <branch> --json ...` + `gh pr checks`. **"PR 없음"은
  구조화 데이터가 아니다** [Codex S3]: 성공 호출은 "no PR"을 데이터로 안 돌려주고, 그냥
  비-0 exit + stderr다(Orca `isNoPullRequestError` `client.ts:220-223`:
  `/no pull requests? found|could not find.*pull request/i`). 매핑은 "비-0 exit + 고정
  영어 stderr substring = `None`, 그 밖의 실패 = `Unavailable(분류)`, 성공 = `Found`".
  suaegi의 기존 `run_expecting`/extra-ok-code 패턴(`compare.rs:144-157` `rev_resolves`)을
  재사용하고, LC_ALL=C(§3.1)로 stderr를 안정화한다.

### 3.4 생성 자격 게이팅 (조사 §1 하단, Orca `hosted-review-creation-eligibility`)
"Create PR"을 언제 **제안조차** 할지 판별하는 층. v1 최소 게이팅:
- 브랜치 존재 + gh 인증 + 기존 PR 없음, **그리고 `git rev-parse --abbrev-ref @{u}`가
  성공(upstream 추적 ref 존재 = 브랜치가 push됨)** [Codex B3]. Orca가 이걸 load-bearing으로
  취급한다 — `hasUpstream === false`면 `blockedReason: 'no_upstream', nextAction: 'publish'`로
  막는다(`hosted-review-creation-eligibility-snapshot.ts:55-97`). 빼면 push 안 된 브랜치에
  "Create PR"이 뜨고 `gh pr create`가 tty 없이 불투명하게 실패한다. git 호출 하나면 되고
  suaegi가 이미 아는 것이다.
- 정교화(dirty/needs_sync 등)는 follow-up.

### 3.5 도메인 · 영속화
`Worktree`(`crates/suaegi-core/src/domain.rs:41`)에 `linked_github_pr: Option<u64>` 추가
(Orca `hosted-review.ts:45`). **`#[serde(default)]` 필수** — 단일 JSON 영속화에 additive,
스키마는 forward/back 호환(`persistence.rs:96-105`), 옛 파일은 필드 없이 로드돼야 한다
(Plan 6 follow-up에서 얻은 교훈). 이 번호로 리뷰를 재해석한다.

### 3.6 UI (net-new iced)
- **Create PR 다이얼로그**: title/body/base/draft. worktree 컨텍스트 메뉴/사이드바에서.
- **worktree별 PR 상태 인디케이터**: open/merged/draft + CI(passing/failing/pending).
  `Unavailable`은 "상태 모름"으로 표시하지 "PR 없음"으로 표시하지 않는다.
- 리뷰 패널은 7b.

### 3.7 새로고침 정책 (7a-1 명시 — Codex S2)
Orca의 실제 답은 `pr-refresh-coordinator.ts`(900+줄: 우선순위 큐·백그라운드 예산·상태별
freshness·백오프·창 가시성)이지만 **전부 follow-up으로 미룬다**(§7). 그러면 7a-1에 트리거가
0이 돼 구현자가 "매 렌더마다 폴링(gh 도배)" 또는 "첫 로드 후 영영 안 갱신"으로 추측한다.
**7a-1 정책을 명시한다**: worktree가 **active가 될 때 1회** 조회 + **명시적 수동 새로고침**
액션. 백그라운드 폴링은 코디네이터를 짓는 follow-up 전까지 없다.

## 4. 7a-2 상세 (HTTP + 시크릿 저장)

- **시크릿 저장 subsystem** (조사 §3.4, 하드 선결). suaegi는 평문 JSON뿐
  (`persistence.rs:189`), 키체인 없음. `keyring` 계열 크레이트로 macOS Keychain / Windows
  Credential Manager / Linux Secret Service 추상. **토큰은 절대 JSON 영속화에 안 들어간다.**
  interim으로 env-var(Orca의 Gitea/Bitbucket 모델)도 가능하나 v1은 키체인 목표.
- **HTTP GitHub impl**: 같은 `ForgeProvider`, REST(+필요시 GraphQL) 직접 호출, 저장된
  토큰 Bearer. 구조화 에러, gh 미설치 환경 지원.
- **provider 선택**: gh 설치·인증 → gh impl; 아니면 저장된 토큰 있으면 HTTP impl; 둘 다
  없으면 preflight가 안내. `getForgeProviderForRepository`(조사 §1) 등가.

## 5. 테스트/mutation 전략

- **GhRunner 테스트 = PATH에 스크립트 fake `gh`** [Codex #Q3 해소]. suaegi의 `GitRunner`는
  주입 불가한 concrete struct(`runner.rs:60`)이고 테스트는 tempdir에 **실제 git**을 돌린다
  (`tests/fixture/mod.rs::init_repo`). gh엔 "가짜 GitHub"이 없으므로, 테스트 프로세스의 PATH
  앞에 **정해진 `--json` 출력/exit code/stderr를 내는 스크립트 `gh` 실행파일**을 얹는다
  (init_repo가 실제 repo를 스크립트하듯 바이너리를 스크립트). 이걸로 출력 파싱·exit code
  분류·None/Unavailable 분기를 트레잇 추상화 없이 실 단위 테스트한다. 실제 gh의
  create/merge/auth 종단은 §6 사람 눈.
- **found/none/unavailable 구분을 mutation으로 고정**: unavailable을 none으로 뭉개면
  "일시 오류가 알려진 PR을 지운다"는 회귀가 나야 한다(MVP의 Degraded 규율).
- **`--json` 파싱**: gh 출력 스키마를 픽스처로. 사람 텍스트 파싱 금지.
- **`linked_github_pr` serde(default)**: 옛 shape 로드 + round-trip.
- 실제 gh create/merge, 실제 네트워크는 헤드리스로 검증 불가 → **사람 눈**으로 명시.
- 모든 회귀 mutation 검증(구현자 계약에 명시, 리뷰어 독립 되돌림).

## 6. 사람 눈 (헤드리스 검증 불가)

- 실제 `gh pr create`가 진짜 PR을 만드는가, `gh pr checks`가 실제 CI를 읽는가.
- 키체인 저장/로드가 각 OS에서 실제로 동작하는가(7a-2).
- PR 상태 인디케이터·Create 다이얼로그 픽셀.

## 7. Follow-ups / 미룬 것

- 7b(merge/리뷰/PR 패널), 7c(GitLab), 별도 플랜(Jira/Linear), 미계획(Gitea/Bitbucket/Azure).
- 생성 자격 게이팅 정교화(§3.4), PR 새로고침 코디네이터/레이트리밋(Orca
  `pr-refresh-coordinator.ts`, rate-limit pill).

## 8. 미해결 — Codex 교차검증 반영 완료

Codex 판정 **IMPLEMENTABLE-AFTER-FIXES**. BLOCKER 3 + SHOULD-FIX 4를 위에 반영했다:
- **B1**(§1): `resolve_repository`가 URL 아닌 **worktree 경로**를 받는다(transport 누출 제거).
- **B2**(§3.3): `gh pr create`는 `--json` 없음 → PR URL 정규식 파싱(명시된 예외).
- **B3**(§3.4): 생성 게이팅에 **upstream 존재** 체크 추가.
- **S1**(§1): `Unavailable`이 분류 enum(`ForgeUnavailable`)을 든다.
- **S2**(§3.7): 7a-1 새로고침 정책 명시(active 시 1회 + 수동).
- **S3**(§3.3): "PR 없음"은 비-0 exit + LC_ALL=C stderr substring.
- **S4**(§3.1): `GH_PROMPT_DISABLED=1`.
- **N1/N2/N3**: 상태 모델 출처 인용 정정(`client.ts:2908`/`types.ts:1228`), CI 단순화 의식적,
  `CreateReviewInput` 필드 명시.

남은 것:
- **#Q1 확정**: 트레잇을 N=1에서 의식적으로 받아들인다(gh+HTTP+7c가 뒤에 들어옴). Codex 동의.
- **#Q4** 시크릿 크레이트(`keyring`)의 **Linux Secret Service는 데몬(gnome-keyring/kwallet)이
  필요**해 헤드리스/SSH 박스에서 실패 [Codex S5] — suaegi가 SSH로 쓰일 수 있으므로 7a-2
  착수 시 폴백 설계 필요. 7a-1 밖.
- **#Q5** 7a=create+status, merge=7b 확정(파괴적 확인 바, Codex 동의).
- **#Q6** `gh` 하드 의존이 의도된 자세(조사 §4.1). "gh 없이도"가 요구면 7a-2(HTTP)가 그걸 해결.
