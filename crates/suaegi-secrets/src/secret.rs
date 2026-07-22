//! 토큰을 감싸는 뉴타입. **핵심은 리댁션 규율**: 값은 오직 [`Secret::expose`]로만 나오고,
//! `Debug`/`Display`/`Serialize`는 값을 절대 드러내지 않는다. 이렇게 해서 `format!("{secret:?}")`,
//! 로그, 에러 문자열, JSON 영속화 어디에도 토큰이 새지 않도록 **타입 레벨에서** 강제한다.

use std::fmt;

/// 비밀 값(토큰 등). 값을 보려면 반드시 [`Secret::expose`]를 호출해야 하므로 콜사이트가
/// grep 한 번으로 감사된다. `Debug`는 `Secret(***)`만 찍고, `Display`/`Serialize`는 없다.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    /// 문자열/`String`을 비밀로 감싼다.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// 비밀 값을 드러낸다. **이 이름이 곧 감사 지점** — 호출부를 grep 하면 토큰이 실제로
    /// 어디서 평문으로 쓰이는지 전수 확인할 수 있다. 로그/에러/직렬화에 넣지 말 것.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// 값을 절대 찍지 않는다. `format!("{:?}")`가 토큰을 새지 않게 하는 리댁션 규율의 핵심.
impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

// `Display`는 **의도적으로 구현하지 않는다** — `{}` 포맷으로 토큰이 새는 경로를 원천 차단한다.
// `Serialize`도 없다 — serde_json 영속화(persistence.rs)에 실수로 실릴 수 없다.
// `PartialEq`도 없다 — 파생 비교는 조기 반환하는 타이밍 오라클이 될 수 있어, 비교가 필요한
// 테스트는 `.expose()`로 명시적으로 하게 둔다.

impl Drop for Secret {
    fn drop(&mut self) {
        // 최선 노력(best-effort) 스크럽: 해제되는 힙에 토큰 바이트가 남지 않도록 0으로 덮는다.
        // volatile write라 옵티마이저가 "죽은 저장"으로 지우지 못한다. 완벽하진 않다(이 값이
        // 앞서 clone/realloc 되었다면 그 사본까지는 못 지운다) — 방어적 심층 방어의 한 겹이다.
        let bytes = unsafe { self.0.as_bytes_mut() };
        for b in bytes.iter_mut() {
            unsafe { std::ptr::write_volatile(b, 0) };
        }
        std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
    }
}
