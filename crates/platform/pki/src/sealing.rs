//! HPKE machine-to-machine secret sealing.
//!
//! A secret that has to reach one machine is encrypted to the key of that
//! machine's certificate, so that it is readable by that machine and by
//! nobody on the way: not the gateway, not a content network, not the system
//! that relays it. The scheme is HPKE (RFC 9180) in its base mode with
//! DHKEM(P-256, HKDF-SHA256), HKDF-SHA256 and ChaCha20-Poly1305. The `info`
//! string binds the sealed value to the command it was made for, so that a
//! value cannot be lifted out of one command and put into another.
//!
//! A sealed value is written as the encapsulated key (65 bytes) followed by
//! the ciphertext, in standard base64. A value can be sealed by the system
//! for a machine, or by one machine for another; the sealer needs only the
//! recipient's public key.

use crate::PkiError;
use crate::certificates::{Certificate, LeafKey, P256_POINT_BYTES};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
use hpke::aead::ChaCha20Poly1305;
use hpke::kdf::HkdfSha256;
use hpke::kem::DhP256HkdfSha256;
use hpke::{Deserializable, OpModeR, OpModeS, Serializable, single_shot_open, single_shot_seal};
use orch_types::Id;

type Kem = DhP256HkdfSha256;

/// Nothing is authenticated beside the sealed value: the command it belongs
/// to is bound through `info`.
const NO_AAD: &[u8] = &[];

/// The key a value is sealed to: the public key of a machine's certificate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecipientKey([u8; P256_POINT_BYTES]);

impl RecipientKey {
    /// The key of `certificate`.
    #[must_use]
    pub fn from_certificate(certificate: &Certificate) -> Self {
        Self(*certificate.public_point())
    }

    /// The key whose uncompressed point is `point`.
    pub fn from_point(point: &[u8]) -> Result<Self, PkiError> {
        let point: [u8; P256_POINT_BYTES] = point
            .try_into()
            .map_err(|_| PkiError::Seal("a key is an uncompressed P-256 point".to_owned()))?;
        // The library refuses a point that is not on the curve when it uses it;
        // refuse it here, where the cause is known.
        <Kem as hpke::Kem>::PublicKey::from_bytes(&point)
            .map_err(|e| PkiError::Seal(format!("not a P-256 public key: {e}")))?;
        Ok(Self(point))
    }

    /// The key as the system writes it where a message names one: the point
    /// in unpadded base64url.
    #[must_use]
    pub fn to_text(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.0)
    }

    /// Reads a key written by [`RecipientKey::to_text`].
    pub fn parse(text: &str) -> Result<Self, PkiError> {
        let bytes = decode_base64(text)?;
        Self::from_point(&bytes)
    }
}

/// Reads base64 in any of its four common spellings: what is sealed is
/// opened whichever way a relay re-wrote it.
fn decode_base64(text: &str) -> Result<Vec<u8>, PkiError> {
    [&STANDARD, &STANDARD_NO_PAD, &URL_SAFE, &URL_SAFE_NO_PAD]
        .iter()
        .find_map(|engine| {
            engine
                .decode(text.trim_end_matches('='))
                .ok()
                .or_else(|| engine.decode(text).ok())
        })
        .ok_or_else(|| PkiError::Seal("not base64".to_owned()))
}

/// The `info` of a value sealed for the command `command_id`: the
/// identifier as it is written everywhere.
#[must_use]
pub fn info_for_command(command_id: Id) -> Vec<u8> {
    command_id.to_string().into_bytes()
}

/// Seals `plaintext` to `recipient`, bound to `info`.
pub fn seal(recipient: &RecipientKey, info: &[u8], plaintext: &[u8]) -> Result<String, PkiError> {
    let public = <Kem as hpke::Kem>::PublicKey::from_bytes(&recipient.0)
        .map_err(|e| PkiError::Seal(format!("not a P-256 public key: {e}")))?;
    let (encapsulated, ciphertext) = single_shot_seal::<ChaCha20Poly1305, HkdfSha256, Kem>(
        &OpModeS::Base,
        &public,
        info,
        plaintext,
        NO_AAD,
    )
    .map_err(|e| PkiError::Seal(format!("sealing failed: {e}")))?;
    let mut sealed = encapsulated.to_bytes().to_vec();
    sealed.extend_from_slice(&ciphertext);
    Ok(STANDARD.encode(sealed))
}

/// Opens a value sealed to `key`'s public half and bound to `info`. A value
/// sealed to another key, or for another command, does not open.
pub fn open(key: &LeafKey, info: &[u8], sealed: &str) -> Result<Vec<u8>, PkiError> {
    let bytes = decode_base64(sealed)?;
    if bytes.len() <= P256_POINT_BYTES {
        return Err(PkiError::Seal("the sealed value is too short".to_owned()));
    }
    let (encapsulated, ciphertext) = bytes.split_at(P256_POINT_BYTES);
    let encapsulated = <Kem as hpke::Kem>::EncappedKey::from_bytes(encapsulated)
        .map_err(|e| PkiError::Seal(format!("the sealed value is malformed: {e}")))?;
    let private = <Kem as hpke::Kem>::PrivateKey::from_bytes(&key.secret_scalar())
        .map_err(|e| PkiError::Seal(format!("the key cannot open values: {e}")))?;
    single_shot_open::<ChaCha20Poly1305, HkdfSha256, Kem>(
        &OpModeR::Base,
        &private,
        &encapsulated,
        info,
        ciphertext,
        NO_AAD,
    )
    .map_err(|_| PkiError::Seal("the value was not sealed to this key for this command".to_owned()))
}
