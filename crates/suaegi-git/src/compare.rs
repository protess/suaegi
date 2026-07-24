use crate::refname::validate_user_ref;
use crate::runner::{GitError, GitRunner};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// 바이너리 판정에 볼 앞부분 바이트 수. git 자신이 쓰는 값과 같다.
pub const BINARY_SNIFF_BYTES: usize = 8192;

/// suaegi가 훅 주입을 위해 worktree에 써 넣는 파일. **우리가 만든 것이지 사용자의
/// 변경이 아니므로** untracked 수집에서 뺀다 — 안 그러면 우리 파일이 우리 diff
/// 패널에 뜬다.
///
/// **`.git/info/exclude`로 숨기지 않는다.** 실측(git 2.50.1): worktree에서 본
/// `--git-common-dir`은 **주 저장소의 `.git`**이라, 거기 규칙을 쓰면 사용자의
/// 주 체크아웃에서도 그 경로가 무시되고(직접 확인: 주 체크아웃의
/// `status -uall`에서 사라진다) `git worktree remove` 뒤에도 규칙이 남는다.
/// "사용자 저장소를 오염시키지 않는다"는 규칙을 정확히 위반한다.
///
/// tracked 변경은 거르지 않는다 — 사용자가 이 파일을 커밋했다면 그건 사용자 것이다.
const INJECTED_SETTINGS_PATH: &str = ".claude/settings.local.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeStatus {
    Added,
    Modified,
    Deleted,
    Renamed {
        from: String,
    },
    /// `-C`(복사 감지)가 내는 상태. `--name-status -z`에서 **`R`과 같은 두 경로짜리
    /// 레코드**라 한 경로만 소비하면 이후 레코드가 전부 밀린다.
    Copied {
        from: String,
    },
    Other(char),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: String,
    pub status: ChangeStatus,
    pub additions: Option<u32>,
    pub deletions: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchCompare {
    pub merge_base: String,
    pub ahead_count: u32,
    pub files: Vec<ChangedFile>,
}

/// 비교 실패를 **분류한** 결과. 지금까지 셋이 전부 `GitError::Failed` 문자열로
/// 뭉개져 UI가 "설정 문제"와 "진짜 오류"를 구별할 수 없었다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompareOutcome {
    Ready(BranchCompare),
    NoMergeBase,
    UnbornHead,
    InvalidBase,
    /// `CompareHandle::stop_after_current_call` 뒤 남은 호출을 시작하지 않았다.
    /// **오류가 아니다** — 호출부는 배너 없이 조용히 버린다.
    Cancelled,
}

/// `file_diff`의 결과. patch `String` 하나로는 바이너리와 `Other(c)`를 담을 곳이
/// 없었다 — 그 둘을 빈 patch로 뭉개면 UI가 "변경 없음"으로 그린다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileDiff {
    Patch(String),
    /// 앞 `BINARY_SNIFF_BYTES`바이트에 NUL이 있었다.
    Binary,
    TooLarge {
        limit: usize,
    },
    /// `ChangeStatus::Other(c)` — 타입 변경·미병합 등. **추측하지 않는다.**
    NonRenderable(char),
}

/// 어느 쪽 바이트를 볼 것인가. 문자열 `rev` 하나로 두면 안 된다 —
/// `git show <rev>:<path>`는 **untracked 파일을 읽지 못한다.**
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileSource {
    WorkingTree,
    Revision(String),
}

/// **값싼 취소다.** 각 git 호출 *사이*에서만 확인해 남은 호출을 시작하지 않는다.
/// 이름 그대로 **실행 중인 호출은 최대 `DEFAULT_TIMEOUT`(30초) 더 돈다** —
/// 즉시 취소는 `GitRunner`가 완료·타임아웃·취소를 `select`로 경합시키고 세 경로가
/// 같은 kill-and-reap 루틴을 공유해야 가능하고, 그건 이 플랜 범위 밖이다.
#[derive(Debug, Clone, Default)]
pub struct CompareHandle {
    cancel: Arc<AtomicBool>,
}

impl CompareHandle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn stop_after_current_call(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn is_stopped(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

async fn merge_base(
    runner: &GitRunner,
    worktree_path: &Path,
    base_ref: &str,
) -> Result<String, GitError> {
    validate_user_ref(base_ref)?;
    Ok(runner
        .run(worktree_path, &["merge-base", "HEAD", base_ref])
        .await?
        .stdout
        .trim()
        .to_string())
}

/// numstat 카운트 파싱: "-"(바이너리)만 None. 그 외 비숫자는 손상 출력이므로 Parse.
fn parse_count(token: &str, args: &str) -> Result<Option<u32>, GitError> {
    if token == "-" {
        return Ok(None);
    }
    token.parse::<u32>().map(Some).map_err(|e| GitError::Parse {
        args: args.to_string(),
        detail: format!("bad numstat count {token:?}: {e}"),
    })
}

/// numstat 카운트 맵: 경로 -> (추가 줄 수, 삭제 줄 수). `-`(바이너리)는 각각 None.
pub(crate) type NumstatCounts = HashMap<String, (Option<u32>, Option<u32>)>;

/// `--numstat -z` 스트림을 `path -> (adds, dels)` 맵으로 판다. rename/copy는
/// `"adds\tdels\t"`(빈 경로) 뒤에 from, to가 각각 **별도 NUL 레코드**로 이어지는
/// 두-레코드 형태다 — 한 레코드만 소비하면 이후 전부 밀린다.
///
/// `branch_compare`(working-tree diff)와 `commit_show`(commit..commit / `diff-tree
/// --root`)가 공유한다: `-z` numstat 출력 형태가 세 경우 모두 동일하다.
pub(crate) fn parse_numstat_z(stdout: &str) -> Result<NumstatCounts, GitError> {
    let args = "diff --numstat -z";
    let mut counts: NumstatCounts = HashMap::new();
    let mut records = stdout.split('\0');
    while let Some(record) = records.next() {
        if record.is_empty() {
            continue;
        }
        // splitn(3): 파일명에 탭이 있어도 세 번째 조각(경로)이 절단되지 않게
        let mut parts = record.splitn(3, '\t');
        let (Some(a), Some(d)) = (parts.next(), parts.next()) else {
            return Err(GitError::Parse {
                args: args.to_string(),
                detail: format!("truncated record {record:?}"),
            });
        };
        let adds = parse_count(a, args)?;
        let dels = parse_count(d, args)?;
        match parts.next() {
            Some(path) if !path.is_empty() => {
                counts.insert(path.to_string(), (adds, dels));
            }
            _ => {
                // rename/copy: from, to가 이어지는 별도 레코드
                let _from = records.next();
                let to = records.next().ok_or_else(|| GitError::Parse {
                    args: args.to_string(),
                    detail: "rename record missing target path".to_string(),
                })?;
                counts.insert(to.to_string(), (adds, dels));
            }
        }
    }
    Ok(counts)
}

/// `--name-status -z` 스트림을 `ChangedFile` 목록으로 판다. `R`/`C`는 **경로가 둘**인
/// 두-레코드 형태(하나만 소비하면 이후 레코드가 한 칸씩 밀린다). 카운트는 미리 판
/// numstat 맵(`counts`)에서 경로로 조인한다.
///
/// `branch_compare`와 `commit_show`가 공유한다: 세 diff 형태(working-tree,
/// commit..commit, `diff-tree --root`)의 `-z` name-status 출력이 동일하다.
pub(crate) fn parse_name_status_z(
    stdout: &str,
    counts: &NumstatCounts,
) -> Result<Vec<ChangedFile>, GitError> {
    let args = "diff --name-status -z";
    let mut files = Vec::new();
    let mut records = stdout.split('\0');
    while let Some(code) = records.next() {
        if code.is_empty() {
            continue;
        }
        let status_char = code.chars().next().unwrap_or('?');
        let (status, path) = match status_char {
            // R과 C는 **같은 모양**이다. C를 Other로 떨어뜨리면 경로를 하나만
            // 소비해 이후 모든 레코드가 한 칸씩 밀린다.
            'R' | 'C' => {
                let from = records.next().ok_or_else(|| GitError::Parse {
                    args: args.to_string(),
                    detail: format!("record {code:?} missing source path"),
                })?;
                let to = records.next().ok_or_else(|| GitError::Parse {
                    args: args.to_string(),
                    detail: format!("record {code:?} missing target path"),
                })?;
                let from = from.to_string();
                let status = if status_char == 'R' {
                    ChangeStatus::Renamed { from }
                } else {
                    ChangeStatus::Copied { from }
                };
                (status, to.to_string())
            }
            c => {
                let path = records.next().ok_or_else(|| GitError::Parse {
                    args: args.to_string(),
                    detail: format!("record {code:?} missing path"),
                })?;
                let status = match c {
                    'A' => ChangeStatus::Added,
                    'M' => ChangeStatus::Modified,
                    'D' => ChangeStatus::Deleted,
                    other => ChangeStatus::Other(other),
                };
                (status, path.to_string())
            }
        };
        let (additions, deletions) = counts.get(&path).copied().unwrap_or((None, None));
        files.push(ChangedFile {
            path,
            status,
            additions,
            deletions,
        });
    }
    Ok(files)
}

/// `rev-parse --verify --quiet <rev>`가 그 리비전을 풀 수 있는가.
///
/// **`--quiet`가 분류의 핵심이다**(실측, git 2.50.1): 풀리지 않는 리비전은 exit 1에
/// 출력 없음, 저장소가 아닌 등 진짜 오류는 exit 128 + fatal. `--quiet` 없이는 둘 다
/// 128이라 "base ref 오타"와 "저장소가 깨졌다"를 가를 수 없다.
async fn rev_resolves(
    runner: &GitRunner,
    worktree_path: &Path,
    rev: &str,
) -> Result<bool, GitError> {
    let out = runner
        .run_expecting(
            worktree_path,
            &["rev-parse", "--verify", "--quiet", rev],
            &[1],
        )
        .await?;
    Ok(out.code == 0)
}

/// merge-base 대비 **working tree** diff. `<mb>..HEAD`가 아니라 `<mb>`를 단독
/// 인자로 주면 커밋된 변경과 미커밋 변경이 모두 잡힌다. untracked 파일은 diff에
/// 안 잡히므로 `status --porcelain -z`에서 별도 수집해 Added로 합류시킨다.
///
/// git을 **7번** 부른다(분류 프로브 2회 포함). 각각 30초 상한이라 최악 ~210초 —
/// 그래서 매 호출 앞에서 `cancel`을 본다.
pub async fn branch_compare(
    runner: &GitRunner,
    worktree_path: &Path,
    base_ref: &str,
    cancel: &CompareHandle,
) -> Result<CompareOutcome, GitError> {
    validate_user_ref(base_ref)?;

    // 분류 순서를 못 박는다. `merge-base`의 exit 1만으로는 셋을 가를 수 없다 —
    // base ref 오타든, HEAD가 unborn이든, 정말 공통 조상이 없든 전부 1이다.
    if cancel.is_stopped() {
        return Ok(CompareOutcome::Cancelled);
    }
    if !rev_resolves(runner, worktree_path, base_ref).await? {
        return Ok(CompareOutcome::InvalidBase);
    }
    if cancel.is_stopped() {
        return Ok(CompareOutcome::Cancelled);
    }
    if !rev_resolves(runner, worktree_path, "HEAD").await? {
        return Ok(CompareOutcome::UnbornHead);
    }

    if cancel.is_stopped() {
        return Ok(CompareOutcome::Cancelled);
    }
    let mb_out = runner
        .run_expecting(worktree_path, &["merge-base", "HEAD", base_ref], &[1])
        .await?;
    if mb_out.code != 0 {
        return Ok(CompareOutcome::NoMergeBase);
    }
    let mb = mb_out.stdout.trim().to_string();

    if cancel.is_stopped() {
        return Ok(CompareOutcome::Cancelled);
    }
    let ahead_args = format!("{mb}..HEAD");
    let ahead_out = runner
        .run(worktree_path, &["rev-list", "--count", &ahead_args])
        .await?;
    let ahead = ahead_out
        .stdout
        .trim()
        .parse::<u32>()
        .map_err(|e| GitError::Parse {
            args: format!("rev-list --count {ahead_args}"),
            detail: format!("{e}: {:?}", ahead_out.stdout),
        })?;

    // -z: 레코드가 NUL 구분이라 특수문자 경로 안전. -M: rename, -C: copy 감지.
    // `-c core.quotePath=false`는 **넣지 않는다** — 실측(git 2.50.1) 결과 `-z`만으로
    // 비ASCII 경로가 이스케이프 없이 날것으로 나온다(한글 파일명 확인).
    if cancel.is_stopped() {
        return Ok(CompareOutcome::Cancelled);
    }
    let name_status = runner
        .run(
            worktree_path,
            &["diff", "--name-status", "-z", "-M", "-C", &mb],
        )
        .await?;
    if cancel.is_stopped() {
        return Ok(CompareOutcome::Cancelled);
    }
    let numstat = runner
        .run(worktree_path, &["diff", "--numstat", "-z", "-M", "-C", &mb])
        .await?;

    // numstat -z 레코드: "adds\tdels\tpath" 또는 rename/copy 시 "adds\tdels\t" 뒤에
    // from, to가 각각 별도 NUL 레코드로 이어진다. name-status -z 레코드:
    // "X\0path\0", rename "R100\0from\0to\0", copy "C100\0from\0to\0". 두 파서는
    // commit_show와 공유하는 pub(crate) 함수로 추출돼 있다(M0).
    let counts = parse_numstat_z(&numstat.stdout)?;
    let mut files = parse_name_status_z(&name_status.stdout, &counts)?;

    // untracked 파일 수집: status --porcelain -z에서 "?? path" 레코드
    if cancel.is_stopped() {
        return Ok(CompareOutcome::Cancelled);
    }
    let status_out = runner
        .run(
            worktree_path,
            &["status", "--porcelain", "-z", "--untracked-files=all"],
        )
        .await?;
    for record in status_out.stdout.split('\0') {
        if let Some(path) = record.strip_prefix("?? ") {
            if path == INJECTED_SETTINGS_PATH {
                continue;
            }
            files.push(ChangedFile {
                path: path.to_string(),
                status: ChangeStatus::Added,
                additions: None,
                deletions: None,
            });
        }
    }

    Ok(CompareOutcome::Ready(BranchCompare {
        merge_base: mb,
        ahead_count: ahead,
        files,
    }))
}

/// worktree 안에서 파일 하나가 실제로 무엇인지.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Resolved {
    Regular(PathBuf),
    /// **따라가지 않는다.** git은 심볼릭 링크를 링크 *내용*으로 다룬다. delete/rename도
    /// 링크 자체를 대상으로 해야 하므로(타깃을 지우면 안 됨) 항상 un-follow로 돌려준다.
    Symlink(PathBuf),
}

/// 경로 해석 모드. 유일한 차이는 **leaf가 이미 존재해야 하는가**뿐이다 — 어떤
/// 모드도 leaf 심링크를 따라가지 않는다(suaegi는 단일 워크트리 containment,
/// leaf는 링크 내용/링크 자체가 대상이다).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResolveMode {
    /// 모든 컴포넌트가 이미 존재해야 한다. read(파일 내용)와 delete/rename의
    /// **source**(지울 링크 자체)용. Orca `preserveSymlink`(parent만 정규화, leaf는
    /// 보존)에 해당 — 우리 컴포넌트별 walk가 중간 심링크를 즉시 거부하므로 parent
    /// 정규화 없이 같은 보장을 준다(`filesystem-auth.ts:299-318`).
    ExistingOnly,
    /// leaf(그리고 뒤따르는 아직 없는 컴포넌트)가 없어도 된다. create/write/copy와
    /// rename의 **dest**용. 존재하는 접두는 walk가 심링크가 아님을 확인했으므로
    /// escape 불가; 없는 꼬리는 위 어휘 검사로 전부 `Normal`임이 보장된다
    /// (`resolveAuthorizedMissingPath` `filesystem-auth.ts:340-369`의 등가).
    AllowMissingLeaf,
}

/// worktree 밖을 읽거나 쓰지 않도록 상대 경로를 검증하며 내려간다.
///
/// **어휘적 포함만으로는 부족하다.** `a/b/c`가 어휘적으로 안에 있어도 `b`가
/// worktree 밖을 가리키는 심볼릭 링크면 `open`이 그걸 따라간다. 그리고 마지막
/// 컴포넌트에만 `symlink_metadata`를 부르는 것도 부족하다 — 그 호출 자체가
/// **중간 컴포넌트는 이미 따라간 뒤**다. 그래서 한 컴포넌트씩 내려가며 본다.
///
/// 검사 순서는 값싼 것 먼저다(Codex 4): null-byte·비어있음·비-`Normal` 컴포넌트는
/// syscall 없이 즉시 거부하고, 그걸 통과한 뒤에야 컴포넌트별 `symlink_metadata`를
/// 친다(`filesystem-auth.ts:293-338`).
pub(crate) fn resolve_in_worktree(
    worktree: &Path,
    path: &str,
    mode: ResolveMode,
) -> std::io::Result<Resolved> {
    let reject = |detail: &str| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{detail}: {path:?}"),
        )
    };
    // null 바이트는 어떤 fs syscall보다 먼저 명시적으로 거부한다 — 커널까지 가면
    // 경로가 잘려 다른 파일을 가리킬 수 있다(`filesystem-auth.ts:411,531`).
    if path.as_bytes().contains(&0) {
        return Err(reject("path contains a null byte"));
    }
    let rel = Path::new(path);
    if path.is_empty() {
        return Err(reject("empty path"));
    }
    // 절대 경로, `..`, `.`, 루트/드라이브 접두는 전부 거절. git이 내는 경로에는
    // 없지만 이 함수는 create/write 등 공개 표면의 진입점이다.
    if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err(reject("path escapes the worktree"));
    }

    let mut current = worktree.to_path_buf();
    let mut components = rel.components().peekable();
    while let Some(component) = components.next() {
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    if components.peek().is_some() {
                        return Err(reject("path traverses a symlink"));
                    }
                    // leaf 심링크: 따라가지 않고 링크 자체를 돌려준다(read든 delete든).
                    return Ok(Resolved::Symlink(current));
                }
                // 존재하는 non-symlink 디렉터리/파일: 계속 내려간다.
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::NotFound
                    && mode == ResolveMode::AllowMissingLeaf =>
            {
                // 이 컴포넌트부터 끝까지 아직 없다. 남은 컴포넌트를 붙여 미래 경로를
                // 만든다. **여기까지의 접두는 위 루프가 심링크가 아님을 확인**했으므로
                // 존재하는 조상을 통한 escape는 불가능하고, 없는 꼬리는 전부 `Normal`이다.
                for rest in components {
                    current.push(rest);
                }
                return Ok(Resolved::Regular(current));
            }
            // ExistingOnly의 ENOENT 포함 — read/delete는 leaf 존재를 요구한다.
            Err(e) => return Err(e),
        }
    }
    Ok(Resolved::Regular(current))
}

/// read(파일 내용) 경로 해석. 모든 컴포넌트가 존재해야 하고 leaf 심링크는 un-follow.
pub(crate) fn resolve_for_read(worktree: &Path, path: &str) -> std::io::Result<Resolved> {
    resolve_in_worktree(worktree, path, ResolveMode::ExistingOnly)
}

/// create/write/copy/rename-dest 경로 해석. leaf(와 없는 꼬리)가 아직 없어도 된다.
pub(crate) fn resolve_for_write(worktree: &Path, path: &str) -> std::io::Result<Resolved> {
    resolve_in_worktree(worktree, path, ResolveMode::AllowMissingLeaf)
}

/// delete/rename-source 경로 해석. leaf 링크 자체를 대상으로(타깃 미추적), 존재 요구.
pub(crate) fn resolve_preserve_symlink(worktree: &Path, path: &str) -> std::io::Result<Resolved> {
    resolve_in_worktree(worktree, path, ResolveMode::ExistingOnly)
}

/// 블로킹 부분만 뽑았다 — `spawn_blocking`에 넘기고, 그대로 단위 테스트한다.
fn read_head_from_disk(worktree: &Path, path: &str, cap: usize) -> std::io::Result<Vec<u8>> {
    match resolve_for_read(worktree, path)? {
        Resolved::Symlink(link) => {
            let mut bytes = std::fs::read_link(&link)?
                .into_os_string()
                .into_encoded_bytes();
            bytes.truncate(cap);
            Ok(bytes)
        }
        Resolved::Regular(file) => {
            let mut handle = std::fs::File::open(&file)?;
            let mut buf = vec![0u8; cap];
            let mut filled = 0;
            while filled < cap {
                let n = handle.read(&mut buf[filled..])?;
                if n == 0 {
                    break;
                }
                filled += n;
            }
            buf.truncate(filled);
            Ok(buf)
        }
    }
}

/// 앞 `cap`바이트만 돌려준다. 바이너리 판정(NUL 스니핑)은 lossy `String`으로 할 수
/// 없어서 바이트가 필요하다.
///
/// `WorkingTree`는 파일시스템에서 직접, `Revision`은 `git show`로 읽는다 —
/// `git show <rev>:<path>`는 untracked 파일을 못 읽으므로 둘을 나눠야 한다.
pub async fn file_head_bytes(
    runner: &GitRunner,
    worktree_path: &Path,
    src: FileSource,
    path: &str,
    cap: usize,
) -> Result<Vec<u8>, GitError> {
    match src {
        FileSource::WorkingTree => {
            let worktree = worktree_path.to_path_buf();
            let path = path.to_string();
            tokio::task::spawn_blocking(move || read_head_from_disk(&worktree, &path, cap))
                .await
                .map_err(|e| GitError::Io(std::io::Error::other(e)))?
                .map_err(GitError::Io)
        }
        FileSource::Revision(rev) => {
            // `git show`는 앞부분만 달라고 할 수 없어 전체를 낸다. 러너의
            // `MAX_DIFF_BYTES`가 그 위의 안전망이고, 넘치면 `OutputTooLarge`다.
            let spec = format!("{rev}:{path}");
            let mut bytes = runner
                .run_bytes(worktree_path, &["show", &spec], &[])
                .await?
                .stdout;
            bytes.truncate(cap);
            Ok(bytes)
        }
    }
}

/// 상태별로 **어느 쪽 바이트를 스니핑할지**. `Deleted`는 working tree에 없다.
fn sniff_source(status: &ChangeStatus, merge_base: &str) -> FileSource {
    match status {
        ChangeStatus::Deleted => FileSource::Revision(merge_base.to_string()),
        _ => FileSource::WorkingTree,
    }
}

pub async fn file_diff(
    runner: &GitRunner,
    worktree_path: &Path,
    base_ref: &str,
    file: &str,
    status: &ChangeStatus,
) -> Result<FileDiff, GitError> {
    // 타입 변경·미병합 등. 무엇을 보여줘야 하는지 추측하지 않는다.
    if let ChangeStatus::Other(c) = status {
        return Ok(FileDiff::NonRenderable(*c));
    }

    let mb = merge_base(runner, worktree_path, base_ref).await?;
    // `Revision` 스니핑은 `git show`를 거치는데 그건 앞부분만 달라고 할 수 없어
    // 파일 전체를 낸다 — 큰 삭제 파일이면 여기서 상한에 걸린다. 그것도 **오류가
    // 아니라 상태다**: 안 그러면 삭제된 큰 파일만 배너를 띄우고 추가된 큰 파일은
    // `TooLarge`로 곱게 나가는 비대칭이 생긴다.
    let head = match file_head_bytes(
        runner,
        worktree_path,
        sniff_source(status, &mb),
        file,
        BINARY_SNIFF_BYTES,
    )
    .await
    {
        Ok(head) => head,
        Err(GitError::OutputTooLarge { limit }) => return Ok(FileDiff::TooLarge { limit }),
        Err(e) => return Err(e),
    };
    if head.contains(&0) {
        return Ok(FileDiff::Binary);
    }

    let patch = match runner
        .run(worktree_path, &["diff", "-M", "-C", &mb, "--", file])
        .await
    {
        Ok(out) => out.stdout,
        Err(GitError::OutputTooLarge { limit }) => return Ok(FileDiff::TooLarge { limit }),
        Err(e) => return Err(e),
    };
    if !patch.is_empty() {
        return Ok(FileDiff::Patch(patch));
    }
    // 빈 diff는 "변경 없는 tracked 파일"일 수도 있다. 실제 untracked("??")일 때만
    // --no-index로 합성 (차이 있으면 exit 1). 아니면 빈 patch가 정답.
    let status_out = runner
        .run(worktree_path, &["status", "--porcelain", "-z", "--", file])
        .await?;
    let is_untracked = status_out
        .stdout
        .split('\0')
        .any(|r| r.strip_prefix("?? ").is_some_and(|p| p == file));
    if !is_untracked {
        return Ok(FileDiff::Patch(patch));
    }
    let null_device = if cfg!(windows) { "NUL" } else { "/dev/null" };
    match runner
        .run_expecting(
            worktree_path,
            &["diff", "--no-index", "--", null_device, file],
            &[1],
        )
        .await
    {
        Ok(out) => Ok(FileDiff::Patch(out.stdout)),
        Err(GitError::OutputTooLarge { limit }) => Ok(FileDiff::TooLarge { limit }),
        Err(e) => Err(e),
    }
}

pub async fn working_tree_dirty(
    runner: &GitRunner,
    worktree_path: &Path,
) -> Result<bool, GitError> {
    let out = runner
        .run(worktree_path, &["status", "--porcelain"])
        .await?;
    Ok(!out.stdout.trim().is_empty())
}

#[cfg(test)]
mod parser_tests {
    //! M0에서 추출한 `-z` 파서의 직접 단위 테스트. `branch_compare`의 통합
    //! 테스트가 같은 코드를 간접적으로 덮지만(rename/copy 케이스), 추출 함수를
    //! **직접** 못박아 두 레코드 소비 mutant를 이 레벨에서도 죽인다.
    use super::{parse_name_status_z, parse_numstat_z, ChangeStatus};

    /// rename의 numstat `-z`는 `"adds\tdels\t"`(빈 경로) 뒤 from, to가 각각 별도
    /// NUL 레코드로 온다 — 실측 `git diff --numstat -z -M` 출력 그대로.
    /// name-status는 `R075\0from\0to\0`. 두 파서가 각각 두 레코드를 소비하고
    /// to 경로에 카운트를 조인해야 한다.
    #[test]
    fn rename_two_record_form_yields_one_changed_file_with_counts() {
        let numstat = "1\t0\t\0orig.txt\0renamed.txt\0";
        let name_status = "R075\0orig.txt\0renamed.txt\0";
        let counts = parse_numstat_z(numstat).unwrap();
        let files = parse_name_status_z(name_status, &counts).unwrap();

        assert_eq!(
            files.len(),
            1,
            "rename must collapse to a single file: {files:?}"
        );
        let f = &files[0];
        assert_eq!(f.path, "renamed.txt");
        assert_eq!(
            f.status,
            ChangeStatus::Renamed {
                from: "orig.txt".into()
            }
        );
        // to 경로("renamed.txt")로 카운트가 조인돼야 한다. from/빈 경로에 잘못
        // 넣으면 여기가 (None,None)이 된다.
        assert_eq!(f.additions, Some(1));
        assert_eq!(f.deletions, Some(0));
    }

    /// rename 레코드 뒤에 오는 평범한 레코드가 **밀리지 않는지**. from/to를 한
    /// 레코드만 소비하면 뒤따르는 add가 상태·경로 어긋남으로 무너진다.
    #[test]
    fn record_after_rename_stays_aligned() {
        let numstat = "1\t0\t\0orig.txt\0renamed.txt\02\t0\tafter.txt\0";
        let name_status = "R100\0orig.txt\0renamed.txt\0A\0after.txt\0";
        let counts = parse_numstat_z(numstat).unwrap();
        let files = parse_name_status_z(name_status, &counts).unwrap();

        assert_eq!(files.len(), 2, "{files:?}");
        let after = files
            .iter()
            .find(|f| f.path == "after.txt")
            .expect("record after the rename went missing — misaligned");
        assert_eq!(after.status, ChangeStatus::Added);
        assert_eq!(after.additions, Some(2));
    }
}

#[cfg(all(test, unix))]
mod path_safety_tests {
    //! M1 경로 안전 코어. 각 테스트는 하나의 mutant를 죽이도록 설계됐다 —
    //! 실제 심링크/디렉터리를 `tempdir`에 만들어 검증한다(모킹 금지).
    use super::{resolve_for_read, resolve_for_write, resolve_preserve_symlink, Resolved};
    use std::fs;
    use std::os::unix::fs::symlink;

    fn worktree() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    // --- traversal: `..`/절대경로 어휘 거부 (mutant: 비-Normal 검사 제거) ---
    #[test]
    fn rejects_parent_traversal() {
        let wt = worktree();
        assert!(resolve_for_read(wt.path(), "../../etc/passwd").is_err());
        assert!(resolve_for_read(wt.path(), "a/../../b").is_err());
        assert!(resolve_for_write(wt.path(), "../../etc/passwd").is_err());
        // 절대경로도 거부.
        assert!(resolve_for_read(wt.path(), "/etc/passwd").is_err());
    }

    // --- 중간 심링크가 밖을 가리키면 거부 (mutant: peek().is_some() 거부 제거) ---
    #[test]
    fn rejects_middle_symlink_escape() {
        let wt = worktree();
        let outside = worktree();
        fs::write(outside.path().join("secret"), b"x").unwrap();
        // worktree 안 `link` -> 밖 디렉터리.
        symlink(outside.path(), wt.path().join("link")).unwrap();
        // `link/secret`은 어휘적으로 안이지만 중간 심링크가 밖으로 나간다.
        assert!(resolve_for_read(wt.path(), "link/secret").is_err());
    }

    // --- missing-path escape: leaf가 없어도 parent 심링크면 거부 ---
    // (mutant: AllowMissingLeaf에서 심링크 검사를 건너뛰거나 middle 거부 제거)
    #[test]
    fn rejects_missing_leaf_through_parent_symlink() {
        let wt = worktree();
        let outside = worktree();
        // worktree 안 `link` -> 밖 디렉터리. `link/newfile`은 아직 없다.
        symlink(outside.path(), wt.path().join("link")).unwrap();
        // create가 없는 leaf를 인가하려 해도 parent가 밖을 가리키면 거부해야 한다.
        assert!(resolve_for_write(wt.path(), "link/newfile").is_err());
    }

    // --- leaf 심링크는 un-follow로 돌려준다 (mutant: Symlink -> Regular) ---
    #[test]
    fn leaf_symlink_returned_unfollowed_preserve_mode() {
        let wt = worktree();
        let outside = worktree();
        let target = outside.path().join("target");
        fs::write(&target, b"content").unwrap();
        symlink(&target, wt.path().join("link")).unwrap();
        // delete/rename-source: 링크 자체를 대상으로 — 따라가면 밖 파일을 건드린다.
        match resolve_preserve_symlink(wt.path(), "link").unwrap() {
            Resolved::Symlink(p) => assert_eq!(p, wt.path().join("link")),
            other => panic!("leaf 심링크가 un-follow 되지 않았다: {other:?}"),
        }
    }

    // read 모드도 leaf 심링크를 따라가지 않는다(git 의미론).
    #[test]
    fn leaf_symlink_returned_unfollowed_read_mode() {
        let wt = worktree();
        symlink("some-target", wt.path().join("link")).unwrap();
        assert!(matches!(
            resolve_for_read(wt.path(), "link").unwrap(),
            Resolved::Symlink(_)
        ));
    }

    // --- null-byte 거부 (mutant: null-byte 가드 제거) ---
    // Unix stdlib도 interior-nul을 독립적으로 거부하므로("...NUL byte") `is_err`만으로는
    // 우리 가드 제거를 죽일 수 없다 — 공허해진다. **우리 가드가 먼저 쳤는지**를
    // 메시지로 확인해 mutant를 죽인다(우리 메시지는 소문자 "null byte").
    #[test]
    fn rejects_null_byte() {
        let wt = worktree();
        let e = resolve_for_read(wt.path(), "a\0b").unwrap_err();
        assert!(
            e.to_string().contains("null byte"),
            "우리 null-byte 가드가 아니라 stdlib이 거부했다: {e}"
        );
        assert!(resolve_for_write(wt.path(), "a\0b")
            .unwrap_err()
            .to_string()
            .contains("null byte"));
    }

    // --- 정상 깊은 실경로는 read 모드에서 해석된다 ---
    #[test]
    fn resolves_deep_real_path_read() {
        let wt = worktree();
        fs::create_dir_all(wt.path().join("a/b/c")).unwrap();
        fs::write(wt.path().join("a/b/c/file"), b"hi").unwrap();
        assert_eq!(
            resolve_for_read(wt.path(), "a/b/c/file").unwrap(),
            Resolved::Regular(wt.path().join("a/b/c/file")),
        );
    }

    // --- read 모드는 없는 leaf를 거부; write 모드는 인가 ---
    // (mutant: AllowMissingLeaf 분기를 ExistingOnly에도 적용하면 read가 없는 파일을 통과)
    #[test]
    fn read_requires_existence_write_allows_missing() {
        let wt = worktree();
        fs::create_dir_all(wt.path().join("a/b")).unwrap();
        // read: 없는 leaf -> 에러.
        assert!(resolve_for_read(wt.path(), "a/b/newfile").is_err());
        // write: 없는 leaf -> Regular(미래 경로).
        assert_eq!(
            resolve_for_write(wt.path(), "a/b/newfile").unwrap(),
            Resolved::Regular(wt.path().join("a/b/newfile")),
        );
        // write: 없는 중간 디렉터리 + 없는 leaf도 인가(create가 parent를 만든다).
        assert_eq!(
            resolve_for_write(wt.path(), "a/b/new/deep/file").unwrap(),
            Resolved::Regular(wt.path().join("a/b/new/deep/file")),
        );
    }

    // write 모드도 어휘 검사(`..`)와 중간 실-심링크 거부를 유지한다.
    #[test]
    fn write_mode_still_rejects_traversal_and_real_middle_symlink() {
        let wt = worktree();
        let outside = worktree();
        symlink(outside.path(), wt.path().join("link")).unwrap();
        assert!(resolve_for_write(wt.path(), "a/../../b").is_err());
        // 존재하는 중간 심링크는 leaf가 있든 없든 거부.
        assert!(resolve_for_write(wt.path(), "link/sub/newfile").is_err());
    }
}
