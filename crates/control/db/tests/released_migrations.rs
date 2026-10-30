//! Released migration bytes remain identical to the tag's checksum ledger.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use sha2::{Digest, Sha384};
use std::{
    collections::BTreeSet,
    path::{Component, Path},
};

fn checksum(bytes: &[u8]) -> String {
    Sha384::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn unchanged(
    ledger: &str,
    mut read: impl FnMut(&str) -> std::io::Result<Vec<u8>>,
) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for (index, line) in ledger.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() != 2
            || fields[0].len() != 96
            || !fields[0].bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(format!(
                "RELEASED line {} must be a SHA-384 checksum and SQL filename",
                index + 1
            ));
        }
        let file = fields[1];
        let parts: Vec<_> = Path::new(file).components().collect();
        if !matches!(parts.as_slice(), [Component::Normal(_)])
            || !file.ends_with(".sql")
            || !seen.insert(file)
        {
            return Err(format!(
                "RELEASED names one unique migration filename, not {file}"
            ));
        }
        let bytes = read(file)
            .map_err(|_| format!("released migration {file} is missing or unreadable"))?;
        let actual = checksum(&bytes);
        if !fields[0].eq_ignore_ascii_case(&actual) {
            return Err(format!(
                "released migration {file} changed; restore it and add a new numbered migration"
            ));
        }
    }
    Ok(())
}

#[test]
fn every_released_migration_keeps_the_checksum_recorded_at_its_tag() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../migrations");
    let ledger = std::fs::read_to_string(directory.join("RELEASED")).unwrap();
    unchanged(&ledger, |file| std::fs::read(directory.join(file))).unwrap();
}

#[test]
fn editing_or_losing_a_released_migration_is_refused_while_new_files_need_no_old_checksum() {
    let original = b"CREATE TABLE example (id bigint);\n";
    let ledger = format!("{}  0001_example.sql\n", checksum(original));
    assert!(unchanged(&ledger, |_| Ok(original.to_vec())).is_ok());
    assert!(
        unchanged(&ledger, |_| Ok(
            b"CREATE TABLE example (id text);\n".to_vec()
        ))
        .unwrap_err()
        .contains("0001_example.sql changed")
    );
    assert!(
        unchanged(&ledger, |_| Err(std::io::Error::from(
            std::io::ErrorKind::NotFound
        )))
        .unwrap_err()
        .contains("missing")
    );
    assert!(
        unchanged("", |_| panic!(
            "an empty pre-tag ledger names no released file"
        ))
        .is_ok()
    );
    assert!(unchanged("not-a-checksum 0001_example.sql", |_| Ok(original.to_vec())).is_err());
    let escape = format!("{} ../0001_example.sql", checksum(original));
    assert!(
        unchanged(&escape, |_| panic!(
            "a ledger cannot escape the migrations directory"
        ))
        .is_err()
    );
}
