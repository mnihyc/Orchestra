//! Visible text normalization for values stored by PostgreSQL.

pub mod schema;

use crate::Json;
use std::borrow::Cow;
use std::collections::BTreeMap;

const EXACT_PREFIX: &str = "~orch.exact:";

/// A malformed private storage encoding, without its original contents.
#[derive(Debug, thiserror::Error)]
#[error("the stored operational value cannot be decoded")]
pub struct InvalidEncoding;

/// Losslessly represents an operational identity in a PostgreSQL text column.
pub fn encode_key(value: &str) -> String {
    if value.contains('\0') || value.starts_with(EXACT_PREFIX) {
        format!("{EXACT_PREFIX}{}", Json::String(value.to_owned()))
    } else {
        value.to_owned()
    }
}

/// Restores the exact identity; the escape namespace is itself escaped on write.
pub fn decode_key(value: &str) -> Result<String, InvalidEncoding> {
    match value.strip_prefix(EXACT_PREFIX) {
        Some(value) => serde_json::from_str(value).map_err(|_| InvalidEncoding),
        None => Ok(value.to_owned()),
    }
}

/// Keep the declared main/sub-agent namespace searchable while escaping its opaque suffix.
pub fn encode_stream(value: &str) -> String {
    value.strip_prefix("sub:").map_or_else(
        || encode_key(value),
        |suffix| format!("sub:{}", encode_key(suffix)),
    )
}

pub fn decode_stream(value: &str) -> Result<String, InvalidEncoding> {
    value.strip_prefix("sub:").map_or_else(
        || decode_key(value),
        |suffix| decode_key(suffix).map(|value| format!("sub:{value}")),
    )
}

/// Private operational JSON keeps its execution bytes; public item text uses normalize_json instead.
pub fn encode_exact(value: &Json) -> Json {
    match value {
        Json::String(value) => Json::String(encode_key(value)),
        Json::Array(values) => Json::Array(values.iter().map(encode_exact).collect()),
        Json::Object(values) => Json::Object(
            values
                .iter()
                .map(|(key, value)| (encode_key(key), encode_exact(value)))
                .collect(),
        ),
        _ => value.clone(),
    }
}

/// Decodes one storage layer without interpreting restored literal escape prefixes again.
pub fn decode_exact(value: Json) -> Result<Json, InvalidEncoding> {
    Ok(match value {
        Json::String(value) => Json::String(decode_key(&value)?),
        Json::Array(values) => Json::Array(
            values
                .into_iter()
                .map(decode_exact)
                .collect::<Result<_, _>>()?,
        ),
        Json::Object(values) => Json::Object(
            values
                .into_iter()
                .map(|(key, value)| Ok((decode_key(&key)?, decode_exact(value)?)))
                .collect::<Result<_, InvalidEncoding>>()?,
        ),
        other => other,
    })
}

/// The JSON form of an internal row uses the same safe representation as its database column.
pub mod stored_json {
    use super::*;
    pub fn serialize<S: serde::Serializer>(value: &Json, serializer: S) -> Result<S::Ok, S::Error> {
        serde::Serialize::serialize(&encode_exact(value), serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Json, D::Error> {
        decode_exact(serde::Deserialize::deserialize(deserializer)?)
            .map_err(serde::de::Error::custom)
    }
}

/// Optional reference metadata in a frozen internal row.
pub mod stored_key {
    use super::*;
    pub fn serialize<S: serde::Serializer>(
        value: &Option<String>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serde::Serialize::serialize(&value.as_deref().map(encode_key), serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<String>, D::Error> {
        let value: Option<String> = serde::Deserialize::deserialize(deserializer)?;
        value
            .as_deref()
            .map(decode_key)
            .transpose()
            .map_err(serde::de::Error::custom)
    }
}

pub mod stored_stream {
    use super::*;
    pub fn serialize<S: serde::Serializer>(value: &str, serializer: S) -> Result<S::Ok, S::Error> {
        serde::Serialize::serialize(&encode_stream(value), serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<String, D::Error> {
        let value: String = serde::Deserialize::deserialize(deserializer)?;
        decode_stream(&value).map_err(serde::de::Error::custom)
    }
}

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
#[allow(clippy::unwrap_used, reason = "regression assertions")]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn exact_operational_values_do_not_alias_repaired_text_or_literal_escape_prefixes() {
        let values = [
            "source\0id",
            "source�id",
            "~orch.exact:\"source\\u0000id\"",
            "ordinary",
        ];
        let encoded: Vec<_> = values.iter().map(|value| encode_key(value)).collect();
        assert_eq!(
            encoded
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            values.len()
        );
        for (original, stored) in values.iter().zip(encoded) {
            assert!(!stored.contains('\0'));
            assert_eq!(decode_key(&stored).unwrap(), *original);
        }
        let original =
            json!({"field\0":{"operand":"value\0"},"field�":"other","literal":values[2]});
        let stored = encode_exact(&original);
        assert!(!needs_repair(&stored));
        assert_eq!(decode_exact(stored).unwrap(), original);
    }

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
