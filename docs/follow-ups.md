# 추적 중인 후속 항목

리뷰에서 확인됐지만 해당 플랜 범위 밖이라 미룬 것들. 각 항목은 **언제까지** 처리해야
하는지를 함께 적는다.

## Plan 3(UI 배선) 시작 전에 처리

이 셋은 지금은 잠복 상태지만, Plan 3가 `suaegi-term`을 UI 루프에 물리는 순간
실제 버그가 된다.

1. **`PtySession::try_wait`가 fire-once다** (`crates/suaegi-term/src/pty.rs`)
   수확이 끝나면(`wait()`가 끝났거나 `try_wait` 자신이 수확했거나) 이후 모든
   `try_wait`는 영원히 `Ok(None)`을 돌려준다. 즉 `Ok(None)`이 "아직 실행 중"과
   "이미 어딘가에서 수확됨" 두 가지를 뜻하게 되어, 폴링하는 쪽에서 오해하기 쉽다.
   수정안: `Lifecycle`에 종료 코드를 담고(`reaped: Option<i32>`) `try_wait`가
   알려진 코드를 돌려주게 한다. 현재 저장소 안에는 영향받는 호출자가 없다.
   → Plan 3의 폴러는 그때까지 `TerminalSession`의 원자값(`exit_code`/`is_running`)을
   쓰고 `PtySession::try_wait`를 직접 쓰지 않는다.

2. **`match_agent`의 경로 세그먼트 과매칭** (`crates/suaegi-term/src/agent.rs`)
   실행 파일 토큰에 대해 basename이 아니라 **모든 경로 세그먼트**를 검사해서,
   `~/code/codex/run.sh`나 `/home/claude/bin/backup.sh`처럼 디렉토리 이름이
   에이전트명과 같은 실행 파일이 에이전트로 오인된다. 세그먼트 매칭이 필요한 건
   런처의 스크립트 인자(`node .../claude-code/cli.js`)뿐이다.
   수정안: 실행 파일 토큰은 basename만, 런처의 두 번째 토큰은 세그먼트 전체.
   영향은 현재 존재 감지 배지가 틀리는 정도이며, 권위 소스는 Plan 5의 hook이다.

3. **`TerminalSession::Drop`의 unix join이 멈출 수 있다** (`crates/suaegi-term/src/session.rs`)
   `killpg(SIGKILL)`은 자식의 프로세스 그룹까지만 닿는다. `setsid()`로 그룹을
   빠져나갔지만 상속받은 슬레이브 FD를 닫지 않은 자손이 있으면 리더가 EOF를 보지
   못해 join이 영원히 대기한다. 코딩 에이전트에서는 드물지만, Plan 3가 Drop을 UI
   스레드에 올리면 UI가 멈춘다.
   수정안: 기한이 있는 join(데드라인 초과 시 detach).

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

## 개발 환경 (코드 아님)

7. **전역 gitconfig의 평문 PAT**
   `/Users/james/projects/james/.gitconfig`의 `url.https://protess:<TOKEN>@github.com/.insteadOf`
   규칙에 토큰이 평문으로 있다. 그냥 지우면 같은 파일이 github.com 헬퍼를 gh(회사
   계정)로 고정해두어 protess 저장소들이 깨진다. 계정 분리 정책(예: 디렉토리별
   `includeIf` + keychain)을 정한 뒤 정리해야 한다.
