use crate::runner::GitError;

/// 사용자 입력 ref가 git 옵션으로 해석되는 것(`--force` 등)을 차단한다.
/// base_ref를 git 인자로 넘기는 모든 공개 함수는 이걸 먼저 호출한다.
pub fn validate_user_ref(r: &str) -> Result<(), GitError> {
    if r.is_empty() || r.starts_with('-') {
        return Err(GitError::Parse {
            args: "ref validation".to_string(),
            detail: format!("invalid ref: {r:?}"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_normal_refs() {
        for r in ["main", "origin/main", "feature/x", "v1.0", "HEAD~2"] {
            assert!(validate_user_ref(r).is_ok(), "{r}");
        }
    }

    #[test]
    fn rejects_empty_and_option_like() {
        for r in ["", "-x", "--force", "-"] {
            assert!(validate_user_ref(r).is_err(), "{r:?}");
        }
    }
}
