# Plan 9 — editor + file tree (확정)

조사: `docs/superpowers/research/2026-07-23-plan9-editor-filetree.md` (Orca @
v1.4.150-rc.0, 인용 file:line 고정). Codex 교차검증 판정 **IMPLEMENTABLE-AFTER-FIXES**
반영 완료(인용 정확도 ~15건 검증, 오류 0). 이 문서가 구현 계약이다.

## 0. 결정 (조사 §0 + Codex 확정)

- **Orca는 Monaco 풀 에디터를 임베드**하지만 **포팅하지 않는다.** Plan 9의 검증 가능한
  가치는 **백엔드**(경로 안전·디렉터리 리스팅·ignore/git-status·안전 read/write·외부
  런처 argv) — 전부 순수/mutation 검증 가능. UI 두 표면(트리 위젯, 최소 에디터)만 사람눈.
- 최소 에디터 위젯 = iced 0.14 내장 `text_editor`(cosmic-text). Monaco/LSP/멀티커서
  범위 밖(연기).
- **위협모델 (Codex 5)**: suaegi는 단일 프로세스 네이티브 앱 — Orca의 렌더러↔main 신뢰
  경계가 없다. Orca의 `getAllowedRoots`/`registeredWorktreeRoots`/외부경로 인가 LRU
  머신(`filesystem-auth.ts:39-517`)은 **미포팅**. suaegi의 경계는 **단일 워크트리
  containment**뿐. 그래도 경로 안전은 방어심층(버그·미래 에이전트 주도 쓰기)으로 유지.
- **이미 있는 것 재사용 금지 재구축**: suaegi diff 패널(Plan 5) == Orca Source Control
  changed-files 트리. **재구축 안 함.** File Explorer(전체 파일, lazy readdir)와 파일
  read/write 표면만 net-new. Git-status는 merge-base diff(`compare.rs`)와 **다른 연산**
  (`git status --porcelain`, working tree) — 새 리더 필요.

## 1. 크레이트 배치 (Codex Q1: 새 크레이트 없이 suaegi-git에 흡수)

fs 리스팅/read/write + 경로 안전은 `suaegi-git`에 둔다(이미 `resolve_in_worktree`가
`compare.rs`에 있고 ignore/status는 `GitRunner` 직접 필요). 별도 `suaegi-fs`는 지금
필요 없음. `suaegi-app`이 이들에 의존, 역방향 금지.

## 2. 마일스톤 (smallest-first, backend-before-UI; [eyes]=사람눈)

### M1 — 경로 안전 코어 (보안 경계, 최우선)
**기존 `compare.rs:360-398` `resolve_in_worktree`를 확장**한다(평행 구현 금지 — 크레이트
내 두 갈래 drift가 이 저장소 mutation 규율이 잡으려는 바로 그 클래스, Codex 3).
현재: 컴포넌트별 `symlink_metadata`로 중간 심링크 거부, leaf 심링크는 un-follow 반환
(read-only, `file_head_bytes`용). 추가할 것:
- **missing-path 조상 walk (Codex 2, 필수 — 연기 불가)**: create/rename/copy가 아직
  없는 leaf를 인가한다(`fs:createFile` `filesystem-mutations.ts:71-87`, `fs:rename`/
  `fs:copy` `:105-149`가 존재하지 않는 new path에 대해 매번 `resolveAuthorizedMissingPath`
  조상 walk `filesystem-auth.ts:340-369`을 친다). 가장 가까운 존재하는 조상까지 올라가
  canonicalize 후 재검증.
- **preserveSymlink 모드 (delete/rename용)**: 링크 자체를 대상으로, 타깃을 따라가지 않음
  (`filesystem-auth.ts:299-318`).
- **null-byte 명시 거부** (`:411,531`).
- **검사 순서 (Codex 4)**: 값싼 non-canonicalized lexical containment 먼저(realpath
  syscall 없이 즉시 거부) → 통과 시에만 realpath+재검사(`filesystem-auth.ts:293-338`).
- **crux/보안**: `../../etc/passwd` traversal, in-worktree 심링크→외부 escape,
  check-open TOCTOU, (macOS-first라 Windows drive-relative/`\\?\`/8.3은 방어적 거부만,
  깊은 패리티는 연기). **mutation 테스트가 traversal + symlink-escape + missing-path-
  parent-symlink 변형을 반드시 죽여야 한다.** 완전 자율.

### M2 — 디렉터리 리스팅 (한 레벨 readdir)
`list_dir(authorized_dir) -> Vec<Entry{name, is_dir, is_symlink}>`, dirs-first 후 name
정렬(`filesystem.ts:511-526`). lazy per-dir(Orca 트리; flat Quick-Open 리스터 아님).
심링크는 보고하되 디렉터리로 auto-follow 안 함. 권한거부 하위디렉터리는 우아하게 degrade.
완전 자율.

### M3 — ignore 필터 + git-status 데코레이션
`GitRunner` 위 두 새 함수:
- `check_ignored(root, rel_paths) -> set`. **선결 (Codex 1): `GitRunner`에 stdin 없음**
  (`runner.rs:188` `Stdio::null()` 하드코딩). `git check-ignore -z --stdin`
  (`check-ignored-paths.ts:18-30`)이 불가. **결정: `GitRunner`에 stdin 지원 변형 추가**
  (positional-arg `git check-ignore <path>...` 청킹은 인자 길이 한계·경로수 폭발 위험이
  있어 stdin이 정석). 이 stdin 추가를 M3 첫 서브태스크로.
- `working_tree_status(root) -> Map<rel, Status>` via `git status --porcelain=v1 -z`
  (merge-base `compare.rs`와 **별개 연산**, Codex Q4). `-z` rename 레코드는 두 NUL-분리
  경로 — `compare.rs:33-37,269-319`의 파싱 규율 재사용(같은 버그 클래스).
- 하드코딩 hide: `.git`, `node_modules`(`file-explorer-entries.ts:3-5`, 렌더러 등가).
- ignore 권위 = git. transient(timeout 등)은 절대 "무시 안됨"으로 뭉개지 말 것.
- 회귀 테스트 mutation 검증(porcelain -z 파싱, rename 두-경로). 완전 자율.

### M4 — 안전 파일 read
`read_file(authorized_path) -> {content|binary, size}`: 최대 크기(Orca 50MB text,
`filesystem.ts:130,565-569`), 첫 8192B null-byte sniff→binary 플래그
(`filesystem.ts:426-434`, suaegi 기존 `BINARY_SNIFF_BYTES`와 일치). 버퍼링 전 cap.
워크트리-상대 identity 반환. 완전 자율.

### M5 — 안전 파일 write (Codex 7로 대폭 축소)
`write_file(authorized_path, content)`: 매 호출 경로 main-side 재검증(렌더러 절대경로
불신), 디렉터리에 쓰기 lstat-가드(`filesystem.ts:816-829`).
- **원자적 쓰기 (Codex Q2, yes)**: temp-sibling + rename. `suaegi-core/persistence.rs:200-206`
  의 `NamedTempFile::new_in`+`.persist()` 패턴 재사용(`tempfile` 이미 workspace dep).
  크래시 안전 > Orca 패리티(Orca는 in-place `filesystem.ts:829`).
- **watcher 연기로 축소**: self-write 레지스트리(유일 목적이 fs-watcher 에코 억제,
  `editor-self-write-registry.ts:3-13`, TTL 750ms)·conflict 배너 **미포팅**. M5 =
  **원자적 쓰기 + 저장 후 disk-signature 재베이스라인(`editor-autosave-controller.ts:143-145`)
  + open/save 시점에만 staleness 검사**. 배너급 conflict UI는 watcher 추가하는 후속 플랜으로.
- **crux**: 외부 변경 시 편집 손실(open/save staleness 검사로 감지), 루트 밖 쓰기(M1).
  상태머신 자율. 완전 자율(배너 없음).

### M6 — 외부 에디터 런처 (순수, 깔끔한 승리)
`resolveExternalEditorLaunchSpec`(`external-editor-launch.ts:158-188`) 포팅: command +
워크트리 경로 → spawn spec. 3분기:
1. 직접 실행파일 경로(절대·구분자 있음·존재확인)→executable(`:167-175`);
2. 복합 커맨드(공백 포함)→shell spec(`/bin/sh -c` 또는 `cmd.exe /d /s /c`) + POSIX
   single-quote/Windows double-quote 경로 이스케이프(`:25-40,136-156`);
3. 아니면 bare CLI(기본 `code`)→executable(`:181-187`).
특수: Cursor는 `--new-window`(`:117-120`); win32 VSCode+WSL UNC는 `--remote wsl+<distro>`
(`:122-128`); nvim/vim은 Windows 콘솔 유지(`:8,108`). detached·stdio ignored·unref
(`shell.ts:64-71,92-95`).
- **crux — argv 주입**: 악의적 editor-command 설정 또는 shell 메타문자 경로가 커맨드를
  주입하면 안 됨. shell 분기 이스케이프(`escapePosixPathForShell` `:25-30`)가 가드 —
  **mutation 테스트가 unescaped-path 변형을 죽여야 함.** editor 설정은 suaegi **per-worktree/
  app JSON**에 저장, 사용자 전역 config 절대 안 씀(불변식). Orca `.test.ts` 테이블 미러.
  완전 자율.
- 주의(Codex c): `openInExternalEditor`는 containment 안 거침(`shell.ts:29-40`은 절대+존재만) —
  M6의 crux는 containment 아니라 argv 주입. 확인됨.

### M7 — 파일 트리 위젯 [eyes]
M2 리스팅 위 lazy-expand 트리 + M3 status/ignore 데코, 키보드 네비, 컨텍스트메뉴
mutation(§1c create/rename/copy/delete). 백엔드 위 얇음. drag-drop/inline-rename/가상
스크롤은 후속. James 픽셀 검증.

### M8 — 최소 임베디드 에디터 위젯 [eyes]
iced `text_editor` ↔ M4 read/M5 write/M6 open-in-external 배선. 평문 편집+저장.
타입별 뷰어(image/pdf/csv/ipynb)·syntax highlight는 후속. James 픽셀 검증.

## 3. 첫 컷 (Codex d)
**M1–M6(전부 백엔드·자율·mutation 검증) + M7(단일 UI 표면)**, M8은 James와 "느낌 맞나"
체크포인트. M6이 M8 전에도 "이 파일 편집" 스토리를 준다.

## 4. 미포팅/연기 (조사 defer + Codex)
- Monaco 등가(syntax/LSP/멀티커서/large-paste), rich-markdown(TipTap), 타입별 뷰어,
  **fs-watcher 서브시스템**(→ self-write 레지스트리·conflict 배너도 함께 연기),
  Quick Open 퍼지 팔레트(별도 리스터+매처, 후속 플랜), 트리 drag-drop/inline-rename/
  undo-redo, SSH/WSL 원격, `shell.trashItem` OS 휴지통(MVP는 일반 remove/suaegi 관리 휴지통).
- Orca 두 인가 primitive(containment `resolveAuthorizedPath` vs identity
  `resolveRegisteredWorktreePath`)는 linked-worktree가 repo 밖에 살 수 있어 분리됨 —
  suaegi는 그 등록 모델이 없어 **단일 containment만**. 구현 시 둘을 혼동 말 것(Codex a).
- 미래 external-import는 Orca의 "심링크 realpath-recheck" 말고 **"lstat로 심링크 즉시
  거부 + 서브트리 pre-scan"**(`filesystem-mutations.ts:319-361`, `preScanForSymlinks:597-611`)
  이 더 단순·안전 — 채택 권장(Codex b).

## 5. 순서 (확정)
M1(resolve_in_worktree 확장) → M3 stdin 선결 후 M3 → M2 → M4 → M5 → M6 → M7 [eyes] →
M8 [eyes]. M1이 M2/M4/M5의 전제. 관련: [[suaegi-workflow]], [[mutation-verify-regression-tests]]
