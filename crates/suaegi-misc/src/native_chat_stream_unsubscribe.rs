//! Native-chat stream unsubscribe RPC — verbatim port of Orca's
//! `src/shared/native-chat-stream-unsubscribe.ts` (@ v1.4.146-rc.0).
//!
//! Why (upstream comment, `:1-6`): the runtime keys a native-chat transcript
//! fs-watcher by the cleanup token `agent:sessionId`. A subscribing client
//! must echo that exact token on `nativeChat.unsubscribe` so the watcher is
//! closed when the chat view toggles off (not just on socket close) —
//! otherwise watchers leak per session-switch. Both the web runtime client
//! and mobile use this single key shape; centralizing it here keeps the
//! token from drifting between the two surfaces.
//!
//! # ⚠⚠ P6 — unguarded, non-injective, and this is the *inverse* of
//! `agent_notification_id`
//! [`build_native_chat_subscription_id`] is `format!("{agent}:{session_id}")`
//! (`:15` upstream) — no escaping, no rejection of a `:` in either field.
//! Non-injective: `("a", "b:c")` and `("a:b", "c")` both produce `"a:b:c"`.
//! Unlike [`crate::agent_notification_id::build_agent_notification_id`],
//! where `encodeURIComponent` is load-bearing because the id is later split
//! back apart client-side, **do not** add `encode_uri_component` here — the
//! server re-composes this exact token inline and unencoded
//! (`native-chat.ts:216` upstream) to look up the watcher. An encoded token
//! would never match the server's unencoded key, so "fixing" the collision
//! would make every unsubscribe silently fail to find its watcher, leaking
//! it for the life of the connection. Ported verbatim, with the collision
//! pinned directly the way
//! [`crate::ephemeral_setup_terminal_worktree_id`] pins its own
//! ported-unchanged upstream hazard. (In production the first field comes
//! from a colon-free agent-id set, so the collision does not fire in
//! practice — but that safety lives in a different module and is not
//! visible from here; both parameters here are bare `&str`.)
//!
//! # ⚠ P7 — the subscription-id fallback is nullish (`??`), not truthy
//! (`||`)
//! Upstream (`:26`) is `subscriptionId ?? buildNativeChatSubscriptionId(...)`.
//! The oracle's fixtures only ever supply `subscriptionId` as absent or as
//! the truthy `'pane-2'`, so `??` and `||` agree on every fixture — the only
//! input that tells them apart is `''`. The server branches on truthiness
//! when deciding whether a subscription id is targeted or connection-wide
//! (`native-chat.ts:292` upstream), so folding `Some("")` to `None` here (or
//! swapping in `||`-style truthy-fallback semantics) would turn a targeted
//! single-watcher teardown into a connection-wide mass teardown. Modeled as
//! `subscription_id: Option<&str>` and `Some("")` is used as-is — it is
//! *not* replaced by the composed id. Only `None` (nothing supplied at all)
//! triggers the fallback.
//!
//! # P8 — the oracle asserts literal ids, not just round-trips
//! The oracle fixes `'claude:sess-1'`, `'codex:abc'`, and the method string
//! literally — the separator, field order, and method name are already
//! pinned by the oracle itself (a rare case where the port isn't the first
//! to constrain the literal shape). What the oracle does *not* pin:
//! escaping (P6), the `''` case (P7), and the collision (P6) — those are
//! this module's added pins.
//!
//! # P9 — the RPC frame has exactly two logical fields
//! Upstream `NativeChatUnsubscribeRpc` is `{ method: 'nativeChat.unsubscribe',
//! params: { subscriptionId: string } }` — a literal method type, no other
//! params, no optional fields. Modeled here as a flat
//! [`NativeChatUnsubscribeRpc`] struct (`method` + `subscription_id`) rather
//! than a nested `params` struct, since Rust has no anonymous-object
//! literal-type equivalent to gain from the extra nesting level; `method`
//! is pinned both as the struct's runtime value and as the standalone
//! [`NATIVE_CHAT_UNSUBSCRIBE_METHOD`] constant.

/// The `nativeChat.unsubscribe` RPC method name (`:9` upstream, a literal
/// type there). Pinned as a constant so a typo in the method string is
/// caught directly (P9).
pub const NATIVE_CHAT_UNSUBSCRIBE_METHOD: &str = "nativeChat.unsubscribe";

/// The unsubscribe RPC frame a client sends on teardown to close the
/// transcript watcher. Flattened relative to upstream's nested
/// `{ params: { subscriptionId } }` (P9) — `method` is always
/// [`NATIVE_CHAT_UNSUBSCRIBE_METHOD`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeChatUnsubscribeRpc {
    pub method: &'static str,
    pub subscription_id: String,
}

/// The cleanup token the server keys the transcript watcher under:
/// `format!("{agent}:{session_id}")` (`:15` upstream). Unguarded and
/// non-injective — a `:` in either field is not escaped or rejected (P6).
/// Do not add `encode_uri_component`: the server re-composes this same
/// unencoded token to look the watcher up (P6).
pub fn build_native_chat_subscription_id(agent: &str, session_id: &str) -> String {
    format!("{agent}:{session_id}")
}

/// The unsubscribe RPC frame. `subscription_id` is the nullish (`??`, not
/// `||`) fallback slot (P7): `None` composes the id via
/// [`build_native_chat_subscription_id`], but `Some("")` is used verbatim —
/// it is never folded to the composed id.
pub fn build_native_chat_unsubscribe(
    agent: &str,
    session_id: &str,
    subscription_id: Option<&str>,
) -> NativeChatUnsubscribeRpc {
    NativeChatUnsubscribeRpc {
        method: NATIVE_CHAT_UNSUBSCRIBE_METHOD,
        subscription_id: match subscription_id {
            Some(id) => id.to_string(),
            None => build_native_chat_subscription_id(agent, session_id),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_native_chat_subscription_id, build_native_chat_unsubscribe,
        NativeChatUnsubscribeRpc, NATIVE_CHAT_UNSUBSCRIBE_METHOD,
    };

    // Oracle: native-chat-stream-unsubscribe.test.ts

    #[test]
    fn derives_the_cleanup_token_as_agent_colon_session_id() {
        assert_eq!(
            build_native_chat_subscription_id("claude", "sess-1"),
            "claude:sess-1"
        );
    }

    #[test]
    fn builds_the_unsubscribe_rpc_frame_mobile_and_web_share() {
        assert_eq!(
            build_native_chat_unsubscribe("codex", "abc", None),
            NativeChatUnsubscribeRpc {
                method: "nativeChat.unsubscribe",
                subscription_id: "codex:abc".to_string(),
            }
        );
    }

    #[test]
    fn echoes_a_pane_specific_token_when_the_subscriber_supplied_one() {
        assert_eq!(
            build_native_chat_unsubscribe("claude", "same-session", Some("pane-2")),
            NativeChatUnsubscribeRpc {
                method: "nativeChat.unsubscribe",
                subscription_id: "pane-2".to_string(),
            }
        );
    }

    // Mandatory extra pins (oracle-silent — plan §3, P6/P7/P9):

    /// P6 crux pin: the id builder is non-injective — two distinct
    /// `(agent, session_id)` pairs collide on the same composed token
    /// because the `:` separator is not escaped in either field.
    #[test]
    fn pin_unescaped_colon_causes_a_genuine_collision() {
        let id_1 = build_native_chat_subscription_id("a", "b:c");
        let id_2 = build_native_chat_subscription_id("a:b", "c");
        assert_eq!(id_1, "a:b:c");
        assert_eq!(id_2, "a:b:c");
        assert_eq!(id_1, id_2);
    }

    /// P6: no percent-encoding (or any other escaping) is applied — colons,
    /// spaces, and non-ASCII characters in either field pass through
    /// verbatim, matching the server's unencoded re-composition.
    #[test]
    fn pin_no_encoding_is_applied_to_either_field() {
        assert_eq!(
            build_native_chat_subscription_id("agent name", "session:1"),
            "agent name:session:1"
        );
        assert_eq!(
            build_native_chat_subscription_id("claude", "sess-\u{00e9}"),
            "claude:sess-\u{00e9}"
        );
    }

    /// P7 crux pin: `Some("")` is used as-is — it is NOT folded to `None`
    /// and does NOT fall back to the composed subscription id. Only `None`
    /// triggers the fallback; this is nullish (`??`) semantics, not
    /// truthy (`||`) semantics.
    #[test]
    fn pin_empty_string_subscription_id_is_kept_verbatim_not_folded_to_composed_id() {
        let rpc = build_native_chat_unsubscribe("claude", "sess-1", Some(""));
        assert_eq!(rpc.subscription_id, "");
        assert_ne!(rpc.subscription_id, "claude:sess-1");
    }

    /// P7: `None` (nothing supplied at all) is the only input that triggers
    /// the composed-id fallback.
    #[test]
    fn pin_none_subscription_id_falls_back_to_composed_id() {
        let rpc = build_native_chat_unsubscribe("claude", "sess-1", None);
        assert_eq!(rpc.subscription_id, "claude:sess-1");
    }

    /// P9 crux pin: the method string literal, independent of the oracle's
    /// `toEqual` frame assertions above.
    #[test]
    fn pin_unsubscribe_method_literal() {
        assert_eq!(NATIVE_CHAT_UNSUBSCRIBE_METHOD, "nativeChat.unsubscribe");
    }
}
