//! Discarded archive entries cannot bypass the expansion bound of a skill import.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_engine::skills::archive::{File, Limits, build, read_archive, unpack};

fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::default(),
    ));
    for (path, data) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        builder.append_data(&mut header, path, *data).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

#[test]
fn discarded_tar_entries_do_not_escape_the_archive_expansion_bound() {
    let limits = Limits {
        max_bytes: 1024,
        max_files: 2,
    };
    let skill = b"---\nname: bounded\ndescription: A small skill.\n---\n# Bounded\n";
    let discarded = vec![0_u8; usize::try_from(limits.archive_bytes()).unwrap() * 2];
    let bytes = archive(&[("SKILL.md", skill), (".git/objects/discarded", &discarded)]);
    assert!(
        bytes.len() < usize::try_from(limits.archive_bytes()).unwrap(),
        "the compressed upload fits; the decompressed input does not"
    );
    assert!(
        read_archive(&bytes, None, limits).is_err(),
        "discarding a repository's files must not permit unbounded decompression"
    );
    assert_eq!(
        build(
            read_archive(
                &archive(&[("SKILL.md", skill), (".git/HEAD", b"metadata")]),
                None,
                limits,
            )
            .unwrap(),
            limits,
        )
        .unwrap()
        .name,
        "bounded",
        "a skill inside the bound remains usable"
    );
}

#[test]
fn tar_metadata_is_bounded_before_an_entry_is_returned() {
    let limits = Limits {
        max_bytes: 1024,
        max_files: 2,
    };
    let skill: &[u8] = b"---\nname: bounded\ndescription: A small skill.\n---\n# Bounded\n";
    let empty: &[u8] = b"";
    let name = format!(
        ".git/{}",
        "a".repeat(usize::try_from(limits.archive_bytes()).unwrap() * 2)
    );
    for entries in [
        [(name.as_str(), empty), ("SKILL.md", skill)],
        [("SKILL.md", skill), (name.as_str(), empty)],
    ] {
        let bytes = archive(&entries);
        assert!(bytes.len() < usize::try_from(limits.archive_bytes()).unwrap());
        assert!(
            read_archive(&bytes, None, limits).is_err(),
            "GNU long-name metadata is part of the decompressed input budget, including after a valid skill"
        );
    }
}

#[test]
fn a_stored_version_keeps_its_generated_metadata_when_place_limits_are_narrowed() {
    let mut files = vec![File {
        path: "SKILL.md".to_owned(),
        data: b"---\nname: frozen\ndescription: A small skill.\n---\n# Frozen\n".to_vec(),
        executable: false,
    }];
    // These relative paths stay below four KiB, while their GNU names need more space than their tiny content.
    let prefix = "p/".repeat(1600);
    for name in ["one", "two", "three", "four"] {
        files.push(File {
            path: format!("{prefix}{name}"),
            data: vec![b'x'],
            executable: false,
        });
    }
    let made = build(
        files,
        Limits {
            max_bytes: 4096,
            max_files: 16,
        },
    )
    .unwrap();
    let narrowed = Limits {
        max_bytes: 128,
        max_files: 5,
    };
    let back = unpack(&made.archive, narrowed).unwrap();
    assert_eq!(back.len(), 5);
    assert_eq!(
        build(back, narrowed).unwrap().archive,
        made.archive,
        "a stored version is still the same files after the place lowers its limits"
    );
}
