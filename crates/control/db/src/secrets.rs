//! At-rest sealing for stored secrets.
//!
//! A secret column holds `nonce (24 bytes) ‖ ciphertext` under
//! XChaCha20-Poly1305 with the system's master key. The row's own key is
//! bound in as associated data, so a sealed value copied into another row
//! does not open there. The master key is a file of the deployment and is
//! never stored in the database.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hmac::{Hmac, Mac};
use orch_types::Hash;
use sha2::Sha256;

/// The length of the nonce that precedes every sealed value.
const NONCE_BYTES: usize = 24;

/// The system's master key, which seals what is stored.
#[derive(Clone)]
pub struct MasterKey([u8; 32]);

impl std::fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MasterKey(…)")
    }
}

/// A sealed value that does not open: it was sealed under another key, for
/// another row, or it is damaged. Which of these is not told.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("a sealed value does not open")]
pub struct SealError;

impl MasterKey {
    /// The key whose bytes are `bytes`.
    #[must_use]
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Stable opaque equality evidence, never a credential or a sealed value.
    /// The caller must serialize an explicit purpose/version and object identity with its data.
    /// Keyed hashing prevents guessing private low-entropy inputs from a published fingerprint.
    pub fn fingerprint(&self, content: &[u8]) -> Result<Hash, SealError> {
        let mut mac =
            <Hmac<Sha256> as hmac::KeyInit>::new_from_slice(&self.0).map_err(|_| SealError)?;
        mac.update(content);
        Hash::from_slice(&mac.finalize().into_bytes()).map_err(|_| SealError)
    }

    /// Seals `plain` for the row whose key is `bound_to`.
    pub fn seal(&self, plain: &[u8], bound_to: &[u8]) -> Result<Vec<u8>, SealError> {
        let nonce: [u8; NONCE_BYTES] = rand::random();
        let cipher = XChaCha20Poly1305::new((&self.0).into());
        let sealed = cipher
            .encrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: plain,
                    aad: bound_to,
                },
            )
            .map_err(|_| SealError)?;
        let mut out = Vec::with_capacity(NONCE_BYTES + sealed.len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&sealed);
        Ok(out)
    }

    /// Opens `sealed`, which was sealed for the row whose key is `bound_to`.
    pub fn open(&self, sealed: &[u8], bound_to: &[u8]) -> Result<Vec<u8>, SealError> {
        if sealed.len() < NONCE_BYTES {
            return Err(SealError);
        }
        let (nonce, body) = sealed.split_at(NONCE_BYTES);
        let nonce: [u8; NONCE_BYTES] = nonce.try_into().map_err(|_| SealError)?;
        let cipher = XChaCha20Poly1305::new((&self.0).into());
        cipher
            .decrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: body,
                    aad: bound_to,
                },
            )
            .map_err(|_| SealError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sealed_value_opens_only_under_its_key_and_for_its_row() {
        let key = MasterKey::new([7; 32]);
        let sealed = key.seal(b"sk-live", b"row-1").unwrap();
        assert_eq!(key.open(&sealed, b"row-1").unwrap(), b"sk-live");
        assert_eq!(key.open(&sealed, b"row-2"), Err(SealError));
        assert_eq!(
            MasterKey::new([8; 32]).open(&sealed, b"row-1"),
            Err(SealError)
        );
    }

    #[test]
    fn an_opaque_fingerprint_uses_the_standard_hmac_without_changing_sealing()
    -> Result<(), SealError> {
        // RFC4231 case2's short key padded to32 bytes has the same HMAC block as that short key.
        let mut bytes = [0; 32];
        bytes[..4].copy_from_slice(b"Jefe");
        let key = MasterKey::new(bytes);
        let plain = b"what do ya want for nothing?";
        assert_eq!(
            key.fingerprint(plain)?.to_string(),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert_ne!(
            key.fingerprint(plain)?,
            MasterKey::new([8; 32]).fingerprint(plain)?
        );
        assert_ne!(key.fingerprint(plain)?, Hash::of(plain));
        let sealed = key.seal(plain, b"row")?;
        assert_eq!(key.open(&sealed, b"row")?, plain);
        assert_eq!(key.fingerprint(plain)?, key.fingerprint(plain)?);
        Ok(())
    }

    #[test]
    fn the_same_value_never_seals_to_the_same_bytes_and_a_damaged_one_is_refused() {
        let key = MasterKey::new([7; 32]);
        let first = key.seal(b"x", b"r").unwrap();
        let mut second = key.seal(b"x", b"r").unwrap();
        assert_ne!(first, second);
        let last = second.len() - 1;
        second[last] ^= 1;
        assert_eq!(key.open(&second, b"r"), Err(SealError));
        assert_eq!(key.open(b"short", b"r"), Err(SealError));
        assert!(!format!("{key:?}").contains('7'));
    }
}
