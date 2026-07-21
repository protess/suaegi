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

## Plan 4 조사 중 발견 (기존 문제, 이 작업이 만든 것 아님)

22. ~~**`suaegi-term`의 `pty_test`가 플레이키하다.**~~ → 원인 규명 후 수정 완료
    (`crates/suaegi-term/src/pty.rs`의 `open_pty_retrying`).

    **가설이 틀렸다.** "자식이 준비되기를 기다리는 고정 대기"가 원인일 거라 추정했지만
    아니었다 — 테스트의 대기는 전부 이미 조건 폴링 + 10초 데드라인이라 넉넉했다.
    패닉 지점을 실제로 읽어 보니 **어서션이 아니라 전부 `PtySession::spawn(...).unwrap()`
    줄**이었고, 에러는 `Pty("failed to openpty: Os { code: -6, ... }")`였다.

    **진짜 원인**: macOS(Darwin)의 `openpty(3)`가 동시 호출에 안전하지 않다. 프로젝트
    코드가 전혀 없는 순수 C 프로그램으로 재현했다 — 스레드 14개가 배리어로 동시에
    `openpty`를 부르면 5600회 중 55회가 실패하고, 실패 시 `errno`조차 유효하지 않다
    (`-6`). 단일 스레드 프로세스 14개를 동시에 돌려도 실패하므로 이 경쟁은
    **프로세스를 넘나든다** — 그래서 여러 테스트 바이너리를 동시에 돌리는
    `cargo test --workspace`에서 특히 심하게 터졌다(실측 90% 실패). 실패하는 테스트
    집합이 매번 달랐던 건 그저 **경쟁에서 진 테스트가 매번 달랐기** 때문이다.

    **수정**: `openpty` 실패를 유한 횟수(4회) 재시도한다. 타임아웃을 늘려 덮은 것이
    아니라 일시적 오류를 재시도하는 것이다 — 실측에서 실패 55회가 **전부 두 번째
    시도에서** 성공했고 3번째가 필요한 경우는 0회였다. 첫 재시도는 즉시,
    이후만 백오프한다. 지속 실패는 삼키지 않고 마지막 오류를 올려보낸다.
    프로세스 간 경쟁이라 프로세스 내부 뮤텍스로는 막을 수 없다.

    **실측 (동일 스트레스 A/B, 재시도만 껐다 켬)**:

    | 시나리오 | 재시도 OFF | 재시도 ON |
    |---|---|---|
    | `pty_test` 단독 30/40회 | 19/30 통과 | **40/40** |
    | `pty_test` 6개 동시 × 5라운드 | 3/30 통과 | **30/30** |
    | `pty_test` CPU 14코어 포화 25회 | — | **25/25** |
    | `session_test` 20회 | 6/20 통과 | 19/20 (남은 1개는 아래 23번, 별개 원인) |

    `session_test`도 `TerminalSession::start` → `PtySession::spawn` 경로라 같은
    원인이었고 같은 수정으로 해결됐다. `grid_test`/`presence_test`는 PTY를 열지 않아
    무관하다. 재시도 메커니즘 자체는 `pty.rs`의 단위 테스트 5개로 검증했고,
    5가지 뮤테이션(재시도 비활성화/첫 오류 반환/첫 백오프 비-즉시/is_zero 가드 제거/
    루프 off-by-one)이 각각 해당 테스트를 실제로 실패시키는 것을 확인했다.

23. **`flooding_unread_device_queries_does_not_grow_memory_unbounded`가 병렬 실행에서
    플레이키하다** (`crates/suaegi-term/tests/session_test.rs`). 22번과는 **다른 원인**이라
    이번에 고치지 않고 남긴다. 이 테스트는 `process_rss_kb()`로 **프로세스 전체** RSS를
    재는데, 같은 프로세스 안에서 다른 13개 테스트가 동시에 할당하고 있어 그 할당이
    측정값에 섞인다. 실측: 단독 실행(`--exact`) 25회는 25/25 통과, 전체 병렬 실행
    20회 중 1회 실패(`RSS grew by 10672KB`, 임계값 10MB). 즉 임계값을 넘긴 주체가
    이 테스트가 아닐 수 있다. 제대로 고치려면 프로세스 전역 값 대신 큐 자체의 크기를
    관측하거나 이 테스트만 직렬화해야 한다 — 어느 쪽도 이번 수정 범위 밖이라 기록만
    해 둔다.

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

## Plan 3 최종 리뷰에서 넘긴 것

20. **앱 종료 시 세션 drop이 UI 스레드에서 일어난다.** `AppState`/`SessionStore`가
    보통 경로(`close()` → `Reaper`)를 거치는 건 pane을 하나씩 닫을 때뿐이다.
    창을 닫아 앱이 종료될 때는 `iced::application(...).run()`이 이벤트 루프를
    빠져나오며 `AppState`(그리고 그 안의 `SessionStore` 슬롯들)를 제자리에서
    drop한다 — 스토어가 마지막 클론을 들고 있으면 `Drop for TerminalSession`이
    그 스레드(창/이벤트 루프 스레드)에서 세션당 최대 2초, 슬롯 수만큼 순차로
    실행된다. 창이 멈춰 보이는 건 아니다(이미 닫히는 중이라 아무도 안 본다) —
    종료가 지연될 뿐이다. 하지만 "마지막 drop은 UI 스레드 밖에서" 규칙이 지켜지지
    않는 유일한 경로다.

    깨끗한 수정은 `iced::window::close_requests()`를 구독해 첫 닫기 요청을
    가로채고, 그 시점에 모든 세션을 `SessionStore::close()`(→ Reaper)로 은퇴시킨
    뒤, 전부(또는 바운드된 타임아웃까지) 정리될 때까지 기다렸다가 그제서야
    `window::close(id)`를 실제로 발행하는 것이다. 이번 리뷰에서는 이걸 구현하지
    않았다: `Message` 변형과 구독 배선이 새로 필요하고, "창 닫기 요청을 가로채고
    실제로 닫는" 흐름은 이 저장소의 테스트 하네스(진짜 창이 없는 plain `#[test]`)로
    의미 있게 검증할 방법이 없다 — 마우스/키보드로 창을 조작하는 것도 이 플랜
    범위에서 명시적으로 금지돼 있다(위 19번 참고). 검증 못 할 종료 경로 변경을
    머지 직전에 밀어 넣는 것보다, "행이 아니라 지연된 종료"라는 지금 동작을
    문서화해 두는 쪽을 택했다. 실제 창으로 종료를 조작해 확인할 수 있는 사람이
    붙는 시점(또는 Plan 4/5에서 UI 자동화가 생기는 시점)에 다시 본다.

## PR4 적대적 리뷰에서 넘긴 것

21. **백그라운드 클로저 안의 임의 패닉은 여전히 가드를 영영 못 푼다.**
    (`crates/suaegi-app/src/session_store.rs`) 이번 리뷰에서 `probe_with`의
    poisoned-mutex `expect`는 락을 회수하는 쪽으로 고쳤다(패닉 원인 하나
    제거) — 하지만 `request_presence_with`/`request_snapshot`의 백그라운드
    스레드 클로저 안에서 그 자체가 아닌 다른 이유로 패닉이 나면(예:
    `ProcessProbe::command_line`의 커스텀 구현이 패닉하거나,
    `TerminalSession::snapshot()`이 그리드 인덱싱 버그로 패닉하는 경우)
    `PresenceReady`/`SnapshotReady`가 영영 전송되지 않고 `presence_in_flight`/
    `snapshot_in_flight` 가드가 그대로 묶여 그 세션의 배지/화면이 다시는
    갱신되지 않는다 — 에러도 재시도도 없다. `apply_snapshot`의 가드
    선해제(이번 리뷰에서 고침)는 "결과가 도착했는데 stale"인 경우만
    구한다 — 결과가 아예 전송되지 않는 이 경우는 못 막는다. 모든 백그라운드
    클로저를 `catch_unwind`로 감싸거나 타임아웃/재시도 메커니즘을 넣는
    건 이번 항목이 요구한 "cheap hardening" 범위를 넘는 elaborate
    machinery라 지금은 하지 않았다. `PsProbe`(실제 `ps` 호출)와
    `TerminalSession::snapshot()`은 알려진 패닉 경로가 없어 지금 당장의
    실사용 위험은 낮지만, 커스텀 `ProcessProbe` 구현이 늘어나거나
    `snapshot()`이 더 복잡해지면 재검토한다.
