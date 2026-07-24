//! 워크트리 커밋 로그 = "에이전트가 한 일". Orca의 `git-history` 계열
//! (`git-history-log-parser.ts`, `git-history.ts`, `git-history-types.ts`)을
//! 충실히 이식한다.
//!
//! **git ≥ 2.34 전제.** decoration 필드를 `%(decorate:...separator=%x1f)`의
//! `\u{1f}` 하나로만 분리한다. Orca는 git <2.34용 comma 폴백을 두지만
//! (`git-history.ts:25-27`) suaegi는 modern git 가정(compare.rs 실측 2.50.1)이라
//! 폴백을 이식하지 않는다(F3).

use crate::runner::{GitError, GitRunner};
use std::path::Path;

/// 기본 커밋 수(호출자가 명시 안 하면 이 값을 넘긴다). Orca `GIT_HISTORY_DEFAULT_LIMIT`.
pub const DEFAULT_LIMIT: usize = 50;
/// 커밋 수 상한. Orca `GIT_HISTORY_MAX_LIMIT`.
pub const MAX_LIMIT: usize = 200;

/// **byte-for-byte** Orca `GIT_HISTORY_COMMIT_FORMAT`(`git-history-log-parser.ts:5-6`).
///
/// 7줄 헤더 + body: `[0]=%H [1]=%aN [2]=%aE [3]=%at [4]=%ct(미사용) [5]=%P
/// [6]=decorate`, `[7..]=%B`. **`%ct`를 지우지 마라(F2)** — 미사용이지만 지우면
/// 하위 인덱스가 한 칸씩 밀려 parents/decorations/body가 통째로 어긋난다.
pub const COMMIT_FORMAT: &str =
    "%H%n%aN%n%aE%n%at%n%ct%n%P%n%(decorate:prefix=,suffix=,separator=%x1f)%n%B";

/// decoration 필드 구분자. git ≥2.34의 `%(decorate:separator=%x1f)`가 내는 바이트.
const DECORATION_SEPARATOR: char = '\u{1f}';

/// ref 종류. Orca `GitHistoryRefCategory`('branches'|'remote branches'|'tags'|'commits').
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefCategory {
    /// 로컬 브랜치(`refs/heads/*`).
    Branches,
    /// 리모트 추적 브랜치(`refs/remotes/*`).
    RemoteBranches,
    /// 태그(`refs/tags/*`).
    Tags,
    /// 어디에도 속하지 않는 커밋 자체(detached HEAD 폴백 등).
    Commits,
}

/// 커밋에 붙은 ref 하나(브랜치/태그/리모트). Orca `GitHistoryItemRef`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref {
    /// 정규화된 전체 ref 이름. 예: `refs/heads/main`, `refs/tags/v1`.
    pub id: String,
    /// 표시용 짧은 이름. 예: `main`, `v1`, `origin/main`.
    pub name: String,
    /// 이 ref가 가리키는 커밋 oid.
    pub revision: String,
    /// ref 종류.
    pub category: RefCategory,
}

/// 커밋 한 개. Orca `GitHistoryItem`(subject=첫 줄, body=원본 메시지 전체).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    /// 전체 커밋 해시(`%H`).
    pub id: String,
    /// 앞 7자 축약(`shortGitHash`).
    pub short_id: String,
    /// author 이름(`%aN`, 표시-전용 lossy).
    pub author_name: String,
    /// author 이메일(`%aE`).
    pub author_email: String,
    /// author 시각, unix 초(`%at`). 파싱 실패 시 0.
    pub author_timestamp: i64,
    /// 부모 해시들(`%P`). root 커밋은 빈 벡터.
    pub parents: Vec<String>,
    /// 이 커밋에 붙은 decoration ref들(category 정렬됨).
    pub references: Vec<Ref>,
    /// 커밋 메시지 첫 줄. 비었으면 `(no commit message)`.
    pub subject: String,
    /// 커밋 메시지 원본 전체(`%B`, 후행 개행 1개 제거). 여러 줄일 수 있다.
    pub body: String,
}

/// `load_history` 결과. Orca `GitHistoryResult`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct History {
    /// 커밋 목록(최신순, 최대 `limit`개).
    pub items: Vec<Commit>,
    /// 현재 ref. 브랜치면 Branches, detached면 short-hash/Commits 폴백.
    /// unborn HEAD(빈 repo)이면 None.
    pub current_ref: Option<Ref>,
    /// 현재 브랜치의 업스트림 ref(설정돼 있고 해석되면). 없으면 None.
    pub remote_ref: Option<Ref>,
    /// current와 remote의 merge-base(둘이 다를 때만). 없으면 None.
    pub merge_base: Option<String>,
    /// 업스트림에 로컬이 아직 못 받은 커밋이 있는가.
    pub has_incoming: bool,
    /// 로컬에 업스트림이 아직 못 받은 커밋이 있는가.
    pub has_outgoing: bool,
    /// `limit`보다 많은 커밋이 있는가(`-n limit+1`로 감지).
    pub has_more: bool,
    /// 적용된(clamp된) limit.
    pub limit: usize,
}

/// 워크트리의 커밋 로그를 읽는다. **transient는 절대 에러가 아니다**: unborn
/// HEAD(빈 repo)는 빈 History, detached HEAD는 short-hash 폴백 ref.
///
/// `limit`은 `[1, MAX_LIMIT]`로 clamp된다(0 → 1). 기본값이 필요하면 호출자가
/// `DEFAULT_LIMIT`을 넘긴다.
pub async fn load_history(
    runner: &GitRunner,
    worktree: &Path,
    limit: usize,
) -> Result<History, GitError> {
    let limit = clamp_limit(limit);

    // unborn HEAD(커밋 0개) → rev-parse 실패 → 빈 History. **에러 아님**
    // (Orca `resolveCommit` null → `git-history.ts:164-173`).
    let Some(head_oid) = resolve_commit(runner, worktree, "HEAD").await else {
        return Ok(History {
            items: Vec::new(),
            current_ref: None,
            remote_ref: None,
            merge_base: None,
            has_incoming: false,
            has_outgoing: false,
            has_more: false,
            limit,
        });
    };

    let (current_ref, branch_name) = resolve_current_ref(runner, worktree, &head_oid).await;

    // 업스트림이 없으면 remote_ref=None → has_incoming/has_outgoing 둘 다 false.
    // **에러가 아니다** — 업스트림 미설정은 정상 상태다.
    let remote_ref = match &branch_name {
        Some(b) => resolve_upstream(runner, worktree, b).await,
        None => None,
    };

    // merge-base는 current와 remote가 실제로 다를 때만 구한다(Orca :191-198).
    let mut merge_base = None;
    if let Some(rr) = &remote_ref {
        if rr.revision != current_ref.revision {
            if let Ok(out) = runner
                .run(
                    worktree,
                    &["merge-base", &current_ref.revision, &rr.revision],
                )
                .await
            {
                let t = out.stdout.trim();
                if !t.is_empty() {
                    merge_base = Some(t.to_string());
                }
            }
        }
    }

    // git log --format=<F2> -z --topo-order --decorate=full -n<limit+1> HEAD
    let format_arg = format!("--format={COMMIT_FORMAT}");
    let n_arg = format!("-n{}", limit + 1);
    let out = runner
        .run(
            worktree,
            &[
                "log",
                &format_arg,
                "-z",
                "--topo-order",
                "--decorate=full",
                &n_arg,
                "HEAD",
            ],
        )
        .await?;

    let mut parsed = parse_history_log(&out.stdout);
    // hasMore: limit+1개를 요청했으니 limit보다 많으면 더 있다. items는 앞 limit개.
    let has_more = parsed.len() > limit;
    parsed.truncate(limit);

    // hasIncoming/hasOutgoing: merge-base 기준 불린(Orca :214-218). 업스트림
    // 없으면(merge_base=None) 둘 다 false.
    let has_incoming =
        matches!((&remote_ref, &merge_base), (Some(rr), Some(mb)) if &rr.revision != mb);
    let has_outgoing = matches!(&merge_base, Some(mb) if *mb != current_ref.revision);

    Ok(History {
        items: parsed,
        current_ref: Some(current_ref),
        remote_ref,
        merge_base,
        has_incoming,
        has_outgoing,
        has_more,
        limit,
    })
}

/// limit을 `[1, MAX_LIMIT]`로 clamp. Orca `clampHistoryLimit`(usize는 항상 유한이라
/// Orca의 non-finite→default 분기는 불필요; 0 → 1).
fn clamp_limit(limit: usize) -> usize {
    limit.clamp(1, MAX_LIMIT)
}

/// `<ref>^{commit}`을 rev-parse. 실패(unborn/미존재)면 None — **에러로 올리지 않는다.**
/// Orca `resolveCommit`(`git-history.ts:46-64`).
async fn resolve_commit(runner: &GitRunner, cwd: &Path, ref_: &str) -> Option<String> {
    if ref_.is_empty() || ref_.starts_with('-') {
        return None;
    }
    let spec = format!("{ref_}^{{commit}}");
    match runner
        .run(cwd, &["rev-parse", "--verify", "--end-of-options", &spec])
        .await
    {
        Ok(out) => {
            let t = out.stdout.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        }
        Err(_) => None,
    }
}

/// 현재 ref + 브랜치 이름. `symbolic-ref --quiet --short HEAD`가 성공+비어있지-않으면
/// 브랜치, 실패(detached)면 short-hash/Commits 폴백. **detached는 에러 아님.**
/// Orca `resolveCurrentRef`(`git-history.ts:85-112`).
async fn resolve_current_ref(
    runner: &GitRunner,
    cwd: &Path,
    head_oid: &str,
) -> (Ref, Option<String>) {
    if let Ok(out) = runner
        .run(cwd, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .await
    {
        let name = out.stdout.trim();
        if !name.is_empty() {
            return (
                Ref {
                    id: format!("refs/heads/{name}"),
                    name: name.to_string(),
                    revision: head_oid.to_string(),
                    category: RefCategory::Branches,
                },
                Some(name.to_string()),
            );
        }
    }

    // detached HEAD: symbolic-ref가 --quiet로 exit 1(=Err) 하거나 빈 출력.
    (
        Ref {
            id: head_oid.to_string(),
            name: short_hash(head_oid),
            revision: head_oid.to_string(),
            category: RefCategory::Commits,
        },
        None,
    )
}

/// 브랜치의 업스트림 ref. `for-each-ref --format=%(upstream)%00%(upstream:short)`로
/// 전체+짧은 이름을 얻고, 이름을 다시 rev-parse해 oid를 구한다(objectname은 git
/// 버전 간 비포터블). 없으면 None. Orca `resolveUpstreamRef`(`git-history.ts:114-140`).
async fn resolve_upstream(runner: &GitRunner, cwd: &Path, branch_name: &str) -> Option<Ref> {
    let refspec = format!("refs/heads/{branch_name}");
    let out = runner
        .run(
            cwd,
            &[
                "for-each-ref",
                "--format=%(upstream)%00%(upstream:short)",
                &refspec,
            ],
        )
        .await
        .ok()?;
    let mut parts = out.stdout.split('\0');
    let full = parts.next().unwrap_or("").trim();
    let short = parts.next().unwrap_or("").trim();
    if full.is_empty() || short.is_empty() {
        return None;
    }
    let oid = resolve_commit(runner, cwd, full).await?;
    Some(ref_from_full_name(full, short, &oid))
}

/// 전체 ref 이름 → 분류된 `Ref`. Orca `gitHistoryRefFromFullName`
/// (`git-history-log-parser.ts:135-151`).
fn ref_from_full_name(full_name: &str, fallback_name: &str, revision: &str) -> Ref {
    let id = if full_name.is_empty() {
        fallback_name
    } else {
        full_name
    };
    if let Some(name) = id.strip_prefix("refs/heads/") {
        return Ref {
            id: id.to_string(),
            name: name.to_string(),
            revision: revision.to_string(),
            category: RefCategory::Branches,
        };
    }
    if let Some(name) = id.strip_prefix("refs/remotes/") {
        return Ref {
            id: id.to_string(),
            name: name.to_string(),
            revision: revision.to_string(),
            category: RefCategory::RemoteBranches,
        };
    }
    if let Some(name) = id.strip_prefix("refs/tags/") {
        return Ref {
            id: id.to_string(),
            name: name.to_string(),
            revision: revision.to_string(),
            category: RefCategory::Tags,
        };
    }
    let name = if fallback_name.is_empty() {
        short_hash(revision)
    } else {
        fallback_name.to_string()
    };
    Ref {
        id: id.to_string(),
        name,
        revision: revision.to_string(),
        category: RefCategory::Commits,
    }
}

/// `git log ... -z ...` stdout을 커밋 목록으로 파싱. Orca `parseGitHistoryLog`
/// (`git-history-log-parser.ts:99-133`).
///
/// **F4 — 레코드별 leading `\n` strip.** `-z`에서 각 레코드가 git pretty-print의
/// 선행 개행을 달고 오므로, NUL 분리 후 각 레코드의 선행 `\n`을 벗겨야 line[0](해시)이
/// 비지 않는다. 안 벗기면 2번째부터 모든 커밋이 hash 검사에서 조용히 drop된다.
fn parse_history_log(stdout: &str) -> Vec<Commit> {
    let mut items = Vec::new();
    for raw_record in stdout.split('\0') {
        // F4: Orca `replace(/^\n+/, '')` — 선행 개행 전부 제거.
        let record = raw_record.trim_start_matches('\n');
        if record.trim().is_empty() {
            continue;
        }

        let lines: Vec<&str> = record.split('\n').collect();
        let hash = lines.first().map(|s| s.trim()).unwrap_or("");
        if !is_hex_hash(hash) {
            continue;
        }

        // 고정 오프셋(F2): [1]=aN [2]=aE [3]=at [4]=ct(미사용) [5]=P [6]=decorate.
        let author_name = lines.get(1).copied().unwrap_or("").to_string();
        let author_email = lines.get(2).copied().unwrap_or("").to_string();
        let author_timestamp = lines
            .get(3)
            .and_then(|s| s.trim().parse::<i64>().ok())
            .unwrap_or(0);
        let parents_line = lines.get(5).map(|s| s.trim()).unwrap_or("");
        let decorations = lines.get(6).copied().unwrap_or("");

        // body = [7..]을 개행으로 다시 이어붙이고 후행 개행 1개 제거(Orca `replace(/\n$/, '')`).
        let body = if lines.len() > 7 {
            let joined = lines[7..].join("\n");
            joined
                .strip_suffix('\n')
                .map(str::to_string)
                .unwrap_or(joined)
        } else {
            String::new()
        };

        let parents = if parents_line.is_empty() {
            Vec::new()
        } else {
            parents_line.split(' ').map(String::from).collect()
        };

        items.push(Commit {
            id: hash.to_string(),
            short_id: short_hash(hash),
            author_name,
            author_email,
            author_timestamp,
            parents,
            references: parse_decoration_refs(decorations, hash),
            subject: commit_subject(&body),
            body,
        });
    }
    items
}

/// decoration 필드(`%x1f`-분리)를 분류된 ref 목록으로. Orca `parseGitDecorationRefs`
/// (`git-history-log-parser.ts:17-76`). **F3: `\u{1f}` 하나로만 분리**(comma 폴백
/// 미이식, git ≥2.34 전제).
fn parse_decoration_refs(raw: &str, revision: &str) -> Vec<Ref> {
    if raw.trim().is_empty() {
        return Vec::new();
    }

    let mut refs = Vec::new();
    for part in raw.split(DECORATION_SEPARATOR) {
        let r = part.trim();
        if r.is_empty() || r == "HEAD" || is_remote_head(r) {
            continue;
        }

        if let Some(rest) = r.strip_prefix("HEAD -> refs/heads/") {
            refs.push(Ref {
                id: format!("refs/heads/{rest}"),
                name: rest.to_string(),
                revision: revision.to_string(),
                category: RefCategory::Branches,
            });
            continue;
        }
        if let Some(name) = r.strip_prefix("refs/heads/") {
            refs.push(Ref {
                id: r.to_string(),
                name: name.to_string(),
                revision: revision.to_string(),
                category: RefCategory::Branches,
            });
            continue;
        }
        if let Some(name) = r.strip_prefix("refs/remotes/") {
            refs.push(Ref {
                id: r.to_string(),
                name: name.to_string(),
                revision: revision.to_string(),
                category: RefCategory::RemoteBranches,
            });
            continue;
        }
        if let Some(name) = r.strip_prefix("tag: refs/tags/") {
            refs.push(Ref {
                id: format!("refs/tags/{name}"),
                name: name.to_string(),
                revision: revision.to_string(),
                category: RefCategory::Tags,
            });
        }
        // 그 외(예: `tag: refs/tags/` 아닌 형식)는 버린다 — Orca와 동일.
    }

    refs.sort_by(compare_refs_by_category);
    refs
}

/// `^refs/remotes/[^/]+/HEAD(?:\s|$)` 매칭. 리모트의 심볼릭 HEAD pill을 거른다.
/// (regex 크레이트 없이 수동 매칭.)
fn is_remote_head(r: &str) -> bool {
    let Some(rest) = r.strip_prefix("refs/remotes/") else {
        return false;
    };
    // rest = "<remote>/HEAD..." 이고 <remote>에는 '/'가 없다.
    let Some(slash) = rest.find('/') else {
        return false;
    };
    if slash == 0 {
        return false; // 빈 remote 이름
    }
    let after = &rest[slash + 1..];
    match after.strip_prefix("HEAD") {
        Some(tail) => tail.is_empty() || tail.starts_with(char::is_whitespace),
        None => false,
    }
}

/// category 우선순위(heads<remotes<tags<나머지) 후 이름순. Orca
/// `compareGitHistoryItemRefsByCategory`(`git-history-log-parser.ts:78-97`).
fn compare_refs_by_category(a: &Ref, b: &Ref) -> std::cmp::Ordering {
    fn order(r: &Ref) -> u8 {
        if r.id.starts_with("refs/heads/") {
            1
        } else if r.id.starts_with("refs/remotes/") {
            2
        } else if r.id.starts_with("refs/tags/") {
            3
        } else {
            99
        }
    }
    order(a).cmp(&order(b)).then_with(|| a.name.cmp(&b.name))
}

/// 커밋 메시지 첫 줄(trim). 비었으면 `(no commit message)`. Orca `commitSubject`.
fn commit_subject(message: &str) -> String {
    let first_line = message.split('\n').next().unwrap_or("");
    // `\r?\n` 세만틱: 첫 줄 끝의 `\r`만 제거(CRLF).
    let first = first_line.strip_suffix('\r').unwrap_or(first_line).trim();
    if first.is_empty() {
        "(no commit message)".to_string()
    } else {
        first.to_string()
    }
}

/// 앞 7자 축약. Orca `shortGitHash`.
fn short_hash(hash: &str) -> String {
    hash.chars().take(7).collect()
}

/// `^[0-9a-fA-F]{40,64}$`. Orca의 hash 검사와 동일(SHA-1 40 ~ SHA-256 64).
fn is_hex_hash(s: &str) -> bool {
    (40..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **F4 strip은 hermetic 유닛 테스트로만 검증된다.** 실측 git 2.50.1은 `-z`
    /// 레코드에 선행 개행을 **안 붙여서**(직접 확인) 통합 테스트로는 이 strip이
    /// 죽지 않는다. 하지만 Orca가 관측했듯 일부 git 버전/설정은 레코드마다 선행
    /// 개행을 단다 — 그 입력을 합성해 strip이 load-bearing임을 못박는다. strip을
    /// 지우면 line[0](hash)이 빈값이 돼 커밋이 통째로 drop된다.
    #[test]
    fn strips_leading_newline_per_record() {
        let h1 = "1".repeat(40);
        let h2 = "2".repeat(40);
        // 각 레코드에 선행 `\n`. 필드 순서: hash/aN/aE/at/ct/P/decorate/body.
        let rec = |h: &str, subj: &str| format!("\n{h}\nauthor\ne@x\n100\n100\n\n\n{subj}\n");
        let stdout = format!("{}\0{}\0", rec(&h1, "first"), rec(&h2, "second"));

        let items = parse_history_log(&stdout);
        assert_eq!(items.len(), 2, "선행 개행 strip이 없으면 커밋이 drop된다");
        assert_eq!(items[0].id, h1);
        assert_eq!(items[0].subject, "first");
        assert_eq!(items[0].parents, Vec::<String>::new());
        assert_eq!(items[1].id, h2);
        assert_eq!(items[1].subject, "second");
    }
}
