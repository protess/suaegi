//! Source-control panel group order — verbatim port of Orca's
//! `src/shared/source-control-group-order.ts` (@ v1.4.146-rc.0). The
//! `import type { SourceControlGroupOrder } from './types'` becomes a
//! module-local `enum` per [`suaegi-misc-placement-rule`] — no shared types
//! module, no cross-module import.
//!
//! Membership is exact-string (`value === 'changes-first' || ... ? value :
//! DEFAULT`): no trim, no case-fold. The `'changes-first'` arm is genuinely
//! dead code — its value equals `DEFAULT_SOURCE_CONTROL_GROUP_ORDER`, so
//! removing it from the ternary changes no input's observed behavior; kept
//! verbatim (not simplified to an `else`-only match) so the port stays a
//! literal transcription of the three-way ternary rather than a rewrite that
//! could quietly diverge if a future upstream change ever un-couples the two.

/// The closed set of source-control panel group orders. Membership is exact
/// — no looser matcher (trim/case-fold) belongs here (see module doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceControlGroupOrder {
    ChangesFirst,
    StagedFirst,
    UntrackedFirst,
}

pub const DEFAULT_SOURCE_CONTROL_GROUP_ORDER: SourceControlGroupOrder =
    SourceControlGroupOrder::ChangesFirst;

/// `value === 'changes-first' || value === 'staged-first' || value ===
/// 'untracked-first' ? value : DEFAULT`. `None` (nothing supplied) falls to
/// `DEFAULT`, same as any other non-member value.
pub fn normalize_source_control_group_order(value: Option<&str>) -> SourceControlGroupOrder {
    match value {
        Some("changes-first") => SourceControlGroupOrder::ChangesFirst,
        Some("staged-first") => SourceControlGroupOrder::StagedFirst,
        Some("untracked-first") => SourceControlGroupOrder::UntrackedFirst,
        _ => DEFAULT_SOURCE_CONTROL_GROUP_ORDER,
    }
}

#[cfg(test)]
mod tests {
    use super::SourceControlGroupOrder::{ChangesFirst, StagedFirst, UntrackedFirst};
    use super::*;

    // Oracle: source-control-group-order.test.ts

    #[test]
    fn keeps_supported_source_control_group_orders() {
        assert_eq!(normalize_source_control_group_order(Some("changes-first")), ChangesFirst);
        assert_eq!(normalize_source_control_group_order(Some("staged-first")), StagedFirst);
        assert_eq!(normalize_source_control_group_order(Some("untracked-first")), UntrackedFirst);
    }

    #[test]
    fn falls_back_to_the_default_for_malformed_values() {
        assert_eq!(
            normalize_source_control_group_order(Some("tracked-first")),
            DEFAULT_SOURCE_CONTROL_GROUP_ORDER
        );
        assert_eq!(normalize_source_control_group_order(None), DEFAULT_SOURCE_CONTROL_GROUP_ORDER);
    }

    // Mandatory extra pins (oracle-silent):

    /// F15 — each member pinned directly, plus the `DEFAULT` literal.
    #[test]
    fn pin_each_member_and_default_literal() {
        assert_eq!(normalize_source_control_group_order(Some("changes-first")), ChangesFirst);
        assert_eq!(normalize_source_control_group_order(Some("staged-first")), StagedFirst);
        assert_eq!(normalize_source_control_group_order(Some("untracked-first")), UntrackedFirst);
        assert_eq!(DEFAULT_SOURCE_CONTROL_GROUP_ORDER, ChangesFirst);
    }

    /// F15 — no trim/case-fold: a looser matcher would also pass both oracle
    /// cases above (neither uses whitespace or mixed case), so these pin the
    /// exact-string contract directly.
    #[test]
    fn pin_no_trim_or_case_fold() {
        assert_eq!(
            normalize_source_control_group_order(Some("Changes-First")),
            DEFAULT_SOURCE_CONTROL_GROUP_ORDER
        );
        assert_eq!(
            normalize_source_control_group_order(Some(" staged-first ")),
            DEFAULT_SOURCE_CONTROL_GROUP_ORDER
        );
    }
}
