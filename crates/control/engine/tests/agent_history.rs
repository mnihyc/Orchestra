//! Idle history retention, admission races, stored observations and reconstruction of missing native sessions.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use common::{World, queued, turn_ended};
use orch_agent_contract::history::NativeHistoryFact;
use orch_authz::Caller;
use orch_blob_contract::store::BlobStore;
use orch_blob_local::LocalBlobStore;
use orch_db::queries::agent_history as rows;
use orch_link_contract::commands::{
    AgentHistoryManageResult, Command, CommandError, CommandKind as K, CommandResult, ErrorCode,
    FsMaterializeResult, SessionMode,
};
use orch_link_contract::events::TurnOutcome;
use orch_types::{Clock, Id, new_id};
use serde_json::json;

fn serve_history(world: &World) -> Arc<Mutex<HashSet<String>>> {
    let removed = Arc::new(Mutex::new(HashSet::<String>::new()));
    let kept = removed.clone();
    world
        .machine
        .handler
        .on(K::AgentHistoryManage, move |command| {
            let Command::AgentHistoryManage(body) = command else {
                panic!("the command kind is exact");
            };
            let mut removed = kept.lock().unwrap();
            let facts = body
                .sources
                .iter()
                .map(|source| {
                    if !source.started_by_orch {
                        return NativeHistoryFact {
                            source: source.source.clone(),
                            bytes: None,
                            removed: false,
                            error: Some("history.not_owned".into()),
                        };
                    }
                    if body.remove {
                        removed.insert(source.source.native_session_ref.clone());
                    }
                    NativeHistoryFact {
                        source: source.source.clone(),
                        bytes: Some(4096),
                        removed: removed.contains(&source.source.native_session_ref),
                        error: None,
                    }
                })
                .collect();
            Ok(CommandResult::AgentHistoryManage(
                AgentHistoryManageResult { facts },
            ))
        });
    removed
}

async fn completed(world: &World) -> (Id, Id) {
    let thread = world.thread("Retained conversation").await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Remember the copper key"),
        )
        .await
        .unwrap();
    world
        .run_turn(thread, 0, "The copper key is kept beside the ledger")
        .await;
    world.wait_state(thread, "idle").await;
    world.wait_followed_up(thread).await;
    (thread, world.segment_of(thread).await)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn batched_native_observations_preserve_aliases_stale_identity_and_failed_measurement_facts()
{
    let world = World::new().await;
    serve_history(&world);
    let (_, segment) = completed(&world).await;
    world.engine().inventory_agent_history().await.unwrap();
    let alias = new_id();
    sqlx::query("INSERT INTO segments(id,thread_id,continuation_id,ordinal,agent_id,provider_id,environment_id,machine_id,
        env_generation,arrangement,executor,tool_set,credential_path,native_session_ref,started_by,started_at,seq_updated)
        SELECT $2,s.thread_id,s.continuation_id,s.ordinal+1,s.agent_id,s.provider_id,s.environment_id,s.machine_id,
        s.env_generation,s.arrangement,s.executor,s.tool_set,s.credential_path,s.native_session_ref,s.started_by,s.started_at,s.seq_updated
        FROM segments s WHERE s.id=$1")
        .bind(segment).bind(alias).execute(world.pool.pool()).await.unwrap();
    let mut conn = world.pool.acquire().await.unwrap();
    let original = rows::get(&mut conn, segment).await.unwrap().unwrap();
    rows::measured_batch(
        &mut conn,
        &[rows::Observation {
            segment,
            native: original.native_session_ref.clone(),
            bytes: Some(8192),
            removed: false,
            error: None,
            revision: Some(5),
        }],
        world.clock.now(),
    )
    .await
    .unwrap();
    rows::measured_batch(
        &mut conn,
        &[
            rows::Observation {
                segment,
                native: "a-stale-different-native-identity".into(),
                bytes: Some(1),
                removed: true,
                error: None,
                revision: Some(99),
            },
            rows::Observation {
                segment: alias,
                native: original.native_session_ref.clone(),
                bytes: None,
                removed: false,
                error: Some("history.measurement_unavailable".into()),
                revision: None,
            },
        ],
        world.clock.now(),
    )
    .await
    .unwrap();
    let observed: Vec<(Id, Option<i64>, Option<chrono::DateTime<chrono::Utc>>, i64, Option<String>)> =
        sqlx::query_as("SELECT id,agent_history_bytes,agent_history_removed_at,agent_history_measured_revision,agent_history_error
            FROM segments WHERE id=ANY($1)")
            .bind([segment, alias]).fetch_all(&mut *conn).await.unwrap();
    assert_eq!(observed.len(), 2);
    for (_, bytes, removed, revision, error) in observed {
        assert_eq!(bytes, Some(8192));
        assert!(removed.is_none());
        assert_eq!(revision, 5);
        assert_eq!(error.as_deref(), Some("history.measurement_unavailable"));
    }
    drop(conn);
    world.shutdown().await;
}

fn serve_native_generations(world: &World, missing: bool) {
    let missing = Arc::new(AtomicBool::new(missing));
    world.machine.handler.on(K::SessionOpen, move |command| {
        let Command::SessionOpen(body) = command else {
            panic!("the command kind is exact");
        };
        let resume = matches!(body.mode, SessionMode::Resume | SessionMode::Adopt);
        if resume && missing.swap(false, Ordering::SeqCst) {
            return Err(CommandError::new(
                ErrorCode::SessionNotFound,
                "The native history is missing",
            ));
        }
        Ok(CommandResult::SessionOpen(
            orch_link_contract::commands::SessionOpenResult {
                native_session_ref: if resume {
                    body.native_session_ref.clone().unwrap()
                } else {
                    format!("history-native-{}", body.segment_id)
                },
                resumed: resume,
                import_report: None,
                instruction_sources: Vec::new(),
                approval_reviewer: None,
            },
        ))
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_native_only_no_tools_side_keeps_its_needed_cache_after_seven_idle_days() {
    let clock = Arc::new(orch_testkit::clock::manual());
    let world = World::with_clock(clock.clone()).await;
    let removed = serve_history(&world);
    let parent = world.thread("Parent context").await;
    let (side, segment) = completed(&world).await;
    sqlx::query("UPDATE threads SET side_of=$2,side_reading='no_tools' WHERE node_id=$1")
        .bind(side)
        .bind(parent)
        .execute(world.pool.pool())
        .await
        .unwrap();
    clock.advance(chrono::Duration::days(7));
    world.engine().retain_agent_history().await.unwrap();
    assert!(removed.lock().unwrap().is_empty());
    let mut conn = world.pool.acquire().await.unwrap();
    let kept = rows::get(&mut conn, segment).await.unwrap().unwrap();
    assert_eq!(
        rows::refusal(&mut conn, &kept).await.unwrap(),
        Some("history.in_use")
    );
    assert!(kept.removed_at.is_none() && kept.claim.is_none());
    drop(conn);
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unsaid_handoff_follows_a_resumed_alias_and_only_delivered_notices_are_cleared() {
    let world = World::new().await;
    serve_history(&world);
    serve_native_generations(&world, false);
    let (thread, segment) = completed(&world).await;
    let alias = new_id();
    let mut tx = world.pool.begin().await.unwrap();
    let seq: i64 = sqlx::query_scalar(
        "UPDATE threads SET next_seq=next_seq+1 WHERE node_id=$1 RETURNING next_seq-1",
    )
    .bind(thread)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert!(
        orch_db::queries::threads::end_segment(&mut tx, segment, "switch", world.clock.now(), seq)
            .await
            .unwrap()
    );
    sqlx::query("INSERT INTO segments(id,thread_id,continuation_id,ordinal,agent_id,provider_id,environment_id,machine_id,
            env_generation,arrangement,executor,tool_set,credential_path,native_session_ref,started_by,started_at,seq_updated)
        SELECT $2,s.thread_id,s.continuation_id,s.ordinal+1,s.agent_id,s.provider_id,s.environment_id,s.machine_id,
            s.env_generation,s.arrangement,s.executor,s.tool_set,s.credential_path,s.native_session_ref,s.started_by,s.started_at,s.seq_updated
        FROM segments s WHERE s.id=$1")
        .bind(segment).bind(alias).execute(&mut *tx).await.unwrap();
    let own = format!("agent-history:{segment}");
    let unrelated = format!("agent-history:{}", new_id());
    sqlx::query("UPDATE threads SET current_segment_id=$2,pending_notices=$3 WHERE node_id=$1")
        .bind(thread).bind(alias).bind(json!([
            {"id":own,"kind":"handoff","text":"The prepared conversation reaches its actual native alias"},
            {"id":unrelated,"kind":"handoff","text":"Another native recipient's context must not be substituted"},
            {"id":"ordinary-notice","kind":"announcement","title":"A normal announcement","body":"Ordinary notices still arrive",
                "from":"A source","from_thread":false,"at":"2026-10-27T12:00:00Z"}
        ])).execute(&mut *tx).await.unwrap();
    sqlx::query("UPDATE thread_continuations SET current_segment_id=$2 WHERE thread_id=$1")
        .bind(thread)
        .bind(alias)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let (next, _) = world
        .running_turn(thread, "Use this native conversation")
        .await;
    assert_eq!(
        next.text
            .matches("The prepared conversation reaches its actual native alias")
            .count(),
        1,
        "{}",
        next.text
    );
    assert!(
        next.text.contains("Ordinary notices still arrive"),
        "{}",
        next.text
    );
    assert!(
        !next.text.contains("Another native recipient's context"),
        "{}",
        next.text
    );
    common::eventually(
        "only actual delivery recipients to be acknowledged",
        || async {
            let pending: orch_types::Json =
                sqlx::query_scalar("SELECT pending_notices FROM threads WHERE node_id=$1")
                    .bind(thread)
                    .fetch_one(world.pool.pool())
                    .await
                    .unwrap();
            pending.as_array().unwrap().len() == 1 && pending[0]["id"] == unrelated
        },
    )
    .await;
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_home_larger_than_one_metadata_page_uses_one_shared_native_inventory_pass() {
    let world = World::new().await;
    serve_history(&world);
    let (thread, segment) = completed(&world).await;
    // Cross the existing metadata page once without opening hundreds of model sessions.
    let ids: Vec<Id> = (0..200).map(|_| new_id()).collect();
    sqlx::query("INSERT INTO segments(id,thread_id,continuation_id,ordinal,agent_id,provider_id,environment_id,machine_id,
            env_generation,arrangement,executor,tool_set,credential_path,native_session_ref,started_by,started_at,seq_updated)
        SELECT extra.id,s.thread_id,s.continuation_id,(extra.ordinal+1)::integer,s.agent_id,s.provider_id,s.environment_id,s.machine_id,
            s.env_generation,s.arrangement,s.executor,s.tool_set,s.credential_path,'native-'||extra.id::text,s.started_by,s.started_at,s.seq_updated
        FROM segments s CROSS JOIN unnest($2::uuid[]) WITH ORDINALITY AS extra(id,ordinal) WHERE s.id=$1")
        .bind(segment).bind(&ids).execute(world.pool.pool()).await.unwrap();
    let before: HashSet<_> = world
        .commands
        .issued_commands("agent.history.manage")
        .into_iter()
        .map(|(_, command)| command.id)
        .collect();
    world.engine().inventory_agent_history().await.unwrap();
    let commands = world.commands.issued_commands("agent.history.manage");
    let batches: Vec<orch_link_contract::commands::AgentHistoryManage> = commands
        .into_iter()
        .filter(|(_, command)| !before.contains(&command.id))
        .map(|(_, command)| serde_json::from_value(command.body).unwrap())
        .collect();
    let measured: Vec<_> = batches
        .iter()
        .filter(|batch| !batch.sources.is_empty())
        .collect();
    assert_eq!(measured.len(), 2);
    let passes: HashSet<_> = measured
        .iter()
        .flat_map(|batch| batch.sources.iter().map(|source| source.inventory_pass))
        .collect();
    assert_eq!(passes.len(), 1);
    assert!(!passes.contains(&None));
    assert_eq!(
        measured
            .iter()
            .map(|batch| batch.sources.len())
            .sum::<usize>(),
        201
    );
    assert_eq!(
        batches
            .iter()
            .filter(|batch| batch.sources.is_empty())
            .count(),
        1,
        "the completed pass releases its metadata snapshot"
    );
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM segments WHERE thread_id=$1 AND agent_history_bytes=4096",
                thread
            )
            .await,
        201
    );
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_successful_close_queues_a_stored_measurement_even_when_the_clock_does_not_move() {
    let world = World::with_clock(Arc::new(orch_testkit::clock::manual())).await;
    serve_history(&world);
    let (thread, segment) = completed(&world).await;
    world.engine().inventory_agent_history().await.unwrap();
    for revision in [1_i64, 2, 3] {
        let command = new_id();
        sqlx::query(
            "INSERT INTO commands(id,machine_id,kind,body,state,deadline_at,created_at,updated_at)
            VALUES($1,$2,'session.close.fenced',$3,'queued',$4,$4,$4)",
        )
        .bind(command)
        .bind(world.machine_id)
        .bind(json!({"thread_id":thread,"segment_id":segment}))
        .bind(world.clock.now())
        .execute(world.pool.pool())
        .await
        .unwrap();
        sqlx::query("UPDATE commands SET state='succeeded',result='{}' WHERE id=$1")
            .bind(command)
            .execute(world.pool.pool())
            .await
            .unwrap();
        let mut conn = world.pool.acquire().await.unwrap();
        let dirty = rows::closed_page(&mut conn, None, 10).await.unwrap();
        assert_eq!(dirty.len(), 1);
        assert_eq!(dirty[0].measure_revision, revision);
        drop(conn);
        if revision == 3 {
            world.refuse(
                K::AgentHistoryManage,
                vec![CommandError::new(
                    ErrorCode::Unsupported,
                    "This home does not support native history inventory",
                )],
            );
        }
        world
            .engine()
            .inventory_closed_agent_history()
            .await
            .unwrap();
        let mut conn = world.pool.acquire().await.unwrap();
        assert!(
            rows::closed_page(&mut conn, None, 10)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            rows::get(&mut conn, segment).await.unwrap().unwrap().bytes,
            Some(4096)
        );
        if revision == 3 {
            assert_eq!(
                rows::get(&mut conn, segment)
                    .await
                    .unwrap()
                    .unwrap()
                    .error
                    .as_deref(),
                Some("unsupported")
            );
        }
    }
    let observed = world.ran(K::AgentHistoryManage);
    world
        .engine()
        .inventory_closed_agent_history()
        .await
        .unwrap();
    assert_eq!(
        world.ran(K::AgentHistoryManage),
        observed,
        "an unsupported home waits for the next full inventory instead of retrying every close sweep"
    );
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_idle_setting_removes_at_seven_days_and_zero_keeps_the_native_history() {
    let clock = Arc::new(orch_testkit::clock::manual());
    let world = World::with_clock(clock.clone()).await;
    let removed = serve_history(&world);
    let (thread, segment) = completed(&world).await;
    world
        .set(thread, "retention.agent_history_idle_days", json!(0))
        .await;
    clock.advance(chrono::Duration::days(7));
    world.engine().retain_agent_history().await.unwrap();
    assert!(removed.lock().unwrap().is_empty());
    assert!(
        rows::get(&mut *world.pool.acquire().await.unwrap(), segment)
            .await
            .unwrap()
            .unwrap()
            .removed_at
            .is_none()
    );
    world
        .set(thread, "retention.agent_history_idle_days", json!(7))
        .await;
    world.engine().retain_agent_history().await.unwrap();
    assert_eq!(removed.lock().unwrap().len(), 1);
    let history = rows::get(&mut *world.pool.acquire().await.unwrap(), segment)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(history.removed_at, Some(clock.now()));
    assert_eq!(history.bytes, Some(4096));
    assert!(history.claim.is_none());
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn archived_and_recently_deleted_threads_are_included_in_idle_history_removal() {
    let clock = Arc::new(orch_testkit::clock::manual());
    let world = World::with_clock(clock.clone()).await;
    let removed = serve_history(&world);
    let (archived, archived_segment) = completed(&world).await;
    let (deleted, deleted_segment) = completed(&world).await;
    let mut conn = world.pool.acquire().await.unwrap();
    assert!(
        orch_db::queries::hierarchy::archive(&mut conn, archived, world.ada, clock.now())
            .await
            .unwrap()
    );
    assert_eq!(
        orch_db::queries::hierarchy::delete_subtree(&mut conn, deleted, world.ada, clock.now())
            .await
            .unwrap(),
        1
    );
    drop(conn);
    clock.advance(chrono::Duration::days(7));
    world.engine().retain_agent_history().await.unwrap();
    assert_eq!(removed.lock().unwrap().len(), 2);
    let mut conn = world.pool.acquire().await.unwrap();
    for segment in [archived_segment, deleted_segment] {
        let history = rows::get(&mut conn, segment).await.unwrap().unwrap();
        assert_eq!(history.removed_at, Some(clock.now()));
        assert!(history.claim.is_none());
    }
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT archived_at IS NOT NULL FROM nodes WHERE id=$1")
            .bind(archived)
            .fetch_one(&mut *conn)
            .await
            .unwrap()
    );
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT deleted_at IS NOT NULL FROM nodes WHERE id=$1")
            .bind(deleted)
            .fetch_one(&mut *conn)
            .await
            .unwrap()
    );
    drop(conn);
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_observation_cannot_remove_history_after_a_new_turn_won_the_thread_lock() {
    let world = World::new().await;
    serve_history(&world);
    let (thread, segment) = completed(&world).await;
    world.engine().inventory_agent_history().await.unwrap();
    let observed = rows::get(&mut *world.pool.acquire().await.unwrap(), segment)
        .await
        .unwrap()
        .unwrap();
    let (_turn, _) = world.running_turn(thread, "A new turn wins").await;
    let mut tx = world.pool.begin().await.unwrap();
    assert!(
        !rows::claim(&mut tx, &observed, new_id(), None, world.clock.now())
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    assert!(
        !rows::removing(&mut *world.pool.acquire().await.unwrap(), thread)
            .await
            .unwrap()
    );
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unavailable_home_keeps_its_last_size_and_reports_why_it_cannot_be_removed() {
    let world = World::new().await;
    serve_history(&world);
    let (_thread, segment) = completed(&world).await;
    world.engine().inventory_agent_history().await.unwrap();
    sqlx::query("UPDATE segments SET machine_id=NULL WHERE id=$1")
        .bind(segment)
        .execute(world.pool.pool())
        .await
        .unwrap();
    world.engine().inventory_agent_history().await.unwrap();
    let mut conn = world.pool.acquire().await.unwrap();
    let history = rows::get(&mut conn, segment).await.unwrap().unwrap();
    assert_eq!(history.bytes, Some(4096));
    assert_eq!(
        history.error.as_deref(),
        Some("history.machine_unavailable")
    );
    assert_eq!(
        rows::refusal(&mut conn, &history).await.unwrap(),
        Some("history.machine_unavailable")
    );
    assert!(history.claim.is_none() && history.removed_at.is_none());
    drop(conn);
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn measuring_an_unavailable_harness_never_starts_it_or_runs_its_attach_commands() {
    let world = World::new().await;
    serve_history(&world);
    let (_thread, segment) = completed(&world).await;
    world.engine().inventory_agent_history().await.unwrap();
    let starts = world.ran(K::AgentStart);
    let hooks = world.ran(K::UnitAfterUp);
    sqlx::query("UPDATE harnesses SET status='offline' WHERE provider_id=$1")
        .bind(world.provider)
        .execute(world.pool.pool())
        .await
        .unwrap();
    world.engine().inventory_agent_history().await.unwrap();
    assert_eq!(world.ran(K::AgentStart), starts);
    assert_eq!(world.ran(K::UnitAfterUp), hooks);
    let history = rows::get(&mut *world.pool.acquire().await.unwrap(), segment)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(history.bytes, Some(4096));
    assert_eq!(
        history.error.as_deref(),
        Some("history.measurement_unavailable")
    );
    assert!(history.removed_at.is_none());
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn another_conversation_naming_the_same_native_history_protects_it() {
    let world = World::new().await;
    serve_history(&world);
    let (thread, segment) = completed(&world).await;
    let (_other, alias) = completed(&world).await;
    let mut conn = world.pool.acquire().await.unwrap();
    let native = rows::get(&mut conn, segment)
        .await
        .unwrap()
        .unwrap()
        .native_session_ref;
    sqlx::query("UPDATE segments SET native_session_ref=$2 WHERE id=$1")
        .bind(alias)
        .bind(&native)
        .execute(&mut *conn)
        .await
        .unwrap();
    rows::measured(
        &mut conn,
        segment,
        &native,
        Some(4096),
        false,
        None,
        world.clock.now(),
    )
    .await
    .unwrap();
    let history = rows::get(&mut conn, segment).await.unwrap().unwrap();
    assert_eq!(
        rows::refusal(&mut conn, &history).await.unwrap(),
        Some("history.in_use")
    );
    assert!(
        !rows::claim(&mut conn, &history, new_id(), None, world.clock.now())
            .await
            .unwrap()
    );
    assert!(!rows::removing(&mut conn, thread).await.unwrap());
    drop(conn);
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn waiting_and_held_inputs_and_pending_plans_and_approvals_refuse_removal() {
    for plan in [false, true] {
        let world = World::new().await;
        serve_history(&world);
        let (thread, segment) = completed(&world).await;
        world.engine().inventory_agent_history().await.unwrap();
        let mut conn = world.pool.acquire().await.unwrap();
        let history = rows::get(&mut conn, segment).await.unwrap().unwrap();
        sqlx::query("INSERT INTO approvals(id,kind,thread_id,node_id,title,request,required_permission) VALUES($1,'harness',$2,$2,'Waiting',$3,'')")
            .bind(new_id()).bind(thread).bind(if plan {json!({"request_kind":"plan"})} else {json!({"request_kind":"command"})}).execute(&mut *conn).await.unwrap();
        assert_eq!(
            rows::refusal(&mut conn, &history).await.unwrap(),
            Some("history.approval_waiting")
        );
        drop(conn);
        world.shutdown().await;
    }
    let world = World::new().await;
    serve_history(&world);
    let (thread, segment) = completed(&world).await;
    let (active, _) = world.running_turn(thread, "A turn before Stop").await;
    let input = world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Keep this held input"),
        )
        .await
        .unwrap();
    world
        .engine()
        .interrupt(&Caller::session(world.ada), thread, active.turn_id)
        .await
        .unwrap();
    world
        .say(thread, turn_ended(active.turn_id, TurnOutcome::Interrupted))
        .await;
    world.wait_state(thread, "idle").await;
    world.wait_followed_up(thread).await;
    assert!(world.sync(thread).await.thread.queue.held);
    world.engine().inventory_agent_history().await.unwrap();
    assert_eq!(world.input_state(input.input.id).await.0, "pending");
    let mut conn = world.pool.acquire().await.unwrap();
    let history = rows::get(&mut conn, segment).await.unwrap().unwrap();
    assert_eq!(
        rows::refusal(&mut conn, &history).await.unwrap(),
        Some("history.inputs_waiting")
    );
    drop(conn);
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retained_native_dependencies_and_unowned_history_refuse_removal() {
    let world = World::new().await;
    let (thread, segment) = completed(&world).await;
    let mut conn = world.pool.acquire().await.unwrap();
    let unowned = rows::get(&mut conn, segment).await.unwrap().unwrap();
    assert_eq!(
        rows::refusal(&mut conn, &unowned).await.unwrap(),
        Some("history.not_owned")
    );
    rows::measured(
        &mut conn,
        segment,
        &unowned.native_session_ref,
        Some(1024),
        false,
        None,
        world.clock.now(),
    )
    .await
    .unwrap();
    let owned = rows::get(&mut conn, segment).await.unwrap().unwrap();
    sqlx::query("UPDATE thread_continuations SET native_source=$2 WHERE thread_id=$1")
        .bind(thread).bind(json!({"thread_id":thread,"segment_id":segment,"native_session_ref":owned.native_session_ref})).execute(&mut *conn).await.unwrap();
    assert_eq!(
        rows::refusal(&mut conn, &owned).await.unwrap(),
        Some("history.in_use")
    );
    assert!(
        !rows::claim(&mut conn, &owned, new_id(), None, world.clock.now())
            .await
            .unwrap()
    );
    drop(conn);
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retained_native_creation_and_open_receipts_protect_their_exact_child_history() {
    use orch_agent_contract::history::{NativeInputPrefix, NativePrefixOpened, NativePrefixSource};
    let world = World::new().await;
    serve_history(&world);
    let (thread, segment) = completed(&world).await;
    world.engine().inventory_agent_history().await.unwrap();
    let mut conn = world.pool.acquire().await.unwrap();
    let history = rows::get(&mut conn, segment).await.unwrap().unwrap();
    let opened = NativePrefixOpened {
        session: orch_agent_contract::session::SessionOpened {
            native_session_ref: history.native_session_ref.clone(),
            resumed: false,
            import_report: None,
            instruction_sources: Vec::new(),
            approval_reviewer: None,
        },
        prefix: NativeInputPrefix {
            harness_kind: "codex".into(),
            // The parent differs, so only the retained child receipt can protect this cache.
            source: NativePrefixSource {
                thread_id: thread,
                segment_id: new_id(),
                native_session_ref: "different-parent".into(),
            },
            before_input_ref: "retained-input".into(),
            fingerprint: orch_types::Hash::of(b"retained prefix"),
            empty: false,
        },
    };
    let creation = json!({"operation_id":new_id(),"command_id":new_id(),"machine_id":world.machine_id,"segment_id":segment});
    let created = serde_json::to_value(&opened).unwrap();
    let prefix = serde_json::to_value(&opened.prefix).unwrap();
    let resumed = orch_link_contract::commands::SessionOpenResult {
        native_session_ref: history.native_session_ref.clone(),
        resumed: true,
        import_report: None,
        instruction_sources: Vec::new(),
        approval_reviewer: None,
    };
    for (creation, created, prepared) in [
        (Some(creation), Some(created.clone()), None),
        (None, None, Some(json!({"kind":"prefix","value":created}))),
        (None, None, Some(json!({"kind":"resumed","value":resumed}))),
    ] {
        sqlx::query("UPDATE thread_continuations SET native_source=NULL,native_prefix=$2,native_creation=$3,
            native_creation_result=$4,open_result=$5 WHERE thread_id=$1")
            .bind(thread).bind(&prefix).bind(creation).bind(created).bind(prepared)
            .execute(&mut *conn).await.unwrap();
        assert_eq!(
            rows::refusal(&mut conn, &history).await.unwrap(),
            Some("history.in_use")
        );
        assert!(
            !rows::claim(&mut conn, &history, new_id(), None, world.clock.now())
                .await
                .unwrap()
        );
    }
    drop(conn);
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn adopted_native_history_is_excluded_even_in_a_managed_provider_home() {
    let world = World::new().await;
    serve_history(&world);
    let (_thread, segment) = completed(&world).await;
    let mut conn = world.pool.acquire().await.unwrap();
    orch_db::queries::adoption::mark_adopted(&mut conn, segment)
        .await
        .unwrap();
    orch_db::queries::adoption::mark_opened(&mut conn, segment)
        .await
        .unwrap();
    assert!(
        !rows::get(&mut conn, segment)
            .await
            .unwrap()
            .unwrap()
            .started_by_orch
    );
    drop(conn);
    world.engine().inventory_agent_history().await.unwrap();
    let mut conn = world.pool.acquire().await.unwrap();
    let history = rows::get(&mut conn, segment).await.unwrap().unwrap();
    assert_eq!(history.error.as_deref(), Some("history.not_owned"));
    assert!(!history.owned && history.removed_at.is_none());
    assert_eq!(
        rows::refusal(&mut conn, &history).await.unwrap(),
        Some("history.not_owned")
    );
    drop(conn);
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_outstanding_native_command_holds_admission_through_a_server_restart() {
    let world = World::new().await;
    serve_history(&world);
    let (thread, segment) = completed(&world).await;
    world.engine().inventory_agent_history().await.unwrap();
    let mut tx = world.pool.begin().await.unwrap();
    let observed = rows::get(&mut tx, segment).await.unwrap().unwrap();
    assert!(
        rows::claim(&mut tx, &observed, new_id(), None, world.clock.now())
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    world.machine.handler.hold(K::AgentHistoryManage);
    world.restart_api().await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Wait for the native outcome"),
        )
        .await
        .unwrap();
    assert_eq!(
        world.engine().preflight(thread).await.unwrap(),
        orch_engine::preflight::Outcome::Blocked(orch_engine::preflight::Blocked {
            reason: "error",
            detail: Some(
                "Its agent history is being removed. The next session carries the conversation in."
                    .into()
            ),
        })
    );
    assert_eq!(world.turn_starts(thread).len(), 1);
    world.shutdown().await;
}

async fn recovery(import_neutral: bool, missing: bool, choice: &str) {
    let world = World::new().await;
    serve_history(&world);
    serve_native_generations(&world, missing);
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(LocalBlobStore::open(directory.path()).await.unwrap());
    world.engine().attach_blobs(store.clone());
    let mut descriptor = common::descriptor();
    descriptor["capabilities"]["import_neutral"] = json!(import_neutral);
    world.declares(descriptor).await;
    sqlx::query("UPDATE provider_models SET parameters=jsonb_set(parameters,ARRAY[$2::text],'200000'::jsonb) WHERE provider_id=$1")
        .bind(world.provider).bind(orch_agent_contract::parameters::MODEL_WINDOW_KEY).execute(world.pool.pool()).await.unwrap();
    let (thread, segment) = completed(&world).await;
    let obsolete = format!("agent-history:{segment}");
    sqlx::query("UPDATE threads SET pending_notices=$2 WHERE node_id=$1")
        .bind(thread).bind(json!([{"id":obsolete,"kind":"handoff","text":"Obsolete recovery context must stay with its old native recipient"}]))
        .execute(world.pool.pool()).await.unwrap();
    world
        .set(world.topic, "thread.switch_history", json!(choice))
        .await;
    if !missing {
        let mut conn = world.pool.acquire().await.unwrap();
        let native = rows::get(&mut conn, segment)
            .await
            .unwrap()
            .unwrap()
            .native_session_ref;
        rows::measured(
            &mut conn,
            segment,
            &native,
            Some(4096),
            true,
            None,
            world.clock.now(),
        )
        .await
        .unwrap();
    }
    world.restart_api().await;
    world.engine().attach_blobs(store.clone());
    let file = choice != "none" && !(import_neutral && choice == "carry");
    if file {
        world.machine.handler.hold(K::TurnStart);
        world.machine.handler.hold(K::TurnStartFenced);
    }
    world.machine.handler.on(K::FsMaterialize, |_| {
        Ok(CommandResult::FsMaterialize(FsMaterializeResult {
            files: 1,
            bytes: 1,
            skipped: 0,
        }))
    });
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Continue using what you remember"),
        )
        .await
        .unwrap();
    let next = world.turn_start(thread, 1).await;
    assert!(
        !next.text.contains("Obsolete recovery context"),
        "{}",
        next.text
    );
    if file {
        assert_eq!(world.count("SELECT count(*) FROM items WHERE thread_id=$1 AND payload->>'marker'='agent_history'",thread).await, 0,
            "an opened native session has not yet received its file handoff");
        world.machine.handler.release();
    }
    let opens = world.commands.issued_bodies("session.open");
    let opened = opens
        .iter()
        .filter_map(|(_, body)| {
            serde_json::from_value::<orch_link_contract::commands::SessionOpen>(body.clone()).ok()
        })
        .find(|open| open.segment_id != segment)
        .unwrap();
    if choice == "none" {
        assert_eq!(opened.mode, SessionMode::New);
        assert!(opened.history.is_none());
        assert!(
            !next.text.contains("Remember the copper key"),
            "{}",
            next.text
        );
        assert!(!next.text.contains(".orch/handoff/"), "{}", next.text);
        assert!(world.commands.issued_bodies("fs.materialize").is_empty());
        assert_eq!(
            world
                .count(
                    "SELECT count(*) FROM thread_history_cuts WHERE owner_thread_id=$1",
                    thread
                )
                .await,
            0
        );
    } else if import_neutral && choice == "carry" {
        assert_eq!(opened.mode, SessionMode::ImportNeutral);
        assert!(
            world.commands.issued_bodies("fs.materialize").is_empty(),
            "native imports need no workspace handoff file"
        );
        let imported = opened.history.as_ref().unwrap();
        let mut body = store.get(&imported.blob, None).await.unwrap().body;
        use futures::StreamExt;
        let mut bytes = Vec::new();
        while let Some(chunk) = body.next().await {
            bytes.extend_from_slice(&chunk.unwrap());
        }
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("Remember the copper key"), "{text}");
        assert!(
            text.contains("The copper key is kept beside the ledger"),
            "{text}"
        );
    } else {
        assert_eq!(opened.mode, SessionMode::New);
        assert!(
            next.text.contains("Remember the copper key"),
            "{}",
            next.text
        );
        assert!(next.text.contains(".orch/handoff/"), "{}", next.text);
    }
    assert!(next.text.contains("Continue using what you remember"));
    common::eventually("the native program to accept the carried conversation", || async {
        world.count("SELECT count(*) FROM items WHERE thread_id=$1 AND payload->>'marker'='agent_history'",thread).await == 1
    }).await;
    let line = world
        .count(
            "SELECT count(*) FROM items WHERE thread_id=$1 AND payload->>'marker'='agent_history'",
            thread,
        )
        .await;
    assert_eq!(line, 1, "one reconstruction history line");
    common::eventually(
        "only the delivered recipient's notice to be cleared",
        || async {
            let pending: orch_types::Json =
                sqlx::query_scalar("SELECT pending_notices FROM threads WHERE node_id=$1")
                    .bind(thread)
                    .fetch_one(world.pool.pool())
                    .await
                    .unwrap();
            pending.as_array().unwrap().len() == 1 && pending[0]["id"] == obsolete
        },
    )
    .await;
    assert_eq!(world.count("SELECT count(*) FROM items WHERE thread_id=$1 AND search_text LIKE '%begins empty%'",thread).await,0);
    world
        .say(thread, turn_ended(next.turn_id, TurnOutcome::Completed))
        .await;
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_removed_session_imports_the_recorded_conversation_natively_for_codex() {
    recovery(true, false, "carry").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_removed_session_carries_recent_turns_and_the_full_file_for_claude_code() {
    recovery(false, false, "carry").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unexpectedly_missing_session_carries_the_conversation_instead_of_starting_empty() {
    recovery(false, true, "carry").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn summary_without_a_producer_carries_brief_turns_and_the_full_file() {
    recovery(false, false, "summary").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn explicit_none_starts_without_a_handoff_or_a_retained_history_cut() {
    recovery(true, false, "none").await;
}
