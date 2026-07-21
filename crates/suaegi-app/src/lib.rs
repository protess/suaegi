pub mod agent_status;
pub mod background;
pub mod diff_panel;
pub mod git_tasks;
pub mod layout;
pub mod persistence_thread;
pub mod presence_poll;
pub mod reaper;
pub mod session_store;
pub mod sidebar;
pub mod state;
pub mod terminal;
pub mod workbench;

use iced::widget::row;
use iced::{Element, Length, Size, Subscription};

pub use state::{AppState, Message, OpId};

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn title(&self) -> String {
        "Suaegi".to_string()
    }
    // The real `update` logic lives on `AppState` in `state.rs` — it dispatches
    // Task 3's git operations, so it needs `&mut self` and the full `Message`
    // match, not a thin wrapper here.
    /// 사이드바 · 워크벤치 · (열려 있으면) diff 패널의 3영역.
    ///
    /// diff 패널은 **기본 닫힘**이고 닫혀 있을 때는 `row!`에 아예 들어가지
    /// 않는다 — 폭 0짜리 컨테이너를 넣어두면 그 자체로 레이아웃 계산에 끼어들고,
    /// 무엇보다 "닫힘"이 위젯 트리에 보이지 않는 편이 상태와 화면이 일치한다.
    pub fn view(&self) -> Element<'_, Message> {
        let mut regions: Vec<Element<'_, Message>> =
            vec![sidebar::view(self), workbench::view(self)];
        if let Some(panel) = diff_panel::view(self.diff()) {
            regions.push(panel);
        }
        row(regions).height(Length::Fill).into()
    }

    /// `workbench::subscription`(세션별 generation 피드)과
    /// `presence_poll::subscription`(티어링된 존재 폴링 타이머)을 하나로
    /// 묶는다. 둘 다 앱 전체에 딱 하나씩만 존재해야 하는 구독이므로
    /// `run()`이 이 함수 하나만 `.subscription(...)`에 건다 — 둘을 따로
    /// 걸면 나중에 셋째가 생겼을 때 배선 지점이 두 곳으로 늘어난다.
    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            workbench::subscription(self),
            presence_poll::subscription(self),
        ])
    }
}

/// 훅 서버의 공유 비밀. 루프백 전용이지만 **같은 기계의 다른 프로세스**가 배지를
/// 위조하는 것은 막아야 하므로 추측 불가능해야 한다.
///
/// `getrandom` 같은 의존을 새로 들이지 않고 OS 엔트로피를 직접 읽는다. 읽지 못하면
/// 시간 기반으로 폴백하되 **그 사실을 알린다** — 조용히 약한 토큰을 쓰지 않는다.
fn new_hook_token() -> String {
    let mut bytes = [0u8; 32];
    match std::fs::File::open("/dev/urandom").and_then(|mut f| {
        use std::io::Read;
        f.read_exact(&mut bytes)
    }) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("suaegi: no OS entropy for the hook token ({e}); falling back to a clock-derived value");
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            for (i, b) in bytes.iter_mut().enumerate() {
                *b = ((nanos >> (i % 16 * 8)) as u8) ^ (i as u8);
            }
        }
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn run() -> iced::Result {
    // **서버가 앱보다 먼저 뜬다.** 세션 스폰이 포트를 알아야 하므로 `boot()`
    // 이전에 바인딩한다. 실패하면 배지 없이 계속 간다 — 치명적이지 않다.
    let hooks = agent_status::server::bind(new_hook_token())
        .map_err(|e| eprintln!("suaegi: hook server did not start: {e} (badges stay Unknown)"))
        .ok();
    let (endpoint, hook_sub) = match hooks {
        Some((server, rx)) => (
            Some((server.port(), server.token().to_string())),
            // **구독은 조건 없이 항상 붙인다.** 조건부로 붙였다 떼면 iced가
            // 레시피를 떨구면서 receiver도 사라지고, 이후 빌더는 `pending`밖에
            // 못 준다. 서버를 살려두는 것도 겸한다 — 떨구면 포트가 닫힌다.
            Some((agent_status::subscription::HookSub::new(1, rx), server)),
        ),
        None => (None, None),
    };

    let boot = move || {
        let (mut state, task) = AppState::boot();
        if let Some((port, token)) = endpoint.clone() {
            state.attach_hook_server(port, token);
        }
        (state, task)
    };

    iced::application(boot, AppState::update, AppState::view)
        .title(AppState::title)
        .subscription(move |state: &AppState| {
            let base = AppState::subscription(state);
            match &hook_sub {
                Some((sub, _server)) => Subscription::batch([base, sub.subscription()]),
                None => base,
            }
        })
        .window_size(Size {
            width: 1280.0,
            height: 800.0,
        })
        .run()
}
