# Plan — git write-ops depth (`suaegi-git` 확장) 확정

조사: `docs/superpowers/research/2026-07-23-git-write-ops.md` (Orca @ v1.4.150-rc.0,
인용 file:line 고정). Codex 교차검증 판정 **IMPLEMENTABLE-AFTER-FIXES** 반영(인용 전부
확인, M4 primitive 교정·M3 대폭 축소·커밋 identity 실수 방지). 이 문서가 구현 계약이다.

## 0. 결정 (조사 + Codex 확정)

suaegi는 워크트리 diff를 *보여주고*(`compare.rs`) 상태를 *분류*(`status.rs`)만 할 뿐
스테이징/커밋/discard를 못 한다 — coding-agent 루프(에이전트 작업→사람 리뷰→커밋/discard/
충돌해결)가 read-only 막다른 길. 이 갭을 메운다. **`suaegi-git` 확장**(신규 크레이트 없음).
불변식: 사용자 전역 git config 절대 미접촉, discard는 워크트리 경계 안, secrets/네트워크 0.

## 1. Codex 반영 픽스 (구현자 필독)

- **F1 — M4 discard는 기존 `resolve_in_worktree` 재사용, Orca 포팅 금지 (핵심).** suaegi
  `compare.rs:388-446`의 컴포넌트별 walk가 Orca `validateUntrackedDiscardTarget`
  (`git-discard-path-safety.ts:71-97`, realpath 2회+lexical 비교)보다 **strictly stronger**:
  중간 심링크를 위치 무관 **무조건** 거부(`compare.rs:421-423`), realpath 불필요.
  `ResolveMode::ExistingOnly`(=`resolve_preserve_symlink`)는 leaf 심링크를 `Resolved::Symlink`로
  **un-follow** 반환 = "링크 자체 삭제"(git clean/rm이 원하는 것, 그 모드 doc이 이미
  "delete/rename의 source"용이라 명시). lexical `''`/`.`/`..`/absolute/null-byte 거부가
  syscall 전(`compare.rs:399-412`)에 일어나 Orca `assertTargetIsWorktreeChild` 대체.
  - **ENOENT**: `resolve_in_worktree`가 NotFound면 discard는 **멱등 no-op 성공**(존재 안 하는
    걸 지울 것 없음) — Orca의 nearest-existing-parent walk 포팅 안 함(delete엔 불필요).
  - **TOCTOU**: tracked-restore 뒤 `.gitattributes` smudge/clean 필터가 부작용 낼 수 있으니
    **`git clean` 직전 `resolve_in_worktree` 1회 재검증**(2회 pass 아님 — 각 호출이 이미 fresh
    syscall walk). 마지막 재검증~clean exec 사이 좁은 TOCTOU는 `fs.rs:219` 쓰기 staleness처럼
    **명시하고 수용**(제거 시도 안 함).
  - **git clean**: `git clean -ffdx -- :(literal)<path>` 청크 배치(raw rm 금지 — pathspec-bounded
    traversal이 심링크 부모로 안 내려감, `status.ts:2089` 주석 확인).
  - Windows drive-relative(`C:foo`)는 어느 쪽도 미처리 — Windows 스코프면 `Component::Prefix`
    테스트 필요(플래그, non-blocking).
- **F2 — M3 대폭 축소.** v1 `status --porcelain=v1 -z`(suaegi `status.rs`가 이미 파싱)가
  `parseConflictKind`(`status.ts:877-896`)와 **동일한 XY 코드**(UU/AA/DD/AU/UA/DU/UD) 제공.
  새 v2 파서/git 호출 **불필요**: `classify_xy`(`status.rs:179-195`)가 이미 뽑는 (x,y)에서
  `FileStatus::Conflicted`→**`Conflicted(ConflictKind)` in-place 업그레이드**. submodule(160000)은
  suaegi가 모델 안 하므로 무관. `getConflictCompatibilityStatus`의 existsSync는 fs-only, 별개.
- **F3 — 커밋 identity 실수 방지.** 프로덕션 `commit_changes()`에 `-c user.name/user.email/
  commit.gpgsign` **절대 안 넣는다**(Orca `git-runtime-options.ts:6-15`: cwd/wslDistro/signal만
  전달, identity/signing override 0 — 실 유저로 그의 repo에 커밋하니 override는 서명 제거+가짜
  identity 회귀). **테스트 하네스만의 문제**: `tests/fixture/mod.rs::init_repo`(repo-local config +
  fixture 자체 setup에만 `GIT_CONFIG_GLOBAL`/`NOSYSTEM`) 재사용, 신규 격리 메커니즘 0.
- **F4 — `--no-verify` 안 붙임**(hooks 실 유저처럼 실행, 에이전트가 조용히 우회 금지).
- **F5 — bulk 비-트랜잭션**: 청크 실패 시 일부만 변경. bulk API는 `Result<(),E>` 대신
  **per-path 결과 벡터** 반환(Orca 대비 의도적 개선, 지금 API 설계 중이라 저렴).
- **F6 — discard tracked는 `git restore --worktree --source=HEAD`**(`--worktree`만, bare
  `--source=HEAD`로 떨구면 index도 리셋). staged-add/staged-rename-newname는 HEAD에 없어
  `ls-files --error-unmatch`는 tracked라 하나 restore가 실패 → **명시 통합 테스트 + 의도적
  에러 동작 결정**(조용한 미검 경로 금지).

## 2. 마일스톤 (smallest-first; 각 독립 mutation-verified; 보안 스텝 마지막)

- **M1 — 스테이징 (`stage`/`unstage`, single+bulk).** `git add -- :(literal)<path>` /
  `git restore --staged -- :(literal)<path>`, `BULK_CHUNK_SIZE=100` 청크(E2BIG 회피),
  per-path 결과 벡터(F5). *crux:* `literal_pathspec`(순수, WSL 백슬래시→/) + argv. bulk+single
  둘 다(Q3). *mutation:* pathspec 리터럴 누락, add vs restore 뒤바뀜. real-git 왕복(stage→
  `--cached` 확인→unstage→사라짐). 기초(전부 `literal_pathspec` 재사용).
- **M2 — 커밋 (`commit_changes`).** `git commit -m <msg>` → `{success, error?}`. *crux:* 에러
  채널 규칙 **stderr→stdout→message**(hook/GPG는 stderr, "nothing to commit"은 stdout — 순수,
  mutation), "empty index ⇒ success:false + stdout 메시지" 게이트. **F3 준수**(identity override
  0, fixture 재사용). `getStagedCommitContext`는 **defer**(Q2, 컴포저 UI 없음). *mutation:*
  채널 우선순위 뒤바꿈, empty 게이트 제거.
- **M3-lite — 충돌 kind (in-place).** `FileStatus::Conflicted`→`Conflicted(ConflictKind)`,
  `classify_xy`의 (x,y)에서 직접(UU→both_modified 등 7종, F2). 기존 `status.rs` 소비자 하위호환.
  operation-probe(`detect_conflict_operation`: MERGE_HEAD/rebase-merge/rebase-apply/CHERRY_PICK_HEAD
  → merge|rebase|cherry-pick|unknown) + `resolve_git_dir`(.git 파일 `gitdir:` 포인터 파싱,
  linked-worktree) + `abort_merge`/`abort_rebase`는 **별개 저위험 real-fs**를 `conflict.rs`에
  함께 착지(선택, 무공유 리스크). *crux:* XY→kind 매핑(순수), git dir 포인터 파싱. *mutation:*
  kind 매핑 오류, operation 우선순위.
- **M4 — discard (`discard`/`bulk_discard`). ⚠️ 데이터손실/보안, 마지막.** 최소-안전 스펙:
  타깃별 (1) `resolve_in_worktree`/`resolve_preserve_symlink` 검증(F1, 기존 primitive) — NotFound→
  멱등 no-op; (2) tracked 프로브 `ls-files --error-unmatch`(single)/`ls-files -z`(bulk, `-z`
  규율 재사용); (3) tracked→`git restore --worktree --source=HEAD -- :(literal)path`(F6); (4)
  untracked→**clean 직전 1회 재검증**(F1 TOCTOU) 후 `git clean -ffdx -- :(literal)path` 청크.
  discard `-ffdx`(ignored 삭제) 의도적(Q4) — 미tracked 디렉터리 선택 시 하위 ignored도 쓸어냄이
  literal-pathspec으로 blast 제한됨; **명시 테스트: nested .gitignore 파일이 삭제됨 단언**. *crux/
  보안:* symlink-parent escape/`..`/absolute/root/ENOENT-parent 각각 거부 후 **바깥 타깃 생존**
  적대적 매트릭스. *리스크 HIGH.* **머지 전 M4 전용 보안리뷰**(fs.rs `.git` denylist RCE 테스트
  posture). staged-add가 HEAD 없어 restore 실패하는 F6 케이스 통합 테스트.

## 3. 순서 (확정)
M1 스테이징 → M2 커밋(identity override 없음, fixture 재사용) → M3-lite 충돌 kind(in-place) →
M4 discard(`resolve_in_worktree` 위, 전용 보안리뷰) 마지막. 매 회귀 테스트 mutation 검증(파싱·
경로술어가 공허 테스트 최고 위험 — 저장소 이력 5회). 관련: [[mutation-verify-regression-tests]],
[[path-denylist-case-insensitive]], [[suaegi-workflow]]
