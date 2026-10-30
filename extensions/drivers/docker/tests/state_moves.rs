//! A stopped unit's files below one directory of its state, moved into another unit on a real Docker daemon: what a provider's runtime takes to another host, its sign-ins and harness homes, with their owners and modes.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::Lab;
use orch_driver_contract::driver::EnvironmentDriver;
use serde_json::json;

#[tokio::test]
async fn the_files_below_a_directory_of_a_stopped_units_state_go_into_another_unit_with_their_owners()
 {
    let Some(lab) = Lab::start().await else {
        return;
    };
    let old = lab.request("provider", "carried-from", |request| {
        request["env"] = json!({
            "LAB_RUN": "mkdir -p $ORCH_DATA_DIR/harness/p1/agent && printf signed-in > $ORCH_DATA_DIR/harness/p1/agent/auth.json && chmod 600 $ORCH_DATA_DIR/harness/p1/agent/auth.json && mkdir -p $ORCH_DATA_DIR/harness/other && printf no > $ORCH_DATA_DIR/harness/other/x"
        });
    });
    let old_unit = lab.run(&old).await;
    lab.driver
        .stop(&old_unit, std::time::Duration::from_secs(2))
        .await
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("state.tar");
    assert!(
        lab.driver
            .export_state(&old_unit, "harness/p1", &archive)
            .await
            .unwrap(),
        "the old unit keeps files there"
    );
    assert!(
        !lab.driver
            .export_state(&old_unit, "harness/nobody", &dir.path().join("none.tar"))
            .await
            .unwrap(),
        "nothing is taken where the unit keeps nothing"
    );
    // The new unit takes them before it first starts.
    let new = lab.request("provider", "carried-to", |_| {});
    let new_unit = lab.create(&new).await;
    lab.driver.import_state(&new_unit, &archive).await.unwrap();
    assert_eq!(
        lab.read(&new_unit, "/var/lib/orch/harness/p1/agent/auth.json")
            .await
            .as_deref(),
        Some("signed-in")
    );
    assert_eq!(
        lab.read(&new_unit, "/var/lib/orch/harness/other/x").await,
        None,
        "only the directory asked for goes"
    );
    // Its owner and mode went with it: the archive names the unit's user and keeps the file private.
    let mut read = tar::Archive::new(std::fs::File::open(&archive).unwrap());
    let auth = read
        .entries()
        .unwrap()
        .map(Result::unwrap)
        .find(|entry| entry.path().unwrap().ends_with("agent/auth.json"))
        .expect("the sign-in file is in the archive");
    assert_eq!(
        auth.path().unwrap().to_string_lossy(),
        "harness/p1/agent/auth.json"
    );
    assert_eq!(auth.header().uid().unwrap(), 1000);
    assert_eq!(auth.header().mode().unwrap() & 0o777, 0o600);
    // The new unit runs with it.
    lab.driver.start(&new_unit).await.unwrap();
    lab.wait_done(&new_unit).await;
    // A running unit's files are not taken: they would not stand still.
    let refused = lab
        .driver
        .export_state(&new_unit, "harness/p1", &dir.path().join("running.tar"))
        .await;
    assert!(refused.is_err());
}
