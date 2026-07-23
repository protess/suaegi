//! 워크트리 안의 파일시스템 리스팅과 안전 read/write. 모든 진입점은 M1 경로 안전 코어
//! (`compare.rs`의 `resolve_for_read`/`resolve_for_write`)를 **먼저** 통과한다 —
//! traversal/심링크-escape/null-byte를 거기서 거른다. 이 모듈은 그 위에서 디렉터리 한
//! 레벨을 읽고(M2) 파일 하나를 안전하게 읽고(M4) 원자적으로 쓴다(M5).

use crate::compare::{resolve_for_read, resolve_for_write, Resolved, BINARY_SNIFF_BYTES};
use std::io::{self, Write};
use std::path::{Component, Path};
use std::time::SystemTime;

/// 텍스트 파일 read 상한. Orca `MAX_TEXT_FILE_SIZE`(`filesystem.ts:130`)와 동일한
/// 50 MB. **버퍼링 전에** stat 크기로 걸러 큰 파일을 통째로 메모리에 올리지 않는다
/// (`filesystem.ts:565-569`). `FileDiff::TooLarge`와 같은 규율.
pub const MAX_TEXT_FILE_SIZE: u64 = 50 * 1024 * 1024;

/// 디렉터리 한 엔트리. `is_dir`/`is_symlink`는 **링크 자체**의 타입이다
/// (`symlink_metadata`, Orca `withFileTypes` + `:447-462`) — 심링크→디렉터리라도
/// `is_dir`는 `false`, `is_symlink`는 `true`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    pub is_symlink: bool,
}

/// 파일 내용 분류. 바이너리는 **절대 lossy 문자열로 돌려주지 않는다** — 편집기가
/// 깨진 텍스트를 저장해 파일을 망가뜨리지 않도록.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileContent {
    Text(String),
    /// 앞 `BINARY_SNIFF_BYTES`에 NUL이 있거나 UTF-8이 아니다.
    Binary,
}

/// `read_file`의 결과. `TooLarge`는 `FileDiff::TooLarge`처럼 **형제 변형**이다 —
/// 상한을 넘긴 파일은 읽지 않고 여기로 빠진다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileRead {
    Ready {
        content: FileContent,
        size: u64,
    },
    /// stat 크기가 `limit`를 넘겨 버퍼링하지 않았다.
    TooLarge {
        limit: u64,
    },
}

/// 워크트리 상대 `rel_dir`의 엔트리를 한 레벨만 읽는다(재귀 아님 — lazy per-dir).
///
/// `rel_dir`은 `resolve_for_read`를 먼저 통과한다(escape 거부). 그 결과가
/// `Resolved::Symlink`면 **디렉터리로 따라가지 않고 거부**한다 — Orca는 심링크를
/// non-dir로 취급해(`isDirectoryEntry` `:447-462`) 트리에서 확장 대상이 되지 않으므로,
/// 실제 디렉터리가 아닌 것의 리스팅은 성립하지 않는다.
///
/// 정렬은 **디렉터리 먼저, 그 다음 이름순**(Orca `:521-526`). 개별 엔트리의
/// 읽기 실패(권한 거부 등)는 그 엔트리만 건너뛰고 나머지 리스트는 살린다.
pub fn list_dir(worktree: &Path, rel_dir: &str) -> io::Result<Vec<DirEntry>> {
    // 워크트리 루트 자체는 containment 기준점(신뢰 대상)이라 resolver의 컴포넌트
    // 검사 밖이다 — resolver는 `""`/`"."`를 비-`Normal`로 거부하므로 여기서 직접 편다.
    let dir = if rel_dir.is_empty() || rel_dir == "." {
        worktree.to_path_buf()
    } else {
        match resolve_for_read(worktree, rel_dir)? {
            Resolved::Regular(p) => p,
            Resolved::Symlink(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("refusing to list a symlink as a directory: {rel_dir:?}"),
                ));
            }
        }
    };

    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        // 개별 엔트리 오류는 전체 리스트를 죽이지 않는다(단일 bad 엔트리 degrade).
        let Ok(entry) = entry else { continue };
        // **`symlink_metadata`**(NOT `metadata`): 심링크를 따라가지 않아 `is_dir`/
        // `is_symlink`가 링크 자체의 타입을 반영한다.
        let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        let file_type = meta.file_type();
        entries.push(DirEntry {
            name: entry.file_name().to_string_lossy().into_owned(),
            is_dir: file_type.is_dir(),
            is_symlink: file_type.is_symlink(),
        });
    }

    entries.sort_by(|a, b| {
        if a.is_dir != b.is_dir {
            // 디렉터리 먼저.
            return if a.is_dir {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            };
        }
        a.name.cmp(&b.name)
    });
    Ok(entries)
}

/// 바이트를 텍스트/바이너리로 분류. 앞 `BINARY_SNIFF_BYTES`에 NUL이 있으면 바이너리
/// (Orca `:426-434`). 그렇지 않고 유효한 UTF-8일 때만 `Text` — 유효하지 않은 UTF-8은
/// `Binary`로 (lossy 디코드 금지, 이 지점이 Orca의 `toString('utf-8')`와 다르다).
fn classify_bytes(bytes: Vec<u8>) -> FileContent {
    let sniff_len = bytes.len().min(BINARY_SNIFF_BYTES);
    if bytes[..sniff_len].contains(&0) {
        return FileContent::Binary;
    }
    match String::from_utf8(bytes) {
        Ok(text) => FileContent::Text(text),
        Err(_) => FileContent::Binary,
    }
}

/// 상한을 인자로 받는 내부 구현. 테스트가 50 MB를 실제로 할당하지 않고 경계 로직을
/// 검증할 수 있도록 분리했다(`read_head_from_disk` 추출과 같은 패턴).
fn read_file_with_cap(worktree: &Path, rel_path: &str, cap: u64) -> io::Result<FileRead> {
    match resolve_for_read(worktree, rel_path)? {
        Resolved::Symlink(link) => {
            // git은 심링크를 그 **내용**(타깃 경로 문자열)으로 다룬다 —
            // `read_head_from_disk`와 일관되게 링크를 따라가지 않고 링크 내용을 읽는다.
            let bytes = std::fs::read_link(&link)?
                .into_os_string()
                .into_encoded_bytes();
            let size = bytes.len() as u64;
            if size > cap {
                return Ok(FileRead::TooLarge { limit: cap });
            }
            Ok(FileRead::Ready {
                content: classify_bytes(bytes),
                size,
            })
        }
        Resolved::Regular(file) => {
            // **버퍼링 전에** stat 크기로 상한 검사 — 큰 파일을 메모리에 올리지 않는다.
            let size = std::fs::metadata(&file)?.len();
            if size > cap {
                return Ok(FileRead::TooLarge { limit: cap });
            }
            let bytes = std::fs::read(&file)?;
            Ok(FileRead::Ready {
                content: classify_bytes(bytes),
                size,
            })
        }
    }
}

/// 워크트리 상대 `rel_path`를 안전하게 읽는다. `resolve_for_read`를 먼저 통과하고,
/// `MAX_TEXT_FILE_SIZE`를 넘기면 버퍼링 없이 `TooLarge`. 바이너리(NUL/비-UTF8)는
/// `FileContent::Binary`로, 절대 lossy 텍스트로 돌려주지 않는다.
pub fn read_file(worktree: &Path, rel_path: &str) -> io::Result<FileRead> {
    read_file_with_cap(worktree, rel_path, MAX_TEXT_FILE_SIZE)
}

/// 파일이 우리가 마지막으로 본 이후 **밖에서 바뀌었는지**만 감지하기 위한 최소 지문.
/// `size`(`metadata.len()`) + `mtime`(`metadata.modified()`). 콘텐츠 해시가 아니라
/// stat 기반이라 값싸고, 편집기가 저장 후 재베이스라인하는 데 충분하다
/// (Orca `editor-autosave-controller.ts:143-145`). `SystemTime`은 `Eq`라 그대로 비교한다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSignature {
    pub size: u64,
    pub mtime: SystemTime,
}

impl FileSignature {
    fn from_metadata(meta: &std::fs::Metadata) -> io::Result<Self> {
        Ok(Self {
            size: meta.len(),
            mtime: meta.modified()?,
        })
    }
}

/// `write_file`의 결과. 손실 없이 썼거나(`Written`), 밖에서 바뀐 파일을 덮어쓰지 않고
/// 멈췄거나(`StaleConflict`) 둘 중 하나다 — 에러(권한/traversal/denylist)는 `Err`로 별도.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteOutcome {
    /// 성공. `signature`는 **새 on-disk 지문**이다 — 편집기가 이걸로 재베이스라인해
    /// 다음 저장의 staleness 기준으로 삼는다(Orca autosave-controller:143-145).
    Written { signature: FileSignature },
    /// `expected`가 주어졌는데 on-disk 지문이 달라(밖에서 편집됨) **쓰지 않았다.**
    /// `disk`는 현재 디스크 지문이거나, 파일이 우리 밑에서 삭제됐으면 `None`이다
    /// (스펙은 `FileSignature`지만 삭제 케이스를 손실 없이 담으려 `Option`으로 확장).
    StaleConflict { disk: Option<FileSignature> },
}

/// 워크트리 상대 `rel_path`에 `content`를 **원자적으로** 쓴다. 임의의 바이트를 받는다
/// (`&[u8]`) — 편집기가 UTF-8이 아닌 파일도 저장할 수 있어야 하므로 텍스트를 강요하지 않는다.
///
/// `expected`가 `Some(sig)`면 쓰기 전에 현재 디스크 지문과 대조해, 다르면
/// `StaleConflict`로 빠지고 **밖의 편집을 절대 덮어쓰지 않는다**(데이터 손실 crux).
/// `None`이면 무조건 쓴다(첫 저장/새 파일).
///
/// 검사 순서:
/// 1. `resolve_for_write`로 경로 재검증(traversal/중간-심링크/null-byte/없는-parent-심링크
///    거부 — M1 게이트). 렌더러가 준 raw 경로를 신뢰하지 않는다.
/// 2. `.git/` denylist: **어느 컴포넌트든** `.git`이면 거부(중첩 `sub/.git/hooks/...`도
///    똑같이 위험한 코드실행 벡터). M1 containment는 워크트리 *안쪽*이라 통과시키므로
///    여기서 별도로 막는다(M1 리뷰 이월).
/// 3. leaf-symlink 거부: 기존 leaf가 심링크면 따라가지 않고 거부한다 — 따라가면 밖에
///    쓰거나 심링크를 일반 파일로 망가뜨린다(M1 리뷰 이월).
/// 4. 디렉터리 가드: 대상이 이미 디렉터리면 거부(`filesystem.ts:816`).
/// 5. parent 존재 요구: 원자적 쓰기는 형제 temp가 필요하다. M5는 `mkdir -p` 안 한다
///    (별도 create-dir 연산).
/// 6. staleness 검사(위 참조).
/// 7. 원자적 쓰기: 같은 디렉터리에 temp 작성 → `sync_all` → `persist`(rename).
///
/// staleness stat과 persist 사이에는 경계 있는 TOCTOU가 남는다(동시 로컬 쓰기). Orca와
/// 같고 M2/M4 리뷰에서 수용됐다 — 락을 걸지 않고 문서화만 한다.
pub fn write_file(
    worktree: &Path,
    rel_path: &str,
    content: &[u8],
    expected: Option<&FileSignature>,
) -> io::Result<WriteOutcome> {
    // 1. M1 경로 게이트. raw 경로를 절대 신뢰하지 않는다.
    let resolved = resolve_for_write(worktree, rel_path)?;

    // 2. `.git/` denylist — 어느 컴포넌트든. resolve가 모든 컴포넌트를 `Normal`로
    //    보장했으므로 여기선 이름만 본다. 첫 컴포넌트만이 아니라 전부 검사한다:
    //    `submodule/.git/hooks/pre-commit`도 훅 코드실행 벡터다.
    if Path::new(rel_path)
        .components()
        .any(|c| matches!(c, Component::Normal(name) if name == ".git"))
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("refusing to write inside a .git directory: {rel_path:?}"),
        ));
    }

    // 3. leaf-symlink 거부. 따라가면 밖에 쓰거나 심링크를 일반 파일로 덮어써 망가뜨린다.
    let target = match resolved {
        Resolved::Regular(p) => p,
        Resolved::Symlink(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("refusing to write through an existing symlink: {rel_path:?}"),
            ));
        }
    };

    // 4. 디렉터리 가드. `symlink_metadata`(lstat)로 대상 자체를 본다 — 디렉터리를
    //    temp-rename으로 덮으려 하면 안 된다(`filesystem.ts:816`).
    if let Ok(meta) = std::fs::symlink_metadata(&target) {
        if meta.file_type().is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("refusing to overwrite a directory: {rel_path:?}"),
            ));
        }
    }

    // 5. parent 존재 요구. 원자적 쓰기는 같은 디렉터리에 형제 temp를 만든다.
    let parent = target.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("write target has no parent directory: {rel_path:?}"),
        )
    })?;
    if !parent.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("parent directory does not exist: {rel_path:?}"),
        ));
    }

    // 6. staleness 검사(데이터 손실 crux). `expected`가 있으면 현재 디스크 지문과
    //    대조 — 다르면 밖에서 편집된 것이므로 덮어쓰지 않는다. 삭제됐으면(NotFound)
    //    그것도 conflict(우리 밑에서 사라짐).
    if let Some(expected_sig) = expected {
        match std::fs::metadata(&target) {
            Ok(meta) => {
                let disk = FileSignature::from_metadata(&meta)?;
                if disk != *expected_sig {
                    return Ok(WriteOutcome::StaleConflict { disk: Some(disk) });
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Ok(WriteOutcome::StaleConflict { disk: None });
            }
            Err(e) => return Err(e),
        }
    }

    // 7. 원자적 쓰기: 형제 temp → 바이트 기록 → fsync → rename. temp는 실패 시
    //    `NamedTempFile` drop이 정리한다(`persistence.rs:200-206` 패턴).
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(content)?;
    tmp.as_file().sync_all()?;
    tmp.persist(&target).map_err(|e| e.error)?;

    // 새 on-disk 지문을 돌려준다 — 편집기가 재베이스라인한다.
    let meta = std::fs::metadata(&target)?;
    Ok(WriteOutcome::Written {
        signature: FileSignature::from_metadata(&meta)?,
    })
}

#[cfg(all(test, unix))]
mod tests {
    //! 실제 파일/디렉터리/심링크를 `tempdir`에 만들어 검증한다(모킹 금지). 각 crux
    //! 테스트는 하나의 mutant를 죽이도록 설계됐다.
    use super::{
        list_dir, read_file, read_file_with_cap, write_file, FileContent, FileRead, FileSignature,
        WriteOutcome,
    };
    use std::fs;
    use std::os::unix::fs::symlink;

    fn worktree() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    // --- M2: 정렬은 디렉터리 먼저, 그 다음 이름순 ---
    // (mutant: is_dir 키를 떨어뜨려 이름순만 남기면 dir "zebra"가 file "apple" 뒤로 간다)
    #[test]
    fn list_sorts_dirs_first_then_name() {
        let wt = worktree();
        fs::create_dir(wt.path().join("zebra")).unwrap();
        fs::write(wt.path().join("apple"), b"x").unwrap();
        fs::write(wt.path().join("mango"), b"x").unwrap();
        let entries = list_dir(wt.path(), ".").unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        // dir 먼저(zebra) → 나머지 파일 이름순(apple, mango).
        assert_eq!(names, vec!["zebra", "apple", "mango"]);
        assert!(entries[0].is_dir);
    }

    // --- M2: 심링크는 링크 타입으로 보고 (symlink_metadata, NOT metadata) ---
    // (mutant: symlink_metadata -> metadata 이면 심링크→dir이 is_dir:true/is_symlink:false)
    #[test]
    fn list_symlink_to_dir_reports_link_type_not_target() {
        let wt = worktree();
        let real = worktree();
        fs::create_dir(real.path().join("realdir")).unwrap();
        // 워크트리 안 `link` -> 밖 실제 디렉터리.
        symlink(real.path().join("realdir"), wt.path().join("link")).unwrap();
        let entries = list_dir(wt.path(), ".").unwrap();
        let link = entries.iter().find(|e| e.name == "link").unwrap();
        // symlink_metadata라 링크 타입: 심링크이고 디렉터리 아님.
        assert!(
            link.is_symlink,
            "심링크가 is_symlink:true로 보고되지 않았다"
        );
        assert!(
            !link.is_dir,
            "심링크 타깃(dir)을 따라가 is_dir:true가 됐다 — metadata mutant"
        );
    }

    // --- M2: escape 거부 (M1 재사용이지만 리스팅 진입점에서 고정) ---
    #[test]
    fn list_rejects_traversal() {
        let wt = worktree();
        assert!(list_dir(wt.path(), "../..").is_err());
        assert!(list_dir(wt.path(), "/etc").is_err());
    }

    // --- M2: rel_dir이 심링크면 디렉터리로 따라가지 않고 거부 ---
    #[test]
    fn list_refuses_symlink_dir() {
        let wt = worktree();
        let real = worktree();
        fs::create_dir(real.path().join("realdir")).unwrap();
        fs::write(real.path().join("realdir/inside"), b"x").unwrap();
        symlink(real.path().join("realdir"), wt.path().join("link")).unwrap();
        // `link`을 디렉터리로 리스팅하려 하면 거부(밖 디렉터리 내용 유출 방지).
        assert!(list_dir(wt.path(), "link").is_err());
    }

    // --- M2: 개별 엔트리는 읽되 정상 엔트리들이 다 나온다 ---
    #[test]
    fn list_returns_all_regular_entries() {
        let wt = worktree();
        fs::write(wt.path().join("a.txt"), b"x").unwrap();
        fs::write(wt.path().join("b.txt"), b"x").unwrap();
        fs::create_dir(wt.path().join("sub")).unwrap();
        let entries = list_dir(wt.path(), ".").unwrap();
        assert_eq!(entries.len(), 3);
    }

    // --- M4: 상한 초과 파일은 읽지 않고 TooLarge ---
    // (mutant: `size > cap` 검사를 떨어뜨리면 상한 넘긴 파일을 Text로 읽어버린다)
    #[test]
    fn read_over_cap_is_too_large_without_buffering() {
        let wt = worktree();
        // cap보다 큰 텍스트 파일. 작은 테스트 cap으로 50 MB 할당을 피한다.
        fs::write(wt.path().join("big.txt"), b"0123456789ABCDEF").unwrap(); // 16바이트
        let got = read_file_with_cap(wt.path(), "big.txt", 8).unwrap();
        assert_eq!(got, FileRead::TooLarge { limit: 8 });
    }

    // 경계: cap과 정확히 같으면 통과, cap+1이면 TooLarge.
    #[test]
    fn read_cap_boundary() {
        let wt = worktree();
        fs::write(wt.path().join("exact"), b"12345678").unwrap(); // 8바이트
        assert!(matches!(
            read_file_with_cap(wt.path(), "exact", 8).unwrap(),
            FileRead::Ready { size: 8, .. }
        ));
        fs::write(wt.path().join("over"), b"123456789").unwrap(); // 9바이트
        assert_eq!(
            read_file_with_cap(wt.path(), "over", 8).unwrap(),
            FileRead::TooLarge { limit: 8 }
        );
    }

    // --- M4: 앞 8192B에 NUL 있으면 Binary (텍스트로 디코드 금지) ---
    // (mutant: NUL 스니핑을 떨어뜨리면 lossy/UTF-8 텍스트로 나온다)
    #[test]
    fn read_null_byte_is_binary_not_text() {
        let wt = worktree();
        fs::write(wt.path().join("bin"), b"abc\0def").unwrap();
        let got = read_file(wt.path(), "bin").unwrap();
        assert_eq!(
            got,
            FileRead::Ready {
                content: FileContent::Binary,
                size: 7
            }
        );
    }

    // --- M4: 유효하지 않은 UTF-8은 Binary (lossy 디코드 금지) ---
    // (mutant: from_utf8 실패 시 from_utf8_lossy로 Text를 내면 이 테스트가 깨진다)
    #[test]
    fn read_invalid_utf8_is_binary_not_lossy() {
        let wt = worktree();
        // NUL은 없지만 유효한 UTF-8도 아닌 바이트(0xFF).
        fs::write(wt.path().join("latin1"), b"caf\xe9").unwrap();
        let got = read_file(wt.path(), "latin1").unwrap();
        assert!(
            matches!(
                got,
                FileRead::Ready {
                    content: FileContent::Binary,
                    ..
                }
            ),
            "유효하지 않은 UTF-8이 Binary가 아니라 {got:?}로 나왔다"
        );
    }

    // --- M4: 정상 UTF-8 텍스트는 Text로 ---
    #[test]
    fn read_valid_utf8_is_text() {
        let wt = worktree();
        fs::write(wt.path().join("hi.txt"), "héllo 안녕".as_bytes()).unwrap();
        let got = read_file(wt.path(), "hi.txt").unwrap();
        match got {
            FileRead::Ready {
                content: FileContent::Text(s),
                ..
            } => assert_eq!(s, "héllo 안녕"),
            other => panic!("정상 UTF-8이 Text가 아니다: {other:?}"),
        }
    }

    // --- M4: escape 거부 ---
    #[test]
    fn read_rejects_traversal() {
        let wt = worktree();
        assert!(read_file(wt.path(), "../../etc/passwd").is_err());
        assert!(read_file(wt.path(), "/etc/passwd").is_err());
    }

    // --- M4: leaf 심링크는 링크 내용(타깃 문자열)을 읽는다(git 의미론) ---
    #[test]
    fn read_leaf_symlink_reads_link_content() {
        let wt = worktree();
        symlink("target-path", wt.path().join("link")).unwrap();
        let got = read_file(wt.path(), "link").unwrap();
        match got {
            FileRead::Ready {
                content: FileContent::Text(s),
                size,
            } => {
                assert_eq!(s, "target-path");
                assert_eq!(size, "target-path".len() as u64);
            }
            other => panic!("leaf 심링크가 링크 내용으로 읽히지 않았다: {other:?}"),
        }
    }

    // ============================ M5: 안전 파일 write ============================

    fn written_sig(outcome: WriteOutcome) -> FileSignature {
        match outcome {
            WriteOutcome::Written { signature } => signature,
            other => panic!("Written을 기대했으나 {other:?}"),
        }
    }

    // --- M5 crux: `.git/` denylist — 어느 컴포넌트든 (코드실행 벡터) ---
    // (mutant: denylist를 떼면 parent가 있는 한 write가 성공한다. 그래서 parent(.git/hooks,
    //  sub/.git)를 실제로 만들어, mutation-off 시 파일이 생기게 해서 이 테스트가 깨지게 한다)
    #[test]
    fn write_rejects_dot_git_first_component() {
        let wt = worktree();
        // denylist가 없으면 write가 성공하도록 parent를 실존시킨다.
        fs::create_dir_all(wt.path().join(".git/hooks")).unwrap();
        let target = wt.path().join(".git/hooks/pre-commit");
        assert!(write_file(
            wt.path(),
            ".git/hooks/pre-commit",
            b"#!/bin/sh\nevil\n",
            None
        )
        .is_err());
        assert!(
            !target.exists(),
            "denylist가 뚫려 .git/hooks/pre-commit이 생성됐다"
        );
    }

    #[test]
    fn write_rejects_dot_git_nested_component() {
        let wt = worktree();
        // 중첩 `sub/.git`도 훅 코드실행 벡터 — 첫 컴포넌트만 검사하면 뚫린다.
        fs::create_dir_all(wt.path().join("sub/.git")).unwrap();
        let target = wt.path().join("sub/.git/config");
        assert!(write_file(wt.path(), "sub/.git/config", b"[core]\n", None).is_err());
        assert!(!target.exists(), "중첩 .git denylist가 뚫렸다");
    }

    // --- M5 crux: leaf-symlink 거부 (따라가면 밖에 쓰거나 링크를 망가뜨린다) ---
    // (mutant: 거부를 떼면 persist가 심링크를 일반 파일로 교체한다 — `link`가 더 이상
    //  심링크가 아니게 되어 이 테스트가 깨진다. 밖 타깃도 안 바뀜을 함께 확인한다.)
    #[test]
    fn write_refuses_existing_leaf_symlink() {
        let wt = worktree();
        let outside = worktree();
        let outside_target = outside.path().join("target");
        fs::write(&outside_target, b"ORIGINAL").unwrap();
        // 워크트리 안 `link` -> 밖 파일.
        symlink(&outside_target, wt.path().join("link")).unwrap();

        assert!(write_file(wt.path(), "link", b"clobber", None).is_err());

        // 밖 타깃은 건드리지 않았다.
        assert_eq!(
            fs::read(&outside_target).unwrap(),
            b"ORIGINAL",
            "심링크를 따라가 밖 파일을 덮어썼다"
        );
        // 심링크는 일반 파일로 교체되지 않았다.
        assert!(
            fs::symlink_metadata(wt.path().join("link"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "심링크가 일반 파일로 교체됐다"
        );
    }

    // --- M5 crux: 디렉터리 가드 ---
    // (mutant: 가드를 떼도 persist(file->dir) rename은 OS 에러라 여전히 Err다. 그래서
    //  `is_err`만으로는 mutant를 못 죽인다 — 우리 가드가 낸 **구체 메시지**를 확인한다.)
    #[test]
    fn write_refuses_directory_target() {
        let wt = worktree();
        fs::create_dir(wt.path().join("somedir")).unwrap();
        let e = write_file(wt.path(), "somedir", b"x", None).unwrap_err();
        assert!(
            e.to_string().contains("refusing to overwrite a directory"),
            "우리 디렉터리 가드가 아니라 다른 경로로 거부됐다: {e}"
        );
    }

    // --- M5 crux: staleness — 밖에서 바뀐 파일을 덮어쓰지 않는다 (데이터 손실) ---
    // (mutant: 지문 비교를 always-equal로 바꾸면 stale write가 밖 편집을 덮어써
    //  파일 내용이 "clobber"가 되어 이 테스트가 깨진다.)
    #[test]
    fn write_stale_conflict_does_not_clobber_external_edit() {
        let wt = worktree();
        let sig = written_sig(write_file(wt.path(), "f.txt", b"aaaa", None).unwrap());
        // 밖에서 편집(크기까지 달라 mtime 해상도와 무관하게 지문이 다르다).
        fs::write(wt.path().join("f.txt"), b"EXTERNAL-EDIT").unwrap();

        let out = write_file(wt.path(), "f.txt", b"clobber", Some(&sig)).unwrap();
        assert!(
            matches!(out, WriteOutcome::StaleConflict { .. }),
            "밖에서 바뀐 파일에 StaleConflict가 아니라 {out:?}"
        );
        assert_eq!(
            fs::read(wt.path().join("f.txt")).unwrap(),
            b"EXTERNAL-EDIT",
            "stale write가 밖 편집을 덮어썼다"
        );
    }

    // --- M5: 파일이 우리 밑에서 삭제됐으면 그것도 conflict(disk=None) ---
    #[test]
    fn write_stale_conflict_when_deleted_underneath() {
        let wt = worktree();
        let sig = written_sig(write_file(wt.path(), "gone.txt", b"data", None).unwrap());
        fs::remove_file(wt.path().join("gone.txt")).unwrap();
        let out = write_file(wt.path(), "gone.txt", b"x", Some(&sig)).unwrap();
        assert_eq!(out, WriteOutcome::StaleConflict { disk: None });
    }

    // --- M5: 원자적 happy-path + 재베이스라인 지문이 false conflict를 안 낸다 ---
    #[test]
    fn write_happy_path_and_rebaseline_signature() {
        let wt = worktree();
        let out = write_file(wt.path(), "new.txt", b"hi", None).unwrap();
        let sig = written_sig(out);
        // 정확히 b"hi"가 쓰였다.
        assert_eq!(fs::read(wt.path().join("new.txt")).unwrap(), b"hi");
        // 돌려준 지문이 on-disk 파일과 일치한다.
        let disk = fs::metadata(wt.path().join("new.txt")).unwrap();
        assert_eq!(sig.size, disk.len());
        assert_eq!(sig.mtime, disk.modified().unwrap());
        // 밖 변경 없이 그 지문으로 다시 쓰면 false conflict 없이 또 Written.
        let out2 = write_file(wt.path(), "new.txt", b"hi", Some(&sig)).unwrap();
        assert!(
            matches!(out2, WriteOutcome::Written { .. }),
            "변경 없는 재저장이 false conflict를 냈다: {out2:?}"
        );
    }

    // --- M5: parent가 없으면 에러 (M5는 mkdir -p 안 한다) ---
    #[test]
    fn write_requires_existing_parent() {
        let wt = worktree();
        // `missing/` 디렉터리가 없다.
        assert!(write_file(wt.path(), "missing/file.txt", b"x", None).is_err());
        assert!(!wt.path().join("missing").exists());
    }

    // --- M5: traversal은 여전히 거부(M1 재사용을 write 진입점에서 고정) ---
    #[test]
    fn write_rejects_traversal() {
        let wt = worktree();
        assert!(write_file(wt.path(), "../../etc/evil", b"x", None).is_err());
        assert!(write_file(wt.path(), "/etc/evil", b"x", None).is_err());
    }

    // --- M5: read_file과의 round-trip ---
    #[test]
    fn write_then_read_round_trip() {
        let wt = worktree();
        write_file(wt.path(), "note.txt", "héllo 안녕".as_bytes(), None).unwrap();
        match read_file(wt.path(), "note.txt").unwrap() {
            FileRead::Ready {
                content: FileContent::Text(s),
                ..
            } => assert_eq!(s, "héllo 안녕"),
            other => panic!("write 후 read가 Text가 아니다: {other:?}"),
        }
    }

    // --- M5: 임의 바이트(비-UTF8)도 그대로 쓴다 — 편집기가 바이너리 저장 가능 ---
    #[test]
    fn write_accepts_arbitrary_bytes() {
        let wt = worktree();
        let bytes = &[0xFFu8, 0x00, 0xFE, 0x42];
        write_file(wt.path(), "raw.bin", bytes, None).unwrap();
        assert_eq!(fs::read(wt.path().join("raw.bin")).unwrap(), bytes);
    }
}
