//! Synthetic failed turns prove safe repetition, visible authored continuations, configured failover and durable loop stops.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

#[path = "call_recovery/auxiliary.rs"]
mod auxiliary;
mod common;

use common::{World, eventually, queued, turn_began};
use orch_authz::Caller;
use orch_link_contract::events::{Event, TurnCompleted, TurnError, TurnOutcome};
use orch_provider_contract::calls::{Purpose, Step};
use orch_types::{Id, Json};
use serde_json::json;

async fn failed(world: &World, thread: Id, turn: Id, kind: &str) {
    world.say(thread,Event::TurnCompleted(TurnCompleted {turn_id:turn,status:TurnOutcome::Failed,
        error:Some(serde_json::from_value::<TurnError>(json!({"code":kind,"message":"A synthetic failure","retryable":true,"said":"scripted model failure"})).unwrap())})).await;
    world.wait_turn(turn, "failed").await;
}

async fn recovery(world: &World, thread: Id) -> orch_engine::calls::Recovery {
    let mut conn = world.pool.acquire().await.unwrap();
    orch_engine::calls::read(&mut conn, thread, Purpose::Agent)
        .await
        .unwrap()
        .unwrap()
}

async fn immediate(world: &World, _thread: Id, kind: &str) {
    world.set(world.topic,"calls.rules",json!([{"id":"retry","when":{"kind":kind},"steps":[{"type":"retry","delay_secs":0,"times":20,"backoff":1},{"type":"wait","until":"person"}]}])).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failure_without_effects_reuses_the_same_input_and_its_author() {
    let world = World::new().await;
    let thread = world.thread("Repeat safely").await;
    immediate(&world, thread, "provider_busy").await;
    let input = world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Make the change"),
        )
        .await
        .unwrap();
    let first = world.turn_start(thread, 0).await;
    world.say(thread, turn_began(first.turn_id)).await;
    failed(&world, thread, first.turn_id, "provider_busy").await;
    let second = world.turn_start(thread, 1).await;
    let row: (Id,Option<Id>)=sqlx::query_as("SELECT t.input_id,i.author_id FROM turns t JOIN thread_inputs i ON i.id=t.input_id WHERE t.id=$1")
        .bind(second.turn_id).fetch_one(world.pool.pool()).await.unwrap();
    assert_eq!(row, (input.input.id, Some(world.ada)));
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM thread_inputs WHERE thread_id=$1",
                thread
            )
            .await,
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_partial_answer_is_continued_visibly_in_the_original_authors_name() {
    let world = World::new().await;
    let author = world.person("input_author").await;
    world.give(author, "developer").await;
    let thread = world.thread_by(author, "Continue work").await;
    immediate(&world, thread, "model_refused").await;
    world
        .engine()
        .submit_input(&Caller::session(author), thread, queued("Make the change"))
        .await
        .unwrap();
    let first = world.turn_start(thread, 0).await;
    world.say(thread, turn_began(first.turn_id)).await;
    world
        .say(
            thread,
            common::message_began(first.turn_id, "native-answer"),
        )
        .await;
    world
        .say(
            thread,
            common::message_grew("native-answer", 0, "Already changed one file"),
        )
        .await;
    failed(&world, thread, first.turn_id, "model_refused").await;
    let second = world.turn_start(thread, 1).await;
    world.say(thread, turn_began(second.turn_id)).await;
    let status = orch_engine::calls::view(&mut world.pool.acquire().await.unwrap(), thread)
        .await
        .unwrap()
        .unwrap();
    assert!(
        status.continuing,
        "retained effects, rather than the rule step name, determine the visible action"
    );
    assert!(matches!(status.step, Some(Step::Retry { .. })));
    let input:(Option<Id>,Json)=sqlx::query_as("SELECT i.author_id,i.content FROM turns t JOIN thread_inputs i ON i.id=t.input_id WHERE t.id=$1")
        .bind(second.turn_id).fetch_one(world.pool.pool()).await.unwrap();
    assert_eq!(input.0, Some(author));
    assert_ne!(
        input.0,
        Some(world.ada),
        "the rule writer is not the continuation sender"
    );
    assert_eq!(input.1, json!([{"type":"text","text":"Continue"}]));
    eventually("automatic input is visible",||async {
        world.count("SELECT count(*) FROM items WHERE thread_id=$1 AND kind='user_message' AND payload->'automatic'->>'rule_id'='retry'",thread).await==1
    }).await;
    let marker: Json = sqlx::query_scalar("SELECT payload FROM items WHERE thread_id=$1 AND kind='marker' AND payload->>'marker'='call_recovery' AND payload->'data'->>'action'='continue' ORDER BY seq_created DESC LIMIT 1")
        .bind(thread).fetch_one(world.pool.pool()).await.unwrap();
    assert_eq!(marker["data"]["attempt"], 1);
    assert_eq!(marker["data"]["total"], 20);
    world.set(world.topic,"calls.rules",json!([{"id":"retry","when":{"kind":"model_refused"},"steps":[{"type":"retry","delay_secs":0,"times":99,"backoff":1}]}])).await;
    let retained: Json = sqlx::query_scalar("SELECT payload FROM items WHERE thread_id=$1 AND kind='marker' AND payload->>'marker'='call_recovery' AND payload->'data'->>'action'='continue' ORDER BY seq_created DESC LIMIT 1")
        .bind(thread).fetch_one(world.pool.pool()).await.unwrap();
    assert_eq!(
        retained["data"]["total"], 20,
        "later rule edits cannot rewrite an executed attempt's limit"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_fourth_recovery_in_the_configured_window_stops_without_a_notice_by_default() {
    let world = World::new().await;
    let thread = world.thread("A loop stops").await;
    immediate(&world, thread, "provider_busy").await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Try the service"),
        )
        .await
        .unwrap();
    for at in 0..4 {
        let turn = world.turn_start(thread, at).await;
        world.say(thread, turn_began(turn.turn_id)).await;
        failed(&world, thread, turn.turn_id, "provider_busy").await;
    }
    eventually("guard stops the loop", || async {
        recovery(&world, thread).await.state == "stopped"
    })
    .await;
    assert_eq!(world.turn_starts(thread).len(), 4);
    let state = recovery(&world, thread).await;
    assert_eq!(state.action.unwrap()["count"], 4);
    assert_eq!(
        world
            .scalar::<String>("SELECT state FROM threads WHERE node_id=$1", thread)
            .await,
        "blocked"
    );
    assert_eq!(world.count("SELECT count(*) FROM notifications WHERE thread_id=$1 AND kind IN ('turn.failed','thread.blocked')",thread).await,0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_model_with_ungranted_access_is_never_retried_by_the_shipped_rules() {
    let world = World::new().await;
    let thread = world.thread("Missing access").await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Use this model"),
        )
        .await
        .unwrap();
    let turn = world.turn_start(thread, 0).await;
    world.say(thread, turn_began(turn.turn_id)).await;
    failed(&world, thread, turn.turn_id, "model_unavailable").await;
    eventually("structural access waits", || async {
        recovery(&world, thread).await.state == "waiting"
    })
    .await;
    assert_eq!(world.turn_starts(thread).len(), 1);
    assert_eq!(recovery(&world, thread).await.rule["id"], "other");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_place_lock_controls_the_rules_and_guard_of_its_threads() {
    let world = World::new().await;
    let thread = world.thread("Inherited rules").await;
    world
        .set(world.project, "calls.repeat_limit", json!(7))
        .await;
    world.set(world.topic, "calls.repeat_limit", json!(9)).await;
    sqlx::query("UPDATE settings SET locked=true WHERE node_id=$1 AND key='calls.repeat_limit'")
        .bind(world.project)
        .execute(world.pool.pool())
        .await
        .unwrap();
    world
        .set(world.topic, "calls.repeat_window", json!(900))
        .await;
    world
        .set(world.topic, "calls.notify_on_stop", json!(true))
        .await;
    let mut conn = world.pool.acquire().await.unwrap();
    let policy = orch_engine::calls::Policy::load(&mut conn, thread)
        .await
        .unwrap();
    assert_eq!((policy.limit, policy.window, policy.notify), (7, 900, true));
}

#[test]
fn embeddings_never_fail_over_to_an_incomparable_model_or_dimension() {
    use orch_provider_contract::calls::{Candidate, compatible_embedding};
    let mut candidate = Candidate {
        provider_id: Id::nil(),
        model: "same-model".to_owned(),
        agent_id: None,
        params: json!({}),
        dimensions: Some(256),
    };
    assert!(compatible_embedding(&candidate, "same-model", 256));
    assert!(!compatible_embedding(&candidate, "other-model", 256));
    candidate.dimensions = Some(768);
    assert!(!compatible_embedding(&candidate, "same-model", 256));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_configured_model_failover_returns_at_the_next_turn_even_when_recovery_was_stopped() {
    for stop in [false, true] {
        let world = World::new().await;
        let thread = world.thread("A temporary model").await;
        sqlx::query("INSERT INTO provider_models(id,provider_id,model_name,display_name,parameters,enabled,source,created_by) SELECT $1,provider_id,'fallback-model','Fallback',parameters,true,'manual',created_by FROM provider_models WHERE provider_id=$2 AND model_name=$3")
        .bind(orch_types::new_id()).bind(world.provider).bind(common::MODEL).execute(world.pool.pool()).await.unwrap();
        world.set(world.topic,"calls.rules",json!([{"id":"fallback","when":{"kind":"model_refused"},"steps":[{"type":"failover","candidate":{"provider_id":world.provider,"model":"fallback-model","params":{}}},{"type":"wait","until":"person"}]}])).await;
        world
            .engine()
            .submit_input(
                &Caller::session(world.ada),
                thread,
                queued("Use the service"),
            )
            .await
            .unwrap();
        let first = world.turn_start(thread, 0).await;
        world.say(thread, turn_began(first.turn_id)).await;
        failed(&world, thread, first.turn_id, "model_refused").await;
        let fallback = world.turn_start(thread, 1).await;
        assert_eq!(fallback.model, "fallback-model");
        world.say(thread, turn_began(fallback.turn_id)).await;
        if stop {
            let mut tx = world.pool.begin().await.unwrap();
            orch_db::queries::transaction_guard::lock_action(&mut tx)
                .await
                .unwrap();
            let principal = orch_authz::Principal::load(
                &mut tx,
                Caller::session(world.ada),
                orch_types::Time::new(world.clock.now()),
            )
            .await
            .unwrap();
            world
                .engine()
                .control_call_recovery(
                    tx,
                    principal,
                    thread,
                    orch_api_contract::calls::CallRecoveryAction::Stop,
                )
                .await
                .unwrap();
        }
        world
            .say(
                thread,
                common::turn_ended(fallback.turn_id, TurnOutcome::Completed),
            )
            .await;
        world.wait_turn(fallback.turn_id, "completed").await;
        eventually("original choice is retained for the next turn", || async {
            recovery(&world, thread).await.state == "completed"
        })
        .await;
        world
            .engine()
            .submit_input(
                &Caller::session(world.ada),
                thread,
                queued("The next independent turn"),
            )
            .await
            .unwrap();
        let next = world.turn_start(thread, 2).await;
        assert_eq!(next.model, common::MODEL);
        let own: Json = world
            .scalar(
                "SELECT value FROM settings WHERE node_id=$1 AND key='agent.model'",
                thread,
            )
            .await;
        assert_eq!(own, common::MODEL);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_service_recovery_can_fail_over_but_embeddings_refuse_a_different_identity() {
    use orch_engine::calls::{ServiceCall, ServicePlan, plan_service, reserve};
    let world = World::new().await;
    let thread = world.thread("Service policy").await;
    world.engine().stop();
    world.set(world.topic,"calls.rules",json!([{"id":"service-fallback","when":{},"steps":[{"type":"failover","candidate":{"provider_id":world.provider,"model":"fallback-model","dimensions":256,"params":{}}},{"type":"wait","until":"person"}]}])).await;
    for purpose in [Purpose::Summary, Purpose::Embedding] {
        let root = orch_types::new_id();
        let reference = orch_types::new_id();
        let context = json!({"root_input":root,"root_call":root,"configured_provider":world.provider,"configured_model":common::MODEL,"provider_id":world.provider,"model":common::MODEL});
        let mut tx = world.pool.begin().await.unwrap();
        reserve(
            &mut tx,
            thread,
            purpose,
            reference,
            &json!({"code":"provider_busy","message":"Synthetic capacity","retryable":true}),
            context,
            world.clock.now(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let mut conn = world.pool.acquire().await.unwrap();
        let plan = plan_service(
            &mut conn,
            &ServiceCall {
                node: thread,
                purpose,
                root,
                workflow: None,
                task: None,
                provider: world.provider,
                model: common::MODEL,
                dimensions: Some(256),
            },
            world.clock.now(),
        )
        .await
        .unwrap();
        match (purpose, plan) {
            (
                Purpose::Summary,
                ServicePlan::Ready {
                    call,
                    candidate: Some(candidate),
                },
            ) => {
                assert_ne!(call, root);
                assert_eq!(candidate.model, "fallback-model");
            }
            (Purpose::Embedding, ServicePlan::Paused) => {
                let paused = orch_engine::calls::read(&mut conn, thread, purpose)
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(paused.state, "waiting");
                assert_eq!(paused.context["skipped"], "embedding_identity");
            }
            _ => panic!("the purpose's model identity must decide failover"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_due_retry_survives_an_engine_restart_and_keeps_its_input() {
    let world = World::new().await;
    let thread = world.thread("Durable retry").await;
    world.set(world.topic,"calls.rules",json!([{"id":"deferred","when":{"kind":"provider_busy"},"steps":[{"type":"retry","delay_secs":60,"times":3,"backoff":1}]}])).await;
    let input = world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Keep my input"))
        .await
        .unwrap();
    let first = world.turn_start(thread, 0).await;
    world.say(thread, turn_began(first.turn_id)).await;
    failed(&world, thread, first.turn_id, "provider_busy").await;
    world.restart_api().await;
    sqlx::query("UPDATE model_call_recovery SET due_at=$2 WHERE node_id=$1 AND purpose='agent'")
        .bind(thread)
        .bind(world.clock.now())
        .execute(world.pool.pool())
        .await
        .unwrap();
    world.engine().keep_call_recoveries().await.unwrap();
    let second = world.turn_start(thread, 1).await;
    assert_eq!(
        world
            .scalar::<Id>("SELECT input_id FROM turns WHERE id=$1", second.turn_id)
            .await,
        input.input.id
    );
}
