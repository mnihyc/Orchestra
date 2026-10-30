//! A real Docker replacement keeps workspace, home and state while discarding the old unit's other files.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_driver_contract::{driver::EnvironmentDriver, runtime::RuntimeSpec};
use orch_driver_docker::{DockerDriver, config::DockerConfig};
use orch_types::{Json, new_id};
use serde_json::json;
use std::{path::PathBuf, time::Duration};
use tokio::process::Command;

/// A local daemon command is bounded, even when the daemon is busy.
const DOCKER_WAIT: Duration = Duration::from_secs(120);
/// The disposable connector script needs no time to shut down gracefully.
const STOP_GRACE: Duration = Duration::from_secs(1);
/// An hour keeps the disposable process alive beyond the bounded daemon calls; cleanup stops it.
const DUMMY_LIFETIME_SECS: u32 = 3_600;

fn journal(resources: &Json) {
    if let Some(path) = std::env::var_os("ORCH_UPDATE_DOCKER_JOURNAL") {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        writeln!(file,"\nDocker persistence test resources (recorded before creation, and again after cleanup):\n{}",resources).unwrap();
    }
}

async fn docker(args: &[&str]) -> Result<String, String> {
    let child = Command::new("docker")
        .args(args)
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(DOCKER_WAIT, child)
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[tokio::test]
async fn a_real_docker_successor_keeps_only_the_persistent_parts() {
    if docker(&["image", "inspect", "busybox:latest"])
        .await
        .is_err()
    {
        eprintln!(
            "skipped: the shared daemon has no busybox image; this test never pulls or builds an image"
        );
        return;
    }
    let token = new_id().simple().to_string();
    let first = format!("orch-upd-{token}-old");
    let second = format!("orch-upd-{token}-next");
    let env_network = format!("orchupd{token}-env");
    let ctl_network = format!("orchupd{token}-ctl");
    let volumes = [
        format!("orch-upd-{token}-workspace"),
        format!("orch-upd-{token}-home"),
        format!("orch-upd-{token}-state"),
    ];
    let root = std::env::var_os("ORCH_UPDATE_DOCKER_SCRATCH")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../.tmp/update-docker")
        });
    std::fs::create_dir_all(&root).unwrap();
    let program = root.join("orch-connector");
    std::fs::write(
        &program,
        format!("#!/bin/sh\nexec sleep {DUMMY_LIFETIME_SECS}\n"),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let log = root.join(format!("resources-{token}.json"));
    let resources = json!({"containers":[first,second,format!("{first}-prep"),format!("{second}-prep")],"networks":[env_network,ctl_network],"volumes":[volumes[0],volumes[1],volumes[2],format!("{first}-run"),format!("{second}-run")],"images_created":[],"image_reused":"busybox:latest","scratch":program,"cleaned":false});
    std::fs::write(&log, serde_json::to_vec_pretty(&resources).unwrap()).unwrap();
    journal(&resources);
    eprintln!("resource journal: {}", log.display());
    let mut config = DockerConfig::default();
    config.tool_cache.connector_dir = Some(root.clone());
    config.networks.env = env_network.clone();
    config.networks.ctl = ctl_network.clone();
    config
        .labels
        .insert("orch.update-test".into(), token.clone());
    let driver = DockerDriver::connect(config).await.unwrap();
    let request = |name: &str| -> RuntimeSpec {
        let runtime = new_id();
        serde_json::from_value(json!({"runtime_id":runtime,"kind":"environment","name":name,"driver":"docker","spec":{"image":"busybox:latest","runtime":"runc","user":{"uid":0,"gid":0},"resources":{}},"resources":{},"roles":["environment"],"gateway_url":"wss://gateway.orch.internal:41400","enroll_token":"disposable-test-token","ca_cert":"disposable-test-ca","tools":[],"volumes":{"workspace":{"name":volumes[0],"storage":null},"home":{"name":volumes[1],"storage":null},"state":{"name":volumes[2],"storage":null}},"mounts":[],"env":{},"labels":{"orch.system":format!("orchupd{token}"),"orch.runtime_id":runtime,"orch.kind":"environment","orch.update-test":token}})).unwrap()
    };
    let outcome=async {
        let before=driver.create(&request(&first)).await.map_err(|e|e.to_string())?;
        driver.start(&before).await.map_err(|e|e.to_string())?;
        docker(&["exec",&first,"sh","-c","printf kept-work > /workspace/proof; printf kept-home > /home/agent/proof; printf kept-state > /var/lib/orch/proof; printf discarded > /tmp/proof"]).await?;
        driver.stop(&before,STOP_GRACE).await.map_err(|e|e.to_string())?;
        let next=driver.create(&request(&second)).await.map_err(|e|e.to_string())?;
        driver.start(&next).await.map_err(|e|e.to_string())?;
        let found=docker(&["exec",&second,"sh","-c","cat /workspace/proof /home/agent/proof /var/lib/orch/proof; test ! -e /tmp/proof"]).await?;
        if found!="kept-workkept-homekept-state" {return Err(format!("persistent content changed: {found}"));}
        driver.remove(&before).await.map_err(|e|e.to_string())?;
        let after=docker(&["exec",&second,"cat","/workspace/proof"]).await?;
        if after!="kept-work" {return Err("removing the old unit removed the successor's workspace".to_owned());}
        driver.remove(&next).await.map_err(|e|e.to_string())?;
        Ok::<(),String>(())
    }.await;
    // Cleanup always names only the objects recorded above, even when an operation failed.
    for name in [
        &first,
        &second,
        &format!("{first}-prep"),
        &format!("{second}-prep"),
    ] {
        if docker(&["container", "inspect", name]).await.is_ok() {
            docker(&["container", "rm", "-f", name]).await.unwrap();
        }
    }
    for name in [
        &volumes[0],
        &volumes[1],
        &volumes[2],
        &format!("{first}-run"),
        &format!("{second}-run"),
    ] {
        if docker(&["volume", "inspect", name]).await.is_ok() {
            docker(&["volume", "rm", name]).await.unwrap();
        }
    }
    // Each unit stands on a network of its own, removed here with the two common bridges' names.
    for name in [
        &env_network,
        &ctl_network,
        &format!("{first}-net"),
        &format!("{second}-net"),
    ] {
        if docker(&["network", "inspect", name]).await.is_ok() {
            docker(&["network", "rm", name]).await.unwrap();
        }
    }
    let mut resources: Json = serde_json::from_slice(&std::fs::read(&log).unwrap()).unwrap();
    resources["cleaned"] = json!(true);
    journal(&resources);
    std::fs::write(&log, serde_json::to_vec_pretty(&resources).unwrap()).unwrap();
    assert!(outcome.is_ok(), "{outcome:?}");
}
