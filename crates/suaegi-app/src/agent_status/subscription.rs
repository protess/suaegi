//! 훅 채널을 iced 구독으로 잇는다(0.4).
//!
//! **왜 공유 홀더가 필요한가**: `Subscription::run_with`의 빌더는
//! `fn(&D) -> S`다(`iced_futures-0.14.0/src/subscription.rs:198`). 평범한 함수
//! 포인터이고 `&D`만 받으므로 **`D`에서 receiver를 꺼낼 수 없다.** receiver는
//! 복제할 수 없으니 `D`에 그냥 담아둘 수도 없다. 그래서
//! `Arc<Mutex<Option<Receiver>>>`에 넣고 빌더가 **한 번만 꺼내 간다.**
//!
//! **`Hash`가 불변 `id`만 보아야 한다.** iced는 레시피의 해시로 구독의 정체성을
//! 판단한다 — 정체성이 바뀌면 옛 스트림을 떨구고 빌더를 다시 부르는데, 그때
//! `slot`은 이미 비어 있어 **`pending`밖에 못 준다**(= 배지가 영영 멎는다).
//! `slot`의 내용은 시간에 따라 변하므로 해시에 **절대** 들어가면 안 된다.
//!
//! 같은 이유로 **훅 구독을 조건부로 붙였다 뗐다 하지 않는다.** 한 번 떼면
//! 다시 붙여도 receiver가 없다.

use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use futures::channel::mpsc;
use futures::stream::{BoxStream, StreamExt};
use iced::Subscription;

use crate::agent_status::contract::HookEvent;
use crate::state::Message;

/// 훅 구독의 레시피 데이터. **복제해도 같은 `slot`을 가리킨다** — iced가 빌더를
/// 다시 부르더라도 receiver는 하나뿐이다.
#[derive(Clone)]
pub struct HookSub {
    /// 앱 수명 내내 **바뀌지 않는** 정체성. 해시에 들어가는 유일한 값이다.
    id: u64,
    slot: Arc<Mutex<Option<mpsc::Receiver<HookEvent>>>>,
}

impl Hash for HookSub {
    /// **`id`만 해시한다.** `slot`은 정체성이 아니다 — 위 모듈 주석 참고.
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl std::fmt::Debug for HookSub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // receiver는 Debug가 아니고, 있든 없든 진단에 필요한 것은 "남아 있나"뿐이다.
        let taken = self.slot.lock().map(|s| s.is_none()).unwrap_or(true);
        f.debug_struct("HookSub")
            .field("id", &self.id)
            .field("receiver_taken", &taken)
            .finish()
    }
}

impl HookSub {
    /// `server::bind`가 돌려준 receiver를 감싼다. **앱당 한 번만 만든다.**
    pub fn new(id: u64, rx: mpsc::Receiver<HookEvent>) -> Self {
        Self {
            id,
            slot: Arc::new(Mutex::new(Some(rx))),
        }
    }

    /// `AppState::subscription()`에서 부른다. **조건 없이 항상 부른다** —
    /// 조건부로 붙였다 떼면 두 번째부터는 `pending`이다.
    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::run_with(self.clone(), build).map(Message::HookArrived)
    }
}

/// `fn` 포인터여야 하므로 클로저를 쓸 수 없다(`run_with`의 시그니처).
///
/// **두 갈래가 같은 타입이어야** 해서 양쪽을 `BoxStream`으로 맞춘다. 두 번째
/// 호출부터는 `pending`이다 — 스트림이 끝나면(`None`) iced가 구독을 완료로 보고
/// 정리하는데, 그러면 뒤늦게 정체성이 재평가될 때 되살릴 방법이 없다. 영원히
/// 멈춰 있는 편이 조용히 사라지는 것보다 낫다.
fn build(data: &HookSub) -> BoxStream<'static, HookEvent> {
    // **락 poisoning에도 패닉하지 않는다.** 구독 빌더에서 패닉하면 UI가 죽는다.
    match data.slot.lock() {
        Ok(mut slot) => match slot.take() {
            Some(rx) => rx.boxed(),
            None => futures::stream::pending().boxed(),
        },
        Err(_) => futures::stream::pending().boxed(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::hash_map::DefaultHasher;

    fn hash_of(sub: &HookSub) -> u64 {
        let mut h = DefaultHasher::new();
        sub.hash(&mut h);
        h.finish()
    }

    fn sub_with(id: u64) -> HookSub {
        let (_tx, rx) = mpsc::channel(4);
        HookSub::new(id, rx)
    }

    /// **정체성은 `id`뿐이다.** receiver를 꺼내 `slot`이 비어도 해시가 그대로여야
    /// iced가 레시피를 떨구지 않는다 — 떨구면 되살릴 수 없다.
    #[test]
    fn hash_ignores_the_slot_contents() {
        let sub = sub_with(7);
        let before = hash_of(&sub);

        let taken = build(&sub);
        drop(taken);
        let after = hash_of(&sub);

        assert_eq!(
            before, after,
            "receiver를 꺼냈다고 해시가 바뀌면 iced가 구독을 떨구고, 그 뒤로는 \
             pending밖에 못 준다"
        );
    }

    /// 서로 다른 구독은 서로 다른 정체성을 가져야 한다 — 그렇지 않으면 iced가
    /// 둘을 같은 것으로 보고 하나만 살린다.
    #[test]
    fn different_ids_hash_differently() {
        assert_ne!(hash_of(&sub_with(1)), hash_of(&sub_with(2)));
    }

    /// **복제본은 같은 receiver를 공유한다.** iced가 데이터를 복제해도 스트림이
    /// 둘로 늘어나면 안 된다.
    ///
    /// **송신자를 살려둔 채로 검사해야 한다** — `sub_with`처럼 송신자를 떨구면
    /// 첫 스트림이 즉시 **완료**되어(`None`) "막혔다"와 구별되지 않는다.
    /// (첫 판에서 실제로 이 실수를 했다.)
    #[test]
    fn receiver_is_handed_out_exactly_once_across_clones() {
        let (mut tx, rx) = mpsc::channel(4);
        let sub = HookSub::new(1, rx);
        let clone = sub.clone();

        let mut first = build(&sub);
        let mut second = build(&clone);

        // 진짜 receiver를 쥔 쪽은 **첫 번째**다: 보낸 것이 거기로 나온다.
        //
        // **`block_on(next())`을 쓰지 않는다.** receiver를 아예 넘겨주지 않는
        // 구현에서는 그게 영원히 멈춰 테스트가 실패 대신 **행**이 된다(mutation으로
        // 실제로 겪었다 — CI 타임아웃을 통째로 먹는다). 이미 보낸 값이라 즉시
        // 준비돼 있어야 하므로 `poll_immediate`로 충분하고, 아니면 바로 실패한다.
        tx.try_send(event()).expect("send");
        let got = futures::executor::block_on(futures::future::poll_immediate(first.next()));
        assert!(
            matches!(got, Some(Some(_))),
            "첫 build가 진짜 receiver를 못 받았다 — 이벤트가 도착하지 않는다: {got:?}"
        );

        // 복제본에서 부른 두 번째는 pending이다. 값을 내지도, 끝나지도 않는다.
        assert!(
            futures::executor::block_on(futures::future::poll_immediate(second.next())).is_none(),
            "두 번째 build가 pending이 아니다 — receiver가 두 번 나갔거나 스트림이 끝났다"
        );
    }

    fn event() -> HookEvent {
        use crate::agent_status::contract::{HookEventName, PaneKey, SpawnNonce};
        use suaegi_core::domain::WorktreeId;
        HookEvent {
            pane_key: PaneKey(WorktreeId("/w".into())),
            spawn_nonce: SpawnNonce(1),
            claude_session_id: "s".into(),
            event: HookEventName::Stop,
            tool_name: None,
            agent_id: None,
            background_tasks_empty: Some(true),
        }
    }

    /// 끝난 스트림은 **완료**이지 pending이 아니다. 둘을 구별해 두지 않으면
    /// 위 테스트가 "끝났다"를 "막혔다"로 착각한다.
    #[test]
    fn a_dropped_sender_ends_the_first_stream_but_not_the_fallback() {
        let (tx, rx) = mpsc::channel(4);
        let sub = HookSub::new(3, rx);
        let mut stream = build(&sub);
        drop(tx);
        // 여기도 `poll_immediate`다. 진짜 receiver라면 송신자가 사라진 순간
        // **즉시** `Ready(None)`이라 기다릴 이유가 없고, 아니라면 `block_on`이
        // 영원히 멈춰 실패 대신 행이 된다(위 테스트와 같은 이유).
        let ended = futures::executor::block_on(futures::future::poll_immediate(stream.next()));
        assert_eq!(
            ended,
            Some(None),
            "송신자를 떨구면 첫 스트림은 즉시 끝나야 한다 (Some(None) = 완료)"
        );

        // 반면 fallback은 **영원히 pending**이다 — 끝나지 않는다.
        let mut fallback = build(&sub);
        assert!(
            futures::executor::block_on(futures::future::poll_immediate(fallback.next())).is_none(),
            "fallback이 즉시 완료되면 iced가 구독을 정리해 버린다"
        );
    }
}
