//! Inline `data:` URI for base64 image bytes — verbatim port of Orca's
//! `src/shared/image-data-uri.ts` (@ v1.4.150-rc.0).
//!
//! **No base64 encoding**: the caller-supplied (already-encoded) payload is
//! whitespace-stripped and concatenated verbatim — no decode/validate/re-encode.
//! The only encoding concern is the `/\s/g` strip, which uses the ECMAScript
//! whitespace set ([`crate::js_ws::is_js_whitespace`], **includes U+FEFF,
//! excludes U+0085**), NOT `char::is_whitespace`. The `image/` prefix test is
//! **case-sensitive** (`IMAGE/PNG` → `None`).

use crate::js_ws::is_js_whitespace;

/// Build `data:{mime};base64,{cleaned}`, or `None` when the mime is absent /
/// non-`image/*`, or the payload is empty after stripping JS whitespace.
pub fn build_image_data_uri(mime_type: Option<&str>, base64_content: &str) -> Option<String> {
    // `!mimeType?.startsWith('image/')` — absent mime short-circuits to null.
    let mime = mime_type?;
    if !mime.starts_with("image/") {
        return None;
    }
    // base64Content.replace(/\s/g, '')
    let cleaned: String = base64_content
        .chars()
        .filter(|&ch| !is_js_whitespace(ch))
        .collect();
    if cleaned.is_empty() {
        return None;
    }
    Some(format!("data:{mime};base64,{cleaned}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Oracle: image-data-uri.test.ts

    #[test]
    fn builds_a_data_uri_from_base64_image_bytes() {
        assert_eq!(
            build_image_data_uri(Some("image/png"), "bmV3").as_deref(),
            Some("data:image/png;base64,bmV3")
        );
    }

    #[test]
    fn strips_whitespace_from_line_wrapped_base64_payloads() {
        assert_eq!(
            build_image_data_uri(Some("image/png"), "bm\nV3\t bmV3\r\n").as_deref(),
            Some("data:image/png;base64,bmV3bmV3")
        );
    }

    #[test]
    fn returns_none_for_an_empty_payload() {
        assert_eq!(build_image_data_uri(Some("image/png"), "   \n"), None);
    }

    #[test]
    fn returns_none_for_a_missing_mime_type() {
        assert_eq!(build_image_data_uri(None, "bmV3"), None);
    }

    #[test]
    fn returns_none_for_application_pdf() {
        assert_eq!(build_image_data_uri(Some("application/pdf"), "JVBER"), None);
    }

    #[test]
    fn returns_none_for_a_non_image_mime() {
        assert_eq!(
            build_image_data_uri(Some("application/octet-stream"), "AAAA"),
            None
        );
    }

    // Mandatory extra pins (oracle-silent):

    /// U+FEFF (BOM) is JS whitespace → stripped from the payload.
    #[test]
    fn pin_bom_is_stripped() {
        assert_eq!(
            build_image_data_uri(Some("image/png"), "bm\u{FEFF}V3").as_deref(),
            Some("data:image/png;base64,bmV3")
        );
    }

    /// U+0085 (NEL) is NOT JS whitespace → kept in the payload (Rust
    /// `char::is_whitespace` would wrongly strip it).
    #[test]
    fn pin_nel_is_kept() {
        assert_eq!(
            build_image_data_uri(Some("image/png"), "bm\u{0085}V3").as_deref(),
            Some("data:image/png;base64,bm\u{0085}V3")
        );
    }

    /// The `image/` prefix is case-sensitive — uppercase mime is rejected.
    #[test]
    fn pin_uppercase_mime_rejected() {
        assert_eq!(build_image_data_uri(Some("IMAGE/PNG"), "bmV3"), None);
    }

    /// `image/` with an empty subtype still passes `startsWith`.
    #[test]
    fn pin_empty_subtype_passes() {
        assert_eq!(
            build_image_data_uri(Some("image/"), "bmV3").as_deref(),
            Some("data:image/;base64,bmV3")
        );
    }
}
