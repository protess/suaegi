//! Runtime/mobile protocol compatibility evaluators — verbatim port of Orca's
//! `src/shared/protocol-compat.ts` (@ v1.4.150-rc.0).
//!
//! Why: pure compat evaluators shared between desktop tests, renderer runtime
//! switching, and the mobile mirror. All version numbers are passed in to keep
//! the logic dependency-free and easy to duplicate elsewhere.
//!
//! # Signed versions (the one real trap)
//! The oracle passes `desktopProtocolVersion: -1` (`test:121`) to exercise the
//! kill-switch precedence, so every version field here is `i64`, never a
//! `u32`/`usize`. Do not "fix" this to an unsigned type — it would reject a
//! valid oracle input.
//!
//! # No shared "blocked" struct (the other real trap)
//! Orca's TS type puts `requiredClientProtocolVersion` on the
//! `client-too-old` branch only and `requiredServerProtocolVersion` on the
//! `server-too-old` branch only (each optional, `?`), and the oracle's
//! `toEqual` (`test:77-82`, `test:106-112`) asserts the *other* key is
//! **absent**, not `undefined`-but-present. A single Rust struct with both
//! fields as `Option<i64>` would let a bug default the wrong one to `Some(0)`
//! without any type-level guard. Modeling each reason as its own enum variant
//! makes that class of bug unrepresentable.

/// Result of [`evaluate_runtime_compat`]. Each `*TooOld` variant carries only
/// the fields the corresponding TS union member has — see the module doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeCompatVerdict {
    Ok {
        client_protocol_version: i64,
        server_protocol_version: i64,
    },
    ClientTooOld {
        client_protocol_version: i64,
        server_protocol_version: i64,
        required_client_protocol_version: i64,
    },
    ServerTooOld {
        client_protocol_version: i64,
        server_protocol_version: i64,
        required_server_protocol_version: i64,
    },
}

/// Input for [`evaluate_runtime_compat`], mirroring the TS inline object type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeCompatInput {
    pub client_protocol_version: i64,
    pub min_compatible_server_protocol_version: i64,
    pub server_protocol_version: Option<i64>,
    pub server_min_compatible_client_protocol_version: Option<i64>,
}

/// Evaluate whether a runtime RPC client/server pair may talk to each other.
///
/// Why: absent fields are protocol 0. New clients can give old servers a
/// clear "update server" error instead of attempting partially-supported
/// RPCs.
///
/// Check order is load-bearing (D7): client-too-old is checked *before*
/// server-too-old, and both comparisons are strict `<` (equal versions
/// pass).
pub fn evaluate_runtime_compat(input: RuntimeCompatInput) -> RuntimeCompatVerdict {
    let server_protocol_version = input.server_protocol_version.unwrap_or(0);
    let required_client_protocol_version = input
        .server_min_compatible_client_protocol_version
        .unwrap_or(0);

    if input.client_protocol_version < required_client_protocol_version {
        return RuntimeCompatVerdict::ClientTooOld {
            client_protocol_version: input.client_protocol_version,
            server_protocol_version,
            required_client_protocol_version,
        };
    }
    if server_protocol_version < input.min_compatible_server_protocol_version {
        return RuntimeCompatVerdict::ServerTooOld {
            client_protocol_version: input.client_protocol_version,
            server_protocol_version,
            required_server_protocol_version: input.min_compatible_server_protocol_version,
        };
    }
    RuntimeCompatVerdict::Ok {
        client_protocol_version: input.client_protocol_version,
        server_protocol_version,
    }
}

/// Human-readable description of a [`RuntimeCompatVerdict`]. The three
/// strings are copied verbatim from `describeRuntimeCompatBlock`
/// (`protocol-compat.ts:56-64`).
pub fn describe_runtime_compat_block(verdict: &RuntimeCompatVerdict) -> String {
    match verdict {
        RuntimeCompatVerdict::Ok { .. } => "Runtime client and server are compatible.".to_string(),
        RuntimeCompatVerdict::ClientTooOld {
            client_protocol_version,
            required_client_protocol_version,
            ..
        } => format!(
            "This Orca client is too old for the selected server. Update Orca on this machine. Client protocol {client_protocol_version}, server requires client protocol {required_client_protocol_version}."
        ),
        RuntimeCompatVerdict::ServerTooOld {
            server_protocol_version,
            required_server_protocol_version,
            ..
        } => format!(
            "The selected Orca server is too old for this client. Update Orca on the server. Server protocol {server_protocol_version}, client requires server protocol {required_server_protocol_version}."
        ),
    }
}

/// Result of [`evaluate_compat`]. Each `*TooOld` variant carries only the
/// fields the corresponding TS union member has (same rationale as
/// [`RuntimeCompatVerdict`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompatVerdict {
    Ok,
    MobileTooOld {
        desktop_version: i64,
        required_mobile_version: i64,
    },
    DesktopTooOld {
        desktop_version: i64,
        required_desktop_version: i64,
    },
}

/// Input for [`evaluate_compat`], mirroring the TS inline object type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompatInput {
    pub mobile_protocol_version: i64,
    pub min_compatible_desktop_version: i64,
    pub desktop_protocol_version: Option<i64>,
    pub desktop_min_compatible_mobile_version: Option<i64>,
}

/// Evaluate whether a mobile/desktop protocol pair may talk to each other
/// (D7 — same algorithm as [`evaluate_runtime_compat`], different payload).
///
/// Why: absent fields → 0 lets mobile keep talking to pre-PR desktops.
/// Bumping `min_compatible_desktop_version` above 0 will fence those older
/// desktops alongside any explicitly-version-0 desktop, which is the
/// intended kill-switch behavior.
///
/// Why mobile-too-old precedence: if desktop says "I refuse this mobile
/// build" (kill switch), that wins over any local mobile judgment about
/// desktop's age — this check runs before the desktop-too-old check.
pub fn evaluate_compat(input: CompatInput) -> CompatVerdict {
    let desktop_version = input.desktop_protocol_version.unwrap_or(0);
    let required_mobile = input.desktop_min_compatible_mobile_version.unwrap_or(0);

    if input.mobile_protocol_version < required_mobile {
        return CompatVerdict::MobileTooOld {
            desktop_version,
            required_mobile_version: required_mobile,
        };
    }
    if desktop_version < input.min_compatible_desktop_version {
        return CompatVerdict::DesktopTooOld {
            desktop_version,
            required_desktop_version: input.min_compatible_desktop_version,
        };
    }
    CompatVerdict::Ok
}

#[cfg(test)]
mod tests {
    use super::*;

    // Oracle constants copied from `protocol-version.ts` (not itself in scope
    // for this port — only referenced by the protocol-compat oracle).
    const MOBILE_V: i64 = 1;
    const DESKTOP_PROTOCOL_VERSION: i64 = 3;
    const MIN_COMPATIBLE_MOBILE_VERSION: i64 = 2;
    const RUNTIME_PROTOCOL_VERSION: i64 = 3;
    const MIN_COMPATIBLE_RUNTIME_CLIENT_VERSION: i64 = 2;
    const MIN_COMPATIBLE_RUNTIME_SERVER_VERSION: i64 = 2;

    // Oracle: protocol-compat.test.ts `describe('evaluateCompat', ...)`

    #[test]
    fn compat_ok_when_desktop_fields_undefined_and_constants_wide_open() {
        let verdict = evaluate_compat(CompatInput {
            mobile_protocol_version: MOBILE_V,
            min_compatible_desktop_version: 0,
            desktop_protocol_version: None,
            desktop_min_compatible_mobile_version: None,
        });
        assert_eq!(verdict, CompatVerdict::Ok);
    }

    #[test]
    fn compat_ok_when_desktop_reports_version_equal_to_mobile() {
        let verdict = evaluate_compat(CompatInput {
            mobile_protocol_version: MOBILE_V,
            min_compatible_desktop_version: 0,
            desktop_protocol_version: Some(MOBILE_V),
            desktop_min_compatible_mobile_version: Some(0),
        });
        assert_eq!(verdict, CompatVerdict::Ok);
    }

    #[test]
    fn compat_ok_when_desktop_reports_a_newer_version() {
        let verdict = evaluate_compat(CompatInput {
            mobile_protocol_version: MOBILE_V,
            min_compatible_desktop_version: 0,
            desktop_protocol_version: Some(MOBILE_V + 5),
            desktop_min_compatible_mobile_version: Some(0),
        });
        assert_eq!(verdict, CompatVerdict::Ok);
    }

    #[test]
    fn compat_allows_desktop_3_to_roll_out_before_mobile_2_updates() {
        let verdict = evaluate_compat(CompatInput {
            mobile_protocol_version: 2,
            min_compatible_desktop_version: 2,
            desktop_protocol_version: Some(3),
            desktop_min_compatible_mobile_version: Some(2),
        });
        assert_eq!(verdict, CompatVerdict::Ok);
    }

    #[test]
    fn compat_allows_mobile_3_to_roll_out_before_desktop_2_updates() {
        let verdict = evaluate_compat(CompatInput {
            mobile_protocol_version: 3,
            min_compatible_desktop_version: 2,
            desktop_protocol_version: Some(2),
            desktop_min_compatible_mobile_version: Some(2),
        });
        assert_eq!(verdict, CompatVerdict::Ok);
    }

    #[test]
    fn compat_blocks_mobile_too_old_when_desktop_requires_newer_mobile() {
        let verdict = evaluate_compat(CompatInput {
            mobile_protocol_version: MOBILE_V,
            min_compatible_desktop_version: 0,
            desktop_protocol_version: Some(5),
            desktop_min_compatible_mobile_version: Some(MOBILE_V + 1),
        });
        assert_eq!(
            verdict,
            CompatVerdict::MobileTooOld {
                desktop_version: 5,
                required_mobile_version: MOBILE_V + 1,
            }
        );
    }

    #[test]
    fn compat_coerces_none_desktop_version_to_0_in_verdict_payload() {
        let verdict = evaluate_compat(CompatInput {
            mobile_protocol_version: MOBILE_V,
            min_compatible_desktop_version: 0,
            desktop_protocol_version: None,
            desktop_min_compatible_mobile_version: Some(MOBILE_V + 1),
        });
        assert_eq!(
            verdict,
            CompatVerdict::MobileTooOld {
                desktop_version: 0,
                required_mobile_version: MOBILE_V + 1,
            }
        );
    }

    #[test]
    fn compat_blocks_desktop_too_old_when_below_local_minimum() {
        let verdict = evaluate_compat(CompatInput {
            mobile_protocol_version: MOBILE_V,
            min_compatible_desktop_version: 5,
            desktop_protocol_version: Some(3),
            desktop_min_compatible_mobile_version: Some(0),
        });
        assert_eq!(
            verdict,
            CompatVerdict::DesktopTooOld {
                desktop_version: 3,
                required_desktop_version: 5,
            }
        );
    }

    /// D6: negative version payload (`desktop_protocol_version: -1`) — the
    /// crux pin that rules out an unsigned version type.
    #[test]
    fn compat_mobile_too_old_wins_precedence_with_negative_desktop_version() {
        let verdict = evaluate_compat(CompatInput {
            mobile_protocol_version: MOBILE_V,
            min_compatible_desktop_version: 99,
            desktop_protocol_version: Some(-1),
            desktop_min_compatible_mobile_version: Some(MOBILE_V + 1),
        });
        assert_eq!(
            verdict,
            CompatVerdict::MobileTooOld {
                desktop_version: -1,
                required_mobile_version: MOBILE_V + 1,
            }
        );
    }

    #[test]
    fn compat_with_min_compatible_desktop_version_0_every_reported_desktop_passes() {
        for v in [0, 1, 2, 99] {
            let verdict = evaluate_compat(CompatInput {
                mobile_protocol_version: MOBILE_V,
                min_compatible_desktop_version: 0,
                desktop_protocol_version: Some(v),
                desktop_min_compatible_mobile_version: Some(0),
            });
            assert_eq!(verdict, CompatVerdict::Ok);
        }
    }

    #[test]
    fn compat_hard_blocks_protocol_1_mobile_for_binary_terminal_stream_cutover() {
        let verdict = evaluate_compat(CompatInput {
            mobile_protocol_version: 1,
            min_compatible_desktop_version: DESKTOP_PROTOCOL_VERSION,
            desktop_protocol_version: Some(DESKTOP_PROTOCOL_VERSION),
            desktop_min_compatible_mobile_version: Some(MIN_COMPATIBLE_MOBILE_VERSION),
        });
        assert_eq!(
            verdict,
            CompatVerdict::MobileTooOld {
                desktop_version: DESKTOP_PROTOCOL_VERSION,
                required_mobile_version: MIN_COMPATIBLE_MOBILE_VERSION,
            }
        );
    }

    // Oracle: protocol-compat.test.ts `describe('evaluateRuntimeCompat', ...)`

    #[test]
    fn runtime_keeps_current_client_and_server_self_compatible() {
        let verdict = evaluate_runtime_compat(RuntimeCompatInput {
            client_protocol_version: RUNTIME_PROTOCOL_VERSION,
            min_compatible_server_protocol_version: MIN_COMPATIBLE_RUNTIME_SERVER_VERSION,
            server_protocol_version: Some(RUNTIME_PROTOCOL_VERSION),
            server_min_compatible_client_protocol_version: Some(
                MIN_COMPATIBLE_RUNTIME_CLIENT_VERSION,
            ),
        });
        assert_eq!(
            verdict,
            RuntimeCompatVerdict::Ok {
                client_protocol_version: RUNTIME_PROTOCOL_VERSION,
                server_protocol_version: RUNTIME_PROTOCOL_VERSION,
            }
        );
    }

    #[test]
    fn runtime_allows_app_versions_to_skew_when_protocol_ranges_overlap() {
        let verdict = evaluate_runtime_compat(RuntimeCompatInput {
            client_protocol_version: RUNTIME_PROTOCOL_VERSION,
            min_compatible_server_protocol_version: MIN_COMPATIBLE_RUNTIME_SERVER_VERSION,
            server_protocol_version: Some(RUNTIME_PROTOCOL_VERSION + 3),
            server_min_compatible_client_protocol_version: Some(RUNTIME_PROTOCOL_VERSION - 1),
        });
        assert_eq!(
            verdict,
            RuntimeCompatVerdict::Ok {
                client_protocol_version: RUNTIME_PROTOCOL_VERSION,
                server_protocol_version: RUNTIME_PROTOCOL_VERSION + 3,
            }
        );
    }

    #[test]
    fn runtime_blocks_when_server_requires_newer_client_protocol() {
        let verdict = evaluate_runtime_compat(RuntimeCompatInput {
            client_protocol_version: RUNTIME_PROTOCOL_VERSION,
            min_compatible_server_protocol_version: MIN_COMPATIBLE_RUNTIME_SERVER_VERSION,
            server_protocol_version: Some(RUNTIME_PROTOCOL_VERSION + 1),
            server_min_compatible_client_protocol_version: Some(RUNTIME_PROTOCOL_VERSION + 1),
        });
        assert_eq!(
            verdict,
            RuntimeCompatVerdict::ClientTooOld {
                client_protocol_version: RUNTIME_PROTOCOL_VERSION,
                server_protocol_version: RUNTIME_PROTOCOL_VERSION + 1,
                required_client_protocol_version: RUNTIME_PROTOCOL_VERSION + 1,
            }
        );
        assert!(describe_runtime_compat_block(&verdict).contains("client is too old"));
    }

    #[test]
    fn runtime_blocks_when_server_protocol_below_client_minimum() {
        let verdict = evaluate_runtime_compat(RuntimeCompatInput {
            client_protocol_version: RUNTIME_PROTOCOL_VERSION,
            min_compatible_server_protocol_version: RUNTIME_PROTOCOL_VERSION,
            server_protocol_version: Some(RUNTIME_PROTOCOL_VERSION - 1),
            server_min_compatible_client_protocol_version: Some(0),
        });
        assert_eq!(
            verdict,
            RuntimeCompatVerdict::ServerTooOld {
                client_protocol_version: RUNTIME_PROTOCOL_VERSION,
                server_protocol_version: RUNTIME_PROTOCOL_VERSION - 1,
                required_server_protocol_version: RUNTIME_PROTOCOL_VERSION,
            }
        );
        assert!(describe_runtime_compat_block(&verdict).contains("server is too old"));
    }

    /// D6: `None` server fields are treated as protocol 0.
    #[test]
    fn runtime_treats_none_server_fields_as_protocol_0() {
        let verdict = evaluate_runtime_compat(RuntimeCompatInput {
            client_protocol_version: RUNTIME_PROTOCOL_VERSION,
            min_compatible_server_protocol_version: 1,
            server_protocol_version: None,
            server_min_compatible_client_protocol_version: None,
        });
        assert_eq!(
            verdict,
            RuntimeCompatVerdict::ServerTooOld {
                client_protocol_version: RUNTIME_PROTOCOL_VERSION,
                server_protocol_version: 0,
                required_server_protocol_version: 1,
            }
        );
    }

    // Extra pins (oracle-silent):

    /// D6 crux pin: the `ClientTooOld` verdict must not carry a
    /// `required_server_protocol_version` field at all — proven here by
    /// exhaustive `match` (a shared struct with `Option` fields could not
    /// make this a compile-time guarantee).
    #[test]
    fn pin_client_too_old_variant_carries_only_its_own_field() {
        let verdict = evaluate_runtime_compat(RuntimeCompatInput {
            client_protocol_version: 0,
            min_compatible_server_protocol_version: 0,
            server_protocol_version: Some(0),
            server_min_compatible_client_protocol_version: Some(5),
        });
        match verdict {
            RuntimeCompatVerdict::ClientTooOld {
                required_client_protocol_version,
                ..
            } => assert_eq!(required_client_protocol_version, 5),
            other => panic!("expected ClientTooOld, got {other:?}"),
        }
    }

    /// D7: check-order pin — when both the client-too-old and
    /// server-too-old conditions would independently fire, client-too-old
    /// wins because it is checked first.
    #[test]
    fn pin_runtime_client_too_old_wins_precedence_over_server_too_old() {
        let verdict = evaluate_runtime_compat(RuntimeCompatInput {
            client_protocol_version: 0,
            min_compatible_server_protocol_version: 99,
            server_protocol_version: Some(-1),
            server_min_compatible_client_protocol_version: Some(1),
        });
        assert_eq!(
            verdict,
            RuntimeCompatVerdict::ClientTooOld {
                client_protocol_version: 0,
                server_protocol_version: -1,
                required_client_protocol_version: 1,
            }
        );
    }

    #[test]
    fn describe_runtime_compat_block_ok_string_is_verbatim() {
        let verdict = RuntimeCompatVerdict::Ok {
            client_protocol_version: 1,
            server_protocol_version: 1,
        };
        assert_eq!(
            describe_runtime_compat_block(&verdict),
            "Runtime client and server are compatible."
        );
    }

    /// D7 crux pin: `client_protocol_version` exactly equal to
    /// `server_min_compatible_client_protocol_version` must be OK — the
    /// client-too-old comparison is strict `<`, so an equal version passes.
    /// Why: a regression from `<` to `<=` at this comparison site would flip
    /// this exact-equality case to `ClientTooOld`.
    #[test]
    fn pin_runtime_client_protocol_version_equal_to_required_is_ok() {
        // server_protocol_version (5) is well above min_compatible_server_protocol_version
        // (0) — not equal — so this test isolates the client-too-old comparison
        // site and cannot be incidentally caught by a mutation at the
        // server-too-old comparison site.
        let verdict = evaluate_runtime_compat(RuntimeCompatInput {
            client_protocol_version: 2,
            min_compatible_server_protocol_version: 0,
            server_protocol_version: Some(5),
            server_min_compatible_client_protocol_version: Some(2),
        });
        match verdict {
            RuntimeCompatVerdict::Ok {
                client_protocol_version,
                server_protocol_version,
            } => {
                assert_eq!(client_protocol_version, 2);
                assert_eq!(server_protocol_version, 5);
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    /// D7 crux pin: `server_protocol_version` exactly equal to
    /// `min_compatible_server_protocol_version` must be OK — the
    /// server-too-old comparison is strict `<`, so an equal version passes.
    /// Why: a regression from `<` to `<=` at this comparison site would flip
    /// this exact-equality case to `ServerTooOld`.
    #[test]
    fn pin_runtime_server_protocol_version_equal_to_required_is_ok() {
        // client_protocol_version (5) is well above server_min_compatible_client_protocol_version
        // (0) — not equal — so this test isolates the server-too-old comparison
        // site and cannot be incidentally caught by a mutation at the
        // client-too-old comparison site.
        let verdict = evaluate_runtime_compat(RuntimeCompatInput {
            client_protocol_version: 5,
            min_compatible_server_protocol_version: 2,
            server_protocol_version: Some(2),
            server_min_compatible_client_protocol_version: Some(0),
        });
        match verdict {
            RuntimeCompatVerdict::Ok {
                client_protocol_version,
                server_protocol_version,
            } => {
                assert_eq!(client_protocol_version, 5);
                assert_eq!(server_protocol_version, 2);
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    /// D7 crux pin: `mobile_protocol_version` exactly equal to
    /// `desktop_min_compatible_mobile_version` must be OK — the
    /// mobile-too-old comparison is strict `<`, so an equal version passes.
    /// Why: a regression from `<` to `<=` at this comparison site would flip
    /// this exact-equality case to `MobileTooOld`.
    #[test]
    fn pin_compat_mobile_protocol_version_equal_to_required_is_ok() {
        // desktop_protocol_version (5) is well above min_compatible_desktop_version
        // (0) — not equal — so this test isolates the mobile-too-old comparison
        // site and cannot be incidentally caught by a mutation at the
        // desktop-too-old comparison site.
        let verdict = evaluate_compat(CompatInput {
            mobile_protocol_version: 2,
            min_compatible_desktop_version: 0,
            desktop_protocol_version: Some(5),
            desktop_min_compatible_mobile_version: Some(2),
        });
        match verdict {
            CompatVerdict::Ok => {}
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    /// D7 crux pin: `desktop_protocol_version` exactly equal to
    /// `min_compatible_desktop_version` must be OK — the desktop-too-old
    /// comparison is strict `<`, so an equal version passes. Why: a
    /// regression from `<` to `<=` at this comparison site would flip this
    /// exact-equality case to `DesktopTooOld`.
    #[test]
    fn pin_compat_desktop_protocol_version_equal_to_required_is_ok() {
        // mobile_protocol_version (5) is well above desktop_min_compatible_mobile_version
        // (0) — not equal — so this test isolates the desktop-too-old comparison
        // site and cannot be incidentally caught by a mutation at the
        // mobile-too-old comparison site.
        let verdict = evaluate_compat(CompatInput {
            mobile_protocol_version: 5,
            min_compatible_desktop_version: 2,
            desktop_protocol_version: Some(2),
            desktop_min_compatible_mobile_version: Some(0),
        });
        match verdict {
            CompatVerdict::Ok => {}
            other => panic!("expected Ok, got {other:?}"),
        }
    }
}
