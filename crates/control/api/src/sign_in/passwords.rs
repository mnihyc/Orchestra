//! Passwords: kept as Argon2id hashes, compared in constant time, and the rules a new one has to meet.
//!
//! A hash costs memory and time on purpose, so it is made and checked on a
//! thread of the blocking pool and only a few at once: a flood of attempts
//! must not be able to take all the memory of the machine. A name that
//! nobody has is checked against a hash of a password nobody knows, so that
//! the work does not tell whether the name exists.

use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use argon2::{Algorithm, Argon2, Params, Version};
use std::sync::OnceLock;
use tokio::sync::Semaphore;

/// The memory a hash takes, in KiB: 19 MiB, which with two passes and one
/// lane is the smallest setting of Argon2id that the OWASP password storage
/// guidance accepts.
pub const MEMORY_KIB: u32 = 19 * 1024;
/// How many passes over that memory a hash makes.
pub const PASSES: u32 = 2;
/// How many lanes a hash uses: one, so that a hash takes one processor.
pub const LANES: u32 = 1;

/// How many hashes are made or checked at once. Each holds 19 MiB, so four
/// are 76 MiB at most however many people try at the same moment, and the
/// other processors stay with the rest of the API.
const PARALLEL_HASHES: usize = 4;

/// How many bytes of randomness the password behind the hash that names
/// that nobody has are checked against has.
const DECOY_BYTES: usize = 32;

static WORKERS: Semaphore = Semaphore::const_new(PARALLEL_HASHES);

/// A hash that could not be made.
#[derive(Debug, thiserror::Error)]
#[error("a password could not be hashed: {0}")]
pub struct HashFailed(String);

fn context() -> Result<Argon2<'static>, HashFailed> {
    let params = Params::new(MEMORY_KIB, PASSES, LANES, None)
        .map_err(|error| HashFailed(error.to_string()))?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

fn hash_now(password: &str) -> Result<String, HashFailed> {
    let hash = context()?
        .hash_password(password.as_bytes())
        .map_err(|error| HashFailed(error.to_string()))?;
    Ok(hash.to_string())
}

/// The hash of a password nobody has: what a name that nobody has is checked
/// against, made the first time it is needed and kept.
fn decoy() -> &'static str {
    static DECOY: OnceLock<String> = OnceLock::new();
    DECOY.get_or_init(|| {
        let secret: [u8; DECOY_BYTES] = rand::random();
        // A decoy that could not be made leaves an empty string, which no
        // password matches and which is checked the way any other hash is
        // (it is refused when it is read, at the cost of the read alone).
        hash_now(&hex(&secret)).unwrap_or_default()
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Both known and unknown accounts perform one verification; only a stored hash can authenticate.
fn checked_by(
    password: &str,
    stored: Option<&str>,
    verify: impl FnOnce(&[u8], &str) -> bool,
) -> bool {
    let checked = verify(password.as_bytes(), stored.unwrap_or_else(|| decoy()));
    stored.is_some() && checked
}

/// The Argon2id hash of `password`, as a PHC string, with a salt of its own.
pub async fn hash(password: &str) -> Result<String, HashFailed> {
    let password = password.to_owned();
    let _permit = WORKERS
        .acquire()
        .await
        .map_err(|error| HashFailed(error.to_string()))?;
    tokio::task::spawn_blocking(move || hash_now(&password))
        .await
        .map_err(|error| HashFailed(error.to_string()))?
}

/// Whether `password` is the one `stored` is the hash of. Where there is no
/// hash (a name nobody has, a person who has no password), or it does not
/// read, the password is checked against a decoy and is never right, so that
/// the work is the same as for a hash that is there.
pub async fn verify(password: &str, stored: Option<&str>) -> bool {
    let password = password.to_owned();
    let stored = stored.map(str::to_owned);
    let Ok(_permit) = WORKERS.acquire().await else {
        return false;
    };
    tokio::task::spawn_blocking(move || {
        let Ok(context) = context() else {
            return false;
        };
        checked_by(&password, stored.as_deref(), |password, hash| {
            context.verify_password(password, hash).is_ok()
        })
    })
    .await
    .unwrap_or(false)
}

/// Why a new password is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// It has fewer characters than the system asks for.
    TooShort {
        /// The fewest characters.
        min: u32,
    },
    /// It is the username or the address of the person.
    TheName,
    /// The whole of it, case folded, is on the release's list of commonly used passwords.
    Common,
    /// The release's list of common passwords is not the one it was built with: a fault of the
    /// installation, which refuses every new password rather than check none.
    ListDamaged(String),
}

impl Refused {
    /// What is wrong with it, in a sentence.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::TooShort { min } => format!("must have at least {min} characters"),
            Self::TheName => "must not be the username or the address".to_owned(),
            Self::Common => COMMON_SENTENCE.to_owned(),
            Self::ListDamaged(why) => format!("the list of common passwords is damaged: {why}"),
        }
    }

    /// The answer to a request that set this password, for the member `field` that carried it: a common
    /// password is `auth.password_common` (400), a damaged list a fault of the installation (500, its cause
    /// in the log), anything else a fault of the request at that member.
    #[must_use]
    pub fn into_api_error(self, field: &str) -> crate::middleware::requests::ApiError {
        use crate::middleware::requests::ApiError;
        use orch_api_contract::common::ErrorCode;
        match self {
            Self::Common => {
                ApiError::new(ErrorCode::AuthPasswordCommon, COMMON_SENTENCE).with("field", field)
            }
            Self::ListDamaged(_) => ApiError::internal(self.message()),
            Self::TooShort { .. } | Self::TheName => ApiError::invalid(field, &self.message()),
        }
    }
}

/// What a person is told when the password they chose is a common one.
pub const COMMON_SENTENCE: &str =
    "This password is commonly used. Choose a different password or a longer phrase.";

/// The release's list of common passwords: SecLists' ten thousand most common (`common_passwords/README.md`).
const COMMON_LIST: &str = include_str!("common_passwords/10k-most-common.txt");
/// The SHA-256 of that file at its pinned commit: a list that is not this one is not checked against.
const COMMON_LIST_SHA256: &str = "68782d6a4a19a4768d5f15dd66bd534e7a33055cc755411e33f16d18c50fdcce";
/// How many nonempty lines the pinned file has.
const COMMON_LIST_LINES: usize = 10_001;
/// Values people use that the file lacks, refused all the same.
const COMMON_ADDED: &[&str] = &["password1234"];

/// The common passwords, case folded, read from the release once and checked against the pinned hash
/// and count; `Err` says why the list cannot be trusted.
fn common_passwords() -> Result<&'static std::collections::HashSet<String>, String> {
    use sha2::Digest;
    static LIST: OnceLock<Result<std::collections::HashSet<String>, String>> = OnceLock::new();
    LIST.get_or_init(|| {
        let digest = hex(&sha2::Sha256::digest(COMMON_LIST.as_bytes()));
        if digest != COMMON_LIST_SHA256 {
            return Err(format!("its SHA-256 is {digest}, not {COMMON_LIST_SHA256}"));
        }
        let lines: Vec<&str> = COMMON_LIST
            .lines()
            .filter(|line| !line.is_empty())
            .collect();
        if lines.len() != COMMON_LIST_LINES {
            return Err(format!(
                "it has {} lines, not {COMMON_LIST_LINES}",
                lines.len()
            ));
        }
        Ok(lines
            .into_iter()
            .chain(COMMON_ADDED.iter().copied())
            .map(str::to_lowercase)
            .collect())
    })
    .as_ref()
    .map_err(Clone::clone)
}

/// Whether `password` may be a new password for the person with this
/// username and address. There is no rule of composition beyond these: its
/// length is counted in characters, it may not be the name they sign in by,
/// without regard to case, and the whole of it, case folded, may not be on the
/// release's list of common passwords (a part of it may: a phrase that holds a
/// common word is not a common password).
pub fn check_new(
    password: &str,
    min_chars: u32,
    username: &str,
    email: Option<&str>,
) -> Result<(), Refused> {
    let length = u32::try_from(password.chars().count()).unwrap_or(u32::MAX);
    if length < min_chars {
        return Err(Refused::TooShort { min: min_chars });
    }
    let lower = password.to_lowercase();
    if lower == username.to_lowercase() || email.is_some_and(|email| lower == email.to_lowercase())
    {
        return Err(Refused::TheName);
    }
    if common_passwords()
        .map_err(Refused::ListDamaged)?
        .contains(&lower)
    {
        return Err(Refused::Common);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_password_is_kept_as_an_argon2id_hash_that_only_it_opens() {
        let stored = hash("correct horse battery").await.unwrap();
        assert!(
            stored.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"),
            "{stored}"
        );
        assert!(verify("correct horse battery", Some(&stored)).await);
        assert!(!verify("correct horse batter", Some(&stored)).await);
        // The same password does not hash to the same bytes twice.
        assert_ne!(stored, hash("correct horse battery").await.unwrap());
    }

    #[tokio::test]
    async fn no_hash_and_a_hash_that_does_not_read_are_never_right() {
        assert!(!verify("anything at all", None).await);
        assert!(!verify("anything at all", Some("not a hash")).await);
        assert!(!verify("", Some("")).await);
    }

    #[tokio::test]
    async fn a_name_nobody_has_is_checked_with_the_work_of_one_somebody_has() {
        let stored = hash("correct horse battery").await.unwrap();
        let mut checked = Vec::new();
        for stored in [Some(stored.as_str()), None] {
            assert!(!checked_by("a wrong password", stored, |password, hash| {
                // Observe the actual Argon2 algorithm/version/cost used, omitting salt and hash.
                checked.push(hash.split('$').take(4).collect::<Vec<_>>().join("$"));
                context().unwrap().verify_password(password, hash).is_ok()
            }));
        }
        assert_eq!(
            checked.len(),
            2,
            "one real verification for each account lookup result"
        );
        assert_eq!(
            checked[0], checked[1],
            "unknown names use the same algorithm and memory/pass/lane costs"
        );
    }

    #[test]
    fn a_new_password_has_enough_characters_and_is_not_the_name() {
        assert_eq!(
            check_new("short", 10, "alice", None),
            Err(Refused::TooShort { min: 10 })
        );
        // Characters are counted, not bytes: ten of them are more than ten bytes.
        assert_eq!(check_new("пароль-пароль", 10, "alice", None), Ok(()));
        assert_eq!(
            check_new("alice-the-user", 10, "alice-the-user", None),
            Err(Refused::TheName)
        );
        assert_eq!(
            check_new("Alice@Example.org", 10, "alice", Some("alice@example.org")),
            Err(Refused::TheName)
        );
        assert_eq!(check_new("0123456789", 10, "alice", None), Ok(()));
    }

    #[test]
    fn the_release_carries_the_pinned_list_of_common_passwords_and_the_value_it_adds() {
        let list = common_passwords().expect("the shipped list is the pinned one");
        assert_eq!(list.len(), COMMON_LIST_LINES + COMMON_ADDED.len());
        assert!(list.contains("qwertyuiop") && list.contains("password1234"));
    }

    #[test]
    fn a_common_password_is_refused_whole_and_case_folded_and_a_phrase_that_holds_one_is_not() {
        for common in [
            "qwertyuiop",
            "QwertyUIOP",
            "1234567890",
            "password1234",
            "Password1234",
        ] {
            assert_eq!(
                check_new(common, 10, "alice", None),
                Err(Refused::Common),
                "{common}"
            );
        }
        // Not a part of it: a longer phrase with a common word in it is the person's own.
        assert_eq!(
            check_new("qwertyuiop is my keyboard", 10, "alice", None),
            Ok(())
        );
        assert_eq!(check_new("password12345", 10, "alice", None), Ok(()));
        // The rules before it still come first, and say what they said.
        assert_eq!(
            check_new("qwerty", 10, "alice", None),
            Err(Refused::TooShort { min: 10 })
        );
        assert_eq!(Refused::Common.message(), COMMON_SENTENCE);
        let answer = Refused::Common.into_api_error("new_password");
        assert_eq!(
            answer.problem.code,
            orch_api_contract::common::ErrorCode::AuthPasswordCommon
        );
        assert_eq!(answer.problem.status, 400);
    }
}
