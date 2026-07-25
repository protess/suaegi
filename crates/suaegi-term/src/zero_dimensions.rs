//! Port of Orca `shared/terminal-zero-dimensions-diagnostic.ts` (@ v1.4.150-rc.0).
//!
//! The zero-dimensions diagnostic is emitted by the PTY connect path and cleared
//! once a hidden pane becomes visible and refits. Keeping the message text and
//! its matcher together keeps both sites in sync.

const ZERO_DIMENSIONS_PREFIX: &str = "Terminal has zero dimensions (";

/// Build the zero-dimensions diagnostic message for the given cols×rows.
pub fn create_terminal_zero_dimensions_message(cols: u32, rows: u32) -> String {
    format!(
        "Terminal has zero dimensions ({cols}\u{d7}{rows}). The pane container may not be visible."
    )
}

/// True when `message` is a zero-dimensions diagnostic (prefix match).
pub fn is_terminal_zero_dimensions_diagnostic(message: &str) -> bool {
    message.starts_with(ZERO_DIMENSIONS_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_its_own_message_through_the_matcher() {
        assert!(is_terminal_zero_dimensions_diagnostic(
            &create_terminal_zero_dimensions_message(0, 0)
        ));
    }

    #[test]
    fn message_contains_the_multiplication_sign_and_dims() {
        // U+00D7 MULTIPLICATION SIGN, not ASCII 'x'.
        assert_eq!(
            create_terminal_zero_dimensions_message(12, 34),
            "Terminal has zero dimensions (12\u{d7}34). The pane container may not be visible."
        );
    }

    #[test]
    fn does_not_match_unrelated_terminal_errors() {
        assert!(!is_terminal_zero_dimensions_diagnostic("Paste failed."));
        assert!(!is_terminal_zero_dimensions_diagnostic(
            "Failed to save terminal session state"
        ));
    }
}
