//! The codes of a second step: the code that is mailed, and the one an authenticator app shows, with the key that seals an authenticator's secret.
//!
//! A mailed code is eight characters of thirty-two that cannot be mistaken
//! for one another, written `XXXX-XXXX` and read without regard to case,
//! hyphen or space; only its hash is stored. An authenticator's code is the
//! six digits of RFC 6238 with the 30 seconds and the neighbouring steps
//! that every app is compatible with. Its secret is stored sealed under the
//! system's master key, bound to the person it belongs to.

use chrono::{DateTime, Utc};
use orch_db::secrets::{MasterKey, SealError};
use orch_types::Id;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;
use subtle::ConstantTimeEq;
use totp_rs::{Builder, Totp};

/// The characters a mailed code is written in: the digits and the capitals
/// without `I`, `L`, `O` and `U`, thirty-two of them, so that each character
/// carries five bits and none is taken for another when it is read out.
pub const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// How many characters a mailed code has: forty bits, which at eight tries
/// in three hours is out of reach even of someone who tries a thousand
/// names at once.
pub const CODE_CHARS: usize = orch_api_contract::auth::EMAIL_CODE_CHARS as usize;

/// A new mailed code: [`CODE_CHARS`] characters of the alphabet, at random.
#[must_use]
pub fn new_code() -> String {
    (0..CODE_CHARS)
        // The index is below the length of the alphabet, by the range it is drawn from.
        .map(|_| char::from(ALPHABET[rand::random_range(0..ALPHABET.len())]))
        .collect()
}

/// A code as a person is shown it: in two halves, `XXXX-XXXX`.
#[must_use]
pub fn display(code: &str) -> String {
    let half = CODE_CHARS / 2;
    match (code.get(..half), code.get(half..)) {
        (Some(first), Some(second)) => format!("{first}-{second}"),
        _ => code.to_owned(),
    }
}

/// A code as a person typed it, brought to the form it is stored in: upper
/// case, without hyphens and spaces. It is nothing unless it then has the
/// right number of characters of the alphabet.
#[must_use]
pub fn normalize(typed: &str) -> Option<String> {
    let code: String = typed
        .chars()
        .filter(|character| !character.is_whitespace() && *character != '-')
        .flat_map(char::to_uppercase)
        .collect();
    let fits = code.len() == CODE_CHARS && code.bytes().all(|byte| ALPHABET.contains(&byte));
    fits.then_some(code)
}

/// What is stored of a code: the SHA-256 of the identifier of the person it
/// is for and the code, so that the same code for two people is two values.
#[must_use]
pub fn hash(user: Id, code: &str) -> Vec<u8> {
    let mut digest = Sha256::new();
    digest.update(user.as_bytes());
    digest.update(code.as_bytes());
    digest.finalize().to_vec()
}

/// Whether `typed` is the code whose hash `stored` is, for `user`, compared
/// in constant time. Without a stored hash, or with something that is no code
/// at all, the same work is done and the answer is no.
#[must_use]
pub fn matches(stored: Option<&[u8]>, user: Id, typed: &str) -> bool {
    let wanted = normalize(typed);
    let given = hash(user, wanted.as_deref().unwrap_or_default());
    let stored = stored.unwrap_or(&[]);
    let same = bool::from(given.as_slice().ct_eq(stored));
    same && wanted.is_some()
}

/// How many digits an authenticator's code has: what every app shows.
pub const TOTP_DIGITS: u8 = 6;
/// How long one code lasts, in seconds: what every app uses.
pub const TOTP_STEP_SECS: u64 = 30;
/// How many steps either side of the present a code is still accepted in: one,
/// so that a clock a few seconds off, or a code typed slowly, still counts.
pub const TOTP_SKEW: u16 = 1;
/// How many bytes an authenticator's secret has: 160 bits, which RFC 4226
/// recommends.
pub const TOTP_SECRET_BYTES: usize = 20;

/// Why an authenticator could not be made or read.
#[derive(Debug, thiserror::Error)]
pub enum AuthenticatorError {
    /// The secret could not be made into a generator of codes.
    #[error("an authenticator could not be built: {0}")]
    Build(String),
    /// The master key is not there.
    #[error("the master key is not available: {0}")]
    NoKey(String),
    /// A sealed secret does not open.
    #[error(transparent)]
    Seal(#[from] SealError),
}

/// A secret for a new authenticator, at random.
#[must_use]
pub fn new_secret() -> Vec<u8> {
    let secret: [u8; TOTP_SECRET_BYTES] = rand::random();
    secret.to_vec()
}

/// The generator of codes for `secret`, labelled for an app with the name
/// of the person and of the system. A colon in either would end the label, so
/// it is replaced.
pub fn authenticator(
    secret: Vec<u8>,
    account: &str,
    issuer: &str,
) -> Result<Totp, AuthenticatorError> {
    let clean = |text: &str| text.replace(':', " ");
    Builder::new()
        .with_digits(TOTP_DIGITS)
        .with_skew(TOTP_SKEW)
        .with_step_duration(TOTP_STEP_SECS)
        .with_secret(secret)
        .with_account_name(clean(account))
        .with_issuer(Some(clean(issuer)))
        .build()
        .map_err(|error| AuthenticatorError::Build(format!("{error:?}")))
}

/// The time step of `code` at `now`, if it is one an authenticator with
/// `secret` shows in the steps around then. Spaces typed in it do not matter.
#[must_use]
pub fn totp_step(totp: &Totp, code: &str, now: DateTime<Utc>) -> Option<i64> {
    let typed: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    let seconds = u64::try_from(now.timestamp()).ok()?;
    totp.check(&typed, seconds)
        .and_then(|step| i64::try_from(step).ok())
}

/// Where the master key is read from when no one gave it: the file that the
/// deployment mounts into the services that seal secrets.
pub const MASTER_KEY_PATH: &str = "/run/secrets/orch_master_key";

/// The variable that names another file for the master key, for a machine
/// where the deployment's path is not.
pub const MASTER_KEY_VARIABLE: &str = "ORCH_MASTER_KEY_FILE";

static MASTER_KEY: OnceLock<MasterKey> = OnceLock::new();

/// Gives the master key to this module, once: whoever composes the program
/// from its own configuration, and tests. The answer says whether the key was
/// taken; a key given earlier stays.
pub fn install_master_key(key: MasterKey) -> bool {
    MASTER_KEY.set(key).is_ok()
}

/// The master key: the one that was given, or else the one in the file
/// the deployment provides.
fn master_key() -> Result<&'static MasterKey, AuthenticatorError> {
    if let Some(key) = MASTER_KEY.get() {
        return Ok(key);
    }
    let path = std::env::var(MASTER_KEY_VARIABLE).unwrap_or_else(|_| MASTER_KEY_PATH.to_owned());
    let bytes = orch_config::read_secret_hex::<32>(std::path::Path::new(&path))
        .map_err(|error| AuthenticatorError::NoKey(error.to_string()))?;
    Ok(MASTER_KEY.get_or_init(|| MasterKey::new(bytes)))
}

/// An authenticator's secret sealed for the person `user`: it opens only for
/// them, under this system's key.
pub fn seal_secret(user: Id, secret: &[u8]) -> Result<Vec<u8>, AuthenticatorError> {
    Ok(master_key()?.seal(secret, user.as_bytes())?)
}

/// The secret that `sealed` holds for the person `user`.
pub fn open_secret(user: Id, sealed: &[u8]) -> Result<Vec<u8>, AuthenticatorError> {
    Ok(master_key()?.open(sealed, user.as_bytes())?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn a_mailed_code_is_eight_characters_that_cannot_be_mistaken_for_one_another() {
        for _ in 0..200 {
            let code = new_code();
            assert_eq!(code.len(), 8);
            assert!(
                code.bytes()
                    .all(|byte| ALPHABET.contains(&byte) && !b"ILOU".contains(&byte)),
                "{code}"
            );
        }
        assert_eq!(ALPHABET.len(), 32);
        assert_eq!(display("ABCD1234"), "ABCD-1234");
    }

    #[test]
    fn a_code_is_read_without_regard_to_case_hyphen_or_space() {
        assert_eq!(normalize("abcd-1234").as_deref(), Some("ABCD1234"));
        assert_eq!(normalize(" ab cd 12-34 ").as_deref(), Some("ABCD1234"));
        // Too short, too long, or a character the alphabet does not have.
        assert_eq!(normalize("ABCD123"), None);
        assert_eq!(normalize("ABCD12345"), None);
        assert_eq!(normalize("ABCD-123I"), None);
        assert_eq!(normalize("123456"), None);
    }

    #[test]
    fn only_the_hash_of_a_code_for_one_person_matches_it() {
        let (ada, bob) = (orch_types::new_id(), orch_types::new_id());
        let stored = hash(ada, "ABCD1234");
        assert!(matches(Some(&stored), ada, "abcd-1234"));
        assert!(!matches(Some(&stored), ada, "ABCD1235"));
        assert!(!matches(Some(&stored), bob, "ABCD1234"));
        assert!(!matches(None, ada, "ABCD1234"));
        assert!(!matches(Some(&stored), ada, "not a code"));
    }

    #[test]
    fn a_code_of_an_authenticator_is_six_digits_of_the_step_it_is_shown_in() {
        let totp = authenticator(vec![7; TOTP_SECRET_BYTES], "ada", "Orch").unwrap();
        let now = chrono::Utc.with_ymd_and_hms(2026, 10, 8, 14, 0, 0).unwrap();
        let seconds = u64::try_from(now.timestamp()).unwrap();
        let code = totp.generate(seconds).to_string();
        assert_eq!(code.len(), 6);
        let step = i64::try_from(seconds / TOTP_STEP_SECS).unwrap();
        assert_eq!(totp_step(&totp, &code, now), Some(step));
        // A code with spaces typed in it, half a minute later, and a minute later.
        let spaced = format!("{} {}", &code[..3], &code[3..]);
        let later = now + chrono::Duration::seconds(30);
        assert_eq!(totp_step(&totp, &spaced, later), Some(step));
        let much_later = now + chrono::Duration::seconds(95);
        assert_eq!(totp_step(&totp, &code, much_later), None);
        assert_eq!(totp_step(&totp, "12345", now), None);
        let url = totp.to_url().unwrap();
        assert!(url.starts_with("otpauth://totp/Orch:ada?secret="), "{url}");
    }

    #[test]
    fn a_colon_in_a_label_does_not_end_it() {
        let totp = authenticator(vec![1; TOTP_SECRET_BYTES], "ada", "Acme: Orch").unwrap();
        assert_eq!(totp.issuer(), Some("Acme  Orch"));
    }

    #[test]
    fn a_sealed_secret_opens_for_its_person_alone() {
        install_master_key(MasterKey::new([5; 32]));
        let (ada, bob) = (orch_types::new_id(), orch_types::new_id());
        let sealed = seal_secret(ada, b"twenty bytes secret!").unwrap();
        assert_eq!(open_secret(ada, &sealed).unwrap(), b"twenty bytes secret!");
        assert!(open_secret(bob, &sealed).is_err());
    }
}
