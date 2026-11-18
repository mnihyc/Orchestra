//! Wire format of personal and service-account API keys.
use super::API_TOKEN_PREFIX;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use orch_types::Id;

const ID_LEN: usize = 36; // The same lowercase hyphenated UUID returned in the key list.
const SECRET_BYTES: usize = 32;
const SECRET_LEN: usize = 43; // 256 bits in unpadded base64url.
const CHECK_LEN: usize = 6; // Six base62 digits hold every unsigned CRC32.
const DIGITS: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

fn checksum(body: &[u8]) -> [u8; CHECK_LEN] {
    let mut crc = crc32fast::hash(body);
    let mut digits = [b'0'; CHECK_LEN];
    for digit in digits.iter_mut().rev() {
        *digit = DIGITS[(crc % 62) as usize];
        crc /= 62;
    }
    digits
}

/// Mint the public row ID and its bearer together so lists, audit and revocation use the same ID.
#[must_use]
pub fn new_api_key() -> (Id, String) {
    let id = orch_types::new_id();
    let random: [u8; SECRET_BYTES] = rand::random();
    let mut key = format!("{API_TOKEN_PREFIX}{id}_{}", URL_SAFE_NO_PAD.encode(random));
    let check = checksum(key.as_bytes());
    key.extend(check.into_iter().map(char::from));
    (id, key)
}

/// Validate the entire external representation before acquiring a database connection.
pub(super) fn valid(key: &str) -> bool {
    let split = API_TOKEN_PREFIX.len() + ID_LEN + 1 + SECRET_LEN;
    if key.len() != split + CHECK_LEN || !key.is_ascii() {
        return false;
    }
    let (body, check) = key.split_at(split);
    let Some((id, secret)) = body
        .strip_prefix(API_TOKEN_PREFIX)
        .and_then(|rest| rest.split_once('_'))
    else {
        return false;
    };
    id.len() == ID_LEN
        && !id.bytes().any(|byte| byte.is_ascii_uppercase())
        && Id::parse_str(id).is_ok()
        && secret.len() == SECRET_LEN
        && checksum(body.as_bytes()).as_slice() == check.as_bytes()
        && URL_SAFE_NO_PAD
            .decode(secret)
            .is_ok_and(|decoded| decoded.len() == SECRET_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scanner_can_verify_the_printed_bytes_without_an_account() {
        // This fixed noncredential contains underscores in the encoded random field.
        let body = format!(
            "opat_00000000-0000-4000-8000-000000000000_{}8",
            "_".repeat(42)
        );
        assert_eq!(&checksum(body.as_bytes()), b"1w7ypa");
        assert!(valid(&format!("{body}1w7ypa")));
        assert_eq!(checksum(b""), *b"000000");
    }

    #[test]
    fn a_matching_checksum_still_requires_a_canonical_id_and_secret() {
        let (_, key) = new_api_key();
        let body = &key[..key.len() - CHECK_LEN];
        for (index, byte) in [
            (API_TOKEN_PREFIX.len(), b'F'),
            (body.len() - 1, b'/'),
            (body.len() - 1, b'B'),
        ] {
            let mut altered = body.as_bytes().to_vec();
            altered[index] = byte;
            altered.extend_from_slice(&checksum(&altered));
            assert!(!valid(std::str::from_utf8(&altered).unwrap()));
        }
    }
}
