# git-remote 조사: push · pull · fetch · upstream · 크리덴셜

> 2026-07-24. Orca v1.4.150-rc.0 소스를 **직접 읽고** `file:line`으로 인용한다.
> suaegi 쪽 주장은 `crates/…`로 인용한다. 구현하지 않는다 — 이 문서가 산출물이다.
>
> **가장 중요한 발견 한 줄:** Orca는 git에 **토큰을 절대 주입하지 않는다.**
> URL 임베드도, `GIT_ASKPASS` 스크립트도, `http.extraHeader`도 없다. 오직 **OS의
> 앰비언트 크리덴셜 헬퍼**(osxkeychain / GCM / gh credential helper / SSH 키)에
> 의존하고, git을 **비대화형으로 강제**해 프롬프트에 걸려 행(hang)하지 않게 할 뿐이다.
> 이 사실이 push의 자율 검증 가능성 전체를 결정한다(§1, §2).

---

## 0. 요약 — 이 조사가 확정한 결정

1. **크리덴셜 = 앰비언트 헬퍼 의존, 토큰 주입 없음.** Orca `src/main/git/remote.ts`의
   어떤 원격 연산도 토큰을 argv/env/URL에 넣지 않는다. `grep -rInE 'x-access-token|
   extraHeader|insteadOf' src/main`(non-test) = **0건**. 헬퍼는 살려두고
   대화형 UI만 끈다(`credential.interactive=false`, `GIT_TERMINAL_PROMPT=0`,
   `GIT_ASKPASS=''`, `SSH_ASKPASS=''`, `GIT_SSH_COMMAND='ssh -o BatchMode=yes'`,
   `GCM_INTERACTIVE=never`) — `git-credential-prompt-env.ts:83-121`, `runner.ts:636-655`.
2. **`stripCredentialsFromMessage`는 그럼에도 필수.** 사용자가 `https://x-access-token:
   TOKEN@github.com/...` 형태 URL을 remote에 심어놨을 수 있고, git 에러가 그 URL을
   에코한다. 주입 안 해도 **방어적 스크럽**은 남는다 — `git-remote-error.ts:29-34`.
3. **push는 대부분 자율 검증 가능(AV).** 토큰 주입이 없으므로 argv 구성·에러 분류·
   sanitizer·upstream 파싱은 순수 단위/뮤테이션 테스트, 원격 왕복(push/fetch/pull, up-to-date,
   non-fast-forward)은 **로컬 bare remote**로 전부 결정론적 검증 가능. 검증 불가한 건
   **오직 github.com 상대 라이브 HTTPS/SSH 인증**뿐(사람 눈, 후순위).
4. **레이어링: suaegi-git이 원격 연산을 소유하고, suaegi-secrets 의존은 넣지 않는다.**
   Orca처럼 앰비언트 헬퍼에 의존하면 `suaegi-git`은 secrets를 몰라도 된다. 토큰 주입은
   별도 후속(라이브 인증 마일스톤)에서만 필요하고, 그때도 `Secret`을 **egress에서만**
   `.expose()`한다. §4 참고.
5. **워크플로 깨뜨리는 죄:** non-fast-forward 거부를 "성공"으로 오분류하는 것. 코딩 에이전트
   루프(commit→push→PR)에서 push가 조용히 실패하면 PR이 낡은 커밋을 가리킨다. 이건
   discard가 데이터 유실 마일스톤이었던 것처럼 **워크플로 정합성 마일스톤**이다.

---

## 1. 원격 연산 서베이 (인용)

모든 연산은 `gitExecFileAsync(args, gitOptionsForWorktree(worktreePath, options))`로 실행되며,
env는 `runner.ts`의 `nonInteractiveGitEnv`/`buildNetworkSshPolicyEnv`가 씌운다(§2). 실패는
전부 `normalizeGitErrorMessage(error, op)`를 거쳐 던져진다 — `remote.ts` 각 catch.

### 1.1 push — `gitPush` (`remote.ts:179-217`)

- **argv:**
  `['push', ...(forceWithLease ? ['--force-with-lease'] : []), '--set-upstream', ...(target ? [remote, refspec] : ['origin', 'HEAD'])]`
- **upstream 자동 선택:** `pushTarget`이 없으면 `getConfiguredPushTarget`(:16-53)이
  `branch.<b>.pushRemote` → `remote.pushDefault` → `branch.<b>.remote` 순으로 remote를,
  `branch.<b>.merge`(`refs/heads/` 벗겨) refspec을 뽑아 `HEAD:<branchRef>`를 만든다.
  없으면 `origin HEAD` + `--set-upstream`으로 **첫 발행**(fork 오배송 방지 로직 :156-172).
- **`--set-upstream` 항상 켬:** 매 push가 upstream을 재확정. 첫 발행 시 tracking ref 생성.
- **`--force-with-lease`는 옵션 플래그**(`options.forceWithLease`), 절대 `--force` 아님.
  lease는 로컬 remote-tracking ref 기준이라 "내가 마지막으로 fetch한 이후 남이 밀었으면 거부".
- **명시 타깃 검증:** `validateGitPushTarget`(`push-target-validation.ts`)이
  `check-ref-format --branch <name>` + shape 검증. **인젝션 방어** — 브랜치명이 argv 옵션으로
  오해되지 않게.
- **분류:** 실패는 `normalizeGitErrorMessage(err,'push')`. non-fast-forward/`fetch first` →
  "Push rejected: remote has newer commits" (§1.6).

### 1.2 pull — `gitPull` / `gitFastForward` / `gitPullRebaseFromBase` (`remote.ts:220-291`)

- **정책: 기본 `git pull`(사용자 설정 merge 전략) 유지.** Orca는 rebase를 강제하지 않는다
  — "diverged 브랜치가 에러 대신 화해하고, 충돌은 기존 충돌 해결 플로로 표면화"(:274-277 주석).
- **divergence fallback:** Git 2.27+는 정책 없으면 divergent pull을 거부. `runPullWithDivergenceFallback`
  (`git-remote-error.ts:106-119`)이 그 에러를 감지하면 `--no-rebase`(historical merge default)를
  앞에 붙여 **한 번 재시도**. 단 pullArgs가 이미 `--rebase/--ff-only/…`를 지정했으면 재시도 안 함
  (`pullArgsSpecifyReconciliation` :98-100).
- **argv 변형 3종:**
  - 일반: `['pull']` (또는 effective upstream 있으면 `['pull', remote, branch]`).
  - fast-forward: `['pull', '--ff-only', …]`.
  - base에서 rebase: `['pull', '--rebase', source.remoteName, source.branchName]`.
- **upstream 해석:** `resolveEffectiveGitUpstream`(`git-effective-upstream.ts`)로 configured
  upstream이 없거나 legacy(origin/main tracking인데 push는 origin/<branch>)면 **UI가 보고하는
  effective 브랜치**를 pull. read cache 무효화(`runWithGitReadCacheInvalidation`)로 감싼다.

### 1.3 fetch — `gitFetch` (`remote.ts:296-311`)

- **argv:** `['fetch', '--prune', ...(target ? [remoteName] : [])]`. 항상 `--prune`(죽은
  remote-tracking ref 정리).
- 가장 단순 — 로컬 상태 안 건드리고 remote-tracking ref만 갱신. **read-ish** 연산이라
  M2에서 먼저.

### 1.4 upstream 감지/갱신 (`git-effective-upstream.ts`, `git-remote-error.ts:150-165`)

- `rev-parse --abbrev-ref HEAD@{u}`로 configured upstream. 없으면 `isNoUpstreamError`가
  **`fatal:` 접두사 + 특정 문구**(no upstream/no tracking/HEAD does not point/ambiguous
  `HEAD@{u}`)를 요구해 오탐 차단(:149-165).
- effective upstream: configured가 없으면 `refs/remotes/<remote>/<branch>` 존재로 추론.
- forge eligibility가 이미 이걸 씀 — `crates/suaegi-forge/src/eligibility.rs:35,71` "upstream
  추적 ref 존재 = 브랜치가 push됨". **push 마일스톤이 이 upstream 신호의 생산자**다.

### 1.5 크리덴셜 공급 메커니즘 (§2에서 상술)

없음(주입). 앰비언트 헬퍼 + 비대화형 강제.

### 1.6 에러 sanitization + 분류 (`git-remote-error.ts`)

`normalizeGitErrorMessage(error, op)` (:167-247) 순서:

1. **맨 앞에서 크리덴셜 스크럽**(:180) — 이후 모든 분기가 이미 redacted 텍스트로 동작.
2. submodule push 실패 상세(push/undefined only).
3. **non-fast-forward / `fetch first`** (push/undefined only, :198-203) → "Push rejected:
   remote has newer commits (non-fast-forward). Please pull or sync first." — fetch/pull에도
   같은 문구가 나오므로 **push로 게이트**.
4. pre-push hook 실패(push only) → hook 출력 그대로.
5. `could not read Username`/`Authentication failed` → "Authentication failed. Check your
   remote credentials." (**auth 실패**)
6. `Could not resolve host`/`Network is unreachable` → "Network error." (**네트워크 실패**)
7. `no tracking information`/`no upstream` → "Branch has no upstream. Publish first."
8. pull divergent → pull 정책 설정 안내.
9. local changes overwritten / untracked overwritten → 커밋/스태시 안내.
10. 폴백: `extractTailLine`(마지막 비어있지 않은 stderr 줄만 — 로컬 경로/env 누출 최소화).

**sanitizer 정규식**(:29-34):
```
USERPASS_URL_PATTERN = /([a-z][a-z0-9+.-]*:\/\/)[^\s/@:]+:[^\s/@]+@/gi   // 임의 scheme의 user:pass@
HTTPS_TOKEN_URL_PATTERN = /(https?:\/\/)[^\s/@:]+@/gi                     // http(s)의 단독 user@ (토큰)
stripCredentialsFromMessage = replace(USERPASS,'$1').replace(HTTPS_TOKEN,'$1')
```
**핵심 미묘함:** `user:pass@`는 아무 scheme나 스트립하지만 **단독 `user@`는 http(s)에서만**.
SSH의 `git@host:...`는 user-info가 필수라 스트립하면 URL이 깨진다. 이 비대칭이 정확성의 핵심.

---

## 2. 크리덴셜 공급 + 보안 규율 (SECURITY 마일스톤)

### 2.1 Orca의 실제 메커니즘 (인용)

Orca가 git에 크리덴셜을 주는 방법 = **주지 않는다.** git이 자기 크리덴셜 헬퍼로 스스로
찾게 두고, 대화형 UI만 봉쇄한다. `gitCredentialPromptGuardEnv`
(`git-credential-prompt-env.ts:83-121`):

```
GIT_TERMINAL_PROMPT = '0'          // git이 프롬프트 대신 에러
GIT_ASKPASS = env.GIT_ASKPASS ?? ''  // GUI 헬퍼가 막지 않게 (caller가 준 건 보존)
SSH_ASKPASS = env.SSH_ASKPASS ?? ''
GCM_INTERACTIVE = 'never'          // GCM은 terminal/askpass 무시하고 자체 GUI 열 수 있음
+ git config: credential.interactive=false, credential.guiPrompt=false  // 헬퍼는 살리되 대화만 끔
```
`runner.ts:636-655` `nonInteractiveGitEnv`가 추가로 `GIT_SSH_COMMAND='ssh -o BatchMode=yes'`
(SSH도 프롬프트 대신 에러). **주석이 이유를 못 박음(:637-641):** 헤드리스 `serve`에서 프롬프트에
걸린 git이 모든 클라이언트를 wedge시키니(issue #5308) fail-fast.

**의미:** push가 실제로 인증되려면 사용자 머신에 이미 osxkeychain/GCM에 캐시된 크리덴셜,
또는 gh가 설치한 credential helper, 또는 SSH agent 키가 있어야 한다. Orca는 그걸 **셋업하지
않는다** — 존재를 가정한다.

### 2.2 토큰이 로그/에러/argv에 새지 않는 이유

- **argv 누출 없음:** 토큰이 argv에 없으니 `ps`에도 안 뜬다(URL 임베드를 안 하므로). suaegi가
  나중에 주입한다면 이게 회귀 위험 1순위 — argv 대신 env/askpass로.
- **에러 누출 방어:** 그래도 remote URL에 토큰이 박혀 있을 수 있어 `stripCredentialsFromMessage`가
  **맨 먼저** 돈다. suaegi-forge에도 이미 같은 클래스 방어가 있음 —
  `crates/suaegi-forge/src/classify.rs:194` "token ghp_SECRET_LEAK leaked in stderr" 테스트.

### 2.3 suaegi로 옮길 때 — `Secret` 규율

`crates/suaegi-secrets/src/secret.rs`: `Secret`은 `Debug=Secret(***)`, `Display/Serialize/PartialEq
없음`, `Drop`에서 volatile zero-scrub. 값은 오직 `.expose()`로만. **규율: `.expose()`는 git
호출 egress 단 한 곳에서만, 에러 메시지엔 절대.**

suaegi-git 현 상태(`crates/suaegi-git/src/runner.rs:241-242`)는 `LC_ALL=C` +
`GIT_TERMINAL_PROMPT=0`만 세팅. env는 상속(`env_clear` 없음)이라 **앰비언트 헬퍼는 이미 동작**.
원격 연산엔 Orca 수준 가드가 빠져 있음 — M1에서 채울 **정확한 델타**:
`GIT_ASKPASS=''`, `SSH_ASKPASS=''`, `GIT_SSH_COMMAND='ssh -o BatchMode=yes'`,
`GCM_INTERACTIVE=never`, config `credential.interactive=false`/`credential.guiPrompt=false`.
(단독 `.env("GIT_ASKPASS","")`는 caller가 준 askpass를 지운다 — Orca는 `?? ''`로 보존. suaegi는
env 상속이므로 `env.remove` 대신 `if unset then set ''` 동등 로직 필요.)

---

## 3. 자율 검증(AV) 전략 — a/b/c 분리 (CRITICAL)

토큰 주입이 없다는 사실 덕에 **push조차 대부분 AV**다. 세 계층:

### (a) AV — 로컬 bare remote 왕복 (인증 불필요)
`git init --bare`(tempdir) → `git remote add origin <bare>` → push/fetch/pull. 두 번째 clone으로
assert. suaegi-git 테스트 하네스가 이미 tempdir git init을 함(`run/…` 하네스). 검증 가능:
- push 성공 + `--set-upstream`이 tracking ref/`branch.<b>.merge` 생성.
- **up-to-date** push(두 번째 push가 "Everything up-to-date").
- **non-fast-forward 거부**: cloneB가 밀고, cloneA가 stale 상태로 push → 거부 → 분류가
  "성공"이 **아님**을 assert (워크플로 죄 방지 회귀).
- **`--force-with-lease`**: lease stale일 때 거부 vs fresh일 때 통과.
- fetch `--prune`가 삭제된 remote 브랜치의 tracking ref를 제거.
- pull merge vs `--ff-only` 거부 vs divergent-fallback(`--no-rebase` 재시도) 경로.
- upstream 감지: push 전/후 `HEAD@{u}` 유무.

bare remote는 file:// 이므로 크리덴셜 헬퍼가 개입 안 함 — 순수 git 메커니즘만 탄다.

### (b) AV — 순수 함수 (git 무관)
- **`strip_credentials_from_message`**: `user:pass@`/`https token@` 스크럽, **SSH `git@host`
  보존**, 다중 scheme, 뮤테이션 검증 필수([[mutation-verify-regression-tests]]).
- **outcome 분류**: 고정 stderr 픽스처 → {success, up-to-date, non-fast-forward, auth-failed,
  network-failed, no-upstream, conflict}. Orca `remote.test.ts:279-307`가 픽스처 예시
  (`x-access-token:ghp_secret@...` → 결과에 `x-access-token` 없음).
- **argv 구성**: forceWithLease/target/set-upstream 조합의 argv 스냅샷.
- **upstream 파싱**: `HEAD@{u}` 출력 → remote/branch 분해, `isNoUpstreamError`의 `fatal:` 게이트.

### (c) AV 불가 — 라이브 인증 (사람 눈, 후순위 defer)
github.com/gitlab 상대 실제 HTTPS(토큰)/SSH(키) 인증. 앰비언트 헬퍼에 의존하므로 **테스트
머신의 실제 크리덴셜 상태**에 좌우 → 결정론 없음. 명시적으로 사람이 확인:
"실제 리포에 push → PR이 새 커밋 가리킴". 이건 마일스톤이 아니라 **수동 수용 체크리스트**.

**결론:** "크리덴셜 메커니즘이 push를 근본적으로 AV 불가로 만드는가?" → **아니다, 정반대.**
주입이 없어 메커니즘 전체가 file:// bare remote로 AV. AV 불가는 오직 (c) 라이브 인증뿐.

---

## 4. suaegi 크레이트/모듈 + 레이어링 + 불변식

### 4.1 배치
- **`suaegi-git`이 소유.** 새 모듈 `crates/suaegi-git/src/remote.rs`. `GitRunner`
  (`runner.rs`)를 그대로 씀. upstream 파싱은 `branch.rs`/신규 `upstream.rs`.
- **`suaegi-secrets` 의존 넣지 않는다(현 단계).** Orca가 앰비언트 헬퍼에 의존하듯 suaegi-git도
  토큰을 몰라도 push/pull/fetch가 동작. secrets 결합은 (c) 라이브 인증을 자동화하려 할 때만
  등장하고, 그때도 **주입은 호출자(app 레이어)가 `Secret`을 넘기고 egress에서만 `.expose()`**.
  → suaegi-git은 순수하게 유지, secrets는 상위에서. [[suaegi-project]]
- **GitRunner env 가드 확장**(§2.3 델타)은 `remote.rs`가 아니라 `runner.rs`의 원격 연산
  경로에. 로컬 연산(status/diff)엔 SSH/askpass 가드 불필요하니 원격 전용 헬퍼로 분리 권장.

### 4.2 불변식 (회귀는 전부 뮤테이션 검증 — [[mutation-verify-regression-tests]])
1. **토큰은 에러/로그/argv에 절대.** `Secret`은 egress `.expose()` 단일 지점. sanitizer가
   에러의 마지막 방어선.
2. **transient ≠ false-negative.** non-fast-forward/auth/network 실패를 성공으로 오분류 금지.
   특히 non-fast-forward = 워크플로 죄.
3. **사용자 전역 git config 절대 안 건드림.** 가드는 env + `-c`(per-invocation config), Orca도
   `appendGitConfigEnv`로 env 프로토콜만 씀 — `~/.gitconfig` 안 씀. [[suaegi-rustfmt-no-convention]]
   식으로 "전역 상태 오염 금지" 규율.
4. **비대화형 강제.** 원격 연산은 프롬프트에 hang하지 않고 fail-fast(BatchMode/TERMINAL_PROMPT=0).
5. **뮤테이션 하네스 mtime 함정 주의** — 원격 왕복 테스트도 bare remote 재사용 시 stale 바이너리
   위험. [[mutation-harness-mtime-trap]]

---

## 5. 마일스톤 분해 (smallest-first) + 크럭스

| M | 범위 | 검증 | 크럭스/위험 |
|---|------|------|------------|
| **M1** | 순수 헬퍼: argv 구성 + `strip_credentials_from_message` + outcome 분류 + upstream 파싱 | (b) 순수 단위/뮤테이션 | SSH `git@host` 보존 vs http 토큰 스트립 비대칭; non-fast-forward 분류가 성공 아님 |
| **M2** | fetch + pull (read-ish) — `--prune`, merge 기본, `--ff-only`, divergent `--no-rebase` fallback | (a) 로컬 bare remote | pull 정책(merge 유지 vs rebase 강제 — Orca는 merge); divergent fallback 한 번만 재시도 |
| **M3** | push + `--set-upstream` (+ `--force-with-lease`) — remote에 씀 | (a) 로컬 bare remote (up-to-date/non-ff/lease) | **크리덴셜 가드 env**; non-fast-forward 오분류 = 워크플로 죄; force-with-lease 안전성(절대 `--force` 아님); `--set-upstream`이 upstream 신호 생산 |
| **(c)** | 라이브 HTTPS/SSH 인증 (github.com) | 사람 눈, defer | 앰비언트 헬퍼/키 존재 가정; suaegi-secrets 주입은 여기서만 검토 |

M1→M2→M3 순. M3가 크리덴셜 가드와 워크플로 정합성을 동시에 안는 정점.

---

## 6. Codex 교차검증용 열린 질문

1. **크리덴셜 메커니즘 결정:** Orca를 따라 **앰비언트 헬퍼 의존**(suaegi-git이 secrets 무지)으로
   갈 것인가, 아니면 suaegi-secrets의 토큰을 **주입**(GIT_ASKPASS 스크립트/`http.extraHeader`/URL
   임베드 중 무엇으로?)할 것인가? 주입 시 argv 누출 없는 유일한 안전 경로는? (권고: M1~M3는
   앰비언트, 주입은 (c)로 defer.)
2. **`--force-with-lease`를 M3에 넣나, 별도 M4로 미루나?** 안전하지만 UI 노출 시 오용 위험.
   suaegi 코딩-에이전트 루프가 force push를 실제로 필요로 하나(rebase 후 재push)?
3. **pull 정책:** Orca의 "기본 merge 유지 + divergent fallback"을 그대로 이식하나, 아니면 코딩
   에이전트 워크플로엔 `--ff-only`나 rebase가 더 맞나? conflict를 어떤 플로로 표면화하나(suaegi엔
   아직 충돌 해결 UI가 [[suaegi-project]] 상 없음)?
4. **라이브 인증을 얼마나 defer?** (c)를 완전 수동으로 두나, 아니면 gh credential helper 존재를
   프로브해 "인증 준비됨" 신호(forge eligibility처럼)를 M3에서 낼 가치가 있나?
5. **env 가드 배치:** 원격 전용 헬퍼로 분리 vs `GitRunner::run_full` 공통 경로에 SSH/askpass
   가드 추가. 후자는 로컬 연산에 불필요한 env를 실지만 단순. (권고: 원격 전용.)
6. **suaegi에 submodule/WSL 로직이 필요한가?** Orca 코드의 상당 부분이 submodule push 실패
   정규화·WSL 경로 변환인데 suaegi 범위 밖일 가능성 — M1 분류에서 submodule 케이스를 뺄지.
