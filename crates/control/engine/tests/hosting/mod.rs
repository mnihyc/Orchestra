//! A scripted host for the tests of environments: a node that answers the commands that make, start, stop and remove units, a unit whose connector comes online when the test says it does, and the rows that let a person use them.
//!
//! Between the engine and a unit stand the node (the host's machine, with the role `node`) and the
//! unit's connector (a machine with the role `environment`). Both are scripted: the node logs every
//! command it is given, in order, and answers each with success unless a test told it to refuse; the
//! connector answers `env.attach` with the facts of a small Linux environment. What a real node and a
//! real connector do is the business of the real-daemon test.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(dead_code, reason = "each test file uses some of what is here")]

pub mod cluster;
pub mod physical;

use crate::common::{Added, World, eventually, master_key};
use async_trait::async_trait;
use chrono::Utc;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ResourceKind;
use orch_authz::ownership::{self, NewAccess};
use orch_authz::{Caller, Principal};
use orch_db::Conn;
use orch_db::queries::resources::hosting as host_rows;
use orch_db::queries::resources::{NewHost, insert_host};
use orch_engine::environments::{self, Made, Request};
use orch_engine::hosting::{Enrollment, EnrollmentAsk, HostServices, HostingError, ToolBuild};
use orch_link_contract::commands::{
    Command, CommandError, CommandKind, CommandResult, Empty, ErrorCode, NodeImageEnsureResult,
    NodeRuntimeCreateResult,
};
use orch_link_contract::events::{Event, NodeRuntime};
use orch_link_contract::streams::Stream;
use orch_testkit::doubles::machine::ScriptedMachine;
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

/// The job watcher may take one further pass to store an observed outcome.
const OBSERVATION_PASS: Duration = orch_engine::hosting::WATCH_EVERY;

/// Waits for a hosting event through its configured product bound and one
/// observation pass. The whole predicate is bounded, including a database read.
pub async fn await_event<F, Fut>(what: &str, allowed: Duration, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let observed = tokio::time::timeout(allowed + OBSERVATION_PASS, async {
        loop {
            if check().await {
                return;
            }
            tokio::time::sleep(OBSERVATION_PASS).await;
        }
    })
    .await;
    assert!(
        observed.is_ok(),
        "waiting for {what} exceeded the configured bound {allowed:?} and observation pass {OBSERVATION_PASS:?}"
    );
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the system gives a unit that is made: the token, the address, the authority and the program.
pub struct Services;

/// This fake catalogue is already in memory; observations never ask a simulated machine or file.
pub fn listed_tools(harnesses: &[String]) -> Vec<ToolBuild> {
    let build = |name: &str, version: &str| ToolBuild {
        name: name.to_owned(),
        version: version.to_owned(),
        url: format!("http://tools.test/{name}"),
        sha256: "0".repeat(64),
    };
    let mut tools = vec![build("orch-connector", "0.1.0")];
    tools.extend(harnesses.iter().map(|kind| build(kind, "1.0.0")));
    tools
}

#[async_trait]
impl HostServices for Services {
    fn observed_tools(
        &self,
        _os: &str,
        _arch: &str,
        requests: &[orch_engine::hosting::HarnessRequest],
    ) -> Option<Vec<ToolBuild>> {
        if requests.iter().any(|request| {
            request
                .version
                .as_deref()
                .is_some_and(|version| version != "1.0.0")
        }) {
            return None;
        }
        Some(listed_tools(
            &requests
                .iter()
                .map(|request| request.kind.clone())
                .collect::<Vec<_>>(),
        ))
    }
    async fn enrollment(
        &self,
        conn: &mut Conn,
        ask: &EnrollmentAsk,
    ) -> Result<Enrollment, HostingError> {
        let sealed = master_key()
            .seal(
                format!("oenr_test_{}", ask.runtime).as_bytes(),
                format!("secrets/environment/{}/enroll_token", ask.runtime).as_bytes(),
            )
            .map_err(|error| HostingError::Failed(error.to_string()))?;
        let stored = orch_db::queries::settings::put_secret(
            conn,
            "environment",
            ask.runtime,
            "enroll_token",
            &sealed,
            None,
            Utc::now(),
        )
        .await
        .map_err(|error| HostingError::Failed(error.to_string()))?;
        Ok(Enrollment {
            secret: stored.id,
            gateway_url: "wss://gateway.orch.internal:9443".to_owned(),
            ca_cert: "-----BEGIN CERTIFICATE-----\ntest\n-----END CERTIFICATE-----\n".to_owned(),
            system: "test-system".to_owned(),
        })
    }

    async fn tools(
        &self,
        _: &mut Conn,
        _: &str,
        _: &str,
        harnesses: &[String],
    ) -> Result<Vec<ToolBuild>, HostingError> {
        Ok(listed_tools(harnesses))
    }
}

/// A command the node was given.
#[derive(Debug, Clone)]
pub struct Call {
    /// Its kind.
    pub kind: CommandKind,
    /// Its body.
    pub body: Json,
}

type Refusals = Mutex<HashMap<CommandKind, VecDeque<CommandError>>>;
type OrderedCalls = Arc<Mutex<Vec<(Id, Command)>>>;

/// A host with a node that answers, and the rows that make it a place.
pub struct ScriptedHost {
    /// The host.
    pub id: Id,
    /// The machine of its node.
    pub machine: Id,
    /// The node as a scripted machine.
    pub added: Added,
    calls: Arc<Mutex<Vec<Call>>>,
    refusals: Arc<Refusals>,
}

/// The facts a unit's connector reports when an environment is attached to it.
pub fn facts() -> Json {
    json!({
        "os": "linux", "arch": "x86_64",
        "shell": { "name": "sh", "path": "/bin/sh" },
        "user": "agent", "home": "/home/agent", "workspace_root": "/workspace",
        "path_style": "posix", "case_sensitive": true,
        "executors": [{
            "kind": "orch-exec", "version": "0.1.0", "available": true, "reason": null,
            "sandbox_modes": ["read_only", "workspace", "open"], "tool_sets": ["plain"]
        }],
        "mcp_found": [], "index": "full", "mounts": [],
        "private_views": false, "suspend": false, "git": true, "generation": 1
    })
}

impl ScriptedHost {
    /// A host owned by the world's maker, at home in the root org, with `memory_mb` of memory to
    /// declare, a Docker driver and a node that answers.
    pub async fn on(world: &World, name: &str, memory_mb: i64) -> Self {
        let added = world.add_machine(name, &["node"]).await;
        let id = new_id();
        let mut conn = world.pool.acquire().await.unwrap();
        sqlx::query("UPDATE machines SET placement = 'managed', owner_id = NULL, facts=jsonb_set(facts,'{policy}',$2) WHERE id = $1")
            .bind(added.id)
            .bind(json!(orch_link_contract::policy::LocalPolicy::resolve(&["*".into()],&Default::default(),&Default::default()).unwrap()))
            .execute(&mut *conn)
            .await
            .unwrap();
        insert_host(
            &mut conn,
            &NewHost {
                id,
                machine_id: added.id,
                owner_id: Some(world.ada),
                home_id: Some(world.root),
                name,
            },
            Utc::now(),
        )
        .await
        .unwrap();
        sqlx::query(
            "UPDATE hosts SET capacity = $2, drivers = $3, storages = $4 WHERE id = $1",
        )
        .bind(id)
        .bind(json!({ "cpus": 8, "memory_mb": memory_mb, "disk_gb": 200 }))
        .bind(json!({
            "docker": {
                "name": "docker", "title": "Docker",
                "lifecycle": "unit", "suspend": true, "data_storage": {"made_by_orch":"volume","existing_directory":true,"reason":null},
                "kept_file_transfer": {"supported":true,"reason":null},
                "commands_after_start": {"supported":true,"reason":null},
                "volumes": true,
                "spec_schemas": { "environment": {}, "provider": {} },
                "runtime_classes": ["runc"], "sources": ["image", "build", "archive"]
            }
        }))
        .bind(json!([{ "name": "fast", "default": true, "capacity_gb": 100, "free_gb": 80, "private_views": false }]))
        .execute(&mut *conn)
        .await
        .unwrap();
        ownership::start_resource(
            &mut conn,
            ObjectRef {
                kind: ResourceKind::Host.into(),
                id,
            },
            world.ada,
            NewAccess::Defaults,
            Time::new(Utc::now()),
        )
        .await
        .unwrap();
        drop(conn);
        let host = Self {
            id,
            machine: added.id,
            added,
            calls: Arc::new(Mutex::new(Vec::new())),
            refusals: Arc::new(Mutex::new(HashMap::new())),
        };
        host.script();
        host
    }

    fn script(&self) {
        self.install(None);
    }

    /// Records this fixture's actual node execution order without changing other hosts' logs.
    pub fn trace_calls(&self, log: &OrderedCalls) {
        self.install(Some(Arc::clone(log)));
    }

    fn install(&self, ordered: Option<OrderedCalls>) {
        let handler = &self.added.machine.handler;
        for kind in [
            CommandKind::NodeImageEnsure,
            CommandKind::NodeVolumeEnsure,
            CommandKind::NodeVolumeRemove,
            CommandKind::NodeToolEnsure,
            CommandKind::NodeRuntimeCreate,
            CommandKind::NodeRuntimeStart,
            CommandKind::NodeRuntimeStop,
            CommandKind::NodeRuntimeRemove,
            CommandKind::NodeResourcesApply,
            CommandKind::NodeResourcesInspect,
            CommandKind::NodeCapabilitiesApply,
            CommandKind::NodeKeepEstimate,
            CommandKind::NodeKeepFiles,
            CommandKind::NodeKeptRemove,
            CommandKind::NodeKeptImport,
            CommandKind::NodeKeptSweep,
            CommandKind::NodeResourcesList,
            CommandKind::NodeResourcesRemove,
            CommandKind::NodeNetworkApply,
            CommandKind::NodeRuntimeVolumesMeasure,
            CommandKind::NodeVolumeCarryCheck,
            CommandKind::NodeMountMeasure,
        ] {
            let calls = Arc::clone(&self.calls);
            let ordered = ordered.clone();
            let machine = self.machine;
            let refusals = Arc::clone(&self.refusals);
            handler.on(kind, move |command: &Command| {
                if let Some(ordered) = &ordered { lock(ordered).push((machine, command.clone())); }
                lock(&calls).push(Call {
                    kind,
                    body: command.body().unwrap(),
                });
                if let Some(error) = lock(&refusals).get_mut(&kind).and_then(VecDeque::pop_front) {
                    return Err(error);
                }
                Ok(match command {
                    Command::NodeImageEnsure(_) => {
                        CommandResult::NodeImageEnsure(NodeImageEnsureResult {
                            programs: Default::default(),
                            digest: Some("sha256:abc".to_owned()),
                            built: None,
                        })
                    }
                    Command::NodeVolumeEnsure(_) => CommandResult::NodeVolumeEnsure(Empty {}),
                    Command::NodeVolumeRemove(_) => CommandResult::NodeVolumeRemove(Empty {}),
                    Command::NodeVolumeCarryCheck(_) => CommandResult::NodeVolumeCarryCheck(orch_link_contract::commands::NodeVolumeCarryCheckResult { available: true, reason: None }),
                    Command::NodeMountMeasure(_) => CommandResult::NodeMountMeasure(orch_driver_contract::volumes::VolumeMeasure { file_count: 3, total_bytes: 8192, file_states: None }),
                    Command::NodeRuntimeVolumesMeasure(request) => CommandResult::NodeRuntimeVolumesMeasure(orch_link_contract::commands::NodeRuntimeVolumesMeasureResult { volumes: request.volumes.iter().map(|volume| orch_link_contract::commands::MeasuredVolume { selector: volume.selector, measure: orch_driver_contract::volumes::VolumeMeasure { file_count: 3, total_bytes: 8192,
                        file_states: (volume.selector == orch_driver_contract::volumes::OwnedVolumeSelector::FileStates).then_some(orch_driver_contract::volumes::FileStatesMeasure { roots: 0, objects: 0, total_bytes: 8192, journals: 0, complete: true }) } }).collect() }),
                    Command::NodeToolEnsure(_) => CommandResult::NodeToolEnsure(
                        serde_json::from_value(json!({ "path": "/tools/orch-connector" })).unwrap(),
                    ),
                    Command::NodeRuntimeCreate(request) => {
                        CommandResult::NodeRuntimeCreate(NodeRuntimeCreateResult {
                            unit: orch_driver_contract::units::UnitRef {
                                reference: "unit".to_owned(),
                                name: request.0.name.clone(),
                                runtime_id: request.0.runtime_id,
                                kind: request.0.kind,
                            },
                        })
                    }
                    Command::NodeRuntimeStart(_) => CommandResult::NodeRuntimeStart(Empty {}),
                    Command::NodeRuntimeStop(_) => CommandResult::NodeRuntimeStop(Empty {}),
                    Command::NodeRuntimeRemove(_) => CommandResult::NodeRuntimeRemove(Empty {}),
                    Command::NodeResourcesApply(_)=>CommandResult::NodeResourcesApply(Empty {}),
                    Command::NodeResourcesInspect(_)=>CommandResult::NodeResourcesInspect(orch_driver_contract::runtime::RuntimeChanges {pending_reason:None,live_changes:orch_driver_contract::descriptor::LiveChanges {resources:true,..Default::default()},resources:orch_driver_contract::runtime::Resources {memory_mb:Some(256),..Default::default()},memory_used_bytes:Some(64<<20)}),
                    Command::NodeKeepEstimate(_)=>CommandResult::NodeKeepEstimate(orch_driver_contract::kept::KeepEstimate {bytes:64<<20,seconds:3,flatten:false}),
                    Command::NodeKeepFiles(body)=>CommandResult::NodeKeepFiles(orch_driver_contract::kept::KeptImage {system:"test".into(),environment:body.environment_id,generation:body.generation,capture:body.capture,reference:format!("orch-keep-{}:{}",body.environment_id,body.generation),bytes:64<<20,total_bytes:72<<20,layers:1}),
                    Command::NodeKeptRemove(_)=>CommandResult::NodeKeptRemove(Empty {}),
                    Command::NodeKeptImport(_)=>CommandResult::NodeKeptImport(Empty {}),
                    Command::NodeKeptSweep(_)=>CommandResult::NodeKeptSweep(Empty {}),
                    Command::NodeResourcesList(_)=>CommandResult::NodeResourcesList(orch_driver_contract::resources::ResourceInventory {observed_at:orch_types::Time::new(chrono::DateTime::UNIX_EPOCH),resources:Vec::new(),unavailable:Vec::new()}),
                    Command::NodeResourcesRemove(_)=>CommandResult::NodeResourcesRemove(Empty {}),
                    Command::NodeCapabilitiesApply(body)=>CommandResult::NodeCapabilitiesApply(orch_link_contract::commands::NodeCapabilitiesApplyResult {
                        applied:true,restarted:body.capabilities.get("granted").and_then(Json::as_array).is_some_and(Vec::is_empty),reason:None,
                    }),
                    Command::NodeNetworkApply(body) => {
                        let mut state = orch_api_contract::environment::NetworkState::open(
                            orch_api_contract::environment::UnitNetworkMode::Bridge,
                        );
                        state.required_mode = body.required_mode;
                        state.revision = body.revision.clone();
                        if body.required_mode
                            == orch_api_contract::environment::EgressMode::Restricted
                        {
                            state.state =
                                orch_api_contract::environment::NetworkReadiness::Unavailable;
                            state.reason =
                                Some("the old open unit needs a verified replacement".to_owned());
                        }
                        CommandResult::NodeNetworkApply(state)
                    }
                    other => {
                        return Err(CommandError::new(
                            ErrorCode::Unsupported,
                            format!("no script for {}", other.kind()),
                        ));
                    }
                })
            });
        }
    }

    /// Adds a fixture-specific reply while retaining the node's call log and queued refusals.
    pub fn answer<F>(&self, kind: CommandKind, reply: F)
    where
        F: Fn(&Command) -> Result<CommandResult, CommandError> + Send + Sync + 'static,
    {
        let calls = Arc::clone(&self.calls);
        let refusals = Arc::clone(&self.refusals);
        self.added
            .machine
            .handler
            .on(kind, move |command: &Command| {
                lock(&calls).push(Call {
                    kind,
                    body: command.body().unwrap(),
                });
                if let Some(error) = lock(&refusals).get_mut(&kind).and_then(VecDeque::pop_front) {
                    return Err(error);
                }
                reply(command)
            });
    }

    /// Every command the node was given, in order.
    pub fn calls(&self) -> Vec<Call> {
        lock(&self.calls).clone()
    }

    /// The kinds of the commands the node was given, in order.
    pub fn kinds(&self) -> Vec<CommandKind> {
        self.calls().iter().map(|call| call.kind).collect()
    }

    /// How many commands of a kind the node was given.
    pub fn count(&self, kind: CommandKind) -> usize {
        self.calls().iter().filter(|call| call.kind == kind).count()
    }

    /// The next command of this kind fails, with this error.
    pub fn refuse(&self, kind: CommandKind, error: CommandError) {
        lock(&self.refusals)
            .entry(kind)
            .or_default()
            .push_back(error);
    }

    /// Waits until the node has been given `count` commands of a kind.
    pub async fn wait_for(&self, kind: CommandKind, count: usize) {
        eventually(&format!("{count} {kind} at the node"), || async move {
            self.count(kind) >= count
        })
        .await;
    }

    /// How many commands of a kind the node has begun, including those it holds back.
    pub fn began(&self, kind: CommandKind) -> usize {
        self.added
            .machine
            .handler
            .executions()
            .iter()
            .filter(|run| run.kind == kind)
            .count()
    }

    /// Waits until the node has begun `count` commands of a kind.
    pub async fn wait_began(&self, kind: CommandKind, count: usize) {
        eventually(
            &format!("{count} {kind} begun at the node"),
            || async move { self.began(kind) >= count },
        )
        .await;
    }

    /// The runtime a `node.runtime.start` was given for, the latest.
    pub fn started(&self) -> Option<Id> {
        self.calls()
            .iter()
            .rev()
            .find(|call| call.kind == CommandKind::NodeRuntimeStart)
            .and_then(|call| call.body["runtime_id"].as_str()?.parse().ok())
    }

    /// The node tells what it finds a unit in.
    pub async fn tell(&self, runtime: Id, state: &str, detail: Option<&str>) {
        self.added
            .machine
            .publish(
                &Stream::Node,
                &Event::NodeRuntime(NodeRuntime {
                    runtime_id: runtime,
                    state: state.to_owned(),
                    detail: detail.map(str::to_owned),
                }),
            )
            .await;
    }
}

/// A unit's connector, scripted.
pub struct Unit {
    /// Its machine.
    pub id: Id,
    /// The machine as the world holds it.
    pub added: Added,
    calls: Arc<Mutex<Vec<Call>>>,
    refusals: ConnectorRefusals,
}

impl Unit {
    /// The kinds of the commands the connector was given, in order.
    pub fn kinds(&self) -> Vec<CommandKind> {
        lock(&self.calls).iter().map(|call| call.kind).collect()
    }

    /// The commands of a kind the connector was given.
    pub fn calls_of(&self, kind: CommandKind) -> Vec<Call> {
        lock(&self.calls)
            .iter()
            .filter(|call| call.kind == kind)
            .cloned()
            .collect()
    }

    /// How many commands of a kind the connector was given.
    pub fn count(&self, kind: CommandKind) -> usize {
        self.calls_of(kind).len()
    }

    /// The next `count` detaches fail while preserving the connector's ordinary command log.
    pub fn refuse_detaches(&self, count: usize, error: CommandError) {
        let remaining = Mutex::new(count);
        let calls = Arc::clone(&self.calls);
        self.added
            .machine
            .handler
            .on(CommandKind::EnvDetach, move |command| {
                let body = command
                    .body()
                    .map_err(|cause| CommandError::new(ErrorCode::Internal, cause.to_string()))?;
                lock(&calls).push(Call {
                    kind: CommandKind::EnvDetach,
                    body,
                });
                let mut left = lock(&remaining);
                if *left > 0 {
                    *left -= 1;
                    return Err(error.clone());
                }
                Ok(CommandResult::EnvDetach(Empty {}))
            });
    }

    /// How many commands of a kind the connector has begun, including those it holds back.
    pub fn began(&self, kind: CommandKind) -> usize {
        self.added
            .machine
            .handler
            .executions()
            .iter()
            .filter(|run| run.kind == kind)
            .count()
    }

    /// The next `env.attach` fails, with this error.
    pub fn refuse_attach(&self, error: CommandError) {
        lock(&self.refusals).push_back(Refusal {
            error,
            hold_next: false,
        });
    }

    /// The next `env.attach` fails, with this error, and the one after it is held back until
    /// [`Unit::let_go`]: what the refusal left stands for as long as a test wants to look at it,
    /// whatever pause the engine puts between two tries.
    pub fn refuse_attach_then_hold(&self, error: CommandError) {
        lock(&self.refusals).push_back(Refusal {
            error,
            hold_next: true,
        });
    }

    /// The connector holds the commands of `kind` back until [`Unit::let_go`].
    pub fn hold(&self, kind: CommandKind) {
        self.added.machine.handler.hold(kind);
    }

    /// The connector goes on with the commands it holds.
    pub fn let_go(&self) {
        self.added.machine.handler.release();
    }
}

/// What a scripted connector was asked, in order.
type ConnectorCalls = Arc<Mutex<Vec<Call>>>;
/// The error a scripted connector gives to an `env.attach`, and whether it then holds the next one back.
struct Refusal {
    error: CommandError,
    hold_next: bool,
}
/// What a scripted connector gives to the next `env.attach` commands, one each.
type ConnectorRefusals = Arc<Mutex<VecDeque<Refusal>>>;

/// Makes a scripted machine answer `env.attach` with the facts of a small Linux environment whose
/// workspace is where it was told, and `exec.start`, keeping what it was asked.
fn answer_as_connector(machine: &ScriptedMachine) -> (ConnectorCalls, ConnectorRefusals) {
    let calls: ConnectorCalls = Arc::new(Mutex::new(Vec::new()));
    let command_log = Arc::clone(&calls);
    machine
        .handler
        .on(CommandKind::UnitAfterUp, move |command| {
            lock(&command_log).push(Call {
                kind: CommandKind::UnitAfterUp,
                body: command.body().unwrap(),
            });
            let Command::UnitAfterUp(body) = command else {
                unreachable!()
            };
            Ok(CommandResult::UnitAfterUp(
                orch_link_contract::commands::UnitAfterUpResult {
                    name: body.name.clone(),
                    exit: Some(0),
                    duration_ms: 0,
                    timed_out: false,
                    first_lines: String::new(),
                },
            ))
        });
    let detached = Arc::clone(&calls);
    machine.handler.on(CommandKind::EnvDetach, move |command| {
        lock(&detached).push(Call {
            kind: CommandKind::EnvDetach,
            body: command.body().unwrap(),
        });
        Ok(CommandResult::EnvDetach(Empty {}))
    });
    let removed = Arc::clone(&calls);
    machine
        .handler
        .on(CommandKind::EnvDataRemove, move |command| {
            lock(&removed).push(Call {
                kind: CommandKind::EnvDataRemove,
                body: command.body().unwrap(),
            });
            Ok(CommandResult::EnvDataRemove(Empty {}))
        });
    let refusals: ConnectorRefusals = Arc::new(Mutex::new(VecDeque::new()));
    let repair_log = Arc::clone(&calls);
    let state_log = Arc::clone(&calls);
    machine
        .handler
        .on(CommandKind::FsRecordVerify, move |command| {
            lock(&state_log).push(Call {
                kind: CommandKind::FsRecordVerify,
                body: command.body().unwrap(),
            });
            Ok(CommandResult::FsRecordVerify(
                orch_link_contract::source::FsRecordVerifyResult {
                    roots: 0,
                    objects: 0,
                    journals: 0,
                },
            ))
        });
    machine
        .handler
        .on(CommandKind::GitWorktreeRepair, move |command| {
            lock(&repair_log).push(Call {
                kind: CommandKind::GitWorktreeRepair,
                body: command.body().unwrap(),
            });
            Ok(CommandResult::GitWorktreeRepair(
                orch_link_contract::source::GitWorktreeRepairResult { repaired: 0 },
            ))
        });
    let (log, refused) = (Arc::clone(&calls), Arc::clone(&refusals));
    // The handler keeps this closure: a weak reference to it avoids a cycle of references.
    let handler = Arc::downgrade(&machine.handler);
    machine
        .handler
        .on(CommandKind::EnvAttach, move |command: &Command| {
            lock(&log).push(Call {
                kind: CommandKind::EnvAttach,
                body: command.body().unwrap(),
            });
            if let Some(refusal) = lock(&refused).pop_front() {
                if refusal.hold_next
                    && let Some(handler) = handler.upgrade()
                {
                    // This command has left the hold already: only the ones that come later wait.
                    handler.hold(CommandKind::EnvAttach);
                }
                return Err(refusal.error);
            }
            let mut reported = facts();
            if let Command::EnvAttach(attach) = command
                && let Some(root) = &attach.workspace_root
            {
                reported["workspace_root"] = json!(root);
            }
            Ok(CommandResult::EnvAttach(
                serde_json::from_value(reported).unwrap(),
            ))
        });
    let log = Arc::clone(&calls);
    machine
        .handler
        .on(CommandKind::ExecStart, move |command: &Command| {
            lock(&log).push(Call {
                kind: CommandKind::ExecStart,
                body: command.body().unwrap(),
            });
            Ok(CommandResult::ExecStart(
                serde_json::from_value(
                    json!({ "service": "mcp/exec", "version": "0.1.0", "tool_sets": ["plain"] }),
                )
                .unwrap(),
            ))
        });
    let log = Arc::clone(&calls);
    machine
        .handler
        .on(CommandKind::FsMkdir, move |command: &Command| {
            let body: Json = command.body().unwrap();
            lock(&log).push(Call {
                kind: CommandKind::FsMkdir,
                body: body.clone(),
            });
            let path = body["path"].as_str().unwrap_or_default();
            Ok(CommandResult::FsMkdir(
                serde_json::from_value(json!({
                    "name": path.rsplit('/').next().unwrap_or_default(), "path": path, "type": "dir",
                    "size": 0, "mtime": "2026-10-12T00:00:00.000Z", "mode": 493, "link_target": null,
                    "cached": false, "hash": null, "media_type": "inode/directory", "is_text": false
                }))
                .unwrap(),
            ))
        });
    (calls, refusals)
}

/// A connector that is online with these roles, as a person's machine has one.
pub async fn connector(world: &World, name: &str, roles: &[&str]) -> Unit {
    let added = world.add_machine(name, roles).await;
    let (calls, refusals) = answer_as_connector(&added.machine);
    Unit {
        id: added.id,
        added,
        calls,
        refusals,
    }
}

/// The world's own machine, which takes the roles of an agent, an environment and a provider, answering
/// as a connector does what is asked of it as an environment.
pub fn primary(world: &World) -> Unit {
    let (calls, refusals) = answer_as_connector(&world.machine);
    Unit {
        id: world.machine_id,
        added: Added {
            id: world.machine_id,
            machine: Arc::clone(&world.machine),
            seen: Arc::clone(&world.seen),
        },
        calls,
        refusals,
    }
}

/// The connector of a unit comes online: a machine with the role `environment` that answers
/// `env.attach` and `exec.start`, which the runtime then knows as its own.
pub async fn enroll(world: &World, runtime: Id) -> Unit {
    enroll_with(world, runtime, None).await
}

/// Enrolls a connector whose first command of `kind` cannot answer until released.
pub async fn enroll_holding(world: &World, runtime: Id, kind: CommandKind) -> Unit {
    enroll_with(world, runtime, Some(kind)).await
}

async fn enroll_with(world: &World, runtime: Id, held: Option<CommandKind>) -> Unit {
    let role_names = {
        let mut conn = world.pool.acquire().await.unwrap();
        sqlx::query_scalar::<_, Json>("SELECT definition FROM (SELECT body AS definition,created_at,id FROM commands WHERE kind='node.runtime.create' AND body->>'runtime_id'=$1 UNION ALL SELECT plan->'created_spec',created_at,id FROM jobs WHERE plan->>'runtime'=$1 AND plan ? 'created_spec') definitions ORDER BY created_at DESC,id DESC LIMIT 1")
            .bind(runtime.to_string())
            .fetch_optional(&mut *conn)
            .await
            .unwrap()
            .and_then(|definition| definition.get("roles").and_then(Json::as_array).cloned())
            .unwrap_or_else(|| vec![json!("environment")])
            .iter()
            .filter_map(Json::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let roles = role_names.iter().map(String::as_str).collect::<Vec<_>>();
    let unit = connector(world, "unit", &roles).await;
    if let Some(kind) = held {
        unit.hold(kind);
    }
    let mut conn = world.pool.acquire().await.unwrap();
    // A unit's connector has the host's managed identity, which a successor keeps when the old unit is removed.
    sqlx::query("UPDATE machines SET placement = 'managed', owner_id = NULL WHERE id = $1")
        .bind(unit.id)
        .execute(&mut *conn)
        .await
        .unwrap();
    host_rows::adopt_machine(&mut conn, unit.id, runtime, Utc::now())
        .await
        .unwrap();
    unit
}

/// A directory of a machine, registered for the world's maker as an environment of theirs, in status
/// `none` as registering leaves it.
pub async fn directory(world: &World, machine: Id, name: &str, root: &str) -> Id {
    use orch_db::queries::environments::routes::{NewConnected, insert_connected};
    let id = new_id();
    let mut conn = world.pool.begin().await.unwrap();
    let made = insert_connected(
        &mut conn,
        &NewConnected {
            id,
            name,
            owner_id: world.ada,
            machine_id: machine,
            home_node_id: world.topic,
            workspace_root: root,
            remote_write: Some(false),
            terminal: Some(false),
        },
        Utc::now(),
    )
    .await
    .unwrap();
    assert!(made, "the home of the directory is there");
    // Registration writes the actual Owners page as well as the stored first-owner column.
    ownership::start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Environment.into(),
            id,
        },
        world.ada,
        NewAccess::Defaults,
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    conn.commit().await.unwrap();
    id
}

/// A template of environments, owned by the maker, that a Docker host makes from a small image.
pub async fn template(world: &World) -> Id {
    template_of(world, &["orch-exec"], &[]).await
}

/// The same, offering `offers` and letting the harnesses `colocated` run beside its tools.
pub async fn template_of(world: &World, offers: &[&str], colocated: &[&str]) -> Id {
    let id = new_id();
    let mut conn = world.pool.acquire().await.unwrap();
    sqlx::query(
        "INSERT INTO env_templates (id, org_id, home_id, owner_id, name, kind, driver, spec)
         VALUES ($1, $2, $2, $3, 'Small', 'environment', 'docker', $4)",
    )
    .bind(id)
    .bind(world.root)
    .bind(world.ada)
    .bind(json!({
        "image": "busybox:latest", "runtime": "runc",
        "user": { "name": "agent", "uid": 1000, "gid": 1000 },
        "resources": { "cpu_millis": 2000, "memory_mb": 2048, "pids": 512, "disk_gb": 20 },
        "resources_max": { "cpu_millis": 8000, "memory_mb": 8192, "pids": 4096, "disk_gb": 100 },
        "needs": { "cpu_millis": 500, "memory_mb": 1024, "disk_gb": 5 },
        "exec": { "offers": offers, "orch_tools": true },
        "colocated_harnesses": colocated, "host_labels": [], "init": null
    }))
    .execute(&mut *conn)
    .await
    .unwrap();
    ownership::start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Template.into(),
            id,
        },
        world.ada,
        NewAccess::Defaults,
        Time::new(Utc::now()),
    )
    .await
    .unwrap();
    id
}

/// Attaches the test double of what the system gives a unit to the world's engine.
pub fn serve(world: &World) {
    world.engine().attach_host_services(Arc::new(Services));
}

/// Makes an environment for the maker as `POST /environments` does, in a transaction of its own, and
/// sees that the engine takes the job up.
pub async fn make(world: &World, template: Id, name: &str) -> Made {
    make_with(world, template, name, None, &json!({})).await
}

/// As [`make`], naming the host and giving the environment's own values.
pub async fn make_with(
    world: &World,
    template: Id,
    name: &str,
    host: Option<Id>,
    config: &Json,
) -> Made {
    try_make(world, template, name, host, config).await.unwrap()
}

/// As [`make_with`], with the error an environment that cannot be made comes to.
pub async fn try_make(
    world: &World,
    template: Id,
    name: &str,
    host: Option<Id>,
    config: &Json,
) -> Result<Made, environments::CreateError> {
    let mut tx = world.pool.begin().await.unwrap();
    let now = Utc::now();
    let principal = Principal::load(&mut tx, Caller::session(world.ada), Time::new(now))
        .await
        .unwrap();
    let made = environments::create(
        &mut tx,
        now,
        &principal,
        &Request {
            owner: world.ada,
            name: Some(name),
            home: world.topic,
            template: Some(template),
            host: host.map(Some),
            config,
        },
    )
    .await?;
    tx.commit().await.unwrap();
    world.engine().kick();
    Ok(made)
}

/// The actual runtime-start command of this creation job, or its terminal failure, at the
/// existing fixture deadline. A failed reservation must not look like a missing enrollment.
pub async fn runtime_started_for_job(world: &World, host: &ScriptedHost, id: Id) -> Id {
    async fn state(world: &World, id: Id) -> (String, Option<Id>, Option<Json>) {
        sqlx::query_as("SELECT state,(plan->>'runtime')::uuid,error FROM jobs WHERE id=$1")
            .bind(id)
            .fetch_one(world.pool.pool())
            .await
            .unwrap()
    }
    let started = |runtime: Id| {
        host.calls().iter().any(|call| {
            call.kind == CommandKind::NodeRuntimeStart && call.body["runtime_id"] == json!(runtime)
        })
    };
    eventually(
        "the admitted creation starts its own runtime or reports a terminal failure",
        || async {
            let (status, runtime, _) = state(world, id).await;
            matches!(status.as_str(), "failed" | "cancelled" | "succeeded")
                || runtime.is_some_and(&started)
        },
    )
    .await;
    let (status, runtime, error) = state(world, id).await;
    assert!(
        !matches!(status.as_str(), "failed" | "cancelled"),
        "the admitted reservation must provision its runtime: {status}, {error:?}"
    );
    let runtime = runtime.expect("the creation job recorded its own runtime");
    assert!(
        started(runtime),
        "the creation must reach its own native runtime-start command"
    );
    runtime
}

/// As [`make`], with a statement run in the same transaction, before the engine can see the job: the
/// way to change what the job finds when it begins.
pub async fn make_after(world: &World, template: Id, name: &str, statement: &str) -> Made {
    let mut tx = world.pool.begin().await.unwrap();
    let now = Utc::now();
    let principal = Principal::load(&mut tx, Caller::session(world.ada), Time::new(now))
        .await
        .unwrap();
    let made = environments::create(
        &mut tx,
        now,
        &principal,
        &Request {
            owner: world.ada,
            name: Some(name),
            home: world.topic,
            template: Some(template),
            host: None,
            config: &json!({}),
        },
    )
    .await
    .unwrap();
    sqlx::query(sqlx::AssertSqlSafe(statement.to_owned()))
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    world.engine().kick();
    made
}

/// What a job stands at: its state, the phase it is in, and its percentage.
pub async fn job(world: &World, id: Id) -> (String, Json, Option<Json>) {
    sqlx::query_as::<_, (String, Json, Option<Json>)>(
        "SELECT state, progress, error FROM jobs WHERE id = $1",
    )
    .bind(id)
    .fetch_one(world.pool.pool())
    .await
    .unwrap()
}

/// Waits until a job is over, and says how it ended.
pub async fn wait_over(world: &World, id: Id) -> (String, Json, Option<Json>) {
    let hosting = world.engine().config().hosting;
    // Provisioning may wait for enrollment and two attach commands before
    // cleanup. Keep the observer alive through those actual configured waits.
    let allowed =
        hosting.enroll_timeout + hosting.attach_wait * 2 + hosting.stop_wait + hosting.job_lease;
    let mut last = job(world, id).await;
    let observed = tokio::time::timeout(allowed, async {
        while matches!(last.0.as_str(), "queued" | "running") {
            tokio::time::sleep(OBSERVATION_PASS).await;
            last = job(world, id).await;
        }
    })
    .await;
    assert!(
        observed.is_ok(),
        "waiting for hosting job {id} to end exceeded {allowed:?}; last state {}, progress {}, error {:?}",
        last.0,
        last.1,
        last.2
    );
    job(world, id).await
}

/// The status of an environment, and what is said of it.
pub async fn status(world: &World, environment: Id) -> (String, Option<String>) {
    sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT status, status_detail FROM environments WHERE id = $1",
    )
    .bind(environment)
    .fetch_one(world.pool.pool())
    .await
    .unwrap()
}

/// The names of the phases of a job, in order, each with its state.
pub fn phases(progress: &Json) -> Vec<(String, String)> {
    progress["phases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|phase| {
            (
                phase["name"].as_str().unwrap().to_owned(),
                phase["state"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// An environment that is up on `host` without having been made by a job: its rows say what a job
/// would have left, and its unit's connector is online.
pub async fn running(
    world: &World,
    host: &ScriptedHost,
    template: Id,
    name: &str,
) -> (Id, Id, Unit) {
    use orch_db::queries::environments::hosting as env_rows;
    let (environment, runtime) = (new_id(), new_id());
    let now = Utc::now();
    let mut conn = world.pool.acquire().await.unwrap();
    env_rows::insert_managed(
        &mut conn,
        &env_rows::NewManaged {
            id: environment,
            name,
            owner_id: world.ada,
            home_node_id: world.topic,
            template_id: template,
            config: &json!({}),
            host_entry_id: None,
            template_entry_id: None,
        },
        now,
    )
    .await
    .unwrap();
    let volumes = json!({
        "workspace": { "name": format!("orch-ws-{environment}"), "storage": null },
        "home": { "name": format!("orch-home-{environment}"), "storage": null },
        "state": { "name": format!("orch-state-{environment}"), "storage": null },
    });
    env_rows::insert_runtime(
        &mut conn,
        &env_rows::NewRuntime {
            id: runtime,
            host_id: host.id,
            unit_name: &orch_engine::jobs::provision::unit_name(&runtime),
            environment_id: environment,
            driver: "docker",
            template_id: template,
            spec_hash: &[0; 32],
            generation: 1,
            volumes: &volumes,
        },
        now,
    )
    .await
    .unwrap();
    // A running unit has a frozen producer record even when this fixture skips the create job.
    // Recreate/restore must never reconstruct it from a template somebody may have edited later.
    let template_row = host_rows::template(&mut conn, template)
        .await
        .unwrap()
        .unwrap();
    let own: Option<Json> = orch_settings::get(
        &mut conn,
        orch_settings::Place::Environment(environment),
        orch_settings_contract::registry::keys::ENV_RESOURCES,
    )
    .await
    .unwrap();
    let limits = orch_engine::places::limits_of(&template_row.spec, own.as_ref());
    let wanted = orch_engine::jobs::provision::harnesses_wanted(&template_row.spec, &[]);
    let host_state = host_rows::host(&mut conn, host.id).await.unwrap().unwrap();
    let tools = Services
        .tools(
            &mut conn,
            &host_state.platform_os,
            &host_state.platform_arch,
            &wanted,
        )
        .await
        .unwrap();
    let enrollment = Services
        .enrollment(
            &mut conn,
            &EnrollmentAsk {
                runtime,
                host: host.id,
                created_by: world.ada,
                name_hint: name.to_owned(),
            },
        )
        .await
        .unwrap();
    let unit_name = orch_engine::jobs::provision::unit_name(&runtime);
    let mut definition =
        orch_engine::jobs::provision::runtime_spec(&orch_engine::jobs::provision::SpecInputs {
            runtime,
            name: &unit_name,
            driver: "docker",
            spec: &template_row.spec,
            limits: &limits,
            enrollment: &enrollment,
            tools: &tools,
            volumes: &volumes,
        });
    definition["network_policy"] = json!(
        orch_engine::network::policy(
            &mut conn,
            orch_settings::Place::Environment(environment),
            &template_row.spec
        )
        .await
        .unwrap()
    );
    let hash = orch_engine::updates::definition_hash("docker", &template_row.spec, &limits, &tools);
    sqlx::query("UPDATE runtimes SET spec_hash=$2 WHERE id=$1")
        .bind(runtime)
        .bind(hash.as_bytes().as_slice())
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("INSERT INTO jobs(id,kind,environment_id,state,plan,created_by) VALUES($1,'environment_provision',$2,'succeeded',$3,$4)")
        .bind(new_id()).bind(environment).bind(json!({"runtime":runtime,"created_spec":definition,"tools":tools})).bind(world.ada).execute(&mut *conn).await.unwrap();
    drop(conn);
    let unit = enroll(world, runtime).await;
    let mut conn = world.pool.acquire().await.unwrap();
    env_rows::set_runtime_state(&mut conn, runtime, &["creating"], "running", None, now)
        .await
        .unwrap();
    env_rows::make_running(
        &mut conn,
        environment,
        &["none"],
        &env_rows::Up {
            machine: unit.id,
            workspace_root: Some("/workspace"),
            facts: Some(&facts()),
            new_unit: true,
        },
        now,
    )
    .await
    .unwrap();
    (environment, runtime, unit)
}

/// Binds a thread of the world to an environment of the tests, whose tools run through the executor of
/// its unit while the harness stays with the provider.
pub async fn bind(world: &World, thread: Id, environment: Id) {
    sqlx::query(
        "UPDATE threads SET environment_id = $2, arrangement = 'separated', executor = 'orch-exec',
                            tool_set = 'plain'
          WHERE node_id = $1",
    )
    .bind(thread)
    .bind(environment)
    .execute(world.pool.pool())
    .await
    .unwrap();
}

/// A real nonowner Use entry with no inherited unlimited Manage/Use alternative.
pub async fn limited_physical_use(world: &World, kind: ResourceKind, id: Id, maximum: u32) -> Id {
    assert!(matches!(kind, ResourceKind::Machine | ResourceKind::Host));
    let owner = world.person("capacity-source-owner").await;
    let mut tx = world.pool.begin().await.unwrap();
    let principal = Principal::load(
        &mut tx,
        Caller::session(world.ada),
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    ownership::transfer(
        &mut tx,
        &principal,
        ObjectRef {
            kind: kind.into(),
            id,
        },
        owner,
        false,
        &orch_authz::usable::ResourcePolicy::default(),
        &orch_db::audit::Origin::default(),
    )
    .await
    .unwrap();
    let page = orch_db::queries::access::PageKey::new(kind.as_str(), id, "live");
    for permission in [
        format!("{}.use", kind.as_str()),
        format!("{}.manage", kind.as_str()),
    ] {
        orch_db::queries::access::write_row(
            &mut tx,
            &orch_db::queries::access::RowWrite {
                page: &page,
                profile: "any",
                permission: &permission,
                inherit: false,
                locked: false,
            },
            Some(owner),
            world.clock.now(),
        )
        .await
        .unwrap();
    }
    let entry = orch_db::queries::access::testing::add_entry(
        &mut tx,
        &page,
        "any",
        &format!("{}.use", kind.as_str()),
        ("user", Some(world.ada), None),
        None,
        world.clock.now(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE permission_entries SET options=$2 WHERE id=$1")
        .bind(entry)
        .bind(json!({"max_environments":maximum}))
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    entry
}

/// Completed-size assertions use the same background-count wait and second preflight as an open plan.
pub async fn wait_counts(world: &World, plan: &serde_json::Value) -> bool {
    let mut pending = std::collections::HashSet::new();
    let mut values = vec![plan];
    while let Some(value) = values.pop() {
        match value {
            serde_json::Value::Object(object) => {
                if let Some(measurement) = object.get("measurement")
                    && measurement["measured_at"].is_null()
                    && let Some(id) = measurement["job_id"]
                        .as_str()
                        .and_then(|id| id.parse::<Id>().ok())
                {
                    pending.insert(id);
                }
                values.extend(object.values());
            }
            serde_json::Value::Array(array) => values.extend(array),
            _ => {}
        }
    }
    let any = !pending.is_empty();
    for id in pending {
        let (state, _, error) = wait_over(world, id).await;
        assert_eq!(state, "succeeded", "{error:?}");
    }
    any
}

/// A move test that needs complete counts waits for them before preserving its ordinary confirmation assertions.
pub async fn ready_move(
    world: &World,
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    environment: Id,
    request: &orch_api_contract::environment::EnvironmentMovePreflightBody,
    given: Option<&orch_api_contract::forks::ChangeOptions>,
) -> Result<orch_engine::jobs::environment_move::MoveDraft, orch_engine::updates::UpdateError> {
    let draft = world
        .engine()
        .draft_move(conn, principal, environment, request, given)
        .await?;
    if wait_counts(
        world,
        &serde_json::json!({"files":draft.recipe.files,"mounts":draft.mounts}),
    )
    .await
    {
        world
            .engine()
            .draft_move(conn, principal, environment, request, given)
            .await
    } else {
        Ok(draft)
    }
}

/// Directory move tests also review the complete retained count before expecting sizes or confirming.
pub async fn ready_machine_move(
    world: &World,
    principal: &orch_authz::Principal,
    environment: Id,
    request: &orch_api_contract::environment::EnvironmentMovePreflightBody,
    given: Option<&orch_api_contract::forks::ChangeOptions>,
) -> Result<orch_engine::jobs::machine_move::Draft, orch_engine::updates::UpdateError> {
    let draft = world
        .engine()
        .draft_machine_move(principal, environment, request, given)
        .await?;
    if wait_counts(
        world,
        &serde_json::json!({"files":draft.recipe.files,"mounts":draft.mounts}),
    )
    .await
    {
        world
            .engine()
            .draft_machine_move(principal, environment, request, given)
            .await
    } else {
        Ok(draft)
    }
}
