//! Tailnet IPv4 address detection — verbatim port of Orca's
//! `src/shared/tailnet-address.ts` (@ v1.4.150-rc.0).
//!
//! Tailnet (Tailscale/CGNAT) IPv4 addresses live in `100.64.0.0/10`. Phone
//! pairing prefers them over LAN addresses because LAN addresses stop working
//! once devices split networks.
//!
//! The one load-bearing contract: JS `\d` in `/^\d+$/` is **ASCII-only**
//! (`[0-9]`), not Unicode `Nd` — so Arabic-Indic digits like `١٠٠` must be
//! rejected, not accepted. Rust's `char::is_numeric` (and the `regex` crate's
//! `\d`) both match Unicode `Nd` and would silently diverge, so the digit
//! check is hand-rolled on ASCII bytes instead (`suaegi-misc` stays
//! dependency-free — no `regex`).
//!
//! Leading zeros are allowed: JS `Number("0000000100")` is `100`, and Rust's
//! `str::parse::<u64>()` agrees — so no `part.len() <= 3` shortcut (that would
//! reject valid padded octets). Overflow (a very long digit string) makes JS
//! `Number(...)` overflow to `Infinity`, which then fails `Number.isInteger`
//! → `false`; Rust's `parse::<u64>()` fails to parse → `Err` → `false`. Same
//! observable result via a different route.
//!
//! `split('.')` (not `split_terminator`) is required: JS `.split('.')`
//! preserves trailing empty parts (`"1.2.3.4.".split('.')` has 5 parts, so the
//! `parts.length !== 4` check rejects it), and Rust's `str::split` has the
//! same trailing-empty-preserving behavior, whereas `split_terminator` would
//! drop the empty tail and silently accept the malformed input.
//!
//! The JS `octet < 0` check (unreachable — the `/^\d+$/` regex already
//! excludes `-`) is dead code and is not ported; only the reachable
//! `> 255` bound is checked here.

/// `isTailnetIPv4Address`: true iff `address` is exactly 4 dot-separated
/// ASCII-digit octets, each parsing (with leading zeros allowed) to a value
/// `<= 255`, with `octets[0] == 100` and `octets[1]` in `64..=127`
/// (the `100.64.0.0/10` CGNAT/Tailnet allocation).
pub fn is_tailnet_ipv4_address(address: &str) -> bool {
    let parts: Vec<&str> = address.split('.').collect();
    if parts.len() != 4 {
        return false;
    }

    let mut octets = [0u64; 4];
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        match part.parse::<u64>() {
            Ok(value) if value <= 255 => octets[i] = value,
            _ => return false,
        }
    }

    octets[0] == 100 && (64..=127).contains(&octets[1])
}

#[cfg(test)]
mod tests {
    use super::is_tailnet_ipv4_address;

    // Oracle: tailnet-address.test.ts

    #[test]
    fn accepts_the_tailnet_ipv4_allocation_range() {
        assert!(is_tailnet_ipv4_address("100.64.0.1"));
        assert!(is_tailnet_ipv4_address("100.102.47.57"));
        assert!(is_tailnet_ipv4_address("100.127.255.254"));
    }

    #[test]
    fn rejects_non_tailnet_ipv4_addresses_and_malformed_input() {
        assert!(!is_tailnet_ipv4_address("100.63.255.255"));
        assert!(!is_tailnet_ipv4_address("100.128.0.1"));
        assert!(!is_tailnet_ipv4_address("192.168.1.24"));
        assert!(!is_tailnet_ipv4_address("fd7a:115c:a1e0::ce33:2f3a"));
        assert!(!is_tailnet_ipv4_address("100.102.47"));
    }

    // Mandatory extra pins (oracle-silent):

    /// Leading zeros are allowed and parsed as decimal, not rejected by a
    /// `part.len() <= 3` shortcut.
    #[test]
    fn pin_leading_zeros_allowed() {
        assert!(is_tailnet_ipv4_address("100.0064.0.1"));
    }

    /// An empty octet (consecutive dots) must be rejected.
    #[test]
    fn pin_empty_part_rejected() {
        assert!(!is_tailnet_ipv4_address("100..0.1"));
    }

    /// An octet over 255 must be rejected even though it is all-ASCII-digit.
    #[test]
    fn pin_octet_over_255_rejected() {
        assert!(!is_tailnet_ipv4_address("100.999.0.1"));
    }

    /// A very long digit string overflows `u64::parse` -> `Err` -> reject,
    /// mirroring JS `Number(...)` overflowing to `Infinity` -> not an integer.
    #[test]
    fn pin_overflowing_digit_string_rejected() {
        assert!(!is_tailnet_ipv4_address(
            "100.99999999999999999999999999999999.0.1"
        ));
    }

    /// Unicode (Arabic-Indic) digits are NOT ASCII `\d` and must be rejected;
    /// `char::is_numeric` would wrongly accept them.
    #[test]
    fn pin_unicode_digits_rejected() {
        // Arabic-Indic "١٠٠" (100) used as the second octet.
        assert!(!is_tailnet_ipv4_address("100.\u{661}\u{660}\u{660}.0.1"));
    }

    /// A trailing dot yields 5 parts via `split('.')` (last one empty), so it
    /// must fail the length check, not be silently accepted.
    #[test]
    fn pin_trailing_dot_rejected() {
        assert!(!is_tailnet_ipv4_address("1.2.3.4."));
    }

    /// Whitespace-padded input is not trimmed and fails the ASCII-digit check.
    #[test]
    fn pin_whitespace_padded_rejected() {
        assert!(!is_tailnet_ipv4_address(" 100.64.0.1"));
        assert!(!is_tailnet_ipv4_address("100.64.0.1 "));
    }
}
