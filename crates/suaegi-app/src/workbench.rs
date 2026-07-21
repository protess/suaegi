//! `pane_grid` 워크벤치 — 세션마다 캐시된 스냅샷을 읽기 전용 단색 모노스페이스
//! 텍스트로 그린다(색/커서/키 입력/포커스/리사이즈를 갖춘 커스텀 위젯은
//! Plan 4). 세션이 하나도 없으면 pane_grid 자체가 없다 — `pane_grid::State`는
//! pane 없이 존재할 수 없어서, "아직 세션 없음"과 "세션이 있지만 비어 있음"을
//! `Option`으로 구분한다.
//!
//! **구독 동일성이 이 파일의 핵심이다.** `Subscription::run`은 `fn` 포인터라
//! 컨텍스트를 캡처할 수 없으므로 `run_with(data, builder)`를 쓴다. `data`의
//! `Hash`가 세션 id **만** 해싱해야 한다 — `Arc<TerminalSession>`을 해싱에
//! 들이면(포인터든, 매 프레임 바뀌는 값이든) 프레임마다 다른 데이터로 보여
//! iced가 구독을 파괴하고 다시 만든다. 스트림이 재시작되는 동안 출력이 오면
//! 그 결과는 새 스트림이 아니라 죽는 스트림에 버려지므로, 터미널이 "가끔
//! 멈칫거리는" 게 아니라 아예 갱신을 멈춘 것처럼 보인다.

use std::sync::Arc;
use std::time::Duration;

use futures::stream::{self, Stream};
use iced::widget::{button, container, pane_grid, scrollable, text};
use iced::{Element, Font, Length, Subscription};

use suaegi_term::session::TerminalSession;

use crate::session_store::SessionId;
use crate::state::{AppState, Message};

/// 스트림을 이 간격으로 페이싱한다. `generation()`을 루프에서 그냥 읽으면
/// executor 워커를 점유한 채 busy-spin 하고, `std::thread::sleep`은 async
/// 워커 스레드를 블로킹한다 — 그래서 `tokio::time::sleep`을 쓴다. 60fps
/// 화면 갱신에 충분하면서도 CPU를 태우지 않는 절충값.
const POLL_INTERVAL: Duration = Duration::from_millis(16);

pub fn view(state: &AppState) -> Element<'_, Message> {
    let Some(panes) = state.panes() else {
        return container(text("Select or create a worktree to start a session"))
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into();
    };

    let grid = pane_grid::PaneGrid::new(panes, |pane, session_id, _is_maximized| {
        let session_id = *session_id;
        let title_bar =
            pane_grid::TitleBar::new(text(state.session_title(session_id).to_string()).size(13))
                .controls(pane_grid::Controls::new(
                    button(text("x").size(12)).on_press(Message::PaneCloseRequested(pane)),
                ))
                .padding(6);

        pane_grid::Content::new(session_body(state, session_id)).title_bar(title_bar)
    })
    // 기본 spacing은 0이고 leeway가 없으면 분할선을 마우스로 잡을 수 없다 —
    // 실제로 리사이즈 가능한 격자가 되려면 둘 다 필요하다.
    .spacing(2)
    .on_click(Message::PaneClicked)
    .on_drag(Message::PaneDragged)
    .on_resize(8, Message::PaneResized)
    .width(Length::Fill)
    .height(Length::Fill);

    container(grid)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn session_body(state: &AppState, id: SessionId) -> Element<'_, Message> {
    let body = state.session_store().snapshot_text(id);
    scrollable(text(body).font(Font::MONOSPACE).size(13))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// `Subscription::run_with`의 `data`. **`session`은 절대 해싱에 참여하지
/// 않는다** — 아래 `Hash` 구현과 하단 테스트가 이걸 지킨다.
#[derive(Clone)]
struct TermFeed {
    id: SessionId,
    session: Arc<TerminalSession>,
}

impl std::hash::Hash for TermFeed {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state); // 오직 id — Arc는 동일성에 참여하지 않는다
    }
}

/// 세션마다 하나씩, `generation()`이 바뀔 때마다 `Message::SessionDirty`를
/// 낸다. 전역 `iced::time::every` 하나로 모든 세션을 훑는 대안은 바쁜
/// 터미널과 유휴 터미널을 같은 주기로 묶으므로 택하지 않았다 — 세션별
/// 구독이라야 각자의 속도로 돈다.
pub fn subscription(state: &AppState) -> Subscription<Message> {
    Subscription::batch(
        state
            .session_store()
            .sessions()
            .map(|(id, session)| Subscription::run_with(TermFeed { id, session }, feed_stream)),
    )
}

fn feed_stream(feed: &TermFeed) -> impl Stream<Item = Message> {
    let id = feed.id;
    let session = Arc::clone(&feed.session);
    stream::unfold(session.generation(), move |last_seen| {
        let session = Arc::clone(&session);
        async move {
            loop {
                tokio::time::sleep(POLL_INTERVAL).await;
                let current = session.generation();
                if current != last_seen {
                    return Some((
                        Message::SessionDirty {
                            id,
                            generation: current,
                        },
                        current,
                    ));
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::{Hash, Hasher};

    use crate::session_store::SessionStore;

    fn start_throwaway_session() -> Arc<TerminalSession> {
        Arc::new(SessionStore::spawn_throwaway_for_test())
    }

    /// 해시 입력 바이트를 그대로 기록한다. "우연히 같은 u64"가 아니라
    /// "무엇을 해싱했는지"를 직접 본다.
    #[derive(Default)]
    struct RecordingHasher(Vec<u8>);
    impl Hasher for RecordingHasher {
        fn write(&mut self, bytes: &[u8]) {
            self.0.extend_from_slice(bytes);
        }
        fn finish(&self) -> u64 {
            0
        }
    }
    fn recorded<T: Hash>(v: &T) -> Vec<u8> {
        let mut h = RecordingHasher::default();
        v.hash(&mut h);
        h.0
    }

    #[test]
    fn feed_identity_is_exactly_the_session_id_and_nothing_else() {
        // 서로 다른 세션 객체를 같은 id로 감쌌을 때 같아야 한다.
        // 같은 Arc의 클론 둘로 비교하면 포인터를 해싱해도 통과해버린다.
        let a = TermFeed {
            id: SessionId(7),
            session: start_throwaway_session(),
        };
        let b = TermFeed {
            id: SessionId(7),
            session: start_throwaway_session(),
        };
        assert_eq!(recorded(&a), recorded(&b));
        assert_eq!(recorded(&a), recorded(&7u64), "only the id may be hashed");
    }

    #[test]
    fn different_sessions_have_different_identity() {
        let a = TermFeed {
            id: SessionId(7),
            session: start_throwaway_session(),
        };
        let b = TermFeed {
            id: SessionId(8),
            session: start_throwaway_session(),
        };
        assert_ne!(recorded(&a), recorded(&b));
    }
}
