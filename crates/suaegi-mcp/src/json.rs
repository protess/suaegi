//! VERBATIM port of the value/coercion layer of Orca's
//! `src/shared/mcp-config.ts` (@ v1.4.150-rc.0), milestone M2a.
//!
//! Ported: the order-preserving [`JsonValue`] type + [`parse_json`],
//! [`js_string_of`] (ECMAScript `String()`, `O:149`), and the ECMAScript
//! `Number::toString` formatter ([`JsonNumber::to_ecmascript_string`]).
//!
//! Deferred to M2b: `extract_object_at_path` (`O:170-184`), `summarize_mcp_server`
//! (`O:186-241`), `read_command`/`read_url`/`resolve_transport` (`O:243-275`),
//! `inspect_mcp_config_content` (`O:108-140`), and the
//! `McpServerSummary`/`McpConfigInspection`/`McpServerStatus`/
//! `McpServerTransport` types.
//!
//! # W1 — order-preserving value type
//! `serde_json::Value`'s `Map` is a `BTreeMap`, which silently re-sorts object
//! keys — fatal for a port whose oracle asserts document order (`T:49-75`).
//! [`JsonValue::Object`] is instead a `Vec<(String, JsonValue)>`, filled by a
//! hand-written [`serde::de::Visitor`] (no `#[derive(Deserialize)]`).
//! `serde_json`'s parser hands map entries to `visit_map` in document order
//! regardless of the `preserve_order` cargo feature (which this crate does
//! NOT enable — see `Cargo.toml`), so the visitor recovers that order for
//! free; W2/W3 below then reshape it to match JS's actual enumeration order.

use std::fmt;

use serde::de::{Deserializer, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;

// ---------------------------------------------------------------------------
// JsonNumber / JsonValue
// ---------------------------------------------------------------------------

/// A parsed JSON number, tagged by which `serde_json` visitor callback
/// produced it (`visit_u64`/`visit_i64`/`visit_f64`). JS itself has only one
/// numeric type (a double), but preserving the callback's shape lets integers
/// use their own exact `to_string()` (W5: "for integers use the integer's own
/// `to_string()`") instead of round-tripping through `f64`, which would be
/// lossy above 2^53.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum JsonNumber {
    UInt(u64),
    Int(i64),
    Float(f64),
}

impl JsonNumber {
    /// `O:149`'s `String(rawValue)` applied to a number — ECMAScript
    /// `Number::toString` (ECMA-262 §6.1.6.1.20), NOT Rust's `f64::to_string`
    /// (which never emits exponential notation and prints `-0` instead of
    /// `0`).
    pub fn to_ecmascript_string(&self) -> String {
        match self {
            JsonNumber::UInt(v) => v.to_string(),
            JsonNumber::Int(v) => v.to_string(),
            JsonNumber::Float(v) => format_ecmascript_float(*v),
        }
    }
}

/// `O:…` — order-preserving JSON value (W1). [`JsonValue::Object`] is a
/// `Vec<(String, JsonValue)>` in JS enumeration order (index keys first,
/// ascending numeric — W2 — then the rest in first-seen insertion order —
/// W3's "first position, last value" applies to *both* buckets).
#[derive(Debug, Clone, PartialEq)]
pub enum JsonValue {
    Null,
    Bool(bool),
    Number(JsonNumber),
    String(String),
    Array(Vec<JsonValue>),
    Object(Vec<(String, JsonValue)>),
}

impl<'de> Deserialize<'de> for JsonValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(JsonValueVisitor)
    }
}

struct JsonValueVisitor;

impl<'de> Visitor<'de> for JsonValueVisitor {
    type Value = JsonValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_unit<E>(self) -> Result<JsonValue, E> {
        Ok(JsonValue::Null)
    }

    fn visit_none<E>(self) -> Result<JsonValue, E> {
        Ok(JsonValue::Null)
    }

    fn visit_bool<E>(self, value: bool) -> Result<JsonValue, E> {
        Ok(JsonValue::Bool(value))
    }

    fn visit_u64<E>(self, value: u64) -> Result<JsonValue, E> {
        Ok(JsonValue::Number(JsonNumber::UInt(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<JsonValue, E> {
        Ok(JsonValue::Number(JsonNumber::Int(value)))
    }

    fn visit_f64<E>(self, value: f64) -> Result<JsonValue, E> {
        Ok(JsonValue::Number(JsonNumber::Float(value)))
    }

    fn visit_str<E>(self, value: &str) -> Result<JsonValue, E> {
        Ok(JsonValue::String(value.to_string()))
    }

    fn visit_string<E>(self, value: String) -> Result<JsonValue, E> {
        Ok(JsonValue::String(value))
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<JsonValue, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element::<JsonValue>()? {
            items.push(item);
        }
        Ok(JsonValue::Array(items))
    }

    fn visit_map<A>(self, mut map: A) -> Result<JsonValue, A::Error>
    where
        A: MapAccess<'de>,
    {
        // W3 — "first position, last value": `serde`'s `MapAccess` hands us
        // BOTH entries of a JS-duplicate key (`{"a":1,"b":2,"a":3}`); a naive
        // `Vec::push` for every entry would duplicate the row. Overwrite the
        // value in place at the key's ORIGINAL position instead.
        let mut entries: Vec<(String, JsonValue)> = Vec::new();
        while let Some((key, value)) = map.next_entry::<String, JsonValue>()? {
            match entries
                .iter_mut()
                .find(|(existing_key, _)| *existing_key == key)
            {
                Some(existing) => existing.1 = value,
                None => entries.push((key, value)),
            }
        }
        Ok(JsonValue::Object(reorder_object_keys(entries)))
    }
}

// ---------------------------------------------------------------------------
// W2 — JS own-enumerable-property order: canonical array-index keys first
// (ascending numeric), then the rest in insertion order.
// ---------------------------------------------------------------------------

/// `O:138`/`O:148` (`servers`/`env`, both consumed via `Object.entries`) —
/// reshapes document order into JS enumeration order. Applied to EVERY
/// object, since `visit_map` calls this once per parsed JSON object
/// (including nested ones).
fn reorder_object_keys(entries: Vec<(String, JsonValue)>) -> Vec<(String, JsonValue)> {
    let mut index_positions: Vec<(u32, usize)> = Vec::new();
    let mut rest_positions: Vec<usize> = Vec::new();
    for (position, (key, _)) in entries.iter().enumerate() {
        match canonical_array_index(key) {
            Some(index) => index_positions.push((index, position)),
            None => rest_positions.push(position),
        }
    }
    index_positions.sort_by_key(|&(index, _)| index);

    // `Option::take` lets us move each `JsonValue` out exactly once without
    // cloning, regardless of which bucket claims it.
    let mut slots: Vec<Option<(String, JsonValue)>> = entries.into_iter().map(Some).collect();
    let mut result = Vec::with_capacity(slots.len());
    for (_, position) in index_positions {
        result.push(slots[position].take().expect("index position visited once"));
    }
    for position in rest_positions {
        result.push(slots[position].take().expect("rest position visited once"));
    }
    result
}

/// `O:138`/`O:148` array-index-key predicate: a decimal string, ASCII digits
/// only, no leading zero (except the literal `"0"`), non-empty, in
/// `0..=4294967294` (`2^32 - 2`, JS's `ToUint32(key) !== 2^32 - 1` boundary
/// for array indices). `"-1"`, `"1.5"`, `"01"`, and `"4294967295"` all fail
/// this and fall into the insertion-order bucket instead.
fn canonical_array_index(key: &str) -> Option<u32> {
    if key.is_empty() || !key.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    if key.len() > 1 && key.starts_with('0') {
        return None;
    }
    let value: u64 = key.parse().ok()?;
    if value <= 4_294_967_294 {
        Some(value as u32)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// parse_json
// ---------------------------------------------------------------------------

/// `JSON.parse` entry point. M2b will wrap this error for the `invalid`
/// status path (`O:117-127`, contract decision X8); M2a exposes it directly
/// so the parser has a real (non-dead-code) call site.
pub fn parse_json(input: &str) -> Result<JsonValue, serde_json::Error> {
    serde_json::from_str(input)
}

// ---------------------------------------------------------------------------
// W4 — js_string_of: ECMAScript `String()` applied to a JSON value
// ---------------------------------------------------------------------------

/// `O:149`'s `String(rawValue)` coercion, verbatim: `Value::to_string()` (the
/// JSON debug/display form) is wrong for every non-string variant here.
/// Concretely: `null` -> `"null"`; objects -> the literal `"[object Object]"`
/// (never their contents); arrays -> `Array.prototype.join(',')`, where a
/// `null`/`undefined` element contributes an EMPTY string (not `"null"`) and
/// nested arrays recurse through this same join.
pub fn js_string_of(value: &JsonValue) -> String {
    match value {
        JsonValue::Null => "null".to_string(),
        JsonValue::Bool(b) => b.to_string(),
        JsonValue::Number(n) => n.to_ecmascript_string(),
        JsonValue::String(s) => s.clone(),
        JsonValue::Object(_) => "[object Object]".to_string(),
        JsonValue::Array(items) => items
            .iter()
            .map(|item| match item {
                JsonValue::Null => String::new(),
                other => js_string_of(other),
            })
            .collect::<Vec<_>>()
            .join(","),
    }
}

// ---------------------------------------------------------------------------
// W5 — ECMAScript Number::toString for the Float case
// ---------------------------------------------------------------------------

/// ECMA-262 §6.1.6.1.20 `Number::toString(x, 10)`, restricted to the finite
/// values that can come out of JSON (`serde_json` never yields NaN/Infinity).
/// Rust's `f64::to_string()` diverges on two axes this function corrects:
/// it never emits exponential notation (`1e21` -> Rust's
/// `"1000000000000000000000"`), and it prints `"-0"` for negative zero
/// instead of `"0"`.
///
/// Approach: Rust's `{:e}` formatting (`LowerExp`) already computes the
/// shortest round-tripping decimal digit string — the same guarantee
/// `Number::toString` relies on — so this function only needs to re-thread
/// ECMA's placement rules (plain decimal vs. exponential, and where the
/// decimal point / zero-padding goes) around those digits, rather than
/// deriving them itself.
fn format_ecmascript_float(value: f64) -> String {
    if value == 0.0 {
        // Covers +0.0 and -0.0: IEEE-754 equality treats them as equal, and
        // ECMAScript's Number::toString maps BOTH to the string "0".
        return "0".to_string();
    }
    if value < 0.0 {
        return format!("-{}", format_ecmascript_float(-value));
    }

    // `value` is finite, strictly positive here.
    let exponential = format!("{value:e}");
    let (mantissa, exponent_str) = exponential
        .split_once('e')
        .expect("LowerExp output always contains an 'e'");
    let digits: String = mantissa.chars().filter(|&c| c != '.').collect();
    let digit_count = digits.len() as i64;
    let exponent: i64 = exponent_str
        .parse()
        .expect("LowerExp exponent is a valid integer");
    // `n` per ECMA-262: s * 10^(n - k) == value, where s = digits (k digits).
    let n = exponent + 1;

    if digit_count <= n && n <= 21 {
        let trailing_zeros = "0".repeat((n - digit_count) as usize);
        format!("{digits}{trailing_zeros}")
    } else if 0 < n && n <= 21 {
        let split_at = n as usize;
        format!("{}.{}", &digits[..split_at], &digits[split_at..])
    } else if -6 < n && n <= 0 {
        let leading_zeros = "0".repeat((-n) as usize);
        format!("0.{leading_zeros}{digits}")
    } else {
        let displayed_exponent = n - 1;
        let sign = if displayed_exponent >= 0 { '+' } else { '-' };
        if digit_count == 1 {
            format!("{digits}e{sign}{}", displayed_exponent.abs())
        } else {
            format!(
                "{}.{}e{sign}{}",
                &digits[..1],
                &digits[1..],
                displayed_exponent.abs()
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- W1/W2/W3: order-preserving parse ------------------------------------

    fn object_keys(value: &JsonValue) -> Vec<String> {
        match value {
            JsonValue::Object(entries) => entries.iter().map(|(k, _)| k.clone()).collect(),
            other => panic!("expected an object, got {other:?}"),
        }
    }

    #[test]
    fn w2_index_keys_are_hoisted_ascending_then_insertion_order_rest() {
        let parsed = parse_json(
            r#"{"zebra":1,"2":2,"alpha":3,"10":4,"1":5,"-1":6,"1.5":7,"01":8,"4294967295":9,"4294967294":10}"#,
        )
        .expect("valid JSON");
        assert_eq!(
            object_keys(&parsed),
            vec![
                "1",
                "2",
                "10",
                "4294967294",
                "zebra",
                "alpha",
                "-1",
                "1.5",
                "01",
                "4294967295",
            ]
        );
    }

    #[test]
    fn w2_hoisting_applies_to_nested_objects_too() {
        let parsed = parse_json(r#"{"outer":{"b":1,"2":2,"a":3}}"#).expect("valid JSON");
        let JsonValue::Object(entries) = &parsed else {
            panic!("expected object");
        };
        let (_, inner) = &entries[0];
        assert_eq!(object_keys(inner), vec!["2", "b", "a"]);
    }

    #[test]
    fn w3_duplicate_key_keeps_first_position_but_last_value() {
        let parsed = parse_json(r#"{"a":1,"b":2,"a":3}"#).expect("valid JSON");
        assert_eq!(object_keys(&parsed), vec!["a", "b"]);
        let JsonValue::Object(entries) = &parsed else {
            panic!("expected object");
        };
        assert_eq!(entries[0].1, JsonValue::Number(JsonNumber::UInt(3)));
    }

    // -- W4: js_string_of (ECMAScript String()) ------------------------------

    #[test]
    fn w4_null_is_the_literal_string_null() {
        assert_eq!(js_string_of(&parse_json("null").unwrap()), "null");
    }

    #[test]
    fn w4_true_is_the_literal_string_true() {
        assert_eq!(js_string_of(&parse_json("true").unwrap()), "true");
    }

    #[test]
    fn w4_object_is_object_object_never_its_contents() {
        assert_eq!(
            js_string_of(&parse_json(r#"{"x":1}"#).unwrap()),
            "[object Object]"
        );
        assert_eq!(
            js_string_of(&parse_json("[{}]").unwrap()),
            "[object Object]"
        );
    }

    #[test]
    fn w4_array_joins_elements_with_commas() {
        assert_eq!(js_string_of(&parse_json("[1,2]").unwrap()), "1,2");
    }

    #[test]
    fn w4_empty_array_is_empty_string() {
        assert_eq!(js_string_of(&parse_json("[]").unwrap()), "");
    }

    #[test]
    fn w4_nested_arrays_recurse_through_the_same_join() {
        assert_eq!(js_string_of(&parse_json("[[1,2],[3]]").unwrap()), "1,2,3");
    }

    #[test]
    fn w4_null_array_elements_contribute_empty_string_not_the_word_null() {
        assert_eq!(js_string_of(&parse_json("[null,null,1]").unwrap()), ",,1");
    }

    // -- W5: ECMAScript Number::toString threshold cases ---------------------

    fn ecmascript_float(value: f64) -> String {
        format_ecmascript_float(value)
    }

    #[test]
    fn w5_1e21_switches_to_exponential_with_explicit_plus_sign() {
        assert_eq!(ecmascript_float(1e21), "1e+21");
    }

    #[test]
    fn w5_1e_minus_7_switches_to_exponential() {
        assert_eq!(ecmascript_float(1e-7), "1e-7");
    }

    #[test]
    fn w5_1e_minus_6_stays_plain_decimal() {
        assert_eq!(ecmascript_float(1e-6), "0.000001");
    }

    #[test]
    fn w5_1e20_stays_plain_decimal_integer() {
        assert_eq!(ecmascript_float(1e20), "100000000000000000000");
    }

    #[test]
    fn w5_negative_zero_is_the_string_zero() {
        assert_eq!(ecmascript_float(-0.0_f64), "0");
    }

    #[test]
    fn w5_0_1_is_plain_decimal() {
        assert_eq!(ecmascript_float(0.1), "0.1");
    }

    #[test]
    fn w5_100_has_no_decimal_point() {
        assert_eq!(ecmascript_float(100.0), "100");
    }

    #[test]
    fn w5_1_5_keeps_a_single_fractional_digit() {
        assert_eq!(ecmascript_float(1.5), "1.5");
    }

    #[test]
    fn w5_5e_minus_324_is_the_smallest_denormal() {
        assert_eq!(ecmascript_float(5e-324), "5e-324");
    }

    #[test]
    fn w5_seventeen_significant_digits_round_trip_in_exponential_form() {
        assert_eq!(
            ecmascript_float(1.2345678901234568e29),
            "1.2345678901234568e+29"
        );
    }

    #[test]
    fn w5_integers_use_their_own_to_string_not_the_float_path() {
        // `1` parses as a `u64` (visit_u64), `1.0` as an `f64` (visit_f64);
        // both must format to `"1"`.
        assert_eq!(js_string_of(&parse_json("1").unwrap()), "1");
        assert_eq!(js_string_of(&parse_json("1.0").unwrap()), "1");
        assert_eq!(js_string_of(&parse_json("-3").unwrap()), "-3");
    }
}
