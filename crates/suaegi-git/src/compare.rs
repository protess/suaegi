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
    // from, to가 각각 별도 NUL 레코드로 이어진다.
    let numstat_args = format!("diff --numstat -z -M -C {mb}");
    let mut counts: HashMap<String, (Option<u32>, Option<u32>)> = HashMap::new();
    let mut numstat_records = numstat.stdout.split('\0');
    while let Some(record) = numstat_records.next() {
        if record.is_empty() {
            continue;
        }
        // splitn(3): 파일명에 탭이 있어도 세 번째 조각(경로)이 절단되지 않게
        let mut parts = record.splitn(3, '\t');
        let (Some(a), Some(d)) = (parts.next(), parts.next()) else {
            return Err(GitError::Parse {
                args: numstat_args.clone(),
                detail: format!("truncated record {record:?}"),
            });
        };
        let adds = parse_count(a, &numstat_args)?;
        let dels = parse_count(d, &numstat_args)?;
        match parts.next() {
            Some(path) if !path.is_empty() => {
                counts.insert(path.to_string(), (adds, dels));
            }
            _ => {
                // rename/copy: from, to가 이어지는 별도 레코드
                let _from = numstat_records.next();
                let to = numstat_records.next().ok_or_else(|| GitError::Parse {
                    args: numstat_args.clone(),
                    detail: "rename record missing target path".to_string(),
                })?;
                counts.insert(to.to_string(), (adds, dels));
            }
        }
    }

    // name-status -z 레코드: "X\0path\0", rename "R100\0from\0to\0", copy "C100\0from\0to\0"
    let ns_args = format!("diff --name-status -z -M -C {mb}");
    let mut files = Vec::new();
    let mut records = name_status.stdout.split('\0');
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
                    args: ns_args.clone(),
                    detail: format!("record {code:?} missing source path"),
                })?;
                let to = records.next().ok_or_else(|| GitError::Parse {
                    args: ns_args.clone(),
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
                    args: ns_args.clone(),
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
enum Resolved {
    Regular(PathBuf),
    /// **따라가지 않는다.** git은 심볼릭 링크를 링크 *내용*으로 다룬다.
    Symlink(PathBuf),
}

/// worktree 밖을 읽지 않도록 상대 경로를 검증하며 내려간다.
///
/// **어휘적 포함만으로는 부족하다.** `a/b/c`가 어휘적으로 안에 있어도 `b`가
/// worktree 밖을 가리키는 심볼릭 링크면 `open`이 그걸 따라간다. 그리고 마지막
/// 컴포넌트에만 `symlink_metadata`를 부르는 것도 부족하다 — 그 호출 자체가
/// **중간 컴포넌트는 이미 따라간 뒤**다. 그래서 한 컴포넌트씩 내려가며 본다.
fn resolve_in_worktree(worktree: &Path, path: &str) -> std::io::Result<Resolved> {
    let reject = |detail: &str| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{detail}: {path:?}"),
        )
    };
    let rel = Path::new(path);
    if path.is_empty() {
        return Err(reject("empty path"));
    }
    // 절대 경로, `..`, 루트 접두는 전부 거절. git이 내는 경로에는 없지만
    // `file_head_bytes`는 공개 API다.
    if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err(reject("path escapes the worktree"));
    }

    let mut current = worktree.to_path_buf();
    let mut components = rel.components().peekable();
    while let Some(component) = components.next() {
        current.push(component);
        if std::fs::symlink_metadata(&current)?
            .file_type()
            .is_symlink()
        {
            if components.peek().is_some() {
                return Err(reject("path traverses a symlink"));
            }
            return Ok(Resolved::Symlink(current));
        }
    }
    Ok(Resolved::Regular(current))
}

/// 블로킹 부분만 뽑았다 — `spawn_blocking`에 넘기고, 그대로 단위 테스트한다.
fn read_head_from_disk(worktree: &Path, path: &str, cap: usize) -> std::io::Result<Vec<u8>> {
    match resolve_in_worktree(worktree, path)? {
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
