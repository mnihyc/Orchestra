//! Wire format of personal and service-account API keys.
use orch_types::Id;

/// Mint the public row ID and its bearer together so lists, audit and revocation use the same ID.
#[must_use]
pub fn new_api_key() -> (Id, String) {
    let id = orch_types::new_id();
    (id, super::new_api_token(id))
}

#[cfg(test)]
mod tests {
    use super::super::API_TOKEN_PREFIX;
    use super::*;

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

    #[test]
    fn a_scanner_can_verify_the_printed_bytes_without_an_account() {
        // This fixed noncredential contains underscores in the encoded random field.
        let body = format!(
            "opat_00000000-0000-4000-8000-000000000000_{}8",
            "_".repeat(42)
        );
        assert_eq!(&checksum(body.as_bytes()), b"1w7ypa");
        assert!(orch_api_contract::api_keys::key_id(&format!("{body}1w7ypa")).is_some());
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
            assert!(
                orch_api_contract::api_keys::key_id(std::str::from_utf8(&altered).unwrap())
                    .is_none()
            );
        }
    }
}
