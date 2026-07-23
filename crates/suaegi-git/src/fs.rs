//! 워크트리 안의 파일시스템 리스팅과 안전 read. 모든 진입점은 M1 경로 안전 코어
//! (`compare.rs`의 `resolve_for_read`)를 **먼저** 통과한다 — traversal/심링크-escape/
//! null-byte를 거기서 거른다. 이 모듈은 그 위에서 디렉터리 한 레벨을 읽고(M2) 파일
//! 하나를 안전하게 읽는다(M4).

use crate::compare::{resolve_for_read, Resolved, BINARY_SNIFF_BYTES};
use std::io;
use std::path::Path;

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

#[cfg(all(test, unix))]
mod tests {
    //! 실제 파일/디렉터리/심링크를 `tempdir`에 만들어 검증한다(모킹 금지). 각 crux
    //! 테스트는 하나의 mutant를 죽이도록 설계됐다.
    use super::{list_dir, read_file, read_file_with_cap, FileContent, FileRead};
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
}
