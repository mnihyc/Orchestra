//! `POST /environments` over HTTP makes a container on a real daemon through a real node: the node makes its volumes and its unit, the connector in it enrolls with the token the system made for it and holds a session, the environment is running, and a thread bound to it runs a turn of the scripted agent.
//!
//! The system is composed as the programs compose it: the engine runs and applies what machines say, the
//! node enrolled as a host, the template names a small image the daemon already has, and the connector the
//! unit runs is the static one the lab builds. Everything this makes on the daemon is named by the system
//! after the runtime it made (`orch-environment-…`, `orch-ws-…`), carries the lab's label because the driver
//! puts it on all it makes, and is tracked by the name as soon as the runtime's row says what it is called,
//! so that the lab removes it by that exact name whatever becomes of the test.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "a test fails by panicking"
)]

mod common;

use common::{Lab, NodeUnderTest, ProgramServer, World};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::pages as contract_pages;
use orch_access_contract::permissions::ResourceKind;
use orch_api_contract::binding::{EnvironmentChoice, NewEnvironment};
use orch_api_contract::environment::{
    Environment, EnvironmentCreate, EnvironmentGet, EnvironmentStatus,
};
use orch_api_contract::nodes::{CreateThread, CreateThreadBinding, JobState, ThreadCreate};
use orch_api_contract::threads::{ContentPart, Delivery, ThreadInputCreate, ThreadInputCreateBody};
use orch_api_contract::workspace::{Job, JobGet};
use orch_api_core::hosting::{ApiHostServices, Build, ListedBuilds};
use orch_api_core::middleware::authentication::{ACCESS_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::registry::Registry;
use orch_api_core::routes::resources::sealing::install_master_key;
use orch_api_core::testing::TestApp;
use orch_api_core::thread_engine::EngineApplier;
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::access::{PageKey, SYSTEM_ID, testing as access_testing};
use orch_db::queries::hierarchy::testing::insert_node;
use orch_db::secrets::MasterKey;
use orch_engine::hosting::ToolBuild;
use orch_testkit::clock;
use orch_types::{Id, Time, new_id};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

/// How long the making of an environment may take, with the pull of nothing and the start of a unit.
const MAKING: Duration = Duration::from_secs(180);
/// The model the scripted agent is asked to run.
const MODEL: &str = "fake-1";

/// What the test stands on besides the system and the node.
struct Seeds {
    token: String,
    topic: Id,
    template: Id,
    agent: Id,
}

/// Makes the people and places of the test, as rows: an org with a project and a topic that the person
/// owns, a template of a small image whose harness runs beside its tools, and a scripted agent. Answers what the requests that follow name.
async fn seed(world: &World) -> Seeds {
    let now = world.clock.now();
    let person = world.user;
    let mut conn = world.db.acquire().await.unwrap();
    let (root, project, topic) = (new_id(), new_id(), new_id());
    insert_node(&mut conn, root, "org", None, "Acme", person)
        .await
        .unwrap();
    access_testing::insert_member(&mut conn, root, person, root)
        .await
        .unwrap();
    insert_node(&mut conn, project, "project", Some(root), "Web", person)
        .await
        .unwrap();
    insert_node(&mut conn, topic, "topic", Some(project), "Login", person)
        .await
        .unwrap();
    for node in [root, project, topic] {
        access_testing::name_owner(&mut conn, node, ("user", person), now)
            .await
            .unwrap();
    }
    let page = PageKey::new("system", SYSTEM_ID, "defaults");
    for shipped in contract_pages::shipped() {
        access_testing::add_entry(
            &mut conn,
            &page,
            shipped.profile.as_str(),
            shipped.permission.as_str(),
            ("role", None, Some(shipped.role.as_str())),
            None,
            now,
        )
        .await
        .unwrap();
    }
    // The person signs in.
    let token = new_token(ACCESS_TOKEN_PREFIX);
    orch_db::queries::identity::testing::insert_session(
        &mut conn,
        new_id(),
        person,
        &token_hash(&token),
        now + chrono::Duration::days(1),
    )
    .await
    .unwrap();
    // Nothing is said of the host: its enrollment made its page, and a host of the system is every user's to use.
    // Nor is anything said of who uses what they made: a template, an agent and a provider are their owner's.
    let owned = |kind: ResourceKind, id: Id| ObjectRef {
        kind: kind.into(),
        id,
    };
    // The template: a small image, a harness that runs beside the tools in the unit.
    let template = new_id();
    sqlx::query(
        "INSERT INTO env_templates (id, org_id, home_id, owner_id, name, kind, driver, spec)
         VALUES ($1, $2, $2, $3, 'Small', 'environment', 'docker', $4)",
    )
    .bind(template)
    .bind(root)
    .bind(person)
    .bind(json!({
        "image": common::BUSYBOX,
        "user": { "uid": 1000, "gid": 1000 },
        "network": { "mode": "bridge", "egress": "open", "dns": [] },
        "security": { "read_only_rootfs": false, "no_new_privileges": true, "cap_add": [], "cap_drop": ["NET_RAW"] },
        "mounts": [],
        "resources": { "cpu_millis": 1000, "memory_mb": 256, "pids": 256, "disk_gb": 2 },
        "needs": { "cpu_millis": 100, "memory_mb": 64, "disk_gb": 1 },
        "colocated_harnesses": ["fake"], "host_labels": [], "init": null
    }))
    .execute(&mut *conn)
    .await
    .unwrap();
    start_resource(
        &mut conn,
        owned(ResourceKind::Template, template),
        person,
        NewAccess::Defaults,
        Time::new(now),
    )
    .await
    .unwrap();
    // The scripted agent.
    let agent = new_id();
    sqlx::query(
        "INSERT INTO agents (id, name, owner_id, org_id, home_id, harness_kind, profile, descriptor)
         VALUES ($1, 'Scripted agent', $2, $3, $3, 'fake', '{}', $4)",
    )
    .bind(agent)
    .bind(person)
    .bind(root)
    .bind(json!({
        "kind": "fake", "adapter_version": 1,
        "harness": { "name": "Fake", "version": "1.0.0", "supported_range": ">=1.0.0", "status": "supported" },
        "arrangements": [
            { "type": "together" },
            { "type": "separated", "executor": "orch-exec", "executor_version": "*", "tool_sets": ["plain"] }
        ],
        "native_provider": { "harness_kind": "fake", "title": "Fake subscription", "arrangements": ["separated"] },
        "forwarded_protocols": ["openai-responses"],
        "approval_modes": ["ask", "on_request", "auto"],
        "sandbox_modes": ["read_only", "workspace", "open"],
        "capabilities": {
            "steer": true, "interrupt": true, "approvals": ["command", "file_change", "user_input"],
            "input_images": true, "usage_tokens": true, "memory": "none"
        },
        "parameters": []
    }))
    .execute(&mut *conn)
    .await
    .unwrap();
    start_resource(
        &mut conn,
        owned(ResourceKind::Agent, agent),
        person,
        NewAccess::Defaults,
        Time::new(now),
    )
    .await
    .unwrap();
    Seeds {
        token,
        topic,
        template,
        agent,
    }
}

/// Tracks, by the names its row gives them, every unit and volume a runtime is made of, as soon as there
/// is a row: the lab removes them by those exact names when it is dropped.
fn track_runtimes(world: &World, lab: &Arc<Lab>) -> tokio::task::JoinHandle<()> {
    let (pool, lab) = (world.test_db.pool.clone(), Arc::clone(lab));
    tokio::spawn(async move {
        loop {
            let rows: Vec<(String, serde_json::Value)> =
                sqlx::query_as("SELECT unit_name, volumes FROM runtimes")
                    .fetch_all(&pool)
                    .await
                    .unwrap_or_default();
            for (unit, volumes) in rows {
                let names: Vec<String> = volumes
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter_map(|(_, volume)| volume["name"].as_str().map(str::to_owned))
                    .collect();
                lab.track(&unit, &names);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
}

/// Waits until `check` holds, for as long as an environment may be made in.
async fn eventually<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let began = tokio::time::Instant::now();
    loop {
        if check().await {
            return;
        }
        assert!(began.elapsed() < MAKING, "gave up waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_environment_made_over_http_is_a_container_whose_connector_enrolls_and_a_thread_in_it_runs_a_turn()
 {
    let Some(lab) = Lab::start().await else {
        return;
    };
    let lab = Arc::new(lab);
    let mut world = World::start(&lab.bridge).await;
    let tracker = track_runtimes(&world, &lab);

    // The system as the programs compose it: the engine runs and hears what machines say, and what making
    // a unit asks of the program is answered with the lab's address and the connector the lab built.
    let engine = world.api.app.engine.clone();
    world
        .api
        .link
        .set_applier(Arc::new(EngineApplier::new(engine.clone())));
    install_master_key(MasterKey::new(world.master));
    let program = ProgramServer::serve(&lab.connector, "orch-connector").await;
    let mut config = (*world.config).clone();
    config.gateway.managed_endpoint = format!(
        "wss://gateway.orch.internal:{}",
        world.gateway.pinned.port()
    );
    engine.attach_host_services(Arc::new(ApiHostServices::new(
        world.api.link.certificates().clone(),
        Arc::clone(&world.clock),
        &config,
        Arc::new(ListedBuilds::new(vec![Build {
            os: "linux".to_owned(),
            arch: "x86_64".to_owned(),
            tool: ToolBuild {
                name: "orch-connector".to_owned(),
                version: "0.1.0".to_owned(),
                url: program.url.clone(),
                sha256: program.sha256.clone(),
            },
        }])),
    )));
    engine.start();
    let mut node = NodeUnderTest::start(&world, &lab).await;
    world.wait_online(node.machine_id, true).await;
    // A session is established before the separate machine and host fact streams commit.
    // Placement needs both the driver's actual support and its operator's policy.
    common::eventually(
        "the host's driver and machine policy to be applied",
        || async {
            sqlx::query_scalar::<_, bool>(
                "SELECT COALESCE(h.drivers ? 'docker' AND jsonb_typeof(m.facts->'policy') = 'object', false)
             FROM hosts h JOIN machines m ON m.id = h.machine_id WHERE m.id = $1",
            )
            .bind(node.machine_id)
            .fetch_one(&world.test_db.pool)
            .await
            .unwrap()
        },
    )
    .await;
    let seeds = seed(&world).await;
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let app = TestApp::over(world.api.app.clone(), clock::manual(), registry);

    // The request that makes an environment, over HTTP, answers with the job that makes its unit.
    let made: Environment = app
        .call(EnvironmentCreate, &[])
        .token(&seeds.token)
        .body(&NewEnvironment {
            name: Some("Main".to_owned()),
            home_node_id: Some(seeds.topic),
            template_id: Some(seeds.template),
            ..NewEnvironment::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(made.base.status, EnvironmentStatus::Provisioning);
    let job = made.job.expect("a job makes the unit").id;
    eventually("the job to end", || async {
        let read: Job = app
            .call(JobGet, &[&job])
            .token(&seeds.token)
            .send()
            .await
            .ok();
        match read.state {
            JobState::Succeeded => true,
            JobState::Failed | JobState::Cancelled => {
                panic!("the job did not succeed: {:?}", read.error)
            }
            _ => false,
        }
    })
    .await;

    // A container is on the daemon, its connector enrolled as the runtime's machine and holds a session, and
    // the environment is running with what the connector said it is.
    let (unit, machine): (String, Id) =
        sqlx::query_as("SELECT unit_name, machine_id FROM runtimes WHERE environment_id = $1")
            .bind(made.base.id)
            .fetch_one(&world.test_db.pool)
            .await
            .unwrap();
    assert!(lab.has_container(&unit).await, "no container {unit}");
    world.wait_online(machine, true).await;
    let running: Environment = app
        .call(EnvironmentGet, &[&made.base.id])
        .token(&seeds.token)
        .send()
        .await
        .ok();
    assert_eq!(running.base.status, EnvironmentStatus::Running);
    assert_eq!(running.generation, 1);
    assert_eq!(running.facts["os"], "linux");
    assert_eq!(running.host.as_ref().unwrap().name, lab.name("host"));
    assert!(running.job.is_none());

    // A thread bound to it, with the scripted agent on a provider of the unit's machine, runs a turn.
    let (root, owner) =
        sqlx::query_as::<_, (Id, Id)>("SELECT org_id, owner_id FROM environments WHERE id = $1")
            .bind(made.base.id)
            .fetch_one(&world.test_db.pool)
            .await
            .unwrap();
    let provider = new_id();
    {
        let mut conn = world.db.acquire().await.unwrap();
        sqlx::query(
            "INSERT INTO providers (id, name, owner_id, org_id, home_id, kind, harness_kind, agent_id,
                                    machine_id, arrangements, status)
             VALUES ($1, 'Fake subscription', $2, $3, $3, 'subscription', 'fake', $4, $5,
                     ARRAY['separated'], 'ready')",
        )
        .bind(provider)
        .bind(owner)
        .bind(root)
        .bind(seeds.agent)
        .bind(machine)
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO provider_models (id, provider_id, model_name, display_name, parameters, enabled, source, created_by)
             VALUES ($1, $2, $3, 'Fake One', $4, true, 'manual', $5)",
        )
        .bind(new_id())
        .bind(provider)
        .bind(MODEL)
        .bind(json!({ "efforts": ["low", "high"], "default_effort": "low", "input_images": true }))
        .bind(owner)
        .execute(&mut *conn)
        .await
        .unwrap();
        start_resource(
            &mut conn,
            ObjectRef {
                kind: ResourceKind::Provider.into(),
                id: provider,
            },
            owner,
            NewAccess::Defaults,
            Time::new(world.clock.now()),
        )
        .await
        .unwrap();
    }
    let thread = app
        .call(ThreadCreate, &[&seeds.topic])
        .token(&seeds.token)
        .body(&CreateThread {
            name: Some("In a container".to_owned()),
            binding: Some(CreateThreadBinding {
                agent_id: seeds.agent,
                provider_id: provider,
                arrangement: None,
            }),
            model: Some(MODEL.to_owned()),
            environment: Some(EnvironmentChoice::Use(made.base.id)),
            directory: Some("web".to_owned()),
            ..CreateThread::default()
        })
        .send()
        .await
        .ok();
    let thread = thread.base.id;
    let sent = app
        .call(ThreadInputCreate, &[&thread])
        .token(&seeds.token)
        .body(&ThreadInputCreateBody {
            plan: false,
            content: vec![ContentPart::Text {
                text: "Say it plainly".to_owned(),
            }],
            delivery: Delivery::Queue,
            client_id: "first".to_owned(),
            bypass: None,
            native_action: None,
        })
        .send()
        .await;
    assert_eq!(sent.status, 201);
    eventually("the scripted agent to answer", || async {
        let said: Option<String> = sqlx::query_scalar(
            "SELECT payload ->> 'text' FROM items
              WHERE thread_id = $1 AND kind = 'agent_message' ORDER BY seq_created LIMIT 1",
        )
        .bind(thread)
        .fetch_optional(&world.test_db.pool)
        .await
        .unwrap_or(None);
        said.is_some_and(|text| text == "You said: Say it plainly")
    })
    .await;
    println!(
        "MANAGED MILESTONE: {unit} runs the environment {} and answered a turn",
        made.base.id
    );

    tracker.abort();
    engine.stop();
    node.stop().await;
    world.stop().await;
}
