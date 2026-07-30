//! UI language selection — verbatim port of Orca's `src/shared/ui-language.ts`
//! (@ v1.4.146-rc.0).
//!
//! `'system'` is not a locale, it is a sentinel two consumers branch on
//! (`main-i18n.ts:83`, `ui-locale.ts:66`), so this is modeled as
//! `enum UiLanguage { System, En, Zh, Ko, Ja, Es }` with an [`UiLanguage::as_str`]
//! rather than a bare `&'static str`. The six `UI_LANGUAGE_*` constants are the
//! same truth stated twice — `as_str()` must agree with them — but they stay
//! `pub` because six upstream call sites quote the string constants directly.
//!
//! Membership is exact-string (`Set.has` on an all-`String` set is
//! `SameValueZero`, i.e. plain string equality): no trim, no ASCII/locale
//! lowercasing, and no `-`/`_` locale-tag splitting. The sibling module
//! `ui-locale.ts` really does perform that looser normalization, but it is a
//! different module with different semantics and out of scope here — do not
//! let it bleed into this one.
//!
//! Takes `Option<&str>`, not the caller's already-resolved fallback: upstream
//! calls this as `normalizeUiLanguage(updates.x ?? base.x)`, and `??` is
//! nullish coalescing, so falsy-but-non-nullish inputs (`''`, `0`) reach the
//! function unchanged rather than being replaced by `base.x` first. Absorbing
//! a caller's `??` into this function would change that. The fallback
//! decision belongs to the caller; `None` here always means "no value was
//! supplied at all" and normalizes to `System`, same as any other unknown
//! string.

pub const UI_LANGUAGE_SYSTEM: &str = "system";
pub const UI_LANGUAGE_ENGLISH: &str = "en";
pub const UI_LANGUAGE_CHINESE: &str = "zh";
pub const UI_LANGUAGE_KOREAN: &str = "ko";
pub const UI_LANGUAGE_JAPANESE: &str = "ja";
pub const UI_LANGUAGE_SPANISH: &str = "es";

/// The closed set of supported UI languages. Membership is exact — no
/// looser matcher (trim/lowercase/locale-tag split) belongs here (see module
/// doc). Variant order is not part of the contract: upstream never
/// enumerates, spreads, or serializes the set in an order-observable way, so
/// arm order here is free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiLanguage {
    System,
    En,
    Zh,
    Ko,
    Ja,
    Es,
}

impl UiLanguage {
    pub fn as_str(self) -> &'static str {
        match self {
            UiLanguage::System => UI_LANGUAGE_SYSTEM,
            UiLanguage::En => UI_LANGUAGE_ENGLISH,
            UiLanguage::Zh => UI_LANGUAGE_CHINESE,
            UiLanguage::Ko => UI_LANGUAGE_KOREAN,
            UiLanguage::Ja => UI_LANGUAGE_JAPANESE,
            UiLanguage::Es => UI_LANGUAGE_SPANISH,
        }
    }
}

/// `UI_LANGUAGE_VALUES.has(value) ? value : UI_LANGUAGE_SYSTEM`, exact-string
/// membership against the six values above. `None` (nothing supplied at all)
/// normalizes to `System`, same as any other non-member string.
pub fn normalize_ui_language(value: Option<&str>) -> UiLanguage {
    match value {
        Some(UI_LANGUAGE_SYSTEM) => UiLanguage::System,
        Some(UI_LANGUAGE_ENGLISH) => UiLanguage::En,
        Some(UI_LANGUAGE_CHINESE) => UiLanguage::Zh,
        Some(UI_LANGUAGE_KOREAN) => UiLanguage::Ko,
        Some(UI_LANGUAGE_JAPANESE) => UiLanguage::Ja,
        Some(UI_LANGUAGE_SPANISH) => UiLanguage::Es,
        _ => UiLanguage::System,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Oracle: ui-language.test.ts

    #[test]
    fn accepts_supported_language_settings() {
        assert_eq!(
            normalize_ui_language(Some(UI_LANGUAGE_SYSTEM)),
            UiLanguage::System
        );
        assert_eq!(
            normalize_ui_language(Some(UI_LANGUAGE_ENGLISH)),
            UiLanguage::En
        );
        assert_eq!(
            normalize_ui_language(Some(UI_LANGUAGE_CHINESE)),
            UiLanguage::Zh
        );
        assert_eq!(
            normalize_ui_language(Some(UI_LANGUAGE_KOREAN)),
            UiLanguage::Ko
        );
        assert_eq!(
            normalize_ui_language(Some(UI_LANGUAGE_JAPANESE)),
            UiLanguage::Ja
        );
        assert_eq!(
            normalize_ui_language(Some(UI_LANGUAGE_SPANISH)),
            UiLanguage::Es
        );
    }

    #[test]
    fn falls_back_unknown_values_to_system() {
        assert_eq!(normalize_ui_language(Some("fr")), UiLanguage::System);
        assert_eq!(normalize_ui_language(None), UiLanguage::System);
    }

    // Mandatory extra pins (oracle-silent):

    /// A looser matcher (trim / ASCII-lowercase / locale-tag split) would
    /// also pass both oracle cases above, since `'fr'` fails every one of
    /// those too. These pin exact membership against the specific inputs
    /// that a looser matcher — like the sibling `ui-locale.ts` normalizer —
    /// would incorrectly accept.
    #[test]
    fn pin_case_variants_and_locale_tags_fall_back_to_system() {
        assert_eq!(normalize_ui_language(Some("EN")), UiLanguage::System);
        assert_eq!(normalize_ui_language(Some("en-US")), UiLanguage::System);
        assert_eq!(normalize_ui_language(Some("ko_KR")), UiLanguage::System);
        assert_eq!(normalize_ui_language(Some(" en ")), UiLanguage::System);
        assert_eq!(normalize_ui_language(Some("")), UiLanguage::System);
        assert_eq!(normalize_ui_language(None), UiLanguage::System);
    }

    /// The set is closed at exactly six members; an unrelated valid-looking
    /// language code is not silently accepted.
    #[test]
    fn pin_closed_set_exact_membership() {
        assert_eq!(normalize_ui_language(Some("de")), UiLanguage::System);
        assert_eq!(
            [
                UI_LANGUAGE_SYSTEM,
                UI_LANGUAGE_ENGLISH,
                UI_LANGUAGE_CHINESE,
                UI_LANGUAGE_KOREAN,
                UI_LANGUAGE_JAPANESE,
                UI_LANGUAGE_SPANISH,
            ]
            .len(),
            6
        );
    }

    /// Every variant round-trips through `as_str()` to its matching constant.
    #[test]
    fn pin_as_str_round_trips_every_variant() {
        assert_eq!(UiLanguage::System.as_str(), UI_LANGUAGE_SYSTEM);
        assert_eq!(UiLanguage::En.as_str(), UI_LANGUAGE_ENGLISH);
        assert_eq!(UiLanguage::Zh.as_str(), UI_LANGUAGE_CHINESE);
        assert_eq!(UiLanguage::Ko.as_str(), UI_LANGUAGE_KOREAN);
        assert_eq!(UiLanguage::Ja.as_str(), UI_LANGUAGE_JAPANESE);
        assert_eq!(UiLanguage::Es.as_str(), UI_LANGUAGE_SPANISH);
    }

    /// `None` means "no value supplied" and normalizes to `System` — the
    /// caller's `??` fallback is never absorbed into this function.
    #[test]
    fn pin_none_normalizes_to_system_not_absorbing_caller_fallback() {
        assert_eq!(normalize_ui_language(None), UiLanguage::System);
    }
}
