//! Remote runtime client error classification — verbatim port of Orca's
//! `src/shared/remote-runtime-client-error-classification.ts`
//! (@ v1.4.150-rc.0).
//!
//! Decides whether a failure talking to the remote Orca runtime is worth
//! retrying (transport hiccup) versus surfacing (auth/protocol failure).
//!
//! The one load-bearing contract: the `code` check and the `message` check
//! use **different** case sensitivity. `code` is compared to a 5-entry set
//! **exactly, case-sensitively** (`"TIMEOUT"` does NOT match `"timeout"`).
//! `message` is lowercased first (full Unicode `to_lowercase`, matching JS
//! `String.prototype.toLowerCase`) and then checked for any of 8 substrings,
//! so message matching IS effectively case-insensitive. This asymmetry is
//! transcribed verbatim from the source, not an oversight.
//!
//! `error.code && RECOVERABLE_CODES.has(error.code)` in JS short-circuits on
//! the empty string (`""` is falsy), so `code: Some("")` must fall through to
//! message matching rather than being treated as a (vacuously absent) code
//! match — modeled here as `code.is_some_and(|c| !c.is_empty() && ...)`.
//!
//! `toRemoteRuntimeClientErrorLike` (source lines ~32-43) is **not** ported:
//! it does `unknown` sniffing (`typeof candidate.message === 'string'`) and
//! falls back to `String(error)`, whose JS semantics (e.g. `"[object
//! Object]"` for a plain object, `"undefined"` for `undefined`) have no Rust
//! analog. Recovering a [`RemoteRuntimeClientErrorLike`] from an arbitrary
//! caller-side error value is therefore the **caller's responsibility**; this
//! module only provides the 1:1 struct and an `Error`-message constructor
//! (`from_message`) for the common "plain error with a message" case the
//! oracle exercises via `toRemoteRuntimeClientErrorLike(new Error(message))`.

/// 1:1 port of the TS `RemoteRuntimeClientErrorLike` type (`code` optional,
/// `message` required).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRuntimeClientErrorLike {
    pub code: Option<String>,
    pub message: String,
}

impl RemoteRuntimeClientErrorLike {
    /// Builds a code-less error from a message, mirroring the oracle's
    /// `toRemoteRuntimeClientErrorLike(new Error(message))` path (a plain
    /// `Error` has a string `message` and no `code`).
    pub fn from_message(message: &str) -> Self {
        Self {
            code: None,
            message: message.to_string(),
        }
    }
}

/// Verbatim transcription of `RECOVERABLE_CODES` (source lines 4-8).
const RECOVERABLE_CODES: [&str; 5] = [
    "remote_runtime_unavailable",
    "runtime_timeout",
    "runtime_unavailable",
    "reconnecting",
    "timeout",
];

/// Verbatim transcription of `RECOVERABLE_MESSAGE_FRAGMENTS` (source lines
/// 12-19), already all-lowercase.
const RECOVERABLE_MESSAGE_FRAGMENTS: [&str; 8] = [
    "could not connect to the remote orca runtime",
    "remote orca runtime closed the connection",
    "remote orca runtime connection closed",
    "remote orca runtime is not connected",
    "remote runtime connection closed",
    "remote runtime subscription closed before it started",
    "remote terminal stream is not connected",
    "timed out waiting for the remote orca runtime",
];

/// `isRecoverableRemoteRuntimeConnectionError`: true if `error.code` is
/// present, non-empty, and an exact case-sensitive match against the 5-entry
/// recoverable-codes set; otherwise true if the lowercased `error.message`
/// contains any of the 8 recoverable-message fragments.
pub fn is_recoverable_remote_runtime_connection_error(
    error: &RemoteRuntimeClientErrorLike,
) -> bool {
    if error
        .code
        .as_deref()
        .is_some_and(|code| !code.is_empty() && RECOVERABLE_CODES.contains(&code))
    {
        return true;
    }
    let message = error.message.to_lowercase();
    RECOVERABLE_MESSAGE_FRAGMENTS
        .iter()
        .any(|fragment| message.contains(fragment))
}

#[cfg(test)]
mod tests {
    use super::{is_recoverable_remote_runtime_connection_error, RemoteRuntimeClientErrorLike};

    fn err(code: Option<&str>, message: &str) -> RemoteRuntimeClientErrorLike {
        RemoteRuntimeClientErrorLike {
            code: code.map(str::to_string),
            message: message.to_string(),
        }
    }

    // Oracle: remote-runtime-client-error-classification.test.ts

    #[test]
    fn treats_known_codes_as_recoverable() {
        for code in [
            "remote_runtime_unavailable",
            "runtime_timeout",
            "runtime_unavailable",
            "reconnecting",
        ] {
            assert!(is_recoverable_remote_runtime_connection_error(&err(
                Some(code),
                "transport failed"
            )));
        }
    }

    #[test]
    fn does_not_retry_authentication_or_protocol_failures() {
        assert!(!is_recoverable_remote_runtime_connection_error(&err(
            Some("unauthorized"),
            "bad token"
        )));
        assert!(!is_recoverable_remote_runtime_connection_error(&err(
            Some("invalid_runtime_response"),
            "bad frame"
        )));
    }

    #[test]
    fn normalizes_unstructured_connection_failures_from_a_message() {
        for message in [
            "Could not connect to the remote Orca runtime.",
            "Remote Orca runtime closed the connection.",
            "Remote Orca runtime connection closed.",
            "Remote Orca runtime is not connected.",
            "Remote runtime subscription closed before it started.",
        ] {
            let error = RemoteRuntimeClientErrorLike::from_message(message);
            assert!(is_recoverable_remote_runtime_connection_error(&error));
        }
    }

    // Mandatory extra pins (oracle-silent):

    /// `timeout` is in the recoverable-codes set but never exercised by the
    /// oracle's `it.each`.
    #[test]
    fn pin_timeout_code_is_recoverable() {
        assert!(is_recoverable_remote_runtime_connection_error(&err(
            Some("timeout"),
            "irrelevant"
        )));
    }

    /// At least 3 message fragments the oracle never exercises.
    #[test]
    fn pin_untested_message_fragments_are_recoverable() {
        assert!(is_recoverable_remote_runtime_connection_error(&err(
            None,
            "Remote terminal stream is not connected right now."
        )));
        assert!(is_recoverable_remote_runtime_connection_error(&err(
            None,
            "Timed out waiting for the remote Orca runtime to respond."
        )));
        assert!(is_recoverable_remote_runtime_connection_error(&err(
            None,
            "Remote runtime connection closed unexpectedly."
        )));
    }

    /// `code: Some("")` is falsy in JS (`error.code && ...`), so it must fall
    /// through to message matching instead of short-circuiting.
    #[test]
    fn pin_empty_code_falls_through_to_message_matching() {
        assert!(!is_recoverable_remote_runtime_connection_error(&err(
            Some(""),
            "totally unrelated failure"
        )));
        assert!(is_recoverable_remote_runtime_connection_error(&err(
            Some(""),
            "remote runtime connection closed"
        )));
    }

    /// Code matching is exact and case-SENSITIVE: "TIMEOUT" does not match
    /// the "timeout" entry, so this must fail unless the message matches.
    #[test]
    fn pin_code_matching_is_case_sensitive() {
        assert!(!is_recoverable_remote_runtime_connection_error(&err(
            Some("TIMEOUT"),
            "totally unrelated failure"
        )));
    }

    /// Message matching IS case-insensitive (message is lowercased before the
    /// substring check).
    #[test]
    fn pin_message_matching_is_case_insensitive() {
        assert!(is_recoverable_remote_runtime_connection_error(&err(
            None,
            "REMOTE RUNTIME CONNECTION CLOSED"
        )));
    }
}
