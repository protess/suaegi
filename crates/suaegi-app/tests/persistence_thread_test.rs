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
