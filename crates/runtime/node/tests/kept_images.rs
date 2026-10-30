//! A restarted node reports retained installed files, rejects stale owners and sweeps only unused exact images.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod common;
use common::{Rig, SYSTEM};
use orch_driver_contract::kept::{KeptImage, KeptOwner};
use orch_link::commands::Progress;
use orch_link_contract::commands::{NodeKeptSweep, SerialKey};
use orch_types::{Id, new_id};

fn image(environment: Id, generation: i64) -> KeptImage {
    KeptImage {
        system: SYSTEM.into(),
        environment,
        generation,
        capture: new_id(),
        reference: format!("orch-keep-{environment}:{generation}"),
        bytes: 123,
        total_bytes: 456,
        layers: 1,
    }
}

#[tokio::test]
async fn a_node_restart_reports_archived_images_and_its_next_sweep_keeps_their_owner_and_drops_an_orphan()
 {
    let mut rig = Rig::new();
    let kept = image(new_id(), 2);
    let orphan = image(new_id(), 1);
    rig.driver.remember_kept(kept.clone());
    rig.driver.remember_kept(orphan.clone());
    rig.start().await;
    rig.wait_told("the startup kept-image inventory", |events| {
        events.iter().any(|event| {
            event.kind == "node.kept-images"
                && event.body["images"]
                    .as_array()
                    .is_some_and(|images| images.len() == 2)
        })
    })
    .await;
    let epoch = rig
        .role
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .ledger()
        .kept_epoch();
    assert!(matches!(
        rig.send(
            &SerialKey::None,
            &NodeKeptSweep {
                epoch,
                owners: vec![KeptOwner {
                    environment: kept.environment,
                    generation: kept.generation,
                    capture: Some(kept.capture)
                }]
            }
        )
        .await,
        Progress::Succeeded(_)
    ));
    assert_eq!(rig.driver.kept_names(), vec![kept.reference.clone()]);
    rig.stop().await;
    let before = rig.applied().len();
    rig.start().await;
    rig.wait_told(
        "the kept image remains reported after another start",
        |events| {
            events.iter().skip(before).any(|event| {
                event.kind == "node.kept-images"
                    && event.body["images"].as_array().is_some_and(|images| {
                        images
                            .iter()
                            .any(|image| image["reference"] == kept.reference)
                    })
            })
        },
    )
    .await;
    let epoch = rig
        .role
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .ledger()
        .kept_epoch();
    assert!(matches!(
        rig.send(
            &SerialKey::None,
            &NodeKeptSweep {
                epoch,
                owners: vec![]
            }
        )
        .await,
        Progress::Succeeded(_)
    ));
    assert!(
        rig.driver.kept_names().is_empty(),
        "the confirmed purge relinquishes this archived generation"
    );
    rig.stop().await;
}

#[tokio::test]
async fn a_whitelist_from_before_a_new_capture_cannot_remove_that_capture() {
    let mut rig = Rig::new();
    rig.start().await;
    let old_epoch = rig
        .role
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .ledger()
        .kept_epoch();
    rig.role
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .ledger()
        .begin_kept_change()
        .unwrap();
    let fresh = image(new_id(), 1);
    rig.driver.remember_kept(fresh.clone());
    assert!(matches!(
        rig.send(
            &SerialKey::None,
            &NodeKeptSweep {
                epoch: old_epoch,
                owners: vec![]
            }
        )
        .await,
        Progress::Succeeded(_)
    ));
    assert_eq!(rig.driver.kept_names(), vec![fresh.reference]);
    rig.stop().await;
}
