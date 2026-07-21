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

/// `pane_grid::Axis`의 serde 거울. iced 타입은 `Serialize`를 갖지 않고, 갖게
/// 만들 수도 없다(외래 타입) — 그리고 **`suaegi-core`는 iced를 모른다.**
/// 값이 둘뿐이라 거울의 유지 비용이 사실상 없다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersistedAxis {
    Horizontal,
    Vertical,
}

/// `pane_grid::Configuration<T>`의 serde 거울. 저장할 때 `State::layout()`의
/// `Node`를 걸으며 만들고, 복원할 때 `Configuration`으로 되돌린다.
///
/// **잎이 `SessionId`가 아니라 `WorktreeId`인 것이 핵심이다.** `SessionId`는
/// 실행마다 매기는 카운터라 재시작을 넘지 못하고, `pane_grid::Pane`/`Split`의
/// 내부 `usize`는 비공개라 애초에 직렬화할 수 없다. worktree id는 경로에서
/// 나오므로 앱을 껐다 켜도 같다 — 훅 상관관계(`PaneKey`)와 레이아웃 복원이
/// **같은 키**를 쓴다.
///
/// **`suaegi-app`이 아니라 여기 사는 이유**: [`SessionState`]가 이걸 필드로
/// 담고, `SessionState`는 `suaegi-core`의 타입이다. 반대 방향 의존은 없다
/// (`suaegi-app → suaegi-term → suaegi-core`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersistedPane {
    Split {
        axis: PersistedAxis,
        ratio: f32,
        a: Box<PersistedPane>,
        b: Box<PersistedPane>,
    },
    Leaf(WorktreeId),
}

/// **Task 5 주의**: 여기에 `PersistedPane` 필드를 더하는 순간 `ratio: f32`
/// 때문에 `Eq`를 파생할 수 없다. `SessionState`와 이를 담는 [`PersistedState`]
/// 양쪽에서 `Eq`를 떼야 하고, `Eq`를 요구하는 호출부가 있으면 같이 고쳐야 한다.
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
        Self {
            workspace_root: home.join("suaegi-workspaces"),
        }
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
            settings: Settings {
                workspace_root: PathBuf::from("/tmp/ws"),
            },
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
