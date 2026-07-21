# 추적 중인 후속 항목

리뷰에서 확인됐지만 해당 플랜 범위 밖이라 미룬 것들. 각 항목은 **언제까지** 처리해야
하는지를 함께 적는다.

## Plan 3(UI 배선) 시작 전에 처리 — 완료

세 항목 모두 처리됐다(각각 별도 커밋, `term-hardening` 브랜치).
자세한 내용은 `.superpowers/sdd/hardening-c-report.md` 참고.

1. ~~`PtySession::try_wait`가 fire-once다~~ → `b03dabd`로 수정. `Lifecycle`에
   `exit_code: Option<i32>`를 추가해 `wait()`/`try_wait()` 어느 쪽이 먼저
   수확하든 이후 `try_wait` 호출이 항상 알려진 코드를 돌려주게 했다(멱등).

2. ~~`match_agent`의 경로 세그먼트 과매칭~~ → `bcd6b5b`로 수정. 실행 파일
   토큰은 이제 basename만 검사하고, 런처의 스크립트 인자(두 번째 토큰)만
   세그먼트 전체를 검사한다.

3. ~~`TerminalSession::Drop`의 unix join이 멈출 수 있다~~ → `befb642`로 수정.
   `join_with_deadline`이 2초 데드라인까지만 폴링하고 넘기면 detach한다.
   (macOS/Darwin에서는 `killpg` 이후 세션 리더가 죽으면 탈출한 자손이 슬레이브를
   들고 있어도 마스터가 즉시 EOF를 본다는 걸 실측으로 확인해 — Linux와 다른
   BSD 계열 pty 동작 — 이 시나리오 자체는 이 호스트에서 재현되지 않았다. 그래서
   `join_with_deadline` 메커니즘 자체를 직접 단위 테스트했다.)

## CI 도입 시 처리

4. **테스트 타임아웃 하네스**
   `saturated_write_queue_does_not_stall_the_reader`(session_test.rs)는 회귀 시
   깔끔한 어서션 실패가 아니라 **행**으로 실패한다. 이 저장소엔 아직 CI가 없어
   지금은 무해하지만, CI를 붙일 때 `cargo-nextest`의 per-test timeout이나 잡 레벨
   타임아웃을 반드시 함께 설정한다.

5. **`CACHE_REVALIDATE_AFTER` 경계 미테스트** (`crates/suaegi-term/src/presence.rs`)
   20회 히트 후 재검증 경로에 테스트가 없다. 폴링 주기를 소유하는 Plan 3에서
   이 상수가 의미를 갖게 되므로 그때 단위 테스트를 추가한다.

## 성능 — 실측 후 판단

6. **스냅샷 셀 복사 비용** (`crates/suaegi-term/src/grid.rs`)
   `snapshot()`이 뷰포트 셀을 매번 복사한다(빈 셀 패딩 포함). 일반적인 터미널
   크기에서는 문제없지만 Plan 4의 렌더 경로에 놓인다. 실제 렌더 벤치마크를 보고
   판단한다 — 추측으로 최적화하지 않는다.

## 결정 필요 (코드 변경 보류)

8. **Windows에서 `claude.exe` 미탐지** (`crates/suaegi-term/src/agent.rs`)
   `process_names`가 codex는 `&["codex", "codex.exe"]`로 두 형태를 다 갖고
   있지만 claude는 `&["claude", "claude-code"]`뿐이라 `.exe` 확장자가 없다.
   Windows에서 basename 매칭이 `claude.exe`를 놓친다(pre-existing, 이
   브랜치의 변경으로 생긴 문제 아님). `bcd6b5b`에서 확정한 basename-only
   매칭 규칙과 어떻게 맞물릴지(단순히 `"claude.exe"`를 추가할지, 확장자를
   벗기는 정규화를 basename_matches에 넣을지) 별도로 결정한 뒤에 고친다.

## 개발 환경 (코드 아님)

7. **전역 gitconfig의 평문 PAT**
   `/Users/james/projects/james/.gitconfig`의 `url.https://protess:<TOKEN>@github.com/.insteadOf`
   규칙에 토큰이 평문으로 있다. 그냥 지우면 같은 파일이 github.com 헬퍼를 gh(회사
   계정)로 고정해두어 protess 저장소들이 깨진다. 계정 분리 정책(예: 디렉토리별
   `includeIf` + keychain)을 정한 뒤 정리해야 한다.

## suaegi-core — 미래 스키마 가드의 허점 (Plan 3 리뷰에서 발견) — 완료

9. ~~**미래 스키마 **백업**은 가드를 세우지 않는다**~~ (`crates/suaegi-core/src/persistence.rs`)
   → `981342f`로 수정. `load_from_backups()`가 이제 `parse_trusted`의 거부 사유를
   구분한다 — 미래 스키마(`Err(true)`)면 `future_schema_guard`를 세우고 다음 슬롯을
   계속 보고, 손상/파싱 실패(`Err(false)`)는 지금처럼 그냥 건너뛴다. 회귀 테스트:
   `a_future_schema_backup_also_blocks_saves`(가드가 서야 함),
   `a_merely_corrupt_backup_does_not_block_saves`(쓰레기 백업은 막지 않아야 함).

## Plan 4로 넘기는 것 (터미널 커스텀 위젯)

Plan 3의 워크벤치(`crates/suaegi-app/src/workbench.rs`)는 읽기 전용 단색
모노스페이스 텍스트로 세션 → 스냅샷 → 구독 → 화면 사슬이 실제로 도는 것만
증명한다. 다음은 전부 Plan 4 몫이다:

10. **색/커서/폰트 속성.** 스냅샷 셀은 `fg`/`bg`/`flags`(alacritty_terminal의
    `Color`/`Flags`)를 이미 들고 있지만 지금은 버려지고 단색으로만 그린다.

11. **키 입력 → PTY.** 지금 워크벤치는 완전히 읽기 전용이다. `Widget::update`가
    포커스를 `operation::Focusable`로만 받으므로(`Widget::on_event`가 아니다,
    `canvas`로는 불가능) 커스텀 위젯이 필요하다. `TerminalSession::write`가
    돌려주는 `bool`(입력 유실 여부)을 피드백하는 UI도 이때 같이 들어간다.

12. **마우스(선택/스크롤/마우스 리포팅) + pane_grid와의 합성.** 터미널 본문이
    마우스 이벤트를 소비해야 하는데 `pane_grid`도 같은 영역에 `on_click`과
    분할 히트테스트를 건다. 이 설계에서 가장 깨지기 쉬운 가정이므로 Plan 4에서
    가장 먼저 스파이크할 것(계획 문서에 이미 명시돼 있다).

13. **리사이즈.** 세션은 지금 고정 80×50으로 스폰된다(`session_store.rs`의
    `DEFAULT_ROWS`/`DEFAULT_COLS`). pane 크기에 맞춘 실제 리사이즈는 커스텀
    위젯이 크기를 알 수 있어야 가능하다.

## Plan 5로 넘기는 것

14. **세션 레이아웃 복원.** `PersistedState.session.active_worktree_id`는
    Task 8에서 배선했다 — worktree를 선택할 때마다 `AppState::persist()`가
    실제로 디스크에 쓴다(`state.rs`의 `Message::WorktreeSelected` 핸들러).
    하지만 부팅 시(`AppState::boot`/`from_load`)에는 읽지 않는다 — 재시작 후
    어느 worktree가 선택돼 있었는지, 어떤 pane 분할이 열려 있었는지 복원하는
    UI는 Plan 5 몫이다.

15. **worktree 메타데이터가 재조회 때마다 유실된다.** `AppState::persisted_snapshot`
    (`state.rs`)이 저장하는 `Worktree.created_at_unix_ms`/`created_with_agent`는
    `worktrees_by_repo`(git이 돌려주는 `WorktreeEntry`)에서 매번 새로 합성한
    자리표시자(`0`/`None`)다 — 실제 생성 시각·생성 에이전트를 추적하는 곳이
    Plan 3엔 없다. git이 그 정보를 안 주므로, 어딘가(아마 세션 시작 시점)에서
    직접 기록해 둬야 한다. 세션 레이아웃 복원이 이 메타데이터를 쓰게 되는
    시점에 같이 처리한다.

16. **에이전트 상태 3색(working/waiting/done).** 지금 사이드바 배지는
    "에이전트가 떠 있는가"만 안다(`AgentPresence`). hook 서버가 붙는 Plan 5의
    몫이다(계획 문서에 이미 명시돼 있다).

## Task 8에서 남긴 것

17. **future-schema 저장 가드가 부팅 시점엔 조용하다.** `PersistenceHandle::spawn`이
    반환하는 `LoadDiagnostics.save_blocked`는 `AppState::boot`이 지금 아무데도
    쓰지 않는다 — 가드가 서 있어도 사용자가 뭔가를 바꿔 첫 `persist()` 호출이
    실패할 때까지는(그제서야 `SaveStatus::Failed`가 상태 표시줄에 뜬다) 조용하다.
    Task 0이 막으려 한 게 바로 이 케이스(손상된 본파일 + 미래 스키마 백업)인데,
    사용자는 앱을 열고 몇 걸음 걷기 전까지는 "저장이 막혀 있다"는 걸 모른다.
    부팅 직후에 바로 보여줄지, 그냥 첫 실패 시 알리는 지금 방식으로 충분한지는
    UX 판단이 필요해 코드를 바꾸지 않고 남겨둔다.

18. **앱 데이터 파일 위치.** `crates/suaegi-app/src/persistence_thread.rs`의
    `default_data_file()`이 `dirs::config_dir()/suaegi/data.json`(macOS:
    `~/Library/Application Support/suaegi/data.json`)으로 정했다 —
    `workspace_root`(worktree들이 실제로 생기는 곳, 기본값
    `~/suaegi-workspaces`)와는 다른 위치다. 여태 이 결정이 어디에도 문서화돼
    있지 않았다.

19. **Step 2(종단 흐름) 중 사람이 손으로 확인해야 하는 부분이 남아 있다.**
    담당 에이전트는 마우스/키보드로 앱 창을 직접 조작할 수 있는 수단이 없었고
    (합성 클릭은 이 플랜의 좌표 계산이 멀티 모니터 환경에서 엉뚱한 창을 때린
    전례가 있어 명시적으로 금지돼 있다), 그래서 실제로 확인한 건 앱이
    뜨는지·부팅 시 repo/worktree가 복원되는지(데이터 파일을 직접 심어
    재현)뿐이다. **사람이 직접 확인해야 할 것**: worktree 생성 버튼 클릭 →
    실제 세션이 셸 출력을 보여주는지, 두 번째 worktree로 분할했을 때 양쪽이
    독립적으로 도는지, worktree 여러 개를 빠르게 닫아도 UI가 멈추지 않는지
    (reaper 검증 — `SessionStore`의 각 로직은 단위 테스트로 실측했지만 실제
    `pane_grid` UI에서 연타했을 때의 체감은 다른 문제다).
