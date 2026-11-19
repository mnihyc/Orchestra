//! A unit under a runtime with a kernel of its own, on a real daemon: the runtime is described as a class, the unit is
//! held to what the class says (its own resolver file, no capability the class does not offer), and Docker inside is
//! prepared at every start in the way the boundary needs. The tests that need a runtime other than runc run where the
//! daemon has it (`ORCH_TEST_DOCKER_SOCKET` names such a daemon: a runtime of the names below, declared to the driver as
//! the operator would) and otherwise say so and pass.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::time::Duration;

use bollard::models::ExecConfig;
use common::{Gateway, Lab, bridge_address};
use futures::StreamExt;
use orch_driver_contract::driver::{DriverError, EnvironmentDriver};
use orch_driver_contract::isolation::Boundary;
use orch_driver_contract::network::EgressMode;
use orch_driver_contract::units::UnitRef;
use orch_driver_docker::config::RuntimeSettings;
use serde_json::json;

/// The runtimes of the proof daemons and what the operator declares of each: (name, boundary, keeps software on stop).
const DECLARED: [(&str, Boundary, bool); 3] = [
    ("kata", Boundary::OwnKernel, true),
    ("runsc", Boundary::UserSpaceKernel, false),
    ("runsc-keep", Boundary::UserSpaceKernel, true),
];

/// A lab whose driver has the declarations above, and the names among them that its daemon has.
async fn lab_with_runtimes() -> Option<(Lab, Vec<(&'static str, Boundary, bool)>)> {
    let lab = Lab::with(|config, dir| {
        config.egress.dir = Some(dir.join("egress"));
        for (name, boundary, keeps) in DECLARED {
            config.runtimes.insert(
                name.to_owned(),
                RuntimeSettings {
                    boundary,
                    keeps_software_on_stop: Some(keeps),
                    ..RuntimeSettings::default()
                },
            );
        }
    })
    .await?;
    let known = lab
        .docker
        .info()
        .await
        .unwrap()
        .runtimes
        .unwrap_or_default();
    let present: Vec<_> = DECLARED
        .into_iter()
        .filter(|(name, _, _)| known.contains_key(*name))
        .collect();
    Some((lab, present))
}

/// Runs `script` as `user` in the unit; the status and what it printed.
async fn exec(lab: &Lab, unit: &UnitRef, user: &str, script: &str) -> (i64, String) {
    let created = lab
        .docker
        .create_exec(
            &unit.reference,
            ExecConfig {
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                user: Some(user.to_owned()),
                cmd: Some(vec!["/bin/sh".into(), "-c".into(), script.into()]),
                ..ExecConfig::default()
            },
        )
        .await
        .unwrap();
    let mut text = String::new();
    if let bollard::exec::StartExecResults::Attached { mut output, .. } =
        lab.docker.start_exec(&created.id, None).await.unwrap()
    {
        while let Some(chunk) = output.next().await {
            text.push_str(&String::from_utf8_lossy(&chunk.unwrap().into_bytes()));
        }
    }
    let status = lab
        .docker
        .inspect_exec(&created.id)
        .await
        .unwrap()
        .exit_code
        .unwrap_or(-1);
    (status, text)
}

/// What the `docker_inside` capability gives, as the control plane freezes it into a request.
fn docker_inside() -> serde_json::Value {
    json!({
        "allowed": ["docker_inside"], "base": ["docker_inside"], "granted": [],
        "patches": { "docker_inside": {
            "cap_add": ["ALL"],
            "security_opt": ["seccomp=unconfined", "apparmor=unconfined", "systempaths=unconfined"],
            "device_cgroup_rules": ["b 7:* rwm", "c 10:237 rwm"]
        } }
    })
}

#[tokio::test]
async fn the_daemons_default_runtime_is_a_class_that_shares_the_hosts_kernel_and_offers_no_docker_inside()
 {
    let Some(lab) = Lab::start().await else {
        return;
    };
    let descriptor = lab.driver.describe();
    let default = descriptor.default_runtime.as_deref().unwrap();
    let class = descriptor
        .runtimes
        .iter()
        .find(|class| class.is_named(default))
        .expect("the default runtime is among the classes");
    assert_eq!(class.boundary, Boundary::SharedKernel);
    assert_eq!(
        class.live_changes, descriptor.live_changes,
        "a unit under a shared kernel is changed as the driver changes units: its helper reaches it"
    );
    assert!(
        !class.capabilities.contains_key("docker_inside"),
        "under a shared kernel the only ways to Docker are the host's socket and privileged mode"
    );
    assert!(class.capabilities.contains_key("kvm"));
}

#[tokio::test]
async fn a_capability_the_runtime_does_not_offer_is_refused_before_anything_is_made() {
    let Some(lab) = Lab::start().await else {
        return;
    };
    let spec = lab.request("environment", "refused", |request| {
        request["capabilities"] = docker_inside();
    });
    lab.track(&spec);
    let refused = lab.driver.create(&spec).await.unwrap_err();
    assert!(
        matches!(&refused, DriverError::Unsupported(why) if why.contains("docker_inside") && why.contains("not offered")),
        "{refused:?}"
    );
    assert!(
        !lab.has_container(&spec.name).await,
        "nothing was made for the refused request"
    );
}

#[tokio::test]
async fn a_unit_under_a_runtime_with_a_kernel_of_its_own_has_a_resolver_file_of_its_own_and_says_what_holds_it_in()
 {
    let Some((lab, present)) = lab_with_runtimes().await else {
        return;
    };
    if present.is_empty() {
        eprintln!(
            "not run: the daemon has none of {:?}",
            DECLARED.map(|d| d.0)
        );
        return;
    }
    let descriptor = lab.driver.describe();
    for (name, boundary, keeps) in present {
        let class = descriptor
            .runtimes
            .iter()
            .find(|class| class.is_named(name))
            .unwrap_or_else(|| panic!("{name} is among the classes"));
        assert_eq!(class.boundary, boundary, "{name}");
        assert_eq!(class.keeps_software_on_stop, keeps, "{name}");
        assert_eq!(class.recreate_preserves_files, keeps, "{name}");
        assert!(
            !class.live_changes.mounts,
            "{name}: its units cannot take mounts live"
        );
        assert!(class.capabilities.contains_key("docker_inside"), "{name}");
        for key in ["kvm", "privileged", "docker_socket", "host_network", "gpu"] {
            assert!(!class.capabilities.contains_key(key), "{name} offers {key}");
            assert!(
                class.unavailable.contains_key(key),
                "{name} says why not {key}"
            );
        }
        assert!(
            class.memory_overhead_mb > 0,
            "{name} costs the host something"
        );

        let spec = lab.request("environment", &format!("class-{name}"), |request| {
            request["spec"]["runtime"] = json!(name);
            request["spec"]["network"]["dns"] = json!(["9.9.9.9"]);
        });
        let unit = lab.run(&spec).await;
        let labels = lab.inspect(&unit).await.config.unwrap().labels.unwrap();
        assert_eq!(labels["orch.boundary"], boundary.as_str(), "{name}");
        let (status, resolver) = exec(&lab, &unit, "0", "cat /etc/resolv.conf").await;
        assert_eq!(status, 0, "{name}: {resolver}");
        assert!(
            resolver.contains("nameserver 9.9.9.9"),
            "{name}: {resolver}"
        );
        assert!(
            !resolver.contains("127.0.0.11"),
            "{name}: the daemon's resolver cannot be reached from there: {resolver}"
        );
        let found = lab.launch(&unit).await;
        assert_eq!(found["uid"], "1000 gid=1000", "{name}");
        assert_eq!(found["workspace_writable"], "yes", "{name}");
        assert_eq!(found["data_writable"], "yes", "{name}");
    }
}

#[tokio::test]
async fn docker_inside_under_a_kernel_in_user_space_is_a_directory_of_the_state_volume_made_again_at_every_start()
 {
    let Some((lab, present)) = lab_with_runtimes().await else {
        return;
    };
    let present: Vec<_> = present
        .into_iter()
        .filter(|(_, boundary, _)| *boundary == Boundary::UserSpaceKernel)
        .collect();
    if present.is_empty() {
        eprintln!("not run: the daemon has no runtime with a kernel in user space");
        return;
    }
    // The kept overlay is essential: stale process files would survive there without a temporary mount.
    for (name, _, _) in present {
        let spec = lab.request(
            "environment",
            &format!("inside-directory-{name}"),
            |request| {
                request["spec"]["runtime"] = json!(name);
                request["capabilities"] = docker_inside();
                request["spec"]["security"]["no_new_privileges"] = json!(false);
            },
        );
        let unit = lab.run(&spec).await;
        let labels = lab.inspect(&unit).await.config.unwrap().labels.unwrap();
        assert_eq!(labels["orch.docker_inside_gb"], "100");
        let (status, said) = exec(
            &lab,
            &unit,
            "0",
            "mountpoint -q /var/lib/docker && mountpoint -q /var/run/docker && echo stale > /var/run/docker/old-process && echo mounted",
        )
        .await;
        assert_eq!(status, 0, "{said}");
        let (status, said) = exec(
            &lab,
            &unit,
            "0",
            "echo kept > /var/lib/docker/probe && cat /var/lib/orch/docker-inside/probe",
        )
        .await;
        assert_eq!(
            (status, said.trim()),
            (0, "kept"),
            "the data is in the state volume"
        );
        // A stop takes the mount with it; the next start makes it again and the data is still there.
        lab.driver
            .stop(&unit, Duration::from_secs(5))
            .await
            .unwrap();
        lab.driver.start(&unit).await.unwrap();
        let (status, said) = exec(
            &lab,
            &unit,
            "0",
            "mountpoint -q /var/lib/docker && mountpoint -q /var/run/docker && test ! -e /var/run/docker/old-process && cat /var/lib/docker/probe",
        )
        .await;
        assert_eq!((status, said.trim()), (0, "kept"), "{said}");
    }
}

#[tokio::test]
async fn docker_inside_in_a_virtual_machine_needs_a_file_system_tool_in_the_image_and_says_so() {
    let Some((lab, present)) = lab_with_runtimes().await else {
        return;
    };
    let Some((name, _, _)) = present
        .into_iter()
        .find(|(_, boundary, _)| *boundary == Boundary::OwnKernel)
    else {
        eprintln!("not run: the daemon has no runtime with a kernel of its own");
        return;
    };
    let spec = lab.request("environment", "inside-file", |request| {
        request["spec"]["runtime"] = json!(name);
        request["capabilities"] = docker_inside();
        request["spec"]["security"]["no_new_privileges"] = json!(false);
    });
    let unit = lab.create(&spec).await;
    let refused = lab.driver.start(&unit).await.unwrap_err();
    assert!(
        matches!(&refused, DriverError::Unavailable(why) if why.contains("docker_inside") && why.contains("mkfs.ext4")),
        "busybox has no mkfs.ext4: {refused:?}"
    );
}

#[tokio::test]
async fn a_restricted_unit_under_a_guest_runtime_reaches_its_proxy_and_nothing_else_where_that_was_proved()
 {
    let Some((lab, present)) = lab_with_runtimes().await else {
        return;
    };
    if present.is_empty() || !lab.driver.facts().restricted_egress {
        eprintln!(
            "not run: no guest runtime, or no daemon that can make a restricted unit's network"
        );
        return;
    }
    let Some(bridge) = bridge_address(&lab.docker).await else {
        eprintln!("not run: the daemon's default bridge has no address");
        return;
    };
    let descriptor = lab.driver.describe();
    for (name, _, _) in present {
        let class = descriptor
            .runtimes
            .iter()
            .find(|class| class.is_named(name))
            .unwrap();
        // Restricted egress is offered under a guest only where a unit of the class was seen to reach nothing.
        assert!(
            class.network_egress.contains(&EgressMode::Restricted),
            "{name}: the probe found it holds: {:?}",
            class.network_egress
        );
        let gateway = Gateway::listen(&bridge);
        let spec = lab.request("environment", &format!("sealed-{name}"), |request| {
            request["spec"]["runtime"] = json!(name);
            request["spec"]["network"]["mode"] = json!("none");
            request["gateway_url"] = json!(format!("wss://gateway.orch.internal:{}", gateway.port));
            request["env"] = json!({
                "GATEWAY_PORT": gateway.port.to_string(),
                "LAB_RUN": "p=${HTTPS_PROXY#http://}; ph=${p%:*}; pp=${p##*:}; echo $ph > $ORCH_DATA_DIR/proxy.txt; for i in 1 2 3 4 5 6 7 8 9 10; do printf 'CONNECT gateway.orch.internal:%s HTTP/1.1\\r\\n\\r\\n' \"$GATEWAY_PORT\" | nc -w 5 \"$ph\" \"$pp\" > $ORCH_DATA_DIR/gateway.txt 2>&1; grep -q ' 200 ' $ORCH_DATA_DIR/gateway.txt && break; sleep 1; done; nc -w 3 1.1.1.1 80 > $ORCH_DATA_DIR/outside.txt 2>&1; echo \"rc=$?\" >> $ORCH_DATA_DIR/outside.txt; cat /etc/resolv.conf > $ORCH_DATA_DIR/resolv.txt"
            });
        });
        let unit = lab.run(&spec).await;
        let written = |file: &'static str| {
            let lab = &lab;
            let unit = &unit;
            async move {
                lab.read(unit, &format!("/var/lib/orch/{file}"))
                    .await
                    .unwrap_or_default()
            }
        };
        let through = written("gateway.txt").await;
        assert!(
            through.contains("200 Connection established"),
            "{name}: {through:?}"
        );
        assert!(
            through.contains("gateway-hello from "),
            "{name}: {through:?}"
        );
        let outside = written("outside.txt").await;
        assert!(
            !outside.contains("rc=0"),
            "{name}: nothing outside answers: {outside:?}"
        );
        let proxy = written("proxy.txt").await;
        let resolver = written("resolv.txt").await;
        assert!(
            resolver.contains(&format!("nameserver {}", proxy.trim())),
            "{name}: the proxy filters the names: {resolver:?} / {proxy:?}"
        );
    }
}
