//! The canonical public encoding of an identity-bound API key.

use orch_types::Id;

const PREFIX: &str = crate::surface::KEY_TOKEN_PREFIX;
const ID_BYTES: usize = 36; // The lowercase hyphenated UUID shown in the key list.
const SECRET_BYTES: usize = 32;
const SECRET_TEXT: usize = 43;
const CHECKSUM_TEXT: usize = 6;
const PAYLOAD_BYTES: usize = PREFIX.len() + ID_BYTES + 1 + SECRET_TEXT;
const KEY_BYTES: usize = PAYLOAD_BYTES + CHECKSUM_TEXT;
const BASE62: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Encode one key's own UUID and exactly 256 secret bits, with a checksum over all preceding text.
/// Entropy is supplied by the credential issuer; this encoding never generates it or stores a secret.
#[must_use]
pub fn encode(id: Id, secret: &[u8; SECRET_BYTES]) -> String {
    let payload = format!("{PREFIX}{id}_{}", secret_text(secret));
    format!("{payload}{}", checksum(crc32(payload.as_bytes())))
}

/// Validate width, alphabet, canonical UUID/base64url spelling and CRC32 before any storage access.
/// Only the public identity leaves this parser; the bearer secret is never returned or logged.
#[must_use]
pub fn key_id(value: &str) -> Option<Id> {
    if value.len() != KEY_BYTES || !value.is_ascii() || !value.starts_with(PREFIX) {
        return None;
    }
    let bytes = value.as_bytes();
    let identity = &value[PREFIX.len()..PREFIX.len() + ID_BYTES];
    if bytes[PREFIX.len() + ID_BYTES] != b'_'
        || !identity
            .bytes()
            .enumerate()
            .all(|(position, byte)| match position {
                8 | 13 | 18 | 23 => byte == b'-',
                _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte),
            })
    {
        return None;
    }
    let payload = &value[..PAYLOAD_BYTES];
    if &value[PAYLOAD_BYTES..] != checksum(crc32(payload.as_bytes())).as_str() {
        return None;
    }
    let encoded = &value[PREFIX.len() + ID_BYTES + 1..PAYLOAD_BYTES];
    if !encoded.bytes().all(|byte| base64_value(byte).is_some())
        || base64_value(*encoded.as_bytes().last()?)? & 3 != 0
    {
        return None;
    }
    Id::parse_str(identity).ok()
}

/// Exactly 32 bytes need ten complete base64url groups and one two-byte group.
fn secret_text(secret: &[u8; SECRET_BYTES]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut text = String::with_capacity(SECRET_TEXT);
    for group in secret.chunks(3) {
        let first = group[0];
        let second = group.get(1).copied().unwrap_or(0);
        let third = group.get(2).copied().unwrap_or(0);
        text.push(char::from(ALPHABET[usize::from(first >> 2)]));
        text.push(char::from(
            ALPHABET[usize::from((first & 3) << 4 | second >> 4)],
        ));
        if group.len() > 1 {
            text.push(char::from(
                ALPHABET[usize::from((second & 15) << 2 | third >> 6)],
            ));
        }
        if group.len() > 2 {
            text.push(char::from(ALPHABET[usize::from(third & 63)]));
        }
    }
    text
}

fn base64_value(byte: u8) -> Option<u8> {
    Some(match byte {
        b'A'..=b'Z' => byte - b'A',
        b'a'..=b'z' => byte - b'a' + 26,
        b'0'..=b'9' => byte - b'0' + 52,
        b'-' => 62,
        b'_' => 63,
        _ => return None,
    })
}

/// CRC-32/ISO-HDLC, as used by checksum-bearing secret formats. Its fixed input is one short key.
fn crc32(bytes: &[u8]) -> u32 {
    let mut state = u32::MAX;
    for &byte in bytes {
        state ^= u32::from(byte);
        for _ in 0..8 {
            state = (state >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(state & 1));
        }
    }
    !state
}

fn checksum(mut value: u32) -> String {
    let mut text = [b'0'; CHECKSUM_TEXT];
    for byte in text.iter_mut().rev() {
        *byte = BASE62[(value % 62) as usize];
        value /= 62;
    }
    text.into_iter().map(char::from).collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "a case fails by panicking")]
    use super::*;
    use orch_types::Id;

    fn id() -> Id {
        "00112233-4455-6677-8899-aabbccddeeff".parse().unwrap()
    }

    #[test]
    fn an_api_key_names_its_own_identity_and_preserves_all_thirty_two_secret_bytes() {
        let secret = std::array::from_fn(|index| index as u8);
        let key = encode(id(), &secret);
        assert_eq!(
            key,
            "opat_00112233-4455-6677-8899-aabbccddeeff_AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh83iWi3N"
        );
        assert_eq!(key_id(&key), Some(id()));
        assert_ne!(encode(id(), &[0; 32]), encode(id(), &[1; 32]));
    }

    #[test]
    fn the_checksum_has_the_standard_crc_vector_and_six_canonical_base62_digits() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(checksum(0), "000000");
        assert_eq!(checksum(u32::MAX), "4gfFC3");
    }

    #[test]
    fn every_single_character_typo_is_refused_without_looking_up_a_key() {
        let key = encode(id(), &[7; 32]);
        for index in 0..key.len() {
            let mut changed = key.as_bytes().to_vec();
            changed[index] = if changed[index] == b'0' { b'1' } else { b'0' };
            assert_eq!(
                key_id(std::str::from_utf8(&changed).unwrap()),
                None,
                "byte {index}"
            );
        }
    }

    #[test]
    fn a_valid_checksum_does_not_make_an_old_noncanonical_or_malformed_encoding_valid() {
        let key = encode(id(), &[0; 32]);
        for malformed in [
            "opat_synthetic".to_owned(),
            format!("{key}x"),
            key[..key.len() - 1].to_owned(),
            key.replace("opat_", "oat_"),
        ] {
            assert_eq!(key_id(&malformed), None);
        }
        let payload = &key[..key.len() - 6];
        for malformed in [
            payload.replacen("aabb", "AABB", 1),
            payload.replacen("00112233-4455", "001122334455", 1),
            payload.replacen("0011", "z011", 1),
            format!("{}B", &payload[..payload.len() - 1]),
        ] {
            let valid_crc = format!("{malformed}{}", checksum(crc32(malformed.as_bytes())));
            assert_eq!(key_id(&valid_crc), None);
        }
        let overflow = format!("{payload}zzzzzz");
        assert_eq!(key_id(&overflow), None);
    }
}
