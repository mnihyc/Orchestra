//! A Container environment on a real Kubernetes cluster (k3s in Docker), made, started, stopped, started again with its volumes and its writable layer kept, and removed, through a real gateway and API and the node's own driver path.
//!
//! The test needs a disposable cluster and says what it needs when it is missing; it runs nothing and fails nothing
//! without one. It names the cluster by environment variables:
//!
//! - `ORCH_TEST_K8S_KUBECONFIG`: the private kubeconfig of the disposable cluster;
//! - `ORCH_TEST_K8S_NAMESPACE`: the namespace the node's driver owns (default `orch-k8s-proof`);
//! - `ORCH_TEST_K8S_STORAGE_CLASS`: the class for claims (default `local-path`);
//! - `ORCH_TEST_K8S_HELPER_IMAGE`: the trusted helper image, pinned to a digest, as the cluster pulls it;
//! - `ORCH_TEST_K8S_UNIT_IMAGE`: the small image the unit runs, as the cluster pulls it;
//! - `ORCH_TEST_K8S_GATEWAY_ADDRESS`: the address of this machine that pods reach, on which the gateway listens;
//! - `ORCH_TEST_K8S_KUBECTL`: a command that runs `kubectl` against the cluster, to look at what the driver made;
//! - `ORCH_TEST_CONNECTOR_BINARY`: the static connector the unit runs.
//!
//! Everything it makes in the cluster carries the test's token in its name and is looked at, never removed, by this
//! test: the namespace belongs to the disposable cluster.
#![cfg(feature = "kubernetes")]
#![cfg(target_os = "linux")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "a test fails by panicking"
)]

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use common::{ProgramServer, World, eventually_for};
use orch_api_core::certificates::TokenFor;
use orch_api_core::dispatch::RunError;
use orch_driver_contract::runtime::{RuntimeKind, unit_labels, unit_name};
use orch_link_contract::commands::{
    CommandBody, NodeRuntimeCreate, NodeRuntimeCreateResult, NodeRuntimeRemove, NodeRuntimeStart,
    NodeRuntimeStop, NodeToolEnsure, NodeVolumeEnsure, NodeVolumeRemove, SerialKey,
};
use orch_node_core::drivers::{DriverHost, Registrations};
use orch_node_core::images::BlobDesk;
use orch_node_core::settings::NodeSettings;
use orch_types::{Id, Json, new_id};
use serde_json::json;

/// A cluster pod takes longer than a local container: an image pull, a scheduler, a staged connector and an
/// enrollment through two network address translations.
const CLUSTER_PATIENCE: Duration = Duration::from_secs(300);

/// What the test needs from the disposable cluster.
#[derive(Clone)]
struct Cluster {
    kubeconfig: PathBuf,
    namespace: String,
    storage_class: String,
    helper_image: String,
    unit_image: String,
    gateway: String,
    kubectl: Vec<String>,
    connector: PathBuf,
    token: String,
}

fn variable(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

impl Cluster {
    /// The cluster, or `None` with a word on what is missing.
    fn from_environment() -> Option<Self> {
        let mut missing = Vec::new();
        let mut need = |name: &str| {
            let value = variable(name);
            if value.is_none() {
                missing.push(name.to_owned());
            }
            value
        };
        let kubeconfig = need("ORCH_TEST_K8S_KUBECONFIG");
        let helper_image = need("ORCH_TEST_K8S_HELPER_IMAGE");
        let unit_image = need("ORCH_TEST_K8S_UNIT_IMAGE");
        let gateway = need("ORCH_TEST_K8S_GATEWAY_ADDRESS");
        let kubectl = need("ORCH_TEST_K8S_KUBECTL");
        let connector = need("ORCH_TEST_CONNECTOR_BINARY");
        if !missing.is_empty() {
            eprintln!("skipped: the disposable cluster is not named: {missing:?}");
            return None;
        }
        let id = new_id().simple().to_string();
        Some(Self {
            kubeconfig: PathBuf::from(kubeconfig?),
            namespace: variable("ORCH_TEST_K8S_NAMESPACE")
                .unwrap_or_else(|| "orch-k8s-proof".to_owned()),
            storage_class: variable("ORCH_TEST_K8S_STORAGE_CLASS")
                .unwrap_or_else(|| "local-path".to_owned()),
            helper_image: helper_image?,
            unit_image: unit_image?,
            gateway: gateway?,
            kubectl: kubectl?.split_whitespace().map(str::to_owned).collect(),
            connector: PathBuf::from(connector?),
            token: id[id.len() - 12..].to_owned(),
        })
    }

    fn system(&self) -> String {
        format!("orch-test-k8s-{}", self.token)
    }

    /// The machine policy's connection object for the driver.
    fn connection(&self) -> Json {
        json!({
            "kubeconfig": self.kubeconfig,
            "namespace": self.namespace,
            "storage_class": self.storage_class,
            "volume_request": "1Gi",
            "runtime_classes": [{"runtime": "runc"}],
            "tool_stager_image": self.helper_image,
        })
    }

    /// What `kubectl` prints for these arguments, in the test's namespace; a refusal is a failure.
    fn kubectl(&self, arguments: &[&str]) -> String {
        let (program, prefix) = self.kubectl.split_first().expect("a kubectl command");
        let output = std::process::Command::new(program)
            .args(prefix)
            .args(["-n", &self.namespace])
            .args(arguments)
            .output()
            .expect("kubectl could not be run");
        assert!(
            output.status.success(),
            "kubectl {arguments:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    /// The same, where a refusal is an answer (an object that is not there).
    fn kubectl_try(&self, arguments: &[&str]) -> Option<String> {
        let (program, prefix) = self.kubectl.split_first().expect("a kubectl command");
        let output = std::process::Command::new(program)
            .args(prefix)
            .args(["-n", &self.namespace])
            .args(arguments)
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    /// A shell command in the unit's workload container, as the image's own user.
    fn shell(&self, pod: &str, script: &str) -> String {
        self.kubectl(&["exec", pod, "-c", "workload", "--", "sh", "-c", script])
    }
}

/// A shell in the unit through the system's own terminal path: the environment attached to the connector in
/// the pod, a shell opened by a command, and a tunnel to it that a ticket the system signed opens. Answers what
/// the screen showed once `wanted` appeared, or why it did not.
async fn terminal_says(
    world: &World,
    machine: Id,
    environment: Id,
    input: &str,
    wanted: &str,
) -> Result<String, String> {
    use orch_link_contract::commands::{EnvAttach, IndexMode, PtyClose, PtyOpen};
    use orch_link_contract::tunnels::Service;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let dispatcher = world.api.link.dispatcher();
    dispatcher
        .run(
            machine,
            &SerialKey::Environment(environment),
            &EnvAttach {
                resources: None,
                data_generation: None,
                storage: None,
                data_sources: None,
                environment_id: environment,
                workspace_root: None,
                ignore: Vec::new(),
                index: IndexMode::Full,
                mounts: None,
                variables: BTreeMap::new(),
                secrets: BTreeMap::new(),
            },
        )
        .await
        .map_err(|error| format!("env.attach: {error}"))?;
    let terminal_id = new_id();
    dispatcher
        .run(
            machine,
            &SerialKey::None,
            &PtyOpen {
                environment_id: environment,
                terminal_id,
                cols: 100,
                rows: 30,
                cwd: String::new(),
                record: false,
            },
        )
        .await
        .map_err(|error| format!("pty.open: {error}"))?;
    let ticket = world
        .api
        .link
        .tickets()
        .terminal(machine, Service::Pty { terminal_id })
        .map_err(|error| format!("ticket: {error}"))?;
    let mut tunnel = tokio::time::timeout(
        Duration::from_secs(60),
        world.api.link.terminals().open(machine, &ticket),
    )
    .await
    .map_err(|_| "the shell never answered the tunnel".to_owned())?
    .map_err(|error| format!("tunnel: {error:?}"))?;
    tunnel
        .write_all(input.as_bytes())
        .await
        .map_err(|error| format!("typing: {error}"))?;
    let mut seen = String::new();
    let mut buffer = vec![0_u8; 4096];
    let outcome = tokio::time::timeout(Duration::from_secs(60), async {
        while !seen.contains(wanted) {
            let count = tunnel
                .read(&mut buffer)
                .await
                .map_err(|error| format!("reading: {error}"))?;
            if count == 0 {
                return Err(format!("the tunnel ended; the screen showed {seen:?}"));
            }
            seen.push_str(&String::from_utf8_lossy(&buffer[..count]));
        }
        Ok(())
    })
    .await;
    drop(tunnel);
    let _ = dispatcher
        .run(machine, &SerialKey::None, &PtyClose { terminal_id })
        .await;
    match outcome {
        Ok(Ok(())) => Ok(seen),
        Ok(Err(why)) => Err(why),
        Err(_) => Err(format!("{wanted:?} never came; the screen showed {seen:?}")),
    }
}

/// A line of the proof's log, with the time it was said.
fn say(what: &str) {
    eprintln!("[{}] {what}", chrono::Utc::now().format("%H:%M:%SZ"));
}

/// What the cluster holds when a step fails: the pods, their events, and what each container said.
fn diagnose(cluster: &Cluster) {
    let show = |title: &str, arguments: &[&str]| {
        eprintln!(
            "--- {title}\n{}",
            cluster.kubectl_try(arguments).unwrap_or_default()
        );
    };
    show("pods", &["get", "pods", "-o", "wide"]);
    show("events", &["get", "events", "--sort-by=.lastTimestamp"]);
    let pods = cluster
        .kubectl_try(&["get", "pods", "-o", "name"])
        .unwrap_or_default();
    for pod in pods.lines() {
        show(&format!("describe {pod}"), &["describe", pod]);
        for container in ["stage", "guard", "workload"] {
            show(
                &format!("logs {pod} {container}"),
                &["logs", pod, "-c", container, "--tail=60"],
            );
        }
    }
}

/// A runtime a test makes through the node.
struct KubeUnit {
    runtime_id: Id,
    environment_id: Id,
    template_id: Id,
    name: String,
    volumes: Vec<(String, String)>,
    hint: String,
}

/// The system's side of one node: what the engine's provisioning sends, written out for this driver.
struct KubeFleet<'a> {
    world: &'a World,
    cluster: &'a Cluster,
    host: Id,
}

impl KubeFleet<'_> {
    async fn run<B: CommandBody>(&self, key: &SerialKey, body: &B) -> Result<B::Result, RunError> {
        self.world
            .api
            .link
            .dispatcher()
            .run(self.host, key, body)
            .await
    }

    fn unit(&self) -> KubeUnit {
        let runtime_id = new_id();
        KubeUnit {
            runtime_id,
            environment_id: new_id(),
            template_id: new_id(),
            name: unit_name(RuntimeKind::Environment, &runtime_id),
            volumes: ["workspace", "home", "state"]
                .iter()
                .map(|role| {
                    (
                        (*role).to_owned(),
                        format!("orch-test-k8s-{}-{role}", self.cluster.token),
                    )
                })
                .collect(),
            hint: format!("orch-test-k8s-{}-token", self.cluster.token),
        }
    }

    /// The connector put in the node's tool cache from a server on loopback, as the first phase of a provisioning.
    async fn prepare(&self) -> ProgramServer {
        let server = ProgramServer::serve(&self.cluster.connector, "orch-connector").await;
        self.run(
            &SerialKey::Tool("orch-connector".into()),
            &NodeToolEnsure {
                job_id: None,
                name: "orch-connector".into(),
                version: "0.1.0".into(),
                url: server.url.clone(),
                sha256: server.sha256.clone(),
            },
        )
        .await
        .expect("the connector could not be put in the cache");
        server
    }

    /// The rows that provisioning commits before the node may report on a runtime.
    async fn record(&self, unit: &KubeUnit) {
        let mut tx = self.world.db.begin().await.unwrap();
        let home: Id = sqlx::query_scalar("SELECT personal_org_id FROM users WHERE id=$1")
            .bind(self.world.user)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        sqlx::query("INSERT INTO env_templates(id,name,driver,spec) VALUES($1,$2,'kubernetes','{}') ON CONFLICT (id) DO NOTHING")
            .bind(unit.template_id)
            .bind(&unit.name)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("INSERT INTO environments(id,name,owner_id,org_id,home_node_id,placement,template_id) VALUES($1,$2,$3,$4,$4,'managed',$5) ON CONFLICT (id) DO NOTHING")
            .bind(unit.environment_id).bind(&unit.name).bind(self.world.user).bind(home).bind(unit.template_id)
            .execute(&mut *tx).await.unwrap();
        sqlx::query("INSERT INTO runtimes(id,kind,host_id,unit_name,environment_id,driver,template_id,spec_hash,state,successor_of) SELECT $1,'environment',id,$2,$3,'kubernetes',$4,$5,'creating',NULL FROM hosts WHERE machine_id=$6")
            .bind(unit.runtime_id).bind(&unit.name).bind(unit.environment_id).bind(unit.template_id).bind(vec![0u8;32]).bind(self.host)
            .execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
    }

    /// The labels a volume is made with, and removed by: the system that owns it.
    fn volume_labels(&self) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("orch.system".into(), self.cluster.system()),
            ("orch.test".into(), self.cluster.token.clone()),
        ])
    }

    /// Makes the volumes of a unit, then the unit, as the phases `volumes` and `create` do.
    async fn make(&self, unit: &KubeUnit) -> Result<NodeRuntimeCreateResult, RunError> {
        self.record(unit).await;
        let system = self.cluster.system();
        for (_, name) in &unit.volumes {
            self.run(
                &SerialKey::Volume(name.clone()),
                &NodeVolumeEnsure {
                    driver: "kubernetes".into(),
                    name: name.clone(),
                    storage: None,
                    labels: self.volume_labels(),
                },
            )
            .await?;
        }
        let token = self.world.token(TokenFor::Machine, &unit.hint).await;
        let secret = self
            .world
            .secret(unit.runtime_id, "enroll_token", &token)
            .await;
        let mut volumes = serde_json::Map::new();
        for (role, name) in &unit.volumes {
            volumes.insert(role.clone(), json!({ "name": name, "storage": null }));
        }
        let mut labels = unit_labels(&system, RuntimeKind::Environment, &unit.runtime_id);
        labels.insert("orch.test".into(), self.cluster.token.clone());
        let spec = json!({
            "runtime_id": unit.runtime_id, "kind": "environment", "name": unit.name, "driver": "kubernetes",
            "spec": { "image": self.cluster.unit_image },
            "resources": {}, "roles": ["environment"],
            "gateway_url": format!("wss://{}:{}", self.cluster.gateway, self.world.gateway.pinned.port()),
            "enroll_token": { "$secret": secret },
            "ca_cert": self.world.secrets.ca_certificate().pem(),
            "tools": [], "volumes": volumes, "mounts": [], "env": {},
            "labels": labels
        });
        self.run(
            &SerialKey::Runtime(unit.runtime_id),
            &NodeRuntimeCreate(serde_json::from_value(spec).unwrap()),
        )
        .await
    }

    async fn start(&self, unit: &KubeUnit) -> Result<(), RunError> {
        let runtime_id = unit.runtime_id;
        self.run(
            &SerialKey::Runtime(runtime_id),
            &NodeRuntimeStart { runtime_id },
        )
        .await
        .map(|_| ())
    }

    async fn stop(&self, unit: &KubeUnit) -> Result<(), RunError> {
        let runtime_id = unit.runtime_id;
        self.run(
            &SerialKey::Runtime(runtime_id),
            &NodeRuntimeStop {
                runtime_id,
                reason: None,
            },
        )
        .await
        .map(|_| ())
    }

    /// `node.volume.remove`, with the labels the volume was made with.
    async fn remove_volume(&self, name: &str) -> Result<(), RunError> {
        self.run(
            &SerialKey::Volume(name.to_owned()),
            &NodeVolumeRemove {
                driver: "kubernetes".into(),
                name: name.to_owned(),
                storage: None,
                labels: self.volume_labels(),
            },
        )
        .await
        .map(|_| ())
    }

    async fn remove(&self, unit: &KubeUnit) -> Result<(), RunError> {
        let runtime_id = unit.runtime_id;
        self.run(
            &SerialKey::Runtime(runtime_id),
            &NodeRuntimeRemove {
                runtime_id,
                keep_volumes: true,
                volume_refs: vec![],
            },
        )
        .await
        .map(|_| ())
    }
}

/// What the pod of a unit is, as the cluster reports it: its identity and its workload container's.
#[derive(Debug, PartialEq, Eq)]
struct Identity {
    pod_uid: String,
    workload_container: String,
    restarts: String,
}

/// The claims of the test's namespace, by their native names.
fn claims(cluster: &Cluster) -> BTreeSet<String> {
    cluster
        .kubectl(&["get", "pvc", "-o", "name"])
        .lines()
        .map(str::to_owned)
        .collect()
}

fn identity(cluster: &Cluster, pod: &str) -> Identity {
    let get = |path: &str| cluster.kubectl(&["get", "pod", pod, "-o", &format!("jsonpath={path}")]);
    Identity {
        pod_uid: get("{.metadata.uid}"),
        workload_container: get("{.status.containerStatuses[?(@.name=='workload')].containerID}"),
        restarts: get("{.status.containerStatuses[?(@.name=='workload')].restartCount}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_container_environment_is_made_started_stopped_started_again_with_its_files_kept_and_removed_on_a_cluster()
 {
    let Some(cluster) = Cluster::from_environment() else {
        return;
    };
    // Whatever step fails, the log says what the cluster held at that moment.
    let for_the_panic = cluster.clone();
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        original(info);
        diagnose(&for_the_panic);
    }));
    say("the world and the node start");
    let mut world = World::start_on(&cluster.gateway, &cluster.gateway).await;
    let mut node = common::NodeUnderTest::start_with(
        &world,
        &cluster.token,
        &cluster.system(),
        orch_node::kubernetes::register(Registrations::new()).unwrap(),
        |settings| {
            // The machine declares Kubernetes in its policy, as an operator does with `orch-node policy set`,
            // and the node's settings follow that policy.
            let mut drivers = orch_link_contract::policy::MachineDrivers::managed();
            drivers.allowed = vec![orch_node::kubernetes::NAME.to_owned()];
            drivers
                .configurations
                .insert(orch_node::kubernetes::NAME.to_owned(), cluster.connection());
            let store = orch_node_core::machine_policy::open(settings).expect("the machine policy");
            store
                .set_here(
                    orch_link_contract::policy::PolicyKey::Drivers,
                    &json!(drivers),
                )
                .expect("the machine could not declare Kubernetes");
            orch_node_core::machine_policy::configure(settings).expect("the settings follow it");
        },
        &[orch_node::kubernetes::NAME],
    )
    .await;
    let fleet = KubeFleet {
        world: &world,
        cluster: &cluster,
        host: node.machine_id,
    };
    let _program = fleet.prepare().await;
    say("the connector is in the node's tool cache");

    // The unit, made and started: its connector enrolls through the gateway as a machine of its own.
    let unit = fleet.unit();
    let before = claims(&cluster);
    fleet.make(&unit).await.expect("the unit could not be made");
    let ours: BTreeSet<String> = claims(&cluster).difference(&before).cloned().collect();
    assert_eq!(ours.len(), unit.volumes.len(), "one claim for each volume");
    say("made: the claims and the pod exist, the unit is stopped");
    fleet
        .start(&unit)
        .await
        .expect("the unit could not be started");
    let machine = tokio::time::timeout(CLUSTER_PATIENCE, world.wait_enrolled(&unit.hint))
        .await
        .expect("the connector never enrolled");
    eventually_for(CLUSTER_PATIENCE, "the connector to be online", || {
        let (world, machine) = (&world, machine);
        async move { world.api.link.is_online(machine) }
    })
    .await;
    say(&format!(
        "started: the connector enrolled and is online, machine {machine}"
    ));

    // A command in the unit: a file on each of the claims and one in the container's own writable layer.
    let first = identity(&cluster, &unit.name);
    say(&format!("first identity: {first:?}"));
    assert_eq!(first.restarts, "0");
    let said = cluster.shell(
        &unit.name,
        "echo claim > /workspace/claim-marker && echo layer > /layer-marker && echo ran-in-the-unit && id -u",
    );
    assert!(said.contains("ran-in-the-unit"), "{said}");
    say(&format!("a command ran in the unit: {said:?}"));
    // The same, through Orch: a terminal on the environment, typed into through a tunnel the system opens.
    let terminal = terminal_says(
        &world,
        machine,
        unit.environment_id,
        "echo $((6*7)) > /workspace/orch-terminal-marker; echo orch-terminal-$(cat /workspace/orch-terminal-marker)\n",
        "orch-terminal-42",
    )
    .await;
    say(&format!("a terminal through Orch: {terminal:?}"));

    // Stopped: the connector says goodbye and the unit stays, the same pod and the same container.
    fleet
        .stop(&unit)
        .await
        .expect("the unit could not be stopped");
    eventually_for(CLUSTER_PATIENCE, "the connector to go offline", || {
        let (world, machine) = (&world, machine);
        async move { !world.api.link.is_online(machine) }
    })
    .await;
    assert_eq!(
        identity(&cluster, &unit.name),
        first,
        "a stopped unit is kept"
    );
    say("stopped: the connector is offline and the same pod and container stay");

    // Started again: the same machine comes back with the identity it kept on its state claim, and nothing enrolls
    // again; the claim's file and the writable layer's file are still there.
    fleet
        .start(&unit)
        .await
        .expect("the unit could not be started again");
    eventually_for(CLUSTER_PATIENCE, "the connector to come back", || {
        let (world, machine) = (&world, machine);
        async move { world.api.link.is_online(machine) }
    })
    .await;
    assert_eq!(identity(&cluster, &unit.name), first);
    assert_eq!(
        cluster.shell(&unit.name, "cat /workspace/claim-marker /layer-marker"),
        "claim\nlayer"
    );
    say("started again: same pod, same container, the claim's file and the layer's file are there");
    if terminal.is_ok() {
        assert_eq!(
            cluster.shell(&unit.name, "cat /workspace/orch-terminal-marker"),
            "42",
            "the file the terminal wrote stays on the claim"
        );
    }

    // Stopped for the end, and removed: the pod goes, the claims stay.
    fleet
        .stop(&unit)
        .await
        .expect("the unit could not be stopped");
    fleet
        .remove(&unit)
        .await
        .expect("the unit could not be removed");
    assert!(
        cluster
            .kubectl_try(&["get", "pod", &unit.name, "-o", "name"])
            .is_none(),
        "the pod is gone"
    );
    assert!(ours.is_subset(&claims(&cluster)), "the claims stay");
    say(&format!(
        "removed: the pod is gone and its {} claims stay",
        ours.len()
    ));

    // The volumes go when the system says so, each with the labels it was made with.
    for (_, volume) in &unit.volumes {
        fleet
            .remove_volume(volume)
            .await
            .expect("a volume could not be removed");
    }
    assert!(
        ours.is_disjoint(&claims(&cluster)),
        "the claims are gone with their volumes"
    );
    // Removing what is not there is done.
    for (_, volume) in &unit.volumes {
        fleet
            .remove_volume(volume)
            .await
            .expect("removing a removed volume is done");
    }
    say("volumes removed: their claims are gone");

    assert!(
        terminal.is_ok(),
        "the terminal through Orch failed: {terminal:?}"
    );
    node.stop().await;
    world.stop().await;
}

#[tokio::test]
async fn a_machine_that_declares_kubernetes_reports_its_setup_schema_and_offers_nothing_until_a_cluster_answers()
 {
    let directory = tempfile::tempdir().unwrap();
    let mut settings = NodeSettings::in_dir(directory.path());
    settings.drivers = vec![orch_node::kubernetes::NAME.to_owned()];
    let connection = json!({
        "kubeconfig": directory.path().join("no-such-kubeconfig"),
        "namespace": "work",
        "storage_class": "local-path",
        "volume_request": "10Gi",
        "runtime_classes": [{"runtime": "runc"}],
        "tool_stager_image": format!("registry.example/helper@sha256:{}", "a".repeat(64)),
    });
    settings
        .driver_configurations
        .insert(orch_node::kubernetes::NAME.to_owned(), connection.clone());
    let registrations = orch_node::kubernetes::register(Registrations::new()).unwrap();
    let host = DriverHost::load(&settings, registrations, Arc::new(BlobDesk::new())).await;
    // Nothing is schedulable, and the reason is the cluster, not the declaration.
    assert!(host.descriptors().is_empty());
    let failure = host.failures();
    assert!(
        failure["kubernetes"].contains("kubeconfig"),
        "the reason names the unreadable kubeconfig: {failure:?}"
    );
    // The setup schema is still reported, and it accepts a complete object and refuses a faulty one.
    let schemas = host.configuration_schemas();
    let schema = &schemas["kubernetes"];
    orch_plugin::schema::validate(schema, &connection).expect("a complete connection object");
    let mut faulty = connection;
    faulty["namespace"] = json!("Not A Namespace");
    assert!(orch_plugin::schema::validate(schema, &faulty).is_err());
}
