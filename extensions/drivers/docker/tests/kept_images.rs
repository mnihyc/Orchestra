//! Real same-image replacements keep installed files, remove obsolete names and flatten their own layers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
use common::{BUSYBOX, Lab};
use orch_driver_contract::driver::EnvironmentDriver;
use orch_driver_contract::runtime::RuntimeSpec;
use orch_driver_contract::units::UnitRef;
use orch_types::new_id;
use serde_json::json;
use std::time::Duration;

fn successor(
    lab: &Lab,
    original: &RuntimeSpec,
    image: orch_driver_contract::kept::KeptImage,
    label: &str,
    script: &str,
) -> RuntimeSpec {
    let mut spec = original.clone();
    spec.runtime_id = new_id();
    spec.name = lab.name(label);
    spec.labels
        .insert("orch.runtime_id".into(), spec.runtime_id.to_string());
    spec.kept_image = Some(image);
    spec.env.insert("LAB_RUN".into(), script.into());
    spec
}

async fn stopped(lab: &Lab, unit: &UnitRef) {
    lab.driver.stop(unit, Duration::from_secs(2)).await.unwrap();
}

async fn started(lab: &Lab, unit: &UnitRef) {
    // Successors share state, so an earlier boot's completion file is not this boot's proof.
    let mut archive = tar::Builder::new(Vec::new());
    let pending = b"starting\n";
    let mut header = tar::Header::new_gnu();
    header.set_size(pending.len() as u64);
    header.set_mode(0o644);
    archive
        .append_data(&mut header, "done.txt", pending.as_slice())
        .unwrap();
    let options = bollard::query_parameters::UploadToContainerOptionsBuilder::default()
        .path("/var/lib/orch")
        .build();
    lab.docker
        .upload_to_container(
            &unit.reference,
            Some(options),
            bollard::body_full(archive.into_inner().unwrap().into()),
        )
        .await
        .unwrap();
    lab.driver.start(unit).await.unwrap();
    lab.wait_done(unit).await;
}

async fn run(lab: &Lab, spec: &RuntimeSpec) -> UnitRef {
    let unit = lab.create(spec).await;
    started(lab, &unit).await;
    unit
}

#[tokio::test]
async fn two_recreates_keep_usr_opt_and_etc_files_and_leave_only_the_newest_owned_name() {
    let Some(lab) = Lab::start().await else {
        return;
    };
    lab.have(BUSYBOX).await;
    let environment = new_id();
    let first=lab.request("environment","keep-first",|request| {
        request["spec"]["user"]=json!({"uid":0,"gid":0});
        request["env"]=json!({"LAB_RUN":"mkdir -p /usr/local; echo first > /opt/installed-before; echo setting > /etc/kept-setting; echo program > /usr/local/kept-program"});
    });
    let old = run(&lab, &first).await;
    stopped(&lab, &old).await;
    let capture = new_id();
    let one = lab
        .driver
        .keep_files(&old, environment, 2, capture)
        .await
        .unwrap();
    lab.track_image(&one.reference);
    assert_eq!(one.reference, format!("orch-keep-{environment}:2"));
    assert!(one.bytes > 0);
    assert_eq!(
        lab.driver
            .keep_files(&old, environment, 2, capture)
            .await
            .unwrap(),
        one,
        "the same durable command reuses its exact snapshot"
    );
    let second = successor(
        &lab,
        &first,
        one.clone(),
        "keep-second",
        "cat /opt/installed-before /etc/kept-setting /usr/local/kept-program > /var/lib/orch/seen-kept; echo second > /opt/installed-second",
    );
    let current = run(&lab, &second).await;
    assert_eq!(
        lab.read(&current, "/var/lib/orch/seen-kept")
            .await
            .as_deref(),
        Some("first\nsetting\nprogram\n")
    );
    stopped(&lab, &current).await;
    let two = lab
        .driver
        .keep_files(&current, environment, 3, new_id())
        .await
        .unwrap();
    lab.track_image(&two.reference);
    let third = successor(
        &lab,
        &second,
        two.clone(),
        "keep-third",
        "cat /opt/installed-before /opt/installed-second > /var/lib/orch/seen-kept",
    );
    let current = run(&lab, &third).await;
    lab.driver.remove_kept_image(&one).await.unwrap();
    let kept = lab.driver.kept_images(&lab.system).await.unwrap();
    assert_eq!(kept, vec![two.clone()]);
    assert_eq!(
        lab.read(&current, "/var/lib/orch/seen-kept")
            .await
            .as_deref(),
        Some("first\nsecond\n")
    );
    assert!(two.bytes >= one.bytes);
}

#[tokio::test]
async fn a_failed_candidate_name_is_removed_while_the_old_unit_and_its_files_can_be_restored() {
    let Some(lab) = Lab::start().await else {
        return;
    };
    lab.have(BUSYBOX).await;
    let environment = new_id();
    let first = lab.request("environment", "keep-rollback", |request| {
        request["spec"]["user"] = json!({"uid":0,"gid":0});
        request["env"] = json!({"LAB_RUN":"echo installed > /opt/rollback-kept"});
    });
    let old = run(&lab, &first).await;
    stopped(&lab, &old).await;
    let failed = lab
        .driver
        .keep_files(&old, environment, 2, new_id())
        .await
        .unwrap();
    lab.track_image(&failed.reference);
    let mut bad = successor(&lab, &first, failed.clone(), "keep-bad", "true");
    bad.spec["runtime"] = json!("orch-test-no-such-runtime");
    lab.track(&bad);
    assert!(lab.driver.create(&bad).await.is_err());
    lab.driver.remove_kept_image(&failed).await.unwrap();
    assert!(
        lab.driver
            .kept_images(&lab.system)
            .await
            .unwrap()
            .is_empty()
    );
    started(&lab, &old).await;
    assert_eq!(
        lab.read(&old, "/opt/rollback-kept").await.as_deref(),
        Some("installed\n")
    );
}

#[tokio::test]
async fn the_tenth_kept_layer_is_flattened_and_a_transferred_archive_restores_its_exact_snapshot() {
    let Some(lab) = Lab::start().await else {
        return;
    };
    lab.have(BUSYBOX).await;
    let environment = new_id();
    let first = lab.request("environment", "keep-flat-base", |request| {
        request["spec"]["user"] = json!({"uid":0,"gid":0});
        request["env"] = json!({"LAB_RUN":"echo zero > /opt/layer-0"});
    });
    let mut unit = run(&lab, &first).await;
    let mut spec = first.clone();
    let mut previous = None;
    for index in 1..=10 {
        stopped(&lab, &unit).await;
        let estimate = lab.driver.estimate_keep(&unit).await.unwrap();
        assert_eq!(estimate.flatten, index == 10);
        let image = lab
            .driver
            .keep_files(&unit, environment, index + 1, new_id())
            .await
            .unwrap();
        lab.track_image(&image.reference);
        assert_eq!(image.layers, if index == 10 { 1 } else { index as u32 });
        spec = successor(
            &lab,
            &spec,
            image.clone(),
            &format!("keep-flat-{index}"),
            &format!(
                "cat /opt/layer-0 > /var/lib/orch/flat-seen; echo {index} > /opt/layer-{index}"
            ),
        );
        unit = run(&lab, &spec).await;
        assert_eq!(
            lab.read(&unit, "/var/lib/orch/flat-seen").await.as_deref(),
            Some("zero\n")
        );
        if let Some(old) = previous {
            lab.driver.remove_kept_image(&old).await.unwrap();
        }
        previous = Some(image);
    }
    let image = previous.unwrap();
    let archive = lab.dir.path().join("kept-save.tar");
    lab.driver
        .export_kept_image(&image, &archive)
        .await
        .unwrap();
    assert!(std::fs::metadata(&archive).unwrap().len() > 0);
    // Drop the last test unit and its image name; older stopped units stay owned by the lab.
    stopped(&lab, &unit).await;
    lab.driver.remove(&unit).await.unwrap();
    lab.driver.remove_kept_image(&image).await.unwrap();
    lab.driver
        .import_kept_image(&image, &archive)
        .await
        .unwrap();
    let restored = successor(
        &lab,
        &spec,
        image.clone(),
        "kept-imported",
        "cat /opt/layer-0 /opt/layer-9 > /var/lib/orch/import-seen",
    );
    let unit = run(&lab, &restored).await;
    assert_eq!(
        lab.read(&unit, "/var/lib/orch/import-seen")
            .await
            .as_deref(),
        Some("zero\n9\n")
    );
    assert_eq!(
        lab.driver.kept_images(&lab.system).await.unwrap(),
        vec![image]
    );
}
