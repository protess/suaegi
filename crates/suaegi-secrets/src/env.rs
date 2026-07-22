//! env 조회를 **주입 가능**하게 하는 얇은 추상. 단위 테스트가 프로세스 전역 env를 만지지
//! 않도록(포지 크레이트의 `env_lock` 직렬화 함정과 같은 이유) 테스트는 [`MapEnv`]를 쓴다.

use std::collections::HashMap;

/// 이름으로 env 값을 읽는다. 없거나 비-UTF-8이면 `None`.
pub trait EnvLookup {
    fn get(&self, key: &str) -> Option<String>;
}

/// 실제 프로세스 환경. `store`/`load` 공개 함수의 기본 구현이 쓴다.
pub struct ProcessEnv;

impl EnvLookup for ProcessEnv {
    fn get(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }
}

/// 테스트용 인메모리 env. 프로세스 전역 상태를 건드리지 않으므로 병렬 테스트가 안전하다.
#[derive(Default)]
pub struct MapEnv(HashMap<String, String>);

impl MapEnv {
    pub fn new() -> Self {
        Self::default()
    }

    /// 빌더 스타일로 키를 심는다.
    pub fn with(mut self, key: &str, value: &str) -> Self {
        self.0.insert(key.to_string(), value.to_string());
        self
    }
}

impl EnvLookup for MapEnv {
    fn get(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
}
