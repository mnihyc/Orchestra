//! A managed provider over HTTP: a subscription made with the job that makes its runtime, signed in by a method of its adapter (checked against the method's fields, the secrets sealed at once and never returned), what a sign-in shows and asks as data, an answer, a sign-in given up, the harness signed out, the runtime started and stopped by hand, and the provider deleted with its runtime retired.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

#[path = "provider_managed/handover_operation_scopes.rs"]
mod handover_operation_scopes;

use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ResourceKind;
use orch_api_contract::access::NewResourceAccess;
use orch_api_contract::binding::{ArrangementType, EnvironmentChoice};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::environment::Placement;
use orch_api_contract::nodes::{CreateThread, CreateThreadBinding, JobState, ThreadCreate};
use orch_api_contract::resources::{
    CreateProvider, LoginProgress, LoginShowKind, LoginState, Provider, ProviderCreate,
    ProviderDelete, ProviderGet, ProviderHandoverReturn, ProviderHandoverReturnBody,
    ProviderLoginCancel, ProviderLoginGet, ProviderLoginInput, ProviderLoginInputBody,
    ProviderLoginStart, ProviderLoginStartBody, ProviderLogout, ProviderStart,
    ProviderStartStopBody, ProviderStatus, ProviderStop, Upstream,
};
use orch_api_contract::threads::Delivery;
use orch_api_contract::workspace::{Job, JobGet};
use orch_api_core::routes::resources::sealing::install_master_key;
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::resources::access_fixtures as fixtures;
use orch_db::queries::resources::hosting as host_rows;
use orch_db::queries::resources::provider_runtime::{self as runtime_rows, NewRuntime};
use orch_db::secrets::MasterKey;
use orch_link_contract::commands::{
    AgentStartResult, Command, CommandError, CommandKind as K, CommandResult, Empty,
    ErrorCode as Wire, ProviderRefreshResult, Sealed, SealedCredential,
};
use orch_link_contract::events::Event;
use orch_link_contract::streams::Stream;
use orch_settings::Target;
use orch_types::{Id, Json, Time, new_id};
use places::missing_permissions;
use rig::{MODEL, Scene, descriptor, eventually, turn_began};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// A token that no request may ever show again once it is given.
const TOKEN: &str = "sk-ant-oat01-do-not-show-0003";
/// The key every test seals under.
const MASTER: [u8; 32] = [7; 32];

/// The usual scene, with the key that provider secrets are sealed under installed.
async fn scene() -> Scene {
    install_master_key(MasterKey::new(MASTER));
    Scene::new().await
}

/// What the tables say of a provider's jobs, for the message of a test that fails.
async fn jobs_dump(s: &Scene, provider: Id) -> String {
    let rows: Vec<(String, String, Option<Json>)> = sqlx::query_as(
        "SELECT kind, state, error FROM jobs WHERE provider_id = $1 ORDER BY created_at",
    )
    .bind(provider)
    .fetch_all(s.rig.db.pool())
    .await
    .unwrap();
    format!("{rows:?}")
}

/// What the tables say of a provider's sign-in, for the message of a test that fails.
async fn login_dump(s: &Scene, provider: Id) -> String {
    let state: Option<Json> = sqlx::query_scalar("SELECT login_state FROM providers WHERE id = $1")
        .bind(provider)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    let cursors: Vec<(String, i64)> =
        sqlx::query_as("SELECT stream, last_seq FROM link_cursors ORDER BY stream")
            .fetch_all(s.rig.db.pool())
            .await
            .unwrap();
    format!("{state:?} {cursors:?}")
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The descriptor of the tests' harness with the ways of signing in it declares: a token to bring, with a
/// flag beside it, and a code to enter.
fn declared() -> Json {
    let mut declared = descriptor();
    declared["native_provider"]["login_methods"] = json!([
        {
            "id": "setup_token", "title": "Bring a token", "summary": "Paste the token.",
            "kind": "import", "yields": "token", "lifetime_secs": 31_536_000,
            "requires": null, "recommended": false,
            "fields": [
                { "name": "token", "label": "Token", "type": "secret", "required": true,
                  "help": null, "choices": null, "default": null, "max_bytes": null },
                { "name": "verified", "label": "Verified", "type": "boolean", "required": false,
                  "help": null, "choices": null, "default": null, "max_bytes": null }
            ]
        },
        {
            "id": "device_code", "title": "Sign in with a code", "summary": "Shows a code.",
            "kind": "interactive", "yields": "login", "lifetime_secs": null,
            "requires": null, "recommended": true, "fields": []
        }
    ]);
    declared
}

/// What the machine that holds the runtime was asked of a sign-in, in order, with the bodies.
type Heard = Arc<Mutex<Vec<(K, Json)>>>;

/// The scene's machine is the runtime of its provider: the harness there answers what a sign-in asks of
/// it, reports where the sign-in stands as a harness does, and keeps what it was asked. A method that
/// imports a credential completes at once; the other shows an address and a code and asks for an answer.
fn the_harness_answers(s: &Scene, provider: Id) -> Heard {
    let heard: Heard = Arc::new(Mutex::new(Vec::new()));
    let machine = Arc::downgrade(&s.rig.machine);
    let runtime = tokio::runtime::Handle::current();
    let expires = (s.rig.world.now() + chrono::Duration::minutes(15)).to_rfc3339();
    let handler = &s.rig.machine.handler;
    handler.on(K::AgentStart, |_: &Command| {
        Ok(CommandResult::AgentStart(AgentStartResult {
            reused: false,
            harness_version: "1.0.0".to_owned(),
            descriptor: serde_json::from_value(declared()).unwrap(),
        }))
    });
    for kind in [
        K::ProviderLoginStart,
        K::ProviderLoginInput,
        K::ProviderLoginCancel,
        K::ProviderLogout,
        K::ProviderRefresh,
    ] {
        let heard = Arc::clone(&heard);
        let machine = machine.clone();
        let runtime = runtime.clone();
        let expires = expires.clone();
        handler.on(kind, move |command: &Command| {
            lock(&heard).push((kind, command.body().unwrap()));
            let report = |state: Json| {
                if let Some(machine) = machine.upgrade() {
                    runtime.spawn(async move {
                        machine
                            .publish(
                                &Stream::Provider {
                                    provider_id: provider,
                                },
                                &Event::ProviderLogin(serde_json::from_value(state).unwrap()),
                            )
                            .await;
                    });
                }
            };
            Ok(match command {
                Command::ProviderLoginStart(start) => {
                    if start.method == "setup_token" {
                        report(json!({
                            "state": "completed", "method": "setup_token", "shows": [],
                            "asks": null, "expires_at": null, "error": null
                        }));
                    } else {
                        report(json!({
                            "state": "awaiting_user", "method": "device_code",
                            "shows": [
                                { "kind": "uri", "label": "Open this address",
                                  "value": "https://vendor.invalid/device" },
                                { "kind": "code", "label": "Enter this code there", "value": "ABCD-1234" }
                            ],
                            "asks": [{ "name": "answer", "label": "What the page shows", "type": "text",
                                       "required": true, "help": null, "choices": null,
                                       "default": null, "max_bytes": null }],
                            "expires_at": expires, "error": null
                        }));
                    }
                    CommandResult::ProviderLoginStart(Empty {})
                }
                Command::ProviderLoginInput(_) => {
                    report(json!({
                        "state": "completed", "method": "device_code", "shows": [],
                        "asks": null, "expires_at": null, "error": null
                    }));
                    CommandResult::ProviderLoginInput(Empty {})
                }
                Command::ProviderLoginCancel(_) => {
                    report(json!({
                        "state": "idle", "method": null, "shows": [], "asks": null,
                        "expires_at": null, "error": null
                    }));
                    CommandResult::ProviderLoginCancel(Empty {})
                }
                Command::ProviderLogout(_) => CommandResult::ProviderLogout(Empty {}),
                Command::ProviderRefresh(_) => CommandResult::ProviderRefresh(ProviderRefreshResult {
                    source:None,
                    status: "ready".to_owned(),
                    facts: serde_json::from_value(json!({ "plan": "team", "windows": [] })).unwrap(),
                }),
                other => {
                    return Err(CommandError::new(
                        Wire::Unsupported,
                        format!("no script for {}", other.kind()),
                    ));
                }
            })
        });
    }
    heard
}

fn asked(heard: &Heard, kind: K) -> Vec<Json> {
    lock(heard)
        .iter()
        .filter(|(each, _)| *each == kind)
        .map(|(_, body)| body.clone())
        .collect()
}

/// The tables of the database whose rows, written out as text, hold `text` or its bytes.
async fn tables_holding(s: &Scene, text: &str) -> Vec<String> {
    let pool = s.rig.db.pool();
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT table_name::text FROM information_schema.tables
          WHERE table_schema = 'public' AND table_type = 'BASE TABLE' ORDER BY 1",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    let hex: String = text.bytes().map(|byte| format!("{byte:02x}")).collect();
    let mut found = Vec::new();
    for table in tables {
        let sql = format!(
            "SELECT count(*) FROM \"{table}\" t WHERE t::text LIKE '%' || $1 || '%' OR t::text LIKE '%' || $2 || '%'"
        );
        let hits: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
            .bind(text)
            .bind(&hex)
            .fetch_one(pool)
            .await
            .unwrap();
        if hits > 0 {
            found.push(table);
        }
    }
    found
}

/// How many rows hold what was given for a sign-in, sealed, for the provider.
async fn sealed_rows(s: &Scene, provider: Id) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM secrets WHERE owner_kind = 'provider' AND owner_id = $1
            AND starts_with(name, 'login:')",
    )
    .bind(provider)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap()
}

/// Whether a sign-in waits for the provider's runtime.
async fn waits(s: &Scene, provider: Id) -> bool {
    let kept: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT login_pending FROM providers WHERE id = $1")
            .bind(provider)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    kept.is_some()
}

/// A host of the system that the scene's owner may use.
async fn a_host(s: &Scene, name: &str) -> Id {
    let world = &s.rig.world;
    let mut conn = world.conn().await;
    let id = fixtures::insert_resource(
        &mut conn,
        "host",
        Some(s.ada.id()),
        Some(s.tree.org.base.id),
        name,
    )
    .await
    .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Host.into(),
            id,
        },
        s.ada.id(),
        NewAccess::Defaults,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE hosts SET drivers = $2, capacity = $3 WHERE id = $1")
        .bind(id)
        .bind(json!({ "docker": {
            "name": "docker", "title": "Docker",
            "spec_schemas": { "environment": {}, "provider": {} },
            "runtime_classes": ["runc"], "sources": ["image", "build", "archive"]
        }}))
        .bind(json!({ "cpus": 8, "memory_mb": 16_384, "disk_gb": 500 }))
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE machines SET online = true WHERE id = (SELECT machine_id FROM hosts WHERE id = $1)",
    )
    .bind(id)
    .execute(&mut *conn)
    .await
    .unwrap();
    id
}

/// A template of providers' runtimes that the scene's owner uses.
async fn a_template(s: &Scene) -> Id {
    let world = &s.rig.world;
    let mut conn = world.conn().await;
    let id = fixtures::insert_resource(
        &mut conn,
        "template",
        Some(s.ada.id()),
        Some(s.tree.org.base.id),
        "Provider runtime",
    )
    .await
    .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Template.into(),
            id,
        },
        s.ada.id(),
        NewAccess::Defaults,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE env_templates SET kind = 'provider', driver = 'docker', spec = $2 WHERE id = $1",
    )
    .bind(id)
    .bind(json!({
        "image": "busybox:latest", "runtime": "runc",
        "user": { "name": "nonroot", "uid": 65532, "gid": 65532 },
        "resources": { "cpu_millis": 2000, "memory_mb": 2048, "pids": 512 },
        "host_labels": []
    }))
    .execute(&mut *conn)
    .await
    .unwrap();
    id
}

/// The scene's provider, made a managed one whose runtime is up on the scene's machine, with the
/// profile's harness declaring its ways of signing in.
async fn managed_and_up(s: &Scene) -> Id {
    let host = a_host(s, "Build box").await;
    let template = a_template(s).await;
    let provider = s.on.provider;
    let runtime = new_id();
    let now = s.rig.world.now();
    let mut conn = s.rig.db.acquire().await.unwrap();
    sqlx::query("UPDATE agents SET descriptor = $2 WHERE id = $1")
        .bind(s.on.agent)
        .bind(declared())
        .execute(&mut *conn)
        .await
        .unwrap();
    runtime_rows::insert_runtime(
        &mut conn,
        &NewRuntime {
            id: runtime,
            host_id: host,
            unit_name: &format!("orch-provider-{}", &runtime.simple().to_string()[20..]),
            provider_id: provider,
            driver: "docker",
            template_id: template,
            spec_hash: &[0; 32],
            generation: 1,
            volumes: &json!({ "state": { "name": format!("orch-state-{provider}"), "storage": null } }),
        },
        now,
    )
    .await
    .unwrap();
    host_rows::adopt_machine(&mut conn, s.rig.machine_id, runtime, now)
        .await
        .unwrap();
    sqlx::query("UPDATE runtimes SET state = 'running' WHERE id = $1")
        .bind(runtime)
        .execute(&mut *conn)
        .await
        .unwrap();
    provider
}

/// Its unit stopped: the provider has no machine, and nothing is there to be told.
async fn stopped(s: &Scene, provider: Id) {
    sqlx::query("UPDATE runtimes SET state = 'stopped' WHERE provider_id = $1")
        .bind(provider)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE providers SET machine_id = NULL WHERE id = $1")
        .bind(provider)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
}

fn subscription(s: &Scene, name: &str) -> CreateProvider {
    CreateProvider::Subscription {
        runtime_machine_id: None,
        key_machine_id: None,
        name: name.to_owned(),
        home_id: s.tree.org.base.id,
        access: None,
        arrangements: None,
        runtime_config: None,
        agent_id: s.on.agent,
        config: None,
        sign_in: None,
        sources: None,
    }
}

fn on_a_key(s: &Scene, name: &str) -> CreateProvider {
    CreateProvider::Api {
        runtime_machine_id: None,
        name: name.to_owned(),
        home_id: s.tree.org.base.id,
        access: None,
        arrangements: None,
        runtime_config: None,
        upstreams: vec![Upstream {
            protocol: "openai-responses".to_owned(),
            base_url: "https://api.example.com/v1".to_owned(),
            options: json!({}),
        }],
        api_key: Some("sk-live-key".to_owned()),
        secrets: None,
        key_machine_id: None,
        sources: None,
    }
}

async fn get(s: &Scene, provider: Id) -> Provider {
    s.rig
        .world
        .app
        .call(ProviderGet, &[&provider])
        .token(&s.ada.token)
        .send()
        .await
        .ok()
}

async fn login(s: &Scene, provider: Id) -> LoginState {
    s.rig
        .world
        .app
        .call(ProviderLoginGet, &[&provider])
        .token(&s.ada.token)
        .send()
        .await
        .ok()
}

async fn sign_in(
    s: &Scene,
    provider: Id,
    token: &str,
    body: &ProviderLoginStartBody,
) -> orch_api_core::testing::Reply<LoginState> {
    s.rig
        .world
        .app
        .call(ProviderLoginStart, &[&provider])
        .token(token)
        .body(body)
        .send()
        .await
}

fn by(method: &str, pairs: &[(&str, &str)]) -> ProviderLoginStartBody {
    ProviderLoginStartBody {
        method: method.to_owned(),
        values: Some(
            pairs
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect::<BTreeMap<_, _>>(),
        ),
    }
}

async fn eventually_idle(s: &Scene, provider: Id) {
    eventually("the sign-in to end", || async move {
        login(s, provider).await.state == LoginProgress::Idle
    })
    .await;
}

// ---------------------------------------------------------------- making one

/// The members a refusal names as what is wrong.
fn fault_paths(problem: &orch_api_contract::common::Problem) -> Vec<String> {
    problem
        .extra
        .get("errors")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|each| each["path"].as_str().map(str::to_owned))
        .collect()
}

#[tokio::test]
async fn a_directory_provider_uses_its_home_and_keepers_policy_for_sign_in_input_and_logout() {
    for (owned, allowed) in [(true, false), (false, false), (false, true)] {
        let s = scene().await;
        let provider = s.on.provider;
        sqlx::query("UPDATE agents SET descriptor=$2 WHERE id=$1")
            .bind(s.on.agent)
            .bind(declared())
            .execute(s.rig.db.pool())
            .await
            .unwrap();
        let home = if owned {
            json!({"kind":"made_by_orch"})
        } else {
            json!({"kind":"existing_directory","path":"/allowed/native-home"})
        };
        sqlx::query("UPDATE providers SET runtime_config=$2 WHERE id=$1")
            .bind(provider)
            .bind(json!({"runtime.home":home}))
            .execute(s.rig.db.pool())
            .await
            .unwrap();
        sqlx::query("UPDATE machines SET facts=facts || $2 WHERE id=$1").bind(s.rig.machine_id)
            .bind(json!({"environment_descriptor":{"lifecycle":"attach"},"policy":{"provider.signin":allowed}}))
            .execute(s.rig.db.pool()).await.unwrap();
        let heard = the_harness_answers(&s, provider);
        let result = sign_in(&s, provider, &s.ada.token, &by("device_code", &[])).await;
        if !owned && !allowed {
            let problem = result.problem();
            assert_eq!(problem.code, ErrorCode::PlanBlocked);
            // A detail is its code's catalogue sentence; one that names a policy key stays in the log.
            assert_eq!(
                problem.detail,
                orch_api_contract::messages::for_code(ErrorCode::PlanBlocked)
            );
            assert!(
                !lock(&heard)
                    .iter()
                    .any(|(kind, _)| *kind == K::ProviderLoginStart)
            );
            assert_eq!(
                login(&s, provider).await.state,
                LoginProgress::Idle,
                "read-only state remains available"
            );
            continue;
        }
        assert_eq!(result.ok().state, LoginProgress::AwaitingUser);
        assert!(
            lock(&heard)
                .iter()
                .any(|(kind, _)| *kind == K::ProviderLoginStart)
        );
        if !owned {
            sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{policy,provider.signin}','false') WHERE id=$1")
                .bind(s.rig.machine_id).execute(s.rig.db.pool()).await.unwrap();
            let problem = s
                .rig
                .world
                .app
                .call(ProviderLoginInput, &[&provider])
                .token(&s.ada.token)
                .body(&ProviderLoginInputBody {
                    values: BTreeMap::from([("answer".into(), "1234".into())]),
                })
                .send()
                .await
                .problem();
            assert_eq!(problem.code, ErrorCode::PlanBlocked);
            let problem = s
                .rig
                .world
                .app
                .call(ProviderLogout, &[&provider])
                .token(&s.ada.token)
                .send()
                .await
                .problem();
            assert_eq!(problem.code, ErrorCode::PlanBlocked);
            assert!(
                !lock(&heard)
                    .iter()
                    .any(|(kind, _)| *kind == K::ProviderLoginInput || *kind == K::ProviderLogout)
            );
            s.rig
                .world
                .app
                .call(ProviderLoginCancel, &[&provider])
                .token(&s.ada.token)
                .send()
                .await
                .ok();
        }
    }
}

#[tokio::test]
async fn a_person_adds_a_subscription_and_is_given_the_job_that_makes_its_runtime() {
    let s = scene().await;
    let made = s
        .rig
        .world
        .app
        .call(ProviderCreate, &[])
        .token(&s.ada.token)
        .body(&subscription(&s, "Team Codex"))
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
        .provider;
    assert_eq!(made.base.name, "Team Codex");
    assert_eq!(made.base.harness_kind.as_deref(), Some("fake"));
    assert_eq!(made.base.placement, Placement::Managed);
    assert_eq!(made.base.status, ProviderStatus::NeedsSetup);
    assert_eq!(made.owner.username, "ada");
    assert_eq!(made.agent.as_ref().map(|agent| agent.id), Some(s.on.agent));
    let allowed: Vec<ArrangementType> = made
        .arrangements
        .iter()
        .filter(|each| each.allowed)
        .map(|each| each.kind)
        .collect();
    assert_eq!(
        allowed,
        [ArrangementType::Separated],
        "what the profile's adapter proposes"
    );
    let runtime = made.runtime.expect("a managed provider shows its runtime");
    assert_eq!(runtime.state, "creating");
    let job = runtime.job.expect("and the job that makes it");
    assert_eq!(job.kind, "provider_provision");
    assert!(
        matches!(job.state, JobState::Queued | JobState::Running),
        "the creation reply keeps the runtime job even if the engine advances: {:?}",
        job.state
    );
    // The job is the provider's: its owner reads it, with the phases of making a runtime.
    let read: Job = s
        .rig
        .world
        .app
        .call(JobGet, &[&job.id])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(read.provider_id, Some(made.base.id));
    assert_eq!(
        read.progress
            .phases
            .iter()
            .map(|phase| phase.name.as_str())
            .collect::<Vec<_>>(),
        // What the template asks to run once the unit is up is a phase of its own.
        [
            "schedule", "image", "volumes", "tools", "create", "enroll", "after_up", "verify",
            "commit"
        ]
    );
    // Nobody who does not manage the provider reads it.
    let other = s
        .rig
        .world
        .app
        .call(JobGet, &[&job.id])
        .token(&s.bea.token)
        .send()
        .await;
    assert!(!other.status.is_success());
    // A provider on a key made here has a runtime of its own as well.
    let api = s
        .rig
        .world
        .app
        .call(ProviderCreate, &[])
        .token(&s.ada.token)
        .body(&on_a_key(&s, "Key"))
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
        .provider;
    let runtime = api.runtime.expect("a provider on a key has a runtime too");
    assert_eq!(
        runtime.job.map(|job| job.kind).as_deref(),
        Some("provider_provision")
    );
}

#[tokio::test]
async fn a_creation_reply_keeps_its_runtime_job_even_when_the_engine_fails_before_making_a_unit() {
    let s = scene().await;
    sqlx::query("UPDATE env_templates SET enabled = false WHERE kind = 'provider'")
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let made = s
        .rig
        .world
        .app
        .call(ProviderCreate, &[])
        .token(&s.ada.token)
        .body(&subscription(&s, "Cannot place yet"))
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
        .provider;
    let runtime = made
        .runtime
        .as_ref()
        .expect("the creation snapshot has its runtime");
    assert_eq!(runtime.state, "creating");
    let job = runtime
        .job
        .as_ref()
        .expect("the creation snapshot has its job");
    let job_id = job.id;
    assert_eq!(job.state, JobState::Queued);
    eventually("the runtime job to fail before creating a unit", || async {
        let state: String = sqlx::query_scalar("SELECT state FROM jobs WHERE id = $1")
            .bind(job_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
        state == "failed"
    })
    .await;
    let units: i64 = sqlx::query_scalar("SELECT count(*) FROM runtimes WHERE provider_id = $1")
        .bind(made.base.id)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(units, 0);
    assert_eq!(
        made.runtime.as_ref().unwrap().job.as_ref().unwrap().id,
        job_id
    );
    let read: Job = s
        .rig
        .world
        .app
        .call(JobGet, &[&job_id])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(read.provider_id, Some(made.base.id));
    assert_eq!(read.state, JobState::Failed);
}

#[tokio::test]
async fn a_subscription_needs_use_of_the_profile_and_a_harness_with_a_sign_in_of_its_own() {
    let s = scene().await;
    // Somebody who may not use the profile is told what is missing.
    let refused = s
        .rig
        .world
        .app
        .call(ProviderCreate, &[])
        .token(&s.dan.token)
        .body(&subscription(&s, "Dan's"))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(missing_permissions(&refused), ["agent.use"]);
    let none: i64 =
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = 'provider_provision'")
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(none, 0, "nothing was made for the refused");
    // A harness that says it has no sign-in of its own is no subscription.
    let mut without = descriptor();
    without["native_provider"] = Json::Null;
    sqlx::query("UPDATE agents SET descriptor = $2 WHERE id = $1")
        .bind(s.on.agent)
        .bind(without)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let invalid = s
        .rig
        .world
        .app
        .call(ProviderCreate, &[])
        .token(&s.ada.token)
        .body(&subscription(&s, "None"))
        .send()
        .await
        .problem();
    assert_eq!(invalid.code, ErrorCode::RequestInvalid);
    assert!(
        fault_paths(&invalid).contains(&"agent_id".to_owned()),
        "{invalid:?}"
    );
}

#[tokio::test]
async fn a_home_may_accept_only_some_harnesses_and_keep_a_subscription_from_being_shared() {
    let s = scene().await;
    let org = s.tree.org.base.id;
    let world = &s.rig.world;
    world
        .set_at(
            Target::Node(org),
            "providers.subscription_sharing",
            json!(["codex"]),
        )
        .await;
    let mut shared = subscription(&s, "Shared");
    if let CreateProvider::Subscription { access, .. } = &mut shared {
        *access = Some(NewResourceAccess::Home);
    }
    let refused = world
        .app
        .call(ProviderCreate, &[])
        .token(&s.ada.token)
        .body(&shared)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::ProviderSubscriptionNotShared);
    // The same subscription kept to its owner is made.
    let mut mine = subscription(&s, "Mine");
    if let CreateProvider::Subscription { access, .. } = &mut mine {
        *access = Some(NewResourceAccess::OnlyMe);
    }
    let made = world
        .app
        .call(ProviderCreate, &[])
        .token(&s.ada.token)
        .body(&mine)
        .send()
        .await;
    assert!(made.status.is_success(), "{:?}", made.body);
    world
        .set_at(
            Target::Node(org),
            "providers.subscription_harnesses",
            json!(["codex"]),
        )
        .await;
    let refused = world
        .app
        .call(ProviderCreate, &[])
        .token(&s.ada.token)
        .body(&subscription(&s, "Not accepted"))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::ProviderSubscriptionNotAllowed);
}

// ---------------------------------------------------------------- signing in

#[tokio::test]
async fn a_script_is_told_to_import_a_credential_instead_of_beginning_an_interactive_sign_in() {
    let s = scene().await;
    let provider = managed_and_up(&s).await;
    let heard = the_harness_answers(&s, provider);
    let token = orch_api_core::middleware::authentication::new_token(
        orch_api_core::middleware::authentication::API_TOKEN_PREFIX,
    );
    let hash = orch_api_core::middleware::authentication::token_hash(&token);
    let mut conn = s.rig.db.acquire().await.unwrap();
    orch_db::queries::identity::testing::insert_api_token(&mut conn, s.ada.id(), &hash)
        .await
        .unwrap();
    sqlx::query("UPDATE api_tokens SET scopes = ARRAY['provider.manage'] WHERE token_hash = $1")
        .bind(hash)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let problem = sign_in(&s, provider, &token, &by("device_code", &[]))
        .await
        .problem();
    assert_eq!(problem.code, ErrorCode::RequestInvalid);
    assert_eq!(problem.extra["errors"][0]["path"], "method");
    assert!(
        problem.extra["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("imports a credential")
    );
    assert!(asked(&heard, K::ProviderLoginStart).is_empty());
    let pending: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT login_pending FROM providers WHERE id = $1")
            .bind(provider)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert!(pending.is_none());
    let imported = sign_in(
        &s,
        provider,
        &token,
        &by("setup_token", &[("token", TOKEN)]),
    )
    .await
    .ok();
    assert_eq!(imported.state, LoginProgress::Completed);
    assert_eq!(asked(&heard, K::ProviderLoginStart).len(), 1);
}

#[tokio::test]
async fn a_sign_in_is_checked_against_the_fields_of_its_method_and_a_secret_is_never_returned() {
    let s = scene().await;
    let provider = managed_and_up(&s).await;
    let heard = the_harness_answers(&s, provider);
    // A method the harness has not is named.
    let no_method = sign_in(&s, provider, &s.ada.token, &by("carrier_pigeon", &[]))
        .await
        .problem();
    assert_eq!(no_method.code, ErrorCode::RequestInvalid);
    assert!(
        fault_paths(&no_method).contains(&"method".to_owned()),
        "{no_method:?}"
    );
    // What does not fit its field is named by the field, every fault at once.
    let bad = sign_in(
        &s,
        provider,
        &s.ada.token,
        &by("setup_token", &[("verified", "perhaps"), ("nonsense", "1")]),
    )
    .await
    .problem();
    assert_eq!(bad.code, ErrorCode::RequestInvalid);
    let paths: Vec<String> = bad.extra["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|each| each["path"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        paths.contains(&"values.verified".to_owned())
            && paths.contains(&"values.nonsense".to_owned()),
        "{paths:?}"
    );
    // What a method requires is asked for.
    let missing = sign_in(
        &s,
        provider,
        &s.ada.token,
        &by("setup_token", &[("verified", "true")]),
    )
    .await
    .problem();
    assert_eq!(missing.code, ErrorCode::RequestInvalid);
    assert!(
        fault_paths(&missing).contains(&"values.token".to_owned()),
        "{missing:?}"
    );
    assert!(
        asked(&heard, K::ProviderLoginStart).is_empty(),
        "nothing was sent for a request that did not fit"
    );
    // Those who may only use the provider cannot sign it in.
    let refused = sign_in(
        &s,
        provider,
        &s.eve.token,
        &by("setup_token", &[("token", TOKEN)]),
    )
    .await
    .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(missing_permissions(&refused), ["provider.manage"]);
    assert!(asked(&heard, K::ProviderLoginStart).is_empty());
    // The one that fits is given to the harness, and the answer is the provider signed in.
    let reply = sign_in(
        &s,
        provider,
        &s.ada.token,
        &by("setup_token", &[("token", TOKEN), ("verified", "true")]),
    )
    .await;
    let raw = String::from_utf8_lossy(&reply.raw).into_owned();
    assert!(!raw.contains(TOKEN), "{raw}");
    let state = reply.ok();
    assert_eq!(state.state, LoginProgress::Completed, "{state:?}");
    assert_eq!(state.method.as_deref(), Some("setup_token"));
    let told = asked(&heard, K::ProviderLoginStart);
    assert_eq!(told.len(), 1);
    assert!(
        told[0]["values"]["token"]["$secret"].is_string(),
        "the secret goes as a reference to its row: {}",
        told[0]
    );
    assert_eq!(told[0]["values"]["verified"], "true");
    assert!(!told[0].to_string().contains(TOKEN));
    // Nothing a request returns has it, and no table does.
    assert!(
        !serde_json::to_string(&get(&s, provider).await)
            .unwrap()
            .contains(TOKEN)
    );
    let again = s
        .rig
        .world
        .app
        .call(ProviderLoginGet, &[&provider])
        .token(&s.ada.token)
        .send()
        .await;
    assert!(!String::from_utf8_lossy(&again.raw).contains(TOKEN));
    assert!(tables_holding(&s, TOKEN).await.is_empty());
    assert_eq!(
        sealed_rows(&s, provider).await,
        0,
        "the harness has it, and the row it was sealed in is gone"
    );
    assert!(!waits(&s, provider).await);
    // The audit row says who signed it in and by which method, and none of what was given.
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT summary || ' ' || details::text FROM audit_log WHERE action = 'provider.login'",
    )
    .fetch_all(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].contains("setup_token"), "{rows:?}");
    assert!(!rows[0].contains(TOKEN));
}

#[tokio::test]
async fn a_sign_in_that_leads_the_person_shows_and_asks_as_data_is_answered_and_can_be_given_up() {
    let s = scene().await;
    let provider = managed_and_up(&s).await;
    let heard = the_harness_answers(&s, provider);
    // Nothing is running at first.
    assert_eq!(login(&s, provider).await.state, LoginProgress::Idle);
    let state = sign_in(&s, provider, &s.ada.token, &by("device_code", &[]))
        .await
        .ok();
    assert_eq!(
        state.state,
        LoginProgress::AwaitingUser,
        "{state:?} {}",
        login_dump(&s, provider).await
    );
    assert_eq!(state.method.as_deref(), Some("device_code"));
    assert_eq!(state.shows.len(), 2);
    assert_eq!(state.shows[0].kind, LoginShowKind::Uri);
    assert_eq!(state.shows[1].kind, LoginShowKind::Code);
    assert_eq!(state.shows[1].value, "ABCD-1234");
    assert_eq!(state.asks.as_ref().unwrap()[0].name, "answer");
    assert!(state.expires_at.is_some());
    // Somebody who cannot manage the provider does not see it.
    let eve = s
        .rig
        .world
        .app
        .call(ProviderLoginGet, &[&provider])
        .token(&s.eve.token)
        .send()
        .await
        .problem();
    assert_eq!(eve.code, ErrorCode::AuthzDenied);
    // An answer that is not what was asked for is refused naming the field, and nothing is sent.
    let input = |who: &str, pairs: &[(&str, &str)]| {
        let call = s
            .rig
            .world
            .app
            .call(ProviderLoginInput, &[&provider])
            .token(who);
        let body = ProviderLoginInputBody {
            values: pairs
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect(),
        };
        async move { call.body(&body).send().await }
    };
    let wrong = input(&s.ada.token, &[("nonsense", "x")]).await.problem();
    assert_eq!(wrong.code, ErrorCode::RequestInvalid);
    assert!(
        fault_paths(&wrong).contains(&"values.nonsense".to_owned()),
        "{wrong:?}"
    );
    assert_eq!(
        input(&s.eve.token, &[("answer", "x")]).await.problem().code,
        ErrorCode::AuthzDenied
    );
    assert!(asked(&heard, K::ProviderLoginInput).is_empty());
    // The answer goes to the harness, which completes the sign-in.
    let done = input(&s.ada.token, &[("answer", "the text the page showed")])
        .await
        .ok();
    assert_eq!(done.state, LoginProgress::Completed, "{done:?}");
    let sent = asked(&heard, K::ProviderLoginInput);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["values"]["answer"], "the text the page showed");
    // Nothing asks for an answer now.
    let nothing = input(&s.ada.token, &[("answer", "again")]).await.problem();
    assert_eq!(nothing.code, ErrorCode::PlanBlocked);
    // Another sign-in, given up: the harness is told, and none is running after.
    let again = sign_in(&s, provider, &s.ada.token, &by("device_code", &[]))
        .await
        .ok();
    assert_eq!(again.state, LoginProgress::AwaitingUser);
    let refused = s
        .rig
        .world
        .app
        .call(ProviderLoginCancel, &[&provider])
        .token(&s.eve.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert!(asked(&heard, K::ProviderLoginCancel).is_empty());
    s.rig
        .world
        .app
        .call(ProviderLoginCancel, &[&provider])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(asked(&heard, K::ProviderLoginCancel).len(), 1);
    eventually_idle(&s, provider).await;
}

#[tokio::test]
async fn a_sign_in_waits_for_its_runtime_sealed_and_is_forgotten_with_its_secret_when_the_runtime_cannot_be_made()
 {
    let s = scene().await;
    let provider = managed_and_up(&s).await;
    let heard = the_harness_answers(&s, provider);
    stopped(&s, provider).await;
    // The unit is started by the sign-in. The host of this scene cannot start one, so the job fails and
    // the sign-in that waited for it goes with it.
    let state = sign_in(
        &s,
        provider,
        &s.ada.token,
        &by("setup_token", &[("token", TOKEN)]),
    )
    .await
    .ok();
    assert!(
        matches!(state.state, LoginProgress::Starting | LoginProgress::Failed),
        "{state:?}"
    );
    assert_eq!(state.method.as_deref(), Some("setup_token"));
    assert!(
        tables_holding(&s, TOKEN).await.is_empty(),
        "what waits for a runtime is sealed: the token is in no table"
    );
    eventually("the sign-in to be given up with its runtime", || async {
        login(&s, provider).await.state == LoginProgress::Failed
    })
    .await;
    assert!(!waits(&s, provider).await);
    assert_eq!(sealed_rows(&s, provider).await, 0);
    let after = login(&s, provider).await;
    assert!(
        after.error.unwrap_or_default().contains("runtime"),
        "it says why: the runtime could not be made"
    );
    assert!(
        asked(&heard, K::ProviderLoginStart).is_empty(),
        "the harness was never told of it"
    );
    assert!(tables_holding(&s, TOKEN).await.is_empty());
    // Given up by the person instead, it is forgotten just the same.
    let again = sign_in(
        &s,
        provider,
        &s.ada.token,
        &by("setup_token", &[("token", TOKEN)]),
    )
    .await;
    let _ = again;
    let cancelled = s
        .rig
        .world
        .app
        .call(ProviderLoginCancel, &[&provider])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(cancelled.state, LoginProgress::Idle);
    assert_eq!(sealed_rows(&s, provider).await, 0);
    assert!(!waits(&s, provider).await);
}

#[tokio::test]
async fn a_provider_is_signed_out_by_asking_its_harness_and_a_provider_on_a_key_has_no_sign_in() {
    let s = scene().await;
    // A provider on a person's machine whose harness keeps its own home there is signed in there: Orch
    // signs in only a home it made, or one whose keeper allows `provider.signin`, and refuses before
    // anything is kept.
    sqlx::query(
        "UPDATE machines SET facts = jsonb_set(facts, '{adapters,0,default_home_owned}', 'false') WHERE id = $1",
    )
    .bind(s.rig.machine_id)
    .execute(s.rig.db.pool())
    .await
    .unwrap();
    let connected = sign_in(
        &s,
        s.on.provider,
        &s.ada.token,
        &by("setup_token", &[("token", TOKEN)]),
    )
    .await
    .problem();
    assert_eq!(connected.code, ErrorCode::PlanBlocked);
    assert!(tables_holding(&s, TOKEN).await.is_empty());
    let provider = managed_and_up(&s).await;
    let heard = the_harness_answers(&s, provider);
    // Whoever only uses the provider does not sign it out.
    let refused = s
        .rig
        .world
        .app
        .call(ProviderLogout, &[&provider])
        .token(&s.eve.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(missing_permissions(&refused), ["provider.manage"]);
    assert!(asked(&heard, K::ProviderLogout).is_empty());
    let out = s
        .rig
        .world
        .app
        .call(ProviderLogout, &[&provider])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(out.base.id, provider);
    assert_eq!(asked(&heard, K::ProviderLogout).len(), 1);
    assert_eq!(login(&s, provider).await.state, LoginProgress::Idle);
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = 'provider.logout'")
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(rows, 1);
    // A provider on a key has a key, and is not signed in.
    let api = s
        .rig
        .world
        .app
        .call(ProviderCreate, &[])
        .token(&s.ada.token)
        .body(&on_a_key(&s, "Key"))
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
        .provider;
    let refused = sign_in(&s, api.base.id, &s.ada.token, &by("setup_token", &[]))
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
}

// ---------------------------------------------------------------- the runtime

#[tokio::test]
async fn the_page_of_a_managed_provider_shows_its_runtime_and_when_it_stops_or_renews() {
    let s = scene().await;
    let provider = managed_and_up(&s).await;
    let page = get(&s, provider).await;
    assert_eq!(page.base.address, "acme.ada.Fake subscription");
    assert_eq!(
        page.agent.as_ref().map(|agent| agent.address.as_str()),
        Some("acme.ada.Fake agent")
    );
    let runtime = page.runtime.expect("a managed provider shows its runtime");
    assert_eq!(runtime.state, "running");
    assert_eq!(runtime.driver, "docker");
    assert_eq!(
        runtime.host.as_ref().map(|host| host.name.as_str()),
        Some("Build box")
    );
    assert_eq!(
        runtime.host.as_ref().map(|host| host.address.as_str()),
        Some("acme.ada.Build box")
    );
    assert_eq!(runtime.template.name, "Provider runtime");
    assert_eq!(runtime.generation, 1);
    assert!(runtime.job.is_none());
    assert!(
        runtime.idle_stop_at.is_none(),
        "the shipped provider default keeps its runtime running"
    );
    sqlx::query("UPDATE providers SET runtime_config=runtime_config||$2 WHERE id=$1")
        .bind(provider)
        .bind(json!({"runtime.idle_stop_minutes":1440}))
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    assert!(
        get(&s, provider)
            .await
            .runtime
            .unwrap()
            .idle_stop_at
            .is_some(),
        "a provider that explicitly opts into a daily idle stop shows its schedule"
    );
    // A sign-in is renewed in time while its runtime runs, as it is used there.
    let due = runtime
        .renew_at
        .expect("a running runtime's sign-in is renewed in time");
    stopped(&s, provider).await;
    let runtime = get(&s, provider).await.runtime.unwrap();
    assert_eq!(runtime.state, "stopped");
    assert!(runtime.idle_stop_at.is_none());
    assert_eq!(
        runtime.renew_at,
        Some(due),
        "a stopped runtime is started to renew its sign-in when it is due"
    );
    // Nobody who is not shown the provider is shown its runtime.
    let stranger = s
        .rig
        .world
        .app
        .call(ProviderGet, &[&provider])
        .token(&s.bea.token)
        .send()
        .await;
    assert!(!stranger.status.is_success());
}

#[tokio::test]
async fn a_runtime_that_is_stopped_is_started_by_hand_with_a_job_and_a_provider_on_a_persons_machine_has_none()
 {
    let s = scene().await;
    let start = |who: &str, provider: Id| {
        let call = s.rig.world.app.call(ProviderStart, &[&provider]).token(who);
        async move { call.body(&ProviderStartStopBody::default()).send().await }
    };
    // The scene's provider is on a person's machine: there is no unit of Orch's to start. Until the
    // machine reports a directory runtime service there is nothing to start at all; then a start
    // attaches the harness there and makes no job.
    let undeclared = start(&s.ada.token, s.on.provider).await.problem();
    assert_eq!(undeclared.code, ErrorCode::ProviderUnavailable);
    sqlx::query("UPDATE machines SET facts = facts || $2 WHERE id = $1")
        .bind(s.rig.machine_id)
        .bind(json!({"provider_runtime_descriptor": {"lifecycle": "attach"}}))
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let attached = start(&s.ada.token, s.on.provider).await.ok();
    assert!(attached.runtime.and_then(|runtime| runtime.job).is_none());
    assert!(
        !s.rig.seen.of_kind(K::AgentStart).is_empty(),
        "its harness is attached there"
    );
    let provider = managed_and_up(&s).await;
    stopped(&s, provider).await;
    // Somebody who does not manage the provider cannot.
    let refused = start(&s.eve.token, provider).await.problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(missing_permissions(&refused), ["provider.manage"]);
    let started = start(&s.ada.token, provider).await.ok();
    let runtime = started.runtime.unwrap();
    if let Some(job) = runtime.job {
        assert_eq!(job.kind, "provider_provision");
    } else {
        let (state,finished):(String,Option<chrono::DateTime<chrono::Utc>>) = sqlx::query_as(
            "SELECT state,finished_at FROM jobs WHERE provider_id = $1 AND kind = 'provider_provision' ORDER BY created_at DESC LIMIT 1")
            .bind(provider).fetch_one(s.rig.db.pool()).await.unwrap();
        assert!(
            finished.is_some() && matches!(state.as_str(), "succeeded" | "failed" | "cancelled"),
            "an absent live job must have already ended: {state}"
        );
    }
    let jobs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE provider_id = $1 AND kind = 'provider_provision'",
    )
    .bind(provider)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(jobs, 1, "{}", jobs_dump(&s, provider).await);
}

#[tokio::test]
async fn a_runtime_is_not_stopped_by_hand_while_a_turn_runs_on_it_unless_the_request_says_to_interrupt()
 {
    let s = scene().await;
    let provider = managed_and_up(&s).await;
    let stop = |who: &str, interrupt: Option<bool>| {
        let call = s.rig.world.app.call(ProviderStop, &[&provider]).token(who);
        async move { call.body(&ProviderStartStopBody { interrupt }).send().await }
    };
    // Somebody who does not manage the provider cannot.
    let refused = stop(&s.eve.token, None).await.problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    // A turn is running on the provider.
    let thread = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .body(&CreateThread {
            name: Some("Busy".to_owned()),
            binding: Some(CreateThreadBinding {
                agent_id: s.on.agent,
                provider_id: provider,
                arrangement: None,
            }),
            model: Some(MODEL.to_owned()),
            environment: Some(EnvironmentChoice::Use(s.on.environment)),
            directory: Some("web".to_owned()),
            ..CreateThread::default()
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    let sent = s
        .say(&s.ada, thread.base.id, "Hello", Delivery::Queue, "busy-1")
        .await;
    assert!(sent.status.is_success(), "{:?}", sent.body);
    let turn = s.rig.turn_start(thread.base.id, 0).await;
    s.rig.say(thread.base.id, turn_began(turn.turn_id)).await;
    s.rig.wait_turn(turn.turn_id, "running").await;
    let busy = stop(&s.ada.token, None).await.problem();
    assert_eq!(busy.code, ErrorCode::ThreadNotIdle);
    assert_eq!(busy.extra["running"], 1);
    assert_eq!(
        get(&s, provider).await.runtime.unwrap().state,
        "running",
        "a refused stop leaves the runtime as it was"
    );
    // Told to interrupt, the engine interrupts the turn before it stops the unit; the stop itself is
    // for the host, which this scene has no machine for.
    let world = &s.rig.world;
    let (interrupted, ()) = tokio::join!(stop(&s.ada.token, Some(true)), async {
        // The force window starts at command issue. Advancing before that point leaves
        // a later interrupt permanently young on this held clock.
        eventually("the exact active turn's interrupt and its issued time to be recorded", || async {
            s.rig.commands.issued_of("turn.interrupt") > 0 && s.rig.seen.of_kind(K::TurnInterrupt).iter().any(|command| matches!(command,
                Command::TurnInterrupt(body) if body.thread_id == thread.base.id && body.turn_id == turn.turn_id))
        }).await;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        world.advance(chrono::Duration::seconds(10));
    });
    assert_ne!(interrupted.status.as_u16(), 409, "{:?}", interrupted.body);
    eventually("the machine to be asked to interrupt the turn", || async {
        !s.rig
            .seen
            .of_kind(orch_link_contract::commands::CommandKind::TurnInterrupt)
            .is_empty()
    })
    .await;
}

/// The machine of the scene is where the provider's runtime is and where the environment that holds the
/// credential runs its harness: it gives the credential back, sealed, when it is asked to. The failures it
/// answers the next returns with are queued in the list.
fn the_environment_gives_back(s: &Scene) -> (Heard, Arc<Mutex<Vec<CommandError>>>) {
    let heard: Heard = Arc::new(Mutex::new(Vec::new()));
    let refusals: Arc<Mutex<Vec<CommandError>>> = Arc::default();
    let handler = &s.rig.machine.handler;
    {
        let (heard, refusals) = (Arc::clone(&heard), Arc::clone(&refusals));
        handler.on(K::AgentCredentialReturn, move |command: &Command| {
            lock(&heard).push((K::AgentCredentialReturn, command.body().unwrap()));
            if let Some(error) = lock(&refusals).pop() {
                return Err(error);
            }
            Ok(CommandResult::AgentCredentialReturn(SealedCredential {
                form: "login".to_owned(),
                credential: Sealed {
                    value: "c2VhbGVkLWJ5LXRoZS1lbnZpcm9ubWVudA".to_owned(),
                },
                expires_at: None,
            }))
        });
    }
    {
        let heard = Arc::clone(&heard);
        handler.on(K::ProviderCredentialAccept, move |command: &Command| {
            lock(&heard).push((K::ProviderCredentialAccept, command.body().unwrap()));
            Ok(CommandResult::ProviderCredentialAccept(Empty {}))
        });
    }
    (heard, refusals)
}

/// The provider's credential is with the scene's environment.
async fn handed_over(s: &Scene, provider: Id) {
    sqlx::query("UPDATE providers SET held_by_environment_id = $2, held_since = $3 WHERE id = $1")
        .bind(provider)
        .bind(s.on.environment)
        .bind(s.rig.world.now())
        .execute(s.rig.db.pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn a_credential_handed_over_is_brought_home_by_whoever_manages_the_provider_or_the_environment_and_given_up_by_the_first()
 {
    let s = scene().await;
    let provider = managed_and_up(&s).await;
    let (heard, refusals) = the_environment_gives_back(&s);
    let bring = |who: &str, interrupt: Option<bool>, declare_lost: Option<bool>| {
        let call = s
            .rig
            .world
            .app
            .call(ProviderHandoverReturn, &[&provider])
            .token(who);
        async move {
            call.body(&ProviderHandoverReturnBody {
                interrupt,
                declare_lost,
            })
            .send()
            .await
        }
    };
    // Nothing is handed over: whoever manages the provider is told so, and the others what they lack.
    let nothing = bring(&s.ada.token, None, None).await.problem();
    assert_eq!(nothing.code, ErrorCode::RequestInvalid);
    handed_over(&s, provider).await;
    let held = get(&s, provider)
        .await
        .held_by
        .expect("the page says who holds it");
    assert_eq!(held.environment.id, s.on.environment);
    // Whoever may only read or use the provider can do neither.
    let viewer = bring(&s.cai.token, None, None).await.problem();
    assert_eq!(viewer.code, ErrorCode::AuthzDenied);
    // Whoever manages only the environment that holds it may bring it home, and may not give it up.
    let lost = bring(&s.eve.token, None, Some(true)).await.problem();
    assert_eq!(lost.code, ErrorCode::AuthzDenied);
    assert_eq!(missing_permissions(&lost), ["provider.manage"]);
    assert!(get(&s, provider).await.held_by.is_some());
    assert!(asked(&heard, K::AgentCredentialReturn).is_empty());
    // An environment that cannot be reached leaves the credential where it is and says so.
    lock(&refusals).push(CommandError::new(
        Wire::OfflinePeer,
        "the environment is away",
    ));
    let failed = bring(&s.eve.token, None, None).await.problem();
    assert_eq!(failed.code, ErrorCode::CredentialReturnFailed);
    // A detail is its code's catalogue sentence; what the environment said stays in the log.
    assert_eq!(
        failed.detail,
        orch_api_contract::messages::for_code(ErrorCode::CredentialReturnFailed)
    );
    assert_eq!(
        asked(&heard, K::AgentCredentialReturn).len(),
        1,
        "the environment was asked"
    );
    assert!(get(&s, provider).await.held_by.is_some());
    // Tried again, it comes home: the environment gives it back sealed and the runtime is handed it as it came.
    let home = bring(&s.eve.token, None, None)
        .await
        .ok()
        .into_full()
        .expect("the ordinary holder receives its provider projection")
        .provider;
    assert!(home.held_by.is_none());
    let accepted = asked(&heard, K::ProviderCredentialAccept);
    assert_eq!(accepted.len(), 1);
    assert_eq!(
        accepted[0]["credential"]["$sealed"],
        "c2VhbGVkLWJ5LXRoZS1lbnZpcm9ubWVudA"
    );
    let audited: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = 'provider.returned'")
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(audited, 1);
    let again = bring(&s.ada.token, None, None).await.problem();
    assert_eq!(
        again.code,
        ErrorCode::RequestInvalid,
        "it is not away any more"
    );
    // Taken away again and given up instead, by whoever manages the provider: it has to be signed in.
    handed_over(&s, provider).await;
    let given_up = bring(&s.ada.token, None, Some(true))
        .await
        .ok()
        .into_full()
        .expect("the ordinary provider manager receives its provider projection")
        .provider;
    assert!(given_up.held_by.is_none());
    assert_eq!(given_up.base.status, ProviderStatus::NeedsSetup);
    let lost_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'provider.handover_lost'",
    )
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(lost_rows, 1);
    assert_eq!(
        asked(&heard, K::ProviderCredentialAccept).len(),
        1,
        "a credential given up is not also taken home"
    );
}

#[tokio::test]
async fn a_deleted_provider_is_signed_out_first_and_its_runtime_is_retired() {
    let s = scene().await;
    let provider = managed_and_up(&s).await;
    let heard = the_harness_answers(&s, provider);
    let delivered = Arc::new(Mutex::new(Vec::<Command>::new()));
    for kind in [K::AgentStop, K::ProviderStop] {
        let delivered = Arc::clone(&delivered);
        let agent = s.on.agent;
        s.rig.machine.handler.on(kind, move |command| {
            let result = match command {
                Command::AgentStop(body)
                    if body.provider_id == provider && body.agent_id == agent =>
                {
                    CommandResult::AgentStop(Empty {})
                }
                Command::ProviderStop(body) if body.provider_id == provider => {
                    CommandResult::ProviderStop(Empty {})
                }
                _ => {
                    return Err(CommandError::new(
                        Wire::NotFound,
                        "another provider or harness",
                    ));
                }
            };
            lock(&delivered).push(command.clone());
            Ok(result)
        });
    }
    // Signing in starts the selected harness through the same public provider producer.
    let signed_in = sign_in(
        &s,
        provider,
        &s.ada.token,
        &by("setup_token", &[("token", TOKEN)]),
    )
    .await
    .ok();
    assert_eq!(signed_in.state, LoginProgress::Completed);
    assert_eq!(asked(&heard, K::ProviderLoginStart).len(), 1);
    let harnesses: Vec<(Id, Id, Id, String)> = sqlx::query_as(
        "SELECT provider_id,agent_id,machine_id,status FROM harnesses WHERE provider_id=$1 ORDER BY agent_id,machine_id"
    ).bind(provider).fetch_all(s.rig.db.pool()).await.unwrap();
    assert_eq!(
        harnesses,
        [(provider, s.on.agent, s.rig.machine_id, "online".to_owned())]
    );
    let before: (Id, String, Json) =
        sqlx::query_as("SELECT id,state,volumes FROM runtimes WHERE provider_id=$1")
            .bind(provider)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(before.1, "running");
    type ThreadBindingRow = (Id, Option<Id>, Option<Id>, Option<Id>);
    let bindings: Vec<ThreadBindingRow> = sqlx::query_as(
        "SELECT node_id,provider_id,agent_id,environment_id FROM threads WHERE provider_id=$1 ORDER BY node_id"
    ).bind(provider).fetch_all(s.rig.db.pool()).await.unwrap();
    let observe = std::env::var_os("ORCH_TEST_SQL_TIMINGS").is_some();
    let stop_counts =
        || [K::AgentStop, K::ProviderStop].map(|kind| s.rig.commands.issued_of(kind.as_str()));
    let before_stops = stop_counts();
    let refused = s
        .rig
        .world
        .app
        .call(ProviderDelete, &[&provider])
        .token(&s.eve.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert!(asked(&heard, K::ProviderLogout).is_empty());
    assert_eq!(
        stop_counts(),
        before_stops,
        "an unauthorized delete issues no stop or purge"
    );
    assert!(lock(&delivered).is_empty());
    let after_denied: (Id, String, Json) =
        sqlx::query_as("SELECT id,state,volumes FROM runtimes WHERE provider_id=$1")
            .bind(provider)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(
        after_denied, before,
        "a denied delete changes no runtime or volume"
    );
    let still_here: (bool, String, Option<Id>) =
        sqlx::query_as("SELECT deleted_at IS NULL,status,machine_id FROM providers WHERE id=$1")
            .bind(provider)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(still_here, (true, "ready".into(), Some(s.rig.machine_id)));
    if observe {
        eprintln!(
            "provider delete denied: issued agent/keeper stops before={before_stops:?} after={:?}",
            stop_counts()
        );
    }
    // The deleted/stopped transaction commits before best-effort native cleanup can answer.
    s.rig.machine.handler.hold(K::AgentStop);
    s.rig.machine.handler.hold(K::ProviderStop);
    let reply = s
        .rig
        .world
        .app
        .call(ProviderDelete, &[&provider])
        .token(&s.ada.token)
        .send()
        .await;
    assert!(reply.status.is_success(), "{:?}", reply.body);
    let stopped: (bool, String, bool, bool) = sqlx::query_as(
        "SELECT deleted_at IS NOT NULL,status,login_pending IS NULL,login_state IS NULL FROM providers WHERE id=$1"
    ).bind(provider).fetch_one(s.rig.db.pool()).await.unwrap();
    assert_eq!(stopped, (true, "disabled".into(), true, true));
    assert!(
        lock(&delivered).is_empty(),
        "stopped state does not wait for native answers"
    );
    let secrets: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM secrets WHERE owner_kind='provider' AND owner_id=$1",
    )
    .bind(provider)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(secrets, 0, "no provider secrets remain");
    s.rig.machine.handler.release();
    eventually("the admitted sign-out, keeper purge and runtime retirement to finish", || async {
        let answered = {
            let commands = lock(&delivered);
            commands.iter().any(|command| matches!(command, Command::AgentStop(body)
                if body.provider_id==provider && body.agent_id==s.on.agent && body.logout==Some(true)))
                && commands.iter().any(|command| matches!(command, Command::ProviderStop(body)
                if body.provider_id==provider && body.purge==Some(true)))
        };
        let retired = sqlx::query_as::<_,(String,Option<Id>)>(
            "SELECT r.state,p.machine_id FROM runtimes r JOIN providers p ON p.id=r.provider_id WHERE r.id=$1"
        ).bind(before.0).fetch_optional(s.rig.db.pool()).await.ok().flatten();
        answered && retired.is_some_and(|(state,machine)| matches!(state.as_str(),"retired"|"removed") && machine.is_none())
    }).await;
    let commands = lock(&delivered).clone();
    // The supervisor may also close provider admission with a non-purging stop.
    // Native sign-out must precede the separate destructive keeper cleanup.
    assert_eq!(
        commands
            .iter()
            .filter_map(|command| match command {
                Command::AgentStop(body)
                    if body.provider_id == provider
                        && body.agent_id == s.on.agent
                        && body.logout == Some(true) =>
                    Some(K::AgentStop),
                Command::ProviderStop(body)
                    if body.provider_id == provider && body.purge == Some(true) =>
                    Some(K::ProviderStop),
                _ => None,
            })
            .collect::<Vec<_>>(),
        [K::AgentStop, K::ProviderStop],
        "the active harness signs out before the keeper is purged"
    );
    let logouts: Vec<_> = commands
        .iter()
        .filter_map(|command| match command {
            Command::AgentStop(body) if body.provider_id == provider => {
                Some((body.agent_id, body.logout))
            }
            _ => None,
        })
        .collect();
    let purges: Vec<_> = commands
        .iter()
        .filter_map(|command| match command {
            Command::ProviderStop(body)
                if body.provider_id == provider && body.purge == Some(true) =>
            {
                Some(body.purge)
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        logouts,
        [(s.on.agent, Some(true))],
        "the active harness receives actual native sign-out as part of stopping"
    );
    assert_eq!(
        purges,
        [Some(true)],
        "the selected keeper purges its provider state"
    );
    assert!(
        asked(&heard, K::ProviderLogout).is_empty(),
        "deletion uses its harness stop protocol"
    );
    if observe {
        let recipe = sqlx::query_as::<_, (i32, i32, bool)>(
            "SELECT jsonb_array_length(plan->'quiesce'->'harnesses'),jsonb_array_length(plan->'quiesce'->'keepers'),
                    (plan->'quiesce'->>'logout')::boolean FROM jobs WHERE kind=$1 AND plan#>>'{quiesce,target,object,id}'=$2
               ORDER BY created_at DESC,id DESC LIMIT 1"
        ).bind(orch_engine::resource_pause::KIND).bind(provider.to_string()).fetch_optional(s.rig.db.pool()).await;
        eprintln!(
            "provider delete accepted: issued agent/keeper stops before={before_stops:?} after={:?} delivered logout={logouts:?} purge={purges:?} recipe harnesses/keepers/logout={recipe:?}",
            stop_counts()
        );
    }
    let state: String = sqlx::query_scalar("SELECT state FROM runtimes WHERE provider_id = $1")
        .bind(provider)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert!(
        matches!(state.as_str(), "retired" | "removed"),
        "the unit and the volume with the sign-in go with the provider: {state}"
    );
    let machine: Option<Id> = sqlx::query_scalar("SELECT machine_id FROM providers WHERE id = $1")
        .bind(provider)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert!(machine.is_none(), "a deleted provider has no machine");
    let retired_volume: Json = sqlx::query_scalar("SELECT volumes FROM runtimes WHERE id=$1")
        .bind(before.0)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(
        retired_volume, before.2,
        "the retired runtime keeps the original volume cleanup metadata"
    );
    let bindings_after: Vec<ThreadBindingRow> = sqlx::query_as(
        "SELECT node_id,provider_id,agent_id,environment_id FROM threads WHERE provider_id=$1 ORDER BY node_id"
    ).bind(provider).fetch_all(s.rig.db.pool()).await.unwrap();
    assert_eq!(
        bindings_after, bindings,
        "the existing thread binding inventory is unchanged"
    );
}

#[tokio::test]
async fn creating_units_keep_retained_facts_without_idle_or_renewal_schedules() {
    let s = scene().await;
    s.rig.engine.stop();
    let provider = managed_and_up(&s).await;
    let now = s.rig.world.now();
    let last_active = now - chrono::Duration::hours(2);
    let renewed = now - chrono::Duration::days(3);
    let expired = now - chrono::Duration::days(1);
    let mut conn = s.rig.db.acquire().await.unwrap();
    let updated = sqlx::query("UPDATE runtimes SET last_active_at=$2 WHERE provider_id=$1 AND successor_of IS NULL AND state NOT IN ('retired','removed')")
        .bind(provider).bind(last_active).execute(&mut *conn).await.unwrap();
    assert_eq!(updated.rows_affected(), 1);
    sqlx::query("UPDATE providers SET credential_renewed_at=$2,facts=jsonb_set(facts,'{credential}',$3),runtime_config=runtime_config||$4 WHERE id=$1")
        .bind(provider).bind(renewed)
        .bind(json!({"yields":"login","expires_at":orch_types::Time::new(expired)}))
        .bind(json!({"runtime.idle_stop_minutes":60,"runtime.renew_days":5}))
        .execute(&mut *conn).await.unwrap();
    drop(conn);
    let before = s.rig.commands.issued_total();
    let running = get(&s, provider).await.runtime.unwrap();
    assert_eq!(running.state, "running");
    assert_eq!(
        running.last_active_at,
        Some(orch_types::Time::new(last_active))
    );
    assert!(running.idle_stop_at.is_some());
    assert_eq!(
        running.renew_at,
        Some(orch_types::Time::new(expired - chrono::Duration::days(5)))
    );
    sqlx::query("UPDATE runtimes SET state='creating' WHERE provider_id=$1")
        .bind(provider)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let creating = get(&s, provider).await.runtime.unwrap();
    assert_eq!(creating.state, "creating");
    assert_eq!(creating.last_active_at, running.last_active_at);
    assert!(creating.idle_stop_at.is_none() && creating.renew_at.is_none());
    assert_eq!(creating.driver, running.driver);
    assert_eq!(creating.generation, running.generation);
    assert_eq!(
        serde_json::to_value(&creating.template).unwrap(),
        serde_json::to_value(&running.template).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&creating.host).unwrap(),
        serde_json::to_value(&running.host).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&creating.network).unwrap(),
        serde_json::to_value(&running.network).unwrap()
    );
    stopped(&s, provider).await;
    let stopped_runtime = get(&s, provider).await.runtime.unwrap();
    assert_eq!(stopped_runtime.state, "stopped");
    assert_eq!(stopped_runtime.last_active_at, running.last_active_at);
    assert!(stopped_runtime.idle_stop_at.is_none());
    assert_eq!(stopped_runtime.renew_at, running.renew_at);
    // A stopped unit still uses its successful-renewal stamp when no expiry was reported.
    sqlx::query("UPDATE providers SET facts=facts#-'{credential,expires_at}' WHERE id=$1")
        .bind(provider)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let fallback = get(&s, provider).await.runtime.unwrap();
    let fallback_due = Some(orch_types::Time::new(renewed + chrono::Duration::days(5)));
    assert_eq!(fallback.renew_at, fallback_due);
    sqlx::query("UPDATE runtimes SET state='creating' WHERE provider_id=$1")
        .bind(provider)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let retained_creating = get(&s, provider).await.runtime.unwrap();
    assert_eq!(retained_creating.last_active_at, running.last_active_at);
    assert!(retained_creating.idle_stop_at.is_none() && retained_creating.renew_at.is_none());
    sqlx::query("UPDATE runtimes SET state='running' WHERE provider_id=$1")
        .bind(provider)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let resumed = get(&s, provider).await.runtime.unwrap();
    assert_eq!(resumed.state, "running");
    assert_eq!(resumed.last_active_at, running.last_active_at);
    assert!(resumed.idle_stop_at.is_some());
    assert_eq!(resumed.renew_at, fallback_due);
    assert_eq!(
        s.rig.commands.issued_total(),
        before,
        "stored schedule projection issues no native command"
    );
}

// Actual stored operation-scoped credentials use the ordinary login HTTP/command producer.
async fn login_key(
    s: &Scene,
    owner: Id,
    provider: Id,
    operations: &[&str],
) -> (String, Id, orch_access_contract::key_scope::KeyScope) {
    use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use orch_db::queries::identity::{NewApiToken, create_api_token_scoped};
    let scope = serde_json::from_value(json!({"rules":[{
        "target":{"kind":"provider","id":provider,"reach":"self"},
        "allow":operations.iter().map(|operation| json!({"operation":operation,"actions":["manage"]})).collect::<Vec<_>>()
    }]})).unwrap();
    let token = new_token(API_TOKEN_PREFIX);
    let id = new_id();
    create_api_token_scoped(
        &mut *s.rig.world.conn().await,
        &NewApiToken {
            id,
            user_id: owner,
            name: "Provider authorization control",
            token_hash: &token_hash(&token),
            scopes: &[],
            node_id: None,
            at: s.rig.world.now(),
            expires_at: None,
        },
        Some(&serde_json::to_value(&scope).unwrap()),
    )
    .await
    .unwrap();
    (token, id, scope)
}

async fn login_input_origin(
    s: &Scene,
    provider: Id,
    owner: Id,
    token: Id,
    scope: orch_access_contract::key_scope::KeyScope,
) -> orch_authz::CapturedOperation {
    let caller = orch_authz::Caller::token(
        owner,
        orch_authz::TokenGrant::from_stored_scope(&[], None, Some(scope), None)
            .unwrap()
            .for_token(token),
    );
    let plan = orch_engine::provider_setup::login_operation(
        &mut *s.rig.world.conn().await,
        provider,
        "ProviderLoginInput",
    )
    .await
    .unwrap();
    orch_authz::CapturedOperation::capture(&plan.admit(&caller, false).unwrap()).unwrap()
}

#[tokio::test]
async fn scoped_provider_interactive_authorization_uses_the_same_declared_method_and_third_party_answer()
 {
    let s = scene().await;
    let provider = managed_and_up(&s).await;
    let heard = the_harness_answers(&s, provider);
    let (starter, _, _) = login_key(
        &s,
        s.ada.id(),
        provider,
        &["ProviderLoginStart.interactive"],
    )
    .await;
    let started = sign_in(&s, provider, &starter, &by("device_code", &[]))
        .await
        .ok();
    assert_eq!(started.state, LoginProgress::AwaitingUser);
    assert_eq!(started.shows[1].value, "ABCD-1234");
    assert_eq!(asked(&heard, K::ProviderLoginStart).len(), 1);
    // An operation name is not a grant for the separate polling or answer operation.
    assert_eq!(
        s.rig
            .world
            .app
            .call(ProviderLoginGet, &[&provider])
            .token(&starter)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    let (answerer, _, _) = login_key(
        &s,
        s.ada.id(),
        provider,
        &["ProviderLoginInput", "ProviderLoginGet"],
    )
    .await;
    let state = s
        .rig
        .world
        .app
        .call(ProviderLoginGet, &[&provider])
        .token(&answerer)
        .send()
        .await
        .ok();
    assert_eq!(state.state, LoginProgress::AwaitingUser);
    let done = s
        .rig
        .world
        .app
        .call(ProviderLoginInput, &[&provider])
        .token(&answerer)
        .body(&ProviderLoginInputBody {
            values: BTreeMap::from([("answer".into(), "actual third-party proof".into())]),
        })
        .send()
        .await
        .ok();
    assert_eq!(done.state, LoginProgress::Completed);
    assert_eq!(
        asked(&heard, K::ProviderLoginInput)[0]["values"]["answer"],
        "actual third-party proof"
    );
    let stored: Json = sqlx::query_scalar("SELECT login_authority FROM providers WHERE id=$1")
        .bind(provider)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(
        stored["origin"]["operation"],
        "ProviderLoginStart.interactive"
    );
    assert!(!stored.to_string().contains("node_ancestors"));
    let public = serde_json::to_string(&get(&s, provider).await).unwrap();
    assert!(!public.contains("login_authority") && !public.contains("token_ids"));
}

#[tokio::test]
async fn scoped_provider_start_requires_the_actual_variant_target_and_current_manager() {
    let s = scene().await;
    let provider = managed_and_up(&s).await;
    let other_provider = {
        let mut conn = s.rig.world.conn().await;
        let id = fixtures::insert_resource(
            &mut conn,
            "provider",
            Some(s.ada.id()),
            Some(s.tree.org.base.id),
            "Other actual provider",
        )
        .await
        .unwrap();
        start_resource(
            &mut conn,
            ObjectRef {
                kind: ResourceKind::Provider.into(),
                id,
            },
            s.ada.id(),
            NewAccess::Defaults,
            Time::new(s.rig.world.now()),
        )
        .await
        .unwrap();
        id
    };
    assert_ne!(
        other_provider, provider,
        "the scope control names another real provider"
    );
    let heard = the_harness_answers(&s, provider);
    for (case, owner, target, operation) in [
        (
            "wrong variant",
            s.ada.id(),
            provider,
            "ProviderLoginStart.import",
        ),
        (
            "wrong target",
            s.ada.id(),
            other_provider,
            "ProviderLoginStart.interactive",
        ),
        (
            "ordinary manager",
            s.eve.id(),
            provider,
            "ProviderLoginStart.interactive",
        ),
    ] {
        let (token, _, _) = login_key(&s, owner, target, &[operation]).await;
        let reply = sign_in(&s, provider, &token, &by("device_code", &[])).await;
        assert_eq!(
            reply.status,
            http::StatusCode::FORBIDDEN,
            "{case}: {:?}",
            reply.body
        );
        assert_eq!(reply.problem().code, ErrorCode::AuthzDenied, "{case}");
    }
    assert!(asked(&heard, K::ProviderLoginStart).is_empty());
    assert!(!waits(&s, provider).await);
}

#[tokio::test]
async fn scoped_provider_original_start_bounds_survive_input_and_cancel_remains_independent() {
    for change in ["revoke", "expire", "narrow"] {
        let s = scene().await;
        let provider = managed_and_up(&s).await;
        let heard = the_harness_answers(&s, provider);
        let (starter, start_id, _) = login_key(
            &s,
            s.ada.id(),
            provider,
            &["ProviderLoginStart.interactive"],
        )
        .await;
        assert_eq!(
            sign_in(&s, provider, &starter, &by("device_code", &[]))
                .await
                .ok()
                .state,
            LoginProgress::AwaitingUser
        );
        match change {
            "revoke" => {
                sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
                    .bind(start_id)
                    .bind(s.rig.world.now())
                    .execute(s.rig.db.pool())
                    .await
                    .unwrap();
            }
            "expire" => {
                sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
                    .bind(start_id)
                    .bind(s.rig.world.now() - chrono::Duration::seconds(1))
                    .execute(s.rig.db.pool())
                    .await
                    .unwrap();
            }
            _ => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(start_id)
                    .bind(json!({"rules":[]}))
                    .execute(s.rig.db.pool())
                    .await
                    .unwrap();
            }
        }
        let (controller, _, _) = login_key(
            &s,
            s.ada.id(),
            provider,
            &["ProviderLoginInput", "ProviderLoginCancel"],
        )
        .await;
        assert_eq!(
            s.rig
                .world
                .app
                .call(ProviderLoginInput, &[&provider])
                .token(&controller)
                .body(&ProviderLoginInputBody {
                    values: BTreeMap::from([("answer".into(), "proof".into())])
                })
                .send()
                .await
                .problem()
                .code,
            ErrorCode::AuthzDenied,
            "{change}"
        );
        assert!(asked(&heard, K::ProviderLoginInput).is_empty(), "{change}");
        s.rig
            .world
            .app
            .call(ProviderLoginCancel, &[&provider])
            .token(&controller)
            .send()
            .await
            .ok();
        assert_eq!(asked(&heard, K::ProviderLoginCancel).len(), 1);
        let active: Option<Json> =
            sqlx::query_scalar("SELECT login_authority FROM providers WHERE id=$1")
                .bind(provider)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap();
        assert!(active.is_none());
    }
}

#[tokio::test]
async fn scoped_provider_later_input_keeps_its_own_accepted_and_current_credential_at_dispatch() {
    use orch_engine::provider_setup::{Asked, LoginAuthority, SignInValue};
    let s = scene().await;
    let provider = managed_and_up(&s).await;
    let heard = the_harness_answers(&s, provider);
    let (starter, _, _) = login_key(
        &s,
        s.ada.id(),
        provider,
        &["ProviderLoginStart.interactive"],
    )
    .await;
    sign_in(&s, provider, &starter, &by("device_code", &[]))
        .await
        .ok();
    let (_, input_id, scope) = login_key(&s, s.ada.id(), provider, &["ProviderLoginInput"]).await;
    let origin = login_input_origin(&s, provider, s.ada.id(), input_id, scope).await;
    let active = LoginAuthority::load(&mut *s.rig.world.conn().await, provider)
        .await
        .unwrap();
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(input_id)
        .bind(s.rig.world.now())
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let result = s
        .rig
        .world
        .app
        .app
        .engine
        .send_login_input_authorized(
            provider,
            &BTreeMap::from([("answer".into(), SignInValue::Text("accepted answer".into()))]),
            origin,
            active,
        )
        .await
        .unwrap();
    assert!(matches!(result, Asked::Refused(_)), "{result:?}");
    assert!(asked(&heard, K::ProviderLoginInput).is_empty());
}

#[tokio::test]
async fn scoped_provider_stale_input_attempt_cannot_send_into_or_clear_a_replacement_login() {
    use orch_engine::provider_setup::{Asked, LoginAuthority, SignInValue};
    let s = scene().await;
    let provider = managed_and_up(&s).await;
    let heard = the_harness_answers(&s, provider);
    let (starter, _, _) = login_key(
        &s,
        s.ada.id(),
        provider,
        &["ProviderLoginStart.interactive"],
    )
    .await;
    sign_in(&s, provider, &starter, &by("device_code", &[]))
        .await
        .ok();
    let old = LoginAuthority::load(&mut *s.rig.world.conn().await, provider)
        .await
        .unwrap();
    let (_, input_id, scope) = login_key(&s, s.ada.id(), provider, &["ProviderLoginInput"]).await;
    let origin = login_input_origin(&s, provider, s.ada.id(), input_id, scope).await;
    sign_in(&s, provider, &starter, &by("device_code", &[]))
        .await
        .ok();
    let current = LoginAuthority::load(&mut *s.rig.world.conn().await, provider)
        .await
        .unwrap();
    assert_ne!(old, current);
    let result = s
        .rig
        .world
        .app
        .app
        .engine
        .send_login_input_authorized(
            provider,
            &BTreeMap::from([("answer".into(), SignInValue::Text("stale answer".into()))]),
            origin,
            old,
        )
        .await
        .unwrap();
    assert!(matches!(result, Asked::Refused(_)));
    assert!(asked(&heard, K::ProviderLoginInput).is_empty());
    assert_eq!(
        LoginAuthority::load(&mut *s.rig.world.conn().await, provider)
            .await
            .unwrap(),
        current
    );
}

#[tokio::test]
async fn scoped_provider_queued_start_rechecks_its_original_key_before_any_native_dispatch() {
    use orch_authz::{Caller, TokenGrant};
    use orch_engine::provider_setup::{Delivered, Pending, login_operation};
    let s = scene().await;
    let provider = managed_and_up(&s).await;
    let heard = the_harness_answers(&s, provider);
    let (_, token, scope) = login_key(
        &s,
        s.ada.id(),
        provider,
        &["ProviderLoginStart.interactive"],
    )
    .await;
    let mut conn = s.rig.world.conn().await;
    let caller = Caller::token(
        s.ada.id(),
        TokenGrant::from_stored_scope(&[], None, Some(scope), None)
            .unwrap()
            .for_token(token),
    );
    let plan = login_operation(&mut conn, provider, "ProviderLoginStart.interactive")
        .await
        .unwrap();
    let pending = Pending::new("device_code", &BTreeMap::new(), s.ada.id())
        .with_authority(&plan.admit(&caller, false).unwrap())
        .unwrap();
    runtime_rows::keep_pending(
        &mut conn,
        provider,
        &pending.to_bytes().unwrap(),
        s.rig.world.now(),
    )
    .await
    .unwrap();
    runtime_rows::keep_login_authority(
        &mut conn,
        provider,
        pending.authority_state().unwrap().as_ref(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(token)
        .bind(s.rig.world.now())
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let result = s
        .rig
        .world
        .app
        .app
        .engine
        .deliver_sign_in(provider)
        .await
        .unwrap();
    assert!(matches!(result, Delivered::Refused(_)), "{result:?}");
    assert!(asked(&heard, K::ProviderLoginStart).is_empty());
    assert!(!waits(&s, provider).await);
}
