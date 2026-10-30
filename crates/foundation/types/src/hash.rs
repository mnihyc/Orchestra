//! Content hashes and their representation.
//!
//! Content is named by its BLAKE3-256 hash, written as 64 lowercase
//! hexadecimal characters.

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The BLAKE3-256 hash of some content.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Hash([u8; 32]);

/// A text that is not a hash.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("a hash is 64 lowercase hexadecimal characters")]
pub struct NotAHash;

impl Hash {
    /// The hash of `content`.
    #[must_use]
    pub fn of(content: &[u8]) -> Self {
        Self(*blake3::hash(content).as_bytes())
    }

    /// The hash whose 32 bytes are `bytes`.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The hash stored as `bytes`, where they are 32.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, NotAHash> {
        <[u8; 32]>::try_from(bytes).map(Self).map_err(|_| NotAHash)
    }

    /// Its 32 bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// A hasher for content that arrives in pieces; finish it with
    /// [`Hash::finish`].
    #[must_use]
    pub fn hasher() -> blake3::Hasher {
        blake3::Hasher::new()
    }

    /// The hash of everything `hasher` was given.
    #[must_use]
    pub fn finish(hasher: &blake3::Hasher) -> Self {
        Self(*hasher.finalize().as_bytes())
    }
}

impl std::fmt::Display for Hash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&hex::encode(self.0))
    }
}

impl std::fmt::Debug for Hash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Hash({self})")
    }
}

impl std::str::FromStr for Hash {
    type Err = NotAHash;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.len() != 64
            || text
                .bytes()
                .any(|b| !matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err(NotAHash);
        }
        let mut bytes = [0u8; 32];
        hex::decode_to_slice(text, &mut bytes).map_err(|_| NotAHash)?;
        Ok(Self(bytes))
    }
}

impl Serialize for Hash {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Hash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for Hash {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Hash".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({ "type": "string", "pattern": "^[0-9a-f]{64}$" })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn written_as_64_lowercase_hex_characters() {
        let hash = Hash::of(b"orch");
        let text = hash.to_string();
        assert_eq!(text.len(), 64);
        assert_eq!(text.parse::<Hash>().unwrap(), hash);
        assert_eq!(serde_json::to_string(&hash).unwrap(), format!("\"{text}\""));
    }

    #[test]
    fn upper_case_and_short_texts_are_refused() {
        let text = Hash::of(b"orch").to_string();
        assert_eq!(text.to_uppercase().parse::<Hash>(), Err(NotAHash));
        assert_eq!(text[..62].parse::<Hash>(), Err(NotAHash));
    }

    #[test]
    fn pieces_hash_as_the_whole() {
        let mut hasher = Hash::hasher();
        hasher.update(b"or");
        hasher.update(b"ch");
        assert_eq!(Hash::finish(&hasher), Hash::of(b"orch"));
    }
}
