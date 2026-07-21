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
