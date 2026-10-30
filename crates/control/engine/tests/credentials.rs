//! A credential that leaves the provider's runtime: a copy issued sealed to the machine of the harness and given to it before its session opens, the login handed over to one environment at a time and brought home, a return that fails and is tried again or given up, and an environment that does not stop before its credential is home.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;
mod runtimes;

use chrono::Utc;
use common::{Added, World, eventually, queued, turn_began, turn_ended};
use hosting::{ScriptedHost, serve};
use orch_agent_contract::arrangement::{Incompatible, NotAllowedBy};
use orch_agent_contract::descriptor::ArrangementType;
use orch_authz::Caller;
use orch_db::queries::resources::provider_runtime as rows;
use orch_engine::binding::{CreateThreadError, NewThread};
use orch_engine::credentials::handover::{ReturnError, ReturnWhy};
use orch_engine::environments::{StopError, StopReason};
use orch_engine::provider_runtime::StopWhy;
use orch_engine::provider_setup::{Asked, Delivered, Pending, SignInValue};
use orch_link_contract::commands::{
    AgentStartResult, Command, CommandError, CommandKind as K, CommandResult, Empty, ErrorCode,
    Sealed, SealedCredential, TurnStart,
};
use orch_link_contract::events::{CredentialNeed, CredentialNeeded, Event, TurnOutcome};
use orch_link_contract::streams::Stream;
use orch_testkit::doubles::machine::ScriptedMachine;
use orch_types::{Id, Json, Time, new_id};
use runtimes::{managed_world_provider, provider_template};
use serde_json::json;
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The descriptor of a harness that can run on a copy of the credential and on the login itself.
fn declared() -> Json {
    let mut declared = common::descriptor();
    declared["arrangements"] = json!([
        { "type": "together" },
        { "type": "separated", "executor": "orch-exec", "executor_version": "*", "tool_sets": ["plain"] },
        { "type": "credential_shared" },
        { "type": "handed_over" }
    ]);
    declared["credentials"] = json!({
        "issues": [
            { "form": "access_token", "custody": "shared", "lifetime_secs": 3600 },
            { "form": "login", "custody": "transferred", "lifetime_secs": null }
        ],
        "accepts": ["access_token", "login"]
    });
    declared["native_provider"]["login_methods"] = json!([{
        "id": "device_code", "title": "Sign in with a code", "summary": "Shows a code.",
        "kind": "interactive", "yields": "login", "lifetime_secs": null,
        "requires": null, "recommended": true, "fields": []
    }]);
    declared
}

/// What the provider's runtime issues: the form asked for, sealed to the machine it is for. The sealed
/// text names the machine, so that a test can see where it went.
fn sealed_for(machine: Id) -> String {
    format!("sealed-for-{machine}")
}

/// What the environment gives back when the credential is returned.
const RETURNED: &str = "sealed-by-the-environment";

/// A world with a harness machine apart from the provider's: an environment on it that can run the
/// harness, a provider that allows a copy to be shared and the login to be handed over, and machines
/// that answer the exchange of credentials.
struct Stand {
    world: World,
    env: Added,
    environment: Id,
    /// The failures the environment answers the next `agent.credential.return` with, one each.
    return_refusals: Arc<Mutex<VecDeque<CommandError>>>,
}

fn script(
    world: &World,
    machine: &Arc<ScriptedMachine>,
    id: Id,
    kind: K,
    reply: impl Fn(&Command) -> Result<CommandResult, CommandError> + Send + Sync + 'static,
) {
    let ordered = Arc::clone(&world.ordered);
    machine.handler.on(kind, move |command: &Command| {
        lock(&ordered).push((id, command.clone()));
        reply(command)
    });
}

async fn stand() -> Stand {
    // The provider was looked at just before: its scheduled read starts no harness while a test counts
    // what a turn asks of the machines.
    stand_on(World::observed().await).await
}

async fn stand_on(world: World) -> Stand {
    let env = world.add_machine("env", &["agent", "environment"]).await;
    let pool = world.pool.pool();
    sqlx::query("UPDATE agents SET descriptor = $2 WHERE id = $1")
        .bind(world.agent)
        .bind(declared())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE providers SET arrangements = ARRAY['separated','credential_shared','handed_over']
          WHERE id = $1",
    )
    .bind(world.provider)
    .execute(pool)
    .await
    .unwrap();
    let (template, environment) = (new_id(), new_id());
    sqlx::query("INSERT INTO env_templates (id, name, driver, spec) VALUES ($1, 'with the harness', 'test', $2)")
        .bind(template)
        .bind(json!({ "colocated_harnesses": ["fake"] }))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO environments (id, name, owner_id, org_id, placement, home_node_id, workspace_root,
                                   machine_id, template_id, generation, status)
         VALUES ($1, 'Elsewhere', $2, $3, 'managed', $4, '/workspace', $5, $6, 1, 'running')",
    )
    .bind(environment)
    .bind(world.ada)
    .bind(world.root)
    .bind(world.topic)
    .bind(env.id)
    .bind(template)
    .execute(pool)
    .await
    .unwrap();
    // Both machines say what their harness declares, and the provider's runtime issues and takes home.
    for (machine, id) in [(&world.machine, world.machine_id), (&env.machine, env.id)] {
        script(&world, machine, id, K::AgentStart, |_| {
            Ok(CommandResult::AgentStart(AgentStartResult {
                reused: false,
                harness_version: "1.0.0".to_owned(),
                descriptor: serde_json::from_value(declared()).unwrap(),
            }))
        });
    }
    let issue_clock = Arc::clone(&world.clock);
    script(
        &world,
        &world.machine,
        world.machine_id,
        K::ProviderCredentialIssue,
        move |command| {
            let Command::ProviderCredentialIssue(issue) = command else {
                return Err(CommandError::new(ErrorCode::Internal, "not an issue"));
            };
            Ok(CommandResult::ProviderCredentialIssue(SealedCredential {
                form: issue.form.clone(),
                credential: Sealed {
                    value: sealed_for(issue.recipient.machine_id),
                },
                expires_at: (issue.form == "access_token")
                    .then(|| Time::new(issue_clock.now() + chrono::Duration::hours(1))),
            }))
        },
    );
    script(
        &world,
        &world.machine,
        world.machine_id,
        K::ProviderCredentialAccept,
        |_| Ok(CommandResult::ProviderCredentialAccept(Empty {})),
    );
    script(&world, &env.machine, env.id, K::AgentCredentialSet, |_| {
        Ok(CommandResult::AgentCredentialSet(Empty {}))
    });
    let return_refusals: Arc<Mutex<VecDeque<CommandError>>> = Arc::default();
    let refusals = Arc::clone(&return_refusals);
    script(
        &world,
        &env.machine,
        env.id,
        K::AgentCredentialReturn,
        move |_| {
            if let Some(error) = lock(&refusals).pop_front() {
                return Err(error);
            }
            Ok(CommandResult::AgentCredentialReturn(SealedCredential {
                form: "login".to_owned(),
                credential: Sealed {
                    value: RETURNED.to_owned(),
                },
                expires_at: None,
            }))
        },
    );
    Stand {
        world,
        env,
        environment,
        return_refusals,
    }
}

impl Stand {
    /// A thread in the environment, in the arrangement.
    async fn thread(&self, kind: ArrangementType, name: &str) -> Id {
        self.thread_in(self.environment, Some(kind), name).await
    }

    async fn thread_in(&self, environment: Id, kind: Option<ArrangementType>, name: &str) -> Id {
        let world = &self.world;
        world
            .engine()
            .create_thread(
                &Caller::session(world.ada),
                NewThread {
                    topic: world.topic,
                    name: name.to_owned(),
                    agent: world.agent,
                    provider: world.provider,
                    environment,
                    directory: "web".to_owned(),
                    arrangement: kind,
                    model: Some(common::MODEL.to_owned()),
                    effort: None,
                },
            )
            .await
            .expect("a thread is created")
            .thread
    }

    async fn say(&self, thread: Id, words: &str) {
        self.world
            .engine()
            .submit_input(&Caller::session(self.world.ada), thread, queued(words))
            .await
            .unwrap();
    }

    /// The world's environment works through Orch's executor: its connector reports the executor, as a unit's
    /// does once attached, and its template offers it, as a template that names its offers does.
    async fn offers_its_executor(&self) {
        let pool = self.world.pool.pool();
        sqlx::query("UPDATE environments SET facts = $2 WHERE id = $1")
            .bind(self.world.environment)
            .bind(hosting::facts())
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE env_templates SET spec = spec || $2
              WHERE id = (SELECT template_id FROM environments WHERE id = $1)",
        )
        .bind(self.world.environment)
        .bind(json!({ "exec": { "offers": ["plain"] } }))
        .execute(pool)
        .await
        .unwrap();
    }

    /// A host declares the driver of the environment's template, with units that are started and stopped from
    /// here: the environment is a managed one with a unit, not a directory on a person's machine.
    async fn hosted_with_units(&self) {
        let mut conn = self.world.pool.acquire().await.unwrap();
        let machine =
            orch_db::queries::resources::testing::insert_machine(&mut conn, "node", "managed")
                .await
                .unwrap();
        let host = new_id();
        orch_db::queries::resources::insert_host(
            &mut conn,
            &orch_db::queries::resources::NewHost {
                id: host,
                machine_id: machine,
                owner_id: Some(self.world.ada),
                home_id: Some(self.world.root),
                name: "node",
            },
            Utc::now(),
        )
        .await
        .unwrap();
        sqlx::query("UPDATE hosts SET drivers = $2 WHERE id = $1")
            .bind(host)
            .bind(json!({ "test": { "name": "test", "title": "Test", "lifecycle": "unit" } }))
            .execute(&mut *conn)
            .await
            .unwrap();
    }

    /// The `nth` turn start the harness machine of the environment was asked for a thread.
    async fn turn_start(&self, thread: Id, nth: usize) -> TurnStart {
        let mut found = None;
        eventually(
            "the harness in the environment to be asked for the turn",
            || {
                found = self
                    .env
                    .seen
                    .all()
                    .into_iter()
                    .filter_map(|command| match command {
                        Command::TurnStart(start) if start.thread_id == thread => Some(start),
                        _ => None,
                    })
                    .nth(nth);
                let done = found.is_some();
                async move { done }
            },
        )
        .await;
        found.unwrap()
    }

    /// The harness in the environment says something about a thread's session.
    async fn env_says(&self, thread: Id, event: Event) {
        let segment = self.world.segment_of(thread).await;
        self.env
            .machine
            .publish(
                &Stream::Session {
                    thread_id: thread,
                    segment_id: segment,
                },
                &event,
            )
            .await;
    }

    /// The turn is taken up and ends, with a reply.
    async fn finish_turn(&self, thread: Id, start: &TurnStart) {
        self.env_says(thread, turn_began(start.turn_id)).await;
        self.env_says(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
            .await;
        self.world.wait_turn(start.turn_id, "completed").await;
        self.world.wait_state(thread, "idle").await;
    }

    /// What the machines of the world ran, in order, as (which machine, which command).
    fn order(&self) -> Vec<(&'static str, K)> {
        self.world
            .ordered()
            .into_iter()
            .map(|(machine, command)| {
                let which = if machine == self.env.id {
                    "environment"
                } else if machine == self.world.machine_id {
                    "provider"
                } else {
                    "other"
                };
                (which, command.kind())
            })
            .collect()
    }

    /// The commands of a kind that the world ran, as bodies.
    fn bodies(&self, kind: K) -> Vec<Json> {
        self.world
            .ordered()
            .into_iter()
            .filter(|(_, command)| command.kind() == kind)
            .map(|(_, command)| command.body().unwrap())
            .collect()
    }

    async fn holder(&self) -> Option<Id> {
        sqlx::query_scalar("SELECT held_by_environment_id FROM providers WHERE id = $1")
            .bind(self.world.provider)
            .fetch_one(self.world.pool.pool())
            .await
            .unwrap()
    }

    async fn audited(&self, action: &str) -> Vec<Json> {
        sqlx::query_scalar("SELECT details FROM audit_log WHERE action = $1 ORDER BY at")
            .bind(action)
            .fetch_all(self.world.pool.pool())
            .await
            .unwrap()
    }
}

/// The relevant part of what the start of a turn on a credential that leaves the provider asked of the
/// machines, in the order they were asked. The provider's machine keeps the login and issues the copy
/// itself, so no harness is started there: a keeper's own harness is started first only where the keeper
/// is a machine apart from the provider's runtime.
fn start_of_a_turn() -> Vec<(&'static str, K)> {
    vec![
        ("environment", K::AgentStart),
        ("provider", K::LeaseOpen),
        ("provider", K::ProviderCredentialIssue),
        ("environment", K::AgentCredentialSet),
        ("environment", K::SessionOpen),
        ("environment", K::TurnStart),
    ]
}

// ---------------------------------------------------------------- a copy that is shared

#[tokio::test]
async fn a_native_credential_kept_elsewhere_uses_a_declared_copy_at_its_separate_runtime() {
    let s = stand().await;
    s.offers_its_executor().await;
    sqlx::query("UPDATE providers SET machine_id=$2,key_machine_id=$3 WHERE id=$1")
        .bind(s.world.provider)
        .bind(s.env.id)
        .bind(s.world.machine_id)
        .execute(s.world.pool.pool())
        .await
        .unwrap();
    let thread = s
        .thread_in(
            s.world.environment,
            Some(ArrangementType::Separated),
            "Native runtime elsewhere",
        )
        .await;
    s.say(thread, "use the independently selected runtime")
        .await;
    let start = s.turn_start(thread, 0).await;
    let lease = &s.bodies(K::LeaseOpen)[0];
    assert_eq!(lease["mode"], "shared");
    assert_eq!(lease["form"], "access_token");
    assert_eq!(lease["recipient"]["machine_id"], json!(s.env.id));
    assert_eq!(
        s.bodies(K::AgentCredentialSet)[0]["custodian"]["machine_id"],
        json!(s.world.machine_id)
    );
    assert!(
        s.world
            .ordered()
            .iter()
            .any(|(machine, command)| *machine == s.world.machine_id
                && command.kind() == K::ProviderCredentialIssue)
    );
    let open = &s.bodies(K::SessionOpen)[0];
    assert_eq!(open["provider"]["path"], "shared");
    assert_eq!(open["provider"]["form"], "access_token");
    assert_eq!(
        open["exec"]["environment_id"],
        json!(s.world.environment),
        "the executor environment stays independent of the harness runtime"
    );
    let starts: Vec<(Id, Json)> = s
        .world
        .ordered()
        .into_iter()
        .filter(|(_, command)| command.kind() == K::AgentStart)
        .map(|(machine, command)| (machine, command.body().unwrap()))
        .collect();
    assert_eq!(
        starts.len(),
        2,
        "the runtime and native keeper each start their own harness"
    );
    // The runtime that receives the copy works in a home Orch made, never in a person's own; the keeper
    // keeps the login where its profile has it (here nothing is named: its harness's own home), and the
    // runtime's choice never redirects it.
    for (machine, body) in &starts {
        if *machine == s.env.id {
            assert_eq!(
                body["runtime"]["home"]["kind"], "made_by_orch",
                "the receiver never silently borrows a person's CLI home: {body}"
            );
        } else {
            assert_eq!(*machine, s.world.machine_id);
            assert!(
                body["runtime"]["home"].is_null(),
                "the keeper keeps its profile's home: {body}"
            );
        }
    }
    s.finish_turn(thread, &start).await;
    s.world.wait_ran(K::LeaseClose, 1).await;
    let descriptor = orch_driver_contract::descriptor::DriverDescriptor::registered_directories(
        "test",
        orch_driver_contract::descriptor::EnvironmentDataStorage {
            made_by_orch: Some(orch_driver_contract::descriptor::MadeDataKind::Directory),
            existing_directory: true,
            reason: None,
        },
    );
    sqlx::query("UPDATE machines SET facts=facts || $2 WHERE id=$1")
        .bind(s.env.id)
        .bind(json!({"provider_runtime_descriptor":descriptor}))
        .execute(s.world.pool.pool())
        .await
        .unwrap();
    s.world
        .engine()
        .stop_provider_runtime(s.world.provider, false, StopWhy::Person(s.world.ada))
        .await
        .unwrap();
    let mut conn = s.world.pool.acquire().await.unwrap();
    let keeper = orch_db::queries::resources::thread_engine::harness(
        &mut conn,
        s.world.provider,
        s.world.agent,
        s.world.machine_id,
    )
    .await
    .unwrap()
    .unwrap();
    let runtime = orch_db::queries::resources::thread_engine::harness(
        &mut conn,
        s.world.provider,
        s.world.agent,
        s.env.id,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        keeper.status, "online",
        "detaching a runtime leaves its independently selected native keeper running"
    );
    assert_eq!(runtime.status, "offline");
}

#[tokio::test]
async fn selecting_a_native_runtime_elsewhere_never_invents_a_shared_form_or_authorizes_its_export()
{
    for no_form in [true, false] {
        let s = stand().await;
        s.offers_its_executor().await;
        sqlx::query("UPDATE providers SET machine_id=$2,key_machine_id=$3 WHERE id=$1")
            .bind(s.world.provider)
            .bind(s.env.id)
            .bind(s.world.machine_id)
            .execute(s.world.pool.pool())
            .await
            .unwrap();
        if no_form {
            let mut descriptor = declared();
            descriptor["credentials"]["issues"] = json!([]);
            sqlx::query("UPDATE agents SET descriptor=$2 WHERE id=$1")
                .bind(s.world.agent)
                .bind(descriptor)
                .execute(s.world.pool.pool())
                .await
                .unwrap();
        } else {
            sqlx::query("UPDATE providers SET arrangements=ARRAY['separated'] WHERE id=$1")
                .bind(s.world.provider)
                .execute(s.world.pool.pool())
                .await
                .unwrap();
        }
        let result = s
            .world
            .engine()
            .create_thread(
                &Caller::session(s.world.ada),
                NewThread {
                    topic: s.world.topic,
                    name: "A runtime does not grant credential access".into(),
                    agent: s.world.agent,
                    provider: s.world.provider,
                    environment: s.world.environment,
                    directory: "web".into(),
                    arrangement: Some(ArrangementType::Separated),
                    model: Some(common::MODEL.into()),
                    effort: None,
                },
            )
            .await;
        if no_form {
            assert!(
                matches!(
                    result,
                    Err(CreateThreadError::Incompatible(
                        Incompatible::NoCredentialForm
                    ))
                ),
                "{result:?}"
            );
        } else {
            assert!(
                matches!(
                    result,
                    Err(CreateThreadError::Incompatible(
                        Incompatible::ArrangementNotAllowed {
                            by: NotAllowedBy::Provider
                        }
                    ))
                ),
                "{result:?}"
            );
        }
        assert!(s.bodies(K::LeaseOpen).is_empty());
        assert!(s.bodies(K::ProviderCredentialIssue).is_empty());
    }
}

#[tokio::test]
async fn a_copy_is_issued_sealed_to_the_machine_of_the_harness_and_given_to_it_before_its_session_opens()
 {
    let s = stand().await;
    let thread = s.thread(ArrangementType::CredentialShared, "shared").await;
    s.say(thread, "hello").await;
    let start = s.turn_start(thread, 0).await;
    let relevant: Vec<_> = s
        .order()
        .into_iter()
        .filter(|(_, kind)| !matches!(kind, K::ProviderStart | K::ProviderStop))
        .collect();
    assert_eq!(relevant, start_of_a_turn());
    // The lease is the copy's: the form, the one machine it may go to, and the turn it is for.
    let lease = &s.bodies(K::LeaseOpen)[0];
    assert_eq!(lease["mode"], "shared");
    assert_eq!(lease["form"], "access_token");
    assert_eq!(lease["recipient"]["machine_id"], json!(s.env.id));
    let issue = &s.bodies(K::ProviderCredentialIssue)[0];
    assert_eq!(issue["lease_id"], lease["lease_id"]);
    assert_eq!(issue["recipient"]["machine_id"], json!(s.env.id));
    // What the provider's runtime sealed is what the harness is given, and the machine that keeps the
    // credential is named as the custodian.
    let set = &s.bodies(K::AgentCredentialSet)[0];
    assert_eq!(set["credential"]["$sealed"], sealed_for(s.env.id));
    assert_eq!(set["form"], "access_token");
    assert_eq!(set["custodian"]["machine_id"], json!(s.world.machine_id));
    assert!(
        set["expires_at"].is_string(),
        "a copy that lapses says when"
    );
    let open = &s.bodies(K::SessionOpen)[0];
    assert_eq!(open["provider"]["path"], "shared");
    assert_eq!(open["provider"]["form"], "access_token");
    // The provider stays where it is: nothing was handed over.
    assert_eq!(s.holder().await, None);
    let row: (String, Option<String>, Option<Id>, Option<Id>) = sqlx::query_as(
        "SELECT mode, form, recipient_machine_id, turn_id FROM leases WHERE thread_id = $1",
    )
    .bind(thread)
    .fetch_one(s.world.pool.pool())
    .await
    .unwrap();
    assert_eq!(row.0, "shared");
    assert_eq!(row.1.as_deref(), Some("access_token"));
    assert_eq!(row.2, Some(s.env.id));
    assert_eq!(row.3, Some(start.turn_id));
    // Every issue is written down, with no value in it.
    let issued = s.audited("provider.credential_issued").await;
    assert_eq!(issued.len(), 1);
    assert_eq!(issued[0]["form"], "access_token");
    assert_eq!(issued[0]["machine"], json!(s.env.id));
    assert!(
        !issued[0].to_string().contains("sealed-for"),
        "what was issued is not in the trail"
    );
    // The lease ends with the turn, and the provider's runtime is told.
    s.finish_turn(thread, &start).await;
    s.world.wait_ran(K::LeaseClose, 1).await;
    let open: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar("SELECT closed_at FROM leases WHERE thread_id = $1")
            .bind(thread)
            .fetch_one(s.world.pool.pool())
            .await
            .unwrap();
    assert!(open.is_some());
}

#[tokio::test]
async fn a_harness_that_asks_for_a_credential_again_within_thirty_seconds_has_its_turn_failed() {
    let s = stand().await;
    let thread = s.thread(ArrangementType::CredentialShared, "shared").await;
    s.say(thread, "hello").await;
    let start = s.turn_start(thread, 0).await;
    s.env_says(thread, turn_began(start.turn_id)).await;
    s.world.wait_turn(start.turn_id, "running").await;
    // It was given a copy a moment ago, and says it needs another.
    s.env_says(
        thread,
        Event::CredentialNeeded(CredentialNeeded {
            reason: CredentialNeed::Rejected,
        }),
    )
    .await;
    s.world.wait_turn(start.turn_id, "failed").await;
    let error: Json = sqlx::query_scalar("SELECT error FROM turns WHERE id = $1")
        .bind(start.turn_id)
        .fetch_one(s.world.pool.pool())
        .await
        .unwrap();
    assert_eq!(error["code"], "provider_unauthorized", "{error}");
    assert_eq!(
        s.bodies(K::ProviderCredentialIssue).len(),
        1,
        "no second copy was issued so soon after the first"
    );
}

#[tokio::test]
async fn a_shared_copy_is_renewed_before_expiry_after_an_api_restart_without_a_harness_request() {
    let clock = orch_testkit::clock::manual();
    let s = stand_on(World::with_clock(Arc::new(clock.clone())).await).await;
    let thread = s
        .thread(ArrangementType::CredentialShared, "shared after restart")
        .await;
    s.say(thread, "Keep working").await;
    let start = s.turn_start(thread, 0).await;
    s.env_says(thread, turn_began(start.turn_id)).await;
    s.world.wait_turn(start.turn_id, "running").await;
    let first = s.bodies(K::AgentCredentialSet)[0]["expires_at"].clone();
    let durable: Option<chrono::DateTime<Utc>> = sqlx::query_scalar(
        "SELECT copy_expires_at FROM leases WHERE thread_id = $1 AND closed_at IS NULL",
    )
    .bind(thread)
    .fetch_one(s.world.pool.pool())
    .await
    .unwrap();
    assert_eq!(
        durable.map(Time::new),
        serde_json::from_value::<Option<Time>>(first.clone()).unwrap()
    );
    s.world.restart_api().await;
    assert_eq!(s.bodies(K::ProviderCredentialIssue).len(), 1);
    // Copies last an hour and the shipped renewal margin is five minutes: 56 minutes is inside it.
    clock.advance(chrono::Duration::minutes(56));
    eventually("a new copy is issued proactively after restart", || async {
        s.audited("provider.credential_issued").await.len() >= 2
    })
    .await;
    let given = s.bodies(K::AgentCredentialSet);
    assert_ne!(
        given[1]["expires_at"], first,
        "a new command must not replay the old expired result"
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT copy_issued_count FROM leases WHERE thread_id = $1 AND closed_at IS NULL",
    )
    .bind(thread)
    .fetch_one(s.world.pool.pool())
    .await
    .unwrap();
    assert_eq!(count, 2);
    assert_eq!(s.world.turn_state(start.turn_id).await, "running");
}

#[tokio::test]
async fn a_thread_on_a_shared_copy_is_refused_when_the_provider_owner_allows_only_the_other_types()
{
    let s = stand().await;
    sqlx::query("UPDATE providers SET arrangements = ARRAY['separated'] WHERE id = $1")
        .bind(s.world.provider)
        .execute(s.world.pool.pool())
        .await
        .unwrap();
    let made = s
        .world
        .engine()
        .create_thread(
            &Caller::session(s.world.ada),
            NewThread {
                topic: s.world.topic,
                name: "refused".to_owned(),
                agent: s.world.agent,
                provider: s.world.provider,
                environment: s.environment,
                directory: "web".to_owned(),
                arrangement: Some(ArrangementType::CredentialShared),
                model: Some(common::MODEL.to_owned()),
                effort: None,
            },
        )
        .await;
    assert!(
        made.is_err(),
        "an arrangement the provider's owner did not allow is not made"
    );
    assert!(s.bodies(K::ProviderCredentialIssue).is_empty());
}

// ---------------------------------------------------------------- the login handed over

#[tokio::test]
async fn the_login_is_taken_in_one_step_and_delivered_to_the_environment_and_a_second_thread_there_needs_nothing()
 {
    let s = stand().await;
    let first = s.thread(ArrangementType::HandedOver, "first").await;
    let second = s.thread(ArrangementType::HandedOver, "second").await;
    s.say(first, "hello").await;
    let start = s.turn_start(first, 0).await;
    let relevant: Vec<_> = s
        .order()
        .into_iter()
        .filter(|(_, kind)| !matches!(kind, K::ProviderStart | K::ProviderStop))
        .collect();
    assert_eq!(relevant, start_of_a_turn());
    assert_eq!(
        s.holder().await,
        Some(s.environment),
        "the environment holds it"
    );
    let lease = &s.bodies(K::LeaseOpen)[0];
    assert_eq!(lease["mode"], "handover");
    assert_eq!(lease["form"], "login");
    assert_eq!(s.bodies(K::ProviderCredentialIssue)[0]["form"], "login");
    assert_eq!(s.bodies(K::AgentCredentialSet)[0]["form"], "login");
    assert_eq!(s.bodies(K::SessionOpen)[0]["provider"]["path"], "handover");
    let taken = s.audited("provider.handed_over").await;
    assert_eq!(taken.len(), 1);
    assert_eq!(taken[0]["environment"], json!(s.environment));
    // It was delivered: the lease under which it was taken says where.
    let recipient: Option<Id> = sqlx::query_scalar(
        "SELECT recipient_machine_id FROM leases WHERE thread_id = $1 AND mode = 'handover'",
    )
    .bind(first)
    .fetch_one(s.world.pool.pool())
    .await
    .unwrap();
    assert_eq!(recipient, Some(s.env.id));
    s.finish_turn(first, &start).await;
    // A second thread in the same environment finds the login there: the same harness process runs it.
    s.say(second, "and me").await;
    let again = s.turn_start(second, 0).await;
    assert_eq!(again.thread_id, second);
    assert_eq!(s.bodies(K::ProviderCredentialIssue).len(), 1, "issued once");
    assert_eq!(s.bodies(K::AgentCredentialSet).len(), 1, "given once");
    assert_eq!(
        s.audited("provider.handed_over").await.len(),
        1,
        "taken once"
    );
}

#[tokio::test]
async fn a_thread_in_another_environment_waits_while_the_login_is_away_and_a_taker_waits_while_other_turns_run()
 {
    let s = stand().await;
    // A thread on the provider's own machine, in the world's environment, whose turn keeps running.
    let at_home = s.thread_in(s.world.environment, None, "at home").await;
    let elsewhere = s.thread(ArrangementType::HandedOver, "elsewhere").await;
    let other = s.thread_in(s.world.environment, None, "other").await;
    s.world
        .engine()
        .submit_input(&Caller::session(s.world.ada), at_home, queued("busy"))
        .await
        .unwrap();
    let busy = s.world.turn_start(at_home, 0).await;
    s.world.say(at_home, turn_began(busy.turn_id)).await;
    s.world.wait_turn(busy.turn_id, "running").await;
    // Nobody's work is interrupted to hand a credential over: the thread waits, and says why.
    s.say(elsewhere, "take it").await;
    s.world
        .wait_blocked(elsewhere, "provider_unavailable")
        .await;
    let detail: Option<String> =
        sqlx::query_scalar("SELECT blocked_detail FROM threads WHERE node_id = $1")
            .bind(elsewhere)
            .fetch_one(s.world.pool.pool())
            .await
            .unwrap();
    assert_eq!(detail.as_deref(), Some("in_use"));
    assert_eq!(s.holder().await, None, "nothing was taken");
    assert!(s.bodies(K::ProviderCredentialIssue).is_empty());
    // When those turns end, the thread goes on, and takes it.
    s.world
        .say(at_home, turn_ended(busy.turn_id, TurnOutcome::Completed))
        .await;
    s.world.wait_turn(busy.turn_id, "completed").await;
    s.turn_start(elsewhere, 0).await;
    assert_eq!(s.holder().await, Some(s.environment));
    // Another environment's threads on the provider now wait for it to come home.
    s.world
        .engine()
        .submit_input(&Caller::session(s.world.ada), other, queued("me too"))
        .await
        .unwrap();
    s.world.wait_blocked(other, "provider_unavailable").await;
    let detail: Option<String> =
        sqlx::query_scalar("SELECT blocked_detail FROM threads WHERE node_id = $1")
            .bind(other)
            .fetch_one(s.world.pool.pool())
            .await
            .unwrap();
    assert_eq!(detail.as_deref(), Some("handed_over"));
}

#[tokio::test]
async fn a_credential_is_brought_home_by_the_environment_giving_it_back_and_the_runtime_taking_it_and_not_while_a_turn_runs()
 {
    let s = stand().await;
    let thread = s.thread(ArrangementType::HandedOver, "away").await;
    s.say(thread, "hello").await;
    let start = s.turn_start(thread, 0).await;
    s.env_says(thread, turn_began(start.turn_id)).await;
    s.world.wait_turn(start.turn_id, "running").await;
    let provider = s.world.provider;
    // Refused while a turn of that environment runs on the provider.
    let busy = s
        .world
        .engine()
        .return_credential(
            provider,
            ReturnWhy::Person {
                by: s.world.ada,
                interrupt: false,
            },
        )
        .await;
    assert!(matches!(busy, Err(ReturnError::Busy(1))), "{busy:?}");
    assert_eq!(s.holder().await, Some(s.environment));
    s.finish_turn(thread, &start).await;
    s.world
        .engine()
        .return_credential(
            provider,
            ReturnWhy::Person {
                by: s.world.ada,
                interrupt: false,
            },
        )
        .await
        .unwrap();
    // The environment's harness gave it back, sealed to the runtime; the runtime was handed it as it was.
    let back = &s.bodies(K::AgentCredentialReturn)[0];
    assert_eq!(back["custodian"]["machine_id"], json!(s.world.machine_id));
    let accepted = &s.bodies(K::ProviderCredentialAccept)[0];
    assert_eq!(accepted["credential"]["$sealed"], RETURNED);
    assert_eq!(accepted["form"], "login");
    assert_eq!(s.holder().await, None);
    assert_eq!(s.audited("provider.returned").await.len(), 1);
    // It is not away any more.
    let again = s
        .world
        .engine()
        .return_credential(
            provider,
            ReturnWhy::Person {
                by: s.world.ada,
                interrupt: false,
            },
        )
        .await;
    assert!(matches!(again, Err(ReturnError::NotHeld)), "{again:?}");
}

#[tokio::test]
async fn a_return_that_fails_leaves_the_credential_where_it_is_and_is_tried_again_or_given_up_never_both()
 {
    let s = stand().await;
    let thread = s.thread(ArrangementType::HandedOver, "away").await;
    s.say(thread, "hello").await;
    let start = s.turn_start(thread, 0).await;
    s.finish_turn(thread, &start).await;
    let provider = s.world.provider;
    let by = ReturnWhy::Person {
        by: s.world.ada,
        interrupt: false,
    };
    // The environment cannot be reached: the credential stays where it is.
    lock(&s.return_refusals).push_back(CommandError::new(
        ErrorCode::OfflinePeer,
        "the environment is away",
    ));
    let failed = s.world.engine().return_credential(provider, by).await;
    let Err(ReturnError::Failed(words)) = failed else {
        panic!("a return that fails says so: {failed:?}");
    };
    assert!(words.contains("the environment is away"), "{words}");
    assert_eq!(s.holder().await, Some(s.environment));
    assert!(s.audited("provider.returned").await.is_empty());
    // Tried again, it comes home.
    s.world
        .engine()
        .return_credential(provider, by)
        .await
        .unwrap();
    assert_eq!(s.holder().await, None);
    // Taken again, and given up instead: the holder is cleared and the provider has to be signed in.
    let thread = s.thread(ArrangementType::HandedOver, "away again").await;
    s.say(thread, "again").await;
    let start = s.turn_start(thread, 0).await;
    s.finish_turn(thread, &start).await;
    assert_eq!(s.holder().await, Some(s.environment));
    s.world
        .engine()
        .declare_credential_lost(provider, s.world.ada)
        .await
        .unwrap();
    assert_eq!(s.holder().await, None);
    let (status, detail): (String, Option<String>) =
        sqlx::query_as("SELECT status, status_detail FROM providers WHERE id = $1")
            .bind(provider)
            .fetch_one(s.world.pool.pool())
            .await
            .unwrap();
    assert_eq!(status, "needs_setup");
    assert!(detail.unwrap_or_default().contains("signed in again"));
    assert_eq!(s.audited("provider.handover_lost").await.len(), 1);
    // Given up, it cannot also come home, nor be given up twice.
    let home = s.world.engine().return_credential(provider, by).await;
    assert!(matches!(home, Err(ReturnError::NotHeld)), "{home:?}");
    let twice = s
        .world
        .engine()
        .declare_credential_lost(provider, s.world.ada)
        .await;
    assert!(matches!(twice, Err(ReturnError::NotHeld)), "{twice:?}");
    assert_eq!(
        s.audited("provider.returned").await.len(),
        1,
        "the one return stands, and no second came after the loss"
    );
}

#[tokio::test]
async fn an_environment_is_not_stopped_before_its_credential_is_home_and_one_that_cannot_return_it_stays_up()
 {
    let s = stand().await;
    let thread = s.thread(ArrangementType::HandedOver, "away").await;
    s.say(thread, "hello").await;
    let start = s.turn_start(thread, 0).await;
    s.finish_turn(thread, &start).await;
    assert_eq!(s.holder().await, Some(s.environment));
    s.hosted_with_units().await;
    // The environment cannot give it back: it is not stopped, and says why.
    lock(&s.return_refusals).push_back(CommandError::new(
        ErrorCode::OfflinePeer,
        "the environment is away",
    ));
    let refused = s
        .world
        .engine()
        .stop_environment(s.environment, false, StopReason::Person(s.world.ada))
        .await;
    assert!(
        matches!(refused, Err(StopError::CredentialReturnFailed(_))),
        "{refused:?}"
    );
    let status: String = sqlx::query_scalar("SELECT status FROM environments WHERE id = $1")
        .bind(s.environment)
        .fetch_one(s.world.pool.pool())
        .await
        .unwrap();
    assert_eq!(status, "running");
    assert_eq!(s.holder().await, Some(s.environment));
    // Asked again, the credential comes home first, and then the environment stops.
    s.world
        .engine()
        .stop_environment(s.environment, false, StopReason::Person(s.world.ada))
        .await
        .unwrap();
    assert_eq!(s.holder().await, None);
    let status: String = sqlx::query_scalar("SELECT status FROM environments WHERE id = $1")
        .bind(s.environment)
        .fetch_one(s.world.pool.pool())
        .await
        .unwrap();
    assert_eq!(status, "stopped");
}

// ---------------------------------------------------------------- the sign-in is the provider's

#[tokio::test]
async fn what_a_person_does_to_a_sign_in_goes_to_the_providers_own_runtime_and_no_environment_is_ever_signed_in()
 {
    let s = stand().await;
    let (world, provider) = (&s.world, s.world.provider);
    // The provider's runtime answers what a sign-in asks of it. The environment has no answer for any of
    // it: were it asked, the test would see the question.
    let signing = [
        K::ProviderLoginStart,
        K::ProviderLoginInput,
        K::ProviderLoginCancel,
        K::ProviderLogout,
    ];
    for kind in signing {
        script(
            world,
            &world.machine,
            world.machine_id,
            kind,
            |command| match command {
                Command::ProviderLoginStart(_) => Ok(CommandResult::ProviderLoginStart(Empty {})),
                Command::ProviderLoginInput(_) => Ok(CommandResult::ProviderLoginInput(Empty {})),
                Command::ProviderLoginCancel(_) => Ok(CommandResult::ProviderLoginCancel(Empty {})),
                Command::ProviderLogout(_) => Ok(CommandResult::ProviderLogout(Empty {})),
                other => Err(CommandError::new(
                    ErrorCode::Unsupported,
                    format!("no script for {}", other.kind()),
                )),
            },
        );
    }
    // The environment holds the credential in both of the ways there are: a copy for a turn that is over,
    // and the login itself.
    let shared = s.thread(ArrangementType::CredentialShared, "shared").await;
    s.say(shared, "one").await;
    let start = s.turn_start(shared, 0).await;
    s.finish_turn(shared, &start).await;
    let handed = s.thread(ArrangementType::HandedOver, "handed").await;
    s.say(handed, "two").await;
    let start = s.turn_start(handed, 0).await;
    s.finish_turn(handed, &start).await;
    assert_eq!(
        s.holder().await,
        Some(s.environment),
        "the login is in the environment"
    );
    // A person signs the provider in by a method, answers what it asked, gives that up and signs out.
    let mut conn = world.pool.acquire().await.unwrap();
    let pending = Pending::new("device_code", &BTreeMap::new(), world.ada);
    rows::keep_pending(
        &mut conn,
        provider,
        &pending.to_bytes().unwrap(),
        Utc::now(),
    )
    .await
    .unwrap();
    drop(conn);
    let delivered = world.engine().deliver_sign_in(provider).await.unwrap();
    assert!(matches!(delivered, Delivered::Sent), "{delivered:?}");
    let answer = BTreeMap::from([("code".to_owned(), SignInValue::Text("1234".to_owned()))]);
    let engine = world.engine();
    assert_eq!(
        engine.send_login_input(provider, &answer).await.unwrap(),
        Asked::Done
    );
    assert_eq!(engine.cancel_sign_in(provider).await.unwrap(), Asked::Done);
    assert_eq!(engine.sign_out(provider).await.unwrap(), Asked::Done);
    // Every one of them went to the machine of the provider's own runtime, in the order they were made.
    let went: Vec<(Id, K)> = world
        .ordered()
        .into_iter()
        .map(|(machine, command)| (machine, command.kind()))
        .filter(|(_, kind)| signing.contains(kind))
        .collect();
    assert_eq!(
        went,
        signing.map(|kind| (world.machine_id, kind)).to_vec(),
        "each of the four, to the provider's runtime"
    );
    // And the environment, which holds the login, was never asked to sign in, to answer, to give up or
    // to sign out: it is given a credential and gives it back, and that is all.
    let asked_of_it: Vec<K> = s
        .env
        .seen
        .all()
        .into_iter()
        .map(|command| command.kind())
        .filter(|kind| signing.contains(kind))
        .collect();
    assert!(asked_of_it.is_empty(), "{asked_of_it:?}");
    assert_eq!(
        s.holder().await,
        Some(s.environment),
        "a sign-in in the runtime does not take the login from where it is"
    );
}

// ---------------------------------------------------------------- a unit that is stopped

/// The provider made a managed one whose unit is stopped, on a host that starts it when it is asked.
async fn with_a_stopped_unit(s: &Stand) -> (ScriptedHost, Id) {
    serve(&s.world);
    let host = ScriptedHost::on(&s.world, "host-1", 16_384).await;
    let template = provider_template(&s.world).await;
    let runtime = managed_world_provider(&s.world, &host, template).await;
    s.world
        .engine()
        .stop_provider_runtime(s.world.provider, false, StopWhy::Person(s.world.ada))
        .await
        .unwrap();
    (host, runtime)
}

/// The jobs of the provider: (mode, state), oldest first.
async fn jobs_of_the_provider(s: &Stand) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT COALESCE(plan ->> 'mode', ''), state FROM jobs WHERE provider_id = $1 ORDER BY created_at",
    )
    .bind(s.world.provider)
    .fetch_all(s.world.pool.pool())
    .await
    .unwrap()
}

async fn runtime_state(s: &Stand, runtime: Id) -> String {
    sqlx::query_scalar("SELECT state FROM runtimes WHERE id = $1")
        .bind(runtime)
        .fetch_one(s.world.pool.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn a_turn_that_needs_a_credential_from_a_stopped_unit_starts_the_unit_first_and_then_goes_on()
{
    let s = stand().await;
    let (host, runtime) = with_a_stopped_unit(&s).await;
    assert_eq!(runtime_state(&s, runtime).await, "stopped");
    let machine: Option<Id> = sqlx::query_scalar("SELECT machine_id FROM providers WHERE id = $1")
        .bind(s.world.provider)
        .fetch_one(s.world.pool.pool())
        .await
        .unwrap();
    assert_eq!(
        machine, None,
        "a stopped unit leaves the provider without a machine"
    );
    let started_before = host.count(K::NodeRuntimeStart);
    // Nobody asks for the unit: a person says something to a thread whose credential is in it.
    let thread = s.thread(ArrangementType::CredentialShared, "shared").await;
    s.say(thread, "hello").await;
    let start = s.turn_start(thread, 0).await;
    assert_eq!(start.thread_id, thread);
    assert_eq!(
        host.count(K::NodeRuntimeStart),
        started_before + 1,
        "the unit is started once, for the turn"
    );
    assert_eq!(host.count(K::NodeRuntimeCreate), 0, "and not made again");
    assert_eq!(runtime_state(&s, runtime).await, "running");
    let machine: Option<Id> = sqlx::query_scalar("SELECT machine_id FROM providers WHERE id = $1")
        .bind(s.world.provider)
        .fetch_one(s.world.pool.pool())
        .await
        .unwrap();
    assert_eq!(
        machine,
        Some(s.world.machine_id),
        "the provider has its machine again"
    );
    assert_eq!(
        jobs_of_the_provider(&s).await,
        [("start".to_owned(), "succeeded".to_owned())],
        "the one job that started it"
    );
    // The copy was issued by the unit that was started, and then given to the harness, as in every turn on
    // a copy: the unit is the provider's own runtime and keeper, so the turn starts no harness there. The
    // unit's source was not read while it was stopped, so the reader of the provider's facts looks at it
    // once it is up, through its harness, on the reader's own schedule: that start is no part of the turn.
    let relevant: Vec<_> = s
        .order()
        .into_iter()
        .filter(|(which, kind)| {
            !matches!(kind, K::ProviderStart | K::ProviderStop)
                && (*which, *kind) != ("provider", K::AgentStart)
        })
        .collect();
    assert_eq!(relevant, start_of_a_turn());
}

#[tokio::test]
async fn a_turn_whose_unit_cannot_be_started_fails_with_the_hosts_words_and_its_input_waits_again()
{
    let s = stand().await;
    let (host, runtime) = with_a_stopped_unit(&s).await;
    host.refuse(
        K::NodeRuntimeStart,
        CommandError::new(ErrorCode::Internal, "the host has no room"),
    );
    let thread = s.thread(ArrangementType::CredentialShared, "shared").await;
    s.say(thread, "hello").await;
    s.world.wait_blocked(thread, "provider_unavailable").await;
    let detail: Option<String> =
        sqlx::query_scalar("SELECT blocked_detail FROM threads WHERE node_id = $1")
            .bind(thread)
            .fetch_one(s.world.pool.pool())
            .await
            .unwrap();
    let detail = detail.unwrap_or_default();
    assert!(
        detail.contains("the host has no room"),
        "the host's own words are the thread's reason: {detail}"
    );
    let jobs = jobs_of_the_provider(&s).await;
    assert_eq!(jobs, [("start".to_owned(), "failed".to_owned())]);
    assert_ne!(runtime_state(&s, runtime).await, "running");
    // Nothing was asked of the harness, and what the person said is kept to be tried again.
    assert!(
        s.env
            .seen
            .all()
            .iter()
            .all(|command| !matches!(command.kind(), K::TurnStart | K::AgentCredentialSet)),
        "the environment was not asked to run the turn"
    );
    let waiting: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM thread_inputs WHERE thread_id = $1 AND state = 'pending'",
    )
    .bind(thread)
    .fetch_one(s.world.pool.pool())
    .await
    .unwrap();
    assert_eq!(waiting, 1);
    let turns: Vec<String> = sqlx::query_scalar("SELECT state FROM turns WHERE thread_id = $1")
        .bind(thread)
        .fetch_all(s.world.pool.pool())
        .await
        .unwrap();
    assert_eq!(turns, ["failed"]);
}
