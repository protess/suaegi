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
