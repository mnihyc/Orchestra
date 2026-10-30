//! Provider observations follow registration, sign-in, page and activity clocks; credential notices follow token lifetime without warning-percent notifications.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;
mod runtimes;

use chrono::Duration;
use common::{World, eventually};
use orch_engine::provider_setup::{Refreshed, Renewed};
use orch_link_contract::commands::{Command, CommandKind, CommandResult, ProviderRefreshResult};
use orch_link_contract::events::Event;
use orch_link_contract::streams::Stream;
use orch_testkit::clock;
use orch_types::{Clock, Json, Time};
use serde_json::json;
use std::sync::{Arc, Mutex};

#[test]
fn a_keepers_new_method_does_not_clear_the_sources_expiry_but_a_told_empty_expiry_does() {
    use orch_engine::provider_fact_reads::merge_observation;
    let old = "2026-10-13T00:00:00.000Z";
    let new = "2026-10-13T00:01:00.000Z";
    let known = json!({"credential":{"method":"old","yields":"token","expires_at":"2026-12-01T00:00:00Z","issued_at":null},"field_times":{"credential":old}});
    let keeper = json!({"credential":{"method":"new","yields":"token","expires_at":null,"issued_at":null},
        "field_times":{"credential.method":new,"credential.yields":new}});
    let merged = merge_observation(&known, keeper);
    assert_eq!(merged["credential"]["method"], "new");
    assert_eq!(merged["credential"]["expires_at"], "2026-12-01T00:00:00Z");
    assert_eq!(merged["field_times"]["credential"], old);
    let cleared = merge_observation(
        &merged,
        json!({"credential":{"method":null,"yields":"none","expires_at":null,"issued_at":null},
        "field_times":{"credential.expires_at":new}}),
    );
    assert!(cleared["credential"]["expires_at"].is_null());
    assert_eq!(cleared["credential"]["method"], "new");
}

async fn world() -> (World, orch_types::ManualClock) {
    let clock = clock::manual();
    let world = World::with_clock(Arc::new(clock.clone())).await;
    (world, clock)
}

#[tokio::test]
async fn a_new_or_changed_source_is_due_without_opening_its_page_and_its_claim_prevents_repeating_it()
 {
    use orch_db::queries::provider_facts::{self, ReadSchedule};
    let (world, clock) = world().await;
    let configured = orch_engine::provider_fact_reads::ReadConfig::from_server(
        &orch_config::ServerConfig::default(),
    );
    let schedule = || ReadSchedule {
        recent: configured.recent,
        active: configured.active,
        idle: configured.idle,
        refused: configured.refused,
    };
    let mut conn = world.pool.acquire().await.unwrap();
    sqlx::query("UPDATE providers SET facts_at=NULL,facts_requested_at=NULL,facts='{}',sources='{}' WHERE id=$1")
        .bind(world.provider).execute(&mut *conn).await.unwrap();
    assert!(
        provider_facts::due_reads(&mut conn, clock.now(), schedule(), 100)
            .await
            .unwrap()
            .contains(&world.provider)
    );
    assert!(
        provider_facts::claim_read(
            &mut conn,
            world.provider,
            false,
            clock.now(),
            configured.page
        )
        .await
        .unwrap()
    );
    assert!(
        !provider_facts::due_reads(&mut conn, clock.now(), schedule(), 100)
            .await
            .unwrap()
            .contains(&world.provider)
    );
    let revision: i64 = sqlx::query_scalar("SELECT revision FROM providers WHERE id=$1")
        .bind(world.provider)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert!(
        provider_facts::set_sources(
            &mut conn,
            world.provider,
            revision,
            &json!({}),
            true,
            clock.now()
        )
        .await
        .unwrap()
    );
    assert!(
        provider_facts::due_reads(&mut conn, clock.now(), schedule(), 100)
            .await
            .unwrap()
            .contains(&world.provider),
        "a saved new source queues its first read without an HTTP read or a clock advance"
    );
}

fn listen(world: &World, status: &'static str, facts: Json) -> Arc<Mutex<usize>> {
    let count: Arc<Mutex<usize>> = Arc::default();
    let called = count.clone();
    world
        .machine
        .handler
        .on(CommandKind::ProviderRefresh, move |_| {
            *called.lock().unwrap() += 1;
            Ok(CommandResult::ProviderRefresh(ProviderRefreshResult {
                source: None,
                status: status.into(),
                facts: serde_json::from_value(facts.clone()).unwrap(),
            }))
        });
    count
}

fn source_descriptor() -> orch_provider_contract::SourceDescriptor {
    let mut descriptor = orch_provider_contract::SourceDescriptor::new(
        "plan",
        "Plan details",
        "Reads the team's plan and spending.",
    );
    descriptor.facts = Some(orch_provider_contract::source::SourceDescriptorFacts {
        reports: vec![
            orch_provider_contract::FactMember::Plan,
            orch_provider_contract::FactMember::OrdinaryUsageAllowed,
        ],
        ..Default::default()
    });
    descriptor
}

#[tokio::test]
async fn a_source_spec_resolves_only_this_providers_named_secrets_for_its_selected_element() {
    let (world, clock) = world().await;
    let mut conn = world.pool.acquire().await.unwrap();
    let facts = orch_db::queries::settings::put_secret(
        &mut conn,
        "provider",
        world.provider,
        "source:facts:field:token",
        b"sealed-facts",
        Some(world.ada),
        clock.now(),
    )
    .await
    .unwrap();
    let models = orch_db::queries::settings::put_secret(
        &mut conn,
        "provider",
        world.provider,
        "source:models:field:token",
        b"sealed-models",
        Some(world.ada),
        clock.now(),
    )
    .await
    .unwrap();
    orch_db::queries::settings::put_secret(
        &mut conn,
        "provider",
        world.root,
        "source:facts:field:missing",
        b"another-owner",
        Some(world.ada),
        clock.now(),
    )
    .await
    .unwrap();
    let choices = json!({"facts":{"by":"program","name":"plan","secrets":["token","missing"]},"models":{"by":"program","name":"catalogue","secrets":["token"]}});
    let facts_spec = orch_engine::provider_sources::spec(
        &mut conn,
        world.provider,
        &choices,
        orch_provider_contract::SourceElement::Facts,
    )
    .await
    .unwrap()
    .unwrap();
    let models_spec = orch_engine::provider_sources::spec(
        &mut conn,
        world.provider,
        &choices,
        orch_provider_contract::SourceElement::Models,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        facts_spec.secrets["token"],
        orch_provider_contract::SourceSecret::Ref(facts.id)
    );
    assert!(!facts_spec.secrets.contains_key("missing"));
    assert_eq!(
        models_spec.secrets["token"],
        orch_provider_contract::SourceSecret::Ref(models.id)
    );
    assert_eq!(models_spec.name.as_deref(), Some("catalogue"));
    assert_eq!(facts_spec.provider.unwrap().id, world.provider);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_answer_from_a_replaced_source_cannot_write_facts_or_metadata_for_the_new_source() {
    use orch_link_contract::commands::ProviderSourceRead;
    let (world, _clock) = world().await;
    sqlx::query("UPDATE providers SET sources=$2 WHERE id=$1")
        .bind(world.provider)
        .bind(json!({"facts":{"by":"program","name":"plan"}}))
        .execute(world.pool.pool())
        .await
        .unwrap();
    let pool = world.pool.pool().clone();
    let provider = world.provider;
    world
        .machine
        .handler
        .on(CommandKind::ProviderRefresh, move |_| {
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    sqlx::query("UPDATE providers SET sources=$2,revision=revision+1 WHERE id=$1")
                        .bind(provider)
                        .bind(json!({"facts":{"by":"none"}}))
                        .execute(&pool)
                        .await
                        .unwrap();
                })
            });
            Ok(CommandResult::ProviderRefresh(ProviderRefreshResult {
                source: Some(ProviderSourceRead {
                    descriptor: Some(source_descriptor()),
                    error: None,
                    supported: Some(true),
                }),
                status: "ready".into(),
                facts: serde_json::from_value(json!({"plan":"obsolete"})).unwrap(),
            }))
        });
    world
        .engine()
        .refresh_provider(world.provider)
        .await
        .unwrap();
    let (facts, sources): (Json, Json) =
        sqlx::query_as("SELECT facts,sources FROM providers WHERE id=$1")
            .bind(world.provider)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_ne!(facts.get("plan"), Some(&json!("obsolete")));
    assert_eq!(sources, json!({"facts":{"by":"none"}}));
}

#[tokio::test]
async fn a_failed_source_read_keeps_the_facts_and_status_beside_its_learned_declaration_and_bounded_error()
 {
    use orch_link_contract::commands::ProviderSourceRead;
    let (world, clock) = world().await;
    let before = json!({"plan":"kept","field_times":{"plan":clock.time()}});
    sqlx::query("UPDATE providers SET status='error',sources=$2,facts=$3,facts_at=$4,updated_at=$4 WHERE id=$1")
        .bind(world.provider).bind(json!({"facts":{"by":"program","name":"plan"}})).bind(&before).bind(clock.now())
        .execute(world.pool.pool()).await.unwrap();
    let seen: Arc<Mutex<Option<orch_provider_contract::SourceSpec>>> = Arc::default();
    let called = seen.clone();
    world
        .machine
        .handler
        .on(CommandKind::ProviderRefresh, move |command| {
            if let Command::ProviderRefresh(body) = command {
                *called.lock().unwrap() = body.facts.clone();
            }
            Ok(CommandResult::ProviderRefresh(ProviderRefreshResult {
                source: Some(ProviderSourceRead {
                    descriptor: Some(source_descriptor()),
                    error: Some(orch_provider_contract::SourceError::new(
                        orch_provider_contract::SourceErrorCode::Timeout,
                        "x".repeat(500),
                    )),
                    supported: None,
                }),
                status: "not_told".into(),
                facts: None,
            }))
        });
    clock.advance(Duration::minutes(1));
    world
        .engine()
        .refresh_provider(world.provider)
        .await
        .unwrap();
    let spec = seen.lock().unwrap().clone().unwrap();
    assert_eq!(spec.by, orch_provider_contract::SourceBy::Program);
    assert_eq!(spec.name.as_deref(), Some("plan"));
    assert_eq!(spec.provider.unwrap().id, world.provider);
    let (status, facts, source, at, status_at): (
        String,
        Json,
        Json,
        chrono::DateTime<chrono::Utc>,
        chrono::DateTime<chrono::Utc>,
    ) = sqlx::query_as("SELECT status,facts,sources,facts_at,status_at FROM providers WHERE id=$1")
        .bind(world.provider)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert_eq!(status, "error");
    assert_eq!(facts, before);
    assert_eq!(at, clock.now() - Duration::minutes(1));
    assert_eq!(status_at, at);
    assert_eq!(source["facts"]["descriptor"]["title"], "Plan details");
    assert_eq!(source["facts"]["last_error"]["code"], "timeout");
    assert_eq!(
        source["facts"]["last_error"]["message"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        300
    );
    world
        .machine
        .handler
        .on(CommandKind::ProviderRefresh, move |_| {
            Ok(CommandResult::ProviderRefresh(ProviderRefreshResult {
                source: Some(ProviderSourceRead {
                    descriptor: Some(source_descriptor()),
                    error: None,
                    supported: Some(true),
                }),
                status: "ready".into(),
                facts: serde_json::from_value(
                    json!({"plan":"current","field_times":{"plan":clock.time()}}),
                )
                .unwrap(),
            }))
        });
    world
        .engine()
        .refresh_provider_for_retry(world.provider)
        .await
        .unwrap();
    let (status, facts, source): (String, Json, Json) =
        sqlx::query_as("SELECT status,facts,sources FROM providers WHERE id=$1")
            .bind(world.provider)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(
        status, "error",
        "a plan read without the key is no proof that the refused key works"
    );
    assert_eq!(facts["plan"], "current");
    assert!(source["facts"]["last_error"].is_null());
}

#[tokio::test]
async fn model_source_errors_keep_the_previous_list_while_unsupported_and_empty_are_distinct() {
    use orch_link_contract::commands::{ProviderDiscoverModelsResult, ProviderSourceRead};
    let (world, clock) = world().await;
    let accepted = world
        .count(
            "SELECT count(*) FROM provider_models WHERE provider_id=$1",
            world.provider,
        )
        .await;
    let known = json!({"models":[{"model_name":"kept","display_name":"Kept","parameters":null}],"field_times":{"models":clock.time()}});
    sqlx::query("UPDATE providers SET sources=$2,facts=$3 WHERE id=$1")
        .bind(world.provider)
        .bind(json!({"models":{"by":"program","name":"plan"}}))
        .bind(&known)
        .execute(world.pool.pool())
        .await
        .unwrap();
    world
        .machine
        .handler
        .on(CommandKind::ProviderDiscoverModels, move |command| {
            let Command::ProviderDiscoverModels(body) = command else {
                panic!("wrong command");
            };
            assert_eq!(
                body.models.as_ref().and_then(|spec| spec.name.as_deref()),
                Some("plan")
            );
            Ok(CommandResult::ProviderDiscoverModels(
                ProviderDiscoverModelsResult {
                    source: Some(ProviderSourceRead {
                        descriptor: Some(source_descriptor()),
                        error: Some(orch_provider_contract::SourceError::new(
                            orch_provider_contract::SourceErrorCode::Unavailable,
                            "The source is restarting.",
                        )),
                        supported: None,
                    }),
                    suggestions: Vec::new(),
                },
            ))
        });
    assert!(
        world
            .engine()
            .discover_provider_models(world.provider)
            .await
            .unwrap()
            .is_none()
    );
    let facts: Json = world
        .scalar("SELECT facts FROM providers WHERE id=$1", world.provider)
        .await;
    assert_eq!(facts, known);
    world
        .machine
        .handler
        .on(CommandKind::ProviderDiscoverModels, move |_| {
            Ok(CommandResult::ProviderDiscoverModels(
                ProviderDiscoverModelsResult {
                    source: Some(ProviderSourceRead {
                        descriptor: Some(source_descriptor()),
                        error: None,
                        supported: Some(false),
                    }),
                    suggestions: Vec::new(),
                },
            ))
        });
    assert!(
        world
            .engine()
            .discover_provider_models(world.provider)
            .await
            .unwrap()
            .is_none()
    );
    let facts: Json = world
        .scalar("SELECT facts FROM providers WHERE id=$1", world.provider)
        .await;
    assert!(facts["models"].is_null());
    let mut descriptor = source_descriptor();
    descriptor.models = Some(orch_provider_contract::source::SourceDescriptorModels {});
    world
        .machine
        .handler
        .on(CommandKind::ProviderDiscoverModels, move |_| {
            Ok(CommandResult::ProviderDiscoverModels(
                ProviderDiscoverModelsResult {
                    source: Some(ProviderSourceRead {
                        descriptor: Some(descriptor.clone()),
                        error: None,
                        supported: Some(true),
                    }),
                    suggestions: Vec::new(),
                },
            ))
        });
    assert_eq!(
        world
            .engine()
            .discover_provider_models(world.provider)
            .await
            .unwrap(),
        Some(Vec::new())
    );
    let facts: Json = world
        .scalar("SELECT facts FROM providers WHERE id=$1", world.provider)
        .await;
    assert_eq!(facts["models"], json!([]));
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM provider_models WHERE provider_id=$1",
                world.provider
            )
            .await,
        accepted,
        "discovery never changes the accepted allowlist"
    );
}

async fn read_count(count: &Arc<Mutex<usize>>, expected: usize) {
    eventually("the provider observation", || async {
        *count.lock().unwrap() == expected
    })
    .await;
}

#[tokio::test]
async fn an_exhausted_provider_with_waiting_inputs_is_observed_at_the_active_cadence_without_a_running_turn()
 {
    let (world, clock) = world().await;
    let count = listen(&world, "not_told", Json::Null);
    sqlx::query(
        "UPDATE providers SET status='exhausted',facts_requested_at=$2,facts_at=$2 WHERE id=$1",
    )
    .bind(world.provider)
    .bind(clock.now())
    .execute(world.pool.pool())
    .await
    .unwrap();
    let thread = world.thread("Waiting for the plan").await;
    world
        .engine()
        .submit_input(
            &orch_authz::Caller::session(world.ada),
            thread,
            common::queued("Continue when the limit lifts"),
        )
        .await
        .unwrap();
    world.wait_blocked(thread, "provider_exhausted").await;
    clock.advance(Duration::minutes(4));
    world.engine().refresh_due_provider_facts().await.unwrap();
    assert_eq!(*count.lock().unwrap(), 0);
    clock.advance(Duration::minutes(1));
    world.engine().refresh_due_provider_facts().await.unwrap();
    read_count(&count, 1).await;
    assert_eq!(
        world
            .count("SELECT count(*) FROM turns WHERE thread_id=$1", thread)
            .await,
        0,
        "observing a limit does not create a turn"
    );
}

#[tokio::test]
async fn a_spend_reset_gets_one_settled_read_even_at_the_idle_cadence() {
    let (world, clock) = world().await;
    let count = listen(&world, "not_told", Json::Null);
    let ended = clock.now() - Duration::minutes(2);
    sqlx::query("UPDATE providers SET facts=$2,facts_requested_at=$3,facts_at=$3 WHERE id=$1").bind(world.provider).bind(json!({"spend":[{"id":"month","unit":"USD","period":"month","used":"12.50","resets_at":Time::new(ended)}]})).bind(clock.now()-Duration::seconds(90)).execute(world.pool.pool()).await.unwrap();
    world.engine().refresh_due_provider_facts().await.unwrap();
    read_count(&count, 1).await;
    world.engine().refresh_due_provider_facts().await.unwrap();
    assert_eq!(
        *count.lock().unwrap(),
        1,
        "an unchanged past reset is not read repeatedly"
    );
}

#[tokio::test]
async fn a_sources_minimum_read_interval_bounds_page_forced_and_reset_reads() {
    let (world, clock) = world().await;
    let count = listen(&world, "not_told", Json::Null);
    let descriptor = json!({"name":"cadence","title":"Cadence","summary":"Reports at most every fifteen minutes","contract":1,"facts":{"reports":[],"spends_resets":false,"credential":"none","min_interval_secs":900,"plans":{}},"models":null,"presents":null,"fields":[],"docs_url":null});
    sqlx::query("UPDATE providers SET sources=$2,facts_requested_at=$3 WHERE id=$1")
        .bind(world.provider)
        .bind(
            json!({"facts":{"by":"program","name":"cadence","config":{},"descriptor":descriptor}}),
        )
        .bind(clock.now())
        .execute(world.pool.pool())
        .await
        .unwrap();
    clock.advance(Duration::minutes(1));
    world
        .engine()
        .refresh_provider(world.provider)
        .await
        .unwrap();
    assert_eq!(*count.lock().unwrap(), 0);
    world
        .engine()
        .refresh_provider_for_retry(world.provider)
        .await
        .unwrap();
    assert_eq!(
        *count.lock().unwrap(),
        0,
        "a forced retry still honors the source's declared minimum"
    );
    sqlx::query("UPDATE providers SET facts=$2 WHERE id=$1").bind(world.provider).bind(json!({"spend":[{"id":"cycle","unit":"credits","period":"window","used":"1","resets_at":Time::new(clock.now()-Duration::minutes(2))}]})).execute(world.pool.pool()).await.unwrap();
    world.engine().refresh_due_provider_facts().await.unwrap();
    assert_eq!(*count.lock().unwrap(), 0);
    clock.advance(Duration::minutes(14));
    world
        .engine()
        .refresh_provider(world.provider)
        .await
        .unwrap();
    read_count(&count, 1).await;
}

#[tokio::test]
async fn registration_and_sign_in_read_facts_and_a_page_reads_at_most_once_a_minute() {
    let (world, clock) = world().await;
    let count = listen(&world, "ready", json!({"plan":"plus","windows":[]}));
    assert!(matches!(
        world
            .engine()
            .refresh_provider(world.provider)
            .await
            .unwrap(),
        Refreshed::Answered { .. }
    ));
    assert_eq!(*count.lock().unwrap(), 1);
    clock.advance(Duration::seconds(59));
    world
        .engine()
        .refresh_provider(world.provider)
        .await
        .unwrap();
    assert_eq!(*count.lock().unwrap(), 1);
    clock.advance(Duration::seconds(1));
    world
        .engine()
        .refresh_provider(world.provider)
        .await
        .unwrap();
    assert_eq!(*count.lock().unwrap(), 2);
    clock.advance(Duration::seconds(1));
    world
        .engine()
        .look_at_completed_sign_in(world.provider)
        .await
        .unwrap();
    assert_eq!(
        *count.lock().unwrap(),
        3,
        "a newly given credential is read without waiting for the page bound"
    );
    let at: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT facts_at FROM providers WHERE id = $1")
            .bind(world.provider)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(
        at,
        clock.now(),
        "even an equal-valued successful read has its new observation time"
    );
}

#[tokio::test]
async fn a_recent_thread_gets_five_minute_reads_an_idle_provider_hourly_and_an_away_machine_none() {
    let (world, clock) = world().await;
    let count = listen(&world, "ready", json!({"plan":"plus","windows":[]}));
    world
        .engine()
        .refresh_provider(world.provider)
        .await
        .unwrap();
    let thread = world.thread("recent").await;
    let (turn, _) = world.running_turn(thread, "Hello").await;
    world.answer(thread, &turn, "answer", "Done").await;
    clock.advance(Duration::minutes(5) - Duration::seconds(1));
    world.engine().refresh_due_provider_facts().await.unwrap();
    assert_eq!(*count.lock().unwrap(), 1);
    clock.advance(Duration::seconds(1));
    world.engine().refresh_due_provider_facts().await.unwrap();
    read_count(&count, 2).await;
    clock.advance(Duration::minutes(11));
    world.engine().refresh_due_provider_facts().await.unwrap();
    assert_eq!(
        *count.lock().unwrap(),
        2,
        "after the quarter hour the provider is on the idle cadence"
    );
    clock.advance(Duration::minutes(49) - Duration::seconds(1));
    world.engine().refresh_due_provider_facts().await.unwrap();
    assert_eq!(*count.lock().unwrap(), 2);
    clock.advance(Duration::seconds(1));
    world.engine().refresh_due_provider_facts().await.unwrap();
    read_count(&count, 3).await;
    sqlx::query("UPDATE machines SET online = false WHERE id = $1")
        .bind(world.machine_id)
        .execute(world.pool.pool())
        .await
        .unwrap();
    clock.advance(Duration::hours(2));
    world.engine().refresh_due_provider_facts().await.unwrap();
    let page = world
        .engine()
        .refresh_provider(world.provider)
        .await
        .unwrap();
    match page {
        Refreshed::Offline => {}
        Refreshed::Answered { status, facts, .. } => {
            // A finishing scheduled read coalesces this page to the already known observation.
            assert_eq!(status, "ready");
            assert_eq!(facts["plan"], "plus");
        }
        other => panic!("an away machine must leave its stored page available: {other:?}"),
    }
    assert_eq!(*count.lock().unwrap(), 3);
}

#[tokio::test]
async fn an_idle_provider_is_read_one_minute_after_each_reported_reset_even_after_a_recent_page_read()
 {
    let (world, clock) = world().await;
    let start = clock.now();
    let count = listen(
        &world,
        "ready",
        json!({"plan":"plus","windows":[],"ordinary_usage_allowed":true}),
    );
    world
        .engine()
        .refresh_provider(world.provider)
        .await
        .unwrap();
    // Different windows and the reached-limit reset each get their own one-time due read.
    let facts = json!({"windows":[
        {"id":"primary","label":"Short window","duration_minutes":300,"used_percent":90,"resets_at":Time::new(start + Duration::minutes(2))},
        {"id":"weekly","label":"Weekly window","duration_minutes":10080,"used_percent":20,"resets_at":Time::new(start + Duration::minutes(4))}
    ],"limit_reached":{"type":"short window","resets_at":Time::new(start + Duration::minutes(6))}});
    sqlx::query("UPDATE providers SET facts = $2 WHERE id = $1")
        .bind(world.provider)
        .bind(&facts)
        .execute(world.pool.pool())
        .await
        .unwrap();
    for (reset, expected) in [(2, 2), (4, 3), (6, 4)] {
        clock.set(start + Duration::minutes(reset) + Duration::seconds(30));
        // A read inside the preceding minute must not push the vendor's reset read later.
        sqlx::query("UPDATE providers SET facts_requested_at = $2 WHERE id = $1")
            .bind(world.provider)
            .bind(clock.now())
            .execute(world.pool.pool())
            .await
            .unwrap();
        clock.advance(Duration::seconds(29));
        world.engine().refresh_due_provider_facts().await.unwrap();
        assert_eq!(*count.lock().unwrap(), expected - 1);
        clock.advance(Duration::seconds(1));
        world.engine().refresh_due_provider_facts().await.unwrap();
        read_count(&count, expected).await;
        // Synchronize with the worker's committed observation before putting the unchanged vendor clocks back.
        eventually("the reset read is committed", || async {
            let requested: Option<chrono::DateTime<chrono::Utc>> =
                sqlx::query_scalar("SELECT facts_requested_at FROM providers WHERE id = $1")
                    .bind(world.provider)
                    .fetch_one(world.pool.pool())
                    .await
                    .unwrap();
            let facts_at: Option<chrono::DateTime<chrono::Utc>> =
                sqlx::query_scalar("SELECT facts_at FROM providers WHERE id = $1")
                    .bind(world.provider)
                    .fetch_one(world.pool.pool())
                    .await
                    .unwrap();
            requested == Some(clock.now()) && facts_at == Some(clock.now())
        })
        .await;
        sqlx::query("UPDATE providers SET facts = $2 WHERE id = $1")
            .bind(world.provider)
            .bind(&facts)
            .execute(world.pool.pool())
            .await
            .unwrap();
        world.engine().refresh_due_provider_facts().await.unwrap();
        assert_eq!(
            *count.lock().unwrap(),
            expected,
            "the same reset does not keep polling"
        );
    }
}

#[tokio::test]
async fn a_read_that_tells_nothing_retains_the_facts_their_time_and_the_provider_state_and_is_still_throttled()
 {
    let (world, clock) = world().await;
    world
        .engine()
        .refresh_provider(world.provider)
        .await
        .unwrap();
    clock.advance(Duration::minutes(1));
    let old = json!({"plan":"plus","windows":[],"identity":"ada@example.invalid"});
    let before = clock.now() - Duration::hours(2);
    sqlx::query("UPDATE providers SET facts = $2,facts_at = $3,status = 'error' WHERE id = $1")
        .bind(world.provider)
        .bind(&old)
        .bind(before)
        .execute(world.pool.pool())
        .await
        .unwrap();
    let count = listen(&world, "not_told", Json::Null);
    for _ in 0..2 {
        world
            .engine()
            .refresh_provider(world.provider)
            .await
            .unwrap();
    }
    assert_eq!(*count.lock().unwrap(), 1);
    let (status, facts, at): (String, Json, chrono::DateTime<chrono::Utc>) =
        sqlx::query_as("SELECT status,facts,facts_at FROM providers WHERE id = $1")
            .bind(world.provider)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(status, "error");
    assert_eq!(facts, old);
    assert_eq!(at, before);
}

async fn notices(world: &World) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE user_id = $1 AND kind = 'provider.attention'",
    )
    .bind(world.ada)
    .fetch_one(world.pool.pool())
    .await
    .unwrap()
}

async fn renewed_at(world: &World) -> Option<chrono::DateTime<chrono::Utc>> {
    sqlx::query_scalar("SELECT credential_renewed_at FROM providers WHERE id = $1")
        .bind(world.provider)
        .fetch_one(world.pool.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn native_renewal_asks_once_more_when_the_lapse_is_unchanged_and_only_success_moves_its_clock()
 {
    let (world, clock) = world().await;
    let original = clock.now() + Duration::days(4);
    let extended = clock.now() + Duration::days(8);
    let facts = json!({"credential":{"yields":"login","expires_at":Time::new(original)}});
    listen(&world, "ready", facts.clone());
    world
        .engine()
        .refresh_provider(world.provider)
        .await
        .unwrap();
    assert_eq!(
        renewed_at(&world).await,
        None,
        "ordinary reads do not count as renewal"
    );
    clock.advance(Duration::minutes(1));
    let requests = Arc::new(Mutex::new(0));
    let seen = requests.clone();
    world
        .machine
        .handler
        .on(CommandKind::ProviderRefresh, move |command| {
            let Command::ProviderRefresh(body) = command else {
                panic!("wrong command")
            };
            let mut count = seen.lock().unwrap();
            if body.renew == Some(true) {
                *count += 1;
            }
            let expires = if *count >= 2 { extended } else { original };
            Ok(CommandResult::ProviderRefresh(ProviderRefreshResult {
                source: None,
                status: "ready".into(),
                facts: serde_json::from_value(
                    json!({"credential":{"yields":"login","expires_at":Time::new(expires)}}),
                )
                .unwrap(),
            }))
        });
    assert!(matches!(
        world.engine().renew_sign_in(world.provider).await.unwrap(),
        Renewed::Held
    ));
    assert_eq!(*requests.lock().unwrap(), 2);
    assert_eq!(renewed_at(&world).await, Some(clock.now()));
    let stored: Json = sqlx::query_scalar("SELECT facts FROM providers WHERE id = $1")
        .bind(world.provider)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert_eq!(
        stored["credential"]["expires_at"],
        json!(Time::new(extended))
    );
    assert_eq!(notices(&world).await, 0);
}

#[tokio::test]
async fn an_unchanged_or_unreadable_native_sign_in_tells_its_managers_once_per_day_and_is_not_success()
 {
    for status in ["ready", "needs_setup", "not_told"] {
        let (world, clock) = world().await;
        let facts = json!({"credential":{"yields":"login","expires_at":Time::new(clock.now()+Duration::days(4))}});
        sqlx::query("UPDATE providers SET facts = $2 WHERE id = $1")
            .bind(world.provider)
            .bind(&facts)
            .execute(world.pool.pool())
            .await
            .unwrap();
        let count = listen(&world, status, facts);
        for _ in 0..2 {
            assert!(matches!(
                world.engine().renew_sign_in(world.provider).await.unwrap(),
                Renewed::Lapsed(_)
            ));
        }
        assert_eq!(
            *count.lock().unwrap(),
            if status == "needs_setup" { 2 } else { 4 }
        );
        assert_eq!(notices(&world).await, 1);
        assert_eq!(renewed_at(&world).await, None);
    }
}

#[tokio::test]
async fn a_native_login_without_a_vendor_lapse_uses_successful_renewal_time_and_an_away_runtime_tells()
 {
    let (world, clock) = world().await;
    listen(
        &world,
        "ready",
        json!({"credential":{"yields":"login","expires_at":null}}),
    );
    assert!(matches!(
        world.engine().renew_sign_in(world.provider).await.unwrap(),
        Renewed::Held
    ));
    let successful = clock.now();
    assert_eq!(renewed_at(&world).await, Some(successful));
    clock.advance(Duration::days(5));
    sqlx::query("UPDATE machines SET online = false WHERE id = $1")
        .bind(world.machine_id)
        .execute(world.pool.pool())
        .await
        .unwrap();
    assert!(matches!(
        world.engine().renew_sign_in(world.provider).await.unwrap(),
        Renewed::Lapsed(_)
    ));
    assert_eq!(renewed_at(&world).await, Some(successful));
    assert_eq!(notices(&world).await, 1);
}

#[tokio::test]
async fn a_ten_day_token_is_warned_once_two_and_a_half_days_before_and_a_year_token_fourteen_days_before()
 {
    for (life, notice) in [
        (Duration::days(10), Duration::hours(60)),
        (Duration::days(365), Duration::days(14)),
    ] {
        let (world, clock) = world().await;
        let provider = runtimes::managed_subscription(&world, "Imported token").await;
        let issued = clock.now();
        let ends = issued + life;
        sqlx::query("UPDATE providers SET status = 'ready',facts = $2 WHERE id = $1").bind(provider).bind(json!({"credential":{"method":"access_token","yields":"token","issued_at":Time::new(issued),"expires_at":Time::new(ends)}})).execute(world.pool.pool()).await.unwrap();
        world
            .engine()
            .watch_provider_credential_ends()
            .await
            .unwrap();
        assert_eq!(notices(&world).await, 0);
        clock.set(ends - notice - Duration::milliseconds(1));
        world
            .engine()
            .watch_provider_credential_ends()
            .await
            .unwrap();
        assert_eq!(notices(&world).await, 0);
        clock.advance(Duration::milliseconds(1));
        world
            .engine()
            .watch_provider_credential_ends()
            .await
            .unwrap();
        assert_eq!(notices(&world).await, 1);
        world
            .engine()
            .watch_provider_credential_ends()
            .await
            .unwrap();
        assert_eq!(notices(&world).await, 1);
        clock.advance(Duration::days(1));
        world
            .engine()
            .watch_provider_credential_ends()
            .await
            .unwrap();
        assert_eq!(
            notices(&world).await,
            1,
            "the credential's same warning is not repeated the next day"
        );
    }
}

#[tokio::test]
async fn an_expiry_notice_reaches_a_current_manager_who_holds_management_through_a_group() {
    use orch_db::queries::access::{PageKey, testing as entries};
    let (world, clock) = world().await;
    let provider = runtimes::managed_subscription(&world, "Group managed token").await;
    let manager = world.person("group-manager").await;
    let other = world.person("other-person").await;
    let mut conn = world.pool.acquire().await.unwrap();
    let group = entries::insert_group(&mut conn, world.root, "Provider operators", world.ada)
        .await
        .unwrap();
    entries::add_to_group(&mut conn, group, manager)
        .await
        .unwrap();
    entries::add_entry(
        &mut conn,
        &PageKey::new("provider", provider, "live"),
        "any",
        "provider.manage",
        ("group", Some(group), None),
        None,
        clock.now(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE providers SET status = 'ready',facts = $2 WHERE id = $1").bind(provider).bind(json!({"credential":{"method":"access_token","yields":"token","issued_at":Time::new(clock.now()-Duration::days(8)),"expires_at":Time::new(clock.now()+Duration::days(2))}})).execute(&mut *conn).await.unwrap();
    drop(conn);
    world
        .engine()
        .watch_provider_credential_ends()
        .await
        .unwrap();
    for (person, expected) in [(world.ada, 1), (manager, 1), (other, 0)] {
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM notifications WHERE user_id = $1 AND kind = 'provider.attention'",
        )
        .bind(person)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
        assert_eq!(count, expected);
    }
}

#[tokio::test]
async fn a_vendor_window_warning_is_kept_for_display_and_not_notified() {
    let (world, _clock) = world().await;
    world.machine.publish(&Stream::Provider {provider_id:world.provider},&Event::ProviderFacts(serde_json::from_value(json!({"plan":"pro","windows":[{"id":"secondary","label":"weekly","duration_minutes":10080,"used_percent":79,"resets_at":null,"severity":"warning","warning_percent":75,"is_active":true,"model_family":null}]})).unwrap())).await;
    eventually("the vendor warning to be stored", || async {
        let facts: Json = sqlx::query_scalar("SELECT facts FROM providers WHERE id = $1")
            .bind(world.provider)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
        facts["windows"][0]["used_percent"].as_f64() == Some(79.0)
    })
    .await;
    assert_eq!(notices(&world).await, 0);
}

#[test]
fn a_turn_observation_keeps_reset_credits_and_model_family_windows_it_did_not_read() {
    let known = json!({"available_resets":{"available_count":2,"credits":null},"windows":[{"id":"secondary:sonnet","used_percent":25}],"identity":"ada@example.invalid"});
    let update = json!({"windows":[{"id":"primary","used_percent":32}],"available_resets":null,"identity":null});
    let facts = orch_engine::provider_fact_reads::merge_observation(&known, update);
    assert_eq!(facts["available_resets"]["available_count"], 2);
    assert_eq!(facts["windows"].as_array().unwrap().len(), 2);
    assert_eq!(facts["identity"], "ada@example.invalid");
}

#[test]
fn an_empty_native_account_report_does_not_redate_or_erase_independent_model_discovery() {
    let known = json!({"models":[{"model_name":"known","display_name":"Known","parameters":{},"listed":true}],
        "field_times":{"models":"2026-10-14T12:00:00Z"}});
    let empty = serde_json::to_value(orch_agent_contract::facts::ProviderFacts::default()).unwrap();
    let facts = orch_engine::provider_fact_reads::merge_observation(&known, empty);
    assert_eq!(facts["models"], known["models"]);
    assert_eq!(
        facts["field_times"]["models"],
        known["field_times"]["models"]
    );
}

#[test]
fn a_keepers_push_cannot_replace_a_programs_plan_balance_limit_or_expiry() {
    let stored = json!({"plan":"team","balances":[{"id":"plan","unit":"credits","remaining":"12.50"}],"limit_reached":{"type":"weekly"},"credential":{"method":"login","yields":"login","expires_at":"2026-11-01T00:00:00Z"},"field_times":{"plan":"2026-10-14T10:00:00Z","credential.expires_at":"2026-10-14T10:00:00Z"}});
    let keeper = json!({"identity":"Reader","plan":"local-plan","balances":[],"limit_reached":null,"credential":{"method":"token","yields":"token","expires_at":"2027-01-01T00:00:00Z"},"credential_verified":true,"field_times":{"identity":"2026-10-14T12:00:00Z","credential":"2026-10-14T12:00:00Z"}});
    let incoming = orch_engine::provider_fact_reads::keeper_observation(keeper);
    assert!(incoming.get("credential_verified").is_none());
    let merged = orch_engine::provider_fact_reads::merge_observation(&stored, incoming);
    assert_eq!(merged["identity"], "Reader");
    assert_eq!(merged["credential"]["yields"], "token");
    assert_eq!(
        merged["credential"]["expires_at"],
        stored["credential"]["expires_at"]
    );
    for member in ["plan", "balances", "limit_reached"] {
        assert_eq!(merged[member], stored[member]);
    }
    assert_eq!(merged["field_times"]["plan"], stored["field_times"]["plan"]);
}

#[test]
fn absent_members_keep_their_values_while_a_stamped_empty_list_clears_only_that_member() {
    let stored = json!({"plan":"team","balances":[{"id":"credits","unit":"USD","remaining":"1.25"}],"spend":[{"id":"month","unit":"USD","period":"month","used":"3.00"}],"field_times":{"plan":"2026-10-14T10:00:00Z","balances":"2026-10-14T10:00:00Z"}});
    let empty = orch_engine::provider_fact_reads::merge_observation(&stored, json!({}));
    assert_eq!(empty, stored);
    let read = orch_engine::provider_fact_reads::merge_observation(
        &stored,
        json!({"balances":[],"field_times":{"balances":"2026-10-14T12:00:00Z"}}),
    );
    assert_eq!(read["balances"], json!([]));
    assert_eq!(read["plan"], stored["plan"]);
    assert_eq!(read["spend"], stored["spend"]);
    assert_eq!(read["field_times"]["plan"], stored["field_times"]["plan"]);
    let verified = json!({"credential_verified":true,"field_times":{"credential_verified":"2026-10-14T10:00:00Z"}});
    let missing = orch_engine::provider_fact_reads::normalize_observation(&json!({})).unwrap();
    let read = orch_engine::provider_fact_reads::merge_observation(&verified, missing);
    assert_eq!(read["credential_verified"], true);
    assert_eq!(
        read["field_times"]["credential_verified"], verified["field_times"]["credential_verified"],
        "a cached proof is dated data; recovery consumes only a new response's proof"
    );
}

#[test]
fn admission_uses_the_reported_flags_and_the_end_of_their_limit_episode_without_spending_making_the_decision()
 {
    let now: Time = "2026-10-15T10:00:00Z".parse().unwrap();
    let facts = |value: Json| {
        serde_json::from_value::<orch_agent_contract::facts::ProviderFacts>(value).unwrap()
    };
    assert!(!orch_engine::provider_fact_reads::admission_exhausted(
        &facts(
            json!({"balances":[{"id":"account","unit":"USD","remaining":"0"}],"spend":[{"id":"total","unit":"USD","period":"total","used":"99","limit":"1"}]})
        ),
        now
    ));
    assert!(orch_engine::provider_fact_reads::admission_exhausted(
        &facts(json!({"ordinary_usage_allowed":false})),
        now
    ));
    let old = json!({"ordinary_usage_allowed":false,"limit_reached":{"type":"weekly","resets_at":"2026-10-15T09:00:00Z"},"field_times":{"ordinary_usage_allowed":"2026-10-15T08:00:00Z"}});
    assert!(!orch_engine::provider_fact_reads::admission_exhausted(
        &facts(old.clone()),
        now
    ));
    let mut current = old;
    current["field_times"]["ordinary_usage_allowed"] = json!(now);
    assert!(
        orch_engine::provider_fact_reads::admission_exhausted(&facts(current), now),
        "a new refusal after the old reset is current"
    );
}

#[test]
fn only_a_binding_given_the_provider_credential_can_claim_it_was_verified() {
    use orch_engine::provider_fact_reads::source_can_verify;
    let program = |credential| json!({"facts":{"by":"program","name":"plan","descriptor":{"facts":{"credential":credential}}}});
    assert!(!source_can_verify(&program("none"), false));
    assert!(source_can_verify(&program("key"), false));
    assert!(
        !source_can_verify(&program("key"), true),
        "a harness's sign-in is never given to a program"
    );
    let address = |sent| json!({"facts":{"by":"address","address":{"url":"https://details.example.invalid","send_credential":sent},"descriptor":{"facts":{"credential":"key"}}}});
    assert!(!source_can_verify(&address(false), false));
    assert!(source_can_verify(&address(true), false));
    assert!(!source_can_verify(&address(true), true));
    assert!(!source_can_verify(&json!({"facts":{"by":"none"}}), false));
}

#[tokio::test]
async fn a_program_not_given_the_key_cannot_recover_a_refusal_by_claiming_verified() {
    let (world, _) = world().await;
    sqlx::query("UPDATE providers SET status='error',sources=$2 WHERE id=$1").bind(world.provider).bind(json!({"facts":{"by":"program","name":"plan","descriptor":{"name":"plan","title":"Plan","summary":"Reads a plan","contract":1,"facts":{"credential":"none"}}}})).execute(world.pool.pool()).await.unwrap();
    listen(
        &world,
        "ready",
        json!({"credential_verified":true,"ordinary_usage_allowed":true}),
    );
    world
        .engine()
        .refresh_provider(world.provider)
        .await
        .unwrap();
    let (status, facts): (String, Json) =
        sqlx::query_as("SELECT status,facts FROM providers WHERE id=$1")
            .bind(world.provider)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(status, "error");
    assert_ne!(
        facts.get("credential_verified"),
        Some(&json!(true)),
        "the unsupported proof is not stored as a fresh authenticated observation"
    );
}

#[tokio::test]
async fn a_quota_denial_does_not_hide_the_keepers_missing_sign_in() {
    for (reported, path, expected) in [
        (
            json!({"ordinary_usage_allowed":false}),
            "/ordinary_usage_allowed",
            json!(false),
        ),
        (
            json!({"limit_reached":{"type":"weekly"}}),
            "/limit_reached/type",
            json!("weekly"),
        ),
    ] {
        let (world, _) = world().await;
        listen(&world, "needs_setup", reported);
        world
            .engine()
            .refresh_provider(world.provider)
            .await
            .unwrap();
        let (status, facts): (String, Json) =
            sqlx::query_as("SELECT status,facts FROM providers WHERE id=$1")
                .bind(world.provider)
                .fetch_one(world.pool.pool())
                .await
                .unwrap();
        assert_eq!(status, "needs_setup");
        assert_eq!(
            facts.pointer(path),
            Some(&expected),
            "the reported allowance remains recorded alongside the sign-in state"
        );
    }
}

#[tokio::test]
async fn a_programs_admission_members_hold_a_key_provider_and_usage_allowed_releases_its_waiting_work()
 {
    let (world, clock) = world().await;
    sqlx::query("UPDATE providers SET sources=$2 WHERE id=$1")
        .bind(world.provider)
        .bind(json!({"facts":{"by":"program","name":"plan","config":{}}}))
        .execute(world.pool.pool())
        .await
        .unwrap();
    listen(
        &world,
        "ready",
        json!({"ordinary_usage_allowed":false,"limit_reached":{"type":"weekly","resets_at":Time::new(clock.now()+Duration::minutes(2))},"field_times":{"ordinary_usage_allowed":clock.time(),"limit_reached":clock.time()}}),
    );
    world
        .engine()
        .refresh_provider(world.provider)
        .await
        .unwrap();
    let status: String = world
        .scalar("SELECT status FROM providers WHERE id=$1", world.provider)
        .await;
    assert_eq!(
        status, "exhausted",
        "admission members govern a key provider even if the role called itself ready"
    );
    let thread = world.thread("Waiting for the program").await;
    world
        .engine()
        .submit_input(
            &orch_authz::Caller::session(world.ada),
            thread,
            common::queued("Use the plan when it is available"),
        )
        .await
        .unwrap();
    world.wait_blocked(thread, "provider_exhausted").await;
    clock.advance(Duration::minutes(1));
    listen(
        &world,
        "ready",
        json!({"ordinary_usage_allowed":true,"limit_reached":null,"field_times":{"ordinary_usage_allowed":clock.time(),"limit_reached":clock.time()}}),
    );
    world
        .engine()
        .refresh_provider(world.provider)
        .await
        .unwrap();
    let status: String = world
        .scalar("SELECT status FROM providers WHERE id=$1", world.provider)
        .await;
    assert_eq!(status, "ready");
    let started = world.turn_start(thread, 0).await;
    world.say(thread, common::turn_began(started.turn_id)).await;
    world.wait_state(thread, "running").await;
}

#[tokio::test]
async fn a_successful_key_read_keeps_denied_credits_with_their_original_value_and_source_time() {
    let (world, clock) = world().await;
    let before = clock.time();
    let known = json!({"key_credits":{"unit":"USD","total_credits":20.0,"total_usage":3.0},
        "field_times":{"key_credits":before}});
    sqlx::query("UPDATE providers SET facts = $2 WHERE id = $1")
        .bind(world.provider)
        .bind(known)
        .execute(world.pool.pool())
        .await
        .unwrap();
    clock.advance(Duration::minutes(5));
    let now = clock.time();
    listen(
        &world,
        "ready",
        json!({"key_usage_supported":true,"key_usage":{
        "vendor":"OpenRouter","unit":"USD","limit":20.0,"remaining":16.0,
        "used":4.0,"used_daily":1.0,"used_weekly":2.0,"used_monthly":3.0},
        "key_credits":null,"field_times":{"key_usage":now}}),
    );
    world
        .engine()
        .refresh_provider(world.provider)
        .await
        .unwrap();
    let (facts, at): (Json, chrono::DateTime<chrono::Utc>) =
        sqlx::query_as("SELECT facts,facts_at FROM providers WHERE id = $1")
            .bind(world.provider)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    let total = facts["spend"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == "key:total")
        .unwrap();
    let account = facts["balances"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == "account")
        .unwrap();
    assert_eq!(total["used"], "4");
    assert_eq!(account["used"], "3");
    assert_eq!(facts["field_times"]["spend.key:total"], json!(now));
    assert_eq!(facts["field_times"]["balances.account"], json!(before));
    assert!(facts.get("key_usage").is_none() && facts.get("key_credits").is_none());
    assert_eq!(at, clock.now());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_old_credentials_vendor_answer_cannot_recover_a_replacement_started_during_the_read() {
    let (world, clock) = world().await;
    world
        .engine()
        .refresh_provider(world.provider)
        .await
        .unwrap();
    clock.advance(Duration::minutes(1));
    sqlx::query("UPDATE providers SET status = 'error',status_detail = 'the credential was refused',updated_at = $2 WHERE id = $1")
        .bind(world.provider).bind(clock.now()).execute(world.pool.pool()).await.unwrap();
    let pool = world.pool.pool().clone();
    let provider = world.provider;
    let answered = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = answered.clone();
    world.machine.handler.on(CommandKind::ProviderRefresh,move |_| {
        seen.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
        tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(async {
            sqlx::query("UPDATE providers SET login_state = $2 WHERE id = $1")
                .bind(provider).bind(json!({"state":"starting","method":"access_token","shows":[],"asks":null,"expires_at":null,"error":null}))
                .execute(&pool).await.unwrap();
        }));
        Ok(CommandResult::ProviderRefresh(ProviderRefreshResult {source:None,status:"ready".into(),
            facts:serde_json::from_value(json!({"plan":"old-plan","windows":[],"credential_verified":true})).unwrap()}))
    });
    world
        .engine()
        .refresh_provider(world.provider)
        .await
        .unwrap();
    assert_eq!(answered.load(std::sync::atomic::Ordering::SeqCst), 1);
    let (status, facts): (String, Json) =
        sqlx::query_as("SELECT status,facts FROM providers WHERE id = $1")
            .bind(world.provider)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(status, "error");
    assert_ne!(facts.get("plan"), Some(&json!("old-plan")));
}
