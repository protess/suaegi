# Suaegi Plan 1: Workspace + suaegi-core + suaegi-git Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rust 워크스페이스를 세우고, 도메인 모델+영속화(`suaegi-core`)와 git CLI 실행 계층(`suaegi-git`)을 UI 없이 완전히 테스트된 상태로 완성한다.

**Architecture:** 4-crate workspace 중 하위 2개를 만든다. `suaegi-core`는 순수 데이터(도메인 타입 + JSON 영속화, 원자적 쓰기 + 롤링 백업 + 손상 폴백 + 미래 스키마 가드). `suaegi-git`은 모든 git 작업을 git CLI subprocess로 실행(라이브러리 없음, Orca 검증 방식)하고 구조화된 에러를 돌려준다. 의존 방향: `git → core`, 역방향 금지.

**Tech Stack:** Rust 1.94 (edition 2021), serde/serde_json, thiserror, tokio(process/time/io-util), dirs, tempfile(영속화 원자적 쓰기용), libc(unix, 프로세스 그룹 킬). dev-deps: tempfile.

**Spec:** `docs/superpowers/specs/2026-07-20-suaegi-mvp-design.md`

## Global Constraints

- Rust edition 2021, `rust-version = "1.94"` 선언 (로컬 툴체인은 `.tool-versions`로 고정됨)
- git 작업은 전부 CLI shell-out; git2/gitoxide 금지
- git 실행 env: `LC_ALL=C`, `GIT_TERMINAL_PROMPT=0` 강제
- worktree add 타임아웃 180초, 그 외 git 명령 기본 30초. 타임아웃 시 Unix는 **프로세스 그룹 전체** SIGKILL 후 reap (git hook/LFS 자식 잔존 방지); Windows는 git 프로세스만 킬 (job object는 post-MVP)
- 영속화: 단일 JSON `<config_dir>/suaegi/data.json`, 원자적 쓰기(같은 디렉토리 NamedTempFile + persist), 롤링 백업 5개(`data.json.bak.0..4`, ≥1시간 간격), 읽기 실패 시 백업 폴백 → 최후엔 default (크래시 금지). **미래 스키마 감지 시 저장 차단** (다운그레이드된 앱이 신버전 데이터를 덮어쓰지 못하게)
- 파일/모듈 이름에 `utils`/`helpers` 금지 (구체적 도메인 이름 사용)
- 모든 커밋은 테스트 통과 상태에서만
- **명시적 MVP 제약 (검토 후 수용된 한계):**
  - 단일 앱 인스턴스 가정. 외부 프로세스와의 worktree/브랜치 생성 경합(TOCTOU)은 방어하지 않고 에러로 표면화만 한다.
  - 비UTF-8 파일 경로는 미지원(`to_string_lossy` 사용). 앱이 생성하는 경로는 항상 UTF-8.
  - 저장 debounce는 core가 아니라 앱 레이어(Plan 3)에서 담당한다. `Store::save`는 동기 API이며 UI 스레드에서 직접 부르지 않고 `spawn_blocking`으로 감싼다(Plan 3).
  - git ≥ 2.28 가정 (`git init -b`는 테스트 픽스처 전용이지만, 지원 버전 하한은 추후 startup check로 명시).
  - 스키마 마이그레이션은 v2 스키마가 생길 때 도입 (v1이 최초 버전이므로 지금은 미래 버전 가드만).

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
rust-version = "1.94"
license = "MIT"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
tokio = { version = "1", features = ["process", "time", "rt-multi-thread", "macros", "io-util"] }
dirs = "6"
tempfile = "3"
libc = "0.2"
```

- [ ] **Step 2: suaegi-core 크레이트 생성**

`crates/suaegi-core/Cargo.toml`:
```toml
[package]
name = "suaegi-core"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
dirs = { workspace = true }
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
rust-version.workspace = true
license.workspace = true

[dependencies]
suaegi-core = { path = "../suaegi-core" }
serde = { workspace = true }
thiserror = { workspace = true }
tokio = { workspace = true }

[target.'cfg(unix)'.dependencies]
libc = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```

`crates/suaegi-git/src/lib.rs`:
```rust
pub mod runner;
pub mod refname;
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
  - `Repo { id, path: PathBuf, display_name: String, worktree_base_ref: Option<String> }` + `Repo::from_path(&Path) -> io::Result<Repo>` — **앱 코드의 표준 생성 경로** (canonicalize로 ID 정규화; serde 역직렬화는 이미 정규화된 데이터를 신뢰)
  - `Worktree { id, repo_id, path: PathBuf, branch: String, display_name: String, created_with_agent: Option<String>, created_at_unix_ms: u64 }`
  - `SessionState { active_worktree_id: Option<WorktreeId> }` — v1 최소 골격. 탭 레이아웃 등은 Plan 3에서 `#[serde(default)]` 필드로 확장 (스키마 마이그레이션 없이 진화 가능하게 지금 자리를 만들어 둔다)
  - `Settings { workspace_root: PathBuf }` + `Settings::default_with_home(home: &Path)`
  - `PersistedState { schema_version: u32, repos, worktrees, session: SessionState, settings }` + `Default`
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
            session: SessionState {
                active_worktree_id: Some(WorktreeId("/tmp/ws/demo/fix-bug".into())),
            },
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
        assert_eq!(d.session, SessionState::default());
    }

    #[test]
    fn default_settings_places_workspace_root_under_home() {
        let s = Settings::default_with_home(&PathBuf::from("/home/u"));
        assert_eq!(s.workspace_root, PathBuf::from("/home/u/suaegi-workspaces"));
    }

    #[test]
    fn repo_from_path_canonicalizes_id() {
        let dir = tempfile::tempdir().unwrap();
        // 상대 경로 요소가 섞여도 동일 디렉토리는 동일 ID가 되어야 한다
        let messy = dir.path().join("sub").join("..");
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        let a = Repo::from_path(dir.path()).unwrap();
        let b = Repo::from_path(&messy).unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(a.path, b.path);
        assert!(!a.display_name.is_empty());
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

impl Repo {
    /// 앱 코드의 표준 Repo 생성 경로. canonicalize로 심볼릭 링크/상대 경로/대소문자
    /// 변형이 서로 다른 ID를 만들지 못하게 한다. (serde 역직렬화는 과거에 이 경로로
    /// 만들어 저장한 데이터를 다시 읽는 것이므로 정규화를 반복하지 않는다)
    pub fn from_path(path: &Path) -> std::io::Result<Self> {
        let canonical = path.canonicalize()?;
        let display_name = canonical
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "repo".to_string());
        Ok(Self {
            id: RepoId(canonical.to_string_lossy().into_owned()),
            path: canonical,
            display_name,
            worktree_base_ref: None,
        })
    }
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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionState {
    #[serde(default)]
    pub active_worktree_id: Option<WorktreeId>,
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
    #[serde(default)]
    pub session: SessionState,
    pub settings: Settings,
}

impl Default for PersistedState {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            schema_version: SCHEMA_VERSION,
            repos: Vec::new(),
            worktrees: Vec::new(),
            session: SessionState::default(),
            settings: Settings::default_with_home(&home),
        }
    }
}
```

- [ ] **Step 4: 테스트 통과 확인**

Run: `cargo test -p suaegi-core`
Expected: 4 passed

- [ ] **Step 5: Commit**

```bash
git add crates/suaegi-core/src/domain.rs crates/suaegi-core/Cargo.toml
git commit -m "feat(core): domain model with canonical repo IDs and session skeleton"
```

---

### Task 3: 영속화 — 원자적 저장/로드 + 동일 내용 스킵

**Files:**
- Create: `crates/suaegi-core/src/persistence.rs`
- Test: 같은 파일 하단 `#[cfg(test)]`

**Interfaces:**
- Produces:
  - `Store::new(data_file: PathBuf) -> Store`
  - `Store::load(&mut self) -> LoadOutcome` — panic 없음. 본파일 로드 성공 시 내부 해시를 시드해 재시작 직후 동일 내용 재저장을 스킵. **폴백 경로에서는 해시를 리셋**해 복구 상태 저장이 스킵되지 않게 함 (손상 파일이 다음 저장으로 즉시 복구되도록)
  - `LoadOutcome { state: PersistedState, source: LoadSource }`
  - `LoadSource { MainFile, Backup(usize), Default }` — 호출자(UI)가 "백업에서 복구됨" 경고를 띄울 수 있도록 손실 여부를 구분해 전달
  - `Store::save(&mut self, state: &PersistedState) -> Result<SaveOutcome, PersistenceError>`
  - `enum SaveOutcome { Written, SkippedUnchanged }`
  - `PersistenceError` (thiserror, `Io` + `Serialize` — Task 4에서 `FutureSchemaGuard` 추가)
  - 원자적 쓰기: `tempfile::NamedTempFile::new_in(parent)` + `persist()` — 고정 이름 tmp 파일의 동시성 문제 회피

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
        let loaded = store.load();
        assert_eq!(loaded.state, state);
        assert_eq!(loaded.source, LoadSource::MainFile);
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
    fn load_seeds_hash_so_fresh_store_skips_identical_save() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("data.json");
        let state = sample_state("a");
        Store::new(file.clone()).save(&state).unwrap();
        // 재시작 시뮬레이션: 새 Store 인스턴스
        let mut fresh = Store::new(file);
        fresh.load();
        assert!(matches!(fresh.save(&state).unwrap(), SaveOutcome::SkippedUnchanged));
    }

    #[test]
    fn fallback_load_resets_hash_so_recovery_state_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("data.json");
        let mut store = Store::new(file.clone());
        let state = PersistedState::default();
        store.save(&state).unwrap();
        // 본파일 손상 → load는 default 폴백 → 같은 default를 저장해도
        // 손상 파일을 실제로 복구(Written)해야 한다. 스킵되면 손상이 영구화된다.
        std::fs::write(&file, "corrupt").unwrap();
        let loaded = store.load();
        assert_eq!(loaded.source, LoadSource::Default);
        assert!(matches!(store.save(&loaded.state).unwrap(), SaveOutcome::Written));
        // 복구 확인
        assert_eq!(store.load().source, LoadSource::MainFile);
    }

    #[test]
    fn load_missing_file_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::new(dir.path().join("nope/data.json"));
        let loaded = store.load();
        assert_eq!(loaded.state, PersistedState::default());
        assert_eq!(loaded.source, LoadSource::Default);
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadSource {
    MainFile,
    Backup(usize),
    Default,
}

#[derive(Debug)]
pub struct LoadOutcome {
    pub state: PersistedState,
    pub source: LoadSource,
}

pub struct Store {
    data_file: PathBuf,
    last_written_hash: Option<u64>,
}

fn content_hash(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

impl Store {
    pub fn new(data_file: PathBuf) -> Self {
        Self { data_file, last_written_hash: None }
    }

    pub fn data_file(&self) -> &PathBuf {
        &self.data_file
    }

    pub fn load(&mut self) -> LoadOutcome {
        if let Ok(text) = fs::read_to_string(&self.data_file) {
            if let Ok(state) = serde_json::from_str::<PersistedState>(&text) {
                // 재시작 직후 동일 상태 재저장을 스킵할 수 있도록 해시 시드
                self.last_written_hash = Some(content_hash(&text));
                return LoadOutcome { state, source: LoadSource::MainFile };
            }
        }
        self.load_from_backups()
    }

    // Task 4에서 백업 폴백 구현. 이 시점엔 default만.
    fn load_from_backups(&mut self) -> LoadOutcome {
        // 폴백 = 본파일이 신뢰 불가. 해시를 리셋해 복구 상태 저장이
        // SkippedUnchanged로 무시되지 않게 한다 (손상 영구화 방지).
        self.last_written_hash = None;
        LoadOutcome { state: PersistedState::default(), source: LoadSource::Default }
    }

    pub fn save(&mut self, state: &PersistedState) -> Result<SaveOutcome, PersistenceError> {
        let json = serde_json::to_string_pretty(state)?;
        let hash = content_hash(&json);
        if self.last_written_hash == Some(hash) {
            return Ok(SaveOutcome::SkippedUnchanged);
        }
        let parent = self
            .data_file
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        fs::create_dir_all(&parent)?;
        // 원자적 쓰기: 같은 디렉토리에 임의 이름 temp 작성 후 rename(persist).
        // (Rust std의 rename은 Windows에서도 MOVEFILE_REPLACE_EXISTING으로 기존 파일 교체)
        let mut tmp = tempfile::NamedTempFile::new_in(&parent)?;
        tmp.write_all(json.as_bytes())?;
        tmp.as_file().sync_all()?;
        tmp.persist(&self.data_file).map_err(|e| e.error)?;
        self.last_written_hash = Some(hash);
        Ok(SaveOutcome::Written)
    }
}
```

- [ ] **Step 4: 테스트 통과 확인**

Run: `cargo test -p suaegi-core`
Expected: 전체 passed (Task 2의 4개 + 신규 6개)

- [ ] **Step 5: Commit**

```bash
git add crates/suaegi-core/src/persistence.rs
git commit -m "feat(core): atomic JSON store with load-source reporting and unchanged-skip"
```

---

### Task 4: 영속화 — 롤링 백업 + 손상 폴백 + 미래 스키마 가드

**Files:**
- Modify: `crates/suaegi-core/src/persistence.rs`

**Interfaces:**
- Consumes: Task 3의 `Store`
- Produces:
  - 저장 성공 경로에서 백업 회전: `data.json.bak.0`(최신)..`.bak.4`, 직전 백업이 1시간 이내면 회전 생략. 회전은 새 파일 rename 직전에 수행 — 회전 후 rename이 실패해도 main 파일은 옛 내용 그대로이고 bak.0 == main이므로 데이터 손실 없음
  - `Store::load()` 폴백: 본파일 파싱 실패 시 `.bak.0..4` 순서로, 전부 실패 시 default
  - **미래 스키마 가드**: 본파일의 `schema_version > SCHEMA_VERSION`이면 백업 폴백하되 `Store`에 가드 플래그를 세운다. 가드 상태에서 `save()`는 `PersistenceError::FutureSchemaGuard` — 다운그레이드된 앱의 autosave가 신버전 데이터를 덮어쓰는 사고 방지. 사용자가 명시적으로 `Store::override_future_schema_guard()`를 호출해야 저장 재개 (Plan 3의 UI가 "이 데이터는 더 새 버전 앱이 만들었습니다" 대화상자에서 호출)
  - `LoadSource`에 손상 폴백과 미래 스키마 폴백 구분은 두지 않고 가드 플래그(`Store::future_schema_guarded()`)로 노출
  - 백업 간격 테스트 훅은 `#[cfg(test)]` 비공개 메서드

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
        let loaded = store.load();
        assert_eq!(loaded.state, v1);
        assert_eq!(loaded.source, LoadSource::Backup(0));
        assert!(!store.future_schema_guarded());
    }

    #[test]
    fn future_schema_blocks_saves_until_overridden() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("data.json");
        let mut store = Store::new(file.clone());
        store.set_backup_min_interval(Duration::ZERO);
        let v1 = sample_state("v1");
        store.save(&v1).unwrap();
        store.save(&sample_state("v2")).unwrap();
        // 미래 버전 앱이 쓴 파일 시뮬레이션
        let mut future = sample_state("future");
        future.schema_version = SCHEMA_VERSION + 1;
        std::fs::write(&file, serde_json::to_string(&future).unwrap()).unwrap();

        let loaded = store.load();
        assert_eq!(loaded.state, v1); // 백업으로 폴백은 하되
        assert!(store.future_schema_guarded()); // 가드가 선다
        // 가드 중 저장은 거부 — 신버전 데이터 덮어쓰기 방지
        assert!(matches!(
            store.save(&loaded.state),
            Err(PersistenceError::FutureSchemaGuard)
        ));
        // 명시적 해제 후에만 저장 가능
        store.override_future_schema_guard();
        assert!(matches!(store.save(&loaded.state).unwrap(), SaveOutcome::Written));
    }

    #[test]
    fn corrupt_main_and_backups_return_default() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("data.json");
        let mut store = Store::new(file.clone());
        store.set_backup_min_interval(Duration::ZERO);
        store.save(&sample_state("a")).unwrap();
        store.save(&sample_state("b")).unwrap();
        std::fs::write(&file, "bad").unwrap();
        std::fs::write(dir.path().join("data.json.bak.0"), "also bad").unwrap();
        let loaded = store.load();
        assert_eq!(loaded.state, PersistedState::default());
        assert_eq!(loaded.source, LoadSource::Default);
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
Expected: `set_backup_min_interval`/`future_schema_guarded` 미정의 컴파일 에러

- [ ] **Step 3: 구현**

`PersistenceError`에 variant 추가:

```rust
#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialize: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("data file was written by a newer app version; saving is blocked")]
    FutureSchemaGuard,
}
```

`Store` 수정 (기존 구조체/함수 대체):

```rust
use crate::domain::SCHEMA_VERSION;
use std::time::{Duration, SystemTime};

const BACKUP_SLOTS: usize = 5;

pub struct Store {
    data_file: PathBuf,
    last_written_hash: Option<u64>,
    backup_min_interval: Duration,
    future_schema_guard: bool,
}

impl Store {
    pub fn new(data_file: PathBuf) -> Self {
        Self {
            data_file,
            last_written_hash: None,
            backup_min_interval: Duration::from_secs(3600),
            future_schema_guard: false,
        }
    }

    pub fn future_schema_guarded(&self) -> bool {
        self.future_schema_guard
    }

    /// 사용자가 "구버전 앱으로 계속 진행(신버전 데이터 덮어쓰기)"을 명시적으로
    /// 선택했을 때만 호출한다 (Plan 3 UI).
    pub fn override_future_schema_guard(&mut self) {
        self.future_schema_guard = false;
    }

    // 테스트 전용 훅 — 공개 API가 아니므로 pub 없음 (in-module 테스트에서만 접근)
    #[cfg(test)]
    fn set_backup_min_interval(&mut self, interval: Duration) {
        self.backup_min_interval = interval;
    }

    fn backup_path(&self, slot: usize) -> PathBuf {
        let name = self.data_file.file_name().unwrap_or_default().to_string_lossy();
        self.data_file.with_file_name(format!("{name}.bak.{slot}"))
    }

    /// schema_version만 먼저 확인 — 미래 스키마 JSON은 전체 구조가 파싱되더라도
    /// 신뢰하면 안 되기 때문. 반환: Ok(state) | Err(true)=미래 스키마 | Err(false)=손상
    fn parse_trusted(text: &str) -> Result<PersistedState, bool> {
        #[derive(serde::Deserialize)]
        struct VersionProbe {
            #[serde(default)]
            schema_version: u32,
        }
        let probe: VersionProbe = serde_json::from_str(text).map_err(|_| false)?;
        if probe.schema_version > SCHEMA_VERSION {
            return Err(true);
        }
        serde_json::from_str::<PersistedState>(text).map_err(|_| false)
    }

    fn load_from_backups(&mut self) -> LoadOutcome {
        self.last_written_hash = None;
        for slot in 0..BACKUP_SLOTS {
            if let Ok(text) = fs::read_to_string(self.backup_path(slot)) {
                if let Ok(state) = Self::parse_trusted(&text) {
                    return LoadOutcome { state, source: LoadSource::Backup(slot) };
                }
            }
        }
        LoadOutcome { state: PersistedState::default(), source: LoadSource::Default }
    }

    /// 본파일을 .bak.0으로 복사하고 기존 백업들을 한 칸씩 뒤로. 직전 백업이
    /// min_interval 이내면 생략 (Orca의 ≥1h 간격 패턴). 미래 mtime(시계 역행)은
    /// "오래됨"으로 취급해 회전이 영구 정지하지 않게 한다.
    fn rotate_backups(&self) -> Result<(), PersistenceError> {
        if !self.data_file.exists() {
            return Ok(());
        }
        let bak0 = self.backup_path(0);
        if let Ok(modified) = fs::metadata(&bak0).and_then(|m| m.modified()) {
            match SystemTime::now().duration_since(modified) {
                Ok(age) if age < self.backup_min_interval => return Ok(()),
                Ok(_) | Err(_) => {} // 오래됐거나 미래 mtime → 회전 진행
            }
        }
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

`load()`와 `save()` 수정:

```rust
    pub fn load(&mut self) -> LoadOutcome {
        self.future_schema_guard = false;
        if let Ok(text) = fs::read_to_string(&self.data_file) {
            match Self::parse_trusted(&text) {
                Ok(state) => {
                    self.last_written_hash = Some(content_hash(&text));
                    return LoadOutcome { state, source: LoadSource::MainFile };
                }
                Err(is_future) => {
                    if is_future {
                        self.future_schema_guard = true;
                    }
                }
            }
        }
        self.load_from_backups()
    }
```

`save()` 첫 줄에 가드 확인 추가, `tmp.persist(...)` 직전에 회전 추가:

```rust
    pub fn save(&mut self, state: &PersistedState) -> Result<SaveOutcome, PersistenceError> {
        if self.future_schema_guard {
            return Err(PersistenceError::FutureSchemaGuard);
        }
        // ... (기존 해시 비교/직렬화 로직 동일) ...
        self.rotate_backups()?;
        tmp.persist(&self.data_file).map_err(|e| e.error)?;
        // ...
    }
```

주의: `load_from_backups`의 해시 리셋(Task 3)은 그대로 유지된다.

- [ ] **Step 4: 테스트 통과 확인**

Run: `cargo test -p suaegi-core`
Expected: 전체 passed (누적 15개)

- [ ] **Step 5: Commit**

```bash
git add crates/suaegi-core/src/persistence.rs
git commit -m "feat(core): rolling backups, corrupt fallback, future-schema save guard"
```

---

### Task 5: suaegi-git — GitRunner (CLI 실행 계층)

**Files:**
- Create: `crates/suaegi-git/src/runner.rs`
- Test: `crates/suaegi-git/tests/runner_test.rs`

**Interfaces:**
- Produces:
  - `GitRunner::new() -> GitRunner` (PATH의 `git` 사용)
  - `GitRunner::run(&self, cwd, args) -> Result<GitOutput, GitError>` — 기본 30초 타임아웃
  - `GitRunner::run_with_timeout(&self, cwd, args, timeout) -> Result<GitOutput, GitError>`
  - `GitRunner::run_expecting(&self, cwd, args, extra_ok_codes: &[i32]) -> Result<GitOutput, GitError>` — 특정 비제로 exit도 성공으로 (예: `diff --no-index`의 1)
  - `GitOutput { stdout: String, stderr: String, code: i32 }`
  - `GitError { Io(std::io::Error) | Timeout { args } | Failed { args, code, stderr } | Parse { args, detail } }` — `Io`는 스폰/파이프/대기 전반의 IO 실패
  - 모든 실행에 env `LC_ALL=C`, `GIT_TERMINAL_PROMPT=0` 주입 — 이후 태스크 전부 이 runner만 사용
  - 타임아웃 시: Unix는 프로세스 그룹(`process_group(0)` + `kill(-pid, SIGKILL)`) 전체를 죽이고 reap — git이 스폰한 hook/LFS/credential helper 자식가 워크트리에 계속 쓰는 것 방지. Windows는 git 프로세스만 킬(문서화된 MVP 한계)

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
async fn run_expecting_accepts_listed_codes() {
    let r = GitRunner::new();
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    std::fs::write(&a, "1\n").unwrap();
    std::fs::write(&b, "2\n").unwrap();
    // --no-index는 차이가 있으면 exit 1 — extra_ok로 수용
    let out = r
        .run_expecting(
            dir.path(),
            &["diff", "--no-index", "--", a.to_str().unwrap(), b.to_str().unwrap()],
            &[1],
        )
        .await
        .unwrap();
    assert_eq!(out.code, 1);
    assert!(out.stdout.contains("-1"));
    assert!(out.stdout.contains("+2"));
}

// `sleep`은 POSIX 전용이므로 Unix에서만 실행. Windows CI에는 별도 타임아웃
// 테스트를 추가할 때까지 이 케이스를 건너뛴다.
#[cfg(unix)]
#[tokio::test]
async fn timeout_kills_process_group_including_descendants() {
    let r = GitRunner::new();
    let dir = tempfile::tempdir().unwrap();
    // 셸 alias가 자식(sh)과 손자(sleep)를 만든다. 타임아웃 후 손자까지 죽어야 한다.
    let marker = format!("suaegi-test-{}", std::process::id());
    let alias = format!("alias.zzz=!sleep 300 & echo $! > {marker}.pid; wait");
    let err = r
        .run_with_timeout(
            dir.path(),
            &["-c", &alias, "zzz"],
            Duration::from_millis(300),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, GitError::Timeout { .. }));
    // 프로세스 그룹 킬이 전파될 시간을 잠깐 주고 손자 생존 여부 확인
    tokio::time::sleep(Duration::from_millis(200)).await;
    if let Ok(pid_text) = std::fs::read_to_string(dir.path().join(format!("{marker}.pid"))) {
        let pid: i32 = pid_text.trim().parse().unwrap();
        // kill(pid, 0) == -1 (ESRCH) 이어야 함 = 이미 죽음
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        assert!(!alive, "descendant sleep survived the timeout kill");
    }
}
```

`tests`에서 `libc`를 쓰므로 `crates/suaegi-git/Cargo.toml`의 dev-dependencies에도 추가:
```toml
[target.'cfg(unix)'.dev-dependencies]
libc = { workspace = true }
```

- [ ] **Step 2: 테스트 실패 확인**

Run: `cargo test -p suaegi-git --test runner_test`
Expected: 컴파일 에러

- [ ] **Step 3: 구현** (`src/runner.rs`)

```rust
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct GitOutput {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("git io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("git {args} timed out")]
    Timeout { args: String },
    #[error("git {args} failed (code {code:?}): {stderr}")]
    Failed { args: String, code: Option<i32>, stderr: String },
    #[error("git {args} produced unparseable output: {detail}")]
    Parse { args: String, detail: String },
}

#[derive(Debug, Clone, Default)]
pub struct GitRunner;

/// Unix: git이 스폰한 hook/LFS/credential helper까지 함께 죽도록 프로세스 그룹
/// 전체에 SIGKILL. Windows: git 프로세스만 (job object는 post-MVP 한계).
fn kill_process_tree(child: &mut Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    let _ = child.start_kill();
}

impl GitRunner {
    pub fn new() -> Self {
        Self
    }

    pub async fn run(&self, cwd: &Path, args: &[&str]) -> Result<GitOutput, GitError> {
        self.run_full(cwd, args, DEFAULT_TIMEOUT, &[]).await
    }

    pub async fn run_with_timeout(
        &self,
        cwd: &Path,
        args: &[&str],
        timeout: Duration,
    ) -> Result<GitOutput, GitError> {
        self.run_full(cwd, args, timeout, &[]).await
    }

    pub async fn run_expecting(
        &self,
        cwd: &Path,
        args: &[&str],
        extra_ok_codes: &[i32],
    ) -> Result<GitOutput, GitError> {
        self.run_full(cwd, args, DEFAULT_TIMEOUT, extra_ok_codes).await
    }

    async fn run_full(
        &self,
        cwd: &Path,
        args: &[&str],
        timeout: Duration,
        extra_ok_codes: &[i32],
    ) -> Result<GitOutput, GitError> {
        let args_str = args.join(" ");
        let mut cmd = Command::new("git");
        cmd.args(args)
            .current_dir(cwd)
            // 파서가 항상 영어 출력을 보도록; 인증 프롬프트로 행 걸리지 않도록
            .env("LC_ALL", "C")
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        cmd.process_group(0); // 타임아웃 시 그룹 전체 킬 가능하게

        let mut child = cmd.spawn()?;
        let mut stdout_pipe = child.stdout.take();
        let mut stderr_pipe = child.stderr.take();
        let mut out = Vec::new();
        let mut err = Vec::new();

        let waited = tokio::time::timeout(timeout, async {
            // stdout/stderr 동시 드레인 — 순차로 읽으면 반대쪽 파이프가 가득 차
            // 자식이 블록되는 교착 가능
            let read_out = async {
                if let Some(s) = stdout_pipe.as_mut() {
                    s.read_to_end(&mut out).await?;
                }
                Ok::<_, std::io::Error>(())
            };
            let read_err = async {
                if let Some(s) = stderr_pipe.as_mut() {
                    s.read_to_end(&mut err).await?;
                }
                Ok::<_, std::io::Error>(())
            };
            let (status, _, _) = tokio::try_join!(child.wait(), read_out, read_err)?;
            Ok::<_, std::io::Error>(status)
        })
        .await;

        let status = match waited {
            Err(_) => {
                kill_process_tree(&mut child);
                let _ = child.wait().await; // 좀비 회수
                return Err(GitError::Timeout { args: args_str });
            }
            Ok(result) => result?,
        };

        let stdout = String::from_utf8_lossy(&out).into_owned();
        let stderr = String::from_utf8_lossy(&err).into_owned();
        let code = status.code().unwrap_or(-1);
        if !status.success() && !extra_ok_codes.contains(&code) {
            return Err(GitError::Failed {
                args: args_str,
                code: status.code(),
                stderr,
            });
        }
        Ok(GitOutput { stdout, stderr, code })
    }
}
```

- [ ] **Step 4: 테스트 통과 확인**

Run: `cargo test -p suaegi-git --test runner_test && cargo clippy -p suaegi-git -- -D warnings`
Expected: 4 passed (Unix; Windows에선 3), clippy 경고 0

- [ ] **Step 5: Commit**

```bash
git add crates/suaegi-git/src/runner.rs crates/suaegi-git/tests/runner_test.rs crates/suaegi-git/Cargo.toml
git commit -m "feat(git): GitRunner with process-group timeout kill and expected-code support"
```

---

### Task 6: ref 검증 + worktree 이름 새니타이즈 + 충돌 후보

**Files:**
- Create: `crates/suaegi-git/src/refname.rs`
- Create: `crates/suaegi-git/src/worktree_name.rs`
- Test: 각 파일 하단 `#[cfg(test)]`

**Interfaces:**
- Produces:
  - `refname::validate_user_ref(r: &str) -> Result<(), GitError>` — 빈 문자열/`-` 시작(옵션 주입) 거부. **base_ref를 받는 모든 공개 함수가 사용** (worktree add, merge-base, diff)
  - `sanitize_worktree_name(input: &str) -> String` — 유니코드 문자/숫자 유지, 그 외 `-` 치환, 연속 `-` 축약, 양끝 `-`/`.` 제거, 빈 결과면 `"workspace"`, 최대 60자, **Windows 예약어(CON/PRN/AUX/NUL/COM1-9/LPT1-9)는 `-ws` 접미사로 회피**. 출력은 항상 `[문자|숫자|-]+` 형태이므로 유효한 git 브랜치명이자 유효한 디렉토리명
  - `candidate_names(base: &str) -> impl Iterator<Item = String>` — `base`, `base-2`, ... `base-100`. **suffix 포함 총 길이도 60자 이내** (base를 잘라서 맞춤)

- [ ] **Step 1: 실패하는 테스트 작성**

`refname.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_normal_refs() {
        for r in ["main", "origin/main", "feature/x", "v1.0", "HEAD~2"] {
            assert!(validate_user_ref(r).is_ok(), "{r}");
        }
    }

    #[test]
    fn rejects_empty_and_option_like() {
        for r in ["", "-x", "--force", "-"] {
            assert!(validate_user_ref(r).is_err(), "{r:?}");
        }
    }
}
```

`worktree_name.rs`:
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
    fn windows_reserved_names_get_suffix() {
        assert_eq!(sanitize_worktree_name("con"), "con-ws");
        assert_eq!(sanitize_worktree_name("CON"), "CON-ws");
        assert_eq!(sanitize_worktree_name("lpt9"), "lpt9-ws");
        // 예약어가 아닌 유사 이름은 그대로
        assert_eq!(sanitize_worktree_name("console"), "console");
    }

    #[test]
    fn windows_superscript_reserved_names_get_suffix() {
        // COM¹/LPT³ 등 위첨자 변형도 Windows 예약 장치명이다
        assert_eq!(sanitize_worktree_name("com¹"), "com¹-ws");
        assert_eq!(sanitize_worktree_name("LPT³"), "LPT³-ws");
    }

    #[test]
    fn output_charset_is_always_ref_safe() {
        // 브랜치명/디렉토리명 안전성의 근거: 문자·숫자·단일 대시 외 아무것도 남지 않는다
        for input in ["a b", "x/../y", "--", "évoluer!", "한글 이름", "a..b", ".git", "-x", "nul"] {
            let out = sanitize_worktree_name(input);
            assert!(!out.is_empty());
            assert!(out.chars().all(|c| c.is_alphanumeric() || c == '-'), "{input} -> {out}");
            assert!(!out.starts_with('-') && !out.ends_with('-'));
            assert!(!out.contains("--"));
        }
    }

    #[test]
    fn candidates_start_with_base_then_numbered() {
        let mut it = candidate_names("fix");
        assert_eq!(it.next().unwrap(), "fix");
        assert_eq!(it.next().unwrap(), "fix-2");
        assert_eq!(candidate_names("fix").last().unwrap(), "fix-100");
    }

    #[test]
    fn suffixed_candidates_stay_within_max_len() {
        let base: String = "a".repeat(60);
        for name in candidate_names(&base) {
            assert!(name.chars().count() <= 60, "{name}");
        }
    }
}
```

- [ ] **Step 2: 테스트 실패 확인**

Run: `cargo test -p suaegi-git --lib`
Expected: 컴파일 에러

- [ ] **Step 3: 구현**

`refname.rs`:
```rust
use crate::runner::GitError;

/// 사용자 입력 ref가 git 옵션으로 해석되는 것(`--force` 등)을 차단한다.
/// base_ref를 git 인자로 넘기는 모든 공개 함수는 이걸 먼저 호출한다.
pub fn validate_user_ref(r: &str) -> Result<(), GitError> {
    if r.is_empty() || r.starts_with('-') {
        return Err(GitError::Parse {
            args: "ref validation".to_string(),
            detail: format!("invalid ref: {r:?}"),
        });
    }
    Ok(())
}
```

`worktree_name.rs`:
```rust
const MAX_LEN: usize = 60;
const MAX_SUFFIX: u32 = 100;

// 대소문자 무관 비교. CON.txt류 확장자 케이스는 sanitize가 '.'을 제거하므로 불필요.
// 위첨자 ¹²³ 변형(COM¹ 등)도 Windows 예약 — is_alphanumeric을 통과하므로 명시 필요.
const WINDOWS_RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7",
    "com8", "com9", "com¹", "com²", "com³", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5",
    "lpt6", "lpt7", "lpt8", "lpt9", "lpt¹", "lpt²", "lpt³",
];

/// 유니코드 문자/숫자만 유지하고 나머지는 `-`로. 출력이 `[alnum|-]`로만 구성되므로
/// git ref로도 디렉토리명으로도 항상 유효하다 (Orca worktree-logic 차용).
/// Windows 예약 장치명은 디렉토리 생성이 불가능하므로 접미사로 회피한다.
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
    let trimmed: String = out
        .trim_matches(|c| c == '-' || c == '.')
        .chars()
        .take(MAX_LEN)
        .collect();
    let trimmed = trimmed.trim_end_matches('-').to_string();
    if trimmed.is_empty() {
        return "workspace".to_string();
    }
    if WINDOWS_RESERVED.contains(&trimmed.to_ascii_lowercase().as_str()) {
        return format!("{trimmed}-ws");
    }
    trimmed
}

pub fn candidate_names(base: &str) -> impl Iterator<Item = String> + '_ {
    std::iter::once(base.to_string()).chain((2..=MAX_SUFFIX).map(move |n| {
        let suffix = format!("-{n}");
        // suffix 포함 총 길이 MAX_LEN 유지 — base를 잘라 맞춘다
        let take = MAX_LEN.saturating_sub(suffix.chars().count());
        let head: String = base.chars().take(take).collect();
        let head = head.trim_end_matches('-');
        format!("{head}{suffix}")
    }))
}
```

- [ ] **Step 4: 테스트 통과 확인**

Run: `cargo test -p suaegi-git --lib`
Expected: refname 2 + worktree_name 10 passed

- [ ] **Step 5: Commit**

```bash
git add crates/suaegi-git/src/refname.rs crates/suaegi-git/src/worktree_name.rs
git commit -m "feat(git): ref validation, sanitization with windows-reserved names, length-safe candidates"
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
  - `probe_repo(runner: &GitRunner, path: &Path) -> Result<RepoProbe, GitError>` — "not a git repository"만 `is_git_repo: false`로 매핑, symbolic-ref는 exit 1(detached HEAD)만 `head_branch: None`으로 매핑. 그 외 실패(권한, 손상 등)는 에러로 전파
  - `RepoProbe { is_git_repo: bool, head_branch: Option<String> }`
  - 테스트 픽스처 `fixture::init_repo(dir: &Path)` — `git init -b main` + 커밋 1개. 글로벌/시스템 git 설정, 훅, 서명 완전 격리 (이후 태스크들이 재사용)

- [ ] **Step 1: 픽스처 작성** (`tests/fixture/mod.rs`)

```rust
use std::path::Path;
use std::process::Command;

/// 테스트용 실제 git repo: `git init -b main` + README 커밋 1개.
/// 개발자 머신의 글로벌/시스템 설정(gpg 서명, 훅 템플릿, credential helper)이
/// 테스트를 오염시키지 않도록 env로 완전 격리한다.
pub fn init_repo(dir: &Path) {
    // 빈 글로벌 설정 파일 + 빈 훅 디렉토리 (크로스플랫폼: /dev/null 대신 실제 빈 파일/디렉토리)
    std::fs::write(dir.join(".test-gitconfig"), "").unwrap();
    std::fs::create_dir_all(dir.join(".no-hooks")).unwrap();
    run(dir, &["init", "-b", "main"]);
    run(dir, &["config", "user.email", "t@example.com"]);
    run(dir, &["config", "user.name", "test"]);
    run(dir, &["config", "commit.gpgsign", "false"]);
    run(dir, &["config", "tag.gpgsign", "false"]);
    run(dir, &["config", "core.hooksPath", ".no-hooks"]);
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    run(dir, &["add", "README.md"]);
    run(dir, &["commit", "-m", "init"]);
}

pub fn run(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", dir.join(".test-gitconfig"))
        .output()
        .expect("spawn git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}
```

주의: `GIT_CONFIG_GLOBAL`은 repo 디렉토리가 아니라 그 안의 파일을 가리키므로, worktree 디렉토리에서 `fixture::run(&wt, ...)`을 호출하는 이후 태스크의 테스트에서는 해당 worktree에 `.test-gitconfig`가 없다. 이를 위해 `run()`은 파일이 없으면 만들도록 첫 줄에 추가한다:

```rust
pub fn run(dir: &Path, args: &[&str]) {
    let cfg = dir.join(".test-gitconfig");
    if !cfg.exists() {
        let _ = std::fs::write(&cfg, "");
    }
    // ... (위와 동일)
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

#[tokio::test]
async fn detached_head_reports_none_branch() {
    let dir = tempfile::tempdir().unwrap();
    fixture::init_repo(dir.path());
    fixture::run(dir.path(), &["checkout", "--detach"]);
    let probe = probe_repo(&GitRunner::new(), dir.path()).await.unwrap();
    assert!(probe.is_git_repo);
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
        Ok(_) => return Ok(RepoProbe { is_git_repo: false, head_branch: None }),
        // "not a git repository"만 정상적인 false. 권한/손상/기타 실패는 전파해
        // 호출자(UI)가 "repo가 아님"과 "확인 불가"를 구분할 수 있게 한다.
        Err(GitError::Failed { ref stderr, .. })
            if stderr.to_lowercase().contains("not a git repository") =>
        {
            return Ok(RepoProbe { is_git_repo: false, head_branch: None })
        }
        Err(e) => return Err(e),
    }
    // symbolic-ref exit 1 = detached HEAD (정상). 그 외 실패는 전파.
    let head = match runner.run(path, &["symbolic-ref", "--short", "HEAD"]).await {
        Ok(o) => {
            let s = o.stdout.trim().to_string();
            (!s.is_empty()).then_some(s)
        }
        Err(GitError::Failed { code: Some(1), .. }) => None,
        Err(e) => return Err(e),
    };
    Ok(RepoProbe { is_git_repo: true, head_branch: head })
}
```

- [ ] **Step 5: 테스트 통과 확인**

Run: `cargo test -p suaegi-git --test repo_probe_test`
Expected: 3 passed

- [ ] **Step 6: Commit**

```bash
git add crates/suaegi-git/src/repo_probe.rs crates/suaegi-git/tests/fixture crates/suaegi-git/tests/repo_probe_test.rs
git commit -m "feat(git): repo probe with detached-head handling and isolated test fixture"
```

---

### Task 8: worktree 생성 (충돌 회피 + 롤백)

**Files:**
- Create: `crates/suaegi-git/src/worktree.rs`
- Test: `crates/suaegi-git/tests/worktree_test.rs`

**Interfaces:**
- Consumes: `GitRunner`(T5), `refname`/`worktree_name`(T6), fixture(T7)
- Produces:
  - `add_worktree(runner, repo_path, requested_name, base_ref, workspace_root) -> Result<CreatedWorktree, WorktreeError>`
  - `CreatedWorktree { path: PathBuf, branch: String, display_name: String }` — `path`는 canonicalize된 절대 경로 (WorktreeId의 근간이므로 상대 workspace_root 입력에도 항상 canonical)
  - `WorktreeError { Git(GitError) | NoAvailableName | InvalidBaseRef(String) | Io(std::io::Error) }`
  - 동작: `refname::validate_user_ref(base_ref)` → 이름 새니타이즈 → `workspace_root/<repo_dir_name>/<name>` 후보 경로+동명 브랜치, 충돌 시 다음 후보 → `git worktree add --no-track -b` (타임아웃 180s) → 실패 시 롤백 (동명 repo 여럿이면 suffix로 회피 — repo별 하위 디렉토리 격리는 post-MVP)
  - `branch_exists`는 rev-parse exit 1만 "없음"으로 매핑, 그 외 에러는 전파

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
    assert!(created.path.is_absolute());
    assert!(created.path.join("README.md").exists());
    let list = r.run(repo.path(), &["worktree", "list", "--porcelain"]).await.unwrap();
    assert!(list.stdout.contains("Fix-Bug"));
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

#[tokio::test]
async fn option_like_base_ref_is_rejected() {
    let repo = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    fixture::init_repo(repo.path());
    let r = GitRunner::new();
    let err = add_worktree(&r, repo.path(), "fix", "--force", ws.path()).await.unwrap_err();
    assert!(matches!(err, WorktreeError::InvalidBaseRef(_)));
}
```

- [ ] **Step 2: 테스트 실패 확인**

Run: `cargo test -p suaegi-git --test worktree_test`
Expected: 컴파일 에러

- [ ] **Step 3: 구현** (`src/worktree.rs`)

```rust
use crate::refname::validate_user_ref;
use crate::runner::{GitError, GitRunner};
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
    #[error("invalid base ref: {0}")]
    InvalidBaseRef(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// rev-parse exit 1만 "브랜치 없음". 타임아웃/스폰 실패/기타 에러를 "없음"으로
/// 오독하면 잘못된 전제로 생성이 진행되므로 전파한다.
async fn branch_exists(
    runner: &GitRunner,
    repo: &Path,
    branch: &str,
) -> Result<bool, GitError> {
    let refname = format!("refs/heads/{branch}");
    match runner
        .run(repo, &["rev-parse", "--verify", "--quiet", &refname])
        .await
    {
        Ok(_) => Ok(true),
        Err(GitError::Failed { code: Some(1), .. }) => Ok(false),
        Err(e) => Err(e),
    }
}

pub async fn add_worktree(
    runner: &GitRunner,
    repo_path: &Path,
    requested_name: &str,
    base_ref: &str,
    workspace_root: &Path,
) -> Result<CreatedWorktree, WorktreeError> {
    validate_user_ref(base_ref)
        .map_err(|_| WorktreeError::InvalidBaseRef(base_ref.to_string()))?;

    let sanitized = sanitize_worktree_name(requested_name);
    let repo_dir_name = repo_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".to_string());
    let parent = workspace_root.join(&repo_dir_name);
    std::fs::create_dir_all(&parent)?;
    // WorktreeId의 근간이 되는 경로이므로 항상 canonical 절대 경로로
    let parent = parent.canonicalize()?;

    let mut chosen: Option<(String, PathBuf)> = None;
    for name in candidate_names(&sanitized) {
        let path = parent.join(&name);
        if path.exists() || branch_exists(runner, repo_path, &name).await? {
            continue;
        }
        chosen = Some((name, path));
        break;
    }
    let (branch, path) = chosen.ok_or(WorktreeError::NoAvailableName)?;

    // --no-track: base가 remote ref일 때 미푸시 브랜치가 "behind"로 오보되는 것 방지 (Orca 차용)
    let path_str = path.to_string_lossy().into_owned();
    let result = runner
        .run_with_timeout(
            repo_path,
            &["worktree", "add", "--no-track", "-b", &branch, &path_str, base_ref],
            WORKTREE_ADD_TIMEOUT,
        )
        .await;

    if let Err(e) = result {
        // 롤백. 브랜치/경로 부재는 위에서 확인했으므로(단일 인스턴스 가정 하에)
        // 여기 있는 생성물은 이번 호출의 부산물이다. 롤백 자체의 실패는 원인
        // 에러를 가리지 않기 위해 무시한다 — 잔여물은 다음 `worktree prune`이 정리.
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
Expected: 4 passed

- [ ] **Step 5: Commit**

```bash
git add crates/suaegi-git/src/worktree.rs crates/suaegi-git/tests/worktree_test.rs
git commit -m "feat(git): worktree creation with collision suffixes, ref validation, rollback"
```

---

### Task 9: worktree 리스팅(-z) + 삭제(결과 보고)

**Files:**
- Modify: `crates/suaegi-git/src/worktree.rs`
- Test: `crates/suaegi-git/tests/worktree_test.rs` (추가)

**Interfaces:**
- Consumes: Task 8의 모듈
- Produces:
  - `list_worktrees(runner, repo_path) -> Result<Vec<WorktreeEntry>, GitError>` — `--porcelain -z` NUL 구분 파싱 (개행 포함 경로 안전)
  - `WorktreeEntry { path: PathBuf, branch: Option<String>, head: Option<String>, is_main: bool }` — `is_main`은 첫 엔트리 (git 문서 보장: "The main worktree is listed first")
  - `remove_worktree(runner, repo_path, worktree_path, force: bool, delete_branch: Option<&str>) -> Result<RemoveOutcome, WorktreeError>`
  - `RemoveOutcome { branch_deletion: BranchDeletion }`
  - `BranchDeletion { NotRequested | Deleted | Failed(String) }` — "이미 없음"은 목표 상태 달성이므로 `Deleted`. 실패는 에러 메시지 보존 (호출자가 정확히 기록/재시도할 수 있게)

- [ ] **Step 1: 실패하는 테스트 추가** (`tests/worktree_test.rs`)

```rust
use suaegi_git::worktree::{list_worktrees, remove_worktree, BranchDeletion};

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
async fn remove_worktree_deletes_dir_and_reports_branch_result() {
    let repo = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    fixture::init_repo(repo.path());
    let r = GitRunner::new();
    let created = add_worktree(&r, repo.path(), "fix", "main", ws.path()).await.unwrap();
    let outcome = remove_worktree(&r, repo.path(), &created.path, false, Some("fix"))
        .await
        .unwrap();
    assert_eq!(outcome.branch_deletion, BranchDeletion::Deleted);
    assert!(!created.path.exists());
    let list = list_worktrees(&r, repo.path()).await.unwrap();
    assert_eq!(list.len(), 1);
    let br = r.run(repo.path(), &["branch", "--list", "fix"]).await.unwrap();
    assert!(br.stdout.trim().is_empty());
}

#[tokio::test]
async fn removing_already_deleted_branch_counts_as_deleted() {
    let repo = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    fixture::init_repo(repo.path());
    let r = GitRunner::new();
    let created = add_worktree(&r, repo.path(), "fix", "main", ws.path()).await.unwrap();
    // 브랜치를 먼저 지워 "이미 없음" 상태를 만든다 (worktree가 잡고 있으므로 강제)
    fixture::run(repo.path(), &["worktree", "remove", "--force", created.path.to_str().unwrap()]);
    fixture::run(repo.path(), &["branch", "-D", "fix"]);
    let second = add_worktree(&r, repo.path(), "fix2", "main", ws.path()).await.unwrap();
    let outcome = remove_worktree(&r, repo.path(), &second.path, false, Some("no-such-branch"))
        .await
        .unwrap();
    // 목표 상태(브랜치 없음)는 달성됐으므로 Deleted
    assert_eq!(outcome.branch_deletion, BranchDeletion::Deleted);
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
    let outcome = remove_worktree(&r, repo.path(), &created.path, true, None).await.unwrap();
    assert_eq!(outcome.branch_deletion, BranchDeletion::NotRequested);
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchDeletion {
    NotRequested,
    /// 삭제 성공 또는 이미 없음 (목표 상태 달성)
    Deleted,
    /// worktree는 제거됐지만 브랜치 삭제 실패 (예: 다른 worktree가 체크아웃 중)
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveOutcome {
    pub branch_deletion: BranchDeletion,
}

/// `git worktree list --porcelain -z` 파싱. -z 모드는 각 속성 라인이 NUL로
/// 끝나고 엔트리 사이에 빈 NUL 레코드가 온다. 경로에 개행이 있어도 안전.
/// git 문서 보장에 따라 첫 엔트리가 main worktree다.
pub async fn list_worktrees(
    runner: &GitRunner,
    repo_path: &Path,
) -> Result<Vec<WorktreeEntry>, GitError> {
    let out = runner
        .run(repo_path, &["worktree", "list", "--porcelain", "-z"])
        .await?;
    let mut entries: Vec<WorktreeEntry> = Vec::new();
    let mut current: Option<WorktreeEntry> = None;
    for record in out.stdout.split('\0') {
        if record.is_empty() {
            // 엔트리 구분자
            if let Some(e) = current.take() {
                entries.push(e);
            }
            continue;
        }
        if let Some(rest) = record.strip_prefix("worktree ") {
            if let Some(e) = current.take() {
                entries.push(e);
            }
            current = Some(WorktreeEntry {
                path: PathBuf::from(rest),
                branch: None,
                head: None,
                is_main: entries.is_empty(),
            });
        } else if let Some(rest) = record.strip_prefix("HEAD ") {
            if let Some(e) = current.as_mut() {
                e.head = Some(rest.to_string());
            }
        } else if let Some(rest) = record.strip_prefix("branch ") {
            if let Some(e) = current.as_mut() {
                e.branch = Some(rest.trim_start_matches("refs/heads/").to_string());
            }
        }
        // detached / locked / prunable 속성은 MVP에서 미사용 — Plan 3+ (삭제 UI)에서 확장
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
) -> Result<RemoveOutcome, WorktreeError> {
    let path_str = worktree_path.to_string_lossy().into_owned();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&path_str);
    runner.run(repo_path, &args).await?;
    let branch_deletion = match delete_branch {
        None => BranchDeletion::NotRequested,
        Some(branch) => match runner.run(repo_path, &["branch", "-D", branch]).await {
            Ok(_) => BranchDeletion::Deleted,
            // "이미 없음"은 목표 상태 달성 — 실패로 보고하면 UI가 헛경고를 띄운다
            Err(GitError::Failed { ref stderr, .. }) if stderr.contains("not found") => {
                BranchDeletion::Deleted
            }
            Err(e) => BranchDeletion::Failed(e.to_string()),
        },
    };
    Ok(RemoveOutcome { branch_deletion })
}
```

- [ ] **Step 4: 테스트 통과 확인**

Run: `cargo test -p suaegi-git --test worktree_test`
Expected: 8 passed (누적)

- [ ] **Step 5: Commit**

```bash
git add crates/suaegi-git/src/worktree.rs crates/suaegi-git/tests/worktree_test.rs
git commit -m "feat(git): NUL-safe worktree listing and removal with branch-deletion outcome"
```

---

### Task 10: base 대비 비교(diff) + 파일 diff + 상태

**Files:**
- Create: `crates/suaegi-git/src/compare.rs`
- Test: `crates/suaegi-git/tests/compare_test.rs`

**Interfaces:**
- Consumes: `GitRunner`(T5), `refname`(T6), fixture(T7), `add_worktree`(T8)
- Produces (Plan 5의 diff 패널이 사용):
  - `branch_compare(runner, worktree_path, base_ref) -> Result<BranchCompare, GitError>` — **merge-base 대비 working tree** diff (커밋된 변경 + 미커밋 변경 + **untracked 파일** 모두 포함 — 에이전트가 add/commit을 안 했어도 "이 에이전트가 뭘 바꿨나"에 답해야 함)
  - `BranchCompare { merge_base: String, ahead_count: u32, files: Vec<ChangedFile> }`
  - `ChangedFile { path: String, status: ChangeStatus, additions: Option<u32>, deletions: Option<u32> }` (`None` = 바이너리 또는 untracked라서 미산출)
  - `ChangeStatus { Added, Modified, Deleted, Renamed { from: String }, Other(char) }` — untracked는 `Added`
  - `file_diff(runner, worktree_path, base_ref, file: &str) -> Result<String, GitError>` — merge-base 대비 working tree unified patch. untracked 파일은 `diff --no-index`(exit 1 허용)로 합성
  - `working_tree_dirty(runner, worktree_path) -> Result<bool, GitError>`
  - 파싱은 전부 `-z` NUL 구분. `ahead_count` 파싱 실패와 잘린 name-status 레코드는 `GitError::Parse` (0이나 스킵으로 뭉개지 않음). numstat 카운트는 `"-"`(바이너리)만 `None`, 그 외 비숫자는 `Parse`
  - base_ref는 `refname::validate_user_ref` 통과 필수 (`merge-base`에 옵션 주입 방지)

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
async fn compare_reports_committed_changes() {
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
    // fixture가 만드는 .test-gitconfig는 untracked로 잡히므로 필터
    let paths: Vec<_> = paths.into_iter().filter(|p| !p.starts_with(".test-")).collect();
    assert_eq!(paths, vec!["README.md", "new.txt"]);
    let readme = cmp.files.iter().find(|f| f.path == "README.md").unwrap();
    assert_eq!(readme.status, ChangeStatus::Modified);
    assert_eq!(readme.additions, Some(1));
    assert_eq!(readme.deletions, Some(1));
}

#[tokio::test]
async fn compare_includes_untracked_files() {
    let (_repo, _ws, wt) = setup().await;
    // add도 commit도 하지 않은 새 파일 — 에이전트 작업 중 가장 흔한 상태
    std::fs::write(wt.join("wip.txt"), "wip\n").unwrap();

    let r = GitRunner::new();
    let cmp = branch_compare(&r, &wt, "main").await.unwrap();
    assert_eq!(cmp.ahead_count, 0);
    let wip = cmp.files.iter().find(|f| f.path == "wip.txt").expect("untracked file missing");
    assert_eq!(wip.status, ChangeStatus::Added);
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
async fn file_diff_synthesizes_patch_for_untracked() {
    let (_repo, _ws, wt) = setup().await;
    std::fs::write(wt.join("wip.txt"), "wip\n").unwrap();
    let r = GitRunner::new();
    let patch = file_diff(&r, &wt, "main", "wip.txt").await.unwrap();
    assert!(patch.contains("+wip"));
}

#[tokio::test]
async fn no_changes_yields_no_tracked_diffs() {
    let (_repo, _ws, wt) = setup().await;
    let r = GitRunner::new();
    let cmp = branch_compare(&r, &wt, "main").await.unwrap();
    assert_eq!(cmp.ahead_count, 0);
    // fixture 부산물(.test-gitconfig 등) 외에는 없어야 한다
    assert!(cmp.files.iter().all(|f| f.path.starts_with(".test-") || f.path.starts_with(".no-hooks")));
}

#[tokio::test]
async fn dirty_detection() {
    let (_repo, _ws, wt) = setup().await;
    let r = GitRunner::new();
    // fixture 부산물이 untracked로 존재하므로 이 테스트는 tracked 변경으로 판별
    std::fs::write(wt.join("README.md"), "dirty\n").unwrap();
    assert!(working_tree_dirty(&r, &wt).await.unwrap());
}
```

주의: fixture가 worktree 안에 `.test-gitconfig`/`.no-hooks`를 만들 수 있어 untracked 검증이 오염된다. 구현 전에 fixture를 한 줄 보강한다 — `init_repo`에서 `.gitignore`에 두 항목을 추가하고 커밋:

```rust
    std::fs::write(dir.join(".gitignore"), ".test-gitconfig\n.no-hooks/\n").unwrap();
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    run(dir, &["add", "README.md", ".gitignore"]);
```

(이후 `compare_reports_committed_changes`/`no_changes_yields_no_tracked_diffs`의 `.test-` 필터는 안전망으로 유지)

- [ ] **Step 2: 테스트 실패 확인**

Run: `cargo test -p suaegi-git --test compare_test`
Expected: 컴파일 에러

- [ ] **Step 3: 구현** (`src/compare.rs`)

```rust
use crate::refname::validate_user_ref;
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

async fn merge_base(
    runner: &GitRunner,
    worktree_path: &Path,
    base_ref: &str,
) -> Result<String, GitError> {
    validate_user_ref(base_ref)?;
    Ok(runner
        .run(worktree_path, &["merge-base", "HEAD", base_ref])
        .await?
        .stdout
        .trim()
        .to_string())
}

/// numstat 카운트 파싱: "-"(바이너리)만 None. 그 외 비숫자는 손상 출력이므로 Parse.
fn parse_count(token: &str, args: &str) -> Result<Option<u32>, GitError> {
    if token == "-" {
        return Ok(None);
    }
    token.parse::<u32>().map(Some).map_err(|e| GitError::Parse {
        args: args.to_string(),
        detail: format!("bad numstat count {token:?}: {e}"),
    })
}

/// merge-base 대비 **working tree** diff. `<mb>..HEAD`가 아니라 `<mb>`를 단독
/// 인자로 주면 커밋된 변경과 미커밋 변경이 모두 잡힌다. untracked 파일은 diff에
/// 안 잡히므로 `status --porcelain -z`에서 별도 수집해 Added로 합류시킨다.
pub async fn branch_compare(
    runner: &GitRunner,
    worktree_path: &Path,
    base_ref: &str,
) -> Result<BranchCompare, GitError> {
    let mb = merge_base(runner, worktree_path, base_ref).await?;
    let ahead_args = format!("{mb}..HEAD");
    let ahead_out = runner
        .run(worktree_path, &["rev-list", "--count", &ahead_args])
        .await?;
    let ahead = ahead_out.stdout.trim().parse::<u32>().map_err(|e| GitError::Parse {
        args: format!("rev-list --count {ahead_args}"),
        detail: format!("{e}: {:?}", ahead_out.stdout),
    })?;

    // -z: 레코드가 NUL 구분이라 특수문자 경로 안전. -M: rename 감지 (Orca와 동일)
    let name_status = runner
        .run(worktree_path, &["diff", "--name-status", "-z", "-M", &mb])
        .await?;
    let numstat = runner
        .run(worktree_path, &["diff", "--numstat", "-z", "-M", &mb])
        .await?;

    // numstat -z 레코드: "adds\tdels\tpath" 또는 rename 시 "adds\tdels\t" 뒤에
    // from, to가 각각 별도 NUL 레코드로 이어진다.
    let numstat_args = format!("diff --numstat -z -M {mb}");
    let mut counts: HashMap<String, (Option<u32>, Option<u32>)> = HashMap::new();
    let mut numstat_records = numstat.stdout.split('\0');
    while let Some(record) = numstat_records.next() {
        if record.is_empty() {
            continue;
        }
        // splitn(3): 파일명에 탭이 있어도 세 번째 조각(경로)이 절단되지 않게
        let mut parts = record.splitn(3, '\t');
        let (Some(a), Some(d)) = (parts.next(), parts.next()) else {
            return Err(GitError::Parse {
                args: numstat_args.clone(),
                detail: format!("truncated record {record:?}"),
            });
        };
        let adds = parse_count(a, &numstat_args)?;
        let dels = parse_count(d, &numstat_args)?;
        match parts.next() {
            Some(path) if !path.is_empty() => {
                counts.insert(path.to_string(), (adds, dels));
            }
            _ => {
                // rename: from, to가 이어지는 별도 레코드
                let _from = numstat_records.next();
                let to = numstat_records.next().ok_or_else(|| GitError::Parse {
                    args: numstat_args.clone(),
                    detail: "rename record missing target path".to_string(),
                })?;
                counts.insert(to.to_string(), (adds, dels));
            }
        }
    }

    // name-status -z 레코드: "X\0path\0" 또는 rename "R100\0from\0to\0"
    let ns_args = format!("diff --name-status -z -M {mb}");
    let mut files = Vec::new();
    let mut records = name_status.stdout.split('\0');
    while let Some(code) = records.next() {
        if code.is_empty() {
            continue;
        }
        let status_char = code.chars().next().unwrap_or('?');
        let (status, path) = match status_char {
            'R' => {
                let from = records.next().ok_or_else(|| GitError::Parse {
                    args: ns_args.clone(),
                    detail: "rename record missing source path".to_string(),
                })?;
                let to = records.next().ok_or_else(|| GitError::Parse {
                    args: ns_args.clone(),
                    detail: "rename record missing target path".to_string(),
                })?;
                (ChangeStatus::Renamed { from: from.to_string() }, to.to_string())
            }
            c => {
                let path = records.next().ok_or_else(|| GitError::Parse {
                    args: ns_args.clone(),
                    detail: format!("record {code:?} missing path"),
                })?;
                let status = match c {
                    'A' => ChangeStatus::Added,
                    'M' => ChangeStatus::Modified,
                    'D' => ChangeStatus::Deleted,
                    other => ChangeStatus::Other(other),
                };
                (status, path.to_string())
            }
        };
        let (additions, deletions) = counts.get(&path).copied().unwrap_or((None, None));
        files.push(ChangedFile { path, status, additions, deletions });
    }

    // untracked 파일 수집: status --porcelain -z에서 "?? path" 레코드
    let status_out = runner
        .run(worktree_path, &["status", "--porcelain", "-z", "--untracked-files=all"])
        .await?;
    for record in status_out.stdout.split('\0') {
        if let Some(path) = record.strip_prefix("?? ") {
            files.push(ChangedFile {
                path: path.to_string(),
                status: ChangeStatus::Added,
                additions: None,
                deletions: None,
            });
        }
    }

    Ok(BranchCompare { merge_base: mb, ahead_count: ahead, files })
}

pub async fn file_diff(
    runner: &GitRunner,
    worktree_path: &Path,
    base_ref: &str,
    file: &str,
) -> Result<String, GitError> {
    let mb = merge_base(runner, worktree_path, base_ref).await?;
    let patch = runner
        .run(worktree_path, &["diff", "-M", &mb, "--", file])
        .await?
        .stdout;
    if !patch.is_empty() {
        return Ok(patch);
    }
    // 빈 diff는 "변경 없는 tracked 파일"일 수도 있다. 실제 untracked("??")일 때만
    // --no-index로 합성 (차이 있으면 exit 1). 아니면 빈 patch가 정답.
    let status = runner
        .run(worktree_path, &["status", "--porcelain", "-z", "--", file])
        .await?;
    let is_untracked = status
        .stdout
        .split('\0')
        .any(|r| r.strip_prefix("?? ").is_some_and(|p| p == file));
    if !is_untracked {
        return Ok(patch);
    }
    let null_device = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let out = runner
        .run_expecting(
            worktree_path,
            &["diff", "--no-index", "--", null_device, file],
            &[1],
        )
        .await?;
    Ok(out.stdout)
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
Expected: 6 passed

- [ ] **Step 5: 전체 검증 + Commit**

Run: `cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check`
Expected: 전부 통과

```bash
git add crates/suaegi-git/src/compare.rs crates/suaegi-git/tests/compare_test.rs crates/suaegi-git/tests/fixture/mod.rs
git commit -m "feat(git): working-tree compare with untracked files and strict NUL parsing"
```

---

## 검토에서 기각/유보한 지적 (근거 기록)

Codex 교차 검증(2라운드)에서 나온 지적 중 반영하지 않은 것과 이유:

**라운드 1 기각 (라운드 2에서 codex가 일부 철회):**
- "Windows에서 `fs::rename`은 기존 파일을 교체할 수 없다" — 사실과 다름 (Rust std는 `MOVEFILE_REPLACE_EXISTING` 사용). **codex가 라운드 2에서 철회함.**
- 외부 프로세스와의 TOCTOU 경합 방어 — 단일 인스턴스 가정으로 수용 (Global Constraints 명시). 경합 시 에러 표면화가 MVP 동작.
- `OsString` 전면 도입(비UTF-8 경로) — MVP 미지원으로 문서화.
- 디렉토리 fsync — 크래시 내구성 과설계. 백업 5개가 실질 방어선.
- 저장 debounce — 앱 레이어(Plan 3) 책임으로 이관.

**라운드 2 기각:**
- `Repo` 필드 비공개화로 canonical 불변식 강제 — serde 역직렬화는 자기가 저장한 데이터를 읽는 것이므로 재정규화 불필요. `from_path`를 "표준 생성 경로"로 문서화하는 선에서 수용 (뉴타입 사방 전파는 MVP 과설계).
- 구버전 스키마 마이그레이션 부재 — v1이 최초 버전. v0은 존재하지 않으므로 마이그레이션 대상이 없음. v2 도입 시점에 마이그레이션 프레임 추가 (Global Constraints 명시).
- 로드 진단의 세분화(권한 vs IO vs 손상) — `LoadSource` + 미래 스키마 가드로 MVP 요구는 충족. 세분화는 Plan 3에서 UI 요구가 생기면.
- Windows job object (타임아웃 시 자식 트리 킬) — Unix 프로세스 그룹 킬만 구현, Windows는 문서화된 한계 (Global Constraints).

## 후속 플랜 로드맵 (이 문서 범위 아님)

- Plan 2: `suaegi-term` — portable-pty 세션 + alacritty_terminal 그리드 + 에이전트 레지스트리/폴링
- Plan 3: `suaegi-app` 셸 — iced 앱 골격, 사이드바, 레이아웃 트리
- Plan 4: 터미널 위젯(iced_term 포크) + 워크벤치 배선
- Plan 5: 에이전트 hook 서버 + diff 패널 + 세션 복원

각 플랜은 직전 플랜 완료 후 작성한다 (iced API 등 실측이 필요한 부분의 정밀도 확보).
