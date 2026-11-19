//! Making an image available through the real Docker driver on a real daemon: a build tells the job how far it is with the lines the daemon prints, answers what it was built from, and when it fails keeps the end of its log on the job.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{BUSYBOX, Fleet, Lab, NodeUnderTest, World, code_of};
use orch_link_contract::commands::{NodeImageEnsure, SerialKey};
use orch_types::new_id;
use serde_json::json;

#[tokio::test]
async fn a_dockerfile_is_built_by_the_daemon_with_its_steps_told_to_the_job_and_a_failed_build_keeps_its_log()
 {
    let Some(lab) = Lab::start().await else {
        return;
    };
    let mut world = World::start(&lab.bridge).await;
    let mut node = NodeUnderTest::start(&world, &lab).await;
    let fleet = Fleet::new(&world, &lab, &node);

    // A build that works: its steps are the progress, and it says which image it started from.
    let job = new_id();
    let good = json!({ "build": { "dockerfile": format!("FROM {BUSYBOX}\nRUN echo {} > /marker\n", lab.token) } });
    // The command reply and durable reports travel independently. Bound both by the same patience.
    let deadline = tokio::time::Instant::now() + common::PATIENCE;
    let built = tokio::time::timeout_at(
        deadline,
        fleet.run(
            &SerialKey::Image("good".into()),
            &NodeImageEnsure {
                driver: "docker".into(),
                spec: good.clone(),
                job_id: Some(job),
            },
        ),
    )
    .await
    .expect("the build finishes within its patience")
    .expect("the image could not be built");
    assert!(built.digest.is_some());
    let record = built.built.expect("a build says what it was built from");
    assert!(
        record
            .bases
            .iter()
            .any(|base| base.reference.starts_with("busybox")),
        "{record:?}"
    );

    tokio::time::timeout_at(
        deadline,
        common::eventually("the build's terminal progress to be committed", || {
            let (world, host) = (&world, node.machine_id);
            async move {
                world.told(host, "node").await.iter().any(|(kind, body)| {
                    kind == "job.progress"
                        && body["job_id"] == json!(job)
                        && body["progress"]["percent"] == 100
                        && body["progress"]["phases"][0]["state"] == "done"
                })
            }
        }),
    )
    .await
    .expect("the terminal report shares the build's original patience");
    let reported = world.told(node.machine_id, "node").await;
    let progress: Vec<_> = reported
        .iter()
        .filter(|(kind, body)| kind == "job.progress" && body["job_id"] == json!(job))
        .map(|(_, body)| body.clone())
        .collect();
    assert!(!progress.is_empty());
    let last = progress.last().unwrap();
    assert_eq!(last["progress"]["percent"], 100);
    assert_eq!(last["progress"]["phases"][0]["state"], "done");
    assert!(last["log"].as_str().unwrap().contains("RUN echo"), "{last}");

    // Asking again finds it.
    fleet
        .run(
            &SerialKey::Image("good".into()),
            &NodeImageEnsure {
                driver: "docker".into(),
                spec: good,
                job_id: None,
            },
        )
        .await
        .expect("found again");

    // A build that fails: the command fails as an image failure and the job keeps the end of the log.
    let failing = new_id();
    let error = fleet
        .run(
            &SerialKey::Image("bad".into()),
            &NodeImageEnsure {
                driver: "docker".into(),
                spec: json!({ "build": { "dockerfile": format!("FROM {BUSYBOX}\nRUN echo {} && exit 3\n", lab.token) } }),
                job_id: Some(failing),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(code_of(&error), "image_failed", "{error}");
    // Provisioning records its selected runtime before an image command can fail.
    // Its log can arrive after that command's retained failure, on the separate node stream.
    assert!(
        orch_db::queries::machine_reports::owns_host_job(
            &mut world.test_db.pool.acquire().await.unwrap(),
            node.machine_id,
            failing,
        )
        .await
        .unwrap()
    );
    common::eventually("the log of the failed build", || {
        let (world, host) = (&world, node.machine_id);
        async move {
            world.told(host, "node").await.iter().any(|(kind, body)| {
                kind == "job.progress"
                    && body["job_id"] == json!(failing)
                    && body.get("log").is_some()
            })
        }
    })
    .await;
    let reported = world.told(node.machine_id, "node").await;
    let log = reported
        .iter()
        .rev()
        .find(|(kind, body)| {
            kind == "job.progress" && body["job_id"] == json!(failing) && body.get("log").is_some()
        })
        .map(|(_, body)| body["log"].as_str().unwrap().to_owned())
        .unwrap();
    assert!(log.contains("Step 2/2") || log.contains("exit"), "{log}");

    node.stop().await;
    world.stop().await;
}
