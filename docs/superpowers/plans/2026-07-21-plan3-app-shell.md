# Suaegi Plan 3: suaegi-app (iced 앱 셸) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 실행되는 데스크톱 앱. repo를 등록하고, worktree를 만들고, 각 worktree에서 터미널 세션이 돌고, 출력이 화면에 보이고, 에이전트 존재가 사이드바에 뜨고, 상태가 재시작을 넘어 살아남는다.

**Architecture:** iced 0.14 Elm 아키텍처. UI 스레드는 원자값 읽기와 채널 송신만 한다 — 블로킹 작업(프로세스 스폰, `ps`, fsync, canonicalize, 스냅샷, 세션 drop)은 전용 스레드로, git은 `Task::perform`으로 tokio에서 돈다.

**Tech Stack:** iced 0.14 (**features: tokio, canvas, advanced, lazy**), tokio(스트림 페이싱), futures(채널), alacritty_terminal 0.25.1, 기존 `suaegi-core`/`suaegi-git`/`suaegi-term`.

**Spec:** `docs/superpowers/specs/2026-07-20-suaegi-mvp-design.md`
**선행:** Plan 1, Plan 2, term-hardening — 전부 main에 머지 완료

---

## Global Constraints

### iced 0.14 API — 기억과 다른 것들 (vendored 소스로 확인)

각 항목은 컴파일 실패 1회를 뜻한다.

1. **`Subscription::run`은 `fn` 포인터**를 받는다(`iced_futures-0.14.0/src/subscription.rs:182`) — 캡처 불가. 키를 잡으려면 `run_with(data, builder)`를 쓰고 컨텍스트는 전부 `data`로.
2. **`iced::time::every`는 기본 features에 없다.** 0.14 `default`는 `thread-pool` 백엔드이고 그쪽 `time` 모듈은 비어 있다. `tokio` feature 필요.
3. **`suaegi-git`은 tokio 리액터 없이 런타임 패닉한다.** `tokio::process`가 리액터를 요구하는데 thread-pool executor의 `enter`는 no-op다. **컴파일은 통과하고 실행에서 터진다.**
4. **`Widget::on_event`는 없다** — `Widget::update`, `()` 반환, `&Event` 참조. 소비는 `shell.capture_event()`.
5. **`application()`의 첫 인자는 `boot`**. `.run_with(...)`는 존재하지 않는다.
6. `update`는 `()` 반환 가능(`impl From<()> for Task<T>`). `view`는 `&State`를 받는다.
7. `Row::align_items` 없음 → `Row::align_y` / `Column::align_x` (이름의 축이 **교차축**).
8. `iced::stream::channel`은 **`AsyncFnOnce`**: `async move |mut out| { ... }`.
9. `pane_grid::State::panes`는 **공개 필드**, `State::new`는 **`(Self, Pane)`**, `close`는 `Option<(T, Pane)>`로 **`T`를 돌려준다**.
10. `iced_runtime::task::blocking`은 `iced`가 재수출하지 않는다 — 버전 스큐로 `Task`가 갈리는 걸 피해 직접 인라인한다(Task 1).
11. **`std::sync::mpsc::Receiver`는 `Stream`이 아니다** — `Task::stream`에 넣을 수 없다. 스레드 → UI 채널은 `futures::channel::mpsc::unbounded`를 쓴다(워커가 보고하다 막히면 안 되므로 unbounded).

### 스레딩 규칙

- **UI 스레드 허용**: `generation()`, `is_running()`, `exit_code()`, `write()`, `resize()`, `scroll_display()` — 원자값 또는 논블로킹 송신
- **블로킹 스레드**: `TerminalSession::start`(fork/exec), `snapshot()`(락+수백 KB 할당), `PresenceMonitor::probe`(`ps` fork/exec), `Store::save`(fsync), `Repo::from_path`(canonicalize), 그리고 **세션의 마지막 drop**
- **`Task::perform`(tokio)**: `suaegi-git` 전부

### 비동기 결과의 3원칙 (이 플랜에서 가장 자주 깨지는 규칙)

1. **모든 완료 메시지는 발신 맥락을 나른다.** `Result<T,E>`만 담으면 동시 진행 중인 작업이 순서를 바꿔 끝났을 때 엉뚱한 대상에 적용된다. 요청 시 발급한 `OpId`와 대상 식별자(`RepoId`/`WorktreeId`/`SessionId`)를 함께 싣는다. **실패 메시지도 마찬가지다** — 실패했을 때야말로 "무엇이 실패했는지"가 필요하다.
2. **오래된 결과는 버린다.** 반복되는 작업(스냅샷, 존재 판정, worktree 목록)은 요청마다 seq/generation을 실어 보내고, 캐시보다 오래된 결과는 무시한다.
3. **진행 중이면 새로 띄우지 않는다.** in-flight 가드가 없으면 느린 `ps`나 스냅샷이 쌓여 프로세스가 겹치고 락을 두고 경합한다.

### 위험 지점

- **`Drop for TerminalSession`은 UI 스레드를 최대 2초 멈춘다**(`session.rs:42`). **`Arc` 하나를 워커로 옮기는 것으로는 부족하다** — 구독·폴링이 든 클론이 나중에 떨어지면 마지막 파괴는 UI 스레드에서 일어난다. **Reaper**가 필요하고, 여러 세션을 동시에 기다릴 수 있어야 한다(하나가 오래 걸린다고 뒤의 세션이 막히면 안 된다).
- **`snapshot()`은 호출마다 뷰포트 전체를 새로 할당**하고(80×50 ≈ 190KB) 리더 스레드와 같은 `FairMutex`를 두고 경합한다.
- **`TerminalSession::write`의 `bool`을 무시하지 않는다** — `false`면 입력이 유실된 것이고, 피드백이 없으면 사용자는 키가 씹혔다고만 느낀다.

### 범위 경계

- 터미널 렌더링은 **읽기 전용 단색 모노스페이스 텍스트**다. 색/커서/키 입력/포커스/리사이즈를 갖춘 커스텀 위젯은 **Plan 4**. 여기서는 세션 → 스냅샷 → 구독 → 화면의 사슬이 실제로 돈다는 걸 증명하는 게 목적이다.
- 에이전트는 `AgentPresence`(존재 여부)만. `working|waiting|done`은 Plan 5의 hook 서버.
- diff 패널, 세션 레이아웃 복원은 이 플랜에 없다.

### 공통 규칙

- edition 2021, `rust-version = "1.94"`. 파일/모듈 이름에 `utils`/`helpers` 금지
- 최종 게이트: `cargo test --workspace -- --test-threads=4 && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`
- **회귀 테스트는 mutation으로 검증한다.** 이 프로젝트에서 "통과하지만 아무것도 지키지 않는 테스트"가 네 번 나왔고 넷 다 읽어서는 멀쩡해 보였다. 수정을 되돌려 실패하는 걸 확인하지 않은 회귀 테스트는 완료로 치지 않는다
- 이 호스트는 병렬 테스트가 많으면 `openpty: Unknown error: -6`(PTY 풀 고갈)이 난다 → `--test-threads=4`

---

### Task 0: suaegi-core — 미래 스키마 **백업**도 저장을 막는다

**Files:** `crates/suaegi-core/src/persistence.rs`

**왜 Plan 3에 있나:** Plan 3는 이 프로젝트에서 처음으로 실제 사용자 데이터를 **저장**하는 코드다. 현재 `load_from_backups()`는 `parse_trusted`가 거부한 백업(미래 스키마 포함)을 조용히 건너뛰고 다음 슬롯으로 간다. 그래서 **본파일이 손상 + 백업이 더 새 버전**이면 앱은 `LoadSource::Default`로 떨어지면서 가드를 세우지 않고, 이후 저장이 신버전 데이터를 덮어쓴다. 가드는 본파일이 미래 스키마일 때만 선다. 드문 조합이지만 결과가 데이터 손실이라 셸을 올리기 전에 닫는다.

**Interfaces:**
- 동작 변경: 백업을 **미래 스키마 때문에** 거부하면 `future_schema_guard`를 세운다. (손상/파싱 실패로 거부한 경우는 지금처럼 그냥 다음 슬롯으로 — 그건 덮어써도 되는 쓰레기다)
- 공개 API 변경 없음. `future_schema_guarded()`가 더 많은 경우에 `true`가 될 뿐이다

- [ ] **Step 1: 실패하는 테스트** (`persistence.rs`의 tests 모듈에 추가)

```rust
    #[test]
    fn a_future_schema_backup_also_blocks_saves() {
        // 본파일은 손상, 백업은 더 새 버전 — 이 조합에서 저장을 막지 않으면
        // 다음 저장이 신버전 데이터를 덮어쓴다.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("data.json");
        std::fs::write(&file, "{ corrupt").unwrap();
        let mut future = sample_state("newer");
        future.schema_version = SCHEMA_VERSION + 1;
        std::fs::write(
            dir.path().join("data.json.bak.0"),
            serde_json::to_string(&future).unwrap(),
        )
        .unwrap();

        let mut store = Store::new(file);
        let loaded = store.load();
        assert_eq!(loaded.source, LoadSource::Default, "a future backup is not usable data");
        assert!(
            store.future_schema_guarded(),
            "a future-schema backup must block saves, or we overwrite newer data"
        );
        assert!(matches!(
            store.save(&PersistedState::default()),
            Err(PersistenceError::FutureSchemaGuard)
        ));
    }

    #[test]
    fn a_merely_corrupt_backup_does_not_block_saves() {
        // 쓰레기 백업 때문에 저장이 막히면 사용자는 아무것도 못 한다
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("data.json");
        std::fs::write(&file, "{ corrupt").unwrap();
        std::fs::write(dir.path().join("data.json.bak.0"), "also garbage").unwrap();

        let mut store = Store::new(file);
        store.load();
        assert!(!store.future_schema_guarded());
        assert!(store.save(&PersistedState::default()).is_ok());
    }
```

- [ ] **Step 2: 실패 확인** → `cargo test -p suaegi-core`

- [ ] **Step 3: 구현**

`load_from_backups`가 `parse_trusted`의 **거부 사유를 구분**하게 한다. `parse_trusted`는 이미 `Err(true)`=미래 스키마 / `Err(false)`=손상을 돌려주므로, 백업 루프에서 `Err(true)`를 만나면 `self.future_schema_guard = true`로 세우고 계속 다음 슬롯을 본다(더 오래된 백업 중에 쓸 수 있는 게 있을 수 있다).

- [ ] **Step 4: 통과 확인 + 회귀 없음**

Run: `cargo test --workspace -- --test-threads=4 && cargo clippy --workspace --all-targets -- -D warnings`
기존 미래 스키마 테스트(`future_schema_blocks_saves_until_overridden`)가 그대로 통과해야 한다.

- [ ] **Step 5: Commit**

```bash
git add crates/suaegi-core/src/persistence.rs
git commit -m "fix(core): block saves when a backup was rejected for a future schema"
```

`docs/follow-ups.md`의 항목 9를 완료로 표시한다.

---

### Task 1: 크레이트 스캐폴드 + 블로킹 브리지

**Files:** `Cargo.toml`(수정: `members`에 `"crates/suaegi-app"` 추가), `crates/suaegi-app/{Cargo.toml, src/lib.rs, src/main.rs, src/background.rs, src/state.rs}`

**Interfaces:**
- 실행되는 빈 iced 앱
- `background::blocking<T: Send + 'static>(f: impl FnOnce(mpsc::Sender<T>) + Send + 'static) -> Task<T>`
- **`src/lib.rs`가 필요하다** — 통합 테스트가 `suaegi_app::...`를 import하므로 바이너리만으로는 태스크별로 컴파일되지 않는다. `main.rs`는 `lib`를 부르는 얇은 바이너리로 둔다
- 뒤 태스크들이 참조할 공용 타입을 **여기서 미리 만든다**(뒤로 미루면 Task 3~7이 컴파일되지 않는다):
  ```rust
  /// 비동기 작업 하나를 식별한다. 결과가 순서를 바꿔 도착해도 대상을 잃지 않게 한다.
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
  pub struct OpId(pub u64);

  #[derive(Debug, Clone)]
  pub enum Message { /* 태스크마다 변형을 추가한다 */ }

  #[derive(Default)]
  pub struct AppState { /* 태스크마다 필드를 추가한다 */ }
  ```

- [ ] **Step 1: 매니페스트**

workspace `[workspace.dependencies]`에:
```toml
iced = { version = "0.14", features = ["tokio", "canvas", "advanced", "lazy"] }
futures = "0.3"
```

`crates/suaegi-app/Cargo.toml`:
```toml
[package]
name = "suaegi-app"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
suaegi-core = { path = "../suaegi-core" }
suaegi-git = { path = "../suaegi-git" }
suaegi-term = { path = "../suaegi-term" }
# 스냅샷 셀이 Color/Flags/CursorShape를 재수출 없이 노출하므로 직접 의존한다
alacritty_terminal = { workspace = true }
iced = { workspace = true }
futures = { workspace = true }
# 세션 스트림 페이싱에 tokio::time::sleep이 필요하다 (std::thread::sleep은 async 워커를 막는다)
tokio = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```

**`tokio` feature는 선택이 아니다** — 빼면 `time::every`가 없고(컴파일 실패) `suaegi-git`이 실행 중 패닉한다(컴파일 통과).

- [ ] **Step 2: 실패하는 테스트** (`src/background.rs` 하단)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc as std_mpsc;
    use std::time::Duration;

    #[test]
    fn blocking_body_runs_off_the_calling_thread() {
        let (tx, rx) = std_mpsc::channel();
        let caller = std::thread::current().id();
        let _task = blocking(move |_out: futures::channel::mpsc::Sender<()>| {
            tx.send(std::thread::current().id()).unwrap();
        });
        let ran_on = rx.recv_timeout(Duration::from_secs(5)).expect("body ran");
        assert_ne!(ran_on, caller, "blocking body must not run on the caller thread");
    }
}
```

- [ ] **Step 3: 구현** (`src/background.rs`)

```rust
use futures::channel::mpsc;
use iced::Task;

/// 블로킹 작업을 전용 OS 스레드에서 돌리고 결과를 메시지 스트림으로 돌려준다.
///
/// `iced_runtime::task::blocking`과 같지만 직접 들고 있는다: `iced`는 이걸
/// 재수출하지 않고, `iced_runtime`을 따로 의존하면 버전이 어긋났을 때 서로
/// 호환되지 않는 `Task` 타입이 두 개 생긴다.
pub fn blocking<T>(f: impl FnOnce(mpsc::Sender<T>) + Send + 'static) -> Task<T>
where
    T: Send + 'static,
{
    let (sender, receiver) = mpsc::channel(1);
    std::thread::spawn(move || f(sender));
    Task::stream(receiver)
}
```

- [ ] **Step 4: 최소 앱**

`src/lib.rs` — 라이브러리가 본체다 (통합 테스트가 여기를 import한다):
```rust
pub mod background;
pub mod state;

use iced::widget::{center, text};
use iced::{Element, Size};

pub use state::{AppState, Message, OpId};

impl AppState {
    pub fn new() -> Self { Self::default() }
    pub fn title(&self) -> String { "Suaegi".to_string() }
    pub fn update(&mut self, _message: Message) {}
    pub fn view(&self) -> Element<'_, Message> { center(text("Suaegi")).into() }
}

pub fn run() -> iced::Result {
    iced::application(AppState::new, AppState::update, AppState::view)
        .title(AppState::title)
        .window_size(Size { width: 1280.0, height: 800.0 })
        .run()
}
```

`src/main.rs` — 얇은 바이너리:
```rust
fn main() -> iced::Result {
    suaegi_app::run()
}
```

`src/state.rs` — 공용 타입 (뒤 태스크들이 확장한다):
```rust
/// 비동기 작업 하나를 식별한다. 결과가 순서를 바꿔 도착해도 대상을 잃지 않게 한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OpId(pub u64);

#[derive(Debug, Clone)]
pub enum Message {}

#[derive(Default)]
pub struct AppState {}
```

- [ ] **Step 5: 검증 + Commit**

Run: `cargo build --workspace && cargo test -p suaegi-app && cargo clippy --workspace --all-targets -- -D warnings`
추가로 `cargo run -p suaegi-app`으로 창이 뜨는지 눈으로 확인하고 리포트에 적는다(자동 테스트로 못 잡는다).

```bash
git add Cargo.toml Cargo.lock crates/suaegi-app
git commit -m "chore: scaffold suaegi-app with a blocking-work bridge"
```

---

### Task 2: 영속화 스레드

**Files:** `crates/suaegi-app/src/persistence_thread.rs`, `tests/persistence_thread_test.rs`

**Interfaces:**
```rust
pub struct PersistenceBoot {
    pub handle: PersistenceHandle,
    pub load: LoadDiagnostics,
    /// futures 채널이어야 Task::stream에 넣을 수 있다 (std mpsc는 Stream이 아니다).
    /// unbounded: 워커가 결과를 보고하다 막히면 안 된다.
    pub results: futures::channel::mpsc::UnboundedReceiver<SaveReport>,
}

pub struct LoadDiagnostics {
    pub state: PersistedState,
    /// 데이터가 어디서 왔는지 — 신규 설치와 복구 실패를 구분한다.
    /// LoadSource만으로는 둘 다 Default라 UI가 헛경고를 띄우게 된다.
    pub origin: LoadOrigin,
    pub save_blocked: bool,   // Store::future_schema_guarded() (public)
}

pub enum LoadOrigin {
    /// 본파일도 백업도 **하나도 없었다** — 신규 설치. 경고 금지.
    Fresh,
    Loaded,                   // 본파일 정상
    Recovered { slot: usize },// 백업에서 복구 — 사용자에게 알린다
    /// 뭔가는 있었는데 쓸 수 있는 게 없었다 — 강하게 알린다.
    /// (본파일이 없고 백업만 있는데 그 백업들이 다 깨진 경우도 여기다)
    RecoveryFailed,
}

/// 저장 요청 하나의 최종 상태. **모든 seq는 정확히 한 번 보고된다** —
/// debounce로 대체된 요청도 조용히 사라지지 않고 Superseded로 보고한다.
pub struct SaveReport { pub seq: u64, pub status: SaveStatus }

pub enum SaveStatus {
    Written,
    SkippedUnchanged,
    /// 더 새 요청이 들어와 이 요청은 쓰이지 않았다. 실패가 아니다.
    Superseded { by: u64 },
    Failed(String),
}

impl PersistenceHandle {
    pub fn spawn(data_file: PathBuf) -> PersistenceBoot;
    /// 논블로킹. 발급한 seq를 반환한다. 워커가 죽어 송신이 실패하면
    /// 핸들이 직접 SaveReport{seq, Failed(..)}를 결과 채널로 흘려보낸다 —
    /// 죽은 워커는 자기 죽음을 보고할 수 없다.
    pub fn save(&self, state: PersistedState) -> u64;
    pub fn override_future_schema_guard(&self);
}
```
- 워커는 **debounce**한다: 마지막 요청 후 300ms 조용하면 저장. 중복 내용은 `Store`의 해시 비교가 `SkippedUnchanged`로 거른다
- `Drop`은 밀린 저장을 flush하고 스레드를 join한다 — 종료 시 마지막 변경이 유실되면 안 된다
- 앱은 부팅 시 `results`를 `Task::stream(...).map(Message::Saved)`로 한 번 배선한다

- [ ] **Step 1: 실패하는 테스트** (`tests/persistence_thread_test.rs`)

```rust
use suaegi_app::persistence_thread::{LoadOrigin, PersistenceHandle, SaveReport, SaveStatus};
use suaegi_core::domain::{PersistedState, Repo, RepoId, SCHEMA_VERSION};

fn state_with(name: &str) -> PersistedState {
    let mut s = PersistedState::default();
    s.repos.push(Repo {
        id: RepoId(format!("/tmp/{name}")),
        path: std::path::PathBuf::from(format!("/tmp/{name}")),
        display_name: name.into(),
        worktree_base_ref: None,
    });
    s
}

/// 결과 채널을 끝까지 읽어 모은다 (핸들 drop 후 호출 — 그때 채널이 닫힌다).
fn drain(rx: futures::channel::mpsc::UnboundedReceiver<SaveReport>) -> Vec<SaveReport> {
    futures::executor::block_on(futures::StreamExt::collect::<Vec<_>>(rx))
}

#[test]
fn a_missing_data_file_is_fresh_not_a_recovery_failure() {
    // 신규 설치에서 "데이터 손실" 경고를 띄우면 안 된다
    let dir = tempfile::tempdir().unwrap();
    let boot = PersistenceHandle::spawn(dir.path().join("data.json"));
    assert!(matches!(boot.load.origin, LoadOrigin::Fresh));
    assert!(!boot.load.save_blocked);
}

#[test]
fn a_corrupt_file_with_no_backup_reports_recovery_failure() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("data.json");
    std::fs::write(&file, "{ not json").unwrap();
    let boot = PersistenceHandle::spawn(file);
    assert!(matches!(boot.load.origin, LoadOrigin::RecoveryFailed));
}

#[test]
fn a_missing_main_file_with_corrupt_backups_is_not_fresh() {
    // 본파일만 보고 판단하면 이 경우가 Fresh로 오분류되고, 실제로는 데이터를
    // 잃은 사용자에게 아무 경고도 뜨지 않는다.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("data.json");
    std::fs::write(dir.path().join("data.json.bak.0"), "{ corrupt").unwrap();
    let boot = PersistenceHandle::spawn(file);
    assert!(matches!(boot.load.origin, LoadOrigin::RecoveryFailed));
}

#[test]
fn saves_land_on_disk_and_survive_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("data.json");
    let boot = PersistenceHandle::spawn(file.clone());
    boot.handle.save(state_with("alpha"));
    drop(boot.handle); // flush + join

    let again = PersistenceHandle::spawn(file);
    assert!(matches!(again.load.origin, LoadOrigin::Loaded));
    assert_eq!(again.load.state.repos[0].display_name, "alpha");
}

#[test]
fn rapid_saves_are_debounced_into_a_single_write() {
    // "마지막 상태가 파일에 있다"만 보면 50번 전부 fsync해도 통과한다.
    // debounce를 검증하려면 실제 쓰기 횟수를 세야 한다.
    let dir = tempfile::tempdir().unwrap();
    let boot = PersistenceHandle::spawn(dir.path().join("data.json"));
    for i in 0..50 {
        boot.handle.save(state_with(&format!("s{i}")));
    }
    drop(boot.handle);
    let reports = drain(boot.results);
    let written = reports.iter().filter(|r| matches!(r.status, SaveStatus::Written)).count();
    assert_eq!(written, 1, "50 rapid saves must collapse into one write, got {written}");
}

#[test]
fn every_issued_seq_is_reported_exactly_once() {
    // debounce로 대체된 요청도 조용히 사라지면 안 된다 — Superseded로 답이 와야
    // 호출자가 "이 저장은 어떻게 됐나"를 항상 알 수 있다.
    let dir = tempfile::tempdir().unwrap();
    let boot = PersistenceHandle::spawn(dir.path().join("data.json"));
    let seqs: Vec<u64> = (0..10).map(|i| boot.handle.save(state_with(&format!("s{i}")))).collect();
    drop(boot.handle);

    let reports = drain(boot.results);
    for seq in &seqs {
        let n = reports.iter().filter(|r| r.seq == *seq).count();
        assert_eq!(n, 1, "seq {seq} must be reported exactly once, got {n}");
    }
    let superseded = reports.iter().filter(|r| matches!(r.status, SaveStatus::Superseded { .. })).count();
    assert_eq!(superseded, 9, "nine of ten were replaced before they could be written");
}

#[test]
fn a_future_schema_file_blocks_saves_visibly() {
    // 더 새 버전이 쓴 데이터를 만나면 저장이 막힌다. 그 사실이 UI에 보이지 않으면
    // 사용자는 변경이 사라지는 이유를 알 방법이 없다.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("data.json");
    let mut future = state_with("from-the-future");
    future.schema_version = SCHEMA_VERSION + 1;
    std::fs::write(&file, serde_json::to_string(&future).unwrap()).unwrap();

    let boot = PersistenceHandle::spawn(file);
    assert!(boot.load.save_blocked);
    let seq = boot.handle.save(state_with("attempt"));
    drop(boot.handle);
    let reports = drain(boot.results);
    let blocked = reports.iter().find(|r| r.seq == seq).expect("report for the blocked save");
    assert!(matches!(blocked.status, SaveStatus::Failed(_)),
            "a blocked save must report Failed, not silence");
}
```

`[dev-dependencies]`에 `futures = { workspace = true }` 추가.

- [ ] **Step 2: 실패 확인** → `cargo test -p suaegi-app --test persistence_thread_test`

- [ ] **Step 3: 구현** (`src/persistence_thread.rs`)

```rust
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self as std_mpsc, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use futures::channel::mpsc as fmpsc;
use suaegi_core::domain::PersistedState;
use suaegi_core::persistence::{LoadSource, SaveOutcome, Store};

/// 마지막 요청 후 이만큼 조용하면 실제로 쓴다.
const DEBOUNCE: Duration = Duration::from_millis(300);

enum Request {
    Save { seq: u64, state: Box<PersistedState> },
    OverrideFutureSchemaGuard,
}

pub struct PersistenceHandle {
    tx: Option<Sender<Request>>,
    /// 워커가 죽으면 워커는 자기 죽음을 보고할 수 없다 — 핸들이 대신 보고한다.
    results: fmpsc::UnboundedSender<SaveReport>,
    next_seq: AtomicU64,
    thread: Option<JoinHandle<()>>,
}

impl PersistenceHandle {
    pub fn spawn(data_file: PathBuf) -> PersistenceBoot {
        // Store::new가 경로를 가져가므로 존재 확인용 사본을 먼저 만든다
        let probe_path = data_file.clone();
        let mut store = Store::new(data_file);
        // 부팅 시 1회 동기 로드 — 창이 뜨기 전이라 UI를 막지 않는다
        // Default는 "아무것도 없었다"와 "있었는데 다 못 읽었다" 둘 다를 뜻하므로
        // **로드 전에** 본파일과 백업 슬롯의 존재 여부를 직접 확인해 구분한다.
        // 본파일만 보면 "본파일 없음 + 깨진 백업들" 이 Fresh로 오분류된다.
        let any_persistence_existed = probe_path.exists()
            || (0..5).any(|slot| backup_path(&probe_path, slot).exists());
        let outcome = store.load();
        let origin = match &outcome.source {
            LoadSource::MainFile => LoadOrigin::Loaded,
            LoadSource::Backup(slot) => LoadOrigin::Recovered { slot: *slot },
            LoadSource::Default if any_persistence_existed => LoadOrigin::RecoveryFailed,
            LoadSource::Default => LoadOrigin::Fresh,
        };
        let load = LoadDiagnostics {
            state: outcome.state,
            origin,
            save_blocked: store.future_schema_guarded(),
        };

        let (req_tx, req_rx) = std_mpsc::channel::<Request>();
        let (res_tx, res_rx) = fmpsc::unbounded::<SaveReport>();
        let worker_res_tx = res_tx.clone();

        let thread = std::thread::Builder::new()
            .name("suaegi-persistence".into())
            .spawn(move || {
                let mut pending: Option<(u64, Box<PersistedState>)> = None;
                loop {
                    let timeout = if pending.is_some() { DEBOUNCE } else { Duration::from_secs(3600) };
                    match req_rx.recv_timeout(timeout) {
                        Ok(Request::Save { seq, state }) => {
                            // 대체되는 요청도 조용히 사라지면 안 된다 —
                            // 모든 seq는 정확히 한 번 답을 받는다
                            if let Some((old_seq, _)) = pending.take() {
                                let _ = worker_res_tx.unbounded_send(SaveReport {
                                    seq: old_seq,
                                    status: SaveStatus::Superseded { by: seq },
                                });
                            }
                            pending = Some((seq, state));
                        }
                        Ok(Request::OverrideFutureSchemaGuard) => store.override_future_schema_guard(),
                        Err(RecvTimeoutError::Timeout) => {
                            if let Some((seq, state)) = pending.take() {
                                let status = match store.save(&state) {
                                    Ok(SaveOutcome::Written) => SaveStatus::Written,
                                    Ok(SaveOutcome::SkippedUnchanged) => SaveStatus::SkippedUnchanged,
                                    Err(e) => SaveStatus::Failed(e.to_string()),
                                };
                                let _ = worker_res_tx.unbounded_send(SaveReport { seq, status });
                            }
                        }
                        // 핸들이 사라졌다 — 밀린 저장을 flush하고 끝낸다
                        Err(RecvTimeoutError::Disconnected) => {
                            if let Some((seq, state)) = pending.take() {
                                let status = match store.save(&state) {
                                    Ok(SaveOutcome::Written) => SaveStatus::Written,
                                    Ok(SaveOutcome::SkippedUnchanged) => SaveStatus::SkippedUnchanged,
                                    Err(e) => SaveStatus::Failed(e.to_string()),
                                };
                                let _ = worker_res_tx.unbounded_send(SaveReport { seq, status });
                            }
                            break;
                        }
                    }
                }
            })
            .expect("spawn persistence thread");

        PersistenceBoot {
            handle: PersistenceHandle {
                tx: Some(req_tx),
                results: res_tx,
                next_seq: AtomicU64::new(1),
                thread: Some(thread),
            },
            load,
            results: res_rx,
        }
    }

    pub fn save(&self, state: PersistedState) -> u64 {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        let sent = self
            .tx
            .as_ref()
            .map(|tx| tx.send(Request::Save { seq, state: Box::new(state) }).is_ok())
            .unwrap_or(false);
        if !sent {
            // 워커가 죽었다 — 삼키지 않는다
            let _ = self.results.unbounded_send(SaveReport {
                seq,
                status: SaveStatus::Failed("persistence worker is gone".to_string()),
            });
        }
        seq
    }

    pub fn override_future_schema_guard(&self) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Request::OverrideFutureSchemaGuard);
        }
    }
}

impl Drop for PersistenceHandle {
    fn drop(&mut self) {
        self.tx.take();                      // 워커가 Disconnected를 보게 한다
        if let Some(t) = self.thread.take() {
            let _ = t.join();                // flush 대기 — 저장 하나는 수십 ms
        }
    }
}
```

**주의:** `backup_path(&data_file, slot)`은 `Store`가 쓰는 것과 같은 규칙(`<name>.bak.<slot>`)이어야 한다. `Store`가 이 경로 계산을 공개하지 않으므로 여기서 같은 규칙을 다시 쓰되, **두 곳이 갈라지면 오분류가 되므로** 주석에 그 결합을 명시한다. 더 나은 방법이 있으면(예: `suaegi-core`에 `backup_paths()`를 노출) 그렇게 하고 리포트에 적는다.

**debounce와 seq 보고의 관계:** 워커가 밀린 요청을 새 요청으로 대체할 때, **대체된 요청의 seq를 즉시 `Superseded { by }`로 보고한다.** 그러지 않으면 50번 저장했을 때 49개의 seq가 영원히 답을 못 받아 "모든 seq는 정확히 한 번 보고된다"는 계약이 깨진다.

- [ ] **Step 4~5: 통과 확인 + Commit**

```bash
git commit -m "feat(app): persistence thread with debounced saves and load-origin diagnostics"
```

---

### Task 3: repo 등록 + worktree CRUD

**Files:** `crates/suaegi-app/src/git_tasks.rs`, `tests/git_tasks_test.rs`, `tests/fixture/mod.rs`

**Interfaces:**
- **2층 구조** (테스트 가능성을 위해):
  - `*_now(...) -> Result<T, String>` — 실제 작업 (테스트 대상)
  - `*(...) -> Task<Message>` — 얇은 래퍼 (테스트 불가)
- **repo 등록은 2단계 Task다** — 한 `Task::perform`에 둘 다 넣으면 canonicalize가 tokio 워커를 막는다:
  ```rust
  /// 1단계(블로킹): canonicalize + Repo 구성. `Repo::from_path`가 canonicalize를
  /// 하므로 **여기서만** 부른다 — 2단계에서 다시 부르면 tokio 워커가 막힌다.
  pub fn build_repo_now(path: PathBuf) -> Result<Repo, String>;
  /// 2단계(tokio): 이미 만들어진 Repo로 git probe. Repo를 다시 만들지 않는다.
  pub async fn probe_repo_now(repo: Repo) -> Result<(Repo, Option<String>), String>;
  /// 둘을 합성: blocking → then(perform)
  pub fn add_repo(request: OpId, path: PathBuf) -> Task<Message>;
  ```
- 완료 메시지는 **전부 맥락을 나른다** (실패 경로 포함):
  ```rust
  RepoProbed { request: OpId, requested_path: PathBuf, result: Result<(Repo, Option<String>), String> },
  WorktreesListed { request: OpId, repo_id: RepoId, result: Result<Vec<WorktreeEntry>, String> },
  WorktreeCreated { request: OpId, repo_id: RepoId, result: Result<CreatedWorktree, String> },
  WorktreeRemoved { request: OpId, repo_id: RepoId, worktree_id: WorktreeId, result: Result<RemoveOutcome, String> },
  ```
  `WorktreesListed`에도 `OpId`가 있어야 한다 — 생성/삭제 직후 다시 목록을 부르면 앞선 목록이 뒤에 도착해 최신 결과를 덮을 수 있다. 앱은 repo별 `latest_list_op`를 들고 그보다 오래된 결과를 버린다. **이 규칙은 앱 상태의 순수 함수로 분리해 테스트한다**:
  ```rust
  #[test]
  fn an_out_of_order_worktree_listing_is_discarded() {
      let mut state = AppState::default();
      let repo = RepoId("/tmp/r".into());
      state.note_list_issued(repo.clone(), OpId(2));
      state.apply_worktree_listing(repo.clone(), OpId(2), vec![entry("new")]);
      // 앞서 발급된 목록이 뒤늦게 도착
      state.apply_worktree_listing(repo.clone(), OpId(1), vec![entry("old")]);
      assert_eq!(state.worktree_names(&repo), vec!["new"], "a stale listing must not win");
  }
  ```
- `suaegi-git`은 참조를 받으므로 `async move` 진입 전에 소유 값(`PathBuf`/`String`)을 만든다

- [ ] **Step 1: 실패하는 테스트** (`tests/git_tasks_test.rs`)

`tests/fixture/mod.rs`는 `crates/suaegi-git/tests/fixture/mod.rs`를 복사한다(격리된 git 설정 포함).

```rust
mod fixture;

use suaegi_app::git_tasks::{
    build_repo_now, create_worktree_now, list_worktrees_now, probe_repo_now, remove_worktree_now,
};

#[tokio::test]
async fn probe_rejects_a_non_repo_with_a_readable_error() {
    let dir = tempfile::tempdir().unwrap();
    let repo = build_repo_now(dir.path().to_path_buf()).unwrap();
    let err = probe_repo_now(repo).await.unwrap_err();
    assert!(!err.is_empty());
}

#[tokio::test]
async fn probe_keeps_the_detected_head_branch() {
    let dir = tempfile::tempdir().unwrap();
    fixture::init_repo(dir.path());
    let repo = build_repo_now(dir.path().to_path_buf()).unwrap();
    let (repo, head) = probe_repo_now(repo).await.unwrap();
    assert_eq!(head.as_deref(), Some("main"), "the head branch must not be dropped");
    assert!(repo.path.is_absolute());
}

#[tokio::test]
async fn create_then_list_contains_exactly_the_new_worktree() {
    let repo_dir = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    fixture::init_repo(repo_dir.path());
    let repo = build_repo_now(repo_dir.path().to_path_buf()).unwrap();
    let (repo, _) = probe_repo_now(repo).await.unwrap();

    let created = create_worktree_now(
        repo.clone(), "feature one".into(), "main".into(), ws.path().to_path_buf(),
    ).await.unwrap();
    assert_eq!(created.branch, "feature-one");

    // len()만 보면 main worktree가 두 번 나와도 통과한다 — 경로와 브랜치로 확인한다
    let list = list_worktrees_now(repo).await.unwrap();
    let matched: Vec<_> = list.iter().filter(|e| {
        e.branch.as_deref() == Some(created.branch.as_str())
            && e.path.canonicalize().ok() == created.path.canonicalize().ok()
    }).collect();
    assert_eq!(matched.len(), 1, "exactly one entry must be the worktree we created");
}

#[tokio::test]
async fn remove_takes_the_worktree_out_of_the_listing() {
    let repo_dir = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    fixture::init_repo(repo_dir.path());
    let repo = build_repo_now(repo_dir.path().to_path_buf()).unwrap();
    let (repo, _) = probe_repo_now(repo).await.unwrap();

    let created = create_worktree_now(
        repo.clone(), "doomed".into(), "main".into(), ws.path().to_path_buf(),
    ).await.unwrap();
    remove_worktree_now(repo.clone(), created.path.clone(), false, Some(created.branch.clone()))
        .await
        .unwrap();

    let list = list_worktrees_now(repo).await.unwrap();
    assert!(
        !list.iter().any(|e| e.branch.as_deref() == Some(created.branch.as_str())),
        "the removed worktree must be gone from the listing"
    );
    assert!(!created.path.exists());
}

#[tokio::test]
async fn a_bad_base_ref_surfaces_as_an_error_not_a_panic() {
    let repo_dir = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    fixture::init_repo(repo_dir.path());
    let repo = build_repo_now(repo_dir.path().to_path_buf()).unwrap();
    let (repo, _) = probe_repo_now(repo).await.unwrap();
    let err = create_worktree_now(repo, "x".into(), "no-such-ref".into(), ws.path().to_path_buf())
        .await.unwrap_err();
    assert!(!err.is_empty());
}
```

`[dev-dependencies]`에 `tokio = { workspace = true }` 추가.

- [ ] **Step 2~5**: 구현, 통과 확인, 커밋 (`feat(app): repo probing and worktree CRUD with operation-scoped results`)

---

### Task 4: 사이드바 UI

**Files:** `crates/suaegi-app/src/sidebar.rs`

**Interfaces:** `sidebar::view(state: &AppState) -> Element<'_, Message>` — repo 그룹, worktree 목록, 선택 표시, repo 추가/worktree 생성·삭제, worktree 행의 존재 배지 자리(Task 7에서 채움)

- 레이아웃: `row![sidebar.width(Length::Fixed(260.0)), workbench.width(Length::Fill)]`
- **사이드바를 pane으로 만들지 않는다**: `pane_grid`는 고정 폭 pane이 없고(비율만), 사이드바가 터미널 격자 한가운데로 드래그될 수 있다

- [ ] **Step 1: 순수 로직 테스트** (`Element`는 검사 불가 → 뷰가 쓰는 헬퍼를 테스트)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_rows_group_under_their_repo_in_a_stable_order() { /* ... */ }

    #[test]
    fn a_worktree_whose_repo_is_gone_is_skipped_without_panicking() {
        // 영속화된 worktree가 삭제된 repo를 가리키는 경우
    }

    #[test]
    fn status_line_text_distinguishes_fresh_install_from_recovery_failure() {
        // LoadOrigin::Fresh는 경고를 띄우지 않고, RecoveryFailed는 띄운다
        assert!(status_line(&AppState::fresh()).is_none());
        assert!(status_line(&AppState::recovery_failed()).is_some());
        assert!(status_line(&AppState::recovered(0)).is_some());
    }

    #[test]
    fn a_failed_save_is_visible_in_the_status_line() {
        assert!(status_line(&AppState::with_save_error("disk full")).unwrap().contains("disk full"));
    }
}
```

- [ ] **Step 2~5**: 구현, 통과 확인, 커밋 (`feat(app): sidebar with repo groups, worktree rows, and status line`)

눈으로 확인: repo 추가 → worktree 생성 → 삭제 흐름. 리포트에 적는다.

---

### Task 5: 세션 생명주기 + 스냅샷 캐시 + Reaper

**Files:** `crates/suaegi-app/src/session_store.rs`, `src/reaper.rs`, `tests/session_store_test.rs`

**Interfaces:**
```rust
pub struct SessionId(pub u64);

pub struct SessionSlot {
    pub id: SessionId,
    pub worktree_id: WorktreeId,
    pub session: Arc<TerminalSession>,
    pub snapshot: TerminalSnapshot,
    pub snapshot_generation: u64,
    /// 진행 중인 스냅샷 요청이 **어느 generation을 뜨는 중인지**. bool로는
    /// 엉뚱한 결과가 남의 가드를 풀어 동시 스냅샷이 생긴다.
    pub snapshot_in_flight: Option<u64>,
    pub presence: AgentPresence,
    pub presence_in_flight: bool,
    /// 세션마다 하나 — 틱마다 새로 만들면 pgid 캐시가 죽어 매번 ps를 띄운다
    pub monitor: Arc<Mutex<PresenceMonitor>>,
}

impl SessionStore {
    /// id를 호출자가 미리 발급해 넘긴다 — 동시에 시작한 세션들이 순서를 바꿔
    /// 끝났을 때 어느 슬롯에 넣을지 알 수 있어야 한다.
    pub fn start(&mut self, id: SessionId, worktree: &Worktree, agent: AgentKind,
                 prompt: Option<String>) -> Task<Message>;
    /// 스냅샷은 UI 스레드에서 뜨지 않는다. 이미 진행 중이면 false를 반환하고 띄우지 않는다.
    pub fn request_snapshot(&mut self, id: SessionId, generation: u64) -> (bool, Task<Message>);
    /// 도착한 결과를 반영한다:
    /// - 캐시보다 오래된 generation이면 **버린다**
    /// - 가드는 **자기 요청의 결과일 때만** 푼다 (`in_flight == Some(generation)`)
    /// - 푼 직후 `session.generation()`이 이미 더 나아가 있으면 **곧바로 다음 요청을 낸다** —
    ///   그러지 않으면 스냅샷 중에 도착한 출력이 영영 화면에 반영되지 않는다
    ///   (구독은 그 generation에 대해 이미 알렸으므로 다시 알리지 않는다)
    /// 후속 요청이 필요하면 `Some(task)`를 돌려준다. (`Task`에는 "비었는지"를
    /// 묻는 API가 없으므로 `Option`으로 명시한다 — 호출자와 테스트 모두 이걸 본다)
    pub fn apply_snapshot(&mut self, id: SessionId, generation: u64,
                          snapshot: TerminalSnapshot) -> Option<Task<Message>>;
    /// 슬롯을 꺼내 Arc를 reaper에 넘긴다.
    pub fn close(&mut self, id: SessionId);
    /// 시작 결과가 늦게 도착했을 때 슬롯을 만들지 결정한다. **세션 스토어는 어떤
    /// worktree가 살아 있는지 모른다** — 그건 앱 상태의 정보이므로 호출자가
    /// `worktree_still_exists`로 알려준다. 거절되면 세션은 곧장 reaper로 간다
    /// (고아 세션이 남으면 PTY와 스레드가 새어나간다).
    pub fn accept_started(&mut self, id: SessionId, worktree_id: WorktreeId,
                          session: TerminalSession, worktree_still_exists: bool)
        -> Result<(), StartRejected>;
}
```

**Reaper** (`src/reaper.rs`):
- 전용 스레드 하나가 **대기 목록**을 순회한다. 각 항목은 `Arc<TerminalSession>`이고, `Arc::strong_count == 1`이 된 것부터 떨어뜨린다
- **단일 대기(하나를 기다리며 블로킹)로 만들면 안 된다** — 구독 클론이 오래 남은 세션 하나가 뒤에 닫힌 모든 세션의 정리를 막는다(head-of-line blocking)
- 인터페이스: `Reaper::spawn() -> Reaper`, `Reaper::retire(&self, session: Arc<TerminalSession>)`

- [ ] **Step 1: 실패하는 테스트** (`tests/session_store_test.rs`)

```rust
mod platform; // suaegi-term의 tests/platform/mod.rs 복사

use std::sync::{Arc, Mutex};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

fn wait_until<F: FnMut() -> bool>(t: Duration, mut f: F) -> bool {
    let deadline = Instant::now() + t;
    while Instant::now() < deadline {
        if f() { return true; }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

/// Drop이 **실제로 어느 스레드에서 실행됐는지** 기록하는 센티널.
/// "reaper 스레드 id를 돌려주는" 헬퍼로는 소멸자가 거기서 돌았다는 증거가 안 된다.
struct DropSentinel(Arc<Mutex<Option<ThreadId>>>);
impl Drop for DropSentinel {
    fn drop(&mut self) {
        *self.0.lock().unwrap() = Some(std::thread::current().id());
    }
}

#[test]
fn a_stale_snapshot_result_never_overwrites_a_newer_one() {
    let mut store = SessionStore::for_test();
    let id = store.start_for_test(platform::echo("hello"));
    assert!(wait_until(Duration::from_secs(10), || { store.pump_for_test(id); store.row_text(id, 0).contains("hello") }));
    let newest = store.row_text(id, 0);
    let current = store.snapshot_generation(id);
    let _ = store.apply_snapshot(id, current.saturating_sub(1), blank_snapshot());
    assert_eq!(store.row_text(id, 0), newest, "stale result must be discarded");
}

#[test]
fn only_one_snapshot_request_is_in_flight_per_session() {
    let mut store = SessionStore::for_test();
    let id = store.start_for_test(platform::echo("x"));
    let (first, _) = store.request_snapshot(id, 1);
    let (second, _) = store.request_snapshot(id, 2);
    assert!(first);
    assert!(!second, "a second request must be suppressed while one is in flight");
}

#[test]
fn the_guard_clears_on_its_own_result_and_not_on_someone_elses() {
    // 가드를 안 풀면 그 세션은 영영 스냅샷을 못 뜨고 화면이 굳는다.
    // 반대로 아무 결과에나 풀면 동시 스냅샷이 생겨 리더와 락을 두고 경합한다.
    let mut store = SessionStore::for_test();
    let id = store.start_for_test(platform::echo("x"));

    let (issued, _) = store.request_snapshot(id, 5);
    assert!(issued);

    // 이 요청과 무관한(오래된) 결과가 도착해도 가드는 그대로여야 한다
    assert!(store.apply_snapshot(id, 1, blank_snapshot()).is_none());
    assert!(!store.request_snapshot(id, 6).0, "a foreign result must not release the guard");

    // 자기 결과가 도착하면 풀린다
    let _ = store.apply_snapshot(id, 5, blank_snapshot());
    assert!(store.request_snapshot(id, 6).0, "the matching result must release the guard");
}

#[test]
fn output_arriving_during_a_snapshot_is_not_lost() {
    // 스냅샷이 도는 동안 generation이 올라가면 구독은 그 세대를 이미 알린 뒤라
    // 다시 알리지 않는다. 완료 시점에 다시 요청하지 않으면 그 출력은 영영
    // 화면에 안 나온다 — 터미널이 조용히 멈춘 것처럼 보인다.
    let mut store = SessionStore::for_test();
    let id = store.start_for_test(platform::echo("first"));

    let (issued, _task) = store.request_snapshot(id, 1);
    assert!(issued);
    store.bump_generation_for_test(id, 9); // 스냅샷이 도는 동안 출력이 더 들어왔다
    let follow_up = store.apply_snapshot(id, 1, blank_snapshot());
    assert!(follow_up.is_some(),
            "completion must schedule another snapshot when the session moved on");
}

#[test]
fn closing_through_the_store_drops_the_session_off_the_calling_thread() {
    // **반드시 SessionStore::close()를 거친다.** Reaper를 직접 부르면
    // close()를 "그 자리에서 drop"으로 되돌리는 mutation을 잡지 못한다.
    let mut store = SessionStore::for_test();
    let id = store.start_for_test(platform::sleep_seconds(30));
    let impostor = store.clone_arc_for_test(id); // 구독이 든 클론을 흉내
    let caller = std::thread::current().id();

    store.close(id);
    std::thread::sleep(Duration::from_millis(100));
    assert!(store.reaper_drop_thread_for_test(id).is_none(),
            "reaper must wait while another clone is alive");

    drop(impostor);
    assert!(wait_until(Duration::from_secs(10),
                       || store.reaper_drop_thread_for_test(id).is_some()));
    assert_ne!(store.reaper_drop_thread_for_test(id).unwrap(), caller,
               "the session must not be destroyed on the calling thread");
}

#[test]
fn a_stuck_session_does_not_block_reaping_of_later_ones() {
    // head-of-line blocking 방지: 앞선 세션의 클론이 오래 살아 있어도
    // 뒤에 은퇴한 세션은 제때 정리돼야 한다.
    let reaper = Reaper::spawn();
    let stuck_where = Arc::new(Mutex::new(None));
    let later_where = Arc::new(Mutex::new(None));
    let stuck = Arc::new(DropSentinel(stuck_where.clone()));
    let stuck_clone = stuck.clone();          // 일부러 계속 살려둔다
    let later = Arc::new(DropSentinel(later_where.clone()));

    reaper.retire_for_test(stuck);
    reaper.retire_for_test(later);            // 뒤에 은퇴

    assert!(wait_until(Duration::from_secs(10), || later_where.lock().unwrap().is_some()),
            "a later session must be reaped even while an earlier one is pinned");
    assert!(stuck_where.lock().unwrap().is_none());
    drop(stuck_clone);
}

#[test]
fn a_session_that_exits_reports_its_code_and_stops_running() {
    let mut store = SessionStore::for_test();
    let id = store.start_for_test(platform::exit_with(3));
    assert!(wait_until(Duration::from_secs(10), || store.exit_code(id) == Some(3)));
    assert!(!store.is_running(id));
}

#[test]
fn a_late_start_for_a_deleted_worktree_is_retired_not_orphaned() {
    let mut store = SessionStore::for_test();
    let id = store.next_id();
    let gone = WorktreeId("/tmp/deleted".into());
    let session = start_throwaway_session(platform::sleep_seconds(30));
    // 호출자가 "그 worktree는 이제 없다"고 알려준다
    assert!(store.accept_started(id, gone, session, false).is_err());
    assert_eq!(store.slot_count(), 0, "no orphan slot");
    // 세션은 reaper로 갔어야 한다 — 아니면 PTY와 스레드가 샌다
    assert!(wait_until(Duration::from_secs(10), || store.reaper_retired_count() == 1));
}

#[test]
fn a_stale_presence_result_does_not_overwrite_a_newer_one() {
    let mut store = SessionStore::for_test();
    let id = store.start_for_test(platform::sleep_seconds(30));
    store.apply_presence(id, 2, AgentPresence::Agent(AgentKind::Claude));
    store.apply_presence(id, 1, AgentPresence::NoAgent); // 늦게 도착한 옛 결과
    assert!(matches!(store.presence(id), AgentPresence::Agent(_)),
            "an older presence result must be discarded");
}
```

**mutation 검증 필수** (넷 다 확인하고 관측 결과를 리포트에 적는다):
- `apply_snapshot`의 generation 비교 제거 → 첫 테스트 실패
- `snapshot_in_flight` 가드 제거 → 둘째 실패
- `close`를 그 자리 drop으로 되돌리기 → 셋째 실패
- reaper를 단일 대기(앞의 것을 다 기다린 뒤 다음)로 되돌리기 → 넷째 실패

하나라도 mutation을 통과하면 그 테스트는 이름값을 못 하므로 다시 설계한다.

- [ ] **Step 2~5**: 구현, 통과 확인, 커밋 (`feat(app): session store with off-thread snapshots and a non-blocking reaper`)

---

### Task 6: 워크벤치 (pane_grid + 텍스트) + 세션 구독

**Files:** `crates/suaegi-app/src/workbench.rs`

**Interfaces:**
- `workbench::view(state: &AppState) -> Element<'_, Message>` — `pane_grid` 분할, 각 pane은 캐시된 스냅샷을 모노스페이스 텍스트로
- `workbench::subscription(state: &AppState) -> Subscription<Message>` — 세션별 `generation()` 감시 → `Message::SessionDirty { id, generation }`

**구독 동일성이 이 태스크의 핵심이다.** `Subscription::run`은 `fn` 포인터라 캡처가 안 되므로 `run_with`를 쓰고, `D`의 `Hash`는 **세션 id만** 해싱한다:

```rust
#[derive(Clone)]
struct TermFeed { id: u64, session: Arc<TerminalSession> }

impl std::hash::Hash for TermFeed {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state); // 오직 id — Arc는 동일성에 참여하지 않는다
    }
}
```

`D`에 매 프레임 바뀌는 값(generation, 타임스탬프)을 해싱하면 구독이 프레임마다 파괴/재생성되고 터미널이 끊긴다.

**스트림 본문은 반드시 페이싱한다.** `generation()`을 루프에서 그냥 읽으면 executor 워커를 점유한 채 busy-spin 한다. `std::thread::sleep`은 async 워커를 블로킹하므로 **`tokio::time::sleep`**(약 16ms)을 쓴다. 전역 `iced::time::every` 하나로 모든 세션을 훑는 대안은 바쁜 터미널과 유휴 터미널을 같은 주기로 묶으므로 택하지 않는다.

- [ ] **Step 1: 동일성 테스트** (`src/workbench.rs` 하단)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::{Hash, Hasher};

    /// 해시 입력 바이트를 그대로 기록한다. "우연히 같은 u64"가 아니라
    /// "무엇을 해싱했는지"를 직접 본다.
    #[derive(Default)]
    struct RecordingHasher(Vec<u8>);
    impl Hasher for RecordingHasher {
        fn write(&mut self, bytes: &[u8]) { self.0.extend_from_slice(bytes); }
        fn finish(&self) -> u64 { 0 }
    }
    fn recorded<T: Hash>(v: &T) -> Vec<u8> {
        let mut h = RecordingHasher::default();
        v.hash(&mut h);
        h.0
    }

    #[test]
    fn feed_identity_is_exactly_the_session_id_and_nothing_else() {
        // 서로 다른 세션 객체를 같은 id로 감쌌을 때 같아야 한다.
        // 같은 Arc의 클론 둘로 비교하면 포인터를 해싱해도 통과해버린다.
        let a = TermFeed { id: 7, session: start_throwaway_session() };
        let b = TermFeed { id: 7, session: start_throwaway_session() };
        assert_eq!(recorded(&a), recorded(&b));
        assert_eq!(recorded(&a), recorded(&7u64), "only the id may be hashed");
    }

    #[test]
    fn different_sessions_have_different_identity() {
        let a = TermFeed { id: 7, session: start_throwaway_session() };
        let b = TermFeed { id: 8, session: start_throwaway_session() };
        assert_ne!(recorded(&a), recorded(&b));
    }
}
```

- [ ] **Step 2~5**: 구현, 통과 확인, 커밋 (`feat(app): pane-grid workbench rendering cached snapshots`)

pane_grid 주의(전부 소스 확인됨): `State::new`는 `(Self, Pane)`; `panes`는 공개 필드; `close`는 `Option<(T, Pane)>`로 `T`를 돌려주므로 거기서 세션을 정리하면 자연스럽다; 기본 `spacing = 0` + leeway 없음이면 분할선을 잡을 수 없으니 `.spacing(2).on_resize(8, Message::PaneResized)`; 최대화 중에는 `on_drag`/`on_resize`가 조용히 무시됨; 타이틀바 없는 pane은 드래그 불가.

눈으로 확인: 두 개 이상 세션을 띄우고 분할, 출력이 흐르고, 한쪽을 닫아도 다른 쪽이 살아있는지.

---

### Task 7: 에이전트 존재 폴링 (티어링)

**Files:** `crates/suaegi-app/src/presence_poll.rs`

**Interfaces:**
- `tier(state: &AppState) -> Duration` — 활성 750ms / 유휴 2s
- `subscription(state) -> Subscription<Message>` — `iced::time::every(tier(state))`. **`every`는 `Duration` 자체로 키가 잡히므로** 티어가 바뀌면 런타임이 알아서 타이머를 교체한다
- 틱 처리: `sessions_to_probe(state)`가 in-flight가 아닌 세션만 고르고, 각각 `background::blocking`으로 프로브 → `Message::PresenceProbed { id, seq, presence }`
- **모니터는 슬롯이 소유한다**(`Arc<Mutex<PresenceMonitor>>`) — 틱마다 새로 만들면 pgid 캐시가 죽어 매번 `ps`를 띄운다
- `probe`가 `Task::perform`에 적대적인 이유: 블로킹 `ps` fork/exec, `&mut self`, `&TerminalSession` 참조, **`&dyn ProcessProbe`에 `Send` 바운드 없음**. 그래서 `Arc<TerminalSession>`과 `Arc<Mutex<PresenceMonitor>>`를 블로킹 스레드로 옮기고 **`PsProbe`를 그 안에서 만든다**(`Copy + Send`)

- [ ] **Step 1: 실패하는 테스트** — **디스패치 사이클 자체를 검증한다.** 플래그를 손으로 세우고 필터를 부르는 테스트는 실제 틱 경로가 플래그를 세우는지/지우는지를 증명하지 못한다

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// 호출 횟수를 세는 프로브. 캐시와 in-flight 가드가 실제로 동작하는지
    /// "몇 번 불렸나"로 확인한다.
    struct CountingProbe(Arc<AtomicUsize>);
    impl suaegi_term::presence::ProcessProbe for CountingProbe {
        fn command_line(&self, _pid: i32) -> Option<String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Some("claude".to_string())
        }
    }

    #[test]
    fn active_sessions_poll_faster_than_idle_ones() {
        assert!(tier(&state_with_recent_output()) < tier(&state_idle()));
    }

    #[test]
    fn no_sessions_means_the_slow_tier() {
        assert_eq!(tier(&AppState::default()), IDLE_TIER);
    }

    #[test]
    fn a_tick_while_a_probe_is_in_flight_does_not_dispatch_a_second_one() {
        // 손으로 플래그를 세우지 않는다 — 실제 틱 경로를 두 번 돌린다.
        let mut state = state_with_one_session();
        let dispatched_first = dispatch_tick(&mut state);   // 실제 핸들러
        let dispatched_second = dispatch_tick(&mut state);  // 첫 결과가 오기 전
        assert_eq!(dispatched_first.len(), 1);
        assert!(dispatched_second.is_empty(), "no second probe while one is in flight");
    }

    #[test]
    fn the_guard_clears_when_the_result_arrives_so_the_next_tick_dispatches() {
        let mut state = state_with_one_session();
        let first = dispatch_tick(&mut state);
        apply_presence_result(&mut state, first[0], AgentPresence::NoAgent);
        assert_eq!(dispatch_tick(&mut state).len(), 1, "the guard must clear on result");
    }

    /// 이 검증은 foreground pgid가 관측되는 unix에서만 의미가 있다.
    /// Windows에서는 존재 감지가 항상 Unknown이라 호출 횟수가 0으로 남는다.
    #[cfg(unix)]
    #[test]
    fn the_monitor_cache_survives_across_ticks() {
        // 틱마다 새 모니터를 만들면 "에이전트임"이 캐시되지 않아 ps가 매번 뜬다.
        // 필드 포인터 비교로는 그 mutation을 못 잡으므로 호출 횟수로 본다.
        let calls = Arc::new(AtomicUsize::new(0));
        let mut state = state_with_one_session();

        run_probe_now(&mut state, CountingProbe(calls.clone()));
        let after_first = calls.load(Ordering::SeqCst);
        // 첫 프로브가 실제로 일어났는지부터 확인한다 — 0이면 그 뒤 비교는 공허하다
        assert!(after_first > 0, "the first tick must actually probe; got 0 calls");

        run_probe_now(&mut state, CountingProbe(calls.clone()));
        assert_eq!(calls.load(Ordering::SeqCst), after_first,
                   "a cached agent pgid must not re-probe on the next tick");
    }
}
```

**mutation 검증 필수**: in-flight 가드 제거 → 세 번째 테스트 실패; 틱마다 새 모니터 생성 → 마지막 테스트 실패.

- [ ] **Step 2~5**: 구현, 통과 확인, 커밋 (`feat(app): tiered agent-presence polling off the ui thread`)

---

### Task 8: 통합 + 최종 게이트

**Files:** `src/main.rs`(전체 배선), `docs/follow-ups.md`

- [ ] **Step 1: 자동화 가능한 것부터 테스트로**

수동 체크는 "확인했다"고 표시만 하고 넘어가기 쉬우므로 먼저 못 박는다:
- `LoadOrigin` 네 값이 각각 어떤 상태 표시줄 문구가 되는지 (**Fresh는 경고 없음**)
- `SaveStatus::Failed`가 표시줄에 드러나고, `Superseded`는 드러나지 않는지 (정상 동작을 에러처럼 보이면 안 된다)
- worktree 생성/삭제 실패가 UI 상태에 남는지

(`write()`의 유실 표시는 Plan 3에 **입력 경로가 없으므로** 여기서 다루지 않는다 — Plan 4에서 키 입력과 함께 들어온다.)

- [ ] **Step 2: 종단 흐름 (수동, 관측 결과를 리포트에)**

1. 앱 실행 → 창이 뜬다
2. repo 추가 → 사이드바에 나타난다
3. worktree 생성 → 디스크에 생기고 목록에 나타난다
4. 세션 시작 → 셸 출력이 보인다
5. 두 번째 worktree로 분할 → 양쪽이 독립적으로 돈다
6. 종료 → 재실행 → repo/worktree 복원
7. **worktree 여러 개를 빠르게 닫아도 UI가 멈추지 않는다** (reaper 검증)

- [ ] **Step 3: 최종 게이트**

`cargo test --workspace -- --test-threads=4 && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`

- [ ] **Step 4: follow-ups 갱신 + Commit**

터미널 위젯(색/커서/키 입력/리사이즈/마우스)은 Plan 4, 세션 레이아웃 복원은 Plan 5로 기록.

```bash
git commit -m "feat(app): wire the shell end to end"
```

---

## 이 플랜이 다루지 않는 것

- **터미널 입력.** 읽기 전용이다. 키 입력 → PTY 바이트는 커스텀 위젯이 필요하고(포커스가 `Widget::operate` + `operation::Focusable`로만 오므로 canvas로는 불가능) Plan 4다.
- **색/커서/폰트 속성.** 스냅샷 셀은 `fg`/`bg`/`flags`를 이미 들고 있지만 Plan 3은 단색으로 그린다.
- **마우스.** 선택, 스크롤, 마우스 리포팅 전부 Plan 4.
- **알아둘 위험**: 터미널 pane은 마우스 이벤트를 소비해야 하는데 `pane_grid`도 같은 영역에 `on_click`과 분할 히트테스트를 건다. 마우스를 쓰는 터미널 본문이 `Content` 안에서 깨끗이 합성되는지는 **Plan 4에서 가장 먼저 스파이크할 것** — 이 설계에서 가장 깨지기 쉬운 가정이다.

## 후속 플랜

- Plan 4: 터미널 커스텀 위젯(색/커서/키 입력/포커스/리사이즈/마우스) + 워크벤치 완성
- Plan 5: 에이전트 hook 서버 + diff 패널 + 세션 레이아웃 복원
