//! Real Docker whole-volume carry preserves home metadata and private overlay deletions without copying the backing tree.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{BUSYBOX, Lab};
use orch_driver_contract::driver::EnvironmentDriver;
use orch_driver_contract::runtime::VolumeRole;
use orch_driver_contract::volumes::VolumeRequest;
use orch_types::Json;
use serde_json::json;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_directory_and_a_volume_with_the_same_content_match_and_bytes_names_and_modes_differ() {
    use orch_driver_contract::runtime::DirectoryBinding;
    use orch_driver_contract::volumes::OwnedVolumeSelector;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let native = std::env::var_os("ORCH_TEST_NODE_BINARY");
    let binary = std::env::var_os("ORCH_TEST_STATIC_NODE");
    let helper_image = if binary.is_some() {
        BUSYBOX.to_owned()
    } else {
        std::env::var("ORCH_TEST_VOLUME_HELPER_IMAGE").unwrap_or_else(|_| "orch-dev-harness".into())
    };
    let Some(binary) = binary.or(native) else {
        eprintln!("not run: the current static or native node helper must be supplied");
        return;
    };
    let Some(lab) = Lab::with(|config, _| {
        config.live_helper_image = Some(helper_image.clone());
        config.live_helper_binary = Some(binary.into());
    })
    .await
    else {
        return;
    };
    lab.have(&helper_image).await;
    let root = lab.path("fingerprint-data");
    std::fs::write(root.join("data"), b"same").unwrap();
    let meta = std::fs::metadata(&root).unwrap();
    let binding = DirectoryBinding {
        path: root.to_string_lossy().into_owned(),
        device: Some(meta.dev()),
        inode: Some(meta.ino()),
    };
    let volume = VolumeRequest {
        name: lab.name("fingerprint-volume"),
        storage: None,
        labels: BTreeMap::new(),
    };
    lab.track_volume(&volume.name);
    lab.driver.ensure_volume(&volume).await.unwrap();
    let archive = root.parent().unwrap().join("fingerprint.archive");
    let before = lab.driver.fingerprint_directory(&binding).await.unwrap();
    lab.driver
        .export_directory(&binding, OwnedVolumeSelector::Home, &archive)
        .await
        .unwrap();
    lab.driver.import_volume(&volume, &archive).await.unwrap();
    assert_eq!(
        before,
        lab.driver.fingerprint_volume(&volume).await.unwrap()
    );
    std::fs::write(root.join("data"), b"sAme").unwrap();
    assert_ne!(
        before,
        lab.driver.fingerprint_directory(&binding).await.unwrap()
    );
    std::fs::write(root.join("data"), b"same").unwrap();
    std::fs::rename(root.join("data"), root.join("renamed")).unwrap();
    assert_ne!(
        before,
        lab.driver.fingerprint_directory(&binding).await.unwrap()
    );
    std::fs::rename(root.join("renamed"), root.join("data")).unwrap();
    std::fs::set_permissions(root.join("data"), std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_ne!(
        before,
        lab.driver.fingerprint_directory(&binding).await.unwrap()
    );
    assert_eq!(
        before,
        lab.driver.fingerprint_volume(&volume).await.unwrap(),
        "checking and changing the directory never writes to the volume"
    );
}

fn entry(
    out: &mut Vec<u8>,
    path: &str,
    kind: &str,
    mode: u32,
    data: &[u8],
    xattrs: Vec<(Vec<u8>, Vec<u8>)>,
) {
    let header = json!({"path":path.as_bytes(),"kind":kind,"size":data.len(),"uid":1000,"gid":1000,"mode":mode,"atime":1,"atime_ns":0,"mtime":2,"mtime_ns":0,"link":null,"major":0,"minor":0,"xattrs":xattrs});
    let encoded = serde_json::to_vec(&header).unwrap();
    out.extend_from_slice(&u32::try_from(encoded.len()).unwrap().to_le_bytes());
    out.extend_from_slice(&encoded);
    out.extend_from_slice(data);
}

fn headers(path: &Path) -> BTreeMap<Vec<u8>, Json> {
    let mut file = std::fs::File::open(path).unwrap();
    let mut magic = [0u8; 14];
    file.read_exact(&mut magic).unwrap();
    assert_eq!(&magic, b"ORCH-VOLUME-1\n");
    let mut entries = BTreeMap::new();
    loop {
        let mut length = [0u8; 4];
        file.read_exact(&mut length).unwrap();
        let length = usize::try_from(u32::from_le_bytes(length)).unwrap();
        if length == 0 {
            break;
        }
        let mut encoded = vec![0u8; length];
        file.read_exact(&mut encoded).unwrap();
        let header: Json = serde_json::from_slice(&encoded).unwrap();
        let path: Vec<u8> = serde_json::from_value(header["path"].clone()).unwrap();
        std::io::copy(
            &mut (&mut file).take(header["size"].as_u64().unwrap()),
            &mut std::io::sink(),
        )
        .unwrap();
        entries.insert(path, header);
    }
    entries
}

#[tokio::test]
async fn the_whole_home_and_private_layer_keep_owners_modes_xattrs_whiteouts_and_opaque_directories()
 {
    let Some(binary) = std::env::var_os("ORCH_TEST_STATIC_NODE") else {
        eprintln!("not run: ORCH_TEST_STATIC_NODE must name the released static node helper");
        return;
    };
    let Some(lab) = Lab::with(|config, _| {
        config.live_helper_image = Some(BUSYBOX.into());
        config.live_helper_binary = Some(binary.into());
    })
    .await
    else {
        return;
    };
    if !lab.driver.facts().private_views() {
        eprintln!("not run: this daemon has no Unix private layers");
        return;
    }
    lab.have(BUSYBOX).await;
    lab.driver.check_volume_carry().await.unwrap();
    let lower = lab.path("backing-data");
    std::fs::create_dir(lower.join("opaque")).unwrap();
    std::fs::write(lower.join("removed.txt"), b"must stay deleted").unwrap();
    std::fs::write(lower.join("opaque/lower.txt"), b"must stay hidden").unwrap();
    std::fs::write(lower.join("change.txt"), b"underneath").unwrap();
    let source_layer = lab.name("source-private");
    let source = lab.request("environment", "source-whole", |request| {
        request["mounts"] = json!([{"name":"data","kind":"path","source":lower,"target":"/data/kept","mode":"private","layer":{"name":source_layer,"storage":null}}]);
        request["env"] = json!({"LAB_RUN":"rm /data/kept/removed.txt && rm -rf /data/kept/opaque && mkdir /data/kept/opaque && printf changed > /data/kept/change.txt"});
    });
    let source_home = VolumeRequest {
        name: source.volumes[&VolumeRole::Home].name.clone(),
        storage: None,
        labels: BTreeMap::from([("orch.carried".into(), "true".into())]),
    };
    lab.track_volume(&source_home.name);
    lab.driver.ensure_volume(&source_home).await.unwrap();
    let directory = tempfile::tempdir().unwrap();
    let seed = directory.path().join("seed.archive");
    let mut bytes = b"ORCH-VOLUME-1\n".to_vec();
    entry(&mut bytes, "", "directory", 0o750, &[], Vec::new());
    entry(&mut bytes, ".auth", "directory", 0o750, &[], Vec::new());
    entry(
        &mut bytes,
        ".auth/value",
        "file",
        0o640,
        b"signed",
        vec![(
            b"user.orch-test".to_vec(),
            b"binary\0attribute\xff".to_vec(),
        )],
    );
    bytes.extend_from_slice(&0u32.to_le_bytes());
    std::fs::write(&seed, bytes).unwrap();
    lab.driver.import_volume(&source_home, &seed).await.unwrap();
    let source_unit = lab.run(&source).await;
    assert!(
        lab.read(&source_unit, "/data/kept/removed.txt")
            .await
            .is_none()
    );
    assert!(
        lab.read(&source_unit, "/data/kept/opaque/lower.txt")
            .await
            .is_none()
    );
    let pid = lab.inspect(&source_unit).await.state.unwrap().pid;
    let home_archive = directory.path().join("home.archive");
    let private_archive = directory.path().join("private.archive");
    let source_private = VolumeRequest {
        name: source_layer.clone(),
        storage: None,
        labels: BTreeMap::new(),
    };
    let measured = lab.driver.measure_volume(&source_home).await.unwrap();
    assert_eq!((measured.file_count, measured.total_bytes), (1, 6));
    // This is the node's quiescent export lifecycle: the process is resumed, with the same identity.
    lab.driver.suspend(&source_unit).await.unwrap();
    lab.driver
        .export_volume(&source_home, &home_archive)
        .await
        .unwrap();
    lab.driver
        .export_volume(&source_private, &private_archive)
        .await
        .unwrap();
    lab.driver.resume(&source_unit).await.unwrap();
    assert_eq!(lab.inspect(&source_unit).await.state.unwrap().pid, pid);
    let target_layer = lab.name("target-private");
    let target = lab.request("environment", "target-whole", |request| {
        request["mounts"] = json!([{"name":"data","kind":"path","source":lower,"target":"/data/kept","mode":"private","layer":{"name":target_layer,"storage":null}}]);
        request["env"] = json!({"LAB_RUN":"cat $ORCH_HOME_DIR/.auth/value > $ORCH_DATA_DIR/carried-home; stat -c '%u:%g %a' $ORCH_HOME_DIR/.auth/value > $ORCH_DATA_DIR/home-mode; stat -c '%u:%g %a' $ORCH_HOME_DIR > $ORCH_DATA_DIR/home-root-mode; cat /data/kept/change.txt > $ORCH_DATA_DIR/private-change; test ! -e /data/kept/removed.txt && test ! -e /data/kept/opaque/lower.txt && echo deletions-kept > $ORCH_DATA_DIR/deletions"});
    });
    let target_home = VolumeRequest {
        name: target.volumes[&VolumeRole::Home].name.clone(),
        storage: None,
        labels: BTreeMap::from([("orch.carried".into(), "true".into())]),
    };
    let target_private = VolumeRequest {
        name: target_layer,
        storage: None,
        labels: BTreeMap::from([("orch.carried".into(), "true".into())]),
    };
    for volume in [&target_home, &target_private] {
        lab.track_volume(&volume.name);
        lab.driver.ensure_volume(volume).await.unwrap();
    }
    lab.driver
        .import_volume(&target_home, &home_archive)
        .await
        .unwrap();
    lab.driver
        .import_volume(&target_private, &private_archive)
        .await
        .unwrap();
    // Both imports precede create, so no mounted overlay has its upper changed underneath it.
    let target_unit = lab.run(&target).await;
    assert_eq!(
        lab.read(&target_unit, "/var/lib/orch/carried-home")
            .await
            .as_deref(),
        Some("signed")
    );
    assert_eq!(
        lab.read(&target_unit, "/var/lib/orch/home-mode")
            .await
            .as_deref(),
        Some("1000:1000 640\n")
    );
    assert_eq!(
        lab.read(&target_unit, "/var/lib/orch/home-root-mode")
            .await
            .as_deref(),
        Some("1000:1000 750\n")
    );
    assert_eq!(
        lab.read(&target_unit, "/var/lib/orch/private-change")
            .await
            .as_deref(),
        Some("changed")
    );
    assert_eq!(
        lab.read(&target_unit, "/var/lib/orch/deletions")
            .await
            .as_deref(),
        Some("deletions-kept\n")
    );
    assert_eq!(
        std::fs::read(lower.join("removed.txt")).unwrap(),
        b"must stay deleted"
    );
    assert_eq!(
        std::fs::read(lower.join("opaque/lower.txt")).unwrap(),
        b"must stay hidden"
    );
    assert_eq!(
        std::fs::read(lower.join("change.txt")).unwrap(),
        b"underneath"
    );
    assert!(
        lab.driver
            .import_volume(&target_home, &home_archive)
            .await
            .is_err(),
        "a used destination cannot be overwritten"
    );
    let copied = directory.path().join("copied-home.archive");
    lab.driver.suspend(&target_unit).await.unwrap();
    lab.driver
        .export_volume(&target_home, &copied)
        .await
        .unwrap();
    lab.driver.resume(&target_unit).await.unwrap();
    let copied = headers(&copied);
    assert_eq!(
        copied[b".auth/value".as_slice()]["xattrs"],
        json!([[
            b"user.orch-test".as_slice(),
            b"binary\0attribute\xff".as_slice()
        ]])
    );
    let private = headers(&private_archive);
    let whiteout = &private[b"upper/removed.txt".as_slice()];
    assert_eq!(
        (
            whiteout["kind"].as_str(),
            whiteout["major"].as_u64(),
            whiteout["minor"].as_u64()
        ),
        (Some("character"), Some(0), Some(0))
    );
    let opaque: Vec<(Vec<u8>, Vec<u8>)> =
        serde_json::from_value(private[b"upper/opaque".as_slice()]["xattrs"].clone()).unwrap();
    assert!(
        opaque
            .iter()
            .any(|(name, value)| name == b"trusted.overlay.opaque" && value == b"y")
    );
}

#[tokio::test]
async fn the_owned_file_state_selector_keeps_original_records_while_other_home_files_and_temporary_tokens_stay_behind()
 {
    let Some(binary) = std::env::var_os("ORCH_TEST_STATIC_NODE") else {
        eprintln!("not run: ORCH_TEST_STATIC_NODE names the released static node helper");
        return;
    };
    let Some(lab) = Lab::with(|config, _| {
        config.live_helper_image = Some(BUSYBOX.into());
        config.live_helper_binary = Some(binary.into());
    })
    .await
    else {
        return;
    };
    lab.have(BUSYBOX).await;
    lab.driver.check_volume_carry().await.unwrap();
    let source = lab.request("environment", "source-file-states", |_| {});
    let target = lab.request("environment", "target-file-states", |_| {});
    let source_home = VolumeRequest {
        name: source.volumes[&VolumeRole::Home].name.clone(),
        storage: None,
        labels: BTreeMap::from([("orch.carried".into(), "true".into())]),
    };
    let target_home = VolumeRequest {
        name: target.volumes[&VolumeRole::Home].name.clone(),
        storage: None,
        labels: BTreeMap::from([("orch.carried".into(), "true".into())]),
    };
    for volume in [&source_home, &target_home] {
        lab.track_volume(&volume.name);
        lab.driver.ensure_volume(volume).await.unwrap();
    }
    let directory = tempfile::tempdir().unwrap();
    let seed = directory.path().join("source-home.archive");
    let mut bytes = b"ORCH-VOLUME-1\n".to_vec();
    for path in [
        "",
        ".orch",
        ".orch/file-states",
        ".orch/file-states/roots",
        ".orch/file-states/git",
        ".orch/file-states/git/objects",
        ".orch/file-states/git/objects/aa",
        ".orch/file-states/journals",
        ".orch/file-states/source-credential-interrupted",
    ] {
        entry(&mut bytes, path, "directory", 0o700, &[], Vec::new());
    }
    entry(
        &mut bytes,
        "other-home-file",
        "file",
        0o600,
        b"left behind",
        Vec::new(),
    );
    entry(
        &mut bytes,
        ".orch/file-states/roots/original.json",
        "file",
        0o600,
        b"original root and expiry",
        Vec::new(),
    );
    entry(
        &mut bytes,
        ".orch/file-states/git/objects/aa/recorded",
        "file",
        0o600,
        b"owned immutable bytes",
        Vec::new(),
    );
    entry(
        &mut bytes,
        ".orch/file-states/journals/original.json",
        "file",
        0o600,
        b"original journal",
        Vec::new(),
    );
    entry(
        &mut bytes,
        ".orch/file-states/source-credential-interrupted/password",
        "file",
        0o600,
        b"SCOPED_COMMAND_TOKEN",
        Vec::new(),
    );
    bytes.extend_from_slice(&0u32.to_le_bytes());
    std::fs::write(&seed, bytes).unwrap();
    lab.driver.import_volume(&source_home, &seed).await.unwrap();
    let measured = lab
        .driver
        .measure_file_states(&source_home)
        .await
        .unwrap()
        .file_states
        .unwrap();
    assert!(measured.complete);
    assert_eq!(
        (measured.roots, measured.objects, measured.journals),
        (1, 1, 1)
    );
    let archive = directory.path().join("file-states.archive");
    lab.driver
        .export_file_states(&source_home, &archive)
        .await
        .unwrap();
    let mut input = tokio::fs::File::open(&archive).await.unwrap();
    lab.driver
        .import_file_states_stream(&target_home, &mut input)
        .await
        .unwrap();
    let target_export = directory.path().join("target-home.archive");
    lab.driver
        .export_volume(&target_home, &target_export)
        .await
        .unwrap();
    let entries = headers(&target_export);
    assert!(entries.contains_key(b".orch/file-states/roots/original.json".as_slice()));
    assert!(entries.contains_key(b".orch/file-states/git/objects/aa/recorded".as_slice()));
    assert!(entries.contains_key(b".orch/file-states/journals/original.json".as_slice()));
    assert!(!entries.contains_key(b"other-home-file".as_slice()));
    assert!(
        !entries
            .contains_key(b".orch/file-states/source-credential-interrupted/password".as_slice())
    );
}
