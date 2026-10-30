//! A recovery uses the ordinary verified switch while carrying the failed turn's retained partial work.

use super::*;
use orch_link_contract::events::{Event, TurnCompleted, TurnError};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_configured_provider_failover_carries_the_interrupted_work_through_the_normal_switch() {
    let rig = Rig::new(
        Can {
            export_native: true,
            ..Can::default()
        },
        "other",
        Can {
            import_neutral: true,
            ..Can::default()
        },
    )
    .await;
    let thread = rig.thread_with_history("Carry interrupted work").await;
    rig.world.set(rig.world.topic,"calls.rules",json!([{"id":"fallback","when":{"kind":"model_refused"},"steps":[{"type":"failover","candidate":{"provider_id":rig.other_provider,"agent_id":rig.other_agent,"model":MODEL,"params":{}}},{"type":"wait","until":"person"}]}])).await;
    rig.say(thread, "Finish the work").await;
    let failed = rig.world.turn_start(thread, 1).await;
    rig.world
        .say(thread, common::turn_began(failed.turn_id))
        .await;
    rig.world
        .say(
            thread,
            common::message_began(failed.turn_id, "partial-work"),
        )
        .await;
    rig.world
        .say(
            thread,
            common::message_grew("partial-work", 0, "One change is already made"),
        )
        .await;
    let error:TurnError=serde_json::from_value(json!({"code":"model_refused","message":"Synthetic refusal","said":"Synthetic filter","retryable":true,"retry_after_secs":60,"incomplete":true})).unwrap();
    rig.world
        .say(
            thread,
            Event::TurnCompleted(TurnCompleted {
                turn_id: failed.turn_id,
                status: TurnOutcome::Failed,
                error: Some(error),
            }),
        )
        .await;
    rig.world.wait_turn(failed.turn_id, "failed").await;
    rig.record_native(thread).await;
    sqlx::query("UPDATE model_call_recovery SET due_at=$2 WHERE node_id=$1 AND purpose='agent'")
        .bind(thread)
        .bind(rig.world.clock.now())
        .execute(rig.world.pool.pool())
        .await
        .unwrap();
    rig.world.engine().keep_call_recoveries().await.unwrap();
    let job = Mutex::new(None);
    eventually("ordinary switch is created", || async {
        let mut conn = rig.world.pool.acquire().await.unwrap();
        let recovery = orch_engine::calls::read(
            &mut conn,
            thread,
            orch_provider_contract::calls::Purpose::Agent,
        )
        .await
        .unwrap()
        .unwrap();
        let observed = recovery.context["switch_job"]
            .as_str()
            .and_then(|value| value.parse::<Id>().ok());
        *job.lock().unwrap() = observed;
        assert_ne!(recovery.state, "stopped", "{}", recovery.context);
        observed.is_some()
    })
    .await;
    let job = rig
        .wait_job(job.into_inner().unwrap().unwrap(), "succeeded")
        .await;
    assert_eq!(job.kind, "thread_fork");
    let continued = rig.world.turn_start(thread, 2).await;
    let segment: Id = rig
        .world
        .scalar(
            "SELECT segment_id FROM turns WHERE id=$1",
            continued.turn_id,
        )
        .await;
    let actual: Id = rig
        .world
        .scalar("SELECT provider_id FROM segments WHERE id=$1", segment)
        .await;
    assert_eq!(actual, rig.other_provider);
    let handoff = rig
        .opens()
        .into_iter()
        .find(|open| open.segment_id == segment)
        .unwrap();
    let text = rig
        .blob_text(
            &handoff
                .history
                .expect("the new harness receives neutral history")
                .blob,
        )
        .await;
    assert!(
        text.contains("One change is already made"),
        "the interrupted turn's retained work reaches the new harness: {text}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stopping_recovery_during_a_switch_keeps_its_return_without_an_automatic_input() {
    let rig = Rig::new(
        Can {
            export_native: true,
            import_neutral: true,
            ..Can::default()
        },
        "other",
        Can {
            import_neutral: true,
            ..Can::default()
        },
    )
    .await;
    let thread = rig
        .thread_with_history("Stop future recovery during a switch")
        .await;
    rig.world.set(rig.world.topic,"calls.rules",json!([{"id":"fallback","when":{"kind":"model_refused"},"steps":[{"type":"failover","candidate":{"provider_id":rig.other_provider,"agent_id":rig.other_agent,"model":MODEL,"params":{}}},{"type":"wait","until":"person"}]}])).await;
    rig.say(thread, "Finish the work").await;
    let failed = rig.world.turn_start(thread, 1).await;
    rig.world
        .say(thread, common::turn_began(failed.turn_id))
        .await;
    rig.world
        .say(
            thread,
            common::message_began(failed.turn_id, "kept-before-stop"),
        )
        .await;
    rig.world
        .say(
            thread,
            common::message_grew(
                "kept-before-stop",
                0,
                "The change made before stopping is retained",
            ),
        )
        .await;
    let error: TurnError = serde_json::from_value(json!({"code":"model_refused","message":"Synthetic refusal","retryable":true,"retry_after_secs":60,"incomplete":true})).unwrap();
    rig.world
        .say(
            thread,
            Event::TurnCompleted(TurnCompleted {
                turn_id: failed.turn_id,
                status: TurnOutcome::Failed,
                error: Some(error),
            }),
        )
        .await;
    rig.world.wait_turn(failed.turn_id, "failed").await;
    rig.record_native(thread).await;
    // Hold the actual destination opening before the accepted switch can commit. Its history may
    // travel by neutral import, which needs no native export at all.
    let opened = rig.world.began(CommandKind::SessionOpenFenced);
    rig.world
        .machine
        .handler
        .hold(CommandKind::SessionOpenFenced);
    sqlx::query("UPDATE model_call_recovery SET due_at=$2 WHERE node_id=$1 AND purpose='agent'")
        .bind(thread)
        .bind(rig.world.clock.now())
        .execute(rig.world.pool.pool())
        .await
        .unwrap();
    rig.world.engine().keep_call_recoveries().await.unwrap();
    let job = Mutex::new(None);
    eventually("ordinary recovery switch is created", || async {
        let mut conn = rig.world.pool.acquire().await.unwrap();
        let row = orch_engine::calls::read(
            &mut conn,
            thread,
            orch_provider_contract::calls::Purpose::Agent,
        )
        .await
        .unwrap()
        .unwrap();
        let observed = row.context["switch_job"]
            .as_str()
            .and_then(|value| value.parse::<Id>().ok());
        *job.lock().unwrap() = observed;
        observed.is_some()
    })
    .await;
    let job = job.into_inner().unwrap().unwrap();
    rig.world
        .wait_began(CommandKind::SessionOpenFenced, opened + 1)
        .await;
    let mut tx = rig.world.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_action(&mut tx)
        .await
        .unwrap();
    let principal = orch_authz::Principal::load(
        &mut tx,
        rig.caller(),
        orch_types::Time::new(rig.world.clock.now()),
    )
    .await
    .unwrap();
    rig.world
        .engine()
        .control_call_recovery(
            tx,
            principal,
            thread,
            orch_api_contract::calls::CallRecoveryAction::Stop,
        )
        .await
        .unwrap();
    {
        let mut conn = rig.world.pool.acquire().await.unwrap();
        let row = orch_engine::calls::read(
            &mut conn,
            thread,
            orch_provider_contract::calls::Purpose::Agent,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            row.state, "switching",
            "the accepted job must retain its completion watcher"
        );
        assert_eq!(row.context["manual_stop"], true);
        assert_eq!(row.context["switch_job"], json!(job));
    }
    rig.world.machine.handler.release();
    rig.wait_job(job, "succeeded").await;
    eventually(
        "the stopped switch retains its completed return choice",
        || async {
            let mut conn = rig.world.pool.acquire().await.unwrap();
            let row = orch_engine::calls::read(
                &mut conn,
                thread,
                orch_provider_contract::calls::Purpose::Agent,
            )
            .await
            .unwrap()
            .unwrap();
            row.state == "completed" && row.context["candidate_active"] == true
        },
    )
    .await;
    meanwhile(Duration::from_millis(400)).await;
    assert_eq!(
        rig.world.turn_starts(thread).len(),
        2,
        "Stop must not rerun the interrupted input"
    );
    assert_eq!(
        rig.world
            .count(
                "SELECT count(*) FROM thread_inputs WHERE thread_id=$1 AND state='pending'",
                thread
            )
            .await,
        0
    );
    assert_eq!(rig.world.count("SELECT count(*) FROM thread_inputs WHERE thread_id=$1 AND client_id LIKE 'recovery:%'", thread).await, 0);
    assert_eq!(
        rig.world
            .scalar::<Id>("SELECT provider_id FROM threads WHERE node_id=$1", thread)
            .await,
        rig.other_provider
    );
    rig.say(thread, "The next independent turn").await;
    let next = rig.world.turn_start(thread, 2).await;
    let segment: Id = rig
        .world
        .scalar("SELECT segment_id FROM turns WHERE id=$1", next.turn_id)
        .await;
    assert_eq!(
        rig.world
            .scalar::<Id>("SELECT provider_id FROM segments WHERE id=$1", segment)
            .await,
        rig.world.provider
    );
    assert_eq!(next.model, MODEL);
    let returned = rig
        .opens()
        .into_iter()
        .find(|open| open.segment_id == segment)
        .unwrap();
    let history = rig
        .blob_text(
            &returned
                .history
                .expect("the original harness receives the retained neutral history")
                .blob,
        )
        .await;
    assert!(
        history.contains("The change made before stopping is retained"),
        "returning after Stop must not discard the interrupted answer: {history}"
    );
}
