//! What a person has, turned into a spec on a real Docker daemon: a Dockerfile, an image of a registry and an
//! image archive. A proposal makes nothing; it only reads, and says what it could not carry over.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{BUSYBOX, Lab};
use futures::TryStreamExt;
use orch_driver_contract::driver::{DriverError, EnvironmentDriver};
use orch_driver_contract::imports::{ImportProposal, ImportRequest, NoteLevel};
use orch_driver_contract::runtime::RuntimeKind;
use orch_driver_contract::sources::Source;
use orch_driver_docker::build::{definition_alias, spec_alias};
use serde_json::json;

fn request(kind: RuntimeKind, format: &str, values: serde_json::Value) -> ImportRequest {
    ImportRequest {
        kind,
        format: format.to_owned(),
        values,
    }
}

fn says(proposal: &ImportProposal, level: NoteLevel, part: &str) -> bool {
    proposal
        .notes
        .iter()
        .any(|note| note.level == level && note.text.contains(part))
}

#[tokio::test]
async fn a_dockerfile_becomes_a_spec_with_a_note_for_what_it_does_not_carry_over_and_nothing_is_built()
 {
    let Some(lab) = Lab::start().await else {
        return;
    };
    let text = "FROM busybox:latest\nUSER 1000\nVOLUME /data\nENTRYPOINT [\"/bin/server\"]\nRUN echo imported\n";
    let proposal = lab
        .driver
        .import(&request(
            RuntimeKind::Environment,
            "dockerfile",
            json!({ "dockerfile": text }),
        ))
        .await
        .unwrap();
    assert_eq!(proposal.spec["build"]["dockerfile"], text);
    assert_eq!(proposal.spec["user"], json!({ "uid": 1000, "gid": 1000 }));
    assert!(says(&proposal, NoteLevel::Info, "FROM busybox:latest"));
    assert!(says(&proposal, NoteLevel::Info, "1000"));
    assert!(says(&proposal, NoteLevel::Warning, "/data"));
    assert!(says(&proposal, NoteLevel::Warning, "/bin/server"));
    assert!(!proposal.is_blocked());
    // The driver takes what it proposes.
    lab.driver
        .validate(RuntimeKind::Environment, &proposal.spec)
        .await
        .unwrap();
    // Nothing was built: the image of the proposal is not on the host, and a Git address and an archive become
    // the context of the build.
    let Source::Build(build) = Source::from_spec(&proposal.spec).unwrap() else {
        panic!("a build");
    };
    let definition = definition_alias(&lab.config, &build.definition_hash(None));
    lab.track_image(&definition);
    lab.track_image(&spec_alias(&lab.config, &build));
    assert!(lab.docker.inspect_image(&definition).await.is_err());
    let with_git = lab
        .driver
        .import(&request(
            RuntimeKind::Provider,
            "dockerfile",
            json!({ "dockerfile": "FROM busybox\nCOPY app /app\n", "git": "https://example.invalid/repo.git", "git_ref": "v1" }),
        ))
        .await
        .unwrap();
    assert_eq!(
        with_git.spec["build"]["context"],
        json!({ "git": "https://example.invalid/repo.git", "ref": "v1", "path": "." })
    );
    assert!(!says(&with_git, NoteLevel::Warning, "no context was given"));
    let without = lab
        .driver
        .import(&request(
            RuntimeKind::Provider,
            "dockerfile",
            json!({ "dockerfile": "FROM busybox\nCOPY app /app\n" }),
        ))
        .await
        .unwrap();
    assert!(says(&without, NoteLevel::Warning, "no context was given"));
}

#[tokio::test]
async fn an_image_is_proposed_from_what_the_host_reads_in_it_and_pinned_to_its_digest() {
    let Some(lab) = Lab::start().await else {
        return;
    };
    let proposal = lab
        .driver
        .import(&request(
            RuntimeKind::Environment,
            "image",
            json!({ "reference": BUSYBOX }),
        ))
        .await
        .unwrap();
    assert_eq!(proposal.spec["image"], BUSYBOX);
    let digest = proposal.spec["image_digest"].as_str().unwrap();
    assert!(
        digest.starts_with("sha256:") && digest.len() == 71,
        "{digest}"
    );
    assert!(says(&proposal, NoteLevel::Info, "pinned"));
    assert!(says(&proposal, NoteLevel::Info, "MiB"));
    assert!(!proposal.is_blocked());
    lab.driver
        .validate(RuntimeKind::Environment, &proposal.spec)
        .await
        .unwrap();
    // The pinned digest is the image the host holds.
    let held = lab.docker.inspect_image(BUSYBOX).await.unwrap();
    assert!(
        held.repo_digests
            .unwrap()
            .iter()
            .any(|known| known.ends_with(digest))
    );
    // An image that is not to be had is an image error, and a reference that is not one is refused.
    let missing = lab
        .driver
        .import(&request(
            RuntimeKind::Environment,
            "image",
            json!({ "reference": "localhost:1/orch-test/missing:never" }),
        ))
        .await
        .unwrap_err();
    assert!(matches!(missing, DriverError::Image(_)), "{missing:?}");
}

#[tokio::test]
async fn an_archive_is_read_for_the_image_in_it_without_loading_it() {
    let Some(lab) = Lab::start().await else {
        return;
    };
    let pieces: Vec<bytes::Bytes> = lab
        .docker
        .export_image(BUSYBOX)
        .try_collect()
        .await
        .unwrap();
    let blob = lab.blobs.put(
        pieces
            .iter()
            .flat_map(|piece| piece.to_vec())
            .collect::<Vec<u8>>(),
    );
    let proposal = lab
        .driver
        .import(&request(
            RuntimeKind::Provider,
            "archive",
            json!({ "blob": blob.to_string() }),
        ))
        .await
        .unwrap();
    assert_eq!(
        proposal.spec["archive"],
        json!({ "blob": blob.to_string(), "image": null })
    );
    assert!(says(&proposal, NoteLevel::Info, "busybox"));
    assert!(!proposal.is_blocked());
    assert_eq!(lab.blobs.opened(&blob), 1);
    // The archive was only read: the name it would be loaded under does not exist.
    let alias = orch_driver_docker::images::archive_alias(
        &lab.config,
        &orch_driver_contract::sources::ArchiveSource { blob, image: None },
    );
    lab.track_image(&alias);
    assert!(lab.docker.inspect_image(&alias).await.is_err());
    // An image the archive does not hold is refused with what it does hold, and junk is not an archive.
    let other = lab
        .driver
        .import(&request(
            RuntimeKind::Provider,
            "archive",
            json!({ "blob": blob.to_string(), "image": "not-in-it:1" }),
        ))
        .await
        .unwrap_err();
    assert!(
        matches!(other, DriverError::InvalidSpec(ref why) if why.contains("not-in-it:1")),
        "{other:?}"
    );
    let junk = lab.blobs.put(vec![7_u8; 2048]);
    let error = lab
        .driver
        .import(&request(
            RuntimeKind::Provider,
            "archive",
            json!({ "blob": junk.to_string() }),
        ))
        .await
        .unwrap_err();
    assert!(
        matches!(error, DriverError::InvalidSpec(ref why) if why.starts_with("the archive cannot be read") && !why.chars().any(char::is_control)),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_format_the_driver_does_not_declare_is_unsupported_and_values_that_do_not_fit_are_refused()
 {
    let Some(lab) = Lab::start().await else {
        return;
    };
    let unknown = lab
        .driver
        .import(&request(RuntimeKind::Environment, "compose", json!({})))
        .await
        .unwrap_err();
    assert!(
        matches!(unknown, DriverError::Unsupported(_)),
        "{unknown:?}"
    );
    assert_eq!(unknown.code(), "unsupported");
    for values in [
        json!({}),
        json!({ "dockerfile": "" }),
        json!({ "dockerfile": 7 }),
        json!({ "dockerfile": "FROM busybox", "nothing": 1 }),
    ] {
        let refused = lab
            .driver
            .import(&request(
                RuntimeKind::Environment,
                "dockerfile",
                values.clone(),
            ))
            .await
            .unwrap_err();
        assert!(
            matches!(refused, DriverError::InvalidSpec(_)),
            "{values}: {refused:?}"
        );
    }
    // Every format the driver declares is one it answers.
    for def in lab.driver.describe().imports {
        let answer = lab
            .driver
            .import(&request(RuntimeKind::Environment, &def.format, json!({})))
            .await;
        assert!(
            !matches!(answer, Err(DriverError::Unsupported(_))),
            "{} is declared and not done: {answer:?}",
            def.format
        );
    }
}
