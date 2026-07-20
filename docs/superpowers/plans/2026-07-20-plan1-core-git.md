# Suaegi Plan 1: Workspace + suaegi-core + suaegi-git Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rust 워크스페이스를 세우고, 도메인 모델+영속화(`suaegi-core`)와 git CLI 실행 계층(`suaegi-git`)을 UI 없이 완전히 테스트된 상태로 완성한다.

**Architecture:** 4-crate workspace 중 하위 2개를 만든다. `suaegi-core`는 순수 데이터(도메인 타입 + JSON 영속화, 원자적 쓰기 + 롤링 백업 + 손상 폴백). `suaegi-git`은 모든 git 작업을 git CLI subprocess로 실행(라이브러리 없음, Orca 검증 방식)하고 구조화된 에러를 돌려준다. 의존 방향: `git → core`, 역방향 금지.

**Tech Stack:** Rust 1.94 (edition 2021), serde/serde_json, thiserror, tokio(process/time), dirs. dev-deps: tempfile.

**Spec:** `docs/superpowers/specs/2026-07-20-suaegi-mvp-design.md`

## Global Constraints

- Rust edition 2021, 로컬 툴체인 1.94.0 (`.tool-versions`에 이미 고정됨)
- git 작업은 전부 CLI shell-out; git2/gitoxide 금지
- git 실행 env: `LC_ALL=C`, `GIT_TERMINAL_PROMPT=0` 강제
- worktree add 타임아웃 180초, 그 외 git 명령 기본 30초
- 영속화: 단일 JSON `<config_dir>/suaegi/data.json`, 원자적 쓰기(temp+rename), 롤링 백업 5개(`data.json.bak.0..4`, ≥1시간 간격), 읽기 실패 시 백업 폴백 → 최후엔 default (크래시 금지)
- 파일/모듈 이름에 `utils`/`helpers` 금지 (구체적 도메인 이름 사용)
- 모든 커밋은 테스트 통과 상태에서만

---

### Task 1: Cargo workspace 스캐폴드

**Files:**
- Create: `Cargo.toml` (workspace root)
- Create: `crates/suaegi-core/Cargo.toml`, `crates/suaegi-core/src/lib.rs`
- Create: `crates/suaegi-git/Cargo.toml`, `crates/suaegi-git/src/lib.rs`

**Interfaces:**
- Produces: 빌드 가능한 빈 workspace. 이후 모든 태스크의 토대.

- [ ] **Step 1: workspace root Cargo.toml 작성**

```toml
[workspace]
resolver = "2"
members = ["crates/suaegi-core", "crates/suaegi-git"]

[workspace.package]
version = "0.1.0"
edition = "2021"
license = "MIT"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
tokio = { version = "1", features = ["process", "time", "rt-multi-thread", "macros", "io-util"] }
dirs = "6"
tempfile = "3"
```

- [ ] **Step 2: suaegi-core 크레이트 생성**

`crates/suaegi-core/Cargo.toml`:
```toml
[package]
name = "suaegi-core"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
dirs = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```

`crates/suaegi-core/src/lib.rs`:
```rust
pub mod domain;
pub mod persistence;
```
(모듈 파일은 Task 2, 3에서 생성. 이 시점엔 빈 파일 `domain.rs`, `persistence.rs`를 만들어 컴파일만 되게 한다.)

- [ ] **Step 3: suaegi-git 크레이트 생성**

`crates/suaegi-git/Cargo.toml`:
```toml
[package]
name = "suaegi-git"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
suaegi-core = { path = "../suaegi-core" }
serde = { workspace = true }
thiserror = { workspace = true }
tokio = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```

`crates/suaegi-git/src/lib.rs`:
```rust
pub mod runner;
pub mod worktree_name;
pub mod worktree;
pub mod compare;
pub mod repo_probe;
```
(각 모듈 파일은 해당 태스크에서 생성. 이 시점엔 빈 파일로.)

- [ ] **Step 4: 빌드 확인**

Run: `cargo build --workspace && cargo clippy --workspace -- -D warnings`
Expected: 성공 (경고 0)

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates
git commit -m "chore: scaffold cargo workspace with suaegi-core and suaegi-git"
```

---

### Task 2: suaegi-core 도메인 모델

**Files:**
- Create: `crates/suaegi-core/src/domain.rs`
- Test: 같은 파일 하단 `#[cfg(test)]`

**Interfaces:**
- Produces (이후 모든 크레이트가 사용):
  - `RepoId(pub String)`, `WorktreeId(pub String)` — 뉴타입, canonical path 문자열
  - `Repo { id, path: PathBuf, display_name: String, worktree_base_ref: Option<String> }`
  - `Worktree { id, repo_id, path: PathBuf, branch: String, display_name: String, created_with_agent: Option<String>, created_at_unix_ms: u64 }`
  - `Settings { workspace_root: PathBuf }` + `Settings::default_with_home(home: &Path)`
  - `PersistedState { schema_version: u32, repos: Vec<Repo>, worktrees: Vec<Worktree>, settings: Settings }` + `Default`
  - `SCHEMA_VERSION: u32 = 1`

- [ ] **Step 1: 실패하는 테스트 작성** (`domain.rs` 하단)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn persisted_state_round_trips_through_json() {
        let state = PersistedState {
            schema_version: SCHEMA_VERSION,
            repos: vec![Repo {
                id: RepoId("/tmp/demo".into()),
                path: PathBuf::from("/tmp/demo"),
                display_name: "demo".into(),
                worktree_base_ref: Some("main".into()),
            }],
            worktrees: vec![Worktree {
                id: WorktreeId("/tmp/ws/demo/fix-bug".into()),
                repo_id: RepoId("/tmp/demo".into()),
                path: PathBuf::from("/tmp/ws/demo/fix-bug"),
                branch: "fix-bug".into(),
                display_name: "fix-bug".into(),
                created_with_agent: Some("claude".into()),
                created_at_unix_ms: 1_700_000_000_000,
            }],
            settings: Settings { workspace_root: PathBuf::from("/tmp/ws") },
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: PersistedState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, back);
    }

    #[test]
    fn default_state_is_empty_with_current_schema() {
        let d = PersistedState::default();
        assert_eq!(d.schema_version, SCHEMA_VERSION);
        assert!(d.repos.is_empty() && d.worktrees.is_empty());
    }

    #[test]
    fn default_settings_places_workspace_root_under_home() {
        let s = Settings::default_with_home(&PathBuf::from("/home/u"));
        assert_eq!(s.workspace_root, PathBuf::from("/home/u/suaegi-workspaces"));
    }
}
```

- [ ] **Step 2: 테스트 실패 확인**

Run: `cargo test -p suaegi-core`
Expected: 컴파일 에러 (타입 미정의)

- [ ] **Step 3: 최소 구현**

```rust
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RepoId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorktreeId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repo {
    pub id: RepoId,
    pub path: PathBuf,
    pub display_name: String,
    /// worktree 생성 기본 base ref. None이면 생성 시점에 HEAD 브랜치를 감지해 사용.
    pub worktree_base_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worktree {
    pub id: WorktreeId,
    pub repo_id: RepoId,
    pub path: PathBuf,
    pub branch: String,
    pub display_name: String,
    pub created_with_agent: Option<String>,
    pub created_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub workspace_root: PathBuf,
}

impl Settings {
    pub fn default_with_home(home: &Path) -> Self {
        Self { workspace_root: home.join("suaegi-workspaces") }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedState {
    pub schema_version: u32,
    #[serde(default)]
    pub repos: Vec<Repo>,
    #[serde(default)]
    pub worktrees: Vec<Worktree>,
    pub settings: Settings,
}

impl Default for PersistedState {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            schema_version: SCHEMA_VERSION,
            repos: Vec::new(),
            worktrees: Vec::new(),
            settings: Settings::default_with_home(&home),
        }
    }
}
```

- [ ] **Step 4: 테스트 통과 확인**

Run: `cargo test -p suaegi-core`
Expected: 3 passed

- [ ] **Step 5: Commit**

```bash
git add crates/suaegi-core/src/domain.rs
git commit -m "feat(core): domain model (Repo, Worktree, Settings, PersistedState)"
```

---

### Task 3: 영속화 — 원자적 저장/로드 + 동일 내용 스킵

**Files:**
- Create: `crates/suaegi-core/src/persistence.rs`
- Test: 같은 파일 하단 `#[cfg(test)]`

**Interfaces:**
- Produces:
  - `Store::new(data_file: PathBuf) -> Store`
  - `Store::load(&self) -> PersistedState` — 실패해도 panic 없이 default 반환
  - `Store::save(&mut self, state: &PersistedState) -> Result<SaveOutcome, PersistenceError>`
  - `enum SaveOutcome { Written, SkippedUnchanged }`
  - `PersistenceError` (thiserror, `Io(std::io::Error)` + `Serialize(serde_json::Error)`)

- [ ] **Step 1: 실패하는 테스트 작성**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::*;
    use std::path::PathBuf;

    fn sample_state(name: &str) -> PersistedState {
        let mut s = PersistedState::default();
        s.repos.push(Repo {
            id: RepoId(format!("/tmp/{name}")),
            path: PathBuf::from(format!("/tmp/{name}")),
            display_name: name.into(),
            worktree_base_ref: None,
        });
        s
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::new(dir.path().join("data.json"));
        let state = sample_state("a");
        assert!(matches!(store.save(&state).unwrap(), SaveOutcome::Written));
        assert_eq!(store.load(), state);
    }

    #[test]
    fn saving_identical_state_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::new(dir.path().join("data.json"));
        let state = sample_state("a");
        store.save(&state).unwrap();
        assert!(matches!(store.save(&state).unwrap(), SaveOutcome::SkippedUnchanged));
    }

    #[test]
    fn load_missing_file_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("nope/data.json"));
        assert_eq!(store.load(), PersistedState::default());
    }

    #[test]
    fn save_creates_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::new(dir.path().join("deep/nested/data.json"));
        store.save(&sample_state("a")).unwrap();
        assert!(dir.path().join("deep/nested/data.json").exists());
    }
}
```

- [ ] **Step 2: 테스트 실패 확인**

Run: `cargo test -p suaegi-core persistence`
Expected: 컴파일 에러 (Store 미정의)

- [ ] **Step 3: 최소 구현**

```rust
use crate::domain::PersistedState;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialize: {0}")]
    Serialize(#[from] serde_json::Error),
}

#[derive(Debug, PartialEq, Eq)]
pub enum SaveOutcome {
    Written,
    SkippedUnchanged,
}

pub struct Store {
    data_file: PathBuf,
    last_written_hash: Option<u64>,
}

impl Store {
    pub fn new(data_file: PathBuf) -> Self {
        Self { data_file, last_written_hash: None }
    }

    pub fn data_file(&self) -> &PathBuf {
        &self.data_file
    }

    pub fn load(&self) -> PersistedState {
        match fs::read_to_string(&self.data_file) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(state) => state,
                Err(_) => self.load_from_backups(),
            },
            Err(_) => self.load_from_backups(),
        }
    }

    // Task 4에서 백업 폴백 구현. 이 시점엔 default만.
    fn load_from_backups(&self) -> PersistedState {
        PersistedState::default()
    }

    pub fn save(&mut self, state: &PersistedState) -> Result<SaveOutcome, PersistenceError> {
        let json = serde_json::to_string_pretty(state)?;
        let mut hasher = DefaultHasher::new();
        json.hash(&mut hasher);
        let hash = hasher.finish();
        if self.last_written_hash == Some(hash) {
            return Ok(SaveOutcome::SkippedUnchanged);
        }
        if let Some(parent) = self.data_file.parent() {
            fs::create_dir_all(parent)?;
        }
        // 원자적 쓰기: 같은 디렉토리에 temp 작성 후 rename
        let tmp = self.data_file.with_extension("json.tmp");
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(json.as_bytes())?;
            f.sync_all()?;
        }
        fs::rename(&tmp, &self.data_file)?;
        self.last_written_hash = Some(hash);
        Ok(SaveOutcome::Written)
    }
}
```

- [ ] **Step 4: 테스트 통과 확인**

Run: `cargo test -p suaegi-core`
Expected: 전체 passed (Task 2의 3개 + 신규 4개)

- [ ] **Step 5: Commit**

```bash
git add crates/suaegi-core/src/persistence.rs
git commit -m "feat(core): atomic JSON store with unchanged-skip"
```

---

### Task 4: 영속화 — 롤링 백업 + 손상 폴백

**Files:**
- Modify: `crates/suaegi-core/src/persistence.rs`

**Interfaces:**
- Consumes: Task 3의 `Store`
- Produces:
  - 저장 성공 시 백업 회전: `data.json.bak.0`(최신)..`.bak.4`, 직전 백업이 1시간 이내면 회전 생략
  - `Store::load()`가 본파일 파싱 실패 시 `.bak.0..4` 순서로 폴백, 전부 실패 시 default
  - 테스트 훅: `Store::set_backup_min_interval(Duration)` (기본 3600초)

- [ ] **Step 1: 실패하는 테스트 추가** (`persistence.rs` tests 모듈에)

```rust
    use std::time::Duration;

    #[test]
    fn corrupt_main_file_falls_back_to_backup() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("data.json");
        let mut store = Store::new(file.clone());
        store.set_backup_min_interval(Duration::ZERO);
        let v1 = sample_state("v1");
        store.save(&v1).unwrap();
        let v2 = sample_state("v2");
        store.save(&v2).unwrap(); // v2 저장 직전에 v1이 .bak.0으로 회전됨
        std::fs::write(&file, "{ corrupted!!").unwrap();
        assert_eq!(store.load(), v1);
    }

    #[test]
    fn corrupt_main_and_backups_return_default() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("data.json");
        let mut store = Store::new(file.clone());
        store.set_backup_min_interval(Duration::ZERO);
        store.save(&sample_state("a")).unwrap();
        std::fs::write(&file, "bad").unwrap();
        std::fs::write(dir.path().join("data.json.bak.0"), "also bad").unwrap();
        assert_eq!(store.load(), PersistedState::default());
    }

    #[test]
    fn backups_rotate_up_to_five() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::new(dir.path().join("data.json"));
        store.set_backup_min_interval(Duration::ZERO);
        for i in 0..8 {
            store.save(&sample_state(&format!("s{i}"))).unwrap();
        }
        for i in 0..5 {
            assert!(dir.path().join(format!("data.json.bak.{i}")).exists(), "bak.{i}");
        }
        assert!(!dir.path().join("data.json.bak.5").exists());
    }

    #[test]
    fn backup_rotation_respects_min_interval() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::new(dir.path().join("data.json"));
        // 기본 간격(1h): 첫 백업은 생기되, 연속 저장이 회전을 반복하지는 않는다
        store.save(&sample_state("a")).unwrap();
        store.save(&sample_state("b")).unwrap(); // bak.0 = a 생성 (첫 백업)
        store.save(&sample_state("c")).unwrap(); // bak.0이 신선 → 회전 생략
        assert!(dir.path().join("data.json.bak.0").exists());
        assert!(!dir.path().join("data.json.bak.1").exists());
    }
```

- [ ] **Step 2: 테스트 실패 확인**

Run: `cargo test -p suaegi-core`
Expected: `set_backup_min_interval` 미정의 컴파일 에러

- [ ] **Step 3: 구현**

`Store` 구조체/함수 수정:

```rust
use std::time::{Duration, SystemTime};

const BACKUP_SLOTS: usize = 5;

pub struct Store {
    data_file: PathBuf,
    last_written_hash: Option<u64>,
    backup_min_interval: Duration,
}

impl Store {
    pub fn new(data_file: PathBuf) -> Self {
        Self {
            data_file,
            last_written_hash: None,
            backup_min_interval: Duration::from_secs(3600),
        }
    }

    pub fn set_backup_min_interval(&mut self, interval: Duration) {
        self.backup_min_interval = interval;
    }

    fn backup_path(&self, slot: usize) -> PathBuf {
        let name = self.data_file.file_name().unwrap_or_default().to_string_lossy();
        self.data_file.with_file_name(format!("{name}.bak.{slot}"))
    }

    fn load_from_backups(&self) -> PersistedState {
        for slot in 0..BACKUP_SLOTS {
            if let Ok(text) = fs::read_to_string(self.backup_path(slot)) {
                if let Ok(state) = serde_json::from_str(&text) {
                    return state;
                }
            }
        }
        PersistedState::default()
    }

    /// 본파일을 .bak.0으로 밀어넣고 기존 백업들을 한 칸씩 뒤로. 직전 백업이
    /// min_interval 이내면 생략 (Orca의 ≥1h 간격 패턴).
    fn rotate_backups(&self) -> Result<(), PersistenceError> {
        if !self.data_file.exists() {
            return Ok(());
        }
        let bak0 = self.backup_path(0);
        if let Ok(meta) = fs::metadata(&bak0) {
            if let Ok(modified) = meta.modified() {
                let age = SystemTime::now()
                    .duration_since(modified)
                    .unwrap_or(Duration::ZERO);
                if age < self.backup_min_interval {
                    return Ok(());
                }
            }
        }
        // Windows의 fs::rename은 대상이 존재하면 실패하므로 마지막 슬롯부터 비운다
        let oldest = self.backup_path(BACKUP_SLOTS - 1);
        if oldest.exists() {
            fs::remove_file(&oldest)?;
        }
        for slot in (0..BACKUP_SLOTS - 1).rev() {
            let from = self.backup_path(slot);
            if from.exists() {
                fs::rename(&from, self.backup_path(slot + 1))?;
            }
        }
        fs::copy(&self.data_file, &bak0)?;
        Ok(())
    }
}
```

`save()`의 `fs::rename(&tmp, &self.data_file)?;` 직전에 한 줄 추가:

```rust
        self.rotate_backups()?;
        fs::rename(&tmp, &self.data_file)?;
```

- [ ] **Step 4: 테스트 통과 확인**

Run: `cargo test -p suaegi-core`
Expected: 전체 passed (누적 11개)

- [ ] **Step 5: Commit**

```bash
git add crates/suaegi-core/src/persistence.rs
git commit -m "feat(core): rolling backups with corrupt-file fallback"
```

---

### Task 5: suaegi-git — GitRunner (CLI 실행 계층)

**Files:**
- Create: `crates/suaegi-git/src/runner.rs`
- Test: `crates/suaegi-git/tests/runner_test.rs`

**Interfaces:**
- Produces:
  - `GitRunner::new() -> GitRunner` (PATH의 `git` 사용)
  - `GitRunner::run(&self, cwd: &Path, args: &[&str]) -> Result<GitOutput, GitError>` — 기본 30초 타임아웃
  - `GitRunner::run_with_timeout(&self, cwd: &Path, args: &[&str], timeout: Duration) -> Result<GitOutput, GitError>`
  - `GitOutput { stdout: String, stderr: String }` (성공 시에만 반환; exit≠0은 Err)
  - `GitError { Spawn(std::io::Error) | Timeout { args: String } | Failed { args: String, code: Option<i32>, stderr: String } }` (thiserror)
  - 모든 실행에 env `LC_ALL=C`, `GIT_TERMINAL_PROMPT=0` 주입 — 이후 태스크 전부 이 runner만 사용

- [ ] **Step 1: 실패하는 테스트 작성** (`tests/runner_test.rs`)

```rust
use std::time::Duration;
use suaegi_git::runner::{GitError, GitRunner};

#[tokio::test]
async fn run_version_succeeds() {
    let r = GitRunner::new();
    let out = r.run(std::env::temp_dir().as_path(), &["--version"]).await.unwrap();
    assert!(out.stdout.starts_with("git version"));
}

#[tokio::test]
async fn failed_command_returns_structured_error() {
    let r = GitRunner::new();
    let dir = tempfile::tempdir().unwrap();
    let err = r.run(dir.path(), &["worktree", "list"]).await.unwrap_err();
    match err {
        GitError::Failed { code, stderr, .. } => {
            assert_ne!(code, Some(0));
            assert!(stderr.to_lowercase().contains("not a git repository"));
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[tokio::test]
async fn timeout_kills_and_reports() {
    let r = GitRunner::new();
    let dir = tempfile::tempdir().unwrap();
    // stdin은 null로 닫히므로 stdin 대기류 명령은 즉시 끝난다. 확실히 오래 걸리는
    // 명령으로 셸 alias를 사용 (POSIX 전용 테스트; Windows CI에선 cfg 게이트 예정).
    let err = r
        .run_with_timeout(
            dir.path(),
            &["-c", "alias.zzz=!sleep 30", "zzz"],
            Duration::from_millis(200),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, GitError::Timeout { .. }));
}
```

- [ ] **Step 2: 테스트 실패 확인**

Run: `cargo test -p suaegi-git --test runner_test`
Expected: 컴파일 에러

- [ ] **Step 3: 구현** (`src/runner.rs`)

```rust
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct GitOutput {
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("failed to spawn git: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("git {args} timed out")]
    Timeout { args: String },
    #[error("git {args} failed (code {code:?}): {stderr}")]
    Failed { args: String, code: Option<i32>, stderr: String },
}

#[derive(Debug, Clone, Default)]
pub struct GitRunner;

impl GitRunner {
    pub fn new() -> Self {
        Self
    }

    pub async fn run(&self, cwd: &Path, args: &[&str]) -> Result<GitOutput, GitError> {
        self.run_with_timeout(cwd, args, DEFAULT_TIMEOUT).await
    }

    pub async fn run_with_timeout(
        &self,
        cwd: &Path,
        args: &[&str],
        timeout: Duration,
    ) -> Result<GitOutput, GitError> {
        let args_str = args.join(" ");
        let mut child = Command::new("git")
            .args(args)
            .current_dir(cwd)
            // 파서가 항상 영어 출력을 보도록; 인증 프롬프트로 행 걸리지 않도록
            .env("LC_ALL", "C")
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;

        let waited = tokio::time::timeout(timeout, async {
            let out = child.wait_with_output().await?;
            Ok::<_, std::io::Error>(out)
        })
        .await;

        let output = match waited {
            Err(_) => return Err(GitError::Timeout { args: args_str }),
            Ok(result) => result?,
        };

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        if !output.status.success() {
            return Err(GitError::Failed {
                args: args_str,
                code: output.status.code(),
                stderr,
            });
        }
        Ok(GitOutput { stdout, stderr })
    }
}
```

- [ ] **Step 4: 테스트 통과 확인**

Run: `cargo test -p suaegi-git --test runner_test`
Expected: 3 passed

- [ ] **Step 5: Commit**

```bash
git add crates/suaegi-git/src/runner.rs crates/suaegi-git/tests/runner_test.rs
git commit -m "feat(git): GitRunner with timeout and structured errors"
```

---

### Task 6: worktree 이름 새니타이즈 + 충돌 후보

**Files:**
- Create: `crates/suaegi-git/src/worktree_name.rs`
- Test: 같은 파일 하단 `#[cfg(test)]`

**Interfaces:**
- Produces:
  - `sanitize_worktree_name(input: &str) -> String` — 유니코드 문자/숫자 유지, 그 외 `-` 치환, 연속 `-` 축약, 양끝 `-`/`.` 제거, 빈 결과면 `"workspace"`, 최대 60자
  - `candidate_names(base: &str) -> impl Iterator<Item = String>` — `base`, `base-2`, `base-3`, ... `base-100`

- [ ] **Step 1: 실패하는 테스트 작성**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_unicode_letters_and_digits() {
        assert_eq!(sanitize_worktree_name("버그수정 v2"), "버그수정-v2");
    }

    #[test]
    fn collapses_and_trims_dashes() {
        assert_eq!(sanitize_worktree_name("--fix!!bug--"), "fix-bug");
    }

    #[test]
    fn rejects_dot_prefix_and_double_dots() {
        assert_eq!(sanitize_worktree_name("..hidden"), "hidden");
        assert_eq!(sanitize_worktree_name("a..b"), "a-b");
    }

    #[test]
    fn empty_input_falls_back() {
        assert_eq!(sanitize_worktree_name("!!!"), "workspace");
    }

    #[test]
    fn truncates_to_60_chars() {
        let long = "a".repeat(100);
        assert_eq!(sanitize_worktree_name(&long).chars().count(), 60);
    }

    #[test]
    fn candidates_start_with_base_then_numbered() {
        let mut it = candidate_names("fix");
        assert_eq!(it.next().unwrap(), "fix");
        assert_eq!(it.next().unwrap(), "fix-2");
        assert_eq!(candidate_names("fix").last().unwrap(), "fix-100");
    }
}
```

- [ ] **Step 2: 테스트 실패 확인**

Run: `cargo test -p suaegi-git worktree_name`
Expected: 컴파일 에러

- [ ] **Step 3: 구현**

```rust
const MAX_LEN: usize = 60;
const MAX_SUFFIX: u32 = 100;

/// 유니코드 문자/숫자만 유지하고 나머지는 `-`로. git이 거부하는 `..`,
/// 선행 `.` 은 만들 수 없는 구조로 정규화한다 (Orca worktree-logic 차용).
pub fn sanitize_worktree_name(input: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = true; // 선행 대시 방지
    for ch in input.chars() {
        if ch.is_alphanumeric() {
            out.push(ch);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed: String = out.trim_matches(|c| c == '-' || c == '.').chars().take(MAX_LEN).collect();
    if trimmed.is_empty() {
        "workspace".to_string()
    } else {
        trimmed
    }
}

pub fn candidate_names(base: &str) -> impl Iterator<Item = String> + '_ {
    std::iter::once(base.to_string())
        .chain((2..=MAX_SUFFIX).map(move |n| format!("{base}-{n}")))
}
```

- [ ] **Step 4: 테스트 통과 확인**

Run: `cargo test -p suaegi-git worktree_name`
Expected: 6 passed

- [ ] **Step 5: Commit**

```bash
git add crates/suaegi-git/src/worktree_name.rs
git commit -m "feat(git): worktree name sanitization and collision candidates"
```

---

### Task 7: repo 검증 + 기본 base 감지

**Files:**
- Create: `crates/suaegi-git/src/repo_probe.rs`
- Create: `crates/suaegi-git/tests/fixture/mod.rs` (공용 테스트 픽스처 — `mod.rs` 위치라야 빈 테스트 바이너리로 컴파일되지 않음)
- Test: `crates/suaegi-git/tests/repo_probe_test.rs`

**Interfaces:**
- Consumes: `GitRunner` (Task 5)
- Produces:
  - `probe_repo(runner: &GitRunner, path: &Path) -> Result<RepoProbe, GitError>`
  - `RepoProbe { is_git_repo: bool, head_branch: Option<String> }`
  - 테스트 픽스처 `fixture::init_repo(dir: &Path)` — `git init -b main` + 커밋 1개 생성 (이후 태스크들이 재사용)

- [ ] **Step 1: 픽스처 작성** (`tests/fixture/mod.rs`)

```rust
use std::path::Path;
use std::process::Command;

/// 테스트용 실제 git repo: `git init -b main` + README 커밋 1개.
pub fn init_repo(dir: &Path) {
    run(dir, &["init", "-b", "main"]);
    run(dir, &["config", "user.email", "t@example.com"]);
    run(dir, &["config", "user.name", "test"]);
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    run(dir, &["add", "."]);
    run(dir, &["commit", "-m", "init"]);
}

pub fn run(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("LC_ALL", "C")
        .output()
        .expect("spawn git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}
```

- [ ] **Step 2: 실패하는 테스트 작성** (`tests/repo_probe_test.rs`)

```rust
mod fixture;

use suaegi_git::repo_probe::probe_repo;
use suaegi_git::runner::GitRunner;

#[tokio::test]
async fn detects_git_repo_and_head_branch() {
    let dir = tempfile::tempdir().unwrap();
    fixture::init_repo(dir.path());
    let probe = probe_repo(&GitRunner::new(), dir.path()).await.unwrap();
    assert!(probe.is_git_repo);
    assert_eq!(probe.head_branch.as_deref(), Some("main"));
}

#[tokio::test]
async fn non_repo_reports_false() {
    let dir = tempfile::tempdir().unwrap();
    let probe = probe_repo(&GitRunner::new(), dir.path()).await.unwrap();
    assert!(!probe.is_git_repo);
    assert_eq!(probe.head_branch, None);
}
```

- [ ] **Step 3: 테스트 실패 확인**

Run: `cargo test -p suaegi-git --test repo_probe_test`
Expected: 컴파일 에러

- [ ] **Step 4: 구현** (`src/repo_probe.rs`)

```rust
use crate::runner::{GitError, GitRunner};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoProbe {
    pub is_git_repo: bool,
    pub head_branch: Option<String>,
}

pub async fn probe_repo(runner: &GitRunner, path: &Path) -> Result<RepoProbe, GitError> {
    match runner.run(path, &["rev-parse", "--is-inside-work-tree"]).await {
        Ok(out) if out.stdout.trim() == "true" => {}
        // "not a git repository"는 에러가 아니라 정상적인 false 응답으로 취급
        Ok(_) | Err(GitError::Failed { .. }) => {
            return Ok(RepoProbe { is_git_repo: false, head_branch: None })
        }
        Err(e) => return Err(e),
    }
    let head = runner
        .run(path, &["symbolic-ref", "--short", "HEAD"])
        .await
        .ok()
        .map(|o| o.stdout.trim().to_string())
        .filter(|s| !s.is_empty());
    Ok(RepoProbe { is_git_repo: true, head_branch: head })
}
```

- [ ] **Step 5: 테스트 통과 확인**

Run: `cargo test -p suaegi-git --test repo_probe_test`
Expected: 2 passed

- [ ] **Step 6: Commit**

```bash
git add crates/suaegi-git/src/repo_probe.rs crates/suaegi-git/tests/fixture.rs crates/suaegi-git/tests/repo_probe_test.rs
git commit -m "feat(git): repo probe with head-branch detection and shared test fixture"
```

---

### Task 8: worktree 생성 (충돌 회피 + 롤백)

**Files:**
- Create: `crates/suaegi-git/src/worktree.rs`
- Test: `crates/suaegi-git/tests/worktree_test.rs`

**Interfaces:**
- Consumes: `GitRunner`(T5), `sanitize_worktree_name`/`candidate_names`(T6), fixture(T7)
- Produces:
  - `add_worktree(runner, repo_path, requested_name, base_ref, workspace_root) -> Result<CreatedWorktree, WorktreeError>`
  - `CreatedWorktree { path: PathBuf, branch: String, display_name: String }`
  - `WorktreeError { Git(GitError) | NoAvailableName | Io(std::io::Error) }`
  - 동작: 이름 새니타이즈 → `workspace_root/<repo_dir_name>/<name>` 후보 경로+동명 브랜치, 디렉토리/브랜치 충돌 시 다음 후보 → `git worktree add --no-track -b <branch> <path> <base>` (타임아웃 180s) → 실패 시 생성물 롤백

- [ ] **Step 1: 실패하는 테스트 작성** (`tests/worktree_test.rs`)

```rust
mod fixture;

use suaegi_git::runner::GitRunner;
use suaegi_git::worktree::{add_worktree, WorktreeError};

#[tokio::test]
async fn creates_worktree_with_new_branch() {
    let repo = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    fixture::init_repo(repo.path());
    let r = GitRunner::new();
    let created = add_worktree(&r, repo.path(), "Fix Bug!", "main", ws.path()).await.unwrap();
    assert_eq!(created.branch, "Fix-Bug");
    assert!(created.path.join("README.md").exists());
    let list = r.run(repo.path(), &["worktree", "list", "--porcelain"]).await.unwrap();
    assert!(list.stdout.contains(created.path.to_str().unwrap()));
}

#[tokio::test]
async fn name_collision_gets_numeric_suffix() {
    let repo = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    fixture::init_repo(repo.path());
    let r = GitRunner::new();
    let first = add_worktree(&r, repo.path(), "fix", "main", ws.path()).await.unwrap();
    let second = add_worktree(&r, repo.path(), "fix", "main", ws.path()).await.unwrap();
    assert_eq!(first.branch, "fix");
    assert_eq!(second.branch, "fix-2");
    assert_ne!(first.path, second.path);
}

#[tokio::test]
async fn bad_base_ref_fails_without_leftover_directory() {
    let repo = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    fixture::init_repo(repo.path());
    let r = GitRunner::new();
    let err = add_worktree(&r, repo.path(), "fix", "no-such-ref", ws.path()).await.unwrap_err();
    assert!(matches!(err, WorktreeError::Git(_)));
    // 롤백: workspace_root 아래에 잔여 디렉토리가 없어야 한다
    let repo_dir = ws.path().join(repo.path().file_name().unwrap());
    let leftover = std::fs::read_dir(&repo_dir).map(|d| d.count()).unwrap_or(0);
    assert_eq!(leftover, 0);
}
```

- [ ] **Step 2: 테스트 실패 확인**

Run: `cargo test -p suaegi-git --test worktree_test`
Expected: 컴파일 에러

- [ ] **Step 3: 구현** (`src/worktree.rs`)

```rust
use crate::runner::{GitError, GitOutput, GitRunner};
use crate::worktree_name::{candidate_names, sanitize_worktree_name};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// OneDrive placeholder 등으로 checkout이 스톨할 수 있어 넉넉히 (Orca 차용).
const WORKTREE_ADD_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Debug, Clone)]
pub struct CreatedWorktree {
    pub path: PathBuf,
    pub branch: String,
    pub display_name: String,
}

#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    #[error(transparent)]
    Git(#[from] GitError),
    #[error("no available worktree name after 100 attempts")]
    NoAvailableName,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

async fn branch_exists(runner: &GitRunner, repo: &Path, branch: &str) -> bool {
    runner
        .run(repo, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")])
        .await
        .is_ok()
}

pub async fn add_worktree(
    runner: &GitRunner,
    repo_path: &Path,
    requested_name: &str,
    base_ref: &str,
    workspace_root: &Path,
) -> Result<CreatedWorktree, WorktreeError> {
    let sanitized = sanitize_worktree_name(requested_name);
    let repo_dir_name = repo_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".to_string());
    let parent = workspace_root.join(&repo_dir_name);
    std::fs::create_dir_all(&parent)?;

    let mut chosen: Option<(String, PathBuf)> = None;
    for name in candidate_names(&sanitized) {
        let path = parent.join(&name);
        if path.exists() || branch_exists(runner, repo_path, &name).await {
            continue;
        }
        chosen = Some((name, path));
        break;
    }
    let (branch, path) = chosen.ok_or(WorktreeError::NoAvailableName)?;

    // --no-track: base가 remote ref일 때 미푸시 브랜치가 "behind"로 오보되는 것 방지 (Orca 차용)
    let path_str = path.to_string_lossy().into_owned();
    let result: Result<GitOutput, GitError> = runner
        .run_with_timeout(
            repo_path,
            &["worktree", "add", "--no-track", "-b", &branch, &path_str, base_ref],
            WORKTREE_ADD_TIMEOUT,
        )
        .await;

    if let Err(e) = result {
        // 롤백: 부분 생성물 제거 (디렉토리, worktree 등록, 브랜치)
        let _ = runner
            .run(repo_path, &["worktree", "remove", "--force", &path_str])
            .await;
        if path.exists() {
            let _ = std::fs::remove_dir_all(&path);
        }
        let _ = runner.run(repo_path, &["worktree", "prune"]).await;
        let _ = runner.run(repo_path, &["branch", "-D", &branch]).await;
        return Err(e.into());
    }

    Ok(CreatedWorktree { path, branch: branch.clone(), display_name: branch })
}
```

- [ ] **Step 4: 테스트 통과 확인**

Run: `cargo test -p suaegi-git --test worktree_test`
Expected: 3 passed

- [ ] **Step 5: Commit**

```bash
git add crates/suaegi-git/src/worktree.rs crates/suaegi-git/tests/worktree_test.rs
git commit -m "feat(git): worktree creation with collision suffixes and rollback"
```

---

### Task 9: worktree 리스팅 + 삭제

**Files:**
- Modify: `crates/suaegi-git/src/worktree.rs`
- Test: `crates/suaegi-git/tests/worktree_test.rs` (추가)

**Interfaces:**
- Consumes: Task 8의 모듈
- Produces:
  - `list_worktrees(runner, repo_path) -> Result<Vec<WorktreeEntry>, GitError>`
  - `WorktreeEntry { path: PathBuf, branch: Option<String>, head: Option<String>, is_main: bool }`
  - `remove_worktree(runner, repo_path, worktree_path, force: bool, delete_branch: Option<&str>) -> Result<(), WorktreeError>` — dirty worktree는 `force: true`일 때만 제거(자동 force 재시도 없음, 호출자가 결정), 브랜치 삭제는 worktree 제거와 분리 실행

- [ ] **Step 1: 실패하는 테스트 추가** (`tests/worktree_test.rs`)

```rust
use suaegi_git::worktree::{list_worktrees, remove_worktree};

#[tokio::test]
async fn list_includes_main_and_created_worktrees() {
    let repo = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    fixture::init_repo(repo.path());
    let r = GitRunner::new();
    let created = add_worktree(&r, repo.path(), "fix", "main", ws.path()).await.unwrap();
    let list = list_worktrees(&r, repo.path()).await.unwrap();
    assert_eq!(list.len(), 2);
    assert!(list[0].is_main);
    assert_eq!(list[1].branch.as_deref(), Some("fix"));
    assert_eq!(
        list[1].path.canonicalize().unwrap(),
        created.path.canonicalize().unwrap()
    );
}

#[tokio::test]
async fn remove_worktree_deletes_dir_and_optionally_branch() {
    let repo = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    fixture::init_repo(repo.path());
    let r = GitRunner::new();
    let created = add_worktree(&r, repo.path(), "fix", "main", ws.path()).await.unwrap();
    remove_worktree(&r, repo.path(), &created.path, false, Some("fix")).await.unwrap();
    assert!(!created.path.exists());
    let list = list_worktrees(&r, repo.path()).await.unwrap();
    assert_eq!(list.len(), 1);
    // 브랜치도 삭제됨
    let br = r.run(repo.path(), &["branch", "--list", "fix"]).await.unwrap();
    assert!(br.stdout.trim().is_empty());
}

#[tokio::test]
async fn remove_dirty_worktree_requires_force() {
    let repo = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    fixture::init_repo(repo.path());
    let r = GitRunner::new();
    let created = add_worktree(&r, repo.path(), "fix", "main", ws.path()).await.unwrap();
    std::fs::write(created.path.join("dirty.txt"), "x").unwrap();
    let err = remove_worktree(&r, repo.path(), &created.path, false, None).await;
    assert!(err.is_err());
    remove_worktree(&r, repo.path(), &created.path, true, None).await.unwrap();
    assert!(!created.path.exists());
}
```

- [ ] **Step 2: 테스트 실패 확인**

Run: `cargo test -p suaegi-git --test worktree_test`
Expected: 컴파일 에러

- [ ] **Step 3: 구현** (`src/worktree.rs`에 추가)

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub is_main: bool,
}

/// `git worktree list --porcelain` 파싱. 첫 엔트리가 main worktree.
pub async fn list_worktrees(
    runner: &GitRunner,
    repo_path: &Path,
) -> Result<Vec<WorktreeEntry>, GitError> {
    let out = runner.run(repo_path, &["worktree", "list", "--porcelain"]).await?;
    let mut entries = Vec::new();
    let mut current: Option<WorktreeEntry> = None;
    for line in out.stdout.lines() {
        if let Some(rest) = line.strip_prefix("worktree ") {
            if let Some(e) = current.take() {
                entries.push(e);
            }
            current = Some(WorktreeEntry {
                path: PathBuf::from(rest),
                branch: None,
                head: None,
                is_main: entries.is_empty(),
            });
        } else if let Some(rest) = line.strip_prefix("HEAD ") {
            if let Some(e) = current.as_mut() {
                e.head = Some(rest.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("branch ") {
            if let Some(e) = current.as_mut() {
                e.branch = Some(rest.trim_start_matches("refs/heads/").to_string());
            }
        }
    }
    if let Some(e) = current.take() {
        entries.push(e);
    }
    Ok(entries)
}

pub async fn remove_worktree(
    runner: &GitRunner,
    repo_path: &Path,
    worktree_path: &Path,
    force: bool,
    delete_branch: Option<&str>,
) -> Result<(), WorktreeError> {
    let path_str = worktree_path.to_string_lossy().into_owned();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&path_str);
    runner.run(repo_path, &args).await?;
    if let Some(branch) = delete_branch {
        // 브랜치 삭제는 worktree 제거와 분리 — 실패해도 worktree 제거는 유지 (Orca 차용)
        let _ = runner.run(repo_path, &["branch", "-D", branch]).await;
    }
    Ok(())
}
```

- [ ] **Step 4: 테스트 통과 확인**

Run: `cargo test -p suaegi-git --test worktree_test`
Expected: 6 passed (누적)

- [ ] **Step 5: Commit**

```bash
git add crates/suaegi-git/src/worktree.rs crates/suaegi-git/tests/worktree_test.rs
git commit -m "feat(git): worktree listing and removal with separate branch deletion"
```

---

### Task 10: base 대비 비교(diff) + 파일 diff + 상태

**Files:**
- Create: `crates/suaegi-git/src/compare.rs`
- Test: `crates/suaegi-git/tests/compare_test.rs`

**Interfaces:**
- Consumes: `GitRunner`(T5), fixture(T7), `add_worktree`(T8)
- Produces (Plan 5의 diff 패널이 사용):
  - `branch_compare(runner, worktree_path, base_ref) -> Result<BranchCompare, GitError>`
  - `BranchCompare { merge_base: String, ahead_count: u32, files: Vec<ChangedFile> }`
  - `ChangedFile { path: String, status: ChangeStatus, additions: Option<u32>, deletions: Option<u32> }` (`None` = 바이너리)
  - `ChangeStatus { Added, Modified, Deleted, Renamed { from: String }, Other(char) }`
  - `file_diff(runner, worktree_path, base_ref, file: &str) -> Result<String, GitError>` — unified patch
  - `working_tree_dirty(runner, worktree_path) -> Result<bool, GitError>`

- [ ] **Step 1: 실패하는 테스트 작성** (`tests/compare_test.rs`)

```rust
mod fixture;

use suaegi_git::compare::{branch_compare, file_diff, working_tree_dirty, ChangeStatus};
use suaegi_git::runner::GitRunner;
use suaegi_git::worktree::add_worktree;

async fn setup() -> (tempfile::TempDir, tempfile::TempDir, std::path::PathBuf) {
    let repo = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    fixture::init_repo(repo.path());
    let r = GitRunner::new();
    let created = add_worktree(&r, repo.path(), "feat", "main", ws.path()).await.unwrap();
    (repo, ws, created.path)
}

#[tokio::test]
async fn compare_reports_added_modified_deleted() {
    let (_repo, _ws, wt) = setup().await;
    std::fs::write(wt.join("new.txt"), "new\n").unwrap();
    std::fs::write(wt.join("README.md"), "changed\n").unwrap();
    fixture::run(&wt, &["add", "."]);
    fixture::run(&wt, &["commit", "-m", "change"]);

    let r = GitRunner::new();
    let cmp = branch_compare(&r, &wt, "main").await.unwrap();
    assert_eq!(cmp.ahead_count, 1);
    let mut paths: Vec<_> = cmp.files.iter().map(|f| f.path.as_str()).collect();
    paths.sort();
    assert_eq!(paths, vec!["README.md", "new.txt"]);
    let readme = cmp.files.iter().find(|f| f.path == "README.md").unwrap();
    assert_eq!(readme.status, ChangeStatus::Modified);
    assert_eq!(readme.additions, Some(1));
    assert_eq!(readme.deletions, Some(1));
}

#[tokio::test]
async fn file_diff_returns_unified_patch() {
    let (_repo, _ws, wt) = setup().await;
    std::fs::write(wt.join("README.md"), "changed\n").unwrap();
    fixture::run(&wt, &["add", "."]);
    fixture::run(&wt, &["commit", "-m", "change"]);

    let r = GitRunner::new();
    let patch = file_diff(&r, &wt, "main", "README.md").await.unwrap();
    assert!(patch.contains("-hello"));
    assert!(patch.contains("+changed"));
}

#[tokio::test]
async fn no_changes_yields_empty_file_list() {
    let (_repo, _ws, wt) = setup().await;
    let r = GitRunner::new();
    let cmp = branch_compare(&r, &wt, "main").await.unwrap();
    assert_eq!(cmp.ahead_count, 0);
    assert!(cmp.files.is_empty());
}

#[tokio::test]
async fn dirty_detection() {
    let (_repo, _ws, wt) = setup().await;
    let r = GitRunner::new();
    assert!(!working_tree_dirty(&r, &wt).await.unwrap());
    std::fs::write(wt.join("x.txt"), "x").unwrap();
    assert!(working_tree_dirty(&r, &wt).await.unwrap());
}
```

- [ ] **Step 2: 테스트 실패 확인**

Run: `cargo test -p suaegi-git --test compare_test`
Expected: 컴파일 에러

- [ ] **Step 3: 구현** (`src/compare.rs`)

```rust
use crate::runner::{GitError, GitRunner};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeStatus {
    Added,
    Modified,
    Deleted,
    Renamed { from: String },
    Other(char),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: String,
    pub status: ChangeStatus,
    pub additions: Option<u32>,
    pub deletions: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchCompare {
    pub merge_base: String,
    pub ahead_count: u32,
    pub files: Vec<ChangedFile>,
}

pub async fn branch_compare(
    runner: &GitRunner,
    worktree_path: &Path,
    base_ref: &str,
) -> Result<BranchCompare, GitError> {
    let mb = runner
        .run(worktree_path, &["merge-base", "HEAD", base_ref])
        .await?
        .stdout
        .trim()
        .to_string();
    let ahead = runner
        .run(worktree_path, &["rev-list", "--count", &format!("{mb}..HEAD")])
        .await?
        .stdout
        .trim()
        .parse::<u32>()
        .unwrap_or(0);

    // -M: rename 감지 (Orca와 동일)
    let range = format!("{mb}..HEAD");
    let name_status = runner
        .run(worktree_path, &["diff", "--name-status", "-M", &range])
        .await?;
    let numstat = runner
        .run(worktree_path, &["diff", "--numstat", "-M", &range])
        .await?;

    // numstat: "adds\tdels\tpath" ("-"는 바이너리). rename 행("from => to" 표기)은
    // 경로 매칭이 어긋나 (None, None)으로 남는다 — MVP 허용 범위.
    let mut counts: HashMap<String, (Option<u32>, Option<u32>)> = HashMap::new();
    for line in numstat.stdout.lines() {
        let mut parts = line.split('\t');
        let (Some(a), Some(d), Some(path)) = (parts.next(), parts.next(), parts.last()) else {
            continue;
        };
        counts.insert(path.to_string(), (a.parse().ok(), d.parse().ok()));
    }

    let mut files = Vec::new();
    for line in name_status.stdout.lines() {
        let mut parts = line.split('\t');
        let Some(code) = parts.next() else { continue };
        let status_char = code.chars().next().unwrap_or('?');
        let (status, path) = match status_char {
            'R' => {
                let from = parts.next().unwrap_or_default().to_string();
                let to = parts.next().unwrap_or_default().to_string();
                (ChangeStatus::Renamed { from }, to)
            }
            'A' => (ChangeStatus::Added, parts.next().unwrap_or_default().to_string()),
            'M' => (ChangeStatus::Modified, parts.next().unwrap_or_default().to_string()),
            'D' => (ChangeStatus::Deleted, parts.next().unwrap_or_default().to_string()),
            c => (ChangeStatus::Other(c), parts.next().unwrap_or_default().to_string()),
        };
        let (additions, deletions) = counts.get(&path).copied().unwrap_or((None, None));
        files.push(ChangedFile { path, status, additions, deletions });
    }

    Ok(BranchCompare { merge_base: mb, ahead_count: ahead, files })
}

pub async fn file_diff(
    runner: &GitRunner,
    worktree_path: &Path,
    base_ref: &str,
    file: &str,
) -> Result<String, GitError> {
    let mb = runner
        .run(worktree_path, &["merge-base", "HEAD", base_ref])
        .await?
        .stdout
        .trim()
        .to_string();
    let range = format!("{mb}..HEAD");
    Ok(runner
        .run(worktree_path, &["diff", "-M", &range, "--", file])
        .await?
        .stdout)
}

pub async fn working_tree_dirty(
    runner: &GitRunner,
    worktree_path: &Path,
) -> Result<bool, GitError> {
    let out = runner
        .run(worktree_path, &["status", "--porcelain"])
        .await?;
    Ok(!out.stdout.trim().is_empty())
}
```

- [ ] **Step 4: 테스트 통과 확인**

Run: `cargo test -p suaegi-git --test compare_test`
Expected: 4 passed

- [ ] **Step 5: 전체 검증 + Commit**

Run: `cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check`
Expected: 전부 통과

```bash
git add crates/suaegi-git/src/compare.rs crates/suaegi-git/tests/compare_test.rs
git commit -m "feat(git): branch compare, file diff, and dirty detection"
```

---

## 후속 플랜 로드맵 (이 문서 범위 아님)

- Plan 2: `suaegi-term` — portable-pty 세션 + alacritty_terminal 그리드 + 에이전트 레지스트리/폴링
- Plan 3: `suaegi-app` 셸 — iced 앱 골격, 사이드바, 레이아웃 트리
- Plan 4: 터미널 위젯(iced_term 포크) + 워크벤치 배선
- Plan 5: 에이전트 hook 서버 + diff 패널 + 세션 복원

각 플랜은 직전 플랜 완료 후 작성한다 (iced API 등 실측이 필요한 부분의 정밀도 확보).
