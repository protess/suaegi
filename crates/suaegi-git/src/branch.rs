//! 로컬 브랜치 목록 = 브랜치 피커용. Orca `listLocalBranches`
//! (`checkout.ts:38-73`)를 충실히 이식한다.
//!
//! **`git branch`가 아니라 `for-each-ref`.** porcelain(`git branch`)은 로케일
//! 의존 decoration(현재 브랜치 앞의 `* `, HEAD 화살표 등)을 섞어 스크립트
//! 파싱이 깨진다. `for-each-ref --format=...`는 안정적·스크립트용 출력이라
//! Orca가 이걸 쓴다.
//!
//! checkout(브랜치 스위치)은 **defer** — worktree-per-branch 모델이라 in-place
//! 스위치 소비자가 없다(플랜 §2 M3, §3).

use crate::runner::{GitError, GitRunner};
use std::path::Path;

/// **byte-for-byte** Orca의 for-each-ref format(`checkout.ts:41`).
///
/// `%(HEAD)` = 현재 브랜치면 `*`, 아니면 공백 한 칸. `%09` = TAB. `%(refname:short)`
/// = 짧은 브랜치 이름(`main`, `feature/x`). 즉 한 줄은 `<marker>\t<name>`.
/// **format 토큰을 바꾸지 마라** — marker/TAB/이름 중 하나라도 어긋나면 파싱이
/// 통째로 깨진다(mutation-target).
const BRANCH_FORMAT: &str = "%(HEAD)%09%(refname:short)";

/// 로컬 브랜치 하나. Orca는 `{ current, branches: string[] }`로 분리해 돌려주지만
/// suaegi는 브랜치별로 `is_current`를 붙여 한 목록에 담는다(호출부가 피커에서
/// 항목마다 "현재" 표시를 그리기 쉽다).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    /// 짧은 브랜치 이름(`%(refname:short)`). 예: `main`, `feature/x`.
    pub name: String,
    /// 이 브랜치가 워크트리에 체크아웃돼 있으면 true(`%(HEAD)`의 `*` 마커).
    pub is_current: bool,
}

/// 로컬 브랜치(`refs/heads/*`)를 나열한다. **현재 브랜치가 맨 앞**, 나머지는 git의
/// ref 순서(refname 오름차순). Orca `listLocalBranches`가 현재 브랜치를 위로
/// 올려 피커가 "여기 있다"를 맨 위에 보여주는 것과 같다(`checkout.ts:66-72`).
///
/// **transient≠false-negative**: 빈 repo(커밋 0개, unborn HEAD)나 detached HEAD는
/// **에러가 아니라** 자연스러운 결과다. 빈 repo → `refs/heads/`가 비어 빈 Vec.
/// detached HEAD → 어떤 브랜치도 `*`를 못 받아 전부 `is_current=false`(브랜치
/// **위에** 있지 않으니 맞다).
pub async fn list_branches(runner: &GitRunner, worktree: &Path) -> Result<Vec<Branch>, GitError> {
    let format_arg = format!("--format={BRANCH_FORMAT}");
    let out = runner
        .run(worktree, &["for-each-ref", &format_arg, "refs/heads/"])
        .await?;

    let mut branches: Vec<Branch> = Vec::new();
    for line in out.stdout.split('\n') {
        // for-each-ref는 마지막 줄 뒤에도 개행을 붙여 split이 빈 꼬리를 낸다.
        // 빈 repo면 stdout 자체가 비어 여기서 전부 걸러진다 → 빈 Vec.
        if line.is_empty() {
            continue;
        }
        // `<marker>\t<name>`. marker는 `*`(현재) 또는 공백. git refname은 TAB을
        // 담을 수 없어(`git check-ref-format` 금지) 첫 TAB 분할이 안전하다.
        let Some((marker, name)) = line.split_once('\t') else {
            // format이 깨지지 않는 한 도달 불가. 방어적으로 건너뛴다.
            continue;
        };
        if name.is_empty() {
            continue;
        }
        branches.push(Branch {
            name: name.to_string(),
            is_current: marker == "*",
        });
    }

    // 현재 브랜치를 맨 앞으로(안정 정렬이라 나머지는 git ref 순서 유지). Orca
    // `checkout.ts:66-72`의 current-first 정렬과 동치.
    branches.sort_by_key(|b| !b.is_current);
    Ok(branches)
}

/// 현재 체크아웃된 로컬 브랜치의 짧은 이름. detached HEAD나 빈 repo면 `None`
/// (**에러 아님** — 브랜치 위에 있지 않은 정상 상태다).
///
/// Orca는 별도 호출 없이 `listLocalBranches`의 `current` 필드로 이 값을 얻으므로,
/// suaegi도 목록의 `is_current`에서 파생시켜 진실의 출처를 하나로 둔다.
pub async fn current_branch(
    runner: &GitRunner,
    worktree: &Path,
) -> Result<Option<String>, GitError> {
    let branches = list_branches(runner, worktree).await?;
    Ok(branches.into_iter().find(|b| b.is_current).map(|b| b.name))
}
