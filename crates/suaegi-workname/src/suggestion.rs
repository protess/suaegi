//! Orca's globally de-duplicated marine-creature workspace suggestions.

use std::collections::HashSet;

use crate::js_ws::js_trim;
use crate::MARINE_CREATURES;

pub fn normalize_suggested_name(name: &str) -> String {
    js_trim(name).chars().flat_map(char::to_lowercase).collect()
}

pub fn should_apply_suggested_name(name: &str, previous_suggested_name: &str) -> bool {
    js_trim(name).is_empty() || name == previous_suggested_name
}

/// Pick the same candidate Orca would select for a supplied `Math.random()`-like
/// value. Callers provide existing workspace leaf names across every repo.
pub fn get_suggested_creature_name<'a>(
    used_names: impl IntoIterator<Item = &'a str>,
    random_unit: f64,
) -> String {
    let used: HashSet<String> = used_names
        .into_iter()
        .map(normalize_suggested_name)
        .collect();
    let pick = |items: &[String]| {
        let unit = if random_unit.is_finite() {
            random_unit.clamp(0.0, 1.0 - f64::EPSILON)
        } else {
            0.0
        };
        items[(unit * items.len() as f64).floor() as usize].clone()
    };

    let available: Vec<String> = MARINE_CREATURES
        .iter()
        .map(|name| normalize_suggested_name(name))
        .filter(|name| !used.contains(name))
        .collect();
    if !available.is_empty() {
        return pick(&available);
    }

    let mut suffix = 2usize;
    loop {
        let numbered: Vec<String> = MARINE_CREATURES
            .iter()
            .map(|name| format!("{}-{suffix}", normalize_suggested_name(name)))
            .filter(|name| !used.contains(name))
            .collect();
        if !numbered.is_empty() {
            return pick(&numbered);
        }
        suffix += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestions_are_global_normalized_and_randomly_indexed() {
        let first = normalize_suggested_name(MARINE_CREATURES[0]);
        let third = normalize_suggested_name(MARINE_CREATURES[2]);
        assert_eq!(get_suggested_creature_name([first.as_str()], 0.0), {
            normalize_suggested_name(MARINE_CREATURES[1])
        });
        assert_eq!(
            get_suggested_creature_name(std::iter::empty(), 2.0 / MARINE_CREATURES.len() as f64),
            third
        );
    }

    #[test]
    fn exhausted_base_names_move_to_numbered_variants() {
        let used: Vec<String> = MARINE_CREATURES
            .iter()
            .map(|name| normalize_suggested_name(name))
            .collect();
        assert_eq!(
            get_suggested_creature_name(used.iter().map(String::as_str), 0.0),
            format!("{}-2", normalize_suggested_name(MARINE_CREATURES[0]))
        );
    }

    #[test]
    fn suggestion_only_replaces_blank_or_the_previous_suggestion() {
        assert!(should_apply_suggested_name(" ", "cunner"));
        assert!(should_apply_suggested_name("cunner", "cunner"));
        assert!(!should_apply_suggested_name("my-work", "cunner"));
    }
}
