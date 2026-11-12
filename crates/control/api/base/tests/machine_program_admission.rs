//! Current reported builds and a manager's exact consent release the original queued input.
#![allow(clippy::unwrap_used, reason = "a test fails by panicking")]

mod places;
mod rig;
mod support;

use std::sync::{Arc, Mutex};

use orch_access_contract::{decisions::ObjectRef, permissions::ResourceKind};
use orch_agent_contract::adapter::{AgentAdapter, AgentStart as NativeAgentStart, EventSink};
use orch_agent_contract::facts::AgentProfile;
use orch_agent_contract::loopback::Loopback;
use orch_api_contract::binding::EnvironmentChoice;
use orch_api_contract::environment::{
    Environment, EnvironmentGet, EnvironmentUpdate, EnvironmentUpdateBody,
    MachineEnvironmentCreate, MachineEnvironmentCreateBody,
};
use orch_api_contract::nodes::{CreateThread, CreateThreadBinding, ThreadCreate};
use orch_api_contract::threads::{ContentPart, Delivery, ThreadInputCreate, ThreadInputCreateBody};
use orch_authz::ownership::{NewAccess, start_resource};
use orch_driver_contract::descriptor::{DriverDescriptor, EnvironmentDataStorage};
use orch_link_contract::{
    commands::{AgentStartResult, Command, CommandKind, CommandResult, ExecStartResult, TurnStart},
    events::{EnvironmentFacts, Event, TurnOutcome},
    policy::LocalPolicy,
    session::MachineFacts,
    streams::Stream,
};
use orch_types::{Id, Json, Time};
use places::Who;
use rig::{MODEL, Rig, Tree, eventually};
use serde_json::json;

const KEY: &str = "exec.version_overrides";
const PROGRAM: &str = "/controlled/codex";

fn machine_facts(path: &str, version: &str) -> MachineFacts {
    let mut policy = serde_json::to_value(LocalPolicy::default()).unwrap();
    policy["agent.harnesses"] =
        json!([{"kind":"fake","path":"/test/harness"},{"kind":"codex","path":path}]);
    policy["provider.arrangements"] = json!([
        "separated",
        "key_separated",
        "credential_shared",
        "handed_over"
    ]);
    policy["environment.roots"] = json!(["/workspace"]);
    policy["environment.remote_write"] = json!(true);
    policy["exec.offers"] = json!(["codex-exec-server"]);
    serde_json::from_value(json!({
        "hostname":"controlled","os_version":"synthetic","cpus":1,"memory_bytes":0,
        "harnesses":[{"kind":"fake","path":"/test/harness","version":"1.0.0","daemon":false},
            {"kind":"codex","path":path,"version":version,"daemon":false}],
        "adapters":[{"kind":"fake","adapter_version":1,"builtin":true,"default_home_owned":true}],
        "plugins":[],"policy":policy,"rates":{"up_bps":0,"down_bps":0,"round_trip_ms":0},"clock_offset_ms":0,
        "environment_descriptor":DriverDescriptor::registered_directories("test",EnvironmentDataStorage {
            made_by_orch:None,existing_directory:true,reason:None,
        })
    })).unwrap()
}

fn environment_facts(root: &str, generation: i64, version: &str) -> EnvironmentFacts {
    serde_json::from_value(json!({
        "os":"linux","arch":"x86_64","shell":{"name":"sh","path":"/bin/sh"},
        "user":"agent","home":"/home/agent","workspace_root":root,"path_style":"posix","case_sensitive":true,
        "executors":[{"kind":"codex-exec-server","version":version,"available":true,"reason":null,
            "sandbox_modes":["read_only","workspace","open"]}],
        "mcp_found":[],"index":"full","mounts":[],"private_views":false,"suspend":false,"git":false,"generation":generation
    })).unwrap()
}

struct Floor {
    rig: Rig,
    owner: Who,
    tree: Tree,
    agent: Id,
    provider: Id,
    environment: Id,
    current: Arc<Mutex<(String, String)>>,
}

impl Floor {
    async fn new() -> Self {
        let rig = Rig::new().await;
        let owner = rig.person("ada").await;
        let tree = rig.tree(&owner).await;
        let mut descriptor = rig::descriptor();
        descriptor["arrangements"] =
            json!([{"type":"separated","executor":"codex-exec-server","executor_version":"same"}]);
        rig.machine.handler.on(CommandKind::AgentStart, move |_| {
            Ok(CommandResult::AgentStart(AgentStartResult {
                reused: false,
                harness_version: "1.0.0".into(),
                descriptor: serde_json::from_value(descriptor.clone()).unwrap(),
            }))
        });
        let on = rig.resources(&owner, &tree).await;
        // A newly unreported profile learns through the same native start receipt as production.
        sqlx::query("UPDATE agents SET descriptor='{}' WHERE id=$1")
            .bind(on.agent)
            .execute(rig.db.pool())
            .await
            .unwrap();
        assert!(matches!(
            rig.engine
                .learn_descriptor(on.agent, on.provider, Some(on.environment))
                .await
                .unwrap(),
            orch_engine::provider_setup::Learning::Known
        ));
        let actual: Option<String> = sqlx::query_scalar("SELECT harness_version FROM harnesses WHERE provider_id=$1 AND agent_id=$2 AND machine_id=$3")
            .bind(on.provider).bind(on.agent).bind(rig.machine_id).fetch_one(rig.db.pool()).await.unwrap();
        assert_eq!(actual.as_deref(), Some("1.0.0"));
        let facts = machine_facts(PROGRAM, "1.0.0");
        sqlx::query("UPDATE machines SET owner_id=$2,home_id=n.id,org_id=n.org_id,facts=$3 FROM users u,nodes n WHERE machines.id=$1 AND u.id=$2 AND n.id=u.personal_org_id")
            .bind(rig.machine_id).bind(owner.id()).bind(serde_json::to_value(facts).unwrap()).execute(rig.db.pool()).await.unwrap();
        let mut conn = rig.db.acquire().await.unwrap();
        start_resource(
            &mut conn,
            ObjectRef {
                kind: ResourceKind::Machine.into(),
                id: rig.machine_id,
            },
            owner.id(),
            NewAccess::Defaults,
            Time::new(rig.world.now()),
        )
        .await
        .unwrap();
        drop(conn);
        let current: Arc<Mutex<(String, String)>> =
            Arc::new(Mutex::new((PROGRAM.into(), "1.0.0".into())));
        let versions = Arc::clone(&current);
        rig.machine
            .handler
            .on(CommandKind::EnvAttach, move |command| {
                let Command::EnvAttach(body) = command else {
                    unreachable!()
                };
                let version = versions.lock().unwrap().1.clone();
                Ok(CommandResult::EnvAttach(environment_facts(
                    body.workspace_root.as_deref().unwrap_or("/workspace"),
                    0,
                    &version,
                )))
            });
        let versions = Arc::clone(&current);
        rig.machine
            .handler
            .on(CommandKind::ExecStart, move |command| {
                let Command::ExecStart(body) = command else {
                    unreachable!()
                };
                assert_eq!(body.executor, "codex-exec-server");
                Ok(CommandResult::ExecStart(ExecStartResult {
                    service: "exec/codex-exec-server".into(),
                    version: versions.lock().unwrap().1.clone(),
                    tool_sets: Vec::new(),
                }))
            });
        let environment = rig
            .world
            .app
            .call(MachineEnvironmentCreate, &[&rig.machine_id])
            .token(&owner.token)
            .body(&MachineEnvironmentCreateBody {
                data_sources: None,
                home_id: Some(tree.org.base.id),
                name: "Local program workspace".into(),
                root: "/workspace".into(),
                home_node_id: Some(tree.topic.base.id),
                remote_write: Some(true),
                terminal: None,
                access: None,
            })
            .send()
            .await
            .ok()
            .into_full()
            .unwrap_or_else(|error| {
                panic!("ordinary directory registration retains full native content: {error:?}")
            })
            .base
            .id;
        eventually("the connected native tools to be ready", || async {
            sqlx::query_scalar::<_, String>("SELECT status FROM environments WHERE id=$1")
                .bind(environment)
                .fetch_one(rig.db.pool())
                .await
                .unwrap()
                == "running"
        })
        .await;
        Self {
            rig,
            owner,
            tree,
            agent: on.agent,
            provider: on.provider,
            environment,
            current,
        }
    }

    async fn thread(&self) -> Id {
        self.rig
            .world
            .app
            .call(ThreadCreate, &[&self.tree.topic.base.id])
            .token(&self.owner.token)
            .body(&CreateThread {
                name: Some("Current native builds".into()),
                binding: Some(CreateThreadBinding {
                    agent_id: self.agent,
                    provider_id: self.provider,
                    arrangement: Some(orch_api_contract::binding::ArrangementType::Separated),
                }),
                model: Some(MODEL.into()),
                environment: Some(EnvironmentChoice::Use(self.environment)),
                directory: Some(String::new()),
                prepare_source: Some(false),
                ..Default::default()
            })
            .send()
            .await
            .ok()
            .into_full()
            .unwrap()
            .base
            .id
    }

    async fn report(&self, path: &str, version: &str) {
        *self.current.lock().unwrap() = (path.into(), version.into());
        self.rig
            .machine
            .publish(
                &Stream::Machine,
                &Event::MachineFacts(machine_facts(path, version)),
            )
            .await;
        let generation: i32 = sqlx::query_scalar("SELECT generation FROM environments WHERE id=$1")
            .bind(self.environment)
            .fetch_one(self.rig.db.pool())
            .await
            .unwrap();
        self.rig
            .machine
            .publish(
                &Stream::Env {
                    environment_id: self.environment,
                },
                &Event::EnvFacts(environment_facts(
                    "/workspace",
                    i64::from(generation),
                    version,
                )),
            )
            .await;
        eventually("both current program and executor facts to commit",||async {
            let (program,executor): (Option<String>,Option<String>) = sqlx::query_as(
                "SELECT m.facts->'harnesses'->1->>'version',e.facts->'executors'->0->>'version' FROM environments e JOIN machines m ON m.id=e.machine_id WHERE e.id=$1")
                .bind(self.environment).fetch_one(self.rig.db.pool()).await.unwrap();
            program.as_deref()==Some(version)&&executor.as_deref()==Some(version)
        }).await;
    }

    async fn submit(&self, thread: Id, text: &str, client: &str) -> Id {
        self.rig
            .world
            .app
            .call(ThreadInputCreate, &[&thread])
            .token(&self.owner.token)
            .body(&ThreadInputCreateBody {
                plan: false,
                content: vec![ContentPart::Text { text: text.into() }],
                delivery: Delivery::Queue,
                client_id: client.into(),
                bypass: None,
                native_action: None,
            })
            .send()
            .await
            .ok()
            .into_full()
            .unwrap_or_else(|error| {
                panic!("a browser-session input keeps its full projection: {error:?}")
            })
            .input
            .id
    }

    async fn blocked(&self, thread: Id, version: &str) {
        let expected = format!(
            "Codex on m1 is {version}; this agent runs 1.0.0. An environment manager can allow it on the environment's page."
        );
        eventually("the binding refusal to commit", || async {
            let (state, reason): (String, Option<String>) =
                sqlx::query_as("SELECT state,blocked_reason FROM threads WHERE node_id=$1")
                    .bind(thread)
                    .fetch_one(self.rig.db.pool())
                    .await
                    .unwrap();
            state == "blocked" && reason.as_deref() == Some("binding_invalid")
        })
        .await;
        let detail: Option<String> =
            sqlx::query_scalar("SELECT blocked_detail FROM threads WHERE node_id=$1")
                .bind(thread)
                .fetch_one(self.rig.db.pool())
                .await
                .unwrap();
        assert_eq!(detail.as_deref(), Some(expected.as_str()));
    }

    async fn allow(&self, path: &str, version: &str) -> Environment {
        let current = self
            .rig
            .world
            .app
            .call(EnvironmentGet, &[&self.environment])
            .token(&self.owner.token)
            .send()
            .await
            .ok();
        let mut config = current.config;
        config[KEY] = json!({"codex":{"path":path,"version":version}});
        self.rig
            .world
            .app
            .call(EnvironmentUpdate, &[&self.environment])
            .token(&self.owner.token)
            .body(&EnvironmentUpdateBody {
                data_sources: None,
                name: None,
                config: Some(config),
                remote_write: None,
                terminal: None,
                browser: None,
                vnc: None,
                home_node_id: None,
                expected_revision: current.revision,
            })
            .send()
            .await
            .ok()
            .into_full()
            .unwrap()
    }

    async fn finish(&self, thread: Id, turn: &TurnStart, input: Id) {
        let actual: Id = sqlx::query_scalar("SELECT input_id FROM turns WHERE id=$1")
            .bind(turn.turn_id)
            .fetch_one(self.rig.db.pool())
            .await
            .unwrap();
        assert_eq!(
            actual, input,
            "no fresh input is fabricated to release waiting work"
        );
        self.rig
            .answer(thread, turn, "controlled-reply", "Done")
            .await;
        self.rig
            .say(
                thread,
                rig::turn_ended(turn.turn_id, TurnOutcome::Completed),
            )
            .await;
        self.rig.wait_turn(turn.turn_id, "completed").await;
        self.rig.wait_state(thread, "idle").await;
    }
}

#[tokio::test]
async fn exact_machine_consent_releases_original_words_and_a_same_version_path_change_needs_it_again()
 {
    let floor = Floor::new().await;
    let thread = floor.thread().await;
    // A different cached declaration is not this provider runtime's actual build.
    sqlx::query("UPDATE agents SET descriptor=jsonb_set(descriptor,'{harness,version}','\"9.9.9\"') WHERE id=$1")
        .bind(floor.agent).execute(floor.rig.db.pool()).await.unwrap();
    floor.report(PROGRAM, "2.0.0").await;
    let input = floor
        .submit(thread, "Original mismatched-build work", "mismatch")
        .await;
    floor.blocked(thread, "2.0.0").await;
    assert!(floor.rig.turn_starts(thread).is_empty());
    let allowed = floor.allow(PROGRAM, "2.0.0").await;
    assert_eq!(allowed.config[KEY]["codex"]["by"], json!(floor.owner.id()));
    let first = floor.rig.turn_start(thread, 0).await;
    floor.finish(thread, &first, input).await;
    let replacement = "/controlled/replacement-codex";
    floor.report(replacement, "2.0.0").await;
    let input = floor
        .submit(thread, "Original changed-path work", "new-path")
        .await;
    floor.blocked(thread, "2.0.0").await;
    assert_eq!(floor.rig.turn_starts(thread).len(), 1);
    let shown = floor
        .rig
        .world
        .app
        .call(EnvironmentGet, &[&floor.environment])
        .token(&floor.owner.token)
        .send()
        .await
        .ok();
    assert_eq!(
        serde_json::to_value(shown).unwrap()["exec"]["machine_programs"][0]["approval"],
        Json::Null
    );
    floor.allow(replacement, "2.0.0").await;
    let second = floor.rig.turn_start(thread, 1).await;
    floor.finish(thread, &second, input).await;
    floor.report(replacement, "unknown").await;
    let input = floor
        .submit(thread, "Original unknown-build work", "unknown")
        .await;
    floor.blocked(thread, "unknown").await;
    assert_eq!(floor.rig.turn_starts(thread).len(), 2);
    floor.allow(replacement, "unknown").await;
    let third = floor.rig.turn_start(thread, 2).await;
    floor.finish(thread, &third, input).await;
    floor.report(replacement, "3.0.0").await;
    let input = floor
        .submit(thread, "Original facts-recovery work", "facts-recovery")
        .await;
    floor.blocked(thread, "3.0.0").await;
    assert_eq!(floor.rig.turn_starts(thread).len(), 3);
    // The native report now proves the normal known-equal case. No permission write or second input wakes it.
    floor.report(replacement, "1.0.0").await;
    let fourth = floor.rig.turn_start(thread, 3).await;
    floor.finish(thread, &fourth, input).await;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM thread_inputs WHERE thread_id=$1")
        .bind(thread)
        .fetch_one(floor.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(count, 4);
}

#[tokio::test]
async fn any_agent_consent_cannot_admit_a_runtime_reported_older_than_the_adapter_range() {
    let floor = Floor::new().await;
    let thread = floor.thread().await;
    floor.allow(PROGRAM, "1.0.0").await;
    let mut profile: AgentProfile = serde_json::from_value(
        sqlx::query_scalar::<_, Json>("SELECT profile FROM agents WHERE id=$1")
            .bind(floor.agent)
            .fetch_one(floor.rig.db.pool())
            .await
            .unwrap(),
    )
    .unwrap();
    profile.config["harness_version"] = json!("0.5.0");
    let home = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let mut adapter =
        orch_adapter_fake::FakeAdapter::new(Arc::clone(&floor.rig.world.app.app.clock));
    let (sink, _events) = EventSink::channel();
    let refused = adapter
        .start(
            NativeAgentStart {
                provider_id: floor.provider,
                provider_kind: None,
                agent_id: floor.agent,
                harness_kind: "fake".into(),
                profile: profile.clone(),
                home: Some(home.path().display().to_string()),
                provider_config: json!({}),
                loopback: Loopback::new(41777, orch_plugin_contract::Secret::new("controlled")),
                binary: None,
            },
            sink,
        )
        .await
        .unwrap_err();
    adapter.stop().await.unwrap();
    let failed = orch_connector_core::harnesses::command_error(&refused);
    assert_eq!(failed.code, "harness_unavailable");
    assert!(failed.detail.contains("0.5.0"));
    assert!(
        failed
            .detail
            .contains(orch_adapter_fake::descriptor::SUPPORTED_RANGE)
    );
    let expected_profile = profile.clone();
    let failure = failed.clone();
    let (agent, provider) = (floor.agent, floor.provider);
    floor
        .rig
        .machine
        .handler
        .on(CommandKind::AgentStart, move |command| {
            let Command::AgentStart(body) = command else {
                unreachable!()
            };
            assert_eq!((body.agent_id, body.provider_id), (agent, provider));
            assert_eq!(body.profile, expected_profile);
            Err(failure.clone())
        });
    sqlx::query("UPDATE agents SET profile=$2,descriptor='{}' WHERE id=$1")
        .bind(floor.agent)
        .bind(serde_json::to_value(profile).unwrap())
        .execute(floor.rig.db.pool())
        .await
        .unwrap();
    assert!(matches!(
        floor
            .rig
            .engine
            .learn_descriptor(floor.agent, floor.provider, Some(floor.environment))
            .await
            .unwrap(),
        orch_engine::provider_setup::Learning::Unknown(_)
    ));
    // This fixture keeps immediate commands in its scripted link. Observe the actual failed
    // receipt at the harness's exact start id; refusal retains the last successful build.
    eventually(
        "the actual unsupported native start to be retained",
        || async {
            let (status, version, command): (String, Option<String>, Option<Id>) = sqlx::query_as(
                "SELECT status,harness_version,start_command_id FROM harnesses
             WHERE provider_id=$1 AND agent_id=$2 AND machine_id=$3",
            )
            .bind(floor.provider)
            .bind(floor.agent)
            .bind(floor.rig.machine_id)
            .fetch_one(floor.rig.db.pool())
            .await
            .unwrap();
            let Some(command) = command else {
                return false;
            };
            let info =
                orch_engine::commands::Commands::status(floor.rig.commands.as_ref(), command)
                    .await
                    .unwrap();
            let body = orch_engine::commands::Commands::retained_body(
                floor.rig.commands.as_ref(),
                command,
            )
            .await
            .unwrap();
            let receipt = info.is_some_and(|info| {
                matches!(info.outcome,
            Some(orch_engine::commands::Outcome::Failed(error))
                if error.code == failed.code && error.detail == failed.detail
                    && error.retryable == failed.retryable)
            });
            status == "error"
                && version.as_deref() == Some("1.0.0")
                && receipt
                && body.is_some_and(|body| {
                    body.pointer("/profile/config/harness_version")
                        .and_then(Json::as_str)
                        == Some("0.5.0")
                })
        },
    )
    .await;
    let input = floor
        .submit(thread, "An old runtime is still unsupported", "old-range")
        .await;
    eventually("unsupported runtime binding to remain blocked", || async {
        let (state, reason): (String, Option<String>) =
            sqlx::query_as("SELECT state,blocked_reason FROM threads WHERE node_id=$1")
                .bind(thread)
                .fetch_one(floor.rig.db.pool())
                .await
                .unwrap();
        state == "blocked" && reason.as_deref() == Some("binding_invalid")
    })
    .await;
    assert!(floor.rig.turn_starts(thread).is_empty());
    let retained: (String, Option<Id>, Option<Id>) =
        sqlx::query_as("SELECT state,author_id,turn_id FROM thread_inputs WHERE id=$1")
            .bind(input)
            .fetch_one(floor.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(retained, ("pending".into(), Some(floor.owner.id()), None));
    let current = floor
        .rig
        .world
        .app
        .call(EnvironmentGet, &[&floor.environment])
        .token(&floor.owner.token)
        .send()
        .await
        .ok();
    assert_eq!(current.config[KEY]["codex"]["path"], PROGRAM);
    assert_eq!(current.config[KEY]["codex"]["version"], "1.0.0");
}
