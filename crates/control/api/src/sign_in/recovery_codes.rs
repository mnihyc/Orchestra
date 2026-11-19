//! Ten one-use, one-hundred-bit authenticator backups; only user-bound, domain-separated hashes are stored.

use super::codes::ALPHABET;
use orch_types::Id;
use sha2::{Digest, Sha256};

/// Ten backups fit one saved sheet while allowing several recoveries before replacement.
pub const CODE_COUNT: usize = 10;
/// Twenty independent base-32 characters give 100 random bits, unlike a short online-only mailed code.
pub const CODE_CHARS: usize = 20;
/// Four-character groups keep a saved code readable and easy to transcribe.
const GROUP_CHARS: usize = 4;
const DOMAIN: &[u8] = b"orch:second-step-recovery:v1\0";

/// Generates a plaintext batch in memory, for one response after its hashes commit.
#[must_use]
pub fn issue() -> Vec<String> {
    (0..CODE_COUNT)
        .map(|_| {
            let random: [u8; CODE_CHARS] = rand::random();
            random
                .chunks(GROUP_CHARS)
                .map(|group| {
                    group
                        .iter()
                        // Five bits select one of 32 symbols uniformly, with no remainder bias.
                        .map(|byte| char::from(ALPHABET[usize::from(byte & 31)]))
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("-")
        })
        .collect()
}

/// Accepts case, spaces and displayed hyphens; nothing else changes the code's identity.
#[must_use]
pub fn normalize(typed: &str) -> Option<String> {
    let code: String = typed
        .chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '-')
        .flat_map(char::to_uppercase)
        .collect();
    (code.len() == CODE_CHARS && code.bytes().all(|byte| ALPHABET.contains(&byte))).then_some(code)
}

/// Hashes the normalized code for one person, in a namespace separate from mail and token proofs.
#[must_use]
pub fn hash(user: Id, normalized: &str) -> Vec<u8> {
    let mut hash = Sha256::new();
    hash.update(DOMAIN);
    hash.update(user.as_bytes());
    hash.update(normalized.as_bytes());
    hash.finalize().to_vec()
}

/// Every issued display code must have its corresponding stored hash; malformed input never drops silently.
#[must_use]
pub fn hashes(user: Id, codes: &[String]) -> Option<Vec<Vec<u8>>> {
    codes
        .iter()
        .map(|code| normalize(code).map(|code| hash(user, &code)))
        .collect()
}

/// Binds the narrow recovery-management allowance to the current sealed authenticator version.
#[must_use]
pub fn factor_hash(user: Id, secret: &[u8]) -> Vec<u8> {
    let mut hash = Sha256::new();
    hash.update(b"orch:recovery-management:v1\0");
    hash.update(user.as_bytes());
    hash.update(secret);
    hash.finalize().to_vec()
}

/// A verified backup may be the LAST one: let this session replace it once, with a fresh first proof.
/// Five minutes matches an identity provider's fresh proof; another act cannot spend this allowance.
pub async fn grant_management(
    conn: &mut orch_db::Conn,
    user: &orch_db::queries::identity::UserRow,
    session: Id,
    now: chrono::DateTime<chrono::Utc>,
) -> crate::middleware::requests::ApiResult<()> {
    if let Some(secret) = user.totp_secret.as_deref().filter(|_| user.totp_on()) {
        let factor = factor_hash(user.id, secret);
        let reference: [u8; 32] = rand::random();
        orch_db::queries::recovery_codes::grant_management(
            conn, user.id, session, &factor, &reference, now,
        )
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issued_codes_can_be_saved_and_typed_with_spaces_or_lower_case() {
        let codes = issue();
        assert_eq!(codes.len(), CODE_COUNT);
        for code in codes {
            assert_eq!(code.split('-').count(), CODE_CHARS / GROUP_CHARS);
            assert!(code.split('-').all(|group| group.len() == GROUP_CHARS));
            let Some(normalized) = normalize(&code) else {
                panic!("an issued code is readable")
            };
            assert_eq!(
                normalize(&code.to_lowercase().replace('-', " ")).as_deref(),
                Some(normalized.as_str())
            );
            assert!(normalize(&normalized[..CODE_CHARS - 1]).is_none());
        }
        assert!(normalize("000000").is_none());
    }

    #[test]
    fn a_saved_code_is_bound_to_its_person_and_is_not_a_mailed_code_hash() {
        let first = orch_types::new_id();
        let second = orch_types::new_id();
        let code = "0123456789ABCDEFGHJK";
        assert_ne!(hash(first, code), hash(second, code));
        assert_ne!(hash(first, code), super::super::codes::hash(first, code));
    }
}
