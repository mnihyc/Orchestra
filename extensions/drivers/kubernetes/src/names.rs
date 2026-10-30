//! Map node volume names to Kubernetes DNS-subdomain names without losing identity.

use orch_driver_contract::driver::DriverError;
use orch_types::Hash;

/// Matches the node's portable volume-name bound.
const MAX_VOLUME_NAME_BYTES: usize = 255;
/// Splitting the full 256-bit hash keeps each DNS label below Kubernetes' 63-byte limit.
const HASH_HALF_HEX: usize = 32;

/// The claim stores the complete logical name as an ownership annotation.
pub(crate) const LOGICAL_NAME_ANNOTATION: &str = "orch.volume-name";

/// Maps every portable node volume name deterministically; ownership checks still compare the original name.
pub(crate) fn claim(logical: &str) -> Result<String, DriverError> {
    if logical.len() > MAX_VOLUME_NAME_BYTES
        || !logical
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        || !logical.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '-')
        })
        || logical.contains("..")
    {
        return Err(DriverError::InvalidSpec(
            "the logical volume name is invalid".into(),
        ));
    }
    let hash = Hash::of(logical.as_bytes()).to_string();
    Ok(format!(
        "orch-v-{}.{}",
        &hash[..HASH_HALF_HEX],
        &hash[HASH_HALF_HEX..]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_names_with_uppercase_underscores_and_maximum_length_get_dns_safe_claims() {
        let longest = "x".repeat(MAX_VOLUME_NAME_BYTES);
        for logical in ["Work_A-1", "orch-ws-0199abcd-1234", longest.as_str()] {
            let native = claim(logical).unwrap();
            assert_eq!(claim(logical).unwrap(), native);
            assert!(native.len() <= 253);
            for label in native.split('.') {
                assert!(!label.is_empty() && label.len() <= 63);
                assert!(label.bytes().all(|byte| byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || byte == b'-'));
                assert!(label.as_bytes()[0].is_ascii_alphanumeric());
                assert!(label.as_bytes()[label.len() - 1].is_ascii_alphanumeric());
            }
        }
        assert_ne!(claim("Work_A").unwrap(), claim("work_a").unwrap());
    }

    #[test]
    fn invalid_logical_names_never_become_claim_paths() {
        let too_long = "x".repeat(MAX_VOLUME_NAME_BYTES + 1);
        for invalid in [
            "",
            "-x",
            "../x",
            "a/b",
            "a b",
            "a..b",
            ".hidden",
            too_long.as_str(),
        ] {
            assert!(claim(invalid).is_err(), "{invalid}");
        }
    }
}
