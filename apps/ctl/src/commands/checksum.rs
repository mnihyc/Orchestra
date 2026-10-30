//! The BLAKE3 hash of files, printed as `b3sum` prints it, for a host that has no `b3sum`.
//!
//! A release names every file with its BLAKE3 hash, and the updater checks the
//! files against them before it installs anything. Most hosts have no program
//! that computes that hash; this one is in the release, and prints what
//! `b3sum` would, so that a script uses either the same way.

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// How much of a file is read at a time: large enough that the disk, not the
/// loop, is the limit.
const CHUNK_BYTES: usize = 1 << 20;

/// The BLAKE3 hash of the file at `path`, in lowercase hexadecimal.
pub fn hash_file(path: &Path) -> Result<String, String> {
    let wrong = |error: io::Error| format!("cannot read {}: {error}", path.display());
    let mut file = File::open(path).map_err(wrong)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0_u8; CHUNK_BYTES];
    loop {
        let read = file.read(&mut buffer).map_err(wrong)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// Prints `<hash>  <path>` for each file, and fails at the first that cannot
/// be read.
pub fn run(files: &[PathBuf]) -> Result<(), String> {
    let mut output = io::stdout().lock();
    for path in files {
        let hash = hash_file(path)?;
        writeln!(output, "{hash}  {}", path.display()).map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hash_of_a_file_is_the_one_b3sum_prints() {
        let dir = tempfile::tempdir().expect("a directory");
        let empty = dir.path().join("empty");
        std::fs::write(&empty, b"").expect("a file");
        assert_eq!(
            hash_file(&empty).expect("hashed"),
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
        );
        let words = dir.path().join("words");
        std::fs::write(&words, b"orch").expect("a file");
        assert_eq!(
            hash_file(&words).expect("hashed"),
            blake3::hash(b"orch").to_hex().to_string()
        );
    }

    #[test]
    fn a_file_that_is_not_there_is_named_in_the_complaint() {
        let message = hash_file(Path::new("/nonexistent/orch-release")).unwrap_err();
        assert!(message.contains("/nonexistent/orch-release"), "{message}");
    }
}
