//! Native runtime enrollment and terminals through the real API, gateway and node. Opt in on a proof daemon
//! with runc, Kata and both gVisor overlay modes; an explicitly selected proof never passes by skipping a prerequisite.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use common::{Fleet, Lab, NodeUnderTest, World};
use orch_driver_contract::isolation::Boundary;
use orch_driver_docker::config::RuntimeSettings;
use orch_link_contract::commands::{EnvAttach, IndexMode, PtyClose, PtyOpen, SerialKey};
use orch_link_contract::tunnels::Service;
use orch_node::docker;
use orch_node_core::drivers::Registrations;
use orch_types::new_id;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const CLASSES: [(&str, Boundary, bool); 4] = [
    ("runc", Boundary::SharedKernel, true),
    ("runsc", Boundary::UserSpaceKernel, false),
    ("runsc-keep", Boundary::UserSpaceKernel, true),
    ("kata", Boundary::OwnKernel, true),
];

/// The same ninety-second bound the enrolled-node tests already use, including a tunnel's first output.
const WAIT: Duration = common::PATIENCE;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn declared_runtimes_enroll_run_a_terminal_and_keep_the_same_identity_and_workspace_on_restart()
 {
    if std::env::var("ORCH_TEST_ISOLATION_PROOF").as_deref() != Ok("1") {
        eprintln!("not run: set ORCH_TEST_ISOLATION_PROOF=1 for the native enrolled-runtime proof");
        return;
    }
    // The harness otherwise offers to build this prerequisite. A proof run must use its train's supplied binary.
    let connector = std::env::var("ORCH_TEST_CONNECTOR_BINARY")
        .expect("the native proof requires a prebuilt static ORCH_TEST_CONNECTOR_BINARY");
    assert!(
        Path::new(&connector).is_file(),
        "missing connector {connector}"
    );
    let lab = Lab::start()
        .await
        .expect("the proof daemon and image must exist");
    let runtimes = lab
        .docker
        .info()
        .await
        .unwrap()
        .runtimes
        .unwrap_or_default();
    for (runtime, _, _) in CLASSES {
        assert!(runtimes.contains_key(runtime), "missing runtime {runtime}");
    }
    let mut world = World::start(&lab.bridge).await;
    let adjust = lab.adjust();
    let registrations = Registrations::new().register(
        docker::NAME,
        docker::registration(move |config| {
            adjust(config);
            for (name, boundary, keeps) in CLASSES {
                config.runtimes.insert(
                    name.to_owned(),
                    RuntimeSettings {
                        boundary,
                        keeps_software_on_stop: Some(keeps),
                        ..RuntimeSettings::default()
                    },
                );
            }
        }),
    );
    let mut node = NodeUnderTest::start_over(&world, &lab, registrations).await;
    world.wait_online(node.machine_id, true).await;
    let fleet = Fleet::new(&world, &lab, &node);
    let _program = fleet.prepare().await;

    for (runtime, boundary, _) in CLASSES {
        let unit = fleet.unit(runtime);
        fleet
            .make_with(&unit, None, |request| {
                request["spec"]["runtime"] = json!(runtime);
                // A nested VM needs the same allowance used in the native runtime proof, not a tiny fixture's limit.
                request["resources"] = if boundary == Boundary::OwnKernel {
                    json!({"cpu_millis":4000,"memory_mb":6144,"pids":512})
                } else {
                    json!({"cpu_millis":1000,"memory_mb":256,"pids":256})
                };
            })
            .await
            .expect("the declared runtime could not be made");
        let started = Instant::now();
        fleet
            .start(&unit)
            .await
            .expect("the runtime could not start");
        let machine = world.wait_enrolled(&unit.hint).await;
        world.wait_online(machine, true).await;
        let enrolled_ms = started.elapsed().as_millis();
        let inspected = lab
            .docker
            .inspect_container(&unit.name, None)
            .await
            .unwrap();
        assert_eq!(
            inspected.host_config.unwrap().runtime.as_deref(),
            Some(runtime),
            "the enrolled connector really runs under the requested runtime"
        );
        if boundary != Boundary::SharedKernel {
            let resolvers = lab.read_file(&unit.name, "/etc/resolv.conf").await.unwrap();
            assert!(resolvers.contains("nameserver "), "{runtime}: {resolvers}");
            assert!(!resolvers.contains("127.0.0.11"), "{runtime}: {resolvers}");
        }
        lab.put_file(&unit.name, "/workspace", "kept.txt", "workspace survives\n")
            .await;
        world
            .api
            .link
            .dispatcher()
            .run(
                machine,
                &SerialKey::Environment(unit.environment_id),
                &EnvAttach {
                    resources: None,
                    data_generation: None,
                    storage: None,
                    data_sources: None,
                    environment_id: unit.environment_id,
                    workspace_root: None,
                    ignore: Vec::new(),
                    index: IndexMode::Live,
                    mounts: None,
                    variables: BTreeMap::new(),
                    secrets: BTreeMap::new(),
                },
            )
            .await
            .expect("the enrolled environment could not be attached");

        let terminal = new_id();
        let opened = world
            .api
            .link
            .dispatcher()
            .run(
                machine,
                &SerialKey::None,
                &PtyOpen {
                    environment_id: unit.environment_id,
                    terminal_id: terminal,
                    cols: 100,
                    rows: 30,
                    cwd: String::new(),
                    record: false,
                },
            )
            .await
            .expect("the enrolled connector could not open a terminal");
        let service = Service::Pty {
            terminal_id: terminal,
        };
        assert_eq!(opened.service, service.to_string());
        let ticket = world.api.link.tickets().terminal(machine, service).unwrap();
        let mut tunnel =
            tokio::time::timeout(WAIT, world.api.link.terminals().open(machine, &ticket))
                .await
                .expect("the terminal tunnel exceeded its deadline")
                .expect("the terminal tunnel was refused");
        tokio::time::timeout(WAIT, async {
            tunnel
                .write_all(b"printf 'native-%s\\n' $((6*7)) > /workspace/terminal.txt; cat /workspace/terminal.txt\n")
                .await
                .unwrap();
            let mut output = Vec::new();
            let mut chunk = [0u8; 1024];
            while !String::from_utf8_lossy(&output).contains("native-42") {
                let got = tunnel.read(&mut chunk).await.unwrap();
                assert!(got > 0, "{runtime}: terminal ended before answering");
                output.extend_from_slice(&chunk[..got]);
                assert!(output.len() <= 16 * 1024, "{runtime}: terminal output is bounded");
            }
        })
        .await
        .expect("the terminal did not execute the command within its deadline");
        drop(tunnel);
        assert_eq!(
            lab.read_file(&unit.name, "/workspace/terminal.txt")
                .await
                .as_deref(),
            Some("native-42\n")
        );
        world
            .api
            .link
            .dispatcher()
            .run(
                machine,
                &SerialKey::None,
                &PtyClose {
                    terminal_id: terminal,
                },
            )
            .await
            .expect("the terminal could not close");

        fleet.stop(&unit).await.expect("the runtime could not stop");
        world.wait_online(machine, false).await;
        fleet
            .start(&unit)
            .await
            .expect("the runtime could not restart");
        world.wait_online(machine, true).await;
        assert_eq!(world.machine_of_token(&unit.hint).await, Some(machine));
        for (path, expected) in [
            ("/workspace/kept.txt", "workspace survives\n"),
            ("/workspace/terminal.txt", "native-42\n"),
        ] {
            assert_eq!(
                lab.read_file(&unit.name, path).await.as_deref(),
                Some(expected),
                "{runtime}: {path}"
            );
        }
        fleet
            .stop(&unit)
            .await
            .expect("the runtime could not stop after the proof");
        fleet
            .remove(&unit)
            .await
            .expect("the runtime could not be removed");
        for (_, volume) in &unit.volumes {
            fleet
                .remove_volume(volume)
                .await
                .expect("the owned volume could not be removed");
        }
        println!(
            "ENROLLED RUNTIME {runtime}: machine={machine} enrollment_ms={enrolled_ms}; terminal and workspace restart passed"
        );
    }
    node.stop().await;
    world.stop().await;
}
