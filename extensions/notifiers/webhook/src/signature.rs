//! HMAC payload signatures.
//!
//! A webhook that has a secret signs what it posts: the header
//! `X-Orch-Signature` holds `sha256=` and the hexadecimal HMAC-SHA256 of the
//! body, exactly as it is sent, under the secret. The receiver computes the
//! same over the bytes it received and compares in constant time.

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

/// The header that carries the signature.
pub const HEADER: &str = "X-Orch-Signature";

/// What the value of the header starts with: the algorithm.
const PREFIX: &str = "sha256=";

type HmacSha256 = Hmac<Sha256>;

/// The value of the signature header for `body` under `secret`.
#[must_use]
pub fn sign(secret: &[u8], body: &[u8]) -> String {
    let mut mac = mac(secret);
    mac.update(body);
    format!("{PREFIX}{}", hex::encode(mac.finalize().into_bytes()))
}

fn mac(secret: &[u8]) -> HmacSha256 {
    // An HMAC takes a key of any length, so this cannot fail: an empty or a
    // long secret is hashed or padded as the construction says.
    #[expect(
        clippy::expect_used,
        reason = "HMAC accepts keys of every length, so new_from_slice cannot fail"
    )]
    HmacSha256::new_from_slice(secret).expect("an HMAC takes a key of any length")
}

/// Whether `header` is the signature of `body` under `secret`: the check a
/// receiver makes, in constant time.
#[must_use]
pub fn verify(secret: &[u8], body: &[u8], header: &str) -> bool {
    let Some(hex_digest) = header.trim().strip_prefix(PREFIX) else {
        return false;
    };
    let Ok(given) = hex::decode(hex_digest) else {
        return false;
    };
    let mut mac = mac(secret);
    mac.update(body);
    mac.verify_slice(&given).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_signature_is_the_hexadecimal_hmac_sha256_of_the_body_under_the_secret() {
        // RFC 4231, test case 2.
        let value = sign(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            value,
            "sha256=5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn a_receiver_accepts_the_signature_of_the_bytes_it_got_and_nothing_else() {
        let body = br#"{"id":"1","title":"Turn finished"}"#;
        let header = sign(b"s3cr3t", body);
        assert!(verify(b"s3cr3t", body, &header));
        assert!(
            !verify(b"s3cr3t", br#"{"id":"1","title":"Turn finishec"}"#, &header),
            "another body"
        );
        assert!(!verify(b"other", body, &header), "another secret");
        assert!(
            !verify(b"s3cr3t", body, header.trim_start_matches("sha256=")),
            "no algorithm"
        );
        assert!(!verify(b"s3cr3t", body, "sha256=zz"), "not hexadecimal");
        assert!(!verify(b"s3cr3t", body, "sha256="), "empty");
        assert!(
            !verify(b"s3cr3t", body, &format!("sha1={}", &header[7..])),
            "another algorithm"
        );
    }

    #[test]
    fn a_secret_of_any_length_signs() {
        assert!(sign(b"", b"x").starts_with("sha256="));
        assert_eq!(sign(&[b'k'; 4096], b"x").len(), 7 + 64);
    }
}
