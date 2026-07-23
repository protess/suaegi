//! M3 — ignore 필터 + 워킹트리 git-status 데코레이션.
//!
//! 둘 다 **트리 위젯 데코레이션**용이고 `git`이 권위다. 여기 두 연산은 Plan 5의
//! merge-base diff(`compare.rs`)와 **다른 연산**이다:
//! - `check_ignored`: `git check-ignore -z --stdin` — 어떤 경로가 `.gitignore`/
//!   `core.excludesFile`/`info/exclude` 규칙에 걸리는가. git이 유일한 권위
//!   (`check-ignored-paths.ts:18-30`).
//! - `working_tree_status`: `git status --porcelain=v1 -z` — 워킹트리가 HEAD 대비
//!   더럽/untracked/staged 인가. merge-base 대비 diff가 아니다(Codex Q4).
//!
//! **transient(타임아웃·spawn 실패·exit 128)는 절대 "해당 없음"으로 뭉개지 않는다** —
//! 그러면 무시된 파일이 정상으로, 오류가 "변경 없음"으로 보인다. 오류는 `GitError`로
//! 표면화한다(이 저장소의 "transient ≠ 빈 결과" 규율).

use crate::runner::{GitError, GitRunner};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// 트리 렌더러가 하드코딩으로 숨기는 이름(Orca `file-explorer-entries.ts:3-5` 등가).
///
/// **이건 UI 레이어의 decoration이다(plan §1a).** 필터링을 `list_dir`에 굽지 않는다 —
/// 백엔드 리스팅은 있는 그대로 돌려주고, 트리 위젯이 렌더할 때 이 이름들을 건너뛴다.
/// 그래야 "왜 이 파일이 안 보이지" 같은 디버깅에서 리스터는 진실을 유지한다.
pub const HARDCODED_HIDES: &[&str] = &[".git", "node_modules"];

/// 워킹트리 한 경로의 상태. `git status --porcelain=v1`의 2글자 `XY`를 트리 UI가
/// 쓰는 형태로 접었다. `X`는 인덱스(staged), `Y`는 워킹트리 쪽 상태.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileStatus {
    /// 인덱스나 워킹트리에서 수정됨(`M`), 타입 변경(`T`) 포함.
    Modified,
    /// 새로 추가되어 staged 됨(`A`).
    Added,
    /// 삭제됨(`D`).
    Deleted,
    /// rename 감지(`R`). `from`은 이전 경로다. **porcelain -z에서 목적지 경로가
    /// 레코드에 있고 원본이 다음 NUL 레코드**라 `compare.rs`의 diff 파서와 방향이
    /// 반대다(diff는 원본이 먼저). 두-경로 소비 규율은 같다.
    Renamed { from: String },
    /// copy 감지(`C`). `R`과 같은 두-경로 모양.
    Copied { from: String },
    /// 미추적(`??`).
    Untracked,
    /// 미병합(충돌). porcelain의 unmerged 상태: `DD`,`AU`,`UD`,`UA`,`DU`,`AA`,`UU`.
    Conflicted,
    /// 위 어디에도 안 맞는 `XY`(예: 우리가 특별히 모델링하지 않은 조합). 원본
    /// 두 글자를 그대로 담아 **추측하지 않는다**(`compare.rs`의 `Other`와 같은 규율).
    Other(String),
}

/// 주어진 워크트리-상대 경로들 중 **git이 무시하는** 것들의 집합.
///
/// `git check-ignore -z --stdin`으로 경로를 NUL 구분 stdin에 먹이고, NUL 구분 stdout
/// 으로 무시된 경로를 받는다. positional 인자(`check-ignore <path>...`)는 경로 수가
/// 많으면 인자 길이 한계에 걸리므로 stdin이 정석(`check-ignored-paths.ts:18-30`).
///
/// **exit 코드 규율(실측, git 2.50.1)**:
/// - 0: 하나 이상 무시됨. stdout에 무시된 경로들.
/// - 1: **하나도 무시 안 됨. 오류가 아니다** — 빈 집합을 돌려준다. 빈 stdin도 1이다.
/// - 128: 진짜 오류(저장소 아님 등). `GitError`로 표면화한다.
///
/// 그래서 `run_with_stdin`에 `extra_ok_codes = &[1]`을 준다: 1만 성공-빈결과로 받고
/// 128은 러너가 `GitError::Failed`로 낸다. 이 **`&[1]`이 exit-1 처리의 핵심 라인**이다.
pub async fn check_ignored(
    runner: &GitRunner,
    worktree: &Path,
    rel_paths: &[&str],
) -> Result<HashSet<String>, GitError> {
    // 빈 입력은 git을 부르지 않는다 — check-ignore는 빈 stdin에도 exit 1을 내지만,
    // 스폰 자체를 아끼고 "무시된 것 없음"을 곧장 돌려준다.
    if rel_paths.is_empty() {
        return Ok(HashSet::new());
    }

    let mut stdin = Vec::new();
    for p in rel_paths {
        stdin.extend_from_slice(p.as_bytes());
        stdin.push(0);
    }

    let out = runner
        .run_with_stdin(worktree, &["check-ignore", "-z", "--stdin"], &stdin, &[1])
        .await?;

    // exit 1 = 무시된 것 없음. `&[1]` 덕에 여기 도달하고(러너가 오류로 안 냄),
    // stdout이 비어 아래 파싱이 빈 집합을 내지만 의도를 명시한다.
    if out.code == 1 {
        return Ok(HashSet::new());
    }

    let mut ignored = HashSet::new();
    for path in out.stdout.split('\0') {
        if !path.is_empty() {
            ignored.insert(path.to_string());
        }
    }
    Ok(ignored)
}

/// 워킹트리 상태 맵(경로 → `FileStatus`). `git status --porcelain=v1 -z`.
///
/// HEAD 대비 **워킹트리** 상태다 — merge-base diff(`compare.rs`)와 별개 연산(Codex Q4).
/// `--ignored`는 주지 않는다: 무시 여부는 `check_ignored`가 별도로 답한다(Orca처럼
/// status와 ignore를 분리).
pub async fn working_tree_status(
    runner: &GitRunner,
    worktree: &Path,
) -> Result<HashMap<String, FileStatus>, GitError> {
    let out = runner
        .run(worktree, &["status", "--porcelain=v1", "-z"])
        .await?;
    parse_porcelain_status(&out.stdout)
}

/// `git status --porcelain=v1 -z` 파서. **순수 함수라 단위 테스트로 직접 고정한다.**
///
/// 각 레코드는 `XY<SP>PATH`(XY 2글자 + 공백 + 경로), NUL로 끝난다. `-z`라 경로는
/// 이스케이프 없이 날것이다(특수문자/비ASCII 안전). rename/copy(`X`가 `R`/`C`)는
/// **원본 경로가 다음 NUL 레코드**라 그 레코드를 하나 더 소비해야 한다 — 안 하면
/// 이후 모든 레코드가 밀린다(`compare.rs`의 두-경로 규율과 같은 버그 클래스).
fn parse_porcelain_status(stdout: &str) -> Result<HashMap<String, FileStatus>, GitError> {
    let args = "status --porcelain=v1 -z";
    let mut map = HashMap::new();
    let mut records = stdout.split('\0');
    while let Some(record) = records.next() {
        // 마지막 NUL 뒤의 빈 조각(과 혹시 모를 빈 레코드)은 건너뛴다.
        if record.is_empty() {
            continue;
        }
        // `XY<SP>PATH` — 최소 `XY`(2) + 공백(1) + 경로 최소 1글자 = 4바이트.
        // `XY`와 공백은 항상 ASCII라 바이트 인덱스 2/3이 char 경계에 안전하다.
        if record.len() < 4 {
            return Err(GitError::Parse {
                args: args.to_string(),
                detail: format!("short status record {record:?}"),
            });
        }
        let bytes = record.as_bytes();
        let x = bytes[0] as char;
        let y = bytes[1] as char;
        let path = &record[3..];

        let status = if x == 'R' || x == 'C' {
            // 두-경로 레코드: 현재(목적지) 경로는 `path`, 원본은 **다음** 레코드.
            // (diff --name-status와 방향이 반대: 거긴 원본이 먼저.)
            let from = records.next().ok_or_else(|| GitError::Parse {
                args: args.to_string(),
                detail: format!("rename/copy record {record:?} missing origin path"),
            })?;
            if x == 'R' {
                FileStatus::Renamed {
                    from: from.to_string(),
                }
            } else {
                FileStatus::Copied {
                    from: from.to_string(),
                }
            }
        } else {
            classify_xy(x, y)
        };
        map.insert(path.to_string(), status);
    }
    Ok(map)
}

/// 두 글자 `XY`를 `FileStatus`로. rename/copy(`R`/`C`)는 여기 오지 않는다 —
/// 두-경로 소비가 필요해 호출부에서 먼저 처리한다.
fn classify_xy(x: char, y: char) -> FileStatus {
    match (x, y) {
        ('?', '?') => FileStatus::Untracked,
        // unmerged(충돌) 상태 전부. `A`/`D`가 양쪽에 겹치는 `AA`/`DD`도 충돌이다.
        ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D') => FileStatus::Conflicted,
        _ => {
            // 인덱스(X)를 우선하되 비어있으면 워킹트리(Y)를 본다. `??`는 위에서 걸렀다.
            let code = if x != ' ' && x != '?' { x } else { y };
            match code {
                'M' | 'T' => FileStatus::Modified,
                'A' => FileStatus::Added,
                'D' => FileStatus::Deleted,
                _ => FileStatus::Other(format!("{x}{y}")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! 순수 파서(`parse_porcelain_status`)를 실제 git이 내는 `-z` 바이트로 고정한다.
    //! 이 형태는 위 `sed`/`xxd` 실측(git 2.50.1)에서 그대로 가져왔다.
    use super::{classify_xy, parse_porcelain_status, FileStatus};

    #[test]
    fn parses_modified_added_deleted_untracked() {
        // " M keep\0A  staged\0 D gone\0?? new\0"
        let input = " M keep\0A  staged\0 D gone\0?? new\0";
        let map = parse_porcelain_status(input).unwrap();
        assert_eq!(map.get("keep"), Some(&FileStatus::Modified));
        assert_eq!(map.get("staged"), Some(&FileStatus::Added));
        assert_eq!(map.get("gone"), Some(&FileStatus::Deleted));
        assert_eq!(map.get("new"), Some(&FileStatus::Untracked));
        assert_eq!(map.len(), 4);
    }

    // --- crux: rename 두-경로 소비 (mutant: R을 한-경로로 처리) ---
    // porcelain -z의 rename은 "R  <dest>\0<origin>\0" — 목적지가 레코드에, 원본이
    // 다음 NUL 레코드. 다음 레코드를 소비하지 않으면 원본("orig")이 독립 레코드로
    // 파싱돼 desync(가짜 엔트리 + map.len 증가)된다.
    #[test]
    fn rename_consumes_origin_record() {
        // "RM a2\0a\0?? tail\0" — RM(rename+worktree modified), origin "a", 그리고
        // 뒤에 정상 레코드 하나. 소비 실패 시 "a"가 레코드로 오인돼 tail이 밀린다.
        let input = "RM a2\0a\0?? tail\0";
        let map = parse_porcelain_status(input).unwrap();
        assert_eq!(
            map.get("a2"),
            Some(&FileStatus::Renamed {
                from: "a".to_string()
            }),
        );
        // origin "a"는 **키가 아니다** — 소비됐어야 한다.
        assert!(
            !map.contains_key("a"),
            "origin 경로가 소비되지 않고 키로 샜다"
        );
        // 뒤 레코드가 밀리지 않았다.
        assert_eq!(map.get("tail"), Some(&FileStatus::Untracked));
        assert_eq!(map.len(), 2, "정확히 rename 하나 + tail 하나");
    }

    #[test]
    fn copy_is_two_path_and_carries_origin() {
        let input = "C  dest\0src\0";
        let map = parse_porcelain_status(input).unwrap();
        assert_eq!(
            map.get("dest"),
            Some(&FileStatus::Copied {
                from: "src".to_string()
            }),
        );
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn conflict_states_are_conflicted() {
        for xy in ["UU", "AA", "DD", "AU", "UD", "UA", "DU"] {
            let input = format!("{xy} f\0");
            let map = parse_porcelain_status(&input).unwrap();
            assert_eq!(
                map.get("f"),
                Some(&FileStatus::Conflicted),
                "{xy}가 Conflicted로 분류되지 않았다"
            );
        }
    }

    // 비ASCII 경로가 -z에서 날것으로 온다(이스케이프 없음).
    #[test]
    fn parses_non_ascii_path_raw() {
        let input = " M 안녕.txt\0";
        let map = parse_porcelain_status(input).unwrap();
        assert_eq!(map.get("안녕.txt"), Some(&FileStatus::Modified));
    }

    // classify_xy 단위: index 우선, staged+worktree 조합.
    #[test]
    fn classify_prefers_index_then_worktree() {
        assert_eq!(classify_xy('M', ' '), FileStatus::Modified); // staged modify
        assert_eq!(classify_xy(' ', 'M'), FileStatus::Modified); // worktree modify
        assert_eq!(classify_xy('A', 'M'), FileStatus::Added); // staged add + wt modify
        assert_eq!(classify_xy(' ', 'D'), FileStatus::Deleted);
        assert_eq!(classify_xy('T', ' '), FileStatus::Modified); // typechange
    }

    // 짧은(손상) 레코드는 조용히 넘기지 않고 Parse 오류.
    #[test]
    fn short_record_is_parse_error() {
        assert!(parse_porcelain_status("M\0").is_err());
    }
}
