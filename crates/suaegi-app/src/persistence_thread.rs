use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self as std_mpsc, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use futures::channel::mpsc as fmpsc;
use suaegi_core::domain::PersistedState;
use suaegi_core::persistence::{LoadSource, SaveOutcome, Store, BACKUP_SLOTS};

/// 마지막 요청 후 이만큼 조용하면 실제로 쓴다.
const DEBOUNCE: Duration = Duration::from_millis(300);

enum Request {
    Save {
        seq: u64,
        state: Box<PersistedState>,
    },
    OverrideFutureSchemaGuard,
}

/// 앱 데이터 파일의 기본 위치. OS별 config 디렉터리(macOS:
/// `~/Library/Application Support`, Linux: `~/.config`) 아래 `suaegi/data.json`.
/// `dirs::config_dir()`가 없는 드문 환경에서는 홈 디렉터리 아래 `.suaegi`로
/// 대체한다(`suaegi-core::domain`이 `workspace_root` 기본값을 계산할 때 쓰는
/// 것과 같은 폴백 패턴).
pub fn default_data_file() -> PathBuf {
    match dirs::config_dir() {
        Some(config) => config.join("suaegi").join("data.json"),
        None => dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".suaegi")
            .join("data.json"),
    }
}

pub struct PersistenceBoot {
    pub handle: PersistenceHandle,
    pub load: LoadDiagnostics,
    /// futures 채널이어야 Task::stream에 넣을 수 있다 (std mpsc는 Stream이 아니다).
    /// unbounded: 워커가 결과를 보고하다 막히면 안 된다.
    pub results: fmpsc::UnboundedReceiver<SaveReport>,
}

pub struct LoadDiagnostics {
    pub state: PersistedState,
    /// 데이터가 어디서 왔는지 — 신규 설치와 복구 실패를 구분한다.
    /// LoadSource만으로는 둘 다 Default라 UI가 헛경고를 띄우게 된다.
    pub origin: LoadOrigin,
    pub save_blocked: bool, // Store::future_schema_guarded() (public)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadOrigin {
    /// 본파일도 백업도 **하나도 없었다** — 신규 설치. 경고 금지.
    Fresh,
    Loaded, // 본파일 정상
    Recovered {
        slot: usize,
    }, // 백업에서 복구 — 사용자에게 알린다
    /// 뭔가는 있었는데 쓸 수 있는 게 없었다 — 강하게 알린다.
    /// (본파일이 없고 백업만 있는데 그 백업들이 다 깨진 경우도 여기다)
    RecoveryFailed,
}

/// 저장 요청 하나의 최종 상태. **모든 seq는 정확히 한 번 보고된다** —
/// debounce로 대체된 요청도 조용히 사라지지 않고 Superseded로 보고한다.
// Clone: `Message::Saved` (sidebar's seam for Task 8's persistence wiring)
// carries this into `AppState`, and `Message` as a whole must be `Clone`.
#[derive(Debug, Clone)]
pub struct SaveReport {
    pub seq: u64,
    pub status: SaveStatus,
}

#[derive(Debug, Clone)]
pub enum SaveStatus {
    Written,
    SkippedUnchanged,
    /// 더 새 요청이 들어와 이 요청은 쓰이지 않았다. 실패가 아니다.
    Superseded {
        by: u64,
    },
    Failed(String),
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
            || (0..BACKUP_SLOTS).any(|slot| Store::backup_path(&probe_path, slot).exists());
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
                    let timeout = if pending.is_some() {
                        DEBOUNCE
                    } else {
                        Duration::from_secs(3600)
                    };
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
                        Ok(Request::OverrideFutureSchemaGuard) => {
                            store.override_future_schema_guard()
                        }
                        Err(RecvTimeoutError::Timeout) => {
                            if let Some((seq, state)) = pending.take() {
                                let status = match store.save(&state) {
                                    Ok(SaveOutcome::Written) => SaveStatus::Written,
                                    Ok(SaveOutcome::SkippedUnchanged) => {
                                        SaveStatus::SkippedUnchanged
                                    }
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
                                    Ok(SaveOutcome::SkippedUnchanged) => {
                                        SaveStatus::SkippedUnchanged
                                    }
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

    /// 논블로킹. 발급한 seq를 반환한다. 워커가 죽어 송신이 실패하면
    /// 핸들이 직접 SaveReport{seq, Failed(..)}를 결과 채널로 흘려보낸다 —
    /// 죽은 워커는 자기 죽음을 보고할 수 없다.
    pub fn save(&self, state: PersistedState) -> u64 {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        let sent = self
            .tx
            .as_ref()
            .map(|tx| {
                tx.send(Request::Save {
                    seq,
                    state: Box::new(state),
                })
                .is_ok()
            })
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
        self.tx.take(); // 워커가 Disconnected를 보게 한다
        if let Some(t) = self.thread.take() {
            let _ = t.join(); // flush 대기 — 저장 하나는 수십 ms
        }
    }
}
