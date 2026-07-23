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

/// `..` 또는 `../…`(root 밖으로 탈출)인가. `..name`은 유효한 child라 걸리지 않는다
/// (Orca `isParentRelativePath`, quick-open-filter.ts:173-176).
fn is_parent_relative(rel: &str) -> bool {
    rel == ".." || rel.starts_with("../")
}

/// worktree-상대 exclude 후보 하나를 `/`-구분 root-relative prefix로 정규화한다.
/// malformed / root 밖 / root-equal(`""`·`.`)은 **조용히 drop**(`None`) — stale하거나 오타난
/// exclude 경로가 요청 전체를 실패시키지 못하게(Orca `buildExcludePathPrefixes`의 per-entry
/// 규칙, quick-open-filter.ts:107-131).
///
/// 규칙(순서):
/// 1. 백슬래시 → `/`.
/// 2. absolute(`/…`) 또는 parent(`..`/`../…`) → `None`(root 밖, Orca:122).
/// 3. 끝 `/` 제거(경계 검사 명확화, Orca:126). 남은 게 root-equal(`""`/`.`)이면 `None`
///    (전체 트리를 exclude 거부, Orca:114-117·127). trim 뒤 한 번만 검사하면 `""`·`.`·
///    `./`·`packages/app/`가 전부 이 한 관문을 지난다 — trim 전 별도 검사는 redundant.
pub fn normalize_exclude_path(worktree_rel: &str) -> Option<String> {
    let fwd = worktree_rel.replace('\\', "/");
    if fwd.starts_with('/') || is_parent_relative(&fwd) {
        return None;
    }
    let trimmed = fwd.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "." {
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

        // ancestor 없음: 삽입(같은 key면 Orca Map.set처럼 덮어쓴다).
        if let Some(existing) = collapsed.iter_mut().find(|c| c.rel == ep.rel) {
            existing.include_symlinks = ep.include_symlinks;
        } else {
            collapsed.push(ep);
        }
    }

    collapsed
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

    // hash 자리에 non-hex(대문자 A)가 오면 스테이지가 아니다 → untracked 취급.
    #[test]
    fn stage_rejects_non_hex_hash() {
        // 40자 중 대문자 → hex 아님. 접두가 안 맞아 stage None.
        let rec = format!("100644 {} 0\tf", "A".repeat(40));
        assert_eq!(parse_ls_files_stage_path(&rec), None);
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
