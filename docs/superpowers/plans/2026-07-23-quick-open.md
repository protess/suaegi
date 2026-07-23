# Plan — Quick Open (퍼지 파인더) 확정

조사: `docs/superpowers/research/2026-07-23-quick-open.md` (Orca @ v1.4.150-rc.0,
인용 file:line 고정). Codex 교차검증 판정 **IMPLEMENTABLE-AFTER-FIXES** 반영(인용 전부
정확, spec-정밀 수정 6건). 이 문서가 구현 계약이다.

## 0. 결정 (조사 + Codex 확정)

Cmd+Shift+P 퍼지 파일 파인더(키바인딩이 이제 트리거 제공). **퍼지 매처는 순수 Orca 코드
이식**(라이브러리 아님 — 랭킹 가중치=UX, Orca 테스트가 오라클). 리스터 캐스케이드
rg→git ls-files→walk, **transient≠empty**(실패는 버퍼 버리고 reject/캐스케이드, 절대 잘린
목록을 완전한 것으로 반환 안 함 — 저장소 "무성 절단 금지"·"transient≠빈결과"). UI(팔레트
위젯·키보드 네비·하이라이트)는 deferred 사람눈.

## 1. 레이어링 (Codex Q4/Q5)

- **스코어러 → 새 `suaegi-fuzzy` leaf 크레이트, 의존성 0**(serde도 없이 — 순수 String→score).
  `suaegi-keys` leaf 선례. 어떤 suaegi 크레이트 뒤에도 안 앉음.
- **리스터 → `suaegi-git` 내부**(`pub mod quick_open`). 결정적: `resolve_in_worktree`가
  **`pub(crate)`**(compare.rs:388)라 리스터가 밖에 살면 crate-private 경로안전을 노출/중복해야
  함. GitRunner/`fs::list_dir`(심링크-refuse)/`check_ignored` 재사용.

## 2. 마일스톤

### M1 — 퍼지 스코어러 (`suaegi-fuzzy` 순수, Codex fix 1)
`quick-open-search.ts:38-147` verbatim 이식. `rank(query, files, limit=50) -> Vec<(path, score)>`:
- **순서 crux**: (1) `limit<=0`→`[]` **먼저**; (2) **`query.len()`(trim 전 raw UTF-8 바이트) > 2048**→`[]`
  (Rust `str::len()`=바이트라 직접; **trim 후 검사 금지** — 전-whitespace 2049 쿼리가 reject돼야 하고
  empty-passthrough로 새면 안 됨, Orca 테스트 :145-152가 증명); (3) `normalized = query.trim().replace('\\',"/").to_lowercase()`;
  (4) `normalized` empty→`files[..limit]` score=0(inputIndex 순, 스코어링 전무).
- subsequence: `qi`/`score`/`last_match_idx=-1`; 매칭 시 `gap = last==-1?0:ti-last-1`, `score+=gap`,
  **`if ti>0 && lower_path[ti-1] in {'/','.','-'}: score-=5`**(ti==0은 보너스 없음 — 반전 위험 crux),
  `last=ti; qi++`. 미매칭(`qi<len`)→reject(skip). 끝에 `if lower_filename.contains(normalized): score-=100`(1회).
- `prepare`: `lower_path = path.replace('\\',"/").to_lowercase()`, `lower_filename`=마지막 `/` 뒤.
  **출력 `path`는 verbatim**(매치 키만 정규화). 비교자 `(score asc, inputIndex asc)`, top-`limit`.
- **오라클**: `quick-open-search.test.ts`(11케이스 순수 Vitest) verbatim 이식. *mutation:* 부호(lower=better)
  반전, ti>0 boundary, size-before-trim, -100 substring, reject 각각.

### M2 — 리스터 캐스케이드 (`suaegi-git/src/quick_open.rs`)
**M2a 순수 빌더**: rg argv(2패스: `--files --hidden`, ignored는 `--no-ignore-vcs` 추가; exclude는
directory-form `!**/name` — contents-form 아님; `escapeGlobPath` + 항상 `!`/`:(exclude,glob)` 접두라
argv 주입 불가), git ls-files argv(primary `-z -s --cached --others --exclude-standard --directory
--no-empty-directory`; ignored는 `--cached` 빼고 `--ignored`), `-s` 파싱 정규식 **`^([0-7]{6}) [0-9a-f]{40,64} [0-3]\t`**
(Codex fix 2: SHA-256 지원, 40 하드코딩 금지). 순수 mutation 검증.

**M2b rg/git/walk 드라이버 + transient≠empty (Codex fix 3-6)**:
- **rg 티어**: 가용성 **upfront 1회**(`run_with_timeout` 5s, settled-guard). 2패스 각 10s;
  timeout/kill/spawn-error→버퍼 버리고 **reject**; exit0/1→resolve; exit2 & ≥1경로→resolve; exit2 & 0경로→reject.
  **rg 없음(upfront probe)→git 캐스케이드; rg 있는데 런 실패→하드 에러(second-chance git 폴백 절대 금지)**(fix 5).
- **git 티어**: `rev-parse --is-inside-work-tree` probe는 에러/timeout/non-zero에 **soft-fail→Tier3**(reject 안 함);
  확정-워크트리 후 primary ls-files 실패→**하드 reject(walk 캐스케이드 금지)**(fix 5). ignored-pass는 best-effort
  (실패해도 primary 유지). 디렉터리 placeholder는 **`classify_quick_open_git_entry` 4분기**(fix 3):
  ① gitlink 아니고 untracked-dir placeholder 아님→**keep(lstat 없이)**; ② else lstat: 실패→drop, non-dir→drop,
  dir+`.git`(hasGitEntry)→**fill-nested-repo**(평범 walk), dir 무-`.git`→drop.
  **`includeSymlinks` 전파**: untracked-dir placeholder(directoryPaths)는 **강제 true**(collapse 전 untracked 심링크가
  leaf로 보였으므로 재확장 시 누락 금지), gitlink(gitPaths)는 false; `collapse`가 descendant를 ancestor에 병합 시
  **flag를 위로 OR**(descendant true면 ancestor도 true 승격). (fix 3, 놓치면 collapsed untracked dir 내 심링크 누락.)
- **Tier3 walk**: bounded BFS(`fs::list_dir` 위 `is_dir`만 재귀 — 심링크 미traverse 무료), 하드 cap
  **`QUICK_OPEN_READDIR_MAX_FILES=10_000` + 10s deadline THROW(절대 truncate 아님)**. **deadline 이중 체크포인트**
  (배치별 — 빈 배치도 — AND 엔트리별, fix 6). Tier2 placeholder 확장도 **같은 walker 공유**(별도 primitive 아님).
- **타임아웃 (fix 4)**: GitRunner 기본 30s인데 Orca는 rg/git 10s(rg probe 5s) → `run_with_timeout(cwd, args,
  Duration::from_secs(10))` 명시(기본 30s 재사용 시 3배 느린 실패=UX 회귀).
- **crux**: rg-missing→git→walk 캐스케이드가 **절대 empty 반환 안 함**; SHA-256 파싱; 4분기 classify;
  includeSymlinks 전파; 이중 체크포인트. **오라클**: tempdir-repo fixture(스테이지→확인). `-z` 규율 재사용.

### M3 — excludePaths (얇은 순수 add)
중첩 sibling 워크트리를 segment-boundary root-relative 접두로 정규화(malformed/outside-root/root-equal
silent drop). rg/git argv exclude에 주입. *mutation:* 정규화·drop 규칙.

## 3. 남은 결정 (Codex §4)
maxResults bounded 모드는 **defer**(coupling 깊고 top-50 컷오프면 UI 충분). `check_ignored` 대신
`--exclude-standard`(별도 spawn 없음, check_ignored는 트리 데코용 유지). rg는 optional-with-upfront-fallback.

## 4. 순서 (확정)
M1 스코어러(suaegi-fuzzy) → M2a 순수 빌더 → M2b 드라이버 → M3 excludePaths. UI 팔레트는 사람눈 후속.
관련: [[mutation-verify-regression-tests]], [[suaegi-workflow]]
