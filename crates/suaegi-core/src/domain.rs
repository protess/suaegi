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
    /// **`#[serde(default)]`가 데이터 손실 방어다.** 이 두 필드(6c에서 추가)가
    /// 없는 옛 저장본에서, default가 없으면 `Worktree` **하나**의 역직렬화 실패가
    /// `PersistedState` 전체를 손상 판정으로 떨어뜨려 백업 폴백으로 간다 —
    /// 멀쩡한 repo/worktree 목록을 통째로 잃는 그 사고의 한 갈래다. default가
    /// 있으면 옛 키 없는 객체도 `None`/`0`으로 조용히 읽힌다.
    #[serde(default)]
    pub created_with_agent: Option<String>,
    #[serde(default)]
    pub created_at_unix_ms: u64,
    /// 이 worktree의 브랜치에 연결된 GitHub PR 번호(Plan 7a, Orca `hosted-review.ts:45`).
    /// 이 번호로 리뷰 상태를 재해석한다. **`#[serde(default)]`가 데이터 손실 방어다** —
    /// 이 필드 없는 옛 저장본이 `None`으로 조용히 로드돼야지, 하나의 역직렬화 실패가
    /// `PersistedState` 전체를 손상 판정으로 떨어뜨리면 안 된다(위 두 필드와 같은 등급).
    #[serde(default)]
    pub linked_github_pr: Option<u64>,
    /// 이 워크트리에 링크된 Linear 이슈 식별자(예: `ENG-123`, Plan N1 §1.3). Orca는
    /// provider별 슬롯을 분리한다(`types.ts:479-489`) — GitHub PR과 나란히 **세 필드**로.
    /// 셋 다 **`#[serde(default)]`** — 이 필드 없는 옛 저장본이 `None`으로 조용히 로드돼야
    /// `PersistedState` 전체 손상 판정을 피한다(`linked_github_pr`과 같은 등급).
    #[serde(default)]
    pub linked_linear_issue: Option<String>,
    /// 다중 워크스페이스 구분(organization id). 딥링크/재연결 식별에 필요.
    #[serde(default)]
    pub linked_linear_issue_workspace_id: Option<String>,
    /// `linear.app/{urlKey}/...` 딥링크·재연결 식별자.
    #[serde(default)]
    pub linked_linear_issue_organization_url_key: Option<String>,
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

/// **`Eq`가 없는 이유**: [`PersistedPane`]의 `ratio: f32`가 `Eq`를 막는다
/// (Task 0이 미리 경고해 둔 그대로다). 이를 담는 [`PersistedState`]에서도 같이
/// 뗐다 — `assert_eq!`는 `PartialEq`만 요구하므로 호출부는 그대로 컴파일된다.
///
/// **`SCHEMA_VERSION`은 올리지 않는다.** 영속화 가드가
/// `schema_version > SCHEMA_VERSION`에서만 발동하므로 `#[serde(default)]` 필드
/// 추가는 공짜지만(구버전은 모르는 키를 무시한다), 버전을 올리면 구버전이
/// 가드에 걸려 **저장을 아예 거부한다.**
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionState {
    #[serde(default)]
    pub active_worktree_id: Option<WorktreeId>,
    /// 마지막으로 화면에 있던 pane 트리. `None`이면 복원할 레이아웃이 없다
    /// (첫 실행, 또는 세션을 하나도 열지 않고 껐다).
    #[serde(default)]
    pub panes: Option<PersistedPane>,
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

/// `Eq`가 없는 이유는 [`SessionState`] 참고(`PersistedPane::Split::ratio`가 f32다).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
                linked_github_pr: Some(42),
                linked_linear_issue: Some("ENG-123".into()),
                linked_linear_issue_workspace_id: Some("org-1".into()),
                linked_linear_issue_organization_url_key: Some("acme".into()),
            }],
            session: SessionState {
                active_worktree_id: Some(WorktreeId("/tmp/ws/demo/fix-bug".into())),
                panes: Some(PersistedPane::Split {
                    axis: PersistedAxis::Vertical,
                    ratio: 0.375,
                    a: Box::new(PersistedPane::Leaf(WorktreeId("/tmp/ws/demo/a".into()))),
                    b: Box::new(PersistedPane::Split {
                        axis: PersistedAxis::Horizontal,
                        ratio: 0.5,
                        a: Box::new(PersistedPane::Leaf(WorktreeId("/tmp/ws/demo/b".into()))),
                        b: Box::new(PersistedPane::Leaf(WorktreeId("/tmp/ws/demo/c".into()))),
                    }),
                }),
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

    /// Plan 5의 하드 제약 하나: **레이아웃 필드를 더하면서 `SCHEMA_VERSION`을
    /// 올리지 않는다.** 영속화 가드가 `schema_version > SCHEMA_VERSION`에서만
    /// 발동하므로, 값이 1로 남아 있는 한 구버전 앱도 이 파일을 열어 계속 저장할
    /// 수 있다. 올리는 순간 구버전은 **저장을 아예 거부한다** — 그 회귀를
    /// 컴파일이 아니라 테스트로 잡는다(상수 변경은 조용히 통과하기 때문).
    #[test]
    fn adding_the_layout_field_did_not_bump_the_schema_version() {
        assert_eq!(
            SCHEMA_VERSION, 1,
            "bumping this makes every older build refuse to save at all"
        );
        let json = serde_json::to_string(&PersistedState::default()).unwrap();
        let probe: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            probe["schema_version"], 1,
            "what we actually write must still be readable by a build that only knows v1"
        );
    }

    /// Plan 4 이전 빌드가 쓴 파일에는 `panes` 키가 아예 없다. `#[serde(default)]`가
    /// 그걸 `None`으로 채워야 한다 — 아니면 기존 사용자의 파일이 전부 손상으로
    /// 판정돼 백업 폴백으로 떨어진다.
    #[test]
    fn a_file_written_before_layout_persistence_still_loads() {
        let legacy = r#"{
            "schema_version": 1,
            "session": { "active_worktree_id": "/tmp/ws/demo/fix-bug" },
            "settings": { "workspace_root": "/tmp/ws" }
        }"#;
        let state: PersistedState =
            serde_json::from_str(legacy).expect("a pre-Plan-5 file must still parse");
        assert_eq!(
            state.session.panes, None,
            "a missing layout means 'nothing to restore', not a parse failure"
        );
        // 대조군: 같은 역직렬화가 실제로 내용을 읽고 있다는 것 — 위의 None이
        // "전부 기본값으로 뭉갰다"로도 설명되면 안 된다.
        assert_eq!(
            state.session.active_worktree_id,
            Some(WorktreeId("/tmp/ws/demo/fix-bug".into())),
            "control: the fields that WERE present must have been read"
        );
        assert_eq!(state.settings.workspace_root, PathBuf::from("/tmp/ws"));
    }

    /// 6c 이전 빌드가 쓴 worktree 객체에는 `created_with_agent`/`created_at_unix_ms`
    /// 키가 아예 없다. `#[serde(default)]`가 그걸 `None`/`0`으로 채워야 한다 —
    /// 아니면 이 worktree 하나의 파싱 실패가 저장 파일 전체를 손상으로 판정해
    /// 백업 폴백으로 떨어뜨린다(data-loss 등급).
    #[test]
    fn a_worktree_written_before_the_agent_fields_still_loads() {
        let legacy = r#"{
            "id": "/tmp/ws/demo/fix-bug",
            "repo_id": "/tmp/demo",
            "path": "/tmp/ws/demo/fix-bug",
            "branch": "fix-bug",
            "display_name": "fix-bug"
        }"#;
        let wt: Worktree =
            serde_json::from_str(legacy).expect("a pre-6c worktree object must still parse");
        assert_eq!(
            wt.created_with_agent, None,
            "a missing agent key means 'login shell', not a parse failure"
        );
        assert_eq!(
            wt.created_at_unix_ms, 0,
            "a missing timestamp key must default to 0, not fail the whole file"
        );
        // 대조군: 존재하던 필드는 실제로 읽혔다 — 위의 default가 "전부 기본값으로
        // 뭉갰다"로도 설명되면 안 된다.
        assert_eq!(wt.branch, "fix-bug", "control: present fields must be read");
        assert_eq!(wt.repo_id, RepoId("/tmp/demo".into()));
    }

    /// Plan 7a 이전 빌드가 쓴 worktree 객체에는 `linked_github_pr` 키가 아예 없다.
    /// `#[serde(default)]`가 그걸 `None`으로 채워야 한다 — 아니면 이 worktree 하나의
    /// 파싱 실패가 저장 파일 전체를 손상으로 판정해 백업 폴백으로 떨어뜨린다(data-loss 등급).
    #[test]
    fn a_worktree_written_before_the_linked_pr_field_still_loads() {
        let legacy = r#"{
            "id": "/tmp/ws/demo/fix-bug",
            "repo_id": "/tmp/demo",
            "path": "/tmp/ws/demo/fix-bug",
            "branch": "fix-bug",
            "display_name": "fix-bug",
            "created_with_agent": "claude",
            "created_at_unix_ms": 1700000000000
        }"#;
        let wt: Worktree =
            serde_json::from_str(legacy).expect("a pre-7a worktree object must still parse");
        assert_eq!(
            wt.linked_github_pr, None,
            "a missing linked_github_pr key means 'no PR linked', not a parse failure"
        );
        // 대조군: 존재하던 필드는 실제로 읽혔다.
        assert_eq!(wt.branch, "fix-bug", "control: present fields must be read");
        assert_eq!(wt.created_with_agent, Some("claude".to_string()));
    }

    /// `linked_github_pr`가 있는 값이 JSON을 왕복해도 보존돼야 한다(재해석의 근거).
    #[test]
    fn linked_github_pr_round_trips() {
        let wt = Worktree {
            id: WorktreeId("/tmp/ws/demo/fix-bug".into()),
            repo_id: RepoId("/tmp/demo".into()),
            path: PathBuf::from("/tmp/ws/demo/fix-bug"),
            branch: "fix-bug".into(),
            display_name: "fix-bug".into(),
            created_with_agent: None,
            created_at_unix_ms: 0,
            linked_github_pr: Some(1234),
            linked_linear_issue: None,
            linked_linear_issue_workspace_id: None,
            linked_linear_issue_organization_url_key: None,
        };
        let json = serde_json::to_string(&wt).unwrap();
        let back: Worktree = serde_json::from_str(&json).unwrap();
        assert_eq!(back.linked_github_pr, Some(1234));
    }

    /// Plan N1 이전 빌드가 쓴 worktree 객체에는 `linked_linear_issue*` 세 키가 아예 없다.
    /// `#[serde(default)]`가 그걸 `None`으로 채워야 한다 — 아니면 이 worktree 하나의 파싱
    /// 실패가 저장 파일 전체를 손상으로 판정해 백업 폴백으로 떨어뜨린다(data-loss 등급,
    /// `a_worktree_written_before_the_linked_pr_field_still_loads` 미러).
    #[test]
    fn a_worktree_written_before_the_linear_link_fields_still_loads() {
        let legacy = r#"{
            "id": "/tmp/ws/demo/fix-bug",
            "repo_id": "/tmp/demo",
            "path": "/tmp/ws/demo/fix-bug",
            "branch": "fix-bug",
            "display_name": "fix-bug",
            "created_with_agent": "claude",
            "created_at_unix_ms": 1700000000000,
            "linked_github_pr": 42
        }"#;
        let wt: Worktree =
            serde_json::from_str(legacy).expect("a pre-N1 worktree object must still parse");
        assert_eq!(
            wt.linked_linear_issue, None,
            "a missing linked_linear_issue key means 'no issue linked', not a parse failure"
        );
        assert_eq!(wt.linked_linear_issue_workspace_id, None);
        assert_eq!(wt.linked_linear_issue_organization_url_key, None);
        // 대조군: 존재하던 필드는 실제로 읽혔다 — 위의 default가 "전부 기본값으로 뭉갰다"로도
        // 설명되면 안 된다.
        assert_eq!(wt.branch, "fix-bug", "control: present fields must be read");
        assert_eq!(wt.linked_github_pr, Some(42));
    }

    /// Linear 링크 세 필드가 JSON을 왕복해도 보존돼야 한다(딥링크·재연결의 근거).
    #[test]
    fn linear_link_fields_round_trip() {
        let wt = Worktree {
            id: WorktreeId("/tmp/ws/demo/fix-bug".into()),
            repo_id: RepoId("/tmp/demo".into()),
            path: PathBuf::from("/tmp/ws/demo/fix-bug"),
            branch: "fix-bug".into(),
            display_name: "fix-bug".into(),
            created_with_agent: None,
            created_at_unix_ms: 0,
            linked_github_pr: None,
            linked_linear_issue: Some("ENG-7".into()),
            linked_linear_issue_workspace_id: Some("org-9".into()),
            linked_linear_issue_organization_url_key: Some("acme".into()),
        };
        let json = serde_json::to_string(&wt).unwrap();
        let back: Worktree = serde_json::from_str(&json).unwrap();
        assert_eq!(back.linked_linear_issue.as_deref(), Some("ENG-7"));
        assert_eq!(back.linked_linear_issue_workspace_id.as_deref(), Some("org-9"));
        assert_eq!(
            back.linked_linear_issue_organization_url_key.as_deref(),
            Some("acme")
        );
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
