//! Setting-writer accounting, caller revocation and auxiliary compatibility through the same recovery policies.

use super::common::meanwhile;
use super::*;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_administrative_stop_is_not_a_failure_and_its_author_can_be_continued_explicitly() {
    let world = World::new().await;
    let author = world.person("stopped_author").await;
    world.give(author, "developer").await;
    let thread = world.thread("An administrative pause").await;
    world
        .engine()
        .submit_input(&Caller::session(author), thread, queued("Finish the work"))
        .await
        .unwrap();
    let first = world.turn_start(thread, 0).await;
    world.say(thread, turn_began(first.turn_id)).await;
    sqlx::query("UPDATE providers SET status='disabled' WHERE id=$1")
        .bind(world.provider)
        .execute(world.pool.pool())
        .await
        .unwrap();
    world
        .say(
            thread,
            common::turn_ended(
                first.turn_id,
                orch_link_contract::events::TurnOutcome::Interrupted,
            ),
        )
        .await;
    world.wait_turn(first.turn_id, "interrupted").await;
    meanwhile(Duration::from_millis(400)).await;
    assert_eq!(
        world.turn_starts(thread).len(),
        1,
        "switch-off must not create an automatic retry"
    );
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM model_call_recovery_attempts WHERE node_id=$1",
                thread
            )
            .await,
        0
    );
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM model_call_recovery WHERE node_id=$1",
                thread
            )
            .await,
        0
    );
    world.engine().stop();
    sqlx::query("UPDATE providers SET status='ready' WHERE id=$1")
        .bind(world.provider)
        .execute(world.pool.pool())
        .await
        .unwrap();
    let input: Id = world
        .scalar("SELECT input_id FROM turns WHERE id=$1", first.turn_id)
        .await;
    let mut tx = world.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_action(&mut tx)
        .await
        .unwrap();
    let row = orch_db::queries::threads::lock(&mut tx, thread)
        .await
        .unwrap()
        .unwrap();
    let origin = orch_db::queries::threads::input(&mut tx, input)
        .await
        .unwrap()
        .unwrap();
    let expected = (
        origin.author_id,
        origin.submission_authority.clone(),
        origin.run_as,
        origin.run_as_authority.clone(),
    );
    let reference = orch_types::new_id();
    let mut unit = orch_engine::unit::Unit::new(&mut tx, world.clock.now());
    let continued = world
        .engine()
        .continue_original_input_in(&mut unit, &row, &origin, reference, None)
        .await
        .unwrap()
        .unwrap();
    assert!(
        world
            .engine()
            .continue_original_input_in(&mut unit, &row, &origin, reference, None)
            .await
            .unwrap()
            .is_none(),
        "one resume operation creates one Continue"
    );
    unit.finish().await.unwrap();
    tx.commit().await.unwrap();
    let mut conn = world.pool.acquire().await.unwrap();
    let actual = orch_db::queries::threads::input(&mut conn, continued)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            actual.author_id,
            actual.submission_authority,
            actual.run_as,
            actual.run_as_authority
        ),
        expected
    );
    assert_eq!(actual.author_id, Some(author));
    assert_eq!(actual.content, json!([{"type":"text","text":"Continue"}]));
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM model_call_recovery_attempts WHERE node_id=$1",
                thread
            )
            .await,
        0,
        "an administrative resume is not a failed-call attempt"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_switched_off_summary_target_pauses_without_another_request_or_failure_reservation() {
    use orch_link_contract::commands::{CommandError, CommandKind, ErrorCode};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    for (off_model, reason) in [
        (false, orch_engine::calls::PROVIDER_SWITCHED_OFF),
        (true, orch_engine::calls::MODEL_SWITCHED_OFF),
    ] {
        for prior_failure in [false, true] {
            let world = World::on_a_key("synthetic-off-summary-key").await;
            world.engine().stop();
            let thread = world
                .thread("A summary waits while its provider is off")
                .await;
            world.set(world.topic, "auto.title", json!("off")).await;
            world
                .set(world.topic, "auto.description", json!("summarizer"))
                .await;
            world
                .set(
                    world.topic,
                    "auto.summarizer",
                    json!({"kind":"model","provider_id":world.provider,"model_name":common::MODEL}),
                )
                .await;
            world.set(world.topic, "calls.rules", json!([{"id":"retry","when":{},"steps":[{"type":"retry","delay_secs":0,"times":10,"backoff":1}]}])).await;
            sqlx::query("UPDATE nodes SET description_source='auto' WHERE id=$1")
                .bind(thread)
                .execute(world.pool.pool())
                .await
                .unwrap();
            let calls = Arc::new(AtomicUsize::new(0));
            let count = calls.clone();
            world.machine.handler.on(CommandKind::ProviderComplete, move |_| {
            count.fetch_add(1, Ordering::SeqCst);
            let mut request = orch_agent_contract::requests::UsageRequest { id: "off-summary".to_owned(), ..Default::default() };
            request.finish("provider_busy", Some("Synthetic capacity"), Some("Synthetic capacity"));
            Err(CommandError::new(ErrorCode::Internal, "Synthetic capacity").with_details(json!({"failure":{"code":"provider_busy","status":503},"model_request":request})))
        });
            {
                let mut conn = world.pool.acquire().await.unwrap();
                orch_engine::summaries::due(&mut conn, thread, "refresh", world.clock.now())
                    .await
                    .unwrap();
            }
            if prior_failure {
                assert!(world.engine().summarize_due(thread).await.unwrap());
            }
            let previous = calls.load(Ordering::SeqCst);
            let failures = world
                .count(
                    "SELECT count(*) FROM model_call_recovery_attempts WHERE node_id=$1",
                    thread,
                )
                .await;
            if off_model {
                sqlx::query("UPDATE provider_models SET enabled=false WHERE provider_id=$1")
                    .bind(world.provider)
                    .execute(world.pool.pool())
                    .await
                    .unwrap();
            } else {
                sqlx::query("UPDATE providers SET status='disabled' WHERE id=$1")
                    .bind(world.provider)
                    .execute(world.pool.pool())
                    .await
                    .unwrap();
            }
            assert!(world.engine().summarize_due(thread).await.unwrap());
            assert_eq!(
                calls.load(Ordering::SeqCst),
                previous,
                "no model request is sent to the switched-off provider"
            );
            assert_eq!(
                world
                    .count(
                        "SELECT count(*) FROM model_call_recovery_attempts WHERE node_id=$1",
                        thread
                    )
                    .await,
                failures
            );
            let paused: (Option<String>, Option<chrono::DateTime<chrono::Utc>>) =
                sqlx::query_as("SELECT reason,due_at FROM node_summary_state WHERE node_id=$1")
                    .bind(thread)
                    .fetch_one(world.pool.pool())
                    .await
                    .unwrap();
            assert_eq!(paused, (Some(reason.to_owned()), None));
            // Make ordinary cadence eligible; only the deliberate pause may prevent another call.
            sqlx::query("UPDATE node_summary_state SET next_at=$2 WHERE node_id=$1")
                .bind(thread)
                .bind(world.clock.now())
                .execute(world.pool.pool())
                .await
                .unwrap();
            assert!(
                !world.engine().summarize_due(thread).await.unwrap(),
                "switch-off does not become a periodic retry"
            );
            assert_eq!(calls.load(Ordering::SeqCst), previous);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_candidate_is_skipped_when_the_input_author_loses_use_even_if_the_rule_writer_has_it() {
    let world = World::new().await;
    let author = world.person("input_author").await;
    world.give(author, "developer").await;
    let thread = world.thread_by(author, "Own authority").await;
    world.set(world.topic,"calls.rules",json!([{"id":"fallback","when":{},"steps":[{"type":"failover","candidate":{"provider_id":world.provider,"model":common::MODEL,"params":{}}},{"type":"wait","until":"person"}]}])).await;
    world
        .engine()
        .submit_input(&Caller::session(author), thread, queued("Start work"))
        .await
        .unwrap();
    let first = world.turn_start(thread, 0).await;
    world.say(thread, turn_began(first.turn_id)).await;
    world.access.deny(author);
    failed(&world, thread, first.turn_id, "provider_busy").await;
    eventually("the unusable candidate is skipped", || async {
        recovery(&world, thread).await.state == "waiting"
    })
    .await;
    assert_eq!(world.turn_starts(thread).len(), 1);
    assert_eq!(
        recovery(&world, thread).await.context["skipped"],
        "candidate_not_allowed"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_summary_uses_its_configured_candidate_and_counts_that_model_to_the_setting_writer()
 {
    use orch_link_contract::commands::{
        Command, CommandError, CommandKind, CommandResult, ErrorCode, ProviderCompleteResult,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let world = World::on_a_key("synthetic-summary-key").await;
    world.engine().stop();
    let thread = world.thread("Summary recovery").await;
    sqlx::query("INSERT INTO provider_models(id,provider_id,model_name,display_name,parameters,enabled,source,created_by) SELECT $1,provider_id,'fallback-model','Fallback',parameters,true,'manual',created_by FROM provider_models WHERE provider_id=$2 AND model_name=$3")
        .bind(orch_types::new_id()).bind(world.provider).bind(common::MODEL).execute(world.pool.pool()).await.unwrap();
    world.set(world.topic, "auto.title", json!("off")).await;
    world
        .set(world.topic, "auto.description", json!("summarizer"))
        .await;
    world
        .set(
            world.topic,
            "auto.summarizer",
            json!({"kind":"model","provider_id":world.provider,"model_name":common::MODEL}),
        )
        .await;
    world.set(world.topic,"calls.rules",json!([{"id":"summary-fallback","when":{"purpose":"summary"},"steps":[{"type":"failover","candidate":{"provider_id":world.provider,"model":"fallback-model","params":{}}},{"type":"wait","until":"person"}]}])).await;
    sqlx::query("UPDATE nodes SET description_source='auto' WHERE id=$1")
        .bind(thread)
        .execute(world.pool.pool())
        .await
        .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    world.machine.handler.on(CommandKind::ProviderComplete,move |command| {
        let Command::ProviderComplete(request)=command else {return Err(CommandError::new(ErrorCode::Internal,"not a completion"));};
        if counted.fetch_add(1,Ordering::SeqCst)==0 {
            assert_eq!(request.protocol,"openai-responses");
            let mut observation=orch_agent_contract::requests::UsageRequest {id:"completion".to_owned(),..Default::default()};
            observation.finish("provider_busy",Some("Synthetic capacity"),Some("Model at capacity"));
            return Err(CommandError::new(ErrorCode::Internal,"Synthetic capacity").with_details(json!({"failure":{"code":"provider_busy","status":503},"model_request":observation})));
        }
        assert_eq!(request.protocol,"anthropic-messages");
        assert_eq!(request.model_name,"fallback-model");
        Ok(CommandResult::ProviderComplete(ProviderCompleteResult {text:"A recovered description".to_owned(),usage:orch_agent_contract::events::TokenUsage::from_parts(orch_api_contract::threads::TokensReportedApart {input:100,output:5,..Default::default()}),request_ref:Some("synthetic-summary".to_owned()),timing:Default::default()}))
    });
    {
        let mut conn = world.pool.acquire().await.unwrap();
        orch_engine::summaries::due(&mut conn, thread, "refresh", world.clock.now())
            .await
            .unwrap();
    }
    assert!(world.engine().summarize_due(thread).await.unwrap());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    sqlx::query("UPDATE providers SET protocols=$2 WHERE id=$1")
        .bind(world.provider)
        .bind(vec!["anthropic-messages".to_owned()])
        .execute(world.pool.pool())
        .await
        .unwrap();
    assert!(world.engine().summarize_due(thread).await.unwrap());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        world
            .scalar::<String>("SELECT description FROM nodes WHERE id=$1", thread)
            .await,
        "A recovered description"
    );
    let actual:(String,Option<Id>)=sqlx::query_as("SELECT model,user_id FROM usage_events WHERE thread_id=$1 AND request_ref='synthetic-summary'").bind(thread).fetch_one(world.pool.pool()).await.unwrap();
    assert_eq!(actual, ("fallback-model".to_owned(), Some(world.ada)));
    assert_eq!(world.count("SELECT count(*) FROM usage_events WHERE thread_id=$1 AND request_ref='synthetic-summary'",thread).await,1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_explicit_access_program_is_sent_only_when_the_profile_feature_and_model_allow_it() {
    use orch_link_contract::commands::{
        AgentStartResult, Command, CommandError, CommandKind, CommandResult, ErrorCode,
    };
    for enabled in [true, false] {
        let world = World::new().await;
        let mut descriptor = common::descriptor();
        descriptor["parameters"] = json!([{"key":"access_program","title":"Access program","description":"Synthetic authorised program","group":"safety","type":"enum","options":{"from_model":"access_programs"},"scopes":["thread"],"apply":"next_turn","eligible_when":{"model_has":"access_programs"},"feature":"synthetic.program"}]);
        sqlx::query("UPDATE agents SET descriptor=$2 WHERE id=$1")
            .bind(world.agent)
            .bind(&descriptor)
            .execute(world.pool.pool())
            .await
            .unwrap();
        world
            .machine
            .handler
            .on(CommandKind::AgentStart, move |command| {
                if !matches!(command, Command::AgentStart(_)) {
                    return Err(CommandError::new(ErrorCode::Internal, "not an agent start"));
                }
                Ok(CommandResult::AgentStart(AgentStartResult {
                    reused: false,
                    harness_version: "1.0.0".to_owned(),
                    descriptor: serde_json::from_value(descriptor.clone()).unwrap(),
                }))
            });
        sqlx::query("UPDATE provider_models SET parameters=parameters || $3 WHERE provider_id=$1 AND model_name=$2")
            .bind(world.provider).bind(common::MODEL).bind(json!({"access_programs":["blue"]})).execute(world.pool.pool()).await.unwrap();
        world
            .set(
                world.topic,
                "agent.features",
                json!({"synthetic.program":enabled}),
            )
            .await;
        world.set(world.topic,"calls.rules",json!([{"id":"access-fallback","when":{"kind":"model_refused"},"steps":[{"type":"failover","candidate":{"provider_id":world.provider,"model":common::MODEL,"params":{"access_program":"blue"}}},{"type":"wait","until":"person"}]}])).await;
        let thread = world.thread("Authorised program recovery").await;
        world
            .engine()
            .submit_input(
                &Caller::session(world.ada),
                thread,
                queued("Complete the work"),
            )
            .await
            .unwrap();
        let first = world.turn_start(thread, 0).await;
        world.say(thread, turn_began(first.turn_id)).await;
        failed(&world, thread, first.turn_id, "model_refused").await;
        if enabled {
            let next = world.turn_start(thread, 1).await;
            assert_eq!(next.params.get("access_program"), Some(&json!("blue")));
        } else {
            eventually("ineligible candidate is skipped", || async {
                recovery(&world, thread).await.state == "waiting"
            })
            .await;
            assert_eq!(world.turn_starts(thread).len(), 1);
            assert_eq!(
                recovery(&world, thread).await.context["skipped"],
                "candidate_parameters"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn opting_in_tells_the_owner_once_for_the_same_guard_stop() {
    let world = World::new().await;
    world
        .set(world.topic, "calls.notify_on_stop", json!(true))
        .await;
    world.set(world.topic, "calls.repeat_limit", json!(0)).await;
    let thread = world.thread("An opted-in stop notice").await;
    immediate(&world, thread, "provider_busy").await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Do the work"))
        .await
        .unwrap();
    let turn = world.turn_start(thread, 0).await;
    world.say(thread, turn_began(turn.turn_id)).await;
    failed(&world, thread, turn.turn_id, "provider_busy").await;
    eventually("the owner is told of the guard stop",||async {world.count("SELECT count(*) FROM notifications WHERE thread_id=$1 AND kind='thread.blocked' AND data->>'reason'='repeat_limit'",thread).await==1}).await;
    world.engine().keep_call_recoveries().await.unwrap();
    assert_eq!(world.turn_starts(thread).len(), 1);
    assert_eq!(world.count("SELECT count(*) FROM notifications WHERE thread_id=$1 AND kind='thread.blocked' AND data->>'reason'='repeat_limit'",thread).await,1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replacing_a_rules_steps_while_an_attempt_runs_starts_the_new_chain() {
    let world = World::new().await;
    let thread = world.thread("A revised chain").await;
    immediate(&world, thread, "provider_busy").await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Start the work"),
        )
        .await
        .unwrap();
    let first = world.turn_start(thread, 0).await;
    world.say(thread, turn_began(first.turn_id)).await;
    failed(&world, thread, first.turn_id, "provider_busy").await;
    let second = world.turn_start(thread, 1).await;
    world.say(thread, turn_began(second.turn_id)).await;
    sqlx::query("INSERT INTO provider_models(id,provider_id,model_name,display_name,parameters,enabled,source,created_by) SELECT $1,provider_id,'fallback-model','Fallback',parameters,true,'manual',created_by FROM provider_models WHERE provider_id=$2 AND model_name=$3")
        .bind(orch_types::new_id()).bind(world.provider).bind(common::MODEL).execute(world.pool.pool()).await.unwrap();
    world.set(world.topic,"calls.rules",json!([{"id":"retry","when":{"kind":"provider_busy"},"steps":[{"type":"failover","candidate":{"provider_id":world.provider,"model":"fallback-model","params":{}}},{"type":"wait","until":"person"}]}])).await;
    failed(&world, thread, second.turn_id, "provider_busy").await;
    let third = world.turn_start(thread, 2).await;
    assert_eq!(third.model, "fallback-model");
    assert_eq!(recovery(&world, thread).await.action.unwrap()["attempt"], 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn structural_model_unavailability_keeps_the_candidate_until_configuration_changes() {
    let world = World::new().await;
    let thread = world.thread("Wait for the original model").await;
    sqlx::query("INSERT INTO provider_models(id,provider_id,model_name,display_name,parameters,enabled,source,created_by) SELECT $1,provider_id,'fallback-model','Fallback',parameters,true,'manual',created_by FROM provider_models WHERE provider_id=$2 AND model_name=$3")
        .bind(orch_types::new_id()).bind(world.provider).bind(common::MODEL).execute(world.pool.pool()).await.unwrap();
    world.set(world.topic,"calls.rules",json!([{"id":"fallback","when":{"kind":"model_unavailable"},"steps":[{"type":"failover","candidate":{"provider_id":world.provider,"model":"fallback-model","params":{}}}]}])).await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Start work"))
        .await
        .unwrap();
    let first = world.turn_start(thread, 0).await;
    world.say(thread, turn_began(first.turn_id)).await;
    failed(&world, thread, first.turn_id, "model_unavailable").await;
    let candidate = world.turn_start(thread, 1).await;
    assert_eq!(candidate.model, "fallback-model");
    world.say(thread, turn_began(candidate.turn_id)).await;
    world
        .say(
            thread,
            common::turn_ended(candidate.turn_id, TurnOutcome::Completed),
        )
        .await;
    world.wait_turn(candidate.turn_id, "completed").await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Another independent task"),
        )
        .await
        .unwrap();
    let stayed = world.turn_start(thread, 2).await;
    assert_eq!(
        stayed.model, "fallback-model",
        "unchanged local facts cannot grant structural model access"
    );
    let before: Json = sqlx::query_scalar(
        "SELECT parameters FROM provider_models WHERE provider_id=$1 AND model_name=$2",
    )
    .bind(world.provider)
    .bind(common::MODEL)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    let changed_images = !before["input_images"].as_bool().unwrap_or(false);
    sqlx::query("UPDATE provider_models SET parameters=parameters || $3 WHERE provider_id=$1 AND model_name=$2")
        .bind(world.provider).bind(common::MODEL).bind(json!({"input_images":changed_images})).execute(world.pool.pool()).await.unwrap();
    let after: Json = sqlx::query_scalar(
        "SELECT parameters FROM provider_models WHERE provider_id=$1 AND model_name=$2",
    )
    .bind(world.provider)
    .bind(common::MODEL)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_ne!(
        before, after,
        "the fixture supplies genuinely changed catalogue evidence"
    );
    world.say(thread, turn_began(stayed.turn_id)).await;
    world
        .say(
            thread,
            common::turn_ended(stayed.turn_id, TurnOutcome::Completed),
        )
        .await;
    world.wait_turn(stayed.turn_id, "completed").await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Use the refreshed route"),
        )
        .await
        .unwrap();
    let returned = world.turn_start(thread, 3).await;
    assert_eq!(returned.model, common::MODEL);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn vendor_reset_and_retry_after_are_known_waits_for_the_same_policy() {
    let world = World::new().await;
    world.engine().stop();
    let thread = world.thread("Vendor wait evidence").await;
    let now = world.clock.time().get();
    let reset = now + chrono::Duration::hours(1);
    for (kind, step, failure, due) in [
        (
            "provider_busy",
            json!({"type":"retry","delay_secs":60,"times":3,"backoff":2}),
            json!({"code":"provider_busy","message":"Synthetic reset","retryable":true,"resets_at":orch_types::Time::new(reset)}),
            reset,
        ),
        (
            "provider_exhausted",
            json!({"type":"wait","until":"reset"}),
            json!({"code":"provider_exhausted","message":"Synthetic quota","retryable":false,"retry_after_secs":11}),
            now + chrono::Duration::seconds(11),
        ),
    ] {
        world
            .set(
                world.topic,
                "calls.rules",
                json!([{"id":"vendor-time","when":{"kind":kind},"steps":[step]}]),
            )
            .await;
        let mut tx = world.pool.begin().await.unwrap();
        let row=orch_engine::calls::reserve(&mut tx,thread,Purpose::Summary,orch_types::new_id(),&failure,json!({"root_input":orch_types::new_id(),"provider_id":world.provider,"model":common::MODEL}),now).await.unwrap().unwrap();
        assert_eq!(row.state, "scheduled");
        assert_eq!(row.due_at, Some(due));
        tx.commit().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_subject_shares_its_failure_guard_across_model_call_purposes() {
    let world = World::new().await;
    world.engine().stop();
    let thread = world.thread("One repeat guard").await;
    world.set(world.topic, "calls.repeat_limit", json!(1)).await;
    immediate(&world, thread, "provider_busy").await;
    for (purpose, expected) in [(Purpose::Summary, "scheduled"), (Purpose::Agent, "stopped")] {
        let mut tx = world.pool.begin().await.unwrap();
        let row=orch_engine::calls::reserve(&mut tx,thread,purpose,orch_types::new_id(),&json!({"code":"provider_busy","message":"Synthetic capacity","retryable":true}),json!({"root_input":orch_types::new_id(),"provider_id":world.provider,"model":common::MODEL}),world.clock.now()).await.unwrap().unwrap();
        assert_eq!(row.state, expected);
        if purpose == Purpose::Agent {
            assert_eq!(row.action.unwrap()["count"], 2);
        }
        tx.commit().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_model_failure_without_a_matching_rule_does_not_use_the_old_summary_retry_timer() {
    use orch_link_contract::commands::{CommandError, CommandKind, ErrorCode};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let world = World::on_a_key("synthetic-summary-key").await;
    world.engine().stop();
    let thread = world.thread("No implicit recovery").await;
    world.set(world.topic, "auto.title", json!("off")).await;
    world
        .set(world.topic, "auto.description", json!("summarizer"))
        .await;
    world
        .set(
            world.topic,
            "auto.summarizer",
            json!({"kind":"model","provider_id":world.provider,"model_name":common::MODEL}),
        )
        .await;
    world.set(world.topic, "calls.rules", json!([])).await;
    sqlx::query("UPDATE nodes SET description_source='auto' WHERE id=$1")
        .bind(thread)
        .execute(world.pool.pool())
        .await
        .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    world
        .machine
        .handler
        .on(CommandKind::ProviderComplete, move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            Err(CommandError::new(ErrorCode::Internal, "Synthetic capacity")
                .with_details(json!({"failure":{"code":"provider_busy","status":503}})))
        });
    {
        let mut conn = world.pool.acquire().await.unwrap();
        orch_engine::summaries::due(&mut conn, thread, "refresh", world.clock.now())
            .await
            .unwrap();
    }
    assert!(world.engine().summarize_due(thread).await.unwrap());
    // Even beyond the ordinary cadence, a paused failure has no due work to retry.
    sqlx::query("UPDATE node_summary_state SET next_at=$2 WHERE node_id=$1")
        .bind(thread)
        .bind(world.clock.now() - chrono::Duration::hours(1))
        .execute(world.pool.pool())
        .await
        .unwrap();
    assert!(!world.engine().summarize_due(thread).await.unwrap());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let due: Option<chrono::DateTime<chrono::Utc>> = world
        .scalar(
            "SELECT due_at FROM node_summary_state WHERE node_id=$1",
            thread,
        )
        .await;
    assert!(due.is_none());
}

fn service<'a>(
    world: &World,
    node: Id,
    root: Id,
    workflow: Id,
    task: &'a str,
) -> orch_engine::calls::ServiceCall<'a> {
    orch_engine::calls::ServiceCall {
        node,
        purpose: Purpose::Summary,
        root,
        workflow: Some(workflow),
        task: Some(task),
        provider: world.provider,
        model: common::MODEL,
        dimensions: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_service_workflows_keep_their_own_cursor_and_completion() {
    let world = World::new().await;
    world.engine().stop();
    let thread = world.thread("Concurrent summaries").await;
    immediate(&world, thread, "provider_busy").await;
    let (a, b, wa, wb) = (
        orch_types::new_id(),
        orch_types::new_id(),
        orch_types::new_id(),
        orch_types::new_id(),
    );
    for (root, workflow, task) in [(a, wa, "title"), (b, wb, "handoff")] {
        let mut tx = world.pool.begin().await.unwrap();
        orch_engine::calls::reserve_service(
            &mut tx,
            &service(&world, thread, root, workflow, task),
            orch_types::new_id(),
            &json!({"code":"provider_busy","message":"Synthetic capacity","retryable":true}),
            json!({"provider_id":world.provider,"model":common::MODEL}),
            world.clock.now(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    let mut attempts = Vec::new();
    for (root, workflow, task) in [(a, wa, "title"), (b, wb, "handoff")] {
        let mut tx = world.pool.begin().await.unwrap();
        let plan = orch_engine::calls::plan_service(
            &mut tx,
            &service(&world, thread, root, workflow, task),
            world.clock.now(),
        )
        .await
        .unwrap();
        let orch_engine::calls::ServicePlan::Ready { call, .. } = plan else {
            panic!("each due workflow has its own retry");
        };
        attempts.push(call);
        tx.commit().await.unwrap();
    }
    assert_ne!(attempts[0], attempts[1]);
    let mut conn = world.pool.acquire().await.unwrap();
    orch_engine::calls::finish_service(&mut conn, thread, Purpose::Summary, a)
        .await
        .unwrap();
    assert_eq!(
        orch_engine::calls::read_root(&mut conn, thread, Purpose::Summary, a)
            .await
            .unwrap()
            .unwrap()
            .state,
        "completed"
    );
    assert_eq!(
        orch_engine::calls::read_root(&mut conn, thread, Purpose::Summary, b)
            .await
            .unwrap()
            .unwrap()
            .state,
        "running"
    );
    assert_eq!(
        orch_engine::calls::read_workflow(&mut conn, thread, Purpose::Summary, wb)
            .await
            .unwrap()
            .unwrap()
            .call_key,
        b
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refreshing_automation_cannot_release_a_handoff_waiting_for_a_person() {
    let world = World::new().await;
    world.engine().stop();
    let thread = world.thread("Independent refresh").await;
    let (a, b, wa, wb) = (
        orch_types::new_id(),
        orch_types::new_id(),
        orch_types::new_id(),
        orch_types::new_id(),
    );
    for (root, workflow, task) in [(a, wa, "title"), (b, wb, "handoff")] {
        let mut tx = world.pool.begin().await.unwrap();
        orch_engine::calls::reserve_service(
            &mut tx,
            &service(&world, thread, root, workflow, task),
            orch_types::new_id(),
            &json!({"code":"harness_error","message":"Synthetic failure","retryable":false}),
            json!({"provider_id":world.provider,"model":common::MODEL}),
            world.clock.now(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    let mut conn = world.pool.acquire().await.unwrap();
    orch_engine::summaries::due(&mut conn, thread, "refresh", world.clock.now())
        .await
        .unwrap();
    assert_eq!(
        orch_engine::calls::read_root(&mut conn, thread, Purpose::Summary, a)
            .await
            .unwrap()
            .unwrap()
            .state,
        "dismissed"
    );
    assert_eq!(
        orch_engine::calls::read_root(&mut conn, thread, Purpose::Summary, b)
            .await
            .unwrap()
            .unwrap()
            .state,
        "waiting"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_service_retry_rechecks_a_limit_disabled_after_the_failure() {
    let world = World::new().await;
    world.engine().stop();
    let thread = world.thread("Changed service limit").await;
    immediate(&world, thread, "provider_busy").await;
    let root = orch_types::new_id();
    let workflow = orch_types::new_id();
    let mut tx = world.pool.begin().await.unwrap();
    orch_engine::calls::reserve_service(
        &mut tx,
        &service(&world, thread, root, workflow, "description"),
        orch_types::new_id(),
        &json!({"code":"provider_busy","message":"Synthetic capacity","retryable":true}),
        json!({"provider_id":world.provider,"model":common::MODEL}),
        world.clock.now(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    world.set(world.topic, "calls.repeat_limit", json!(0)).await;
    let mut tx = world.pool.begin().await.unwrap();
    assert!(matches!(
        orch_engine::calls::plan_service(
            &mut tx,
            &service(&world, thread, root, workflow, "description"),
            world.clock.now()
        )
        .await
        .unwrap(),
        orch_engine::calls::ServicePlan::Paused
    ));
    let row = orch_engine::calls::read_root(&mut tx, thread, Purpose::Summary, root)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.state, "stopped");
    assert_eq!(row.context["detail"], "auto_disabled");
    tx.commit().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unusable_model_answer_uses_call_policy_instead_of_summary_backoff() {
    use orch_link_contract::commands::{CommandKind, CommandResult, ProviderCompleteResult};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let world = World::on_a_key("synthetic-summary-key").await;
    world.engine().stop();
    let thread = world.thread("Empty model answer").await;
    world.set(world.topic, "auto.title", json!("off")).await;
    world
        .set(world.topic, "auto.description", json!("summarizer"))
        .await;
    world
        .set(
            world.topic,
            "auto.summarizer",
            json!({"kind":"model","provider_id":world.provider,"model_name":common::MODEL}),
        )
        .await;
    sqlx::query("UPDATE nodes SET description_source='auto' WHERE id=$1")
        .bind(thread)
        .execute(world.pool.pool())
        .await
        .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    world
        .machine
        .handler
        .on(CommandKind::ProviderComplete, move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            Ok(CommandResult::ProviderComplete(ProviderCompleteResult {
                text: "   ".to_owned(),
                usage: orch_agent_contract::events::TokenUsage::from_parts(
                    orch_api_contract::threads::TokensReportedApart {
                        input: 100,
                        output: 5,
                        ..Default::default()
                    },
                ),
                request_ref: Some("synthetic-empty-summary".to_owned()),
                timing: Default::default(),
            }))
        });
    {
        let mut conn = world.pool.acquire().await.unwrap();
        orch_engine::summaries::due(&mut conn, thread, "refresh", world.clock.now())
            .await
            .unwrap();
    }
    assert!(world.engine().summarize_due(thread).await.unwrap());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let mut conn = world.pool.acquire().await.unwrap();
    let paused = orch_engine::calls::read(&mut conn, thread, Purpose::Summary)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(paused.failure["code"], "harness_error");
    assert_eq!(paused.state, "waiting");
    drop(conn);
    let due: Option<chrono::DateTime<chrono::Utc>> = world
        .scalar(
            "SELECT due_at FROM node_summary_state WHERE node_id=$1",
            thread,
        )
        .await;
    assert!(due.is_none());
    assert_eq!(world.count("SELECT count(*) FROM usage_events WHERE thread_id=$1 AND request_ref='synthetic-empty-summary'",thread).await,1,"the unusable answer still costs one actual request");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replaced_service_material_cannot_be_rearmed_by_its_late_failure() {
    let world = World::new().await;
    world.engine().stop();
    let thread = world.thread("Replaced summary material").await;
    immediate(&world, thread, "provider_busy").await;
    let (old, new, workflow) = (
        orch_types::new_id(),
        orch_types::new_id(),
        orch_types::new_id(),
    );
    let failed = orch_types::new_id();
    let failure = json!({"code":"provider_busy","message":"Synthetic capacity","retryable":true});
    let mut tx = world.pool.begin().await.unwrap();
    orch_engine::calls::reserve_service(
        &mut tx,
        &service(&world, thread, old, workflow, "title"),
        failed,
        &failure,
        json!({"provider_id":world.provider,"model":common::MODEL}),
        world.clock.now(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let mut tx = world.pool.begin().await.unwrap();
    assert!(
        matches!(orch_engine::calls::plan_service(&mut tx,&service(&world,thread,new,workflow,"title"),world.clock.now()).await.unwrap(),orch_engine::calls::ServicePlan::Ready {call,..} if call==new)
    );
    tx.commit().await.unwrap();
    let mut tx = world.pool.begin().await.unwrap();
    let late = orch_engine::calls::reserve_service(
        &mut tx,
        &service(&world, thread, old, workflow, "title"),
        orch_types::new_id(),
        &failure,
        json!({"provider_id":world.provider,"model":common::MODEL}),
        world.clock.now(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(late.state, "dismissed");
    assert!(matches!(
        orch_engine::calls::plan_service(
            &mut tx,
            &service(&world, thread, old, workflow, "title"),
            world.clock.now()
        )
        .await
        .unwrap(),
        orch_engine::calls::ServicePlan::Paused
    ));
    tx.commit().await.unwrap();
}

async fn recorded_token(world: &World, model_use: bool) -> (Id, Caller) {
    let id = orch_types::new_id();
    let mut scopes = vec!["thread.view".to_owned(), "thread.interact".to_owned()];
    if model_use {
        scopes.push("provider.use".to_owned());
    }
    let digest = orch_types::Hash::of(id.as_bytes());
    let name = format!("call-{id}");
    orch_db::queries::identity::create_api_token(
        &mut world.pool.acquire().await.unwrap(),
        &orch_db::queries::identity::NewApiToken {
            id,
            user_id: world.ada,
            name: &name,
            token_hash: digest.as_bytes(),
            scopes: &scopes,
            node_id: Some(world.topic),
            at: world.clock.now(),
            expires_at: None,
        },
    )
    .await
    .unwrap();
    (
        id,
        Caller::token(
            world.ada,
            orch_authz::TokenGrant::from_stored(&scopes, Some(world.topic))
                .unwrap()
                .for_token(id),
        ),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn automatic_retries_and_continuations_keep_all_origin_authority_and_stop_after_revocation() {
    for effects in [false, true] {
        let world = World::new().await;
        let thread = world.thread("Recorded recovery authority").await;
        immediate(&world, thread, "model_refused").await;
        let (author_token, author) = recorded_token(&world, false).await;
        let (runner_token, runner) = recorded_token(&world, true).await;
        let sent = world
            .engine()
            .submit_input(
                &author,
                thread,
                queued("Keep this work within its accepted authority"),
            )
            .await
            .unwrap();
        eventually("the narrow input waits for model approval", || async {
            world.input_state(sent.input.id).await.1 == Some("model_use_required".to_owned())
        })
        .await;
        assert!(world.turn_starts(thread).is_empty());
        world
            .engine()
            .run_input(&runner, thread, sent.input.id)
            .await
            .unwrap();
        let first = world.turn_start(thread, 0).await;
        world.say(thread, turn_began(first.turn_id)).await;
        let original = orch_db::queries::threads::input(
            &mut world.pool.acquire().await.unwrap(),
            sent.input.id,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(original.run_as, Some(world.ada));
        assert_eq!(
            original.submission_authority.as_ref().unwrap()["credentials"][0]["token_id"],
            author_token.to_string()
        );
        assert_eq!(
            original.run_as_authority.as_ref().unwrap()["credentials"][0]["token_id"],
            runner_token.to_string()
        );
        if effects {
            world
                .say(
                    thread,
                    common::message_began(first.turn_id, "partial-scoped-work"),
                )
                .await;
            world
                .say(
                    thread,
                    common::message_grew("partial-scoped-work", 0, "One change is already made"),
                )
                .await;
        }
        failed(&world, thread, first.turn_id, "model_refused").await;
        let second = world.turn_start(thread, 1).await;
        world.say(thread, turn_began(second.turn_id)).await;
        let input: Id = world
            .scalar("SELECT input_id FROM turns WHERE id=$1", second.turn_id)
            .await;
        let derived =
            orch_db::queries::threads::input(&mut world.pool.acquire().await.unwrap(), input)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(derived.author_id, original.author_id);
        assert_eq!(derived.submission_authority, original.submission_authority);
        assert_eq!(derived.run_as, original.run_as);
        assert_eq!(derived.run_as_authority, original.run_as_authority);
        assert_eq!(
            input == sent.input.id,
            !effects,
            "only work without effects reuses the input"
        );
        sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
            .bind(author_token)
            .bind(world.clock.now())
            .execute(world.pool.pool())
            .await
            .unwrap();
        failed(&world, thread, second.turn_id, "model_refused").await;
        eventually(
            "revoked original authority stops automatic handling",
            || async { recovery(&world, thread).await.state == "stopped" },
        )
        .await;
        assert_eq!(
            recovery(&world, thread).await.context["detail"],
            "origin_authority"
        );
        assert_eq!(
            world.turn_starts(thread).len(),
            2,
            "the account's broader browser rights cannot start another derived turn"
        );
        let active: bool =
            sqlx::query_scalar("SELECT revoked_at IS NULL FROM api_tokens WHERE id=$1")
                .bind(runner_token)
                .fetch_one(world.pool.pool())
                .await
                .unwrap();
        assert!(
            active,
            "the model approver remains valid; the revoked author's boundary is decisive"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn skipping_a_candidate_does_not_start_the_same_vendor_wait_again() {
    let world = World::new().await;
    world.engine().stop();
    let thread = world.thread("One vendor deadline").await;
    world.set(world.topic,"calls.rules",json!([{"id":"clock","when":{"kind":"provider_busy"},"steps":[{"type":"failover","candidate":{"provider_id":world.provider,"model":common::MODEL,"params":{}}},{"type":"retry","delay_secs":60,"times":1,"backoff":1},{"type":"wait","until":"person"}]}])).await;
    let root = orch_types::new_id();
    let workflow = orch_types::new_id();
    let call = service(&world, thread, root, workflow, "clock");
    let now = world.clock.time().get();
    let deadline = now + chrono::Duration::seconds(120);
    let mut tx = world.pool.begin().await.unwrap();
    let row=orch_engine::calls::reserve_service(&mut tx,&call,orch_types::new_id(),&json!({"code":"provider_busy","message":"Synthetic retry evidence","retryable":true,"retry_after_secs":120}),json!({"provider_id":world.provider,"model":common::MODEL}),now).await.unwrap().unwrap();
    assert_eq!(row.due_at, Some(deadline));
    tx.commit().await.unwrap();
    let mut tx = world.pool.begin().await.unwrap();
    let row = orch_engine::calls::read_root(&mut tx, thread, Purpose::Summary, root)
        .await
        .unwrap()
        .unwrap();
    orch_engine::calls::skip_step(&mut tx, &row, "candidate_not_allowed", deadline)
        .await
        .unwrap();
    let row = orch_engine::calls::read_root(&mut tx, thread, Purpose::Summary, root)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.due_at,
        Some(deadline),
        "the next step keeps the elapsed vendor deadline instead of adding another120seconds"
    );
    assert!(matches!(
        orch_engine::calls::plan_service(&mut tx, &call, deadline)
            .await
            .unwrap(),
        orch_engine::calls::ServicePlan::Ready {
            candidate: None,
            ..
        }
    ));
    tx.commit().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rules_edit_preserves_the_failed_calls_relative_vendor_deadline() {
    let world = World::new().await;
    world.engine().stop();
    let thread = world.thread("Retained vendor deadline").await;
    immediate(&world, thread, "provider_busy").await;
    let root = orch_types::new_id();
    let workflow = orch_types::new_id();
    let call = service(&world, thread, root, workflow, "clock");
    let now = world.clock.time().get();
    let deadline = now + chrono::Duration::seconds(120);
    let reference = orch_types::new_id();
    let mut tx = world.pool.begin().await.unwrap();
    orch_engine::calls::reserve_service(&mut tx,&call,reference,&json!({"code":"provider_busy","message":"Synthetic retry evidence","retryable":true,"retry_after_secs":120}),json!({"provider_id":world.provider,"model":common::MODEL}),now).await.unwrap();
    tx.commit().await.unwrap();
    world.set(world.topic,"calls.rules",json!([{"id":"edited","when":{"kind":"provider_busy"},"steps":[{"type":"retry","delay_secs":75,"times":3,"backoff":2},{"type":"wait","until":"person"}]}])).await;
    let mut tx = world.pool.begin().await.unwrap();
    assert!(matches!(
        orch_engine::calls::plan_service(&mut tx, &call, now + chrono::Duration::seconds(30))
            .await
            .unwrap(),
        orch_engine::calls::ServicePlan::Paused
    ));
    let row = orch_engine::calls::read_root(&mut tx, thread, Purpose::Summary, root)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.due_at,
        Some(deadline),
        "editing the rule cannot restart the received Retry-After interval"
    );
    assert!(matches!(
        orch_engine::calls::plan_service(&mut tx, &call, deadline - chrono::Duration::seconds(1))
            .await
            .unwrap(),
        orch_engine::calls::ServicePlan::Paused
    ));
    assert!(matches!(
        orch_engine::calls::plan_service(&mut tx, &call, deadline)
            .await
            .unwrap(),
        orch_engine::calls::ServicePlan::Ready { .. }
    ));
    let observations: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM model_call_recovery_attempts WHERE node_id=$1 AND id=$2",
    )
    .bind(thread)
    .bind(reference)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        observations, 1,
        "a policy edit does not count one failed call twice"
    );
    tx.commit().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_auxiliary_candidate_cannot_send_an_effort_the_target_model_does_not_offer() {
    use orch_link_contract::commands::{CommandError, CommandKind, ErrorCode};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let world = World::on_a_key("synthetic-effort-key").await;
    world.engine().stop();
    let thread = world.thread("Declared completion effort").await;
    sqlx::query("INSERT INTO provider_models(id,provider_id,model_name,display_name,parameters,enabled,source,created_by) SELECT $1,provider_id,'limited-effort','Limited',$4,true,'manual',created_by FROM provider_models WHERE provider_id=$2 AND model_name=$3")
        .bind(orch_types::new_id()).bind(world.provider).bind(common::MODEL).bind(json!({"efforts":["low"]})).execute(world.pool.pool()).await.unwrap();
    world.set(world.topic, "auto.title", json!("off")).await;
    world
        .set(world.topic, "auto.description", json!("summarizer"))
        .await;
    world
        .set(
            world.topic,
            "auto.summarizer",
            json!({"kind":"model","provider_id":world.provider,"model_name":common::MODEL}),
        )
        .await;
    world.set(world.topic, "calls.rules", json!([{"id":"effort","when":{"purpose":"summary"},"steps":[{"type":"failover","candidate":{"provider_id":world.provider,"model":"limited-effort","params":{"effort":"high"}}},{"type":"wait","until":"person"}]}])).await;
    sqlx::query("UPDATE nodes SET description_source='auto' WHERE id=$1")
        .bind(thread)
        .execute(world.pool.pool())
        .await
        .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    world.machine.handler.on(CommandKind::ProviderComplete, move |_| {
        counted.fetch_add(1, Ordering::SeqCst);
        let mut observed = orch_agent_contract::requests::UsageRequest {id:"completion".to_owned(), ..Default::default()};
        observed.finish("provider_busy", Some("Synthetic capacity"), Some("Model at capacity"));
        Err(CommandError::new(ErrorCode::Internal, "Synthetic capacity").with_details(json!({"failure":{"code":"provider_busy","status":503},"model_request":observed})))
    });
    orch_engine::summaries::due(
        &mut world.pool.acquire().await.unwrap(),
        thread,
        "refresh",
        world.clock.now(),
    )
    .await
    .unwrap();
    assert!(world.engine().summarize_due(thread).await.unwrap());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(world.engine().summarize_due(thread).await.unwrap());
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "unsupported effort is skipped before another model request is issued"
    );
    let row = orch_engine::calls::read(
        &mut world.pool.acquire().await.unwrap(),
        thread,
        Purpose::Summary,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(row.state, "waiting");
    assert_eq!(row.context["skipped"], "candidate_parameters");
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM usage_events WHERE thread_id=$1 AND model='limited-effort'",
                thread
            )
            .await,
        0
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_shipped_agent_policy_schedules_transient_faults_and_a_known_quota_reset() {
    for code in [
        "provider_busy",
        "provider_unreachable",
        "provider_exhausted",
    ] {
        let world = World::new().await;
        let thread = world.thread("Shipped call policy").await;
        world
            .engine()
            .submit_input(&Caller::session(world.ada), thread, queued("Keep the work"))
            .await
            .unwrap();
        let first = world.turn_start(thread, 0).await;
        world.say(thread, turn_began(first.turn_id)).await;
        let reset = world.clock.time().get() + chrono::Duration::hours(1);
        let error: TurnError = serde_json::from_value(json!({"code":code,"message":"Synthetic vendor failure","retryable":code!="provider_exhausted","resets_at":if code=="provider_exhausted" {Some(orch_types::Time::new(reset))} else {None}})).unwrap();
        world
            .say(
                thread,
                Event::TurnCompleted(TurnCompleted {
                    turn_id: first.turn_id,
                    status: TurnOutcome::Failed,
                    error: Some(error),
                }),
            )
            .await;
        world.wait_turn(first.turn_id, "failed").await;
        let row = recovery(&world, thread).await;
        assert_eq!(row.state, "scheduled");
        let due = orch_types::Time::new(row.due_at.unwrap());
        if code == "provider_exhausted" {
            assert_eq!(due, orch_types::Time::new(reset));
            assert_eq!(row.action.as_ref().unwrap()["step"]["until"], "reset");
            assert_eq!(
                world
                    .scalar::<String>("SELECT status FROM providers WHERE id=$1", world.provider)
                    .await,
                "exhausted"
            );
        } else {
            let observed: orch_types::Time =
                row.context["failed_at"].as_str().unwrap().parse().unwrap();
            assert_eq!(
                due,
                orch_types::Time::new(observed.get() + chrono::Duration::seconds(60))
            );
            assert_eq!(row.action.as_ref().unwrap()["step"]["times"], 3);
            assert_eq!(
                world
                    .scalar::<String>("SELECT status FROM providers WHERE id=$1", world.provider)
                    .await,
                "ready"
            );
        }
        world.engine().keep_call_recoveries().await.unwrap();
        assert_eq!(
            world.turn_starts(thread).len(),
            1,
            "the shipped policy cannot spend a second call before its deadline"
        );
    }
}
