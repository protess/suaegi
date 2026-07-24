# Plan — git-remote (push/pull/fetch) 확정

조사: `docs/superpowers/research/2026-07-24-git-remote.md` (Orca @ v1.4.150-rc.0,
인용 file:line). Codex 교차검증 판정 **AFTER-FIXES** 반영(모든 인용 재확인, env-guard
전역화·force-with-lease patch-equivalence·pull ff-only 3수정). 이 문서가 구현 계약이다.

## 0. 결정 (조사 + Codex 확정)

coding-agent 루프(작업→commit→**push**→PR)를 닫는다. forge PR 생성의 암묵 전제.
**핵심: Orca는 토큰을 주입하지 않는다** — OS ambient credential helper(osxkeychain/GCM/gh/
SSH agent) 의존, git을 **비대화형으로만** 강제. 토큰 미주입이라 push가 **로컬 bare remote로
AV 가능**(라이브 github auth만 사람눈). **suaegi-secrets는 M1-M3에 불필요**(ambient 미러).

## 1. Codex 반영 픽스 (구현자 필독)

- **F1 — env-guard 전역화 (remote-only 아님).** Orca `nonInteractiveGitEnv`는 **모든** git 호출의
  기본 정책(`runner.ts:828-841`). suaegi `run_full`(runner.rs:228-252)이 단일 경로이고 이미
  `LC_ALL=C`/`GIT_TERMINAL_PROMPT=0`을 전역 설정 → **guard vars를 같은 전역 run_full에** 추가:
  `GIT_ASKPASS=''`, `SSH_ASKPASS=''`, `GIT_SSH_COMMAND='ssh -o BatchMode=yes'`, `GCM_INTERACTIVE=never`.
  로컬 ops엔 무해(credential 미접촉). **credential helper는 유지**(UI만 끔) — `credential.helper=''`/
  env_clear 절대 금지.
- **F2 — `credential.interactive=false`/`guiPrompt=false`는 `GIT_CONFIG_COUNT`/`GIT_CONFIG_KEY_n`/
  `GIT_CONFIG_VALUE_n` env 프로토콜**(Orca `appendGitConfigEnv` `runner.ts:62-79`) — `-c`를 subcommand
  앞에 끼우는 argv-순서 버그 클래스 회피. run_full에 config env 주입 메커니즘 추가.
- **F3 — force-with-lease는 bare 플래그로 출하 금지.** Orca는 `--force` 절대 안 씀, force-with-lease를
  `shouldForcePushWithLeaseForUpstream`(`git-upstream-status.ts:39-48`) = `hasUpstream && ahead>0 &&
  behind>0 && behindCommitsArePatchEquivalent`로만 auto. patch-equivalence(`git log --oneline
  --cherry-mark --right-only HEAD...<upstream>`, behind 전부 `=`=로컬 rebase-rewrite, probe 실패 시
  보수적 false, `upstream.ts:20-36`) probe와 **함께 M4로**. bare 플래그=Orca 안전바 회귀(자율 에이전트
  위험).
- **F4 — pull 기본 `--ff-only`(Orca와 의도적 divergence).** suaegi는 충돌해결 UI 없음 → merge-on-pull
  충돌 시 워크트리가 stuck. **ff-only는 loud/clean 실패**(stuck 상태 없음). plain merge/rebase는 충돌
  표면 생기면 opt-in.
- **F5 — "success"는 암묵.** git exit 0(up-to-date 포함)은 error normalizer 안 거침 → suaegi
  `GitError::Failed{code,stderr}`(runner.rs:323-330)가 1:1. 단일 refspec `HEAD:<branch>`라 partial-ref
  non-issue.

## 2. 마일스톤

### M1 — 순수 헬퍼 (`remote.rs` 신규, 전부 unit/mutation)
- **`strip_credentials_from_message`**(`git-remote-error.ts:4-23`): `user:pass@`는 **모든 scheme**,
  lone `user@`는 **http(s)만**(SSH `git@host`·`ssh://git@`는 보존 — scheme-anchored). 두 정규식 순차,
  `normalize_git_error_message` **맨 앞** 무조건 실행. *crux/mutation:* `git@github.com` 보존, `token@https`
  strip, **credential에 literal `@`** 케이스(순서-의존 self-heal 전용 테스트로 고정).
- **outcome 분류**: git stderr+exit로 success/up-to-date/non-fast-forward-rejected/auth-failed/network-failed.
  **non-ff는 push에만 gated**(`operation==push`), pull stderr에 오발화 금지. *crux:* **non-ff≠success**
  (대죄 — PR이 stale commit 가리킴).
- argv 빌더(push/pull/fetch), upstream 파스. env-guard(F1/F2)는 run_full에.

### M2 — fetch + pull (로컬 bare remote AV)
`fetch`, `pull --ff-only`(F4). bare remote round-trip(push해둔 걸 fetch/pull, up-to-date, divergent→ff-only
실패). *crux:* ff-only 실패가 clean Err(stuck 아님), transient≠false-negative.

### M3 — push + --set-upstream (env-guard fix)
`push`(단일 refspec `HEAD:<branch>`), 없으면 `--set-upstream`. **`--force` 절대 없음**(force-with-lease는
M4). 전역 env-guard(F1/F2) 적용. *crux:* **non-fast-forward-rejected가 성공으로 오분류 안 됨**(load-bearing
회귀 테스트 — bare remote에 divergent commit 만들어 push reject 유도, Err 단언). 토큰 미-argv-누출.

### M4 — force-with-lease + patch-equivalence probe (함께)
`shouldForcePushWithLeaseForUpstream` 게이트(F3) + `--force-with-lease`. patch-equivalence probe
(`--cherry-mark --right-only`, probe 실패→false 보수적). *crux:* 남의 divergent 기여가 있으면 force 안 함.

## 3. Deferred (명시)
(c) **라이브 HTTPS/SSH auth against github.com/gitlab** = 사람눈, 마일스톤 아님(수동 체크리스트).
suaegi headless는 ambient helper 없으면 push가 auth-failure로 no-op — 수용 갭, (c)에서 gh/GCM 선인증으로 처리.
submodule/WSL drop. suaegi-secrets `.expose()` 주입은 (c) 진입 시에만.

## 4. 순서 (확정)
M1 순수 헬퍼 → M2 fetch/pull(ff-only) → M3 push+set-upstream(env-guard 전역) → M4 force-with-lease+probe.
불변식: 토큰 절대 error/log/argv-누출(sanitizer 마지막 방어선), 사용자 전역 gitconfig 미접촉(env/-c per-invocation),
transient≠false-negative, 매 회귀 mutation 검증. 관련: [[mutation-verify-regression-tests]], [[suaegi-workflow]]
