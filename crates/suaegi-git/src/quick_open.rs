//! M2a — Quick Open(퍼지 파인더) 리스터 캐스케이드의 **순수 빌더/파서**.
//!
//! 프로세스 스폰은 전혀 없다(그건 M2b). 여기 있는 건 전부 순수 함수라 I/O 없이
//! 단위/뮤테이션 테스트로 고정된다. Orca(`v1.4.150-rc.0`)의 아래 순수 모듈을 이식한다:
//! - rg/git ls-files argv 빌더: `shared/quick-open-filter.ts`
//! - `-s` 스테이지 엔트리 파서 + `classifyQuickOpenGitEntry` 4분기: `shared/quick-open-readdir-walk.ts`
//! - `collapseQuickOpenExpansionPaths`의 `includeSymlinks` OR 전파: `shared/quick-open-expansion-paths.ts`
//! - excludePaths 정규화: `shared/quick-open-filter.ts`(`buildExcludePathPrefixes` 내부 규칙)
//!
//! `-z` 스트림은 `status.rs`의 NUL-split 규율(`stdout.split('\0')`, 빈 조각 skip)을 재사용한다.
//!
//! M2b(이 파일 하단)는 위 순수 표면을 호출하는 **드라이버 캐스케이드**를 더한다:
//! rg → git ls-files → raw walk. 핵심 규율은 **transient≠empty**(Codex fix 5/6): 실패/타임아웃
//! 티어는 부분 버퍼를 **버리고** 캐스케이드하거나 하드-에러한다 — 절대 잘린 목록을 완전한 것처럼
//! 반환하지 않는다(무성 절단 = Quick Open이 파일을 놓침 = 저장소의 대죄).

use crate::fs::list_dir;
use crate::runner::GitRunner;
use std::collections::BTreeSet;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};

#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;

// ─── glob 이스케이프 (Orca quick-open-filter.ts:157-171) ───────────────

/// rg/git glob 메타문자. `feature[1]`이라는 디렉터리가 `feature1`을 제외하지 않도록
/// 세그먼트 안의 이 문자들을 백슬래시로 escape한다(Orca `GLOB_META`, quick-open-filter.ts:157).
fn is_glob_meta(ch: char) -> bool {
    matches!(ch, '*' | '?' | '[' | ']' | '{' | '}' | '\\')
}

/// 한 세그먼트를 escape(Orca `escapeGlob`, quick-open-filter.ts:159-166).
fn escape_glob(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for ch in segment.chars() {
        if is_glob_meta(ch) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// `/`로 나눠 각 세그먼트만 escape하고 다시 `/`로 잇는다 — 구분자 `/`는 escape하지
/// 않는다(Orca `escapeGlobPath`, quick-open-filter.ts:168-171).
fn escape_glob_path(rel_path: &str) -> String {
    rel_path
        .split('/')
        .map(escape_glob)
        .collect::<Vec<_>>()
        .join("/")
}

// ─── rg argv 빌더 (Orca quick-open-filter.ts:185-251) ─────────────────

/// rg `--files` 패스의 argv를 만든다(searchRoot/`cwd`는 M2b가 붙인다).
///
/// - base: `--files --hidden`(dotfile 포함; `--follow`는 **의도적 생략** — 심링크가 root
///   밖으로 새거나 루프를 못 돌게, Orca:219의 주석).
/// - `include_ignored`(ignored 패스): `--no-ignore-vcs` 추가 — gitignore/부모/전역-ignore
///   까지 넓힌다(Orca `ignoredPass`, quick-open-filter.ts:240-248).
/// - 각 exclude → **directory-match form** `!**/<escaped>` glob 하나. contents-form
///   `!**/<name>/**`이 아니다: rg는 contents-form만 매칭된 디렉터리로 여전히 내려가므로
///   directory-form만 실제로 traversal을 prune한다(Orca `buildHiddenDirExcludeGlobs`,
///   quick-open-filter.ts:180-195). 값은 `escape_glob_path`로 escape하고 **항상 `!` 접두**라
///   `-foo` 같은 악의적 exclude 값이 argv 플래그로 해석될 수 없다.
///
/// 반환 argv는 `--glob <값>` 쌍이 flat하게 들어간다(searchRoot 미포함).
///
/// ⚠️ **`excludes`는 basename-anywhere blocklist 이름 전용**(`node_modules`, `.git` 같은
/// 디렉터리 *이름*)이다 — `!**/name`은 그 이름을 **어느 깊이에서든** prune한다. **nested-worktree
/// rooted prefix(`packages/app` 같은)를 여기 넣지 말 것**: rooted prefix는 rg에서 rooted
/// `!<prefix>` + `!<prefix>/**` 두-glob 형식이 필요하고(Orca quick-open-filter.ts:224-228),
/// `!**/prefix`로 넣으면 같은 이름의 무관한 하위 경로까지 over-prune된다. worktree prefix는
/// M3가 별도 경로로 처리한다(git 쪽은 `ls_files_args`가 이미 rooted `:(exclude,glob)`로 받음).
pub fn rg_args(include_ignored: bool, excludes: &[String]) -> Vec<String> {
    let mut args = vec!["--files".to_string(), "--hidden".to_string()];
    if include_ignored {
        args.push("--no-ignore-vcs".to_string());
    }
    for ex in excludes {
        args.push("--glob".to_string());
        args.push(format!("!**/{}", escape_glob_path(ex)));
    }
    args
}

// ─── git ls-files argv 빌더 (Orca quick-open-filter.ts:312-344) ────────

/// `git ls-files` argv를 만든다.
///
/// - primary: `-z -s --cached --others --exclude-standard --directory --no-empty-directory`.
///   `-z`는 진짜 git 경로를 NUL로 보존하고, `-s`(stage mode)는 lstat 없이 gitlink를
///   식별하게 한다(Orca:324). `--directory --no-empty-directory`는 untracked 트리를 collapse해
///   caller가 bounded walker로만 확장하게 한다(Orca:321-322).
/// - `include_ignored`(ignored 패스): `--cached`를 빼고 `--ignored`를 넣는다(Orca:334-342).
/// - `exclude_prefixes`가 있으면 끝에 `-- . :(exclude,glob)<p> :(exclude,glob)<p>/**`를
///   붙인다. 양성 `.` pathspec을 먼저 둬 exclude-only pathspec이 git의 edge-case 기본값에
///   의존하지 않게 한다(Orca:311, 320). `:(exclude,glob)` 접두 + `escape_glob_path`라 `-foo`
///   같은 prefix가 플래그로 샐 수 없고, `--`가 이후 전부를 pathspec으로 못박는다.
pub fn ls_files_args(include_ignored: bool, exclude_prefixes: &[String]) -> Vec<String> {
    let mut args = vec!["-z".to_string(), "-s".to_string()];
    if include_ignored {
        // ignored 패스: --cached 없이 --others --ignored.
        args.push("--others".to_string());
        args.push("--ignored".to_string());
    } else {
        args.push("--cached".to_string());
        args.push("--others".to_string());
    }
    args.push("--exclude-standard".to_string());
    args.push("--directory".to_string());
    args.push("--no-empty-directory".to_string());

    if !exclude_prefixes.is_empty() {
        args.push("--".to_string());
        args.push(".".to_string());
        for p in exclude_prefixes {
            let escaped = escape_glob_path(p);
            args.push(format!(":(exclude,glob){escaped}"));
            args.push(format!(":(exclude,glob){escaped}/**"));
        }
    }
    args
}

// ─── `-s` 스테이지 엔트리 파서 (Orca quick-open-readdir-walk.ts:35-51) ──

/// `git ls-files -z -s`가 내는 한 레코드를 뜯은 결과.
///
/// `-z` 스트림의 레코드는 두 형태 중 하나다(Orca `parseQuickOpenGitLsFilesEntry`):
/// 1. **스테이지 엔트리**(`--cached`/`--ignored`가 낸 tracked 경로): `<mode> <hash> <stage>\t<path>`.
///    `-s`가 준 mode/hash가 앞에 붙는다. `is_gitlink`은 mode가 `160000`.
/// 2. **untracked 엔트리**(`--others`가 낸 경로): mode/hash 접두가 **없다** — 경로 그 자체.
///    `--directory` collapse로 디렉터리 placeholder면 끝에 `/`가 붙어 `is_untracked_dir`.
///
/// 스테이지 정규식(하드코딩 금지)의 hex 폭이 crux다: `[0-9a-f]{40,64}` — SHA-1(40) **그리고**
/// SHA-256(64) 둘 다 매칭(Codex fix 2). 40으로 굳히면 SHA-256 저장소에서 스테이지 엔트리를
/// untracked로 오인해 gitlink 감지가 깨진다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitLsFilesEntry<'a> {
    /// 레코드의 경로. 스테이지 엔트리는 tab 뒤, untracked 엔트리는 레코드 전체.
    pub path: &'a str,
    /// mode가 `160000`(gitlink/서브모듈)인가.
    pub is_gitlink: bool,
    /// untracked 디렉터리 placeholder(`--directory` collapse가 남긴 `/` 종결)인가.
    pub is_untracked_dir: bool,
}

/// hex 소문자 자릿수(`[0-9a-f]`)인가.
fn is_lower_hex(b: u8) -> bool {
    b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
}

/// 레코드가 `-s` 스테이지 형태 `^([0-7]{6}) [0-9a-f]{40,64} [0-3]\t`면 `(mode, tab 뒤 path)`를
/// 돌려준다. 아니면 `None`(untracked `--others` 라인). **regex 크레이트 없이** 고정형 접두를
/// 손으로 파싱한다(suaegi-git에 `regex` 의존이 없고 새 dep을 피한다 — plan 제약).
///
/// hex 필드는 정규식의 greedy `{40,64}`와 동치: 연속 hex를 다 먹은 뒤 길이가 `40..=64`
/// 밖이면 매칭 실패(정규식이 backtrack해도 전부 hex라 space를 못 찾아 no-match인 것과 같다).
fn parse_stage_entry(record: &str) -> Option<(&str, &str)> {
    let bytes = record.as_bytes();
    // mode: [0-7]{6}
    if bytes.len() < 6 {
        return None;
    }
    for &b in &bytes[..6] {
        if !(b'0'..=b'7').contains(&b) {
            return None;
        }
    }
    let mut i = 6;
    // 공백
    if bytes.get(i) != Some(&b' ') {
        return None;
    }
    i += 1;
    // hash: [0-9a-f]{40,64}
    let hash_start = i;
    while i < bytes.len() && is_lower_hex(bytes[i]) {
        i += 1;
    }
    let hash_len = i - hash_start;
    if !(40..=64).contains(&hash_len) {
        return None;
    }
    // 공백
    if bytes.get(i) != Some(&b' ') {
        return None;
    }
    i += 1;
    // stage: [0-3]
    match bytes.get(i) {
        Some(&b) if (b'0'..=b'3').contains(&b) => {}
        _ => return None,
    }
    i += 1;
    // 탭
    if bytes.get(i) != Some(&b'\t') {
        return None;
    }
    i += 1;
    // ASCII만으로 여기까지 왔으므로 i는 char 경계다.
    Some((&record[..6], &record[i..]))
}

/// `-s` 스테이지 레코드에서 tab 뒤 경로만 뽑는다. 스테이지 형태가 아니면 `None`.
/// (task 계약 시그니처 — gitlink/untracked 판정은 `parse_ls_files_entry`가 한다.)
pub fn parse_ls_files_stage_path(record: &str) -> Option<&str> {
    parse_stage_entry(record).map(|(_mode, path)| path)
}

/// 한 `-z` 레코드를 `GitLsFilesEntry`로(Orca `parseQuickOpenGitLsFilesEntry`, :37-51).
/// 스테이지 형태면 mode로 gitlink 판정, 아니면 untracked(끝-`/`면 디렉터리 placeholder).
pub fn parse_ls_files_entry(record: &str) -> GitLsFilesEntry<'_> {
    if let Some((mode, path)) = parse_stage_entry(record) {
        GitLsFilesEntry {
            path,
            is_gitlink: mode == "160000",
            is_untracked_dir: false,
        }
    } else {
        GitLsFilesEntry {
            path: record,
            is_gitlink: false,
            is_untracked_dir: record.ends_with('/'),
        }
    }
}

/// `git ls-files -z ...` stdout 전체를 레코드로 쪼개 파싱한다. NUL split + 빈 조각 skip은
/// `status.rs`의 `-z` 규율과 같다(마지막 NUL 뒤 빈 조각을 엔트리로 오인하지 않는다).
pub fn parse_ls_files_stream(stdout: &str) -> Vec<GitLsFilesEntry<'_>> {
    stdout
        .split('\0')
        .filter(|r| !r.is_empty())
        .map(parse_ls_files_entry)
        .collect()
}

// ─── classify 4분기 (Orca quick-open-readdir-walk.ts:97-127) ──────────

/// 디렉터리 placeholder에 대한 lstat 프로브 결과. M2b가 실제 lstat을 해 이 중 하나로
/// 요약하고 `classify_quick_open_git_entry`에 넘긴다(순수). Orca 4분기의 입력을 모델링한다:
/// - `OrdinaryFile`: gitlink도 untracked-dir placeholder도 아님 → lstat 자체가 불필요.
/// - 나머지 4개는 gitlink/untracked-dir라 lstat을 한 뒤의 상태다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitEntryProbe {
    /// 평범한 tracked 파일(gitlink 아님 & untracked-dir 아님). lstat 없이 keep.
    OrdinaryFile,
    /// gitlink/untracked-dir인데 lstat이 실패(catch).
    LstatFailed,
    /// lstat 성공했지만 디렉터리가 아님.
    NotADir,
    /// 디렉터리 + `.git`(파일/디렉터리) 존재 → 중첩 저장소.
    DirWithGit,
    /// 디렉터리인데 `.git` 없음.
    DirWithoutGit,
}

/// classify가 내리는 결정.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitEntryAction {
    /// 결과에 그대로 넣는다.
    Keep,
    /// placeholder를 버린다(확장 안 함).
    DropPlaceholder,
    /// 중첩 저장소를 bounded walk로 채운다.
    FillNestedRepo,
}

/// Orca `classifyQuickOpenGitEntry`(quick-open-readdir-walk.ts:97-127)의 **4분기 결정을
/// 순수 함수로**. lstat/hasGitEntry는 M2b가 하고 결과를 `GitEntryProbe`로 요약해 넘긴다.
///
/// | probe          | action          | Orca 근거 |
/// |----------------|-----------------|-----------|
/// | OrdinaryFile   | Keep            | :107-109  `!isGitlink && !isUntrackedDir → keep`(lstat 없이) |
/// | LstatFailed    | DropPlaceholder | :112-116  lstat catch → drop |
/// | NotADir        | DropPlaceholder | :118-120  `!isDirectory` → drop |
/// | DirWithGit     | FillNestedRepo  | :122-124  `hasGitEntry` → fill-nested-repo |
/// | DirWithoutGit  | DropPlaceholder | :126      dir 무-`.git` → drop |
///
/// (빈 relPath drop(:103-105)은 정규화 단계 책임이라 여기 입력엔 안 온다.)
pub fn classify_quick_open_git_entry(probe: GitEntryProbe) -> GitEntryAction {
    match probe {
        GitEntryProbe::OrdinaryFile => GitEntryAction::Keep,
        GitEntryProbe::LstatFailed => GitEntryAction::DropPlaceholder,
        GitEntryProbe::NotADir => GitEntryAction::DropPlaceholder,
        GitEntryProbe::DirWithGit => GitEntryAction::FillNestedRepo,
        GitEntryProbe::DirWithoutGit => GitEntryAction::DropPlaceholder,
    }
}

// ─── excludePaths 정규화 (Orca quick-open-filter.ts:98-133, 173-176) ──

/// worktree-상대 exclude 후보 하나를 `/`-구분 root-relative prefix로 정규화한다.
/// malformed / root 밖 / root-equal(`""`·`.`)은 **조용히 drop**(`None`) — stale하거나 오타난
/// exclude 경로가 요청 전체를 실패시키지 못하게(Orca `buildExcludePathPrefixes`의 per-entry
/// 규칙, quick-open-filter.ts:107-131).
///
/// 규칙(순서):
/// 1. 백슬래시 → `/`.
/// 2. absolute(`/…`) → `None`(root 밖, Orca:122).
/// 3. 끝 `/` 제거(경계 검사 명확화, Orca:126). 남은 게 root-equal(`""`/`.`)이면 `None`
///    (전체 트리를 exclude 거부, Orca:114-117·127). trim 뒤 한 번만 검사하면 `""`·`.`·
///    `./`·`packages/app/`가 전부 이 한 관문을 지난다 — trim 전 별도 검사는 redundant.
/// 4. **어떤 위치의 `..` 세그먼트든**(leading `../x`이든 mid-path `a/../y`이든) → `None`.
///    Orca는 leading `..`만 거부하고 나머지는 경로 해소가 root 밖으로 못 나가는 M2b 전제에
///    기대지만, 여기선 **방어적으로** 경로 해소 없이 `..`가 있으면 무조건 거부한다 — exclude
///    prefix가 root 밖을 가리키는 위험을 원천 차단(리뷰 F4). `..`는 유닉스에서 항상 부모
///    디렉터리 엔트리라 진짜 파일명이 아니므로 오탐 없음. `..name`은 `..`가 아니라 통과.
pub fn normalize_exclude_path(worktree_rel: &str) -> Option<String> {
    let fwd = worktree_rel.replace('\\', "/");
    if fwd.starts_with('/') {
        return None;
    }
    let trimmed = fwd.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "." {
        return None;
    }
    // 어떤 세그먼트든 `..`면 거부(root 밖 탈출 원천 차단).
    if trimmed.split('/').any(|seg| seg == "..") {
        return None;
    }
    Some(trimmed.to_string())
}

// ─── expansion path collapse (Orca quick-open-expansion-paths.ts:5-37) ─

/// 확장할 디렉터리 placeholder 하나. `include_symlinks`는 이 subtree를 walk할 때 심링크 leaf를
/// 결과에 넣을지다.
///
/// **생성자로 flag를 못박는다**(Orca `expandQuickOpenGitFileListing`, :292-326):
/// - `untracked_dir`: collapse 전 git이 untracked 심링크를 leaf로 보여줬으므로 재확장 때
///   누락하면 안 됨 → **강제 `true`**(Orca directoryPaths, :325).
/// - `gitlink`: 중첩 저장소 확장은 심링크 leaf를 포함하지 않음 → **`false`**(Orca gitPaths 기본, :305).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpansionPath {
    pub rel: String,
    pub include_symlinks: bool,
}

impl ExpansionPath {
    /// untracked 디렉터리 placeholder → `include_symlinks: true`.
    pub fn untracked_dir(rel: impl Into<String>) -> Self {
        Self {
            rel: rel.into(),
            include_symlinks: true,
        }
    }

    /// gitlink 파생 placeholder → `include_symlinks: false`.
    pub fn gitlink(rel: impl Into<String>) -> Self {
        Self {
            rel: rel.into(),
            include_symlinks: false,
        }
    }
}

/// descendant placeholder를 이미 덮는 ancestor에 병합한다(Orca `collapseQuickOpenExpansionPaths`,
/// quick-open-expansion-paths.ts:5-37). 정렬로 ancestor가 먼저 오고, `/`-경계 prefix 조회로
/// ancestor를 찾는다.
///
/// **crux — `includeSymlinks` OR 전파**: descendant가 ancestor에 병합될 때 descendant의
/// `include_symlinks`가 `true`면 ancestor를 `true`로 **승격**한다(Orca:28-30). primary/ignored
/// 패스가 겹칠 때 어느 패스의 심링크-leaf 계약도 잃지 않기 위함. descendant가 `false`면
/// ancestor 값은 그대로(다운그레이드 없음 — OR).
pub fn collapse_expansion_paths(paths: Vec<ExpansionPath>) -> Vec<ExpansionPath> {
    // 정렬: ancestor("a")가 descendant("a/b")보다 먼저 오도록 rel 오름차순.
    let mut sorted = paths;
    sorted.sort_by(|a, b| a.rel.cmp(&b.rel));

    // 삽입 순서를 지키는 (rel -> include_symlinks) 맵.
    let mut collapsed: Vec<ExpansionPath> = Vec::new();

    for ep in sorted {
        // `/` 경계마다 prefix가 이미 collapsed에 있는지 조회(Orca:14-23).
        let mut ancestor_idx: Option<usize> = None;
        let mut search_from = 0;
        while let Some(pos) = ep.rel[search_from..].find('/') {
            let slash = search_from + pos;
            let candidate = &ep.rel[..slash];
            if let Some(idx) = collapsed.iter().position(|c| c.rel == candidate) {
                ancestor_idx = Some(idx);
                break;
            }
            search_from = slash + 1;
        }

        if let Some(idx) = ancestor_idx {
            // OR 전파: descendant true면 ancestor 승격. false면 유지.
            if ep.include_symlinks {
                collapsed[idx].include_symlinks = true;
            }
            continue;
        }

        // ancestor 없음: 삽입. 같은 rel key가 이미 있으면 `include_symlinks`를 **OR**한다
        // (덮어쓰기 금지) — untracked_dir(true)와 gitlink(false)가 같은 rel로 둘 다 오면
        // (primary/ignored 패스 overlap) 심링크-leaf 계약을 잃지 않게 true를 유지한다.
        if let Some(existing) = collapsed.iter_mut().find(|c| c.rel == ep.rel) {
            existing.include_symlinks |= ep.include_symlinks;
        } else {
            collapsed.push(ep);
        }
    }

    collapsed
}

// ═══════════════════════════════════════════════════════════════════════
// M2b — 드라이버 캐스케이드 (Orca filesystem-list-files.ts / -git-fallback.ts /
//        quick-open-readdir-walk.ts / rg-availability.ts)
// ═══════════════════════════════════════════════════════════════════════

/// raw walk의 하드 파일 상한. 넘기면 **truncate가 아니라 throw**한다(Codex fix 6).
/// Orca `QUICK_OPEN_READDIR_MAX_FILES`(quick-open-readdir-budget.ts:1).
pub const QUICK_OPEN_READDIR_MAX_FILES: usize = 10_000;

/// raw walk의 데드라인. 넘기면 **throw**한다. Orca `QUICK_OPEN_READDIR_TIMEOUT_MS`(:2).
pub const QUICK_OPEN_READDIR_TIMEOUT: Duration = Duration::from_secs(10);

/// rg 가용성 프로브 타임아웃(Orca `RG_AVAILABILITY_TIMEOUT_MS`, rg-availability.ts:3).
const RG_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// rg 리스팅 패스 타임아웃(Orca 각 패스 10s, filesystem-list-files.ts:193).
const RG_TIMEOUT: Duration = Duration::from_secs(10);

/// git 호출 타임아웃. **GitRunner 기본 30s가 아니라 10s**(Codex fix 4) — Orca는 rev-parse/
/// ls-files를 10s로 돌린다(-git-fallback.ts:73,231). 기본 30s를 쓰면 3배 느린 실패=UX 회귀.
const GIT_TIMEOUT: Duration = Duration::from_secs(10);

/// Quick Open 리스터 캐스케이드의 실패 분류. UI가 각 클래스에 맞는 안내(예: rg 설치)를
/// 띄울 수 있도록 구분한다. Orca는 walk budget 초과/타임아웃만 "install ripgrep" 안내로
/// 번역한다(`isQuickOpenReaddirBudgetError`, quick-open-readdir-budget.ts:22) — `suggests_install_rg`.
#[derive(Debug, thiserror::Error)]
pub enum QuickOpenError {
    /// rg 런이 실패(타임아웃/시그널-킬/비정상 종료/exit2-무결과). **git 폴백 없이 하드 에러**
    /// — upfront 프로브만 git 캐스케이드를 트리거한다(Codex fix 5).
    #[error("ripgrep listing failed: {detail}")]
    RgFailed { detail: String },
    /// git ls-files primary 패스가 실패. **walk 캐스케이드 없이 하드 리젝트**(Codex fix 5).
    #[error("git ls-files failed: {detail}")]
    GitLsFilesFailed { detail: String },
    /// raw walk가 `max` 파일 상한을 넘겼다. **truncate가 아니라 에러**(Codex fix 6).
    #[error("file listing exceeded {max} files")]
    WalkCapExceeded { max: usize },
    /// raw walk가 데드라인을 넘겼다. **부분 목록이 아니라 에러**(Codex fix 6).
    #[error("file listing timed out")]
    WalkTimeout,
    /// 프로세스 스폰 IO 실패(rg 패스 spawn 등).
    #[error("failed to spawn {program}: {source}")]
    Spawn {
        program: &'static str,
        source: std::io::Error,
    },
}

impl QuickOpenError {
    /// UI가 "ripgrep을 설치하면 더 빠릅니다" 안내를 띄워야 하는가. Orca 패리티:
    /// walk budget(초과/타임아웃)만 그 안내로 번역된다(quick-open-readdir-budget.ts:21-25).
    pub fn suggests_install_rg(&self) -> bool {
        matches!(self, Self::WalkCapExceeded { .. } | Self::WalkTimeout)
    }
}

/// **캐스케이드 진입점**. worktree-상대 경로(forward-slash)를 돌려준다.
///
/// 1. rg 가용성 **upfront 1회** 프로브(5s). 없으면 → **git 캐스케이드**(Orca가 upfront로
///    프로브하는 이유: spawn('rg')의 error/close 경쟁으로 무성 빈결과가 나던 버그를 없애려고,
///    filesystem-list-files.ts:39-52).
/// 2. rg 있으면 → rg 2패스. 런 실패는 **하드 에러**(second-chance git 폴백 금지).
/// 3. git 티어는 rev-parse가 not-a-worktree면 walk로 soft-fail; 확정 워크트리면 ls-files
///    실패는 하드 리젝트.
pub async fn list_quick_open_files(
    worktree: &Path,
    excludes: &[String],
) -> Result<Vec<String>, QuickOpenError> {
    let rg = rg_available(worktree).await;
    dispatch(worktree, excludes, rg).await
}

/// rg 가용성 비트를 **주입 가능**하게 분리한 캐스케이드 코어(테스트가 프로브 없이 분기를 고정).
/// rg 없음(upfront) → git; 있음 → rg 하드-에러 경로. 이 함수가 캐스케이드 결정을 담는다.
async fn dispatch(
    worktree: &Path,
    excludes: &[String],
    rg_available: bool,
) -> Result<Vec<String>, QuickOpenError> {
    if rg_available {
        // rg 있으면 rg만 — 런 실패해도 git으로 새지 않는다(Codex fix 5).
        list_with_rg(worktree, excludes).await
    } else {
        list_with_git(worktree, excludes).await
    }
}

// ─── Tier 1: rg (Orca filesystem-list-files.ts, rg-availability.ts) ────

/// `rg --version`을 5s 타임아웃으로 스폰해 가용성을 본다. spawn ENOENT(rg 없음)나 비-0 종료나
/// 타임아웃 → `false`(→ git 캐스케이드). Node의 error/close 경쟁을 위한 settled-guard는
/// Rust에선 불필요하다 — `spawn()`이 즉시 `Err`을 주거나 `wait()`이 단일 status를 주므로
/// 이중 resolve가 원천적으로 없다(rg-availability.ts:6-15의 문제가 여기선 발생 불가).
async fn rg_available(worktree: &Path) -> bool {
    rg_available_program(worktree, "rg").await
}

async fn rg_available_program(worktree: &Path, program: &str) -> bool {
    let mut cmd = Command::new(program);
    cmd.arg("--version")
        .current_dir(worktree)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        // spawn ENOENT = rg 미설치 → 캐스케이드(하드 에러 아님).
        Err(_) => return false,
    };
    match tokio::time::timeout(RG_PROBE_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) => status.success(),
        Ok(Err(_)) => false,
        Err(_) => {
            kill_child(&mut child);
            let _ = tokio::time::timeout(REAP_TIMEOUT, child.wait()).await;
            false
        }
    }
}

/// rg 2패스(primary, ignored)를 돌려 합집합을 돌려준다. **어느 패스든 하드 실패하면 전체가
/// 하드 에러**(Orca `Promise.all` reject 패리티, filesystem-list-files.ts:226/235-237).
async fn list_with_rg(worktree: &Path, excludes: &[String]) -> Result<Vec<String>, QuickOpenError> {
    list_with_rg_using(worktree, "rg", excludes, RG_TIMEOUT).await
}

/// program/timeout 주입 버전(테스트가 가짜 rg 스크립트 + 짧은 타임아웃으로 각 종료 시나리오를
/// 고정한다). Orca는 두 패스를 병렬로 돌리지만 여기선 순차다 — 결과 합집합은 동일하고
/// 첫 패스 하드 실패 시 즉시 반환(둘째 패스 스킵)은 `Promise.all` 조기 reject와 동치다.
async fn list_with_rg_using(
    worktree: &Path,
    program: &str,
    excludes: &[String],
    timeout: Duration,
) -> Result<Vec<String>, QuickOpenError> {
    let mut files: BTreeSet<String> = BTreeSet::new();
    for include_ignored in [false, true] {
        let mut args = rg_args(include_ignored, excludes);
        // searchRoot('.')는 M2b가 붙인다(cwd=worktree, cwd-상대 출력). Orca filesystem-list-files.ts:68.
        args.push(".".to_string());
        // `?`: 하드 실패는 즉시 전파(둘째 패스로 절대 넘어가지 않음).
        let paths = run_rg_pass(worktree, program, &args, timeout).await?;
        files.extend(paths);
    }
    Ok(files.into_iter().collect())
}

/// 한 rg 패스를 스폰·수집한다. **transient≠empty(Codex fix 5)**:
/// - 타임아웃 → 킬 + 버퍼 폐기 + `RgFailed`.
/// - 시그널-킬 종료 → 버퍼 폐기 + `RgFailed`.
/// - exit 0/1 → 파싱 결과 resolve.
/// - exit 2 & 파싱 경로 ≥1 → resolve(권한 없는 하위 디렉터리 부분 성공 허용, Orca:153-155).
/// - exit 2 & 0경로 → `RgFailed`.
/// - 그 외 종료 코드 → `RgFailed`.
async fn run_rg_pass(
    worktree: &Path,
    program: &str,
    args: &[String],
    timeout: Duration,
) -> Result<Vec<String>, QuickOpenError> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(worktree)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);

    let mut child = cmd.spawn().map_err(|e| QuickOpenError::Spawn {
        program: "rg",
        source: e,
    })?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");

    // stdout 읽기·stderr 배출·wait를 **동시에** — 파이프가 가득 차 자식이 블록되는 교착 방지
    // (runner.rs의 read/wait 동시 규율과 같다).
    let collected = tokio::time::timeout(timeout, async {
        let mut out = Vec::new();
        let read_out = stdout.read_to_end(&mut out);
        let drain_err = drain_to_end(&mut stderr);
        let (status, read_res, _) = tokio::join!(child.wait(), read_out, drain_err);
        (status, out, read_res)
    })
    .await;

    match collected {
        // 타임아웃: 버퍼는 경로 중간에서 잘렸을 수 있다 → 폐기하고 하드 에러(Orca:187-193).
        Err(_) => {
            kill_child(&mut child);
            let _ = tokio::time::timeout(REAP_TIMEOUT, child.wait()).await;
            Err(QuickOpenError::RgFailed {
                detail: "rg list timed out".to_string(),
            })
        }
        Ok((status, out, read_res)) => {
            let status = status.map_err(|e| QuickOpenError::RgFailed {
                detail: format!("rg wait failed: {e}"),
            })?;
            read_res.map_err(|e| QuickOpenError::RgFailed {
                detail: format!("rg read failed: {e}"),
            })?;

            // 시그널-킬 종료(타임아웃/OOM/외부 킬): 스트림된 접두를 돌려주면 무성 빈결과 버그를
            // 재현한다 → 폐기 + 하드 에러(Orca:137-144).
            #[cfg(unix)]
            if let Some(sig) = status.signal() {
                return Err(QuickOpenError::RgFailed {
                    detail: format!("rg killed by signal {sig}"),
                });
            }

            let paths = parse_rg_stdout(&out);
            let code = status.code().unwrap_or(-1);
            if code == 0 || code == 1 {
                Ok(paths)
            } else if code == 2 && !paths.is_empty() {
                // exit 2 = 일부 하위 디렉터리 읽기 실패지만 나머지는 유효 → 부분 성공 허용.
                Ok(paths)
            } else {
                Err(QuickOpenError::RgFailed {
                    detail: format!("rg exited with code {code}"),
                })
            }
        }
    }
}

/// rg `--files` stdout(줄바꿈 구분, git ls-files의 `-z`와 다르다)을 worktree-상대 경로로.
/// cwd-상대 모드: 끝-`\r` 제거, `./` 접두 제거, `.`/빈/절대/`..` 라인은 스킵
/// (Orca `normalizeQuickOpenRgLine` cwd-relative, quick-open-filter.ts:327-337).
fn parse_rg_stdout(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    let mut out = Vec::new();
    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.is_empty() {
            continue;
        }
        let normalized = line.replace('\\', "/");
        let rel = normalized.strip_prefix("./").unwrap_or(&normalized);
        if rel.is_empty() || rel == "." || rel.starts_with('/') || is_parent_rel(rel) {
            continue;
        }
        out.push(rel.to_string());
    }
    out
}

/// `..` 또는 `../…`(root 밖) 여부. `..name`은 유효한 child(Orca `isParentRelativePath`, :210-213).
fn is_parent_rel(rel: &str) -> bool {
    rel == ".." || rel.starts_with("../")
}

// ─── Tier 2: git ls-files (Orca filesystem-list-files-git-fallback.ts) ─

/// git 티어. rev-parse가 not-a-worktree면 walk로 soft-fail; 확정 워크트리면 primary ls-files
/// 실패는 **하드 리젝트**(walk 캐스케이드 금지). ignored 패스는 best-effort(실패해도 primary 유지).
async fn list_with_git(
    worktree: &Path,
    excludes: &[String],
) -> Result<Vec<String>, QuickOpenError> {
    let runner = GitRunner::new();

    // rev-parse probe: 에러/타임아웃/non-zero/"false"는 **soft-fail → Tier3 walk**(reject 안 함,
    // Orca -git-fallback.ts:62-64,70-73). `run_with_timeout`은 non-zero면 `Err`을 주므로 그
    // 자체가 not-a-worktree 신호다. Orca는 exit 0을 무조건 worktree로 보지만 여기선 stdout
    // "true"까지 확인(확정-워크트리) — bare repo("false")는 walk로 내려간다.
    let inside = matches!(
        runner
            .run_with_timeout(worktree, &["rev-parse", "--is-inside-work-tree"], GIT_TIMEOUT)
            .await,
        Ok(o) if o.stdout.trim() == "true"
    );
    if !inside {
        return list_with_walk(worktree, WalkBudget::new());
    }

    // primary ls-files: 실패(타임아웃/시그널/스폰/비정상 종료)는 **하드 리젝트**(Codex fix 5).
    let primary_args = ls_files_args(false, excludes);
    let primary_out = runner
        .run_with_timeout(worktree, &to_argv("ls-files", &primary_args), GIT_TIMEOUT)
        .await
        .map_err(|e| QuickOpenError::GitLsFilesFailed {
            detail: e.to_string(),
        })?;

    // ignored 패스: **best-effort** — 실패해도 primary 결과를 절대 버리지 않는다
    // (Orca:264-271 `.catch(...)` keeping primary). 성공 stdout만 취하고 실패는 삼킨다.
    let ignored_args = ls_files_args(true, excludes);
    let ignored_stdout = runner
        .run_with_timeout(worktree, &to_argv("ls-files", &ignored_args), GIT_TIMEOUT)
        .await
        .map(|o| o.stdout)
        .unwrap_or_default();

    // 두 패스의 레코드를 keep(즉시 수집) / expansion(placeholder 확장)으로 분류.
    let mut files: BTreeSet<String> = BTreeSet::new();
    let mut expansions: Vec<ExpansionPath> = Vec::new();
    for stdout in [primary_out.stdout.as_str(), ignored_stdout.as_str()] {
        for entry in parse_ls_files_stream(stdout) {
            classify_entry(worktree, entry, &mut files, &mut expansions);
        }
    }

    // placeholder 확장: 하나의 공유 budget으로 collapse된 모든 subtree를 walk
    // (Orca expandQuickOpenGitFileListing의 단일 budget, quick-open-readdir-walk.ts:277).
    let mut budget = WalkBudget::new();
    for ep in collapse_expansion_paths(expansions) {
        walk(
            worktree,
            &ep.rel,
            ep.include_symlinks,
            &mut budget,
            &mut files,
        )?;
    }

    Ok(files.into_iter().collect())
}

/// 한 `ls-files` 레코드를 keep/drop/expansion으로 라우팅(Orca -git-fallback.ts:113-139 +
/// expandQuickOpenGitFileListing:292-326). **untracked 디렉터리 placeholder는 classify를 거치지
/// 않고 무조건 확장(include_symlinks=true)** — git이 이미 untracked 트리로 확인해줬고, collapse
/// 전 untracked 심링크를 leaf로 보여줬으므로 재확장 시 누락 금지(Orca directoryPaths, :308-326).
/// gitlink만 `classify_quick_open_git_entry` 4분기를 탄다(fill이면 include_symlinks=false).
fn classify_entry(
    worktree: &Path,
    entry: GitLsFilesEntry<'_>,
    files: &mut BTreeSet<String>,
    expansions: &mut Vec<ExpansionPath>,
) {
    // normalizeGitEntry: 끝 `/` 제거(quick-open-readdir-walk.ts:67).
    let rel = entry.path.trim_end_matches('/');
    if rel.is_empty() {
        return;
    }

    // untracked 디렉터리 → directoryPaths: lstat/classify 없이 강제 확장(true).
    if entry.is_untracked_dir {
        expansions.push(ExpansionPath::untracked_dir(rel));
        return;
    }

    // gitPaths(평범 파일/untracked 파일/gitlink): classify 4분기.
    let probe = if entry.is_gitlink {
        probe_dir(worktree, rel)
    } else {
        GitEntryProbe::OrdinaryFile
    };
    match classify_quick_open_git_entry(probe) {
        GitEntryAction::Keep => {
            files.insert(rel.to_string());
        }
        GitEntryAction::DropPlaceholder => {}
        // gitlink 중첩 저장소 → 확장하되 심링크 leaf는 제외(include_symlinks=false).
        GitEntryAction::FillNestedRepo => expansions.push(ExpansionPath::gitlink(rel)),
    }
}

/// gitlink placeholder를 lstat해 `GitEntryProbe`로 요약(Orca classifyQuickOpenGitEntry의
/// lstat 분기, quick-open-readdir-walk.ts:111-126). lstat은 `symlink_metadata`(링크 미추적).
fn probe_dir(worktree: &Path, rel: &str) -> GitEntryProbe {
    let abs = join_rel(worktree, rel);
    let meta = match std::fs::symlink_metadata(&abs) {
        Ok(m) => m,
        Err(_) => return GitEntryProbe::LstatFailed,
    };
    if !meta.file_type().is_dir() {
        return GitEntryProbe::NotADir;
    }
    // hasGitEntry: `.git`이 파일 또는 디렉터리로 존재(Orca hasGitEntry, :88-95). 심링크 `.git`은
    // isFile/isDirectory 어느 쪽도 아니라 false.
    let git = abs.join(".git");
    let has_git = std::fs::symlink_metadata(&git)
        .map(|m| m.file_type().is_dir() || m.file_type().is_file())
        .unwrap_or(false);
    if has_git {
        GitEntryProbe::DirWithGit
    } else {
        GitEntryProbe::DirWithoutGit
    }
}

// ─── Tier 3: raw walk (Orca quick-open-readdir-walk.ts:163-264) ────────

/// raw walk의 예산: 남은 파일 수 + 데드라인. Orca `QuickOpenReaddirBudget`
/// (quick-open-readdir-budget.ts:4-16). `max_files`는 에러 메시지용으로 함께 보관한다.
struct WalkBudget {
    remaining: usize,
    max_files: usize,
    deadline: Instant,
}

impl WalkBudget {
    fn new() -> Self {
        Self::with_limits(
            QUICK_OPEN_READDIR_MAX_FILES,
            Instant::now() + QUICK_OPEN_READDIR_TIMEOUT,
        )
    }

    /// 테스트가 작은 cap/과거 데드라인을 주입해 10k 실파일 없이 경계를 검증하도록 분리.
    fn with_limits(max_files: usize, deadline: Instant) -> Self {
        Self {
            remaining: max_files,
            max_files,
            deadline,
        }
    }

    /// 데드라인 체크포인트. 넘겼으면 **truncate가 아니라 에러**(Orca `assertQuickOpenReaddirDeadline`).
    fn check_deadline(&self) -> Result<(), QuickOpenError> {
        if Instant::now() > self.deadline {
            Err(QuickOpenError::WalkTimeout)
        } else {
            Ok(())
        }
    }

    /// 파일 하나를 예산에서 차감. 남은 게 0이면 **에러**(Orca `consumeQuickOpenReaddirFileBudget`).
    fn consume(&mut self) -> Result<(), QuickOpenError> {
        if self.remaining == 0 {
            return Err(QuickOpenError::WalkCapExceeded {
                max: self.max_files,
            });
        }
        self.remaining -= 1;
        Ok(())
    }
}

/// Tier3 진입: worktree 루트를 walk(비-git 폴백). include_symlinks=false(Orca 비-git walk 기본).
fn list_with_walk(worktree: &Path, mut budget: WalkBudget) -> Result<Vec<String>, QuickOpenError> {
    let mut files: BTreeSet<String> = BTreeSet::new();
    walk(worktree, "", false, &mut budget, &mut files)?;
    Ok(files.into_iter().collect())
}

/// bounded BFS. `fs::list_dir`(심링크-디렉터리 refuse) 위에서 `is_dir`만 재귀 — 심링크 dir로는
/// 절대 traverse하지 않는다(무료). 심링크 **leaf**는 `include_symlinks`일 때만 결과에 넣는다.
///
/// **이중 체크포인트(Codex fix 6)**: 데드라인을 각 디렉터리 batch마다(빈 배치·엔트리 0개여도)
/// AND 엔트리별로 검사한다. 빈 디렉터리가 몰린 구간이 데드라인 근처에서 체크포인트를 건너뛰고
/// 무성으로 불완전 목록을 반환하지 못하게 한다(Orca:189/218-219 배치 + :223-224 엔트리).
///
/// 이 walker는 **Tier3와 Tier2 placeholder 확장이 공유**하는 단일 primitive다(별도 구현 아님).
fn walk(
    worktree: &Path,
    start_rel: &str,
    include_symlinks: bool,
    budget: &mut WalkBudget,
    out: &mut BTreeSet<String>,
) -> Result<(), QuickOpenError> {
    let mut queue: Vec<String> = vec![start_rel.to_string()];

    while !queue.is_empty() {
        let mut next: Vec<String> = Vec::new();
        for dir_rel in &queue {
            // 배치 체크포인트 ①: 리스팅 **전**(빈 디렉터리도 여기서 걸린다).
            budget.check_deadline()?;
            // 개별 subtree 읽기 실패(권한 거부/심링크 root 등)는 그 디렉터리만 스킵 —
            // budget 에러가 아니라 정상 degrade(Orca readdir catch → entries=[], :210-213).
            let entries = match list_dir(worktree, dir_rel) {
                Ok(e) => e,
                Err(_) => continue,
            };
            // 배치 체크포인트 ②: 리스팅 **후**(readdir 도중 데드라인이 지나도 reject).
            budget.check_deadline()?;

            for entry in entries {
                // 엔트리별 체크포인트(단일 거대 디렉터리가 batch 경계 없이 오래 도는 경우 대비).
                budget.check_deadline()?;

                let child_rel = if dir_rel.is_empty() {
                    entry.name.clone()
                } else {
                    format!("{dir_rel}/{}", entry.name)
                };

                if entry.is_dir {
                    // `.git` 내부는 절대 나열하지 않는다(중첩 저장소 확장 시 git 내부 폭발 방지).
                    if entry.name != ".git" {
                        next.push(child_rel);
                    }
                    continue;
                }
                // leaf: 일반 파일은 항상, 심링크는 include_symlinks일 때만(Orca:241-243).
                if entry.is_symlink && !include_symlinks {
                    continue;
                }
                budget.consume()?;
                out.insert(child_rel);
            }
        }
        queue = next;
    }
    Ok(())
}

// ─── 공유 스폰 헬퍼 ────────────────────────────────────────────────────

/// 킬 후 자식을 거둘 상한(runner.rs `REAP_TIMEOUT`과 같은 5s).
const REAP_TIMEOUT: Duration = Duration::from_secs(5);

/// Unix: 프로세스 **그룹** 전체에 SIGKILL(자식이 스폰한 손자까지) + start_kill.
/// runner.rs `kill_process_tree`와 같은 규율.
fn kill_child(child: &mut Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    let _ = child.start_kill();
}

/// 남은 바이트를 버리며 EOF까지 읽는다(stderr 배출용 — 담지 않으므로 상한 무관).
async fn drain_to_end<R: AsyncRead + Unpin>(reader: &mut R) {
    let mut buf = [0u8; 8192];
    while let Ok(n) = reader.read(&mut buf).await {
        if n == 0 {
            break;
        }
    }
}

/// `Vec<String>` args를 `[subcommand, ...args]` 형태의 `Vec<&str>`로(GitRunner argv용).
fn to_argv<'a>(subcommand: &'a str, args: &'a [String]) -> Vec<&'a str> {
    let mut argv = Vec::with_capacity(args.len() + 1);
    argv.push(subcommand);
    argv.extend(args.iter().map(String::as_str));
    argv
}

/// worktree-상대 `/`-경로를 절대 경로로 join(세그먼트별 — 플랫폼 구분자 안전).
fn join_rel(worktree: &Path, rel: &str) -> std::path::PathBuf {
    let mut p = worktree.to_path_buf();
    for seg in rel.split('/').filter(|s| !s.is_empty()) {
        p.push(seg);
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── rg argv ───────────────────────────────────────────────────

    #[test]
    fn rg_primary_base_no_ignored() {
        // ignored=false → --no-ignore-vcs 없음.
        assert_eq!(rg_args(false, &[]), vec!["--files", "--hidden"]);
    }

    // crux: ignored 패스가 --no-ignore-vcs를 추가한다. mutation: 추가 라인 삭제 → FAIL.
    #[test]
    fn rg_ignored_adds_no_ignore_vcs() {
        assert_eq!(
            rg_args(true, &[]),
            vec!["--files", "--hidden", "--no-ignore-vcs"]
        );
    }

    // crux(injection): 악의적 `-foo` exclude 값은 항상 `!**/`로 감싸여 argv 플래그가 못 된다.
    // mutation: `!` 접두를 떼면(`format!("**/{}")` 등) 이 assert가 FAIL.
    #[test]
    fn rg_exclude_is_bang_prefixed_directory_form() {
        let args = rg_args(false, &["-foo".to_string()]);
        assert_eq!(args, vec!["--files", "--hidden", "--glob", "!**/-foo"]);
        // exclude 값이 raw 플래그로 새지 않음.
        assert!(!args.iter().any(|a| a == "-foo"));
    }

    // crux: glob 메타문자 escape — `feature[1]`이 `feature1`을 잘못 제외하지 않게.
    #[test]
    fn rg_exclude_escapes_glob_meta() {
        let args = rg_args(false, &["feature[1]".to_string()]);
        assert_eq!(args.last().unwrap(), r"!**/feature\[1\]");
    }

    // 다중 세그먼트 exclude: `/`는 escape 안 되고 세그먼트만 escape.
    #[test]
    fn rg_exclude_multi_segment_keeps_slash() {
        let args = rg_args(false, &[".local/share".to_string()]);
        assert_eq!(args.last().unwrap(), "!**/.local/share");
    }

    // ─── git ls-files argv ─────────────────────────────────────────

    #[test]
    fn ls_files_primary_shape() {
        assert_eq!(
            ls_files_args(false, &[]),
            vec![
                "-z",
                "-s",
                "--cached",
                "--others",
                "--exclude-standard",
                "--directory",
                "--no-empty-directory",
            ]
        );
    }

    // crux: ignored 패스는 --cached를 빼고 --ignored를 넣는다.
    // mutation: --cached를 유지하거나 --ignored를 빼면 FAIL.
    #[test]
    fn ls_files_ignored_drops_cached_adds_ignored() {
        let args = ls_files_args(true, &[]);
        assert!(
            !args.iter().any(|a| a == "--cached"),
            "ignored는 --cached 없음"
        );
        assert!(
            args.iter().any(|a| a == "--ignored"),
            "ignored는 --ignored 있음"
        );
        assert!(args.iter().any(|a| a == "--others"));
        assert_eq!(
            args,
            vec![
                "-z",
                "-s",
                "--others",
                "--ignored",
                "--exclude-standard",
                "--directory",
                "--no-empty-directory",
            ]
        );
    }

    // crux(injection): git exclude prefix는 `-- .` 뒤 `:(exclude,glob)`로 감싸여 플래그가 못 됨.
    // mutation: `:(exclude,glob)` 접두를 떼면 `-foo`가 플래그로 새 이 assert가 FAIL.
    #[test]
    fn ls_files_exclude_is_pathspec_wrapped() {
        let args = ls_files_args(false, &["-foo".to_string()]);
        // exclude가 있으면 `-- .` 뒤에 directory-form + contents-form pathspec이 온다.
        assert_eq!(
            &args[args.len() - 4..],
            &[
                "--".to_string(),
                ".".to_string(),
                ":(exclude,glob)-foo".to_string(),
                ":(exclude,glob)-foo/**".to_string(),
            ][..]
        );
        // raw `-foo`는 argv에 없다(플래그로 샐 수 없음).
        assert!(!args.iter().any(|a| a == "-foo"));
    }

    #[test]
    fn ls_files_no_exclude_has_no_pathspec_separator() {
        let args = ls_files_args(false, &[]);
        assert!(!args.iter().any(|a| a == "--"));
    }

    // ─── `-s` 스테이지 파서: {40,64} SHA-1 AND SHA-256 (Codex fix 2) ─

    // crux: SHA-1(40 hex) 스테이지 레코드가 tab 뒤 경로로 파싱된다.
    #[test]
    fn stage_parses_sha1_40_hex() {
        let rec = format!("100644 {} 0\tsrc/main.rs", "a".repeat(40));
        assert_eq!(parse_ls_files_stage_path(&rec), Some("src/main.rs"));
    }

    // crux: SHA-256(64 hex) 스테이지 레코드도 파싱된다. mutation: `{40,64}`를 `== 40`/`{40}`으로
    // 굳히면 이 64-hex 케이스가 매칭 실패해 None → FAIL.
    #[test]
    fn stage_parses_sha256_64_hex() {
        let rec = format!("100644 {} 0\tsrc/lib.rs", "b".repeat(64));
        assert_eq!(parse_ls_files_stage_path(&rec), Some("src/lib.rs"));
        // full entry로도 gitlink=false, untracked=false.
        let e = parse_ls_files_entry(&rec);
        assert_eq!(e.path, "src/lib.rs");
        assert!(!e.is_gitlink);
        assert!(!e.is_untracked_dir);
    }

    // 하한/상한 경계: 39 hex는 실패, 65 hex도 실패.
    #[test]
    fn stage_rejects_out_of_range_hex_width() {
        let short = format!("100644 {} 0\tf", "a".repeat(39));
        let long = format!("100644 {} 0\tf", "a".repeat(65));
        assert_eq!(parse_ls_files_stage_path(&short), None);
        assert_eq!(parse_ls_files_stage_path(&long), None);
    }

    // crux: gitlink(mode 160000) 스테이지 엔트리 → is_gitlink=true.
    // mutation: mode 비교를 다른 값으로 바꾸면 FAIL.
    #[test]
    fn stage_gitlink_mode_160000() {
        let rec = format!("160000 {} 0\tvendor/sub", "c".repeat(40));
        let e = parse_ls_files_entry(&rec);
        assert!(e.is_gitlink);
        assert_eq!(e.path, "vendor/sub");
        assert!(!e.is_untracked_dir);
    }

    // crux: `--others` untracked 라인은 스테이지 접두가 없다 → None(파서가 stage로 오인 안 함).
    #[test]
    fn untracked_line_is_not_stage() {
        assert_eq!(parse_ls_files_stage_path("README.md"), None);
        let e = parse_ls_files_entry("README.md");
        assert_eq!(e.path, "README.md");
        assert!(!e.is_gitlink);
        assert!(!e.is_untracked_dir);
    }

    // crux: untracked **디렉터리** placeholder(끝-`/`) → is_untracked_dir=true.
    // mutation: `ends_with('/')`를 false로 굳히면 FAIL.
    #[test]
    fn untracked_dir_placeholder_trailing_slash() {
        let e = parse_ls_files_entry("build/");
        assert!(e.is_untracked_dir);
        assert!(!e.is_gitlink);
        assert_eq!(e.path, "build/");
    }

    // hash 자리에 non-hex(대문자 A/`g`)가 오면 스테이지가 아니다 → untracked 취급.
    // mutation: hex 검사(is_lower_hex)를 완화(예: 대문자 허용/`b'a'..=b'g'`)하면 이 케이스가
    // stage로 오인돼 Some을 내 FAIL.
    #[test]
    fn stage_rejects_non_hex_hash() {
        // 40자 중 대문자 → hex 아님. 접두가 안 맞아 stage None.
        assert_eq!(
            parse_ls_files_stage_path(&format!("100644 {} 0\tf", "A".repeat(40))),
            None
        );
        // `g`는 [0-9a-f] 밖.
        assert_eq!(
            parse_ls_files_stage_path(&format!("100644 {} 0\tf", "g".repeat(40))),
            None
        );
    }

    // F1: octal mode `[0-7]` 상한 pin. mode에 8/9가 오면 stage 아님.
    // mutation: mode 검사 `[0-7]`→`[0-9]`로 완화하면 이 케이스가 stage로 파싱돼 FAIL.
    #[test]
    fn stage_rejects_non_octal_mode() {
        // 첫 자리 8(octal 밖).
        let rec = format!("800644 {} 0\tf", "a".repeat(40));
        assert_eq!(parse_ls_files_stage_path(&rec), None);
        // 끝자리 9.
        let rec9 = format!("100649 {} 0\tf", "a".repeat(40));
        assert_eq!(parse_ls_files_stage_path(&rec9), None);
    }

    // F1: stage 자리 `[0-3]` 상한 pin. stage 4는 유효한 stage가 아니다.
    // mutation: stage 검사 `[0-3]`→`[0-9]`로 완화하면 이 케이스가 stage로 파싱돼 FAIL.
    #[test]
    fn stage_rejects_out_of_range_stage_digit() {
        let rec = format!("100644 {} 4\tf", "a".repeat(40));
        assert_eq!(parse_ls_files_stage_path(&rec), None);
        // 실제 유효 stage 0..=3은 파싱된다(대조군).
        for s in ['0', '1', '2', '3'] {
            let ok = format!("100644 {} {s}\tf", "a".repeat(40));
            assert_eq!(parse_ls_files_stage_path(&ok), Some("f"), "stage {s}");
        }
    }

    // `-z` 스트림 분리: NUL split + 빈 조각 skip(status.rs 규율 재사용).
    #[test]
    fn stream_splits_nul_and_skips_empty() {
        let sha = "d".repeat(40);
        let stream = format!("100644 {sha} 0\ta.rs\0untracked.txt\0build/\0");
        let entries = parse_ls_files_stream(&stream);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].path, "a.rs");
        assert!(!entries[0].is_untracked_dir);
        assert_eq!(entries[1].path, "untracked.txt");
        assert!(!entries[1].is_untracked_dir);
        assert_eq!(entries[2].path, "build/");
        assert!(entries[2].is_untracked_dir);
    }

    // ─── classify 4분기 (mutation: 한 분기 뒤집기) ──────────────────

    // crux: 5개 probe 입력 각각 정확한 action. 예컨대 DirWithGit→FillNestedRepo가 crux —
    // 이걸 Drop으로 바꾸는 mutation은 이 표에서 즉시 FAIL.
    #[test]
    fn classify_four_branch_table() {
        assert_eq!(
            classify_quick_open_git_entry(GitEntryProbe::OrdinaryFile),
            GitEntryAction::Keep
        );
        assert_eq!(
            classify_quick_open_git_entry(GitEntryProbe::LstatFailed),
            GitEntryAction::DropPlaceholder
        );
        assert_eq!(
            classify_quick_open_git_entry(GitEntryProbe::NotADir),
            GitEntryAction::DropPlaceholder
        );
        assert_eq!(
            classify_quick_open_git_entry(GitEntryProbe::DirWithGit),
            GitEntryAction::FillNestedRepo
        );
        assert_eq!(
            classify_quick_open_git_entry(GitEntryProbe::DirWithoutGit),
            GitEntryAction::DropPlaceholder
        );
    }

    // DirWithGit만 FillNestedRepo다 — 다른 어떤 probe도 FillNestedRepo가 아님(분기 혼선 방지).
    #[test]
    fn classify_only_dir_with_git_fills() {
        for probe in [
            GitEntryProbe::OrdinaryFile,
            GitEntryProbe::LstatFailed,
            GitEntryProbe::NotADir,
            GitEntryProbe::DirWithoutGit,
        ] {
            assert_ne!(
                classify_quick_open_git_entry(probe),
                GitEntryAction::FillNestedRepo
            );
        }
        // OrdinaryFile만 Keep.
        for probe in [
            GitEntryProbe::LstatFailed,
            GitEntryProbe::NotADir,
            GitEntryProbe::DirWithGit,
            GitEntryProbe::DirWithoutGit,
        ] {
            assert_ne!(classify_quick_open_git_entry(probe), GitEntryAction::Keep);
        }
    }

    // ─── excludePaths 정규화 (mutation: 한 drop 규칙 뒤집기) ─────────

    #[test]
    fn normalize_valid_nested_rel() {
        assert_eq!(
            normalize_exclude_path("packages/app"),
            Some("packages/app".to_string())
        );
    }

    // 끝 `/` 제거.
    #[test]
    fn normalize_strips_trailing_slash() {
        assert_eq!(
            normalize_exclude_path("packages/app/"),
            Some("packages/app".to_string())
        );
    }

    // 백슬래시 → `/`.
    #[test]
    fn normalize_backslash_to_slash() {
        assert_eq!(
            normalize_exclude_path("packages\\app"),
            Some("packages/app".to_string())
        );
    }

    // crux: 각 malformed/outside-root/root-equal은 drop(None).
    // mutation: 어느 한 drop 조건을 지우면 그 케이스가 Some을 내 FAIL.
    #[test]
    fn normalize_drops_malformed_and_outside_root() {
        assert_eq!(normalize_exclude_path(""), None, "empty");
        assert_eq!(normalize_exclude_path("."), None, "root-equal '.'");
        assert_eq!(normalize_exclude_path("/etc"), None, "absolute");
        assert_eq!(normalize_exclude_path(".."), None, "parent");
        assert_eq!(normalize_exclude_path("../x"), None, "parent-prefix");
        assert_eq!(normalize_exclude_path("./"), None, "'.' after trim");
    }

    // `..name`은 유효한 child — parent가 아니다(경계 오분류 방지).
    #[test]
    fn normalize_dotdot_name_is_valid_child() {
        assert_eq!(
            normalize_exclude_path("..hidden"),
            Some("..hidden".to_string())
        );
    }

    // F4: 어떤 위치의 `..` 세그먼트든 거부(leading이든 mid-path든) — root 밖 탈출 원천 차단.
    // mutation: `..` 세그먼트 거부 가드(`split('/').any(seg == "..")`)를 제거하면
    // `a/../../x`·`a/..`·`x/../y`가 Some을 내 FAIL.
    #[test]
    fn normalize_rejects_dotdot_any_position() {
        assert_eq!(normalize_exclude_path("a/../../x"), None, "mid escape");
        assert_eq!(normalize_exclude_path("a/.."), None, "trailing ..");
        assert_eq!(normalize_exclude_path("../x"), None, "leading");
        assert_eq!(normalize_exclude_path("x/../y"), None, "mid ..");
        // 대조군: `..`가 없으면 정상 통과.
        assert_eq!(normalize_exclude_path("a/b/c"), Some("a/b/c".to_string()));
    }

    // ─── collapse: includeSymlinks OR 전파 (Codex-flagged crux) ─────

    // crux: descendant(true)가 ancestor(false)에 병합되면 ancestor가 true로 승격.
    // mutation: OR을 "ancestor 값 유지"로 바꾸면 ancestor가 false로 남아 FAIL.
    #[test]
    fn collapse_or_propagates_symlinks_upward() {
        let out = collapse_expansion_paths(vec![
            ExpansionPath::gitlink("a"),         // false
            ExpansionPath::untracked_dir("a/b"), // true, descendant
        ]);
        assert_eq!(out.len(), 1, "descendant는 ancestor에 병합");
        assert_eq!(out[0].rel, "a");
        assert!(
            out[0].include_symlinks,
            "descendant true → ancestor가 true로 승격돼야 한다"
        );
    }

    // 반대로 descendant(false)는 ancestor(true)를 다운그레이드하지 않는다(OR).
    #[test]
    fn collapse_does_not_downgrade_ancestor() {
        let out = collapse_expansion_paths(vec![
            ExpansionPath::untracked_dir("a"), // true
            ExpansionPath::gitlink("a/b"),     // false, descendant
        ]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rel, "a");
        assert!(out[0].include_symlinks, "ancestor true 유지");
    }

    // F3: 같은 rel이 no-ancestor 경로로 중복 삽입되면 flag를 OR한다(다운그레이드 금지).
    // untracked_dir("a")(true)와 gitlink("a")(false)가 같은 rel로 오면 a:true 유지.
    // mutation: `|=`를 `=`(overwrite)로 바꾸면 a:false가 돼 FAIL.
    #[test]
    fn collapse_or_on_duplicate_rel() {
        let out = collapse_expansion_paths(vec![
            ExpansionPath::untracked_dir("a"), // true
            ExpansionPath::gitlink("a"),       // false, 같은 rel
        ]);
        assert_eq!(out.len(), 1, "같은 rel은 하나로");
        assert_eq!(out[0].rel, "a");
        assert!(
            out[0].include_symlinks,
            "중복 rel은 OR — true가 false에 덮이면 안 된다"
        );
    }

    // 무관한 sibling은 병합 안 됨 — 둘 다 유지, flag 각각.
    #[test]
    fn collapse_keeps_unrelated_siblings() {
        let out = collapse_expansion_paths(vec![
            ExpansionPath::gitlink("a"),
            ExpansionPath::untracked_dir("b"),
        ]);
        assert_eq!(out.len(), 2);
        let a = out.iter().find(|e| e.rel == "a").unwrap();
        let b = out.iter().find(|e| e.rel == "b").unwrap();
        assert!(!a.include_symlinks);
        assert!(b.include_symlinks);
    }

    // 세그먼트 경계 ancestor: `a`는 `a-b/c`의 ancestor가 아니다(`a-b`가 아니라 `a` 조회는
    // `/` 경계에서만). `a/b/c`는 `a`에 collapse.
    #[test]
    fn collapse_ancestor_is_segment_boundary() {
        let out = collapse_expansion_paths(vec![
            ExpansionPath::gitlink("a"),
            ExpansionPath::untracked_dir("a-b"), // ancestor "a" 아님(다른 세그먼트)
            ExpansionPath::untracked_dir("a/b/c"), // ancestor "a"
        ]);
        // "a"와 "a-b"는 남고, "a/b/c"는 "a"로 병합.
        assert_eq!(out.len(), 2);
        let a = out.iter().find(|e| e.rel == "a").unwrap();
        assert!(a.include_symlinks, "a/b/c(true)가 a로 OR");
        assert!(out.iter().any(|e| e.rel == "a-b"));
        assert!(!out.iter().any(|e| e.rel == "a/b/c"));
    }
}

// ═══════════════════════════════════════════════════════════════════════
// M2b 드라이버 테스트 — 실제 git/rg를 tempdir에서, 제어 불가한 실패는 가짜 rg 스크립트로
// 주입한다. 각 crux는 하나의 mutant를 죽인다. 모든 파일 쓰기는 tempdir 안에서만.
// ═══════════════════════════════════════════════════════════════════════
#[cfg(all(test, unix))]
mod driver_tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::path::PathBuf;
    use std::process::Command as StdCommand;

    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    /// tempdir에 실행 가능한 가짜 `rg` 스크립트를 만든다.
    fn write_exec(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        let mut perms = std::fs::metadata(&p).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&p, perms).unwrap();
        p
    }

    /// tempdir에서 git 서브커맨드를 돌린다(테스트 픽스처 구성용, async runner와 무관).
    fn git(wt: &Path, args: &[&str]) {
        let status = StdCommand::new("git")
            .args(args)
            .current_dir(wt)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} 실패");
    }

    // ─── Tier 3 walk: cap THROWS, 이중 체크포인트 (Codex fix 6) ─────────

    // THE crux(cap): cap을 넘기면 **truncate가 아니라 에러**. 작은 cap으로 10k 실파일 없이 검증.
    // mutation: consume을 "cap 도달 시 조용히 스킵 후 Ok" 로 바꾸면(=truncate) 이 assert가 FAIL.
    #[test]
    fn walk_cap_throws_not_truncates() {
        let wt = tempdir();
        for i in 0..5 {
            std::fs::write(wt.path().join(format!("f{i}.txt")), b"x").unwrap();
        }
        // cap=3, 데드라인은 넉넉 → 4번째 파일에서 cap 에러.
        let budget = WalkBudget::with_limits(3, Instant::now() + Duration::from_secs(30));
        let r = list_with_walk(wt.path(), budget);
        assert!(
            matches!(r, Err(QuickOpenError::WalkCapExceeded { max: 3 })),
            "cap 초과가 truncate-Ok가 아니라 에러여야 한다: {r:?}"
        );
    }

    // THE crux(이중 체크포인트): 파일이 **하나도 없는** 빈-디렉터리 트리 + 지난 데드라인 →
    // 여전히 WalkTimeout. consume(파일별)에서만 데드라인을 봤다면 파일이 없어 절대 안 걸리고
    // Ok([])를 반환한다 → 이 assert가 죽인다. mutation: 배치 체크포인트 제거 → Ok([]) → FAIL.
    #[test]
    fn walk_empty_dirs_still_checkpoint_deadline() {
        let wt = tempdir();
        std::fs::create_dir_all(wt.path().join("a/b")).unwrap();
        std::fs::create_dir(wt.path().join("c")).unwrap();
        // 지난 데드라인 → 파일이 없어도 배치 체크포인트가 잡아야 한다.
        let past = WalkBudget::with_limits(10_000, Instant::now() - Duration::from_secs(1));
        let r = list_with_walk(wt.path(), past);
        assert!(
            matches!(r, Err(QuickOpenError::WalkTimeout)),
            "빈 디렉터리 트리 + 지난 데드라인은 무성 Ok([])가 아니라 WalkTimeout: {r:?}"
        );
        // 대조군: 미래 데드라인 → 파일 없음 → Ok([]).
        let future = WalkBudget::with_limits(10_000, Instant::now() + Duration::from_secs(30));
        assert_eq!(
            list_with_walk(wt.path(), future).unwrap(),
            Vec::<String>::new()
        );
    }

    // crux(include_symlinks): 심링크 leaf는 flag가 true일 때만 결과에 든다.
    // mutation: `is_symlink && !include_symlinks` 스킵을 지우면 false일 때도 link가 나와 FAIL.
    #[test]
    fn walk_includes_symlink_leaf_only_when_flag_set() {
        let wt = tempdir();
        std::fs::write(wt.path().join("real.txt"), b"x").unwrap();
        symlink("real.txt", wt.path().join("link")).unwrap();

        let mut b1 = WalkBudget::new();
        let mut out1 = BTreeSet::new();
        walk(wt.path(), "", false, &mut b1, &mut out1).unwrap();
        assert!(out1.contains("real.txt"));
        assert!(
            !out1.contains("link"),
            "flag=false인데 심링크 leaf가 나왔다"
        );

        let mut b2 = WalkBudget::new();
        let mut out2 = BTreeSet::new();
        walk(wt.path(), "", true, &mut b2, &mut out2).unwrap();
        assert!(
            out2.contains("link"),
            "flag=true인데 심링크 leaf가 누락됐다"
        );
    }

    // walk는 심링크 **디렉터리**로 traverse하지 않는다(list_dir가 refuse) — 밖 내용 유출 방지.
    #[test]
    fn walk_never_traverses_symlink_dir() {
        let wt = tempdir();
        let outside = tempdir();
        std::fs::create_dir(outside.path().join("secret")).unwrap();
        std::fs::write(outside.path().join("secret/leak.txt"), b"x").unwrap();
        symlink(outside.path().join("secret"), wt.path().join("slink")).unwrap();

        let mut b = WalkBudget::new();
        let mut out = BTreeSet::new();
        walk(wt.path(), "", true, &mut b, &mut out).unwrap();
        // slink는 심링크 leaf로 나올 순 있어도, 그 안의 leak.txt는 절대 나오면 안 된다.
        assert!(
            !out.iter().any(|p| p.contains("leak.txt")),
            "심링크 dir로 traverse했다"
        );
    }

    // ─── Tier 2 git: rev-parse soft-fail → walk (Codex fix 5) ──────────

    // crux: 비-git 디렉터리 → rev-parse 실패 → **walk로 soft-fail**(에러 아님).
    // mutation: "rev-parse 실패 → error" 로 바꾸면 Err → 이 assert(Ok+파일)가 FAIL.
    #[tokio::test]
    async fn non_git_dir_soft_fails_to_walk() {
        let wt = tempdir();
        std::fs::write(wt.path().join("a.txt"), b"x").unwrap();
        std::fs::write(wt.path().join("b.txt"), b"x").unwrap();
        let files = list_with_git(wt.path(), &[]).await.unwrap();
        assert!(files.contains(&"a.txt".to_string()));
        assert!(files.contains(&"b.txt".to_string()));
    }

    // ─── Tier 2 git: ls-files 확정-워크트리 실패 → 하드 리젝트 (Codex fix 5) ─

    // THE crux(하드 리젝트): 확정 워크트리에서 primary ls-files가 실패하면 **walk로 캐스케이드
    // 하지 않고 하드 에러**. 실제 git으로: index를 손상시키면 rev-parse는 여전히 exit 0("true")
    // 이지만 ls-files는 index를 읽다 exit 128로 죽는다 — 비대칭을 실물로 재현.
    // mutation: 실패 시 walk로 캐스케이드하게 바꾸면 walk가 repo를 나열해 Ok → 이 assert가 FAIL.
    #[tokio::test]
    async fn git_ls_files_failure_hard_rejects_no_walk() {
        let wt = tempdir();
        git(wt.path(), &["init", "-q"]);
        std::fs::write(wt.path().join("tracked.txt"), b"x").unwrap();
        git(wt.path(), &["add", "tracked.txt"]);
        // index 손상: rev-parse는 통과, ls-files는 실패.
        std::fs::write(wt.path().join(".git/index"), b"garbage-not-an-index").unwrap();

        let r = list_with_git(wt.path(), &[]).await;
        assert!(
            matches!(r, Err(QuickOpenError::GitLsFilesFailed { .. })),
            "ls-files 실패가 walk 캐스케이드나 Ok가 아니라 하드 리젝트여야 한다: {r:?}"
        );
    }

    // ─── Tier 2 git: happy path + untracked-dir 확장 + 심링크 전파 ─────

    // real happy path: tracked + untracked 파일이 dedup/forward-slash로 나온다.
    #[tokio::test]
    async fn git_tier_real_happy_path() {
        let wt = tempdir();
        git(wt.path(), &["init", "-q"]);
        std::fs::create_dir(wt.path().join("src")).unwrap();
        std::fs::write(wt.path().join("src/main.rs"), b"x").unwrap();
        git(wt.path(), &["add", "src/main.rs"]);
        std::fs::write(wt.path().join("README.md"), b"x").unwrap();

        let files = list_with_git(wt.path(), &[]).await.unwrap();
        assert!(
            files.contains(&"src/main.rs".to_string()),
            "tracked 누락: {files:?}"
        );
        assert!(
            files.contains(&"README.md".to_string()),
            "untracked 누락: {files:?}"
        );
        // 정렬·dedup(BTreeSet) 확인.
        let mut sorted = files.clone();
        sorted.sort();
        assert_eq!(files, sorted, "결과가 정렬돼 있어야 한다");
    }

    // crux(untracked-dir 확장 + include_symlinks 전파): 완전-untracked 디렉터리는 git이 `stuff/`
    // 로 collapse한다 → 확장 walk가 안의 파일 **그리고 심링크**를 surface(untracked-dir는
    // include_symlinks=true). mutation: untracked-dir를 gitlink처럼 false로 확장하면 link 누락 FAIL.
    #[tokio::test]
    async fn git_untracked_dir_expands_and_surfaces_symlink() {
        let wt = tempdir();
        git(wt.path(), &["init", "-q"]);
        std::fs::create_dir(wt.path().join("stuff")).unwrap();
        std::fs::write(wt.path().join("stuff/inner.txt"), b"x").unwrap();
        symlink("inner.txt", wt.path().join("stuff/link")).unwrap();

        let files = list_with_git(wt.path(), &[]).await.unwrap();
        assert!(
            files.contains(&"stuff/inner.txt".to_string()),
            "untracked-dir 확장이 내부 파일을 안 냈다: {files:?}"
        );
        assert!(
            files.contains(&"stuff/link".to_string()),
            "untracked-dir 확장이 심링크 leaf를 누락(include_symlinks 전파 실패): {files:?}"
        );
    }

    // ─── classify 라우팅: gitlink→false, untracked-dir→true (M2b 추가 배선) ─

    // crux: classify_entry가 gitlink을 include_symlinks=false로, untracked-dir를 true로 확장 경로에
    // 넣는다. mutation: 두 flag를 뒤집으면 이 assert가 FAIL.
    #[test]
    fn classify_entry_routes_flags() {
        let wt = tempdir();
        // .git을 품은 디렉터리 → gitlink 확장 대상(DirWithGit → FillNestedRepo).
        std::fs::create_dir(wt.path().join("sub")).unwrap();
        std::fs::write(wt.path().join("sub/.git"), b"gitdir: ...").unwrap();

        let mut files = BTreeSet::new();
        let mut exps = Vec::new();
        classify_entry(
            wt.path(),
            GitLsFilesEntry {
                path: "sub",
                is_gitlink: true,
                is_untracked_dir: false,
            },
            &mut files,
            &mut exps,
        );
        assert_eq!(exps, vec![ExpansionPath::gitlink("sub")]);
        assert!(
            !exps[0].include_symlinks,
            "gitlink 확장은 include_symlinks=false"
        );

        let mut exps2 = Vec::new();
        classify_entry(
            wt.path(),
            GitLsFilesEntry {
                path: "things/",
                is_gitlink: false,
                is_untracked_dir: true,
            },
            &mut files,
            &mut exps2,
        );
        assert_eq!(exps2, vec![ExpansionPath::untracked_dir("things")]);
        assert!(
            exps2[0].include_symlinks,
            "untracked-dir 확장은 include_symlinks=true"
        );
    }

    // probe_dir 4-상태(mutation: is_dir/hasGitEntry 검사 뒤집기).
    #[test]
    fn probe_dir_four_states() {
        let wt = tempdir();
        std::fs::create_dir(wt.path().join("withgit")).unwrap();
        std::fs::write(wt.path().join("withgit/.git"), b"x").unwrap();
        std::fs::create_dir(wt.path().join("plain")).unwrap();
        std::fs::write(wt.path().join("afile"), b"x").unwrap();

        assert_eq!(probe_dir(wt.path(), "withgit"), GitEntryProbe::DirWithGit);
        assert_eq!(probe_dir(wt.path(), "plain"), GitEntryProbe::DirWithoutGit);
        assert_eq!(probe_dir(wt.path(), "afile"), GitEntryProbe::NotADir);
        assert_eq!(probe_dir(wt.path(), "gone"), GitEntryProbe::LstatFailed);
    }

    // ─── Tier 1 rg: 종료 시나리오 (가짜 rg 주입) ───────────────────────

    // rg exit 0 → 파싱 경로 resolve(`./` 접두 제거, dedup, 정렬).
    #[tokio::test]
    async fn rg_exit0_resolves_paths() {
        let wt = tempdir();
        let bin = tempdir();
        let rg = write_exec(bin.path(), "rg", "#!/bin/sh\nprintf './b.rs\\n./a.rs\\n'\n");
        let files =
            list_with_rg_using(wt.path(), rg.to_str().unwrap(), &[], Duration::from_secs(5))
                .await
                .unwrap();
        assert_eq!(files, vec!["a.rs".to_string(), "b.rs".to_string()]);
    }

    // THE cardinal-sin(part 1): exit 2 & 0경로 → **하드 에러**(부분/빈 목록 아님).
    // mutation: exit2-empty에서 버퍼(빈)를 Ok로 반환하면 Ok([]) → 이 assert가 FAIL.
    #[tokio::test]
    async fn rg_exit2_empty_hard_errors() {
        let wt = tempdir();
        let bin = tempdir();
        let rg = write_exec(bin.path(), "rg", "#!/bin/sh\nexit 2\n");
        let r =
            list_with_rg_using(wt.path(), rg.to_str().unwrap(), &[], Duration::from_secs(5)).await;
        assert!(
            matches!(r, Err(QuickOpenError::RgFailed { .. })),
            "exit2-무결과는 하드 에러여야 한다: {r:?}"
        );
    }

    // exit 2 & ≥1경로 → resolve(권한 없는 하위 디렉터리 부분 성공).
    // mutation: "exit2는 항상 에러"로 굳히면 Err → 이 assert가 FAIL.
    #[tokio::test]
    async fn rg_exit2_with_paths_resolves() {
        let wt = tempdir();
        let bin = tempdir();
        let rg = write_exec(bin.path(), "rg", "#!/bin/sh\nprintf './x\\n'\nexit 2\n");
        let files =
            list_with_rg_using(wt.path(), rg.to_str().unwrap(), &[], Duration::from_secs(5))
                .await
                .unwrap();
        assert_eq!(files, vec!["x".to_string()]);
    }

    // THE cardinal-sin(part 2): 타임아웃 → **버퍼 폐기 + 하드 에러**(부분 목록 절대 아님) +
    // git 폴백 없음. 가짜 rg가 한 줄 내고 sleep 30 → 짧은 타임아웃으로 킬.
    // mutation: 타임아웃 시 스트림된 접두를 Ok로 반환하면 Ok(["partial"]) → 이 assert가 FAIL.
    #[tokio::test]
    async fn rg_timeout_hard_errors_not_partial() {
        let wt = tempdir();
        let bin = tempdir();
        let rg = write_exec(
            bin.path(),
            "rg",
            "#!/bin/sh\nprintf './partial\\n'\nsleep 30\n",
        );
        let start = Instant::now();
        let r = list_with_rg_using(
            wt.path(),
            rg.to_str().unwrap(),
            &[],
            Duration::from_millis(300),
        )
        .await;
        assert!(
            matches!(r, Err(QuickOpenError::RgFailed { .. })),
            "타임아웃은 부분 목록이 아니라 하드 에러: {r:?}"
        );
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "타임아웃이 sleep 30을 그대로 기다렸다(킬 실패)"
        );
    }

    // ─── 캐스케이드 dispatch: rg 없음 → git (Codex fix 5) ──────────────

    // crux: rg 미가용(upfront) → **git 캐스케이드**가 파일을 낸다.
    // mutation: "rg 없음 → error"(else 분기를 Err로)로 바꾸면 Err → 이 assert가 FAIL.
    #[tokio::test]
    async fn rg_missing_cascades_to_git() {
        let wt = tempdir();
        git(wt.path(), &["init", "-q"]);
        std::fs::write(wt.path().join("f.txt"), b"x").unwrap();
        git(wt.path(), &["add", "f.txt"]);

        let files = dispatch(wt.path(), &[], false).await.unwrap();
        assert!(
            files.contains(&"f.txt".to_string()),
            "rg 없음이 git 캐스케이드로 이어지지 않았다: {files:?}"
        );
    }

    // rg 있음(upfront) → rg 티어 사용(git 아님). 실제 rg가 있을 때만; 없으면 스킵.
    #[tokio::test]
    async fn rg_present_uses_rg_tier() {
        let wt = tempdir();
        // rg가 없는 환경이면 이 테스트는 의미 없음(가짜 rg 테스트가 rg 경로를 이미 커버).
        if !rg_available(wt.path()).await {
            return;
        }
        std::fs::write(wt.path().join("hello.txt"), b"x").unwrap();
        // rg는 non-git 디렉터리도 나열한다(git 티어라면 walk로 감). 결과에 파일이 있으면 충분.
        let files = dispatch(wt.path(), &[], true).await.unwrap();
        assert!(
            files.contains(&"hello.txt".to_string()),
            "rg 티어 결과 누락: {files:?}"
        );
    }

    // ─── 타임아웃 상수: 10s(기본 30s 아님) (Codex fix 4) ───────────────

    // crux: git 호출은 10s 타임아웃을 쓴다(GitRunner 기본 30s가 아니라).
    // mutation: GIT_TIMEOUT을 30s로 바꾸면 이 assert가 FAIL.
    #[test]
    fn git_timeout_is_ten_seconds_not_default_thirty() {
        assert_eq!(GIT_TIMEOUT, Duration::from_secs(10));
        assert_ne!(GIT_TIMEOUT, crate::runner::DEFAULT_TIMEOUT);
        assert_eq!(RG_TIMEOUT, Duration::from_secs(10));
        assert_eq!(RG_PROBE_TIMEOUT, Duration::from_secs(5));
    }

    // ─── rg stdout 파싱 ────────────────────────────────────────────────

    #[test]
    fn parse_rg_stdout_normalizes() {
        let out = parse_rg_stdout(b"./src/main.rs\r\n./a.rs\n.\n\n/abs\n../esc\nsrc/lib.rs\n");
        // `./` 제거, CRLF의 `\r` 제거, `.`/빈/절대/`..` 스킵.
        assert_eq!(
            out,
            vec![
                "src/main.rs".to_string(),
                "a.rs".to_string(),
                "src/lib.rs".to_string(),
            ]
        );
    }
}
