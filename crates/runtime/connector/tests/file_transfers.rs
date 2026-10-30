//! An archive entry uploads through the command transport, and a stale write preserves the host's captured stat.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod rig_environment;

use orch_link::commands::Progress;
use orch_link_contract::commands::{
    CommandBody, CommandKind, EnvDetach, ExpectedHash, FsArchiveRead, FsFetch, FsFetchResult,
    FsStat, FsWrite, Transfer,
};
use orch_link_contract::content::MAX_CHUNK_BYTES;
use orch_types::Hash;
use rig_environment::EnvRig;
use serde_json::json;

/// The tar format's 512-byte records: one header, one padded payload and two end records.
const TAR_RECORD_BYTES: usize = 512;

#[tokio::test]
async fn a_workspace_file_is_streamed_whole_at_the_exact_cap_even_when_remote_writes_are_off() {
    let mut rig = EnvRig::new();
    rig.config.environment.remote_write = false;
    let directory = rig.directory("work");
    // Larger than one link piece, with binary bytes including NUL: no text conversion or
    // whole-file command frame can stand in for the content transfer.
    let length = usize::try_from(MAX_CHUNK_BYTES).unwrap() + 17;
    let bytes: Vec<u8> = (0..=255).cycle().take(length).collect();
    std::fs::write(directory.join("result.bin"), &bytes).unwrap();
    std::fs::write(directory.join("empty.txt"), []).unwrap();
    rig.start().await;
    rig.attach(&directory).await;
    let answer: FsFetchResult = serde_json::from_value(
        rig.succeed(&FsFetch {
            environment_id: rig.environment,
            mount: None,
            path: "result.bin".into(),
            expect: None,
            max_bytes: Some(i64::try_from(length).unwrap()),
        })
        .await,
    )
    .unwrap();
    assert_eq!(answer.hash, Hash::of(&bytes));
    assert_eq!(answer.size, i64::try_from(bytes.len()).unwrap());
    assert_eq!(
        rig.blobs.content(&answer.hash).unwrap().as_ref(),
        bytes.as_slice()
    );
    let requests = rig.blobs.requests();
    let pieces: Vec<_> = requests.iter().filter(|(kind, _)| kind == "put").collect();
    assert!(
        pieces.len() > 1,
        "the file must travel in more than one piece"
    );
    assert!(
        pieces
            .iter()
            .all(|(_, size)| i64::try_from(*size).unwrap() <= MAX_CHUNK_BYTES)
    );
    let empty = rig
        .succeed(&FsFetch {
            environment_id: rig.environment,
            mount: None,
            path: "empty.txt".into(),
            expect: None,
            max_bytes: Some(0),
        })
        .await;
    assert_eq!(empty["size"], 0);
    assert_eq!(rig.blobs.content(&Hash::of(b"")).unwrap().len(), 0);
    rig.stop().await;
}

#[tokio::test]
async fn a_workspace_file_above_the_callers_limit_is_refused_in_words_without_a_blob() {
    let mut rig = EnvRig::new();
    let directory = rig.directory("work");
    std::fs::write(directory.join("result.txt"), b"12345").unwrap();
    rig.start().await;
    rig.attach(&directory).await;
    let fetch = FsFetch {
        environment_id: rig.environment,
        mount: None,
        path: "result.txt".into(),
        expect: None,
        max_bytes: Some(4),
    };
    let (code, detail) = rig.failure(&fetch).await;
    assert_eq!(code, "too_large");
    assert!(
        detail.contains("result.txt") && detail.contains("5 bytes") && detail.contains("4"),
        "{detail}"
    );
    assert!(rig.blobs.content(&Hash::of(b"12345")).is_none());
    let (code, detail) = rig
        .failure(&FsFetch {
            max_bytes: Some(-1),
            ..fetch
        })
        .await;
    assert_eq!(code, "unsupported");
    assert!(
        detail.contains("limit") && detail.contains("zero or greater"),
        "{detail}"
    );
    rig.stop().await;
}

#[cfg(unix)]
#[tokio::test]
async fn a_workspace_file_read_cannot_escape_its_root_or_read_a_detached_environment() {
    let mut rig = EnvRig::new();
    let directory = rig.directory("work");
    let personal = rig.directory("personal");
    let secret = personal.join("secret.txt");
    std::fs::write(&secret, b"private outside the environment").unwrap();
    std::fs::write(directory.join("result.txt"), b"inside the environment").unwrap();
    std::os::unix::fs::symlink(&secret, directory.join("escape.txt")).unwrap();
    std::os::unix::fs::symlink(&personal, directory.join("escape-dir")).unwrap();
    std::os::unix::fs::symlink("result.txt", directory.join("inside.txt")).unwrap();
    rig.start().await;
    rig.attach(&directory).await;
    let fetch = FsFetch {
        environment_id: rig.environment,
        mount: None,
        path: "result.txt".into(),
        expect: None,
        max_bytes: Some(1024),
    };
    let inside = rig
        .succeed(&FsFetch {
            path: "inside.txt".into(),
            ..fetch.clone()
        })
        .await;
    assert_eq!(inside["hash"], json!(Hash::of(b"inside the environment")));
    for path in [
        "../personal/secret.txt".to_owned(),
        secret.display().to_string(),
        "escape.txt".into(),
        "escape-dir/secret.txt".into(),
    ] {
        let (code, detail) = rig
            .failure(&FsFetch {
                path: path.clone(),
                ..fetch.clone()
            })
            .await;
        assert_eq!(code, "not_found", "{path}: {detail}");
        assert!(!detail.is_empty());
        if path.starts_with("escape") {
            assert!(detail.contains("outside the workspace"), "{detail}");
        }
    }
    assert!(
        rig.blobs
            .content(&Hash::of(b"private outside the environment"))
            .is_none()
    );
    assert_eq!(
        std::fs::read(secret).unwrap(),
        b"private outside the environment"
    );
    let (code, detail) = rig
        .failure(&FsFetch {
            path: String::new(),
            ..fetch.clone()
        })
        .await;
    assert_eq!(code, "unsupported");
    assert!(detail.contains("not a regular file"), "{detail}");
    rig.succeed(&EnvDetach {
        environment_id: rig.environment,
    })
    .await;
    let (code, detail) = rig.failure(&fetch).await;
    assert_eq!(code, "not_found");
    assert!(detail.contains("not attached here"), "{detail}");
    rig.stop().await;
}

fn archive_of(contents: &[u8]) -> Vec<u8> {
    assert!(contents.len() <= TAR_RECORD_BYTES);
    let mut archive = vec![0; TAR_RECORD_BYTES * 4];
    let header = &mut archive[..TAR_RECORD_BYTES];
    header[..8].copy_from_slice(b"data.txt");
    header[100..108].copy_from_slice(b"0000644\0");
    header[108..116].copy_from_slice(b"0000000\0");
    header[116..124].copy_from_slice(b"0000000\0");
    header[124..136].copy_from_slice(format!("{:011o}\0", contents.len()).as_bytes());
    header[136..148].copy_from_slice(b"00000000000\0");
    header[148..156].fill(b' ');
    header[156] = b'0';
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    let checksum: u32 = header.iter().map(|byte| u32::from(*byte)).sum();
    header[148..156].copy_from_slice(format!("{checksum:06o}\0 ").as_bytes());
    archive[TAR_RECORD_BYTES..TAR_RECORD_BYTES + contents.len()].copy_from_slice(contents);
    archive
}

#[tokio::test]
async fn reading_an_archive_entry_uploads_exactly_its_bytes_under_the_catalogues_transfer_direction()
 {
    let mut rig = EnvRig::new();
    let directory = rig.directory("work");
    std::fs::write(directory.join("bundle.tar"), archive_of(b"hello")).unwrap();
    rig.start().await;
    rig.attach(&directory).await;
    assert_eq!(FsArchiveRead::KIND.transfer(), Transfer::Upload);
    assert_eq!(
        CommandKind::NodeImageEnsure.transfer(),
        Transfer::Download,
        "image build context goes from the system to the node"
    );
    let result = rig
        .succeed(&FsArchiveRead {
            environment_id: rig.environment,
            path: "bundle.tar".into(),
            entry: "data.txt".into(),
        })
        .await;
    assert_eq!(result["blob"]["hash"], json!(Hash::of(b"hello")));
    assert_eq!(result["blob"]["size"], 5);
    assert_eq!(
        rig.blobs.content(&Hash::of(b"hello")).unwrap().as_ref(),
        b"hello"
    );
    let stat: orch_link_contract::foreign::FileStat = serde_json::from_value(
        rig.succeed(&FsStat {
            environment_id: rig.environment,
            mount: None,
            path: "bundle.tar".into(),
        })
        .await,
    )
    .unwrap();
    let same: orch_api_contract::workspace::FileStat = stat;
    assert_eq!(same.entry.name, "bundle.tar");
    assert_eq!(same.entry.path, "bundle.tar");
    assert_eq!(
        same.entry.size,
        i64::try_from(archive_of(b"hello").len()).unwrap()
    );
    rig.stop().await;
}

#[tokio::test]
async fn a_changed_file_is_not_overwritten_and_its_captured_stat_survives_the_connector_error_frame()
 {
    let mut rig = EnvRig::new();
    let directory = rig.directory("work");
    let path = directory.join("notes.txt");
    std::fs::write(&path, b"first version").unwrap();
    rig.start().await;
    rig.attach(&directory).await;
    std::fs::write(&path, b"second version is newer").unwrap();
    let sent = rig.stored(b"replacement");
    let error = match rig
        .send(&FsWrite {
            work: None,
            environment_id: rig.environment,
            path: "notes.txt".into(),
            blob: sent,
            mode: None,
            expected_hash: Some(ExpectedHash::Is(Hash::of(b"first version"))),
        })
        .await
    {
        Progress::Failed(error) => error,
        other => panic!("a stale write was expected: {other:?}"),
    };
    assert_eq!(error.code, "conflict");
    assert_eq!(
        error.details.as_ref().unwrap()["current"]["hash"],
        json!(Hash::of(b"second version is newer"))
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"second version is newer");
    std::fs::write(&path, b"third version").unwrap();
    let frame = serde_json::to_value(&error).unwrap();
    assert_eq!(
        frame["details"]["current"]["hash"],
        json!(Hash::of(b"second version is newer"))
    );
    rig.stop().await;
}
