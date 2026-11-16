//! Visible text normalization for values stored by PostgreSQL.

use crate::Json;
use std::borrow::Cow;
use std::collections::BTreeMap;

/// Borrows ordinary content; repairs a copy only when the source needs it.
pub fn json(value: &Json) -> Cow<'_, Json> {
    json_with_count(value).0
}

/// The same boundary with the receipt needed by the value's storage owner.
pub fn json_with_count(value: &Json) -> (Cow<'_, Json>, u64) {
    if needs_repair(value) {
        let mut repaired = value.clone();
        let count = normalize_json(&mut repaired);
        (Cow::Owned(repaired), count)
    } else {
        (Cow::Borrowed(value), 0)
    }
}

/// Whether the public representation will change, including an operational operand's address.
pub fn needs_repair(value: &Json) -> bool {
    match value {
        Json::String(text) => text.contains('\0'),
        Json::Array(values) => values.iter().any(needs_repair),
        Json::Object(values) => values
            .iter()
            .any(|(key, value)| key.contains('\0') || needs_repair(value)),
        _ => false,
    }
}

/// Replaces characters PostgreSQL text cannot represent, returning their exact count.
pub fn normalize_text(text: &mut String) -> u64 {
    let count = u64::try_from(text.bytes().filter(|byte| *byte == 0).count()).unwrap_or(u64::MAX);
    if count != 0 {
        *text = text.replace('\0', "\u{fffd}");
    }
    count
}

/// Normalizes strings and object keys while retaining every original member.
pub fn normalize_json(value: &mut Json) -> u64 {
    match value {
        Json::String(text) => normalize_text(text),
        Json::Array(values) => values.iter_mut().map(normalize_json).sum(),
        Json::Object(members) => {
            let names: Vec<String> = members
                .keys()
                .filter(|name| name.contains('\0'))
                .cloned()
                .collect();
            let mut count = 0;
            let mut ordinals = BTreeMap::new();
            for original in names {
                let Some(value) = members.remove(&original) else {
                    continue;
                };
                let mut name = original;
                count += normalize_text(&mut name);
                let mut unique = name.clone();
                let ordinal = ordinals.entry(name.clone()).or_insert(2usize);
                while members.contains_key(&unique) {
                    unique = format!("{name} ({ordinal})");
                    *ordinal += 1;
                }
                members.insert(unique, value);
            }
            count + members.values_mut().map(normalize_json).sum::<u64>()
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn embedded_zeroes_are_replaced_in_every_string_and_key_without_overwriting_other_values() {
        let mut value = json!({"a\0":{"text":"left\0right","nested":["\0",true,42]},"a\u{fffd}":"unchanged","a\u{fffd} (2)":"also unchanged","escaped":"\\u0000"});
        assert_eq!(normalize_json(&mut value), 3);
        assert_eq!(value["a\u{fffd}"], "unchanged");
        assert_eq!(value["a\u{fffd} (2)"], "also unchanged");
        assert_eq!(value["a\u{fffd} (3)"]["text"], "left\u{fffd}right");
        assert_eq!(
            value["a\u{fffd} (3)"]["nested"],
            json!(["\u{fffd}", true, 42])
        );
        assert_eq!(value["escaped"], "\\u0000");
        assert_eq!(normalize_json(&mut value), 0);
    }
}
