# 추적 중인 후속 항목

리뷰에서 확인됐지만 해당 플랜 범위 밖이라 미룬 것들. 각 항목은 **언제까지** 처리해야
하는지를 함께 적는다.

## 첫 실사용 테스트 세션에서 발견 (MVP 종단 확인)

MVP를 실제로 띄워 사람 눈으로 확인하다가 나온 것들. 헤드리스 하네스로는 **구조적으로**
못 잡는 부류다 — IME·창·실 프로세스가 있어야만 발동한다.

28. ~~**IME 조합창 위치·인라인 렌더링이 최소 구현이다.**~~ → preedit를 현재 터미널
    커서 셀에 on-the-spot으로 직접 그린다. 후보창도 같은 셀의 정확한 사각형을 사용하고,
    IME purpose를 `Terminal`로 전달한다. CJK 전각 문자는 두 칸으로 계산하며 그리드 오른쪽을
    넘는 긴 조합은 xterm처럼 끝부분이 계속 보인다. 정상 경로에서는 런타임 over-the-spot
    문자열을 비워 중복 표시를 막고, 측정이나 커서가 없는 예외 경로에서만 오버레이로
    폴백한다. Commit의 전체 UTF-8 문자열 전달과 자소 분리 방지는 기존 경로를 유지한다.

29. ~~**거부한 권한 요청이 배지를 주황(`Waiting`)에 영구히 남긴다.**~~ → Orca의
    현재 `AGENT_STATUS_STALE_AFTER_MS`와 동일하게 Working/Waiting/Done 훅 상태 모두에
    30분 freshness gate를 적용했다. Claude는 권한 거부 뒤 `Stop`을 보내지 않으므로
    즉시 Done을 추측할 수 없고, 다음 실제 훅이 오기 전에는 Waiting을 유지하는 것이
    정직하다. 다만 한 번 거부한 pane이 영구 주황으로 남지는 않으며, 정확히 30분까지는
    fresh이고 1ms를 넘으면 Unknown으로 감쇠한다. 세션 종료는 30분을 기다리지 않고
    장부를 즉시 제거한다.

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

4. ~~**테스트 타임아웃 하네스**~~ → GitHub Actions의 macOS Rust 잡이
   `cargo-nextest` CI 프로필을 사용한다. 개별 테스트는 60초를 두 번 넘기면 종료되고,
   전체 스위트에는 90분, 잡에는 120분의 상한이 있다. 따라서
   `saturated_write_queue_does_not_stall_the_reader` 같은 회귀도 무한히 매달리지 않고
   실패로 끝난다.

4b. ~~**PTY를 여는 테스트가 전체 스위트 부하에서 여전히 플레이키하다.**~~
   `55c4abd`의 `openpty` 재시도가 대부분을 잡았지만, 동시에 도는 프로세스가 많으면 여전히 난다.
   특징이 일정하다: **전체 스위트에서만, 매번 다른 테스트 이름으로, 재실행하면 통과.**
   관측된 것 — `pty_test` 전반, `session_test`,
   `suaegi-app`의 `state::tests::accepting_a_started_session_registers_it_and_opens_a_pane`,
   `presence_poll::tests::the_guard_clears_when_the_result_arrives_so_the_next_tick_dispatches`.
   마지막 것은 픽스처가 실제 `TerminalSession::start`를 부르므로 같은 원인으로 보이나,
   그 실행의 패닉 메시지를 남기지 못해 **확정은 아니다.**

   Darwin의 `openpty`가 프로세스를 넘나들며 경쟁하므로(자세한 건 `55c4abd`) 프로세스 내부
   조치로는 못 막는다. `.config/nextest.toml`은 macOS의 `suaegi-term`과 실제 세션을 여는
   `suaegi-app` 테스트를 공용 `darwin-pty` 그룹(max-threads=1)에 넣었다. 전체 동시성도 4로
   제한했고, 동일 설정의 최종 전체 실행에서 3797개가 통과하고 6개 opt-in 테스트만 건너뛰었다.

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

5. ~~**`CACHE_REVALIDATE_AFTER` 경계 미테스트**~~ (`crates/suaegi-term/src/presence.rs`)
   → 첫 프로세스 조회 뒤 정확히 20회까지는 캐시된 에이전트를 반환하고, 21번째 히트에서
   같은 pgid를 다시 조회해 셸로 바뀐 상태를 `NoAgent`로 반영하는 경계 테스트를 추가했다.

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

8. ~~**Windows에서 `claude.exe` 미탐지**~~ (`crates/suaegi-term/src/agent.rs`) →
   basename-only 규칙은 유지하고, 공용 `normalized_basename`이 `.exe`/`.cmd`/`.bat`/
   `.ps1`을 대소문자 구분 없이 벗기도록 정리돼 있다. 따라서 Claude만 별칭을 늘리는 대신
   모든 등록 에이전트가 동일한 Windows 규칙을 쓴다. 대문자 `CLAUDE.EXE`가 포함된 전체
   Windows 경로를 Claude로 잡고, 이름이 디렉터리에만 등장하는 경로는 계속 거부하는
   회귀 테스트로 고정했다.

## Plan 5 리뷰에서 결정이 필요해 남긴 것

28. ~~**미래 스키마 거부에 앱 안에서 빠져나갈 길이 없다.**~~ → 사이드바에 영구 경고와
    2단계 확인(`Review save options…` → `Back up & replace`)을 연결했다. 확인 전에는
    기존 가드가 계속 저장을 막고, 확인 후에는 persistence worker의 FIFO에서 가드 해제가
    새 스냅샷보다 먼저 처리된다. `Store::save`는 기존 신버전 본파일을 `bak.0`으로 회전한
    뒤 현재 버전 파일로 교체한다. 워커가 이미 종료됐다면 경고를 해제하지 않고 저장 실패를
    표시한다.

    회귀 테스트는 확인을 열거나 취소한 동안 원본이 한 바이트도 바뀌지 않는 것, 최종 확인
    후 본파일이 현재 스키마로 저장되는 것, 신버전 JSON이 `bak.0`에 그대로 남는 것을 검증한다.

29. ~~**직렬화에는 깊이 제한이 없다(로드에는 있다).**~~ → 저장 가능한 pane 트리 깊이를
    48로 제한했다. serde enum/object 래퍼까지 포함해 reader 재귀 한계에 충분한 여유를 두며,
    48단계는 실제 저장→로드 왕복을 검증한다. 49단계는 JSON 직렬화, 백업 회전, 본파일 교체
    전에 `LayoutTooDeep`으로 거부되어 기존 파일과 백업을 그대로 보존한다.

30. ~~**훅 서버: 16개의 조용한 연결이면 여전히 배지가 막힌다.**~~ → 첫 헤더 바이트에
    500ms 예산을 두고, 헤더+본문 전체에도 기존 5초의 절대 데드라인을 적용했다. 연결 16개를
    모두 잡은 클라이언트가 소켓을 닫지 않아도 슬롯은 curl의 1.5초 예산 안에 자동 회수되며,
    그 뒤 정상 훅이 204와 실제 이벤트를 받는 것을 테스트한다. 바이트를 조금씩 보내 5초
    타임아웃을 매번 재설정하던 전통적 slowloris 경로도 절대 데드라인으로 닫혔다.

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

23. ~~**`flooding_unread_device_queries_does_not_grow_memory_unbounded`가 병렬 실행에서
    플레이키하다**~~ (`crates/suaegi-term/tests/session_test.rs`) → CI를 libtest 바이너리
    단위 병렬 실행에서 nextest의 테스트별 프로세스 실행으로 바꿨다. 이 테스트가 읽는
    프로세스 RSS에는 이제 같은 바이너리의 다른 테스트 할당이 섞이지 않는다. 전역 동시성
    4와 Darwin PTY 직렬 그룹을 적용한 전체 3797개 실행에서 RSS 회귀 테스트도 통과했다.

## PR4 적대적 리뷰에서 넘긴 것 (이어서)

27. ~~**in-flight 가드가 unwind에 안전하지 않다 — 네 곳 전부에 대한 결정이 필요하다.**~~
    → presence/snapshot/resize/extract 네 워커를 공통 panic→completion 경계로 함께
    처리했다. `TerminalSession::resize_lock`도 poison을 회수하도록 바꿔 한 번의 패닉 뒤
    이후 resize가 연쇄 실패하지 않는다. 가드별 실패 해제와 poison 복구를 회귀 테스트로
    고정했다.

    기존 조사:
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

25. ~~**`rustfmt.toml`이 없어 `cargo fmt`가 저장소 전체를 재정렬한다 — 관례 결정 필요.**~~
    Plan 4 구현 중 한 에이전트가 `cargo fmt -p suaegi-app`을 돌렸다가 import가 크레이트
    전역으로 재정렬돼 손으로 되돌렸다. 원인: 크레이트가 edition 2021이라 rustfmt가
    기본으로 style_edition 2021을 고르는데, 코드 일부는 2024 스타일(소문자 우선
    `{tree, Tree}`)로 쓰여 있다.

    **다만 "저장소가 2024로 통일돼 있다"는 진단은 틀렸다** — 확인해보니 어느 쪽으로도
    통일돼 있지 않다. `style_edition = "2024"`를 넣어도 import 차이가 17곳 남고,
    그 방향이 제각각이다. `rustfmt.toml`이 없는 채로 여러 세션이 각자 써온 결과다.

    저장소의 Rust edition과 같은 2021 style edition을 명시하는 `rustfmt.toml`을 추가했다.
    현재 트리 전체가 이 규칙의 `cargo fmt --all -- --check`를 통과하며, CI가 앞으로의
    혼용도 차단한다. 기존 소스 전체를 의미 없이 재작성하지 않고 현재 형식을 기준선으로
    고정했다.

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

26. ~~**위젯 밖으로 나간 마우스 이동은 선택을 더 이상 늘리지 않는다**~~
    (`crates/suaegi-app/src/terminal/mouse.rs`) → 이 위젯 안에서 시작해 아직 버튼을 누르고
    있는 제스처에 한해서, 밖의 포인터를 가장 가까운 행/열 가장자리 셀로 clamp한다. 왼쪽은
    첫 셀의 왼쪽, 오른쪽은 마지막 셀의 오른쪽 선택 경계를 보존하며 위/아래도 첫/마지막
    행으로 붙는다. hover와 휠은 밖에서 계속 무시하므로 다른 pane의 입력을 훔치지 않는다.

    release는 밖에서 일어나도 **반드시 발행한다.** 안 하면 그리드의 포인터 래치가
    영영 안 풀려서, 밖에서 손을 뗀 뒤 그냥 마우스를 움직이기만 해도 선택이 계속 늘어난다.
    이제 사용 가능한 외부 좌표는 같은 가장자리로 clamp하고, OS가 좌표 자체를 잃은 경우만
    마지막으로 알려진 셀을 사용한다. 네 가장자리 motion과 외부 release 회귀 테스트가 있다.

## Plan 5로 넘기는 것

14. ~~**세션 레이아웃 복원.**~~ → `from_load`가 활성 worktree와 `PersistedPane`
    트리를 읽고, hydration 뒤 세션을 재시작해 split 축·비율·활성 pane을 복원한다. 일부
    worktree가 사라졌거나 시작에 실패하면 살아 있는 형제만 승격하며, 일시적 실패가 좋은
    저장 레이아웃을 덮지 않도록 원본을 보존한다. 디스크 왕복과 부분 실패 테스트가 있다.

15. ~~**worktree 메타데이터가 재조회 때마다 유실된다.**~~ → 앱 소유
    `WorktreeMeta`가 생성 에이전트·시각과 연결된 작업 항목을 보존한다. 부팅 시 디스크 값을
    씨딩하고 저장 스냅샷에 재주입하므로 git 재조회나 앱 재시작이 값을 `0`/`None`으로
    되돌리지 않는다. 생성 경로와 load→save 왕복을 테스트한다.

16. ~~**에이전트 상태 3색(working/waiting/done).**~~ → Claude hook, 프로세스 존재,
    OSC title을 pane별 reducer에서 합성해 Unknown/Working/Waiting/Done을 구분한다. 사이드바는
    네 상태를 서로 다른 glyph와 색으로 렌더하고 비정상 종료를 별도 오류 상태로 표시한다.

## Task 8에서 남긴 것

17. ~~**future-schema 저장 가드가 부팅 시점엔 조용하다.**~~ → `from_load`가
    `save_blocked`를 즉시 상태로 올리고 사이드바가 첫 저장 전부터 영구 경고를 렌더한다.
    2단계 `Review save options…` → `Back up & replace` 확인 전에는 원본을 건드리지 않으며,
    승인하면 신버전 파일을 백업한 뒤 현재 스키마로 교체한다.

18. ~~**앱 데이터 파일 위치.**~~ → `docs/port-status.md`의 Local data 절에
    `dirs::config_dir()/suaegi/data.json`(macOS는
    `~/Library/Application Support/suaegi/data.json`)과 기본 worktree 루트
    `~/suaegi-workspaces`가 서로 다른 위치임을 명시했다.

19. ~~**Step 2(종단 흐름) 중 사람이 손으로 확인해야 하는 부분이 남아 있다.**~~ →
    최신 ad-hoc signed macOS 앱을 실제 창으로 검증했다. 복원된 세 worktree PTY가 각자 셸/
    Claude 출력을 유지했고, 같은 worktree의 새 split 두 쪽에 서로 다른 표식을 보냈을 때
    각각 자기 출력만 받았다. 테스트용 터미널 세 개를 연속 닫아도 UI와 RPC가 즉시 응답했다.
    네이티브 빨간 닫기 버튼으로 종료한 뒤 재실행하자 활성 `test-a`, 사용자 작업공간 순서
    `main → agent-test → test-a → test-b`, 기존 PTY 출력과 Claude 세션이 모두 복원됐다.

## Plan 3 최종 리뷰에서 넘긴 것

20. ~~**앱 종료 시 세션 drop이 UI 스레드에서 일어난다.**~~ `AppState`/`SessionStore`가
    보통 경로(`close()` → `Reaper`)를 거치는 건 pane을 하나씩 닫을 때뿐이다.
    창을 닫아 앱이 종료될 때는 `iced::application(...).run()`이 이벤트 루프를
    빠져나오며 `AppState`(그리고 그 안의 `SessionStore` 슬롯들)를 제자리에서
    drop한다 — 스토어가 마지막 클론을 들고 있으면 `Drop for TerminalSession`이
    그 스레드(창/이벤트 루프 스레드)에서 세션당 최대 2초, 슬롯 수만큼 순차로
    실행된다. 창이 멈춰 보이는 건 아니다(이미 닫히는 중이라 아무도 안 본다) —
    종료가 지연될 뿐이다. 하지만 "마지막 drop은 UI 스레드 밖에서" 규칙이 지켜지지
    않는 유일한 경로다.

    → 네이티브 `CloseRequested`를 `WindowClose`로 가로채 첫 요청에서만 최종 스냅샷을
    persistence worker에 보낸다. 완료 대기는 UI 스레드 밖에서 최대 20초로 제한하고, 완료
    또는 타임아웃 뒤 실제 `window::close`를 발행한다. 중복 close는 무시되며 최종 스냅샷이
    대기 중 debounce 저장을 대체하는 worker 테스트와 실제 macOS close/relaunch로 검증했다.

## PR4 적대적 리뷰에서 넘긴 것

21. ~~**백그라운드 클로저 안의 임의 패닉은 여전히 가드를 영영 못 푼다.**~~ →
    네 워커 경로를 공통 `catch_unwind` 경계로 감쌌고 패닉도 완료 메시지로 바꿨다.
    스냅샷 실패는 같은 generation만, presence 실패는 직렬 bool 가드를, resize 실패는
    기존 `ResizeApplied(Err)` 경로를, selection 실패는 조용한 `None` 완료를 사용한다.
    따라서 다음 dirty/tick/resize/copy 요청이 다시 진행된다.

    기존 조사:
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

## Plan 9 M5(안전 파일 write) 리뷰에서 넘긴 것

30. ~~**`FileSignature`(size+mtime)가 same-mtime-tick·same-size 외부 편집을 못 잡는다.**~~
    → 로컬 편집 문서의 signature에 Unix dev/inode/ctime 변경 표식과 SHA-256 내용 지문을
    추가했다. macOS/Linux watcher는 stat-only 변경 표식으로 평상시 파일 내용을 다시 읽지
    않고, 표식을 제공하지 않는 플랫폼만 size/mtime 동률 때 내용을 해시한다. stat 값을
    일부러 같게 만든 테스트에서 watcher가 변경을 발견하고 autosave가
    외부 내용 `bbbb`를 덮지 않는 것을 검증한다. 내용 해시를 제공하지 않는 원격 프로토콜은
    기존 stat 계약을 유지한다.

    기존 조사:
    (`crates/suaegi-git/src/fs.rs`, `FileSignature`/`write_file`). staleness 검사는
    `metadata.len()` + `metadata.modified()`만 비교한다 — 파일시스템 mtime 해상도 안에서
    같은 바이트 수로 외부 편집이 일어나면 지문이 그대로라 `write_file`이 clobber할 수
    있다(Orca 패리티, `editor-autosave-controller.ts`와 같은 한계). 즉시 악용 가능한
    보안 이슈는 아니고 데이터 손실 blind-spot이다. **수정**: `FileSignature`에
    content-hash(또는 inode+ctime)를 추가해 stat 동률일 때도 실제 변경을 감지한다.
    watcher 서브시스템(Plan 9 미포팅)이 붙는 후속 플랜과 함께 보는 게 자연스럽다.

31. ~~**크래시 시 임시 형제가 잔존해 diff 패널에 untracked로 뜬다.**~~ → 편집기 원자
    저장 임시 파일을 예약 이름 `.suaegi-editor-tmp-*.tmp`로 만들고, branch compare의
    untracked 수집에서 접두·접미사와 비어 있지 않은 랜덤 본문이 모두 맞는 파일만 제외한다.
    정상/오류 경로는 tempfile Drop이 계속 정리하고, 크래시 잔존물만 diff 잡음에서 사라진다.

    기존 조사:
    (`crates/suaegi-git/src/fs.rs`, `write_file` step 7). 원자적 쓰기는
    `NamedTempFile::new_in(parent)`로 형제 temp를 만든 뒤 `persist`(rename)한다. 정상
    경로와 실패 경로(`Drop`)는 temp를 정리하지만, **`persist` 직전 프로세스가 크래시하면**
    형제 `.tmpXXXXXX`가 남고, 그게 `branch_compare`의 `status --porcelain` untracked
    수집(`compare.rs`)에 걸려 우리 diff 패널에 뜬다. **수정 후보**: (a) 시작 시 워크트리
    루트의 고아 `.tmp*`를 청소, 또는 (b) untracked 수집에서 `.tmp` 접두 형제를 거른다
    (단, 사용자 실제 파일과 충돌하지 않는 접두 규칙이 필요). LOW — 크래시 창이 매우
    좁고 남은 파일도 무해하다.
