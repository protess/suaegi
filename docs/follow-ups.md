# 추적 중인 후속 항목

리뷰에서 확인됐지만 해당 플랜 범위 밖이라 미룬 것들. 각 항목은 **언제까지** 처리해야
하는지를 함께 적는다.

## 첫 실사용 테스트 세션에서 발견 (MVP 종단 확인)

MVP를 실제로 띄워 사람 눈으로 확인하다가 나온 것들. 헤드리스 하네스로는 **구조적으로**
못 잡는 부류다 — IME·창·실 프로세스가 있어야만 발동한다.

28. **IME 조합창 위치·인라인 렌더링이 최소 구현이다** (`crates/suaegi-app/src/terminal/`).
    한글 자소 분리는 `fix/terminal-ime-input`으로 고쳤다(위젯이 `Event::InputMethod`를
    처리하고 `Commit`을 PTY로 보낸다). 남은 것: **조합창 위치**가 커서 셀 기준이나
    폴백이 좌상단이라 정밀하지 않고, **on-the-spot 인라인 렌더링**을 안 해서 조합 중
    글자는 런타임의 over-the-spot 오버레이로만 보인다. 실사용에서 위치가 거슬리면
    `terminal/render.rs`에서 preedit를 커서 위치에 직접 그리는 쪽으로 간다.

29. **거부한 권한 요청이 배지를 주황(`Waiting`)에 남긴다 — claude가 종료 훅을 안 준다.**
    (`crates/suaegi-app/src/agent_status/`) **실측했다.** 훅 서버에 임시 로그를 붙여
    거부 시 도착 순서를 관측했다:

    ```
    SessionStart → UserPromptSubmit → PreToolUse → PermissionRequest
    [사용자가 "no" 선택] → (아무 훅도 오지 않음)
    ```

    즉 **거부는 claude의 턴을 중단시키는데 중단에는 `Stop`이 붙지 않는다**(정상 완료
    때만 Stop이 뜬다). 우리 리듀서는 마지막 훅(`PermissionRequest`→`Waiting`)을 정확히
    따르고 있고, `Waiting`은 나이로 감쇠하지 않게 설계됐다(답 없는 질문은 몇 시간이고
    정당하게 Waiting). claude가 프로세스로 살아있으므로 presence도 `Exited`/`NoAgent`로
    안 바뀐다 → **다음 훅(다음 `UserPromptSubmit` → 초록)이나 종료 전까지 주황에 고착.**

    이건 **우리 코드의 버그가 아니라** hook 기반 탐지의 구조적 한계다 — 거부에 대한
    신호가 애초에 오지 않으므로 리듀서가 손쓸 데이터가 없다. 정상 완료(회색 Done)와
    거부(주황 고착)가 둘 다 "claude가 유휴 상태로 프롬프트에서 대기"인데 색이 달라
    비일관적이다. 선택지:
    - **(a) 그대로 둔다.** 거부 후 claude는 실제로 다음 지시를 기다리므로 "이 pane이
      당신을 기다린다"(주황)는 틀린 말이 아니다. 종료 조건을 문서화만 한다.
    - **(b) presence로 보강한다.** claude가 유휴 프롬프트에 있는지(활동 없음)를 별도로
      감지해 `Waiting`을 `Done`으로 낮춘다. 다만 "생각 중"과 "유휴"를 프로세스 존재만으로
      구별할 신호가 지금은 없다 — 새 신호원(자식의 tty 상태 등)을 찾아야 한다.
    - 정상 완료(`Stop`→회색)가 실제로 도는지는 이 세션에서 확인하지 못했다(먼저
      마무리했다). (a)/(b) 결정 전에 그것부터 확인한다.

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

4b. **PTY를 여는 테스트가 전체 스위트 부하에서 여전히 플레이키하다.**
   `55c4abd`의 `openpty` 재시도가 대부분을 잡았지만, 동시에 도는 프로세스가 많으면 여전히 난다.
   특징이 일정하다: **전체 스위트에서만, 매번 다른 테스트 이름으로, 재실행하면 통과.**
   관측된 것 — `pty_test` 전반, `session_test`,
   `suaegi-app`의 `state::tests::accepting_a_started_session_registers_it_and_opens_a_pane`,
   `presence_poll::tests::the_guard_clears_when_the_result_arrives_so_the_next_tick_dispatches`.
   마지막 것은 픽스처가 실제 `TerminalSession::start`를 부르므로 같은 원인으로 보이나,
   그 실행의 패닉 메시지를 남기지 못해 **확정은 아니다.**

   Darwin의 `openpty`가 프로세스를 넘나들며 경쟁하므로(자세한 건 `55c4abd`) 프로세스 내부
   조치로는 못 막는다. CI를 붙일 때 **PTY를 여는 테스트를 직렬화**하거나(전용 게이트,
   또는 `--test-threads` 제한) 시스템 pty 풀 상한을 확인한다.

4c. **`suaegi-git` 테스트는 개발자의 전역 gitignore를 읽는다 — 고쳤지만 함정을 기록해 둔다.**
   Plan 5에서 발견: git이 파일을 **나열하는지**에 의존하는 테스트는 그 개발자의
   `~/.config/git/ignore` 내용만큼만 신뢰할 수 있다. 실제로 이 기계의 1번 줄이
   `**/.claude/settings.local.json`이라, 우리 주입 파일을 걸러내는 필터를 **삭제해도**
   테스트가 통과했다.

   **함정**: `GIT_CONFIG_GLOBAL`로는 안 고쳐진다. `core.excludesFile`이 설정 파일과 무관하게
   `$XDG_CONFIG_HOME/git/ignore`를 기본값으로 쓰기 때문에, 그 레버를 당긴 사람은
   **격리됐다고 잘못 결론 내린다.** `GitRunner`는 실제 앱에서 `GIT_CONFIG_GLOBAL`을 설정하면
   안 되므로(사용자 설정을 존중해야 한다) 러너를 바꾸는 것도 답이 아니다.

   **수정**: 공용 픽스처(`crates/suaegi-git/tests/fixture/mod.rs`)가 테스트 저장소에
   `core.excludesFile=/dev/null`을 설정한다. 새 테스트는 이 픽스처를 쓰면 자동으로 격리된다.

5. **`CACHE_REVALIDATE_AFTER` 경계 미테스트** (`crates/suaegi-term/src/presence.rs`)
   20회 히트 후 재검증 경로에 테스트가 없다. 폴링 주기를 소유하는 Plan 3에서
   이 상수가 의미를 갖게 되므로 그때 단위 테스트를 추가한다.

## 성능 — 실측 완료

6. ~~**스냅샷 셀 복사 비용**~~ (`crates/suaegi-term/src/grid.rs`) → **측정했고, 손대지 않기로 했다.**
   `TerminalSnapshot::clone`이 프레임당 **5.65µs**(24×80) / **27.72µs**(50×200)다.
   200×50에서 16ms 예산의 **0.17%** — 설계를 바꿀 가치가 없다.
   (실제 텍스트로 채운 그리드에서 쟀다. 빈 그리드로 쟀으면 `combining`이 전부 비어
   실제보다 유리하게 나왔을 것이다.)

   **damage 추적도 도입하지 않는다.** 셀 전체 `resolve_cell`이 9µs / 48µs이고 CPU
   프레임 준비 총합이 ~70µs / ~310µs다. damage 추적이 아끼는 건 최대 수백 µs인데,
   대가로 스크롤·리사이즈·선택·커서 이동에 걸쳐 dirty 영역 불변식을 계속 맞춰야 한다 —
   이 플랜이 아홉 라운드에 걸쳐 다른 곳에서 제거한 바로 그 종류의 버그다.
   실기기 프로파일에서 텍스트 준비가 지배적으로 나올 때만 재검토한다.

## 결정 필요 (코드 변경 보류)

8. **Windows에서 `claude.exe` 미탐지** (`crates/suaegi-term/src/agent.rs`)
   `process_names`가 codex는 `&["codex", "codex.exe"]`로 두 형태를 다 갖고
   있지만 claude는 `&["claude", "claude-code"]`뿐이라 `.exe` 확장자가 없다.
   Windows에서 basename 매칭이 `claude.exe`를 놓친다(pre-existing, 이
   브랜치의 변경으로 생긴 문제 아님). `bcd6b5b`에서 확정한 basename-only
   매칭 규칙과 어떻게 맞물릴지(단순히 `"claude.exe"`를 추가할지, 확장자를
   벗기는 정규화를 basename_matches에 넣을지) 별도로 결정한 뒤에 고친다.

## Plan 5 리뷰에서 결정이 필요해 남긴 것

28. **미래 스키마 거부에 앱 안에서 빠져나갈 길이 없다.** 가드 자체는 정확히 작동한다 —
    실측: 미래 스키마 파일을 열면 `load source = Default, guarded = true`, 저장은
    `Err("saving is blocked")`로 막히고, **백업 회전도 일어나지 않아 원본이 보존된다.**
    (17번과 달리 여기서 확인된 건 "조용하다"가 아니라 "회복 수단이 없다"이다.)

    문제는 회복 경로다. `LoadDiagnostics::save_blocked`(`persistence_thread.rs:116`)를
    `AppState`가 읽지 않고, `override_future_schema_guard`(`:217`)는 어떤 `Message`에도
    UI에도 연결돼 있지 않다. 앱을 다운그레이드한 사용자는 일반적인 `SaveStatus::Failed`만
    보고 **앱 안에서는 아무것도 할 수 없다.** 파일을 손으로 지우거나 되돌리는 수밖에 없다.

    UX 결정이 필요하다: 부팅 직후 알릴지, 덮어쓰기 버튼을 줄지, 백업으로 되돌리기를 줄지.

29. **직렬화에는 깊이 제한이 없다(로드에는 있다).** serde_json의 재귀 제한이 읽기를 막아주지만
    (실측: 깊이 1000에서 `recursion limit exceeded`), 쓰기에는 제한이 없다. 약 128단계보다
    깊은 레이아웃을 저장하면 **다시 읽을 수 없는 파일**이 된다 — NaN 비율과 같은 전손 형태다.
    실현하려면 PTY를 128개 띄워 중첩 분할해야 하므로 현실적으로 도달 불가에 가깝다.
    레이아웃 깊이에 상한을 두면 닫힌다.

30. **훅 서버: 16개의 조용한 연결이면 여전히 배지가 막힌다 — 알려진 잔여 한계다.**
    연결당 스레드 + `MAX_CONNECTIONS = 16` + 초과 시 즉시 503으로 고쳤고, 공격 비용이
    연결 1개 → 16개로, 실패 양상이 5초 무응답 정체 → 훅의 1.5초 예산 안에 들어오는 즉시
    503으로, 관측 가능성이 불가 → `refused()` 카운터로 바뀌었다. **그래도 제거된 것은
    아니다** — 테스트가 이 잔여를 덮지 않고 고정한다.

    진짜로 없애려면 **첫 헤더 바이트에 짧은 별도 타임아웃**이 필요하다(정상 curl은 즉시
    쓰고, 놀고 있는 연결은 영영 안 쓴다). 창이 5초 → ~1초로 줄어 16슬롯을 붙들기가
    훨씬 어려워진다. 타임아웃 의미론을 바꾸는 변경이라 이번 브랜치에 넣지 않았다.

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
    (`crates/suaegi-term/src/pty.rs`의 `open_pty_retrying`). **다만 완전히 없어지지는
    않았다** — 전체 스위트 부하에서 드물게 남는 잔여는 위 **4b**에 있다. 프로세스를
    넘나드는 경쟁이라 프로세스 내부 조치의 한계다.

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

## PR4 적대적 리뷰에서 넘긴 것 (이어서)

27. **in-flight 가드가 unwind에 안전하지 않다 — 네 곳 전부에 대한 결정이 필요하다.**
    (21번과 같은 뿌리이고, Plan 4가 같은 모양을 두 개 더 늘렸다.)

    `TerminalSession::resize_lock`이 **`std::sync::Mutex`**이고 `.expect("resize mutex poisoned")`로
    잠근다(`session.rs:88,308`). 그 락 아래에서 패닉이 한 번 나면 뮤텍스가 오염되고, 이후
    모든 `resize()`가 `expect`에서 패닉하고, 스폰된 스레드가 unwind하며 sender가 완료 메시지
    없이 drop되고, 코얼레서의 `in_flight`가 세션이 끝날 때까지 `Some(seq)`로 남는다.
    타임아웃도 재무장도 재시도도 없다 — **PTY는 옛 크기에 갇히고 화면은 새 크기로 그려져
    셸과 화면이 영구히 어긋난다.** 리뷰의 프로브: 이후 198번을 더 리사이즈해도
    `in_flight=Some(1)`.

    `session_store.rs:658-661`의 주석은 "실패해도 완료 메시지를 보내므로 안전하다"고 하는데,
    그건 `Err` 반환만 따진 것이고 **클로저가 unwind하는 경우는 덮지 못한다.**
    `extract_in_flight`도 같은 모양이다(`session_store.rs:622-643`) — 메시지 하나를 잃으면
    그 세션의 복사가 영구히 죽는다.

    `TerminalGrid`는 이미 parking_lot/`FairMutex`를 쓰므로 이 오염 비대칭은 설계가 아니라
    사고다. 선택지: (a) `background::blocking` 클로저를 `catch_unwind`로 감싸고 unwind 가드에서
    완료를 보낸다, (b) `resize_lock`을 parking_lot으로 바꾼다(싸지만 `extract_in_flight`는
    안 고쳐진다), (c) 가드에 타임아웃/재무장을 넣는다.
    **네 개 가드(presence/snapshot/resize/extract) 전체에 대해 한 번에 정하고** 개별
    스팟 픽스를 하지 않는다. 관련: `grid.rs:392`/`:626`의 파서·쓰기 큐 뮤텍스도 std라,
    터미널 출력을 파싱하다 패닉하면 리더 스레드가 죽는다.

## Plan 4에서 실측하고 결정한 것

24. **입력 인코딩의 term 락 경합 — 측정했고, 워커로 옮기지 않기로 했다.**
    Plan 4의 계약(0.3)은 인코딩을 `TerminalGrid`가 term 락을 쥔 채 하도록 정했다.
    "락이 짧다"는 것이 보장이 아니라 가정이라 명시했고, 전용 벤치로 실측했다
    (`crates/suaegi-term/tests/latency_bench.rs`, `#[ignore]` — 타이밍 테스트를 CI에
    상시로 두면 잡음이 된다).

    | 조건 | p50 | p95 | p99 | max |
    |---|---|---|---|---|
    | `encode_key_locked` 무경합 | 83ns | 125ns | 167ns | 1.96µs |
    | `encode_key_locked` 리더 포화(64KiB 청크) | 42ns | 209ns | **1µs** | **2.74ms** |
    | `handle_mouse` 리더 포화 | 42ns | 83ns | **649µs** | **3.46ms** |

    **중앙값은 멀쩡하지만 꼬리가 진짜다.** 최악 ~3.5ms는 최대 크기 청크 한 개의 파싱
    시간이다. 즉 "락은 짧다"는 꼬리에서 거짓이다 — 플랜이 의심한 그대로다.

    그런데도 **옮기지 않는다**: 경합하는 자원이 락 자체라 워커도 같은 시간을 기다린다.
    UI 스레드 밖으로 대기를 옮기는 대신 키 입력마다 홉이 하나 늘고, 입력이 스크롤과
    순서가 뒤바뀔 위험이 생긴다(0.8이 스크롤을 UI 스레드에 둔 이유가 순서 보존이다).
    자식이 최대 속도로 출력을 쏟는 동안에만 3.5ms가 나오는 것은 감수할 만하다고 봤다.

    **재검토 조건**: 실사용에서 타이핑 끊김이 체감되면. 그때는 워커가 아니라 **파서가
    청크를 쪼개 락을 자주 놓게** 하는 쪽을 먼저 본다(경합 자원을 줄이는 쪽).

    대조적으로 `extract_selection`은 10000행 스크롤백 전체 선택에서 평균 **5.8ms**로
    명확해서, 플랜이 이미 정한 워커 배치가 가정이 아니라 실측으로 정당화됐다.

25. **`rustfmt.toml`이 없어 `cargo fmt`가 저장소 전체를 재정렬한다 — 관례 결정 필요.**
    Plan 4 구현 중 한 에이전트가 `cargo fmt -p suaegi-app`을 돌렸다가 import가 크레이트
    전역으로 재정렬돼 손으로 되돌렸다. 원인: 크레이트가 edition 2021이라 rustfmt가
    기본으로 style_edition 2021을 고르는데, 코드 일부는 2024 스타일(소문자 우선
    `{tree, Tree}`)로 쓰여 있다.

    **다만 "저장소가 2024로 통일돼 있다"는 진단은 틀렸다** — 확인해보니 어느 쪽으로도
    통일돼 있지 않다. `style_edition = "2024"`를 넣어도 import 차이가 17곳 남고,
    그 방향이 제각각이다. `rustfmt.toml`이 없는 채로 여러 세션이 각자 써온 결과다.

    그래서 이번에 정하지 않았다. 어느 스타일로 통일할지는 저장소 전체를 한 번에
    건드리는 결정이고, 그 리포맷 커밋은 Plan 4 diff와 섞이면 안 된다.
    **정한 뒤 별도 커밋으로** `rustfmt.toml` 추가 + 전체 리포맷을 한다.
    그때까지는 `cargo fmt`를 크레이트 전체에 돌리지 말고 건드린 파일만
    `rustfmt <file>`로 맞춘다.

## Plan 4로 넘기는 것 (터미널 커스텀 위젯)

Plan 3의 워크벤치(`crates/suaegi-app/src/workbench.rs`)는 읽기 전용 단색
모노스페이스 텍스트로 세션 → 스냅샷 → 구독 → 화면 사슬이 실제로 도는 것만
증명한다. 다음은 전부 Plan 4 몫이다:

10. ~~**색/커서/폰트 속성.**~~ → Plan 4에서 처리(`terminal/render.rs`, `palette.rs`). 스냅샷 셀은 `fg`/`bg`/`flags`(alacritty_terminal의
    `Color`/`Flags`)를 이미 들고 있지만 지금은 버려지고 단색으로만 그린다.

11. ~~**키 입력 → PTY.**~~ → Plan 4에서 처리(`terminal/input.rs`, `suaegi-term/src/encode.rs`). 지금 워크벤치는 완전히 읽기 전용이다. `Widget::update`가
    포커스를 `operation::Focusable`로만 받으므로(`Widget::on_event`가 아니다,
    `canvas`로는 불가능) 커스텀 위젯이 필요하다. `TerminalSession::write`가
    돌려주는 `bool`(입력 유실 여부)을 피드백하는 UI도 이때 같이 들어간다.

12. ~~**마우스(선택/스크롤/마우스 리포팅) + pane_grid와의 합성.**~~ → Plan 4에서 처리. 스파이크로 검증했고 `tests/pane_grid_behavior.rs`가 6개 가정을 고정한다. 터미널 본문이
    마우스 이벤트를 소비해야 하는데 `pane_grid`도 같은 영역에 `on_click`과
    분할 히트테스트를 건다. 이 설계에서 가장 깨지기 쉬운 가정이므로 Plan 4에서
    가장 먼저 스파이크할 것(계획 문서에 이미 명시돼 있다).

13. ~~**리사이즈.**~~ → Plan 4에서 처리. 고정 스폰은 부트스트랩 기본값(50행×80열)으로
    남고 위젯의 첫 레이아웃이 발행하는 `Resize`가 실제 크기로 고친다. pane 크기에 맞춘 실제 리사이즈는 커스텀
    위젯이 크기를 알 수 있어야 가능하다.

26. **위젯 밖으로 나간 마우스 이동은 선택을 더 이상 늘리지 않는다** (`crates/suaegi-app/src/terminal/mouse.rs`).
    실제 터미널은 커서가 창 밖으로 나가도 좌표를 가장자리로 **clamp해서 선택을 계속
    늘린다**. 지금은 `Cursor::position_in` → `hit_test`가 둘 다 `Option`이라 밖으로
    나가는 순간 인텐트가 만들어지지 않고 선택이 그 자리에 멈춘다.

    플랜의 문자 그대로를 구현한 결과이고, clamp 규칙은 플랜에 없다. 없는 규칙을 조용히
    발명하는 대신 **현재 동작을 테스트로 고정**해 뒀다. 익숙한 감각을 원하면 clamp 규칙을
    정해서 넣는다 — 어느 축을 어디로 붙일지(가장 가까운 가장자리 셀), 그리고 위젯 밖
    이동을 계속 받으려면 pane_grid가 그 이벤트를 우리에게 주는지부터 확인해야 한다.

    (관련: release는 밖에서 일어나도 **반드시 발행한다.** 안 하면 그리드의 포인터 래치가
    영영 안 풀려서, 밖에서 손을 뗀 뒤 그냥 마우스를 움직이기만 해도 선택이 계속 늘어난다.
    `resolve_route`가 매칭되는 Release에서만 래치를 푼다. 회귀 테스트 있음.)

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
