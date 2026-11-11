//! Restricted confirmations conceal complete native membership while preserving exact equality.

use super::*;

use crate::routes::resources::sealing;

use base64::Engine as _;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
// A digest plus an optional RFC3339 grant deadline uses fewer than 160 JSON bytes.
// One fixed frame also conceals whether a grant deadline is present. The existing
// XChaCha20-Poly1305 envelope contributes its 24-byte nonce and 16-byte tag.
const FRAME_BYTES: usize = 256;

const CIPHER_BYTES: usize = FRAME_BYTES + 24 + 16;

const BINDING: &[u8] = b"operations/document-tree/deletion-confirmation/v1";

pub(super) fn seal(stamp: &domain::MembershipStamp) -> ApiResult<DocumentTreeDeletion> {
    let digest = stamp.deletion_digest().map_err(ApiError::internal)?;
    let mut frame = serde_json::to_vec(&digest).map_err(ApiError::internal)?;
    if frame.len() > FRAME_BYTES {
        return Err(ApiError::internal(
            "the native confirmation exceeds its fixed frame",
        ));
    }
    frame.resize(FRAME_BYTES, b' ');
    let sealed = sealing::installed_master_key()?
        .seal(&frame, BINDING)
        .map_err(ApiError::internal)?;
    Ok(DocumentTreeDeletion::Sealed(DocumentTreeConfirmation {
        confirmation: URL_SAFE_NO_PAD.encode(sealed),
    }))
}

pub(super) enum Decoded {
    Membership(domain::MembershipStamp),
    Digest(domain::DeletionDigest),
}

impl Decoded {
    pub(super) fn as_confirmation(&self) -> domain::DeletionConfirmation<'_> {
        match self {
            Self::Membership(value) => domain::DeletionConfirmation::Membership(value),
            Self::Digest(value) => domain::DeletionConfirmation::Digest(value),
        }
    }
}

fn invalid() -> ApiError {
    ApiError::invalid("deletion_plan", "obtain a fresh deletion preview")
}

pub(super) fn decode(value: &DocumentTreeDeletion) -> ApiResult<Decoded> {
    match value {
        DocumentTreeDeletion::Membership(value) => convert(value).map(Decoded::Membership),
        DocumentTreeDeletion::Sealed(value) => {
            if value.confirmation.len() != (CIPHER_BYTES * 4).div_ceil(3) {
                return Err(invalid());
            }
            let sealed = URL_SAFE_NO_PAD
                .decode(&value.confirmation)
                .map_err(|_| invalid())?;
            if sealed.len() != CIPHER_BYTES {
                return Err(invalid());
            }
            let plain = sealing::installed_master_key()?
                .open(&sealed, BINDING)
                .map_err(|_| invalid())?;
            if plain.len() != FRAME_BYTES {
                return Err(invalid());
            }
            serde_json::from_slice(&plain)
                .map(Decoded::Digest)
                .map_err(|_| invalid())
        }
    }
}
