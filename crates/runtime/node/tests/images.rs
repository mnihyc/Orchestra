//! Making an image available, on the in-memory hub with the in-memory driver: the digest the host holds, the progress a job is told as it goes, the end of a failed build's log kept on the job, the blobs a command names, a tag that moved and a template proposed from what a person has.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::sync::atomic::Ordering;
use std::time::Duration;

use common::Rig;
use orch_driver_contract::descriptor::DriverDescriptor;
use orch_driver_contract::driver::DriverError;
use orch_driver_contract::imports::{ImportNote, ImportProposal, NoteLevel};
use orch_driver_contract::runtime::RuntimeKind;
use orch_driver_contract::sources::SourceKind;
use orch_link_contract::commands::{
    NodeImageEnsure, NodeImageEnsureResult, NodeTemplateImport, NodeTemplateImportResult,
    RuntimeKind as LinkKind, SerialKey,
};
use orch_testkit::doubles::driver::MemoryDriver;
use orch_types::{Hash, Id, new_id};
use serde_json::json;

fn builder() -> MemoryDriver {
    MemoryDriver::new("memory").replacing().with_sources(vec![
        SourceKind::Image,
        SourceKind::Build,
        SourceKind::Archive,
    ])
}

fn ensure(spec: serde_json::Value, job: Option<Id>) -> NodeImageEnsure {
    NodeImageEnsure {
        driver: "memory".to_owned(),
        spec,
        job_id: job,
    }
}

fn key(name: &str) -> SerialKey {
    SerialKey::Image(name.to_owned())
}

fn result_of(value: serde_json::Value) -> NodeImageEnsureResult {
    serde_json::from_value(value).unwrap()
}

#[tokio::test]
async fn image_observations_from_two_drivers_never_mark_each_others_units_as_outdated() {
    use orch_node_core::drivers::DriverHost;
    use std::sync::{Arc, atomic::AtomicUsize};
    let mut rig = Rig::with_driver(builder());
    rig.have_connector();
    let second = Arc::new(
        MemoryDriver::new("second-image-driver")
            .replacing()
            .with_sources(vec![SourceKind::Image]),
    );
    let extras = Arc::new(common::MemoryExtras {
        driver: second.clone(),
        rebuilt: AtomicUsize::new(0),
        registry_looks: AtomicUsize::new(0),
    });
    rig.driver.set_registry_digest("debian:13", "sha256:first");
    second.set_registry_digest("debian:13", "sha256:second");
    let drivers = DriverHost::of(
        &rig.settings,
        rig.desk.clone(),
        vec![
            common::hosted(&rig.driver, &rig.extras),
            common::hosted(&second, &extras),
        ],
    );
    rig.start_with(drivers).await;
    let image = json!({"image":"debian:13"});
    let mut first = rig.spec(RuntimeKind::Environment, "first-image-driver");
    first.spec = image.clone();
    rig.succeed(&key("first-source"), &ensure(image.clone(), None))
        .await;
    rig.made(&first).await;
    let mut other = rig.spec(RuntimeKind::Environment, "second-image-driver");
    other.driver = "second-image-driver".into();
    other.spec = image.clone();
    rig.succeed(
        &key("second-source"),
        &NodeImageEnsure {
            driver: other.driver.clone(),
            spec: image,
            job_id: None,
        },
    )
    .await;
    rig.made(&other).await;
    assert!(
        rig.role().images().look().await.is_empty(),
        "each driver still has the image it observed"
    );
    rig.driver
        .set_registry_digest("debian:13", "sha256:first-updated");
    let changed = rig.role().images().look().await;
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].runtimes, [first.runtime_id]);
    assert!(rig.role().images().why_behind(other.runtime_id).is_none());
    rig.stop().await;
}

/// The `job.progress` events the node told for a job.
fn progress_of(rig: &Rig, job: Id) -> Vec<orch_testkit::doubles::controller::Applied> {
    rig.applied()
        .into_iter()
        .filter(|event| event.kind == "job.progress" && event.body["job_id"] == json!(job))
        .collect()
}

#[tokio::test]
async fn an_image_is_made_available_and_the_digest_the_host_holds_is_answered() {
    let mut rig = Rig::with_driver(builder());
    rig.driver.set_registry_digest("debian:13", "sha256:13");
    rig.start().await;
    let ensured = result_of(
        rig.succeed(
            &key("debian:13"),
            &ensure(json!({ "image": "debian:13" }), None),
        )
        .await,
    );
    assert_eq!(ensured.digest.as_deref(), Some("sha256:13"));
    assert!(ensured.built.is_none());
    // A build answers what it was built from.
    rig.driver.set_build_bases(&[("debian:13", "sha256:13")]);
    let built = result_of(
        rig.succeed(
            &key("a build"),
            &ensure(
                json!({ "build": { "dockerfile": "FROM debian:13\nRUN true" } }),
                None,
            ),
        )
        .await,
    );
    let record = built.built.unwrap();
    assert_eq!(
        (
            record.bases[0].reference.as_str(),
            record.bases[0].digest.as_str()
        ),
        ("debian:13", "sha256:13")
    );
    rig.stop().await;
}

#[tokio::test]
async fn a_driver_without_sources_makes_nothing_available_and_answers_no_digest() {
    let mut rig = Rig::new();
    rig.start().await;
    let ensured = result_of(
        rig.succeed(&key("x"), &ensure(json!({ "image": "x:1" }), None))
            .await,
    );
    assert_eq!(ensured.digest, None);
    rig.stop().await;
}

#[tokio::test]
async fn what_is_long_tells_the_job_how_far_it_is_at_most_once_a_second_and_ends_with_the_bar_full()
{
    let mut rig = Rig::with_driver(builder());
    rig.driver.set_image_lines(&[
        "aa11: Pulling fs layer",
        "bb22: Pulling fs layer",
        "aa11: Pull complete",
        "bb22: Pull complete",
    ]);
    rig.driver.set_image_pause(Duration::from_millis(700));
    rig.start().await;
    let job = new_id();
    let began = std::time::Instant::now();
    rig.succeed(
        &key("slow"),
        &ensure(json!({ "image": "slow:1" }), Some(job)),
    )
    .await;
    let took = began.elapsed();
    rig.wait_told("the end of the progress", |events| {
        events.iter().any(|event| {
            event.kind == "job.progress"
                && event.body["job_id"] == json!(job)
                && event.body["progress"]["percent"] == 100
        })
    })
    .await;

    let told = progress_of(&rig, job);
    assert!(told.len() >= 2, "{told:?}");
    let percents: Vec<i64> = told
        .iter()
        .map(|event| event.body["progress"]["percent"].as_i64().unwrap())
        .collect();
    assert!(
        percents.windows(2).all(|pair| pair[0] <= pair[1]),
        "a bar never goes back: {percents:?}"
    );
    assert_eq!(*percents.last().unwrap(), 100);
    let last = &told.last().unwrap().body["progress"];
    assert_eq!(last["phase"], "image");
    assert_eq!(last["phases"][0]["state"], "done");
    // In between, it says what the driver said, and how far the layers are.
    let between = &told[0].body["progress"];
    assert_eq!(between["phases"][0]["state"], "running");
    assert!(
        between["message"].as_str().unwrap().contains("Pull"),
        "{between}"
    );
    // And never more than once a second while it runs.
    let running = told
        .iter()
        .filter(|event| event.body["progress"]["phases"][0]["state"] == "running")
        .count();
    assert!(
        running as u64 <= took.as_secs() + 1,
        "{running} reports in {took:?}"
    );
    rig.stop().await;
}

#[tokio::test]
async fn an_image_made_for_no_job_tells_no_progress() {
    let mut rig = Rig::with_driver(builder());
    rig.driver.set_image_lines(&["aa: Pull complete"]);
    rig.start().await;
    rig.succeed(&key("quiet"), &ensure(json!({ "image": "quiet:1" }), None))
        .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        rig.applied()
            .iter()
            .all(|event| event.kind != "job.progress")
    );
    rig.stop().await;
}

#[tokio::test]
async fn a_build_that_fails_keeps_the_end_of_its_log_on_the_job_and_fails_the_command() {
    let mut rig = Rig::with_driver(builder());
    rig.driver.set_image_lines(&[
        "Step 1/2 : FROM debian:13",
        "Step 2/2 : RUN make",
        "make: *** [all] Error 2",
    ]);
    rig.driver.fail_image_after_lines(DriverError::Image(
        "the build failed: make: *** [all] Error 2".into(),
    ));
    rig.start().await;
    let job = new_id();
    let error = rig
        .refused(
            &key("broken"),
            &ensure(
                json!({ "build": { "dockerfile": "FROM debian:13\nRUN make" } }),
                Some(job),
            ),
        )
        .await;
    assert_eq!(error.code, "image_failed");
    assert!(error.detail.contains("Error 2"), "{}", error.detail);
    rig.wait_told("the log of the failed build", |events| {
        events.iter().any(|event| {
            event.kind == "job.progress"
                && event.body["job_id"] == json!(job)
                && event.body.get("log").is_some()
        })
    })
    .await;
    let last = progress_of(&rig, job).pop().unwrap().body;
    let log = last["log"].as_str().unwrap();
    assert!(
        log.contains("Step 2/2 : RUN make") && log.ends_with("Error 2\n"),
        "{log}"
    );
    assert_ne!(last["progress"]["phases"][0]["state"], "done");
    rig.stop().await;
}

#[tokio::test]
async fn a_blob_a_command_names_is_fetched_for_that_command_alone_and_its_file_goes_with_it() {
    let mut rig = Rig::with_driver(builder());
    rig.start().await;
    let content = bytes::Bytes::from_static(b"an image archive, in a manner of speaking");
    let hash = rig.blobs.insert(content.clone());
    let command = new_id();
    rig.blobs.open_command(command);

    let announced = rig.desk.announce(command, vec![hash]);
    let path = rig.desk.fetch(&hash).await.unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), content);
    // Nobody else's blob is fetched on this command's word.
    let other: Hash = Hash::of(b"somebody else's");
    assert_eq!(rig.desk.fetch(&other).await.unwrap_err().code, "not_found");
    rig.desk.finish(command).await;
    assert!(!path.exists(), "the files of a command go with it");
    drop(announced);
    assert_eq!(rig.desk.fetch(&hash).await.unwrap_err().code, "not_found");
    rig.stop().await;
}

#[tokio::test]
async fn a_tag_that_moved_is_noticed_and_the_unit_that_runs_the_old_image_says_so_until_it_is_updated()
 {
    let mut rig = Rig::with_driver(builder());
    rig.have_connector();
    rig.driver.set_registry_digest("debian:13", "sha256:old");
    rig.start().await;
    let mut spec = rig.spec(RuntimeKind::Environment, "tagged");
    spec.spec = json!({ "image": "debian:13" });
    rig.made(&spec).await;
    rig.start_runtime(spec.runtime_id).await;
    assert!(
        rig.role().images().look().await.is_empty(),
        "nothing moved yet"
    );

    rig.driver.set_registry_digest("debian:13", "sha256:new");
    let behind = rig.role().images().look().await;
    assert_eq!(behind.len(), 1);
    assert_eq!(behind[0].runtimes, [spec.runtime_id]);
    assert!(
        behind[0].why.contains("sha256:new") && behind[0].why.contains("sha256:old"),
        "{}",
        behind[0].why
    );

    rig.role().reconciler().pass().await;
    rig.wait_told("the note that an update is available", |events| {
        events.iter().any(|event| {
            event.kind == "node.runtime"
                && event.body["runtime_id"] == json!(spec.runtime_id)
                && event.body["state"] == "running"
                && event.body["detail"]
                    .as_str()
                    .is_some_and(|detail| detail.contains("an update is available"))
        })
    })
    .await;

    // The unit goes on running what it was made with: it is behind until it is made anew.
    assert!(rig.role().images().why_behind(spec.runtime_id).is_some());
    assert_eq!(
        rig.driver.state_of(&spec.name),
        orch_driver_contract::units::UnitStateKind::Running
    );
    rig.stop().await;
}

#[tokio::test]
async fn a_build_whose_base_image_moved_is_built_again_when_it_is_next_asked_for() {
    let mut rig = Rig::with_driver(builder());
    rig.have_connector();
    rig.driver.set_build_bases(&[("debian:13", "sha256:old")]);
    rig.driver.set_registry_digest("debian:13", "sha256:old");
    rig.start().await;
    let build = json!({ "build": { "dockerfile": "FROM debian:13\nRUN true" } });
    let mut spec = rig.spec(RuntimeKind::Environment, "built");
    spec.spec = build.clone();
    rig.succeed(&key("built"), &ensure(build.clone(), None))
        .await;
    rig.made(&spec).await;

    // Nothing moved: making it available again finds it.
    assert!(rig.role().images().look().await.is_empty());
    rig.succeed(&key("built"), &ensure(build.clone(), None))
        .await;
    assert_eq!(rig.extras.rebuilt.load(Ordering::SeqCst), 0);

    // The base moved: the build is behind, and the next ask builds it again.
    rig.driver.set_registry_digest("debian:13", "sha256:new");
    assert_eq!(rig.role().images().look().await.len(), 1);
    rig.driver.set_build_bases(&[("debian:13", "sha256:new")]);
    rig.succeed(&key("built"), &ensure(build.clone(), None))
        .await;
    assert_eq!(rig.extras.rebuilt.load(Ordering::SeqCst), 1);
    assert!(
        rig.role().images().behind().is_empty(),
        "built again, it is not behind"
    );
    assert!(rig.role().images().look().await.is_empty());
    rig.stop().await;
}

fn with_import() -> MemoryDriver {
    let descriptor: DriverDescriptor = serde_json::from_value(json!({
        "name": "memory", "version": "1", "title": "Units in memory",
        "spec_schemas": { "environment": {}, "provider": {} }, "volumes": true,
        "imports": [ { "format": "dockerfile", "title": "A Dockerfile", "fields": [] } ]
    }))
    .unwrap();
    MemoryDriver::new("memory").with_descriptor(descriptor)
}

fn import(format: &str, kind: LinkKind) -> NodeTemplateImport {
    NodeTemplateImport {
        driver: "memory".to_owned(),
        kind,
        format: format.to_owned(),
        values: json!({ "dockerfile": "FROM x" }),
    }
}

#[tokio::test]
async fn a_spec_is_proposed_from_what_a_person_has_with_a_note_for_what_could_not_be_carried_over()
{
    let driver = with_import();
    driver.set_import(
        "dockerfile",
        ImportProposal {
            spec: json!({ "build": { "dockerfile": "FROM x" } }),
            notes: vec![
                ImportNote {
                    level: NoteLevel::Info,
                    text: "the last USER is app".into(),
                },
                ImportNote {
                    level: NoteLevel::Warning,
                    text: "declared as a volume: /data".into(),
                },
                ImportNote {
                    level: NoteLevel::Blocker,
                    text: "another architecture".into(),
                },
            ],
        },
    );
    let mut rig = Rig::with_driver(driver);
    rig.start().await;
    let key = SerialKey::None;
    let proposal: NodeTemplateImportResult = serde_json::from_value(
        rig.succeed(&key, &import("dockerfile", LinkKind::Environment))
            .await,
    )
    .unwrap();
    assert_eq!(proposal.spec["build"]["dockerfile"], "FROM x");
    let levels: Vec<_> = proposal
        .notes
        .iter()
        .map(|note| serde_json::to_value(note.level).unwrap())
        .collect();
    assert_eq!(levels, ["info", "warning", "blocker"]);
    assert!(
        rig.driver.units().is_empty() && rig.driver.volumes().is_empty(),
        "an import makes nothing"
    );

    // A format the driver does not declare, and a driver that declares none, are unsupported.
    assert_eq!(
        rig.refused(&key, &import("archive", LinkKind::Provider))
            .await
            .code,
        "unsupported"
    );
    rig.stop().await;
    let mut plain = Rig::new();
    plain.start().await;
    assert_eq!(
        plain
            .refused(&key, &import("dockerfile", LinkKind::Environment))
            .await
            .code,
        "unsupported"
    );
    plain.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_shipped_node_never_polls_a_registry_and_an_explicit_schedule_does() {
    for requested in [false, true] {
        let mut rig = Rig::with_driver(builder());
        assert!(rig.settings.image_check_every.is_zero());
        if requested {
            rig.settings.image_check_every = Duration::from_secs(3600);
        }
        rig.have_connector();
        rig.start().await;
        let spec = rig.spec(RuntimeKind::Environment, "registry-policy");
        rig.made(&spec).await;
        rig.extras.registry_looks.store(0, Ordering::SeqCst);
        tokio::time::advance(Duration::from_secs(48 * 3600)).await;
        // One virtual timer tick drains ready tasks without waiting for another hourly deadline.
        tokio::time::sleep(Duration::from_millis(1)).await;
        let looked = rig.extras.registry_looks.load(Ordering::SeqCst);
        assert_eq!(
            looked > 0,
            requested,
            "only an operator's schedule may look outside by itself"
        );
        rig.stop().await;
    }
}
