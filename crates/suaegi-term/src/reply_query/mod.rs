//! Transport-layer terminal query/reply/color escape scanners, ported from
//! Orca's `shared/terminal-{osc-color-reply,query-reply,reply-query-extraction,
//! reply-query-scan}.ts` (@ v1.4.150-rc.0).
//!
//! These are byte-native (`&[u8]`) scanners for the remote/daemon transport
//! path — they salvage and answer terminal queries out of raw output that may
//! be buffered, hidden, or dropped without passing through the alacritty
//! emulator. They are DISTINCT from the local `grid.rs` emulator path (which
//! already generates CSI-query replies via alacritty) and from the outbound
//! `encode.rs`. Wiring into a daemon output boundary + its byte-sequence
//! counter is deferred (plan8). See
//! `docs/superpowers/plans/2026-07-25-terminal-query-reply.md`.

pub mod osc_color_reply;
pub mod query_reply;
pub mod reply_query_extraction;
pub mod reply_query_scan;

pub use osc_color_reply::{
    css_color_to_osc_rgb, parse_terminal_osc_color_query, send_terminal_osc_color_query_replies,
    terminal_osc_color_query_replies, terminal_osc_color_query_reply,
    terminal_osc_color_query_slots_for_body, TerminalOscColorQueryParseResult,
    TerminalOscColorQueryReplyColors, TerminalOscColorQuerySlot,
};
pub use query_reply::is_terminal_query_reply;
pub use reply_query_extraction::{
    contains_csi_renderer_query, contains_stateful_renderer_query,
    extract_hidden_startup_renderer_query_data, find_csi_final_byte_index,
    is_stateful_renderer_reply_csi_query, is_stateless_renderer_reply_csi_query,
    ExtractedRendererQueryData, HIDDEN_STARTUP_RENDERER_QUERY_PENDING_CHARS,
};
pub use reply_query_scan::{
    scan_terminal_reply_query_sequences, ScanResult, TerminalReplyQueryScanState,
    TerminalReplyQuerySequence, EMPTY_TERMINAL_REPLY_QUERY_SCAN_STATE,
};
