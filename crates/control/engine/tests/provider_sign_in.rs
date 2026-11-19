//! Signing a managed provider in: given to the harness of the provider's own profile in the provider's runtime by reference to a sealed row, and nowhere else; waiting for a runtime that is not up and forgotten with its secrets if it never is; what the harness shows and asks kept as it comes; an answer sent back; a sign-in given up, and the harness signed out; a sign-in that is kept fresh while nothing runs, and a token whose end is told in advance.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;
mod runtimes;

use chrono::{Duration, Utc};
use common::{World, eventually};
use hosting::{ScriptedHost, Unit, phases, serve, wait_over};
use orch_agent_contract::login::{LoginShow, LoginState, LoginStateKind, ShowKind};
use orch_db::queries::resources::provider_runtime as rows;
use orch_engine::provider_runtime::{RuntimeStart, StopWhy};
use orch_engine::provider_setup::{Asked, Delivered, Pending, Renewed, SIGNED_OUT, SignInValue};
use orch_link_contract::commands::{
    AgentStartResult, Command, CommandError, CommandKind as K, CommandResult, Empty, ErrorCode,
    ProviderRefreshResult,
};
use orch_link_contract::events::Event;
use orch_link_contract::streams::Stream;
use orch_types::{Field, FieldType, Id, Json, Time};
use runtimes::{managed_subscription, provider_template};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

const SECRET: &str = "sk-ant-oat01-never-to-be-kept";

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The descriptor the unit's harness answers `agent.start` with: the tests' agent, with the ways of
/// signing in of a harness that has a token to bring and a code to enter.
fn declared() -> Json {
    let mut descriptor = common::descriptor();
    descriptor["native_provider"]["login_methods"] = json!([
        {
            "id": "setup_token", "title": "Bring a token", "summary": "Paste the token.",
            "kind": "import", "yields": "token", "lifetime_secs": 31_536_000,
            "requires": null, "recommended": false,
            "fields": [
                { "name": "token", "label": "Token", "type": "secret", "required": true,
                  "help": null, "choices": null, "default": null, "max_bytes": null },
                { "name": "note", "label": "Note", "type": "text", "required": false,
                  "help": null, "choices": null, "default": null, "max_bytes": null }
            ]
        },
        {
            "id": "device_code", "title": "Sign in with a code", "summary": "Shows a code.",
            "kind": "interactive", "yields": "login", "lifetime_secs": null,
            "requires": null, "recommended": true, "fields": []
        }
    ]);
    descriptor
}

/// What a unit's harness was asked, in order, with the body of each.
type Heard = Arc<Mutex<Vec<(K, Json)>>>;

/// The unit's harness answers what a sign-in asks of it, and keeps what it was asked. A look at its
/// sign-in finds `status`.
fn listen(unit: &Unit, status: &'static str, facts: Json) -> Heard {
    let heard: Heard = Arc::new(Mutex::new(Vec::new()));
    let handler = &unit.added.machine.handler;
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
        let facts = facts.clone();
        handler.on(kind, move |command: &Command| {
            lock(&heard).push((kind, command.body().unwrap()));
            Ok(match command {
                Command::ProviderLoginStart(_) => CommandResult::ProviderLoginStart(Empty {}),
                Command::ProviderLoginInput(_) => CommandResult::ProviderLoginInput(Empty {}),
                Command::ProviderLoginCancel(_) => CommandResult::ProviderLoginCancel(Empty {}),
                Command::ProviderLogout(_) => CommandResult::ProviderLogout(Empty {}),
                Command::ProviderRefresh(_) => {
                    CommandResult::ProviderRefresh(ProviderRefreshResult {
                        source: None,
                        status: status.to_owned(),
                        facts: serde_json::from_value(facts.clone()).unwrap(),
                    })
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
    heard
}

async fn made(world: &World, host: &ScriptedHost, provider: Id) -> (Id, Unit) {
    let before = host.count(K::NodeRuntimeStart);
    let job = runtimes::queue(
        world,
        provider,
        orch_engine::jobs::provider_provision::Mode::Make,
    )
    .await;
    host.wait_for(K::NodeRuntimeStart, before + 1).await;
    let runtime = host.started().unwrap();
    let unit = runtimes::enrol(world, runtime).await;
    // The harness declaration exists from its first launch, including the immediate provider read.
    listen(&unit, "ready", json!({}));
    let (state, _, error) = wait_over(world, job).await;
    assert_eq!(state, "succeeded", "{error:?}");
    eventually("the initial provider read to complete", || async {
        sqlx::query_scalar::<_, bool>("SELECT facts_at IS NOT NULL FROM providers WHERE id = $1")
            .bind(provider)
            .fetch_one(world.pool.pool())
            .await
            .unwrap()
    })
    .await;
    (runtime, unit)
}

fn asked(heard: &Heard, kind: K) -> Vec<Json> {
    lock(heard)
        .iter()
        .filter(|(each, _)| *each == kind)
        .map(|(_, body)| body.clone())
        .collect()
}

/// What a person gave, kept for the runtime the way the request keeps it: the secret in a row of its
/// own, and the rest in the record that names them.
async fn wait_for_runtime(world: &World, provider: Id, method: &str, token: &str) -> Id {
    let mut conn = world.pool.acquire().await.unwrap();
    let secret = common::put_secret(&mut conn, provider, "login:token", token).await;
    let values = BTreeMap::from([
        ("token".to_owned(), SignInValue::Secret(secret)),
        (
            "note".to_owned(),
            SignInValue::Text("for the team".to_owned()),
        ),
    ]);
    rows::keep_pending(
        &mut conn,
        provider,
        &Pending::new(method, &values, world.ada).to_bytes().unwrap(),
        Utc::now(),
    )
    .await
    .unwrap();
    secret
}

async fn login_state(world: &World, provider: Id) -> Option<Json> {
    sqlx::query_scalar("SELECT login_state FROM providers WHERE id = $1")
        .bind(provider)
        .fetch_one(world.pool.pool())
        .await
        .unwrap()
}

async fn kept(world: &World, provider: Id) -> (bool, i64) {
    sqlx::query_as(
        "SELECT login_pending IS NOT NULL,
                (SELECT count(*) FROM secrets WHERE owner_kind = 'provider' AND owner_id = $1
                    AND starts_with(name, 'login:'))
           FROM providers WHERE id = $1",
    )
    .bind(provider)
    .fetch_one(world.pool.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn a_sign_in_is_given_to_the_harness_in_the_runtime_by_reference_and_nothing_of_it_is_kept() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let _template = provider_template(&world).await;
    let provider = managed_subscription(&world, "Team Claude").await;
    let (_runtime, unit) = made(&world, &host, provider).await;
    let heard = listen(&unit, "ready", json!({}));
    let secret = wait_for_runtime(&world, provider, "setup_token", SECRET).await;
    let delivered = world.engine().deliver_sign_in(provider).await.unwrap();
    assert!(matches!(delivered, Delivered::Sent), "{delivered:?}");
    let told = asked(&heard, K::ProviderLoginStart);
    assert_eq!(told.len(), 1);
    assert_eq!(told[0]["method"], "setup_token");
    assert_eq!(told[0]["provider_id"], json!(provider));
    assert_eq!(told[0]["agent_id"], json!(world.agent));
    assert_eq!(
        told[0]["values"]["token"],
        json!({ "$secret": secret }),
        "the secret travels as a reference to its row; the link seals it to the machine"
    );
    assert_eq!(told[0]["values"]["note"], "for the team");
    assert!(
        !told[0].to_string().contains(SECRET),
        "the value is not in the command"
    );
    assert_eq!(
        kept(&world, provider).await,
        (false, 0),
        "the harness has it, and the row and the record that waited are gone"
    );
    assert_eq!(
        login_state(&world, provider).await.unwrap()["state"],
        "starting"
    );
    let tables = world.tables_holding(SECRET).await;
    assert!(tables.is_empty(), "the secret is in no table: {tables:?}");
}

#[tokio::test]
async fn a_sign_in_whose_unit_is_stopped_waits_for_it_and_is_given_to_the_harness_when_it_is_up() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let _template = provider_template(&world).await;
    let provider = managed_subscription(&world, "Team Claude").await;
    let (_runtime, unit) = made(&world, &host, provider).await;
    let heard = listen(&unit, "ready", json!({}));
    world
        .engine()
        .stop_provider_runtime(provider, false, StopWhy::Person(world.ada))
        .await
        .unwrap();
    wait_for_runtime(&world, provider, "setup_token", SECRET).await;
    let delivered = world.engine().deliver_sign_in(provider).await.unwrap();
    let Delivered::Waits(Some(job)) = delivered else {
        panic!("the unit is started and the sign-in waits for it: {delivered:?}");
    };
    assert_eq!(
        kept(&world, provider).await,
        (true, 1),
        "while the unit starts, the sign-in waits with its secret sealed in a row of its own"
    );
    let (state, progress, error) = wait_over(&world, job.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    assert_eq!(phases(&progress)[0].0, "start");
    eventually("the sign-in to be given to the harness", || async {
        !asked(&heard, K::ProviderLoginStart).is_empty()
    })
    .await;
    eventually("what waited to be forgotten", || async {
        kept(&world, provider).await == (false, 0)
    })
    .await;
}

#[tokio::test]
async fn a_sign_in_the_harness_has_no_way_for_is_refused_whole_and_forgotten_with_the_harnesss_words_kept()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let _template = provider_template(&world).await;
    let provider = managed_subscription(&world, "Team Claude").await;
    let (_runtime, unit) = made(&world, &host, provider).await;
    let heard = listen(&unit, "ready", json!({}));
    wait_for_runtime(&world, provider, "carrier_pigeon", SECRET).await;
    let delivered = world.engine().deliver_sign_in(provider).await.unwrap();
    let Delivered::Refused(why) = delivered else {
        panic!("a method the harness lacks is refused: {delivered:?}");
    };
    assert!(why.contains("carrier_pigeon"), "{why}");
    assert!(asked(&heard, K::ProviderLoginStart).is_empty());
    assert_eq!(kept(&world, provider).await, (false, 0));
    let state = login_state(&world, provider).await.unwrap();
    assert_eq!(state["state"], "failed");
    assert!(state["error"].as_str().unwrap().contains("carrier_pigeon"));
    // The harness refusing it in its own words: they are carried whole.
    unit.added
        .machine
        .handler
        .on(K::ProviderLoginStart, |_: &Command| {
            Err(CommandError::new(
                ErrorCode::Internal,
                "the token is not one this vendor issued",
            ))
        });
    wait_for_runtime(&world, provider, "setup_token", SECRET).await;
    let Delivered::Refused(words) = world.engine().deliver_sign_in(provider).await.unwrap() else {
        panic!("the harness refused it");
    };
    assert!(
        words.contains("the token is not one this vendor issued"),
        "{words}"
    );
    assert_eq!(kept(&world, provider).await, (false, 0));
    let tables = world.tables_holding(SECRET).await;
    assert!(tables.is_empty(), "{tables:?}");
}

#[tokio::test]
async fn what_a_sign_in_shows_and_asks_is_kept_as_the_harness_tells_it_and_the_answer_goes_by_reference()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let _template = provider_template(&world).await;
    let provider = managed_subscription(&world, "Team Claude").await;
    let (_runtime, unit) = made(&world, &host, provider).await;
    let heard = listen(&unit, "ready", json!({}));
    let mut waiting = LoginState::starting("device_code");
    waiting.state = LoginStateKind::AwaitingUser;
    waiting.shows = vec![
        LoginShow {
            kind: ShowKind::Uri,
            label: "Open this address".to_owned(),
            value: "https://vendor.invalid/device".to_owned(),
        },
        LoginShow {
            kind: ShowKind::Code,
            label: "Enter this code there".to_owned(),
            value: "ABCD-1234".to_owned(),
        },
    ];
    waiting.asks = Some(vec![Field::new(
        "answer",
        "What the page shows",
        FieldType::Secret,
    )]);
    waiting.expires_at = Some(Time::new(Utc::now() + Duration::minutes(15)));
    unit.added
        .machine
        .publish(
            &Stream::Provider {
                provider_id: provider,
            },
            &Event::ProviderLogin(waiting),
        )
        .await;
    eventually("the state to be kept", || async {
        login_state(&world, provider)
            .await
            .is_some_and(|state| state["state"] == "awaiting_user")
    })
    .await;
    let state = login_state(&world, provider).await.unwrap();
    assert_eq!(state["method"], "device_code");
    assert_eq!(state["shows"][1]["value"], "ABCD-1234");
    assert_eq!(state["asks"][0]["name"], "answer");
    assert!(state["at"].is_string(), "the moment it was heard is noted");
    // The answer: a secret in a row of its own, which the command refers to and which is gone after.
    let mut conn = world.pool.acquire().await.unwrap();
    let row = common::put_secret(&mut conn, provider, "login:answer", SECRET).await;
    drop(conn);
    let values = BTreeMap::from([("answer".to_owned(), SignInValue::Secret(row))]);
    let sent = world
        .engine()
        .send_login_input(provider, &values)
        .await
        .unwrap();
    assert_eq!(sent, Asked::Done);
    let told = asked(&heard, K::ProviderLoginInput);
    assert_eq!(told[0]["values"]["answer"], json!({ "$secret": row }));
    assert!(!told[0].to_string().contains(SECRET));
    assert_eq!(kept(&world, provider).await, (false, 0));
    assert!(world.tables_holding(SECRET).await.is_empty());
}

#[tokio::test]
async fn a_sign_in_given_up_is_cancelled_in_the_harness_and_what_waited_is_forgotten() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let _template = provider_template(&world).await;
    let provider = managed_subscription(&world, "Team Claude").await;
    let (_runtime, unit) = made(&world, &host, provider).await;
    let heard = listen(&unit, "ready", json!({}));
    wait_for_runtime(&world, provider, "setup_token", SECRET).await;
    assert_eq!(kept(&world, provider).await, (true, 1));
    let cancelled = world.engine().cancel_sign_in(provider).await.unwrap();
    assert_eq!(cancelled, Asked::Done);
    assert_eq!(asked(&heard, K::ProviderLoginCancel).len(), 1);
    assert_eq!(kept(&world, provider).await, (false, 0));
    // A runtime that is not up has nothing running to be told.
    world
        .engine()
        .stop_provider_runtime(provider, false, StopWhy::Person(world.ada))
        .await
        .unwrap();
    wait_for_runtime(&world, provider, "setup_token", SECRET).await;
    assert_eq!(
        world.engine().cancel_sign_in(provider).await.unwrap(),
        Asked::NotUp
    );
    assert_eq!(kept(&world, provider).await, (false, 0));
    assert_eq!(
        login_state(&world, provider).await.unwrap()["state"],
        "idle"
    );
}

#[tokio::test]
async fn a_sign_in_that_waited_longer_than_the_system_allows_for_its_runtime_is_forgotten_by_the_schedule()
 {
    let world = World::new().await;
    serve(&world);
    let _template = provider_template(&world).await;
    let provider = managed_subscription(&world, "Team Claude").await;
    wait_for_runtime(&world, provider, "setup_token", SECRET).await;
    // No host can make the runtime; the sign-in has waited for it since long ago.
    sqlx::query("UPDATE providers SET login_pending_at = $2 WHERE id = $1")
        .bind(provider)
        .bind(Utc::now() - Duration::hours(2))
        .execute(world.pool.pool())
        .await
        .unwrap();
    eventually("the sign-in to be given up", || async {
        kept(&world, provider).await == (false, 0)
    })
    .await;
    let state = login_state(&world, provider).await.unwrap();
    assert_eq!(state["state"], "failed");
    assert!(
        state["error"].as_str().unwrap().contains("waited too long"),
        "{state}"
    );
    assert!(world.tables_holding(SECRET).await.is_empty());
}

#[tokio::test]
async fn a_provider_signed_out_needs_a_sign_in_again_and_says_so() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let _template = provider_template(&world).await;
    let provider = managed_subscription(&world, "Team Claude").await;
    let (_runtime, unit) = made(&world, &host, provider).await;
    sqlx::query("UPDATE providers SET status = 'ready' WHERE id = $1")
        .bind(provider)
        .execute(world.pool.pool())
        .await
        .unwrap();
    let heard = listen(&unit, "needs_setup", json!({}));
    let done = world.engine().sign_out(provider).await.unwrap();
    assert_eq!(done, Asked::Done);
    assert_eq!(asked(&heard, K::ProviderLogout).len(), 1);
    let (status, detail): (String, Option<String>) =
        sqlx::query_as("SELECT status, status_detail FROM providers WHERE id = $1")
            .bind(provider)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(status, "needs_setup");
    assert_eq!(detail.as_deref(), Some(SIGNED_OUT));
    assert_eq!(
        login_state(&world, provider).await.unwrap()["state"],
        "idle"
    );
    // A runtime that is not up cannot be asked.
    world
        .engine()
        .stop_provider_runtime(provider, false, StopWhy::Person(world.ada))
        .await
        .unwrap();
    assert_eq!(
        world.engine().sign_out(provider).await.unwrap(),
        Asked::NotUp
    );
}

#[test]
fn completed_sign_ins_run_on_default_stacks_even_if_the_suite_stack_is_larger() {
    common::on_default_stack(
        "a_sign_in_that_completed_is_looked_at_by_the_schedule_and_the_provider_stands_where_the_harness_says",
    );
}

#[tokio::test]
async fn a_sign_in_that_completed_is_looked_at_by_the_schedule_and_the_provider_stands_where_the_harness_says()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let _template = provider_template(&world).await;
    let provider = managed_subscription(&world, "Team Claude").await;
    let (_runtime, unit) = made(&world, &host, provider).await;
    let heard = listen(
        &unit,
        "ready",
        json!({ "plan": "team", "windows": [], "identity": "ada@example.com" }),
    );
    unit.added
        .machine
        .publish(
            &Stream::Provider {
                provider_id: provider,
            },
            &Event::ProviderLogin(LoginState::completed("setup_token")),
        )
        .await;
    eventually("the facts to be asked after the sign-in", || async {
        !asked(&heard, K::ProviderRefresh).is_empty()
    })
    .await;
    eventually("the facts to be kept", || async {
        let facts: Json = sqlx::query_scalar("SELECT facts FROM providers WHERE id = $1")
            .bind(provider)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
        facts["identity"] == "ada@example.com"
    })
    .await;
    eventually("the sign-in to be noted as looked at", || async {
        login_state(&world, provider)
            .await
            .is_some_and(|state| state["looked_at"].is_string())
    })
    .await;
    let before = asked(&heard, K::ProviderRefresh).len();
    common::meanwhile(std::time::Duration::from_millis(600)).await;
    assert_eq!(
        asked(&heard, K::ProviderRefresh).len(),
        before,
        "a sign-in is looked at once"
    );
}

#[tokio::test]
async fn a_stopped_runtime_is_started_to_look_at_its_sign_in_and_a_sign_in_that_no_longer_works_is_told_to_its_managers()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let _template = provider_template(&world).await;
    let provider = managed_subscription(&world, "Team Claude").await;
    let (runtime, unit) = made(&world, &host, provider).await;
    let heard = listen(&unit, "needs_setup", json!({}));
    world
        .engine()
        .stop_provider_runtime(provider, false, StopWhy::Person(world.ada))
        .await
        .unwrap();
    let started = world
        .engine()
        .renew_provider_runtime(provider)
        .await
        .unwrap();
    let RuntimeStart::Job(job) = started else {
        panic!("a stopped runtime is started to renew its sign-in");
    };
    let (state, progress, error) = wait_over(&world, job.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    assert_eq!(
        phases(&progress)
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["start", "enroll", "after_up", "verify", "commit", "refresh"]
    );
    assert!(
        phases(&progress)
            .iter()
            .any(|(name, state)| name == "refresh" && state == "skipped"),
        "a sign-in the harness no longer finds is the end of the phase, said in its words: {progress}"
    );
    assert_eq!(asked(&heard, K::ProviderRefresh).len(), 1);
    assert_eq!(host.count(K::NodeRuntimeStart), 2);
    let told: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE user_id = $1 AND kind = 'provider.attention'",
    )
    .bind(world.ada)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(told, 1, "its managers are told before a thread needs it");
    let _ = runtime;
}

#[tokio::test]
async fn a_look_at_a_sign_in_that_still_works_tells_nobody() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let _template = provider_template(&world).await;
    let provider = managed_subscription(&world, "Team Claude").await;
    let (_runtime, unit) = made(&world, &host, provider).await;
    let _heard = listen(
        &unit,
        "ready",
        json!({ "plan": "team", "credential": {"yields":"login","expires_at":null} }),
    );
    let renewed = world.engine().renew_sign_in(provider).await.unwrap();
    assert_eq!(renewed, Renewed::Held);
    let told: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications WHERE kind = 'provider.attention'")
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(told, 0);
}

#[tokio::test]
async fn a_token_whose_end_is_known_is_told_ahead_once_and_again_when_it_has_lapsed_and_the_provider_needs_a_sign_in()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let _template = provider_template(&world).await;
    let provider = managed_subscription(&world, "Team Claude").await;
    let (_runtime, _unit) = made(&world, &host, provider).await;
    sqlx::query("UPDATE providers SET status = 'ready' WHERE id = $1")
        .bind(provider)
        .execute(world.pool.pool())
        .await
        .unwrap();
    let soon = Utc::now() + Duration::days(10);
    sqlx::query(
        "UPDATE providers SET facts = jsonb_build_object('credential',
                jsonb_build_object('method', 'setup_token', 'yields', 'token', 'expires_at', $2::text))
          WHERE id = $1",
    )
    .bind(provider)
    .bind(soon.to_rfc3339())
    .execute(world.pool.pool())
    .await
    .unwrap();
    let told = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM notifications WHERE user_id = $1 AND kind = 'provider.attention'",
        )
        .bind(world.ada)
        .fetch_one(world.pool.pool())
        .await
        .unwrap()
    };
    eventually(
        "the managers to be told that the token is about to lapse",
        || async { told().await == 1 },
    )
    .await;
    common::meanwhile(std::time::Duration::from_millis(600)).await;
    assert_eq!(
        told().await,
        1,
        "told once however often the schedule looks"
    );
    let status: String = sqlx::query_scalar("SELECT status FROM providers WHERE id = $1")
        .bind(provider)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert_eq!(status, "ready", "it works until it lapses");
    // It lapses.
    sqlx::query(
        "UPDATE providers SET facts = jsonb_set(facts, '{credential,expires_at}', to_jsonb($2::text)) WHERE id = $1",
    )
    .bind(provider)
    .bind((Utc::now() - Duration::hours(1)).to_rfc3339())
    .execute(world.pool.pool())
    .await
    .unwrap();
    eventually("the provider to need a sign-in", || async {
        let (status, detail): (String, Option<String>) =
            sqlx::query_as("SELECT status, status_detail FROM providers WHERE id = $1")
                .bind(provider)
                .fetch_one(world.pool.pool())
                .await
                .unwrap();
        status == "needs_setup" && detail.is_some_and(|text| text.contains("lapsed"))
    })
    .await;
    eventually("the managers to be told that it has lapsed", || async {
        told().await == 2
    })
    .await;
}
