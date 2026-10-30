//! Secret-file loading without logging values.
//!
//! A secret is a file of its own, readable by the service that needs it.
//! What is read is wrapped so that it cannot be printed by accident: its
//! `Debug` shows nothing of it.

use std::path::{Path, PathBuf};

/// Where the programs of a deployment look for its secrets unless they are told another directory.
///
/// The stack binds each secret file by itself under this directory in the container of every program that
/// needs it, so that a program started there without a word about it finds what it was given.
pub const DEFAULT_SECRETS_DIR: &str = "/run/secrets";

/// Bytes that are never shown.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(Vec<u8>);

impl Secret {
    /// Wraps `bytes`.
    #[must_use]
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// The bytes, for the one place that uses them.
    #[must_use]
    pub fn expose(&self) -> &[u8] {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(…)")
    }
}

/// Why a secret could not be read. The value is never part of it.
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    /// The file could not be read.
    #[error("cannot read the secret {path}: {source}")]
    Read {
        /// The file.
        path: PathBuf,
        /// What the system said.
        source: std::io::Error,
    },
    /// The file does not hold what it should.
    #[error("the secret {path} {message}")]
    Malformed {
        /// The file.
        path: PathBuf,
        /// What is wrong with it, without its content.
        message: &'static str,
    },
}

/// Reads the secret at `path` as it is, without a final line break.
pub fn read_secret(path: &Path) -> Result<Secret, SecretError> {
    let mut bytes = std::fs::read(path).map_err(|source| SecretError::Read {
        path: path.to_owned(),
        source,
    })?;
    while matches!(bytes.last(), Some(b'\n' | b'\r')) {
        bytes.pop();
    }
    if bytes.is_empty() {
        return Err(SecretError::Malformed {
            path: path.to_owned(),
            message: "is empty",
        });
    }
    Ok(Secret(bytes))
}

/// Reads the secret at `path`, written in hexadecimal, as `N` bytes.
pub fn read_secret_hex<const N: usize>(path: &Path) -> Result<[u8; N], SecretError> {
    let secret = read_secret(path)?;
    let malformed = |message| SecretError::Malformed {
        path: path.to_owned(),
        message,
    };
    let text = secret.expose();
    if text.len() != N * 2 {
        return Err(malformed("has the wrong length"));
    }
    let digit = |byte: u8| match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    };
    let mut out = [0u8; N];
    for (slot, pair) in out.iter_mut().zip(text.chunks_exact(2)) {
        let (Some(high), Some(low)) = (digit(pair[0]), digit(pair[1])) else {
            return Err(malformed("is not hexadecimal"));
        };
        *slot = high << 4 | low;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_is_read_without_its_line_break_and_never_printed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token");
        std::fs::write(&path, "s3cret\n").unwrap();
        let secret = read_secret(&path).unwrap();
        assert_eq!(secret.expose(), b"s3cret");
        assert!(!format!("{secret:?}").contains("s3cret"));
    }

    /// The Compose file the deployment ships, as the repository holds it.
    const STACK: &str = include_str!("../../../../deploy/compose.yml");

    #[test]
    fn the_default_directory_is_the_one_the_stack_binds_every_secret_under() {
        let named: Vec<&str> = STACK
            .lines()
            .filter_map(|line| line.trim().strip_prefix("ORCH_SECRETS_DIR:"))
            .map(str::trim)
            .collect();
        assert_eq!(
            named,
            [DEFAULT_SECRETS_DIR],
            "the stack tells every service its secrets directory once"
        );
        let mounted: Vec<&str> = STACK
            .lines()
            .filter_map(|line| line.trim().strip_prefix("- ./config/secrets/"))
            .filter_map(|mount| mount.split(':').nth(1))
            .collect();
        assert!(!mounted.is_empty(), "the stack binds secrets");
        for target in mounted {
            // The web edge's own container has its own place for the one file it checks upstreams by.
            if target.starts_with("/etc/orch/secrets/") {
                continue;
            }
            assert_eq!(
                Path::new(target).parent(),
                Some(Path::new(DEFAULT_SECRETS_DIR)),
                "{target}"
            );
        }
    }

    #[test]
    fn a_key_in_hexadecimal_is_read_as_bytes_and_a_short_one_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key");
        std::fs::write(&path, "00ff10\n").unwrap();
        assert_eq!(read_secret_hex::<3>(&path).unwrap(), [0x00, 0xff, 0x10]);
        let error = read_secret_hex::<4>(&path).unwrap_err();
        assert!(!error.to_string().contains("00ff10"));
    }
}
