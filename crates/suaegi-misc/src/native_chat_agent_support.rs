//! Native-chat agent support gating — verbatim port of Orca's
//! `src/shared/native-chat-agent-support.ts` (@ v1.4.146-rc.0).
//!
//! # ⚠⚠ P1 — the set literal is asserted nowhere upstream, and the near-miss
//! id space makes a wrong subset/superset a real hazard
//! `NATIVE_CHAT_SUPPORTED_AGENTS`'s four members are never asserted as a
//! literal anywhere in the upstream repo — the constant is only defined,
//! self-used, and re-exported. Worse, the upstream oracle only ever passes
//! `'claude'` and `'openclaude'` to [`is_native_chat_supported_agent`];
//! `'codex'` and `'grok'` are never exercised by that predicate's own test.
//! So a two-member subset `{claude, openclaude}` passes the oracle, and so
//! does a superset. This is not academic: `agent-catalog.tsx` has 34 agent
//! ids, and three are near-misses of this exact set — `'openclaw'`
//! (`:288`) is one character from `'openclaude'`, `'claude-agent-teams'`
//! (`:54`) prefix-matches `'claude'`, and `'opencode'` (`:88`) is an
//! anagram-adjacent near-miss too. A `startsWith('claude')`
//! reimplementation would ship green. Pin all four members, the full set
//! literal, and rejection of all three near-misses directly.
//!
//! # ⚠ P2 — two independent mechanisms, identical accept-domains, no upstream
//! link
//! [`is_native_chat_supported_agent`] is `Set`/array membership (`:12`
//! upstream); [`resolve_native_chat_transcript_agent`] is a 4-literal `===`
//! chain (`:27,:30` upstream) that never references
//! `NATIVE_CHAT_SUPPORTED_AGENTS` at all. Both mechanisms accept exactly the
//! same four strings today, so `is(a) == resolve(a).is_some()` holds for
//! every input — reimplementing either one in terms of the other produces
//! zero behavioral difference and zero test signal. They are kept as two
//! separate mechanisms anyway: the set is a "transcript-parseable" gate, the
//! resolver is a "which format" mapping, and they are meant to be extended
//! independently (e.g. a future agent could join the set without having a
//! distinct transcript format, or vice versa). Do not delete either
//! mechanism as a "simplification". An agreement pin below checks the two
//! stay in sync on all four members plus a negative — note per the module's
//! own analysis this pin cannot observe a same-behavior reimplementation
//! (see the mutation-verify report for this exact case).
//!
//! # P3 — `should_step_native_chat_ask_answer` gates via the resolver, not
//! the raw agent string
//! Upstream (`:19`) is `resolveNativeChatTranscriptAgent(agent) === 'claude'`
//! — not `agent === 'claude'`. The consumer
//! (`use-native-chat-interactive-send.ts:97-98`) states the intent
//! explicitly: gating on the *transcript* agent rather than the literal
//! agent string lets OpenClaude take the same keystroke-pacing path as
//! Claude, since it writes the same transcript format. A set-based
//! reimplementation (`SUPPORTED.has(a) && a != codex && a != grok`) passes
//! today's oracle too, but silently inherits any future set member as a
//! stepping agent — pin `'openclaude'` -> `true` and `'codex'`/`'grok'` ->
//! `false` directly, not just via the oracle's coverage of them.
//!
//! # P4 — matching is exact, case-sensitive, untrimmed
//! No `.trim()`, no `.toLowerCase()`, no regex anywhere in the source. Do
//! not add `js_ws`/`js_trim` here. `'Claude'`, `' claude '`, and `''` must
//! all be rejected by every one of the three functions.
//!
//! # P5 — `resolve`'s return type excludes `'openclaude'` as a variant
//! `'openclaude'` is a valid *input* but maps to the
//! [`NativeChatTranscriptAgent::Claude`] variant on output (`:24,:27-28`
//! upstream) — it is not itself a member of the return union. Modeled here
//! as `Option<NativeChatTranscriptAgent>` with no `OpenClaude` variant,
//! matching the upstream `NativeChatTranscriptAgent` type alias exactly.

/// The transcript format an agent's native-chat output is parsed as.
/// `'openclaude'` is not a variant here (P5) — it resolves onto
/// [`NativeChatTranscriptAgent::Claude`]. Variant order is not part of the
/// contract: upstream never enumerates or serializes this type in an
/// order-observable way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeChatTranscriptAgent {
    Claude,
    Codex,
    Grok,
}

impl NativeChatTranscriptAgent {
    pub fn as_str(self) -> &'static str {
        match self {
            NativeChatTranscriptAgent::Claude => "claude",
            NativeChatTranscriptAgent::Codex => "codex",
            NativeChatTranscriptAgent::Grok => "grok",
        }
    }
}

/// Agents whose transcripts the native chat view can parse and render
/// (`:4-9` upstream). Exactly four members (P1) — a `Set<string>` upstream,
/// modeled here as a fixed-size array since membership order is not
/// observable and a `Set` adds nothing over exact-string containment.
pub const NATIVE_CHAT_SUPPORTED_AGENTS: [&str; 4] = ["claude", "openclaude", "codex", "grok"];

/// `agent != null && NATIVE_CHAT_SUPPORTED_AGENTS.has(agent)` (`:12`
/// upstream). Exact, case-sensitive, untrimmed (P4) — set membership, kept
/// independent of [`resolve_native_chat_transcript_agent`] (P2).
pub fn is_native_chat_supported_agent(agent: Option<&str>) -> bool {
    match agent {
        Some(a) => NATIVE_CHAT_SUPPORTED_AGENTS.contains(&a),
        None => false,
    }
}

/// True when the agent renders Claude's multi-step `AskUserQuestion` — one
/// question per step, each Enter advancing — so a multi-line answer must be
/// paced per line. Other agents submit the whole answer with a single
/// Enter. Gates via [`resolve_native_chat_transcript_agent`], not the raw
/// agent string (P3): `'openclaude'` steps because it resolves onto
/// `Claude`, not because it equals `'claude'`.
pub fn should_step_native_chat_ask_answer(agent: Option<&str>) -> bool {
    matches!(
        resolve_native_chat_transcript_agent(agent),
        Some(NativeChatTranscriptAgent::Claude)
    )
}

/// `'claude'`/`'openclaude'` -> `Claude`, `'codex'` -> `Codex`, `'grok'` ->
/// `Grok`, anything else (including `None`) -> `None` (`:22-34` upstream).
/// A 4-literal match chain, independent of
/// [`NATIVE_CHAT_SUPPORTED_AGENTS`]/[`is_native_chat_supported_agent`] (P2).
/// Why (upstream comment, `:25-26`): OpenClaude writes the Claude transcript
/// format and layout even though Orca preserves its distinct agent identity
/// for launch and UI behavior.
pub fn resolve_native_chat_transcript_agent(
    agent: Option<&str>,
) -> Option<NativeChatTranscriptAgent> {
    match agent {
        Some("claude") | Some("openclaude") => Some(NativeChatTranscriptAgent::Claude),
        Some("codex") => Some(NativeChatTranscriptAgent::Codex),
        Some("grok") => Some(NativeChatTranscriptAgent::Grok),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        is_native_chat_supported_agent, resolve_native_chat_transcript_agent,
        should_step_native_chat_ask_answer, NativeChatTranscriptAgent, NATIVE_CHAT_SUPPORTED_AGENTS,
    };

    // Oracle: native-chat-agent-support.test.ts

    #[test]
    fn maps_openclaude_onto_the_claude_transcript_format() {
        assert_eq!(
            resolve_native_chat_transcript_agent(Some("openclaude")),
            Some(NativeChatTranscriptAgent::Claude)
        );
        assert_eq!(
            resolve_native_chat_transcript_agent(Some("claude")),
            Some(NativeChatTranscriptAgent::Claude)
        );
    }

    #[test]
    fn passes_codex_and_grok_through_and_rejects_everything_else() {
        assert_eq!(
            resolve_native_chat_transcript_agent(Some("codex")),
            Some(NativeChatTranscriptAgent::Codex)
        );
        assert_eq!(
            resolve_native_chat_transcript_agent(Some("grok")),
            Some(NativeChatTranscriptAgent::Grok)
        );
        assert_eq!(resolve_native_chat_transcript_agent(Some("cursor")), None);
        assert_eq!(resolve_native_chat_transcript_agent(None), None);
    }

    #[test]
    fn recognizes_the_parseable_agents_and_rejects_unknown_nullish_input() {
        assert!(is_native_chat_supported_agent(Some("claude")));
        assert!(is_native_chat_supported_agent(Some("openclaude")));
        assert!(!is_native_chat_supported_agent(Some("cursor")));
        assert!(!is_native_chat_supported_agent(None));
    }

    #[test]
    fn steps_only_the_claude_format_agents_claude_openclaude() {
        assert!(should_step_native_chat_ask_answer(Some("claude")));
        assert!(should_step_native_chat_ask_answer(Some("openclaude")));
    }

    #[test]
    fn does_not_step_other_or_unknown_agents() {
        assert!(!should_step_native_chat_ask_answer(Some("codex")));
        assert!(!should_step_native_chat_ask_answer(Some("grok")));
        assert!(!should_step_native_chat_ask_answer(Some("cursor")));
        assert!(!should_step_native_chat_ask_answer(None));
    }

    // Mandatory extra pins (oracle-silent — plan §3, P1/P2/P3/P4/P5):

    /// P1 crux pin: the full set literal, including element count. The
    /// upstream oracle never asserts this — only two of the four members are
    /// ever passed to `is_native_chat_supported_agent`.
    #[test]
    fn pin_supported_agents_set_literal() {
        assert_eq!(
            NATIVE_CHAT_SUPPORTED_AGENTS,
            ["claude", "openclaude", "codex", "grok"]
        );
    }

    /// P1 crux pin: each member individually accepted by the predicate,
    /// including `'codex'`/`'grok'` which the upstream oracle never
    /// exercises through `is_native_chat_supported_agent` at all.
    #[test]
    fn pin_each_supported_member_is_accepted() {
        for member in NATIVE_CHAT_SUPPORTED_AGENTS {
            assert!(
                is_native_chat_supported_agent(Some(member)),
                "expected {member} to be accepted"
            );
        }
    }

    /// P1 crux pin: near-miss agent ids from `agent-catalog.tsx`'s 34-id
    /// space are rejected, not silently swept in by a looser matcher like
    /// `startsWith('claude')` or `contains('open')`.
    #[test]
    fn pin_near_miss_agent_ids_are_rejected() {
        assert!(!is_native_chat_supported_agent(Some("openclaw")));
        assert!(!is_native_chat_supported_agent(Some("claude-agent-teams")));
        assert!(!is_native_chat_supported_agent(Some("opencode")));
        assert_eq!(resolve_native_chat_transcript_agent(Some("openclaw")), None);
        assert_eq!(
            resolve_native_chat_transcript_agent(Some("claude-agent-teams")),
            None
        );
        assert_eq!(resolve_native_chat_transcript_agent(Some("opencode")), None);
    }

    /// P2: the two independent mechanisms agree on all four supported
    /// members plus a negative. Documented limitation: this pin cannot
    /// detect a same-behavior reimplementation of one mechanism in terms of
    /// the other, since such a reimplementation is, by construction,
    /// identical on every input (see the module-level P2 doc).
    #[test]
    fn pin_set_membership_and_resolver_agree_on_every_member_and_a_negative() {
        for member in NATIVE_CHAT_SUPPORTED_AGENTS {
            assert_eq!(
                is_native_chat_supported_agent(Some(member)),
                resolve_native_chat_transcript_agent(Some(member)).is_some(),
                "disagreement on {member}"
            );
        }
        assert_eq!(
            is_native_chat_supported_agent(Some("cursor")),
            resolve_native_chat_transcript_agent(Some("cursor")).is_some()
        );
    }

    /// P3 crux pin: stepping is gated by resolved transcript format, not
    /// the literal agent string — pinned directly rather than only via the
    /// oracle's coverage of `codex`/`grok`.
    #[test]
    fn pin_openclaude_steps_codex_and_grok_do_not() {
        assert!(should_step_native_chat_ask_answer(Some("openclaude")));
        assert!(!should_step_native_chat_ask_answer(Some("codex")));
        assert!(!should_step_native_chat_ask_answer(Some("grok")));
    }

    /// P4: exact, case-sensitive, untrimmed matching — no `js_trim`, no
    /// ASCII-folding. Checked across all three functions.
    #[test]
    fn pin_case_and_whitespace_variants_are_rejected() {
        assert!(!is_native_chat_supported_agent(Some("Claude")));
        assert!(!is_native_chat_supported_agent(Some(" claude ")));
        assert!(!is_native_chat_supported_agent(Some("")));
        assert_eq!(resolve_native_chat_transcript_agent(Some("Claude")), None);
        assert_eq!(
            resolve_native_chat_transcript_agent(Some(" claude ")),
            None
        );
        assert_eq!(resolve_native_chat_transcript_agent(Some("")), None);
        assert!(!should_step_native_chat_ask_answer(Some("Claude")));
        assert!(!should_step_native_chat_ask_answer(Some(" claude ")));
        assert!(!should_step_native_chat_ask_answer(Some("")));
    }

    /// P5 crux pin: `'openclaude'` is a valid input but is not itself a
    /// variant of the return type — it resolves onto `Claude`, and an
    /// unknown agent resolves onto `None`, never a distinct `OpenClaude`
    /// variant (which does not exist).
    #[test]
    fn pin_openclaude_resolves_to_claude_variant_not_a_distinct_variant() {
        assert_eq!(
            resolve_native_chat_transcript_agent(Some("openclaude")),
            Some(NativeChatTranscriptAgent::Claude)
        );
        assert_eq!(
            resolve_native_chat_transcript_agent(Some("openclaude"))
                .map(NativeChatTranscriptAgent::as_str),
            Some("claude")
        );
        assert_eq!(resolve_native_chat_transcript_agent(Some("unknown-agent")), None);
    }
}
