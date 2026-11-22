//! Tool arguments are arbitrary JSON. Reject duplicate members before a map would discard them.
use serde::de::{Error, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Number};
use std::fmt;

pub(super) struct Value(pub serde_json::Value);

impl<'de> Deserialize<'de> for Value {
    fn deserialize<D: Deserializer<'de>>(input: D) -> Result<Self, D::Error> {
        input.deserialize_any(Argument)
    }
}
struct Argument;
impl<'de> Visitor<'de> for Argument {
    type Value = Value;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("JSON with distinct object members")
    }
    fn visit_bool<E: Error>(self, v: bool) -> Result<Value, E> {
        Ok(Value(v.into()))
    }
    fn visit_i64<E: Error>(self, v: i64) -> Result<Value, E> {
        Ok(Value(v.into()))
    }
    fn visit_u64<E: Error>(self, v: u64) -> Result<Value, E> {
        Ok(Value(v.into()))
    }
    fn visit_f64<E: Error>(self, v: f64) -> Result<Value, E> {
        Ok(Value(
            Number::from_f64(v)
                .ok_or_else(|| E::custom("invalid number"))?
                .into(),
        ))
    }
    fn visit_str<E: Error>(self, v: &str) -> Result<Value, E> {
        Ok(Value(v.into()))
    }
    fn visit_unit<E: Error>(self) -> Result<Value, E> {
        Ok(Value(serde_json::Value::Null))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(Value(v)) = seq.next_element()? {
            values.push(v);
        }
        Ok(Value(values.into()))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut values = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(A::Error::custom("duplicate tool argument"));
            }
            values.insert(key, map.next_value::<Value>()?.0);
        }
        Ok(Value(values.into()))
    }
}
