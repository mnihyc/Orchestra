//! Raised memory and process limits are visible on the same Docker unit and its existing process stays alive.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
use common::{BUSYBOX, Lab};
use orch_driver_contract::driver::EnvironmentDriver;
use orch_driver_contract::runtime::Resources;
use serde_json::json;

#[tokio::test]
async fn raising_live_limits_keeps_the_container_and_its_long_running_process() {
    let Some(lab) = Lab::start().await else {
        return;
    };
    lab.have(BUSYBOX).await;
    if !lab.driver.describe().live_changes.resources {
        eprintln!("not run: this daemon does not enforce live limits");
        return;
    }
    let request=lab.request("environment","live-limits",|request| {
        request["spec"]["user"]=json!({"uid":0,"gid":0});request["resources"]=json!({"memory_mb":256,"pids":64});
        request["env"]=json!({"LAB_RUN":"sleep 3500 & echo $! > /opt/process-kept; echo installed > /opt/file-kept"});
    });
    let unit = lab.run(&request).await;
    let pid = lab.inspect(&unit).await.state.unwrap().pid;
    let process = lab.read(&unit, "/opt/process-kept").await.unwrap();
    lab.driver
        .apply_resources(
            &unit,
            &Resources {
                cpu_millis: Some(1500),
                memory_mb: Some(512),
                memory_swap_mb: lab.driver.facts().swap_limit.then_some(512),
                blkio_weight: Some(600),
                pids: Some(128),
                ..Resources::default()
            },
        )
        .await
        .unwrap();
    let inspected = lab.inspect(&unit).await;
    assert_eq!(inspected.id.as_deref(), Some(unit.reference.as_str()));
    assert_eq!(inspected.state.unwrap().pid, pid);
    let host = inspected.host_config.unwrap();
    assert_eq!(host.memory, Some(512 << 20));
    assert_eq!(host.pids_limit, Some(128));
    assert_eq!(host.nano_cpus, Some(1_500_000_000));
    assert_eq!(host.blkio_weight, Some(600));
    if lab.driver.facts().swap_limit {
        assert_eq!(host.memory_swap, Some(512 << 20));
    }
    assert_eq!(
        lab.driver.stats(&unit).await.unwrap().memory_limit_bytes,
        Some(512 << 20)
    );
    assert_eq!(lab.read(&unit, "/opt/process-kept").await.unwrap(), process);
    assert_eq!(
        lab.read(&unit, "/opt/file-kept").await.as_deref(),
        Some("installed\n")
    );
    assert!(
        matches!(
            lab.driver
                .apply_resources(&unit, &Resources::default())
                .await,
            Err(orch_driver_contract::driver::DriverError::Unsupported(_))
        ),
        "zero-valued Docker updates must not be called an unlimited result"
    );
    let unchanged = lab.inspect(&unit).await;
    assert_eq!(unchanged.state.unwrap().pid, pid);
    assert_eq!(unchanged.host_config.unwrap().memory, Some(512 << 20));
}
