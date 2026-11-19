//! Mounts on a real Docker daemon: read-only enforced by the mount itself, shared written through, a private view
//! that keeps the environment's changes apart from the data and from every other environment.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::time::Duration;

use common::Lab;
use orch_driver_contract::driver::EnvironmentDriver;
use orch_driver_contract::volumes::VolumeRequest;
use serde_json::{Value, json};

/// The request of an environment that runs as root, so that only the mounts decide what may be written.
fn as_root(request: &mut Value) {
    request["spec"]["user"] = json!({ "uid": 0, "gid": 0 });
}

#[tokio::test]
async fn a_read_only_mount_cannot_be_written_and_a_shared_one_is_written_through() {
    let Some(lab) = Lab::start().await else {
        return;
    };
    let read_only = lab.path("ro");
    let shared = lab.path("rw");
    std::fs::write(read_only.join("a.txt"), "original\n").unwrap();
    let volume = VolumeRequest {
        name: lab.name("mount-volume"),
        storage: None,
        labels: [("orch.system".to_owned(), lab.system.clone())].into(),
    };
    lab.track_volume(&volume.name);
    lab.driver.ensure_volume(&volume).await.unwrap();
    let spec = lab.request("environment", "mounts", |request| {
        as_root(request);
        request["mounts"] = json!([
            { "name": "data", "kind": "path", "source": read_only, "target": "/data/ro", "mode": "read_only", "layer": null },
            { "name": "out", "kind": "path", "source": shared, "target": "/data/rw", "mode": "shared", "layer": null },
            { "name": "cache", "kind": "volume", "source": volume.name, "target": "/data/cache", "mode": "shared", "layer": null }
        ]);
        request["env"] = json!({ "LAB_RUN": "echo changed > /data/ro/a.txt; echo \"rc=$?\"; echo new > /data/ro/new.txt; echo \"rc=$?\"; echo written > /data/rw/new.txt; echo kept > /data/cache/kept.txt; cat /data/ro/a.txt" });
    });
    let unit = lab.run(&spec).await;
    let run = lab.read(&unit, "/var/lib/orch/run.txt").await.unwrap();
    assert!(run.contains("Read-only file system"), "{run}");
    assert!(run.contains("rc=1") && !run.contains("rc=0"), "{run}");
    assert_eq!(
        std::fs::read_to_string(read_only.join("a.txt")).unwrap(),
        "original\n"
    );
    assert!(
        !read_only.join("new.txt").exists(),
        "nothing was made in the data"
    );
    assert_eq!(
        std::fs::read_to_string(shared.join("new.txt")).unwrap(),
        "written\n"
    );
    let found = lab.launch(&unit).await;
    let attached: Value = serde_json::from_str(&found["mounts_file"]).unwrap();
    assert_eq!(
        attached,
        json!([
            { "name": "data", "path": "/data/ro", "mode": "read_only" },
            { "name": "out", "path": "/data/rw", "mode": "shared" },
            { "name": "cache", "path": "/data/cache", "mode": "shared" }
        ])
    );
    // The volume keeps what was written to it for the next unit that attaches it.
    lab.driver
        .stop(&unit, Duration::from_secs(3))
        .await
        .unwrap();
    let again = lab.request("environment", "mounts-again", {
        let mounts = spec.mounts.clone();
        move |request| {
            as_root(request);
            request["mounts"] = serde_json::to_value(&mounts).unwrap();
            request["env"] =
                json!({ "LAB_RUN": "cat /data/cache/kept.txt > $ORCH_DATA_DIR/seen.txt" });
        }
    });
    let second = lab.run(&again).await;
    assert_eq!(
        lab.read(&second, "/var/lib/orch/seen.txt").await.as_deref(),
        Some("kept\n")
    );
}

#[tokio::test]
async fn a_mount_whose_target_collides_or_whose_directory_is_missing_is_left_out_and_not_listed() {
    let Some(lab) = Lab::start().await else {
        return;
    };
    let present = lab.path("present");
    let spec = lab.request("environment", "left-out", |request| {
        as_root(request);
        request["mounts"] = json!([
            { "name": "inside", "kind": "path", "source": present, "target": "/workspace/inside", "mode": "read_only", "layer": null },
            { "name": "missing", "kind": "path", "source": "/nowhere/to/be/found", "target": "/data/missing", "mode": "read_only", "layer": null },
            { "name": "fine", "kind": "path", "source": present, "target": "/data/fine", "mode": "read_only", "layer": null },
            { "name": "again", "kind": "path", "source": present, "target": "/data/fine", "mode": "shared", "layer": null }
        ]);
    });
    let unit = lab.run(&spec).await;
    let attached: Value = serde_json::from_str(&lab.launch(&unit).await["mounts_file"]).unwrap();
    assert_eq!(
        attached,
        json!([ { "name": "fine", "path": "/data/fine", "mode": "read_only" } ])
    );
}

#[tokio::test]
async fn a_private_view_keeps_the_changes_of_one_environment_apart_from_the_data_and_from_every_other()
 {
    let Some(lab) = Lab::start().await else {
        return;
    };
    if !lab.driver.describe().private_views {
        eprintln!("skipped: this host cannot make a private view");
        return;
    }
    let data = lab.path("tree");
    std::fs::write(data.join("a.txt"), "original\n").unwrap();
    std::fs::write(data.join("gone.txt"), "to be deleted in one view\n").unwrap();
    std::fs::create_dir_all(data.join("sub")).unwrap();
    std::fs::write(data.join("sub/b.txt"), "deep\n").unwrap();
    let view = |label: &str, run: &'static str| {
        let layer = lab.name(&format!("over-{label}"));
        let data = data.clone();
        lab.request("environment", label, move |request| {
            as_root(request);
            request["mounts"] = json!([
                { "name": "tree", "kind": "path", "source": data, "target": "/data/tree", "mode": "private",
                  "layer": { "name": layer, "storage": null } }
            ]);
            request["env"] = json!({ "LAB_RUN": run });
        })
    };
    let writer = view(
        "writer",
        "echo changed > /data/tree/a.txt; echo new > /data/tree/only-here.txt; rm /data/tree/gone.txt; echo more >> /data/tree/sub/b.txt; ls /data/tree | tr '\\n' ' ' > $ORCH_DATA_DIR/ls.txt",
    );
    let reader = view(
        "reader",
        "cat /data/tree/a.txt > $ORCH_DATA_DIR/a.txt; ls /data/tree | tr '\\n' ' ' > $ORCH_DATA_DIR/ls.txt",
    );
    let writer_unit = lab.run(&writer).await;
    let attached: Value =
        serde_json::from_str(&lab.launch(&writer_unit).await["mounts_file"]).unwrap();
    assert_eq!(
        attached,
        json!([ { "name": "tree", "path": "/data/tree", "mode": "private" } ])
    );
    assert_eq!(
        lab.read(&writer_unit, "/var/lib/orch/ls.txt")
            .await
            .as_deref(),
        Some("a.txt only-here.txt sub ")
    );
    // The data underneath are as they were.
    assert_eq!(
        std::fs::read_to_string(data.join("a.txt")).unwrap(),
        "original\n"
    );
    assert!(data.join("gone.txt").exists());
    assert!(!data.join("only-here.txt").exists());
    assert_eq!(
        std::fs::read_to_string(data.join("sub/b.txt")).unwrap(),
        "deep\n"
    );
    // Another environment, with a layer of its own, sees none of that.
    let reader_unit = lab.run(&reader).await;
    assert_eq!(
        lab.read(&reader_unit, "/var/lib/orch/a.txt")
            .await
            .as_deref(),
        Some("original\n")
    );
    assert_eq!(
        lab.read(&reader_unit, "/var/lib/orch/ls.txt")
            .await
            .as_deref(),
        Some("a.txt gone.txt sub ")
    );
    // Stopped, started and made again on the same layer, the environment finds its own changes.
    lab.driver
        .stop(&writer_unit, Duration::from_secs(3))
        .await
        .unwrap();
    lab.driver.start(&writer_unit).await.unwrap();
    lab.driver
        .stop(&writer_unit, Duration::from_secs(3))
        .await
        .unwrap();
    let layer = writer.mounts[0].layer.clone().unwrap().name;
    let successor = lab.request("environment", "writer-again", {
        let volumes = writer.volumes.clone();
        let data = data.clone();
        move |request| {
            as_root(request);
            request["volumes"] = serde_json::to_value(&volumes).unwrap();
            request["mounts"] = json!([
                { "name": "tree", "kind": "path", "source": data, "target": "/data/tree", "mode": "private", "layer": { "name": layer, "storage": null } }
            ]);
            request["env"] = json!({ "LAB_RUN": "cat /data/tree/a.txt > $ORCH_DATA_DIR/a.txt; cat /data/tree/only-here.txt >> $ORCH_DATA_DIR/a.txt; ls /data/tree | tr '\\n' ' ' > $ORCH_DATA_DIR/ls.txt" });
        }
    });
    lab.driver.remove(&writer_unit).await.unwrap();
    let again = lab.run(&successor).await;
    assert_eq!(
        lab.read(&again, "/var/lib/orch/a.txt").await.as_deref(),
        Some("changed\nnew\n")
    );
    assert_eq!(
        lab.read(&again, "/var/lib/orch/ls.txt").await.as_deref(),
        Some("a.txt only-here.txt sub ")
    );
    assert_eq!(
        std::fs::read_to_string(data.join("a.txt")).unwrap(),
        "original\n"
    );
}

#[tokio::test]
async fn a_volume_location_can_be_a_private_view_too() {
    let Some(lab) = Lab::start().await else {
        return;
    };
    if !lab.driver.describe().private_views {
        eprintln!("skipped: this host cannot make a private view");
        return;
    }
    let volume = VolumeRequest {
        name: lab.name("view-source"),
        storage: None,
        labels: [("orch.system".to_owned(), lab.system.clone())].into(),
    };
    lab.track_volume(&volume.name);
    lab.driver.ensure_volume(&volume).await.unwrap();
    let writer = lab.request("environment", "volume-writer", |request| {
        as_root(request);
        request["mounts"] = json!([
            { "name": "shared", "kind": "volume", "source": volume.name, "target": "/data/shared", "mode": "shared", "layer": null }
        ]);
        request["env"] = json!({ "LAB_RUN": "echo base > /data/shared/base.txt" });
    });
    let first = lab.run(&writer).await;
    lab.driver
        .stop(&first, Duration::from_secs(3))
        .await
        .unwrap();
    let layer = lab.name("over-volume");
    let viewer = lab.request("environment", "volume-viewer", |request| {
        as_root(request);
        request["mounts"] = json!([
            { "name": "shared", "kind": "volume", "source": volume.name, "target": "/data/shared", "mode": "private",
              "layer": { "name": layer, "storage": null } }
        ]);
        request["env"] = json!({ "LAB_RUN": "cat /data/shared/base.txt > $ORCH_DATA_DIR/seen.txt; echo mine > /data/shared/mine.txt; ls /data/shared | tr '\\n' ' ' >> $ORCH_DATA_DIR/seen.txt" });
    });
    let second = lab.run(&viewer).await;
    assert_eq!(
        lab.read(&second, "/var/lib/orch/seen.txt").await.as_deref(),
        Some("base\nbase.txt mine.txt ")
    );
    // The volume itself did not get the file the view wrote.
    let check = lab.request("environment", "volume-check", |request| {
        as_root(request);
        request["mounts"] = json!([
            { "name": "shared", "kind": "volume", "source": volume.name, "target": "/data/shared", "mode": "read_only", "layer": null }
        ]);
        request["env"] = json!({ "LAB_RUN": "ls /data/shared | tr '\\n' ' ' > $ORCH_DATA_DIR/seen.txt" });
    });
    let third = lab.run(&check).await;
    assert_eq!(
        lab.read(&third, "/var/lib/orch/seen.txt").await.as_deref(),
        Some("base.txt ")
    );
}
