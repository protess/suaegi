use crate::domain::{PersistedState, SCHEMA_VERSION};
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialize: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("data file was written by a newer app version; saving is blocked")]
    FutureSchemaGuard,
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

/// 백업 슬롯 개수. `Store` 밖에서 존재 여부를 확인할 때(예: suaegi-app의
/// LoadOrigin 판별)도 같은 개수를 알아야 하므로 public.
pub const BACKUP_SLOTS: usize = 5;

pub struct Store {
    data_file: PathBuf,
    last_written_hash: Option<u64>,
    backup_min_interval: Duration,
    future_schema_guard: bool,
}

fn content_hash(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
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

    pub fn data_file(&self) -> &PathBuf {
        &self.data_file
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

    /// `<name>.bak.<slot>` 명명 규칙. public이라 다른 크레이트가 이 규칙을
    /// 다시 베끼지 않고 같은 계산을 재사용할 수 있다 (예: suaegi-app이 로드
    /// *전에* 본파일/백업 존재 여부를 확인해 LoadOrigin을 판별할 때).
    pub fn backup_path(data_file: &Path, slot: usize) -> PathBuf {
        let name = data_file.file_name().unwrap_or_default().to_string_lossy();
        data_file.with_file_name(format!("{name}.bak.{slot}"))
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

    pub fn load(&mut self) -> LoadOutcome {
        self.future_schema_guard = false;
        if let Ok(text) = fs::read_to_string(&self.data_file) {
            match Self::parse_trusted(&text) {
                Ok(state) => {
                    self.last_written_hash = Some(content_hash(&text));
                    return LoadOutcome {
                        state,
                        source: LoadSource::MainFile,
                    };
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

    fn load_from_backups(&mut self) -> LoadOutcome {
        // 폴백 = 본파일이 신뢰 불가. 해시를 리셋해 복구 상태 저장이
        // SkippedUnchanged로 무시되지 않게 한다 (손상 영구화 방지).
        self.last_written_hash = None;
        for slot in 0..BACKUP_SLOTS {
            if let Ok(text) = fs::read_to_string(Self::backup_path(&self.data_file, slot)) {
                match Self::parse_trusted(&text) {
                    Ok(state) => {
                        return LoadOutcome {
                            state,
                            source: LoadSource::Backup(slot),
                        };
                    }
                    Err(is_future) => {
                        if is_future {
                            self.future_schema_guard = true;
                        }
                        // 손상/파싱 실패는 쓰레기 — 다음 슬롯을 계속 본다
                    }
                }
            }
        }
        LoadOutcome {
            state: PersistedState::default(),
            source: LoadSource::Default,
        }
    }

    /// 본파일을 .bak.0으로 복사하고 기존 백업들을 한 칸씩 뒤로. 직전 백업이
    /// min_interval 이내면 생략 (Orca의 ≥1h 간격 패턴). 미래 mtime(시계 역행)은
    /// "오래됨"으로 취급해 회전이 영구 정지하지 않게 한다.
    fn rotate_backups(&self) -> Result<(), PersistenceError> {
        if !self.data_file.exists() {
            return Ok(());
        }
        let bak0 = Self::backup_path(&self.data_file, 0);
        if let Ok(modified) = fs::metadata(&bak0).and_then(|m| m.modified()) {
            match SystemTime::now().duration_since(modified) {
                Ok(age) if age < self.backup_min_interval => return Ok(()),
                Ok(_) | Err(_) => {} // 오래됐거나 미래 mtime → 회전 진행
            }
        }
        let oldest = Self::backup_path(&self.data_file, BACKUP_SLOTS - 1);
        if oldest.exists() {
            fs::remove_file(&oldest)?;
        }
        for slot in (0..BACKUP_SLOTS - 1).rev() {
            let from = Self::backup_path(&self.data_file, slot);
            if from.exists() {
                fs::rename(&from, Self::backup_path(&self.data_file, slot + 1))?;
            }
        }
        fs::copy(&self.data_file, &bak0)?;
        Ok(())
    }

    pub fn save(&mut self, state: &PersistedState) -> Result<SaveOutcome, PersistenceError> {
        if self.future_schema_guard {
            return Err(PersistenceError::FutureSchemaGuard);
        }
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
        self.rotate_backups()?;
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
        assert!(matches!(
            store.save(&state).unwrap(),
            SaveOutcome::SkippedUnchanged
        ));
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
        assert!(matches!(
            fresh.save(&state).unwrap(),
            SaveOutcome::SkippedUnchanged
        ));
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
        assert!(matches!(
            store.save(&loaded.state).unwrap(),
            SaveOutcome::Written
        ));
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
        assert!(matches!(
            store.save(&loaded.state).unwrap(),
            SaveOutcome::Written
        ));
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
            assert!(
                dir.path().join(format!("data.json.bak.{i}")).exists(),
                "bak.{i}"
            );
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
}
