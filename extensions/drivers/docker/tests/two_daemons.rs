//! Two Docker daemons keep two volumes of one name, and a copy of a mount's data carries it from one to the other.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{BUSYBOX, Lab};
use orch_driver_contract::driver::EnvironmentDriver;
use orch_driver_contract::volumes::VolumeRequest;
use orch_driver_docker::config::PathMap;
use serde_json::json;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Appends one entry of the helper's whole-volume archive.
fn entry(out: &mut Vec<u8>, path: &str, kind: &str, mode: u32, data: &[u8]) {
    let header = json!({"path":path.as_bytes(),"kind":kind,"size":data.len(),"uid":1000,"gid":1000,"mode":mode,"atime":1,"atime_ns":0,"mtime":2,"mtime_ns":0,"link":null,"major":0,"minor":0,"xattrs":[]});
    let encoded = serde_json::to_vec(&header).unwrap();
    out.extend_from_slice(&u32::try_from(encoded.len()).unwrap().to_le_bytes());
    out.extend_from_slice(&encoded);
    out.extend_from_slice(data);
}

/// The regular files of an archive, by path, with their bytes.
fn files(path: &Path) -> BTreeMap<Vec<u8>, Vec<u8>> {
    let mut file = std::fs::File::open(path).unwrap();
    let mut magic = [0u8; 14];
    file.read_exact(&mut magic).unwrap();
    assert_eq!(&magic, b"ORCH-VOLUME-1\n");
    let mut found = BTreeMap::new();
    loop {
        let mut length = [0u8; 4];
        file.read_exact(&mut length).unwrap();
        let length = usize::try_from(u32::from_le_bytes(length)).unwrap();
        if length == 0 {
            break;
        }
        let mut encoded = vec![0u8; length];
        file.read_exact(&mut encoded).unwrap();
        let header: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        let size = usize::try_from(header["size"].as_u64().unwrap()).unwrap();
        let mut data = vec![0u8; size];
        file.read_exact(&mut data).unwrap();
        if header["kind"] == "file" {
            let name: Vec<u8> = serde_json::from_value(header["path"].clone()).unwrap();
            found.insert(name, data);
        }
    }
    found
}

#[tokio::test]
#[ignore = "needs a second Docker daemon (orchprobe-dind2) and the released static node helper"]
async fn one_volume_name_on_two_daemons_is_two_volumes_and_a_copy_carries_the_data_across() {
    let (Some(binary), Some(socket), Some(helper_dir)) = (
        std::env::var_os("ORCH_TEST_STATIC_NODE"),
        std::env::var("ORCH_TEST_SECOND_DOCKER_SOCKET").ok(),
        std::env::var_os("ORCH_TEST_SECOND_HELPER_DIR"),
    ) else {
        eprintln!(
            "not run: ORCH_TEST_STATIC_NODE, ORCH_TEST_SECOND_DOCKER_SOCKET and ORCH_TEST_SECOND_HELPER_DIR name the helper and the second daemon"
        );
        return;
    };
    let binary = PathBuf::from(binary);
    let first = Lab::with(|config, _| {
        config.live_helper_image = Some(BUSYBOX.into());
        config.live_helper_binary = Some(binary.clone());
    })
    .await
    .expect("the first daemon answers");
    // The second daemon sees the helper where it was copied into it, not where this machine keeps it.
    let second = Lab::with(|config, _| {
        config.socket = socket;
        config.live_helper_image = Some(BUSYBOX.into());
        config.live_helper_binary = Some(binary.clone());
        config.path_map = vec![PathMap {
            node: binary.parent().unwrap().to_path_buf(),
            host: PathBuf::from(&helper_dir),
        }];
    })
    .await
    .expect("the second daemon answers");
    first.driver.check_volume_carry().await.unwrap();
    second.driver.check_volume_carry().await.unwrap();
    assert_ne!(
        first.docker.info().await.unwrap().id,
        second.docker.info().await.unwrap().id,
        "two daemons, not one daemon reached twice"
    );

    let mount = orch_types::new_id();
    let name = format!("orch-mount-{mount}");
    let volume = VolumeRequest {
        name: name.clone(),
        storage: None,
        labels: BTreeMap::from([("orch.mount_id".to_owned(), mount.to_string())]),
    };
    first.track_volume(&name);
    second.track_volume(&name);
    first.driver.ensure_volume(&volume).await.unwrap();
    second.driver.ensure_volume(&volume).await.unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let seed = scratch.path().join("seed.archive");
    let mut bytes = b"ORCH-VOLUME-1\n".to_vec();
    entry(&mut bytes, "", "directory", 0o755, &[]);
    entry(&mut bytes, "sets", "directory", 0o755, &[]);
    entry(&mut bytes, "sets/train.csv", "file", 0o644, b"x,y\n1,2\n");
    bytes.extend_from_slice(&0u32.to_le_bytes());
    std::fs::write(&seed, bytes).unwrap();
    first.driver.import_volume(&volume, &seed).await.unwrap();
    let on_first = first.driver.measure_volume(&volume).await.unwrap();
    let on_second = second.driver.measure_volume(&volume).await.unwrap();
    assert_eq!((on_first.file_count, on_first.total_bytes), (1, 8));
    assert_eq!(
        (on_second.file_count, on_second.total_bytes),
        (0, 0),
        "the same name on another daemon is another, empty volume"
    );

    // The copy a move makes: archived where the data lies, unpacked into the location on the other daemon.
    let carried = scratch.path().join("carried.archive");
    first.driver.export_volume(&volume, &carried).await.unwrap();
    second
        .driver
        .import_volume(&volume, &carried)
        .await
        .unwrap();
    let landed = scratch.path().join("landed.archive");
    second.driver.export_volume(&volume, &landed).await.unwrap();
    assert_eq!(
        files(&landed),
        BTreeMap::from([(b"sets/train.csv".to_vec(), b"x,y\n1,2\n".to_vec())]),
        "the copy carries the data"
    );
    // Afterwards each is its own again: a change on one daemon does not reach the other.
    let changed = scratch.path().join("changed.archive");
    let mut bytes = b"ORCH-VOLUME-1\n".to_vec();
    entry(&mut bytes, "", "directory", 0o755, &[]);
    bytes.extend_from_slice(&0u32.to_le_bytes());
    std::fs::write(&changed, bytes).unwrap();
    first.driver.import_volume(&volume, &changed).await.unwrap();
    assert_eq!(
        first
            .driver
            .measure_volume(&volume)
            .await
            .unwrap()
            .file_count,
        0
    );
    assert_eq!(
        second
            .driver
            .measure_volume(&volume)
            .await
            .unwrap()
            .file_count,
        1
    );
}
