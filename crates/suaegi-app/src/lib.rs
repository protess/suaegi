pub mod agent_status;
pub mod background;
pub mod diff_panel;
pub mod external_editor;
pub mod forge_tasks;
pub mod forge_ui;
pub mod git_tasks;
pub mod layout;
pub mod persistence_thread;
pub mod pr_panel;
pub mod presence_poll;
pub mod prompt_inject;
pub mod reaper;
pub mod session_store;
pub mod sidebar;
pub mod state;
pub mod terminal;
pub mod tracker_tasks;
pub mod tracker_ui;
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
        // PR 패널도 diff 패널과 같이 **열렸을 때만** `row!`에 들어간다.
        if let Some(panel) = pr_panel::view(self.pr_panel()) {
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
/// **엔트로피를 못 얻으면 `None`을 돌려주고 훅 기능 전체를 끈다.** 시계에서
/// 유도한 값은 같은 기계의 프로세스가 근사할 수 있으므로 토큰이 아니다 —
/// 그것을 로그로 알리는 것은 안전하게 만들지 못한다. 바인딩 실패와 **똑같이**
/// 다룬다: 배지 없이 계속 간다.
fn new_hook_token() -> Option<String> {
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| {
            use std::io::Read;
            f.read_exact(&mut bytes)
        })
        .map_err(|e| {
            eprintln!(
                "suaegi: no OS entropy for the hook token ({e}); \
                 agent badges are disabled (a clock-derived token is guessable)"
            )
        })
        .ok()?;
    Some(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

pub fn run() -> iced::Result {
    // **서버가 앱보다 먼저 뜬다.** 세션 스폰이 포트를 알아야 하므로 `boot()`
    // 이전에 바인딩한다. 실패하면 배지 없이 계속 간다 — 치명적이지 않다.
    let hooks = new_hook_token().and_then(|token| {
        agent_status::server::bind(token)
            .map_err(|e| eprintln!("suaegi: hook server did not start: {e} (badges stay Unknown)"))
            .ok()
    });
    // **서버 핸들은 `AppState`가 가져간다.** 떨구면 포트가 닫히고, 버린 이벤트
    // 카운터를 읽을 곳도 거기뿐이다. 여기 남는 것은 구독 레시피뿐이다.
    let (server, hook_sub) = match hooks {
        Some((server, rx)) => (
            Some(server),
            Some(agent_status::subscription::HookSub::new(1, rx)),
        ),
        None => (None, None),
    };

    // `iced::application`은 부트 클로저를 여러 번 부르지 않지만 `Fn`을 요구하므로
    // 한 번만 꺼낼 수 있는 자리에 담아 옮긴다.
    let server = std::cell::RefCell::new(server);
    // **서버를 `boot`에 넘긴다.** 복원이 시작하는 세션도 스폰 시점에 포트를
    // 알아야 하므로, 붙이는 시점이 `begin_layout_restore()`보다 늦으면 재시작
    // 직후의 모든 pane이 훅 없이 뜬다.
    let boot = move || AppState::boot(server.borrow_mut().take());

    iced::application(boot, AppState::update, AppState::view)
        .title(AppState::title)
        .subscription(move |state: &AppState| {
            let base = AppState::subscription(state);
            match &hook_sub {
                Some(sub) => Subscription::batch([base, sub.subscription()]),
                None => base,
            }
        })
        .window_size(Size {
            width: 1280.0,
            height: 800.0,
        })
        .run()
}
