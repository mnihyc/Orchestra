//! Genuine old native reports retain their own observations without changing a selected running history or rebilling it.

mod continuation_diagnostics;
mod continuation_execution_support;
mod continuation_read_support;
mod places;
mod rig;
mod support;

use continuation_read_support::{Proof, retained, select};
use orch_agent_contract::history::NativePrefixSource;
use orch_api_contract::threads::{
    ContentPart, Delivery, ThreadContinuationAccepted, ThreadPromptVersionCreate,
    ThreadPromptVersionCreateBody, ThreadSyncGet, ThreadSyncGetQuery, TokenUsage,
};
use orch_link_contract::commands::{Command, CommandKind};
use orch_link_contract::{
    events::{EmittedItemKind, Event, ItemStarted, SessionTitle, TurnOutcome, UsageUpdated},
    streams::Stream,
};
use orch_types::{Id, Json, Time};
use serde_json::json;

fn usage(turn: Id, total: i64) -> Event {
    let counts = TokenUsage {
        input_tokens: total,
        total_tokens: total,
        ..Default::default()
    };
    Event::UsageUpdated(UsageUpdated {
        turn_id: turn,
        turn: counts,
        thread: counts,
        context_window: Some(100_000),
        context_used: Some(total),
        report: None,
    })
}

async fn current(scene: &rig::Scene, thread: Id) -> orch_api_contract::threads::ThreadSync {
    scene
        .rig
        .world
        .app
        .call(ThreadSyncGet, &[&thread])
        .token(&scene.ada.token)
        .query(&ThreadSyncGetQuery::default())
        .send()
        .await
        .ok()
}

async fn selected_start(
    scene: &rig::Scene,
    thread: Id,
) -> Proof<orch_link_contract::commands::TurnStart> {
    let found = tokio::time::timeout(rig::PATIENCE, async {
        loop {
            if let Some(start) = scene.rig.turn_starts(thread).get(2).cloned() {
                break start;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await;
    match found {
        Ok(start) => Ok(start),
        Err(_) => {
            // Same bounded diagnostic allowance as the native-tool fixture; this
            // happens only after the unchanged proof budget has already failed.
            let state=tokio::time::timeout(std::time::Duration::from_secs(2),sqlx::query_scalar::<_,Json>("SELECT jsonb_build_object('state',h.state,'blocked',h.blocked_reason,
                'queue_held',h.queue_held,'current_turn',h.current_turn_id,'segment',h.current_segment_id,
                'job_state',j.state,'phase',j.progress->>'phase','error',j.error->>'code')
                FROM threads h LEFT JOIN LATERAL(SELECT state,progress,error FROM jobs WHERE node_id=h.node_id
                  AND kind='thread_continuation' ORDER BY created_at DESC LIMIT 1) j ON true WHERE h.node_id=$1")
                .bind(thread).fetch_one(scene.rig.db.pool())).await;
            let state = match state {
                Ok(Ok(state)) => state.to_string(),
                Ok(Err(_)) => "metadata_unavailable".into(),
                Err(_) => "metadata_deadline".into(),
            };
            Err(format!("late-stream phase=guarded-selected-native-start exceeded original {:?}; metadata={state}",rig::PATIENCE).into())
        }
    }
}

#[tokio::test]
async fn an_old_stream_cannot_rename_replace_context_reopen_running_or_attribute_items_to_the_selected_turn()
-> Proof {
    continuation_diagnostics::install()?;
    let scene = rig::Scene::new().await;
    let thread = scene.thread().await.base.id;
    scene
        .exchange(thread, 0, "Original first", "Original first answer")
        .await;
    scene
        .exchange(thread, 1, "Original second", "Original second answer")
        .await;
    rig::eventually(
        "real original terminal work is accounted before the reconstruction",
        || async {
            sqlx::query_scalar::<_, bool>(
                "SELECT NOT EXISTS(SELECT 1 FROM turns WHERE thread_id=$1
          AND (end_work IS NOT NULL OR files_end_due OR files_work_pending))",
            )
            .bind(thread)
            .fetch_one(scene.rig.db.pool())
            .await
            .unwrap_or(false)
        },
    )
    .await;
    let (old_turn, old_segment, before_input, native_ref): (Id, Id, Id, String) = sqlx::query_as(
        "SELECT t.id,t.segment_id,t.input_id,s.native_session_ref
        FROM turns t JOIN segments s ON s.id=t.segment_id AND s.continuation_id=t.continuation_id
        WHERE t.thread_id=$1 AND t.ordinal=2",
    )
    .bind(thread)
    .fetch_one(scene.rig.db.pool())
    .await?;
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM items WHERE thread_id=$1 AND turn_id=$2
        AND kind='user_message' AND payload->>'input_id'=$3::uuid::text AND NOT before_orch)"
        )
        .bind(thread)
        .bind(old_turn)
        .bind(before_input)
        .fetch_one(scene.rig.db.pool())
        .await?
    );
    let old_ledger: Vec<Json> =
        sqlx::query_scalar("SELECT to_jsonb(u) FROM usage_events u WHERE turn_id=$1 ORDER BY id")
            .bind(old_turn)
            .fetch_all(scene.rig.db.pool())
            .await?;
    assert_eq!(
        old_ledger.len(),
        1,
        "the actual old native turn has one retained accounting receipt"
    );
    sqlx::query("UPDATE agents SET descriptor=jsonb_set(descriptor,'{capabilities,native_input_prefix}','\"turn_start\"') WHERE id=$1")
        .bind(scene.on.agent).execute(scene.rig.db.pool()).await?;
    let descriptor: Json = sqlx::query_scalar("SELECT descriptor FROM agents WHERE id=$1")
        .bind(scene.on.agent)
        .fetch_one(scene.rig.db.pool())
        .await?;
    let original = NativePrefixSource {
        thread_id: thread,
        segment_id: old_segment,
        native_session_ref: native_ref,
    };
    let native = continuation_execution_support::install(
        &scene.rig.machine.handler,
        scene.on.agent,
        scene.on.provider,
        original.clone(),
        old_turn,
        serde_json::from_value(descriptor)?,
    );
    let baseline = current(&scene, thread).await;
    let active = baseline
        .thread
        .active_continuation_id
        .ok_or("the actual original continuation is absent")?;
    let revision = baseline
        .thread
        .execution_revision
        .ok_or("the actual original execution revision is absent")?;
    let accepted: ThreadContinuationAccepted = scene
        .rig
        .world
        .app
        .call(ThreadPromptVersionCreate, &[&thread, &before_input])
        .token(&scene.ada.token)
        .body(&ThreadPromptVersionCreateBody {
            client_id: "late-report-selected-turn".into(),
            source_continuation_id: active,
            content: vec![ContentPart::Text {
                text: "New selected work".into(),
            }],
            expected_active_continuation_id: active,
            expected_execution_revision: revision,
            expected_queue_revision: baseline.thread.queue.revision,
            active_turn: None,
        })
        .send()
        .await
        .ok();
    let running = selected_start(&scene, thread).await?;
    let input: Id = sqlx::query_scalar("SELECT input_id FROM turns WHERE id=$1")
        .bind(running.turn_id)
        .fetch_one(scene.rig.db.pool())
        .await?;
    assert_eq!(Some(input), accepted.input_id);
    let (selected,turn_owner,segment_owner,segment,child):(Id,Id,Id,Id,String)=sqlx::query_as("SELECT h.active_continuation_id,
        t.continuation_id,s.continuation_id,s.id,s.native_session_ref FROM threads h JOIN turns t ON t.id=h.current_turn_id
        JOIN segments s ON s.id=h.current_segment_id AND s.id=t.segment_id WHERE h.node_id=$1 AND t.id=$2")
        .bind(thread).bind(running.turn_id).fetch_one(scene.rig.db.pool()).await?;
    assert_eq!(
        (selected, turn_owner, segment_owner),
        (
            accepted.continuation_id,
            accepted.continuation_id,
            accepted.continuation_id
        )
    );
    assert_ne!(running.turn_id, old_turn);
    assert_ne!(segment, old_segment);
    assert_ne!(child, original.native_session_ref);
    let witnessed = native
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let prefix = witnessed
        .iter()
        .filter_map(|command| match command {
            Command::SessionPrefixOpen(open) => Some(open),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(prefix.len(), 1);
    assert_eq!(prefix[0].prefix.source, original);
    assert!(!prefix[0].prefix.empty);
    assert_eq!(prefix[0].prefix.before_input_ref, old_turn.to_string());
    assert_eq!(prefix[0].session.segment_id, segment);
    let starts = scene
        .rig
        .seen
        .all()
        .into_iter()
        .filter_map(|command| match command {
            Command::TurnStartFenced(start) if start.turn.turn_id == running.turn_id => Some(start),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(starts.len(), 1);
    assert_eq!(starts[0].segment_id, segment);
    assert_eq!(starts[0].turn.thread_id, thread);
    assert_eq!(
        child,
        format!("scoped-native-child-{segment}"),
        "the selected segment must retain the exact PrefixOpen result identity"
    );
    let refresh = witnessed
        .iter()
        .filter_map(|command| match command {
            Command::SessionOpenFenced(open) if open.session.segment_id == segment => Some(open),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        refresh.len(),
        1,
        "the actual fenced refresh must reach the strict prepared-child handler"
    );
    for open in refresh {
        assert_eq!(
            open.session.native_session_ref.as_deref(),
            Some(child.as_str())
        );
        assert_eq!(
            open.predecessor.as_ref(),
            Some(&NativePrefixSource {
                thread_id: thread,
                segment_id: segment,
                native_session_ref: child.clone()
            })
        );
    }
    assert_eq!(
        scene
            .rig
            .commands
            .issued_of(CommandKind::SessionPrefixOpen.as_str()),
        1
    );
    let commands = orch_engine::continuations::recipe::Commands::of(accepted.operation_id);
    assert_eq!(
        scene.rig.machine.handler.runs_of(commands.open_prefix),
        1,
        "the original admitted prefix command runs once"
    );
    let journal: orch_link::journal::Entry = serde_json::from_slice(
        &tokio::fs::read(
            scene
                .rig
                .machine
                .data_dir()
                .join("journal")
                .join(format!("{}.json", commands.open_prefix)),
        )
        .await?,
    )?;
    assert_eq!(journal.cmd.id, commands.open_prefix);
    assert_eq!(journal.cmd.kind, CommandKind::SessionPrefixOpen.as_str());
    assert_eq!(journal.cmd.key, Some(thread.to_string()));
    assert!(
        journal.cmd.body == serde_json::to_value(prefix[0])?,
        "the received typed prefix body equals the real native journal body"
    );
    let applied: orch_agent_contract::history::NativePrefixOpened = serde_json::from_value(
        journal
            .result()
            .ok_or("the exact prefix command has no actual durable terminal result")?
            .outcome
            .clone()
            .map_err(|error| error.detail)?,
    )?;
    assert_eq!(applied.prefix, prefix[0].prefix);
    assert_eq!(applied.session.native_session_ref, child);
    scene
        .rig
        .answer(
            thread,
            &running,
            "selected-answer",
            "The selected work is running",
        )
        .await;
    scene.rig.say(thread, usage(running.turn_id, 123)).await;
    scene
        .rig
        .world
        .set_at(
            orch_settings::Target::Node(scene.tree.topic.base.id),
            "auto.title",
            json!("harness"),
        )
        .await;
    sqlx::query("UPDATE nodes SET title_source='auto',auto_updated_at=NULL WHERE id=$1")
        .bind(thread)
        .execute(scene.rig.db.pool())
        .await?;
    scene
        .rig
        .say(
            thread,
            Event::SessionTitle(SessionTitle {
                title: "Selected lane title".into(),
            }),
        )
        .await;
    rig::eventually("the eligible selected native title and context are committed",||async {
        let row=sqlx::query_scalar::<_,bool>("SELECT n.name='Selected lane title' AND t.usage->>'context_used'='123'
          AND t.state='running' AND t.current_turn_id=$2 FROM threads t JOIN nodes n ON n.id=t.node_id WHERE t.node_id=$1")
            .bind(thread).bind(running.turn_id).fetch_one(scene.rig.db.pool()).await;
        row.unwrap_or(false)
    }).await;
    let before = current(&scene, thread).await;
    let activity: (i64, chrono::DateTime<chrono::Utc>) =
        sqlx::query_as("SELECT activity_seq,last_activity_at FROM threads WHERE node_id=$1")
            .bind(thread)
            .fetch_one(scene.rig.db.pool())
            .await?;
    let starts = scene.rig.turn_starts(thread).len();
    scene.rig.world.advance(chrono::Duration::seconds(1));
    let stream = Stream::Session {
        thread_id: thread,
        segment_id: old_segment,
    };
    scene
        .rig
        .machine
        .publish(
            &stream,
            &Event::SessionTitle(SessionTitle {
                title: "Rejected old lane title".into(),
            }),
        )
        .await;
    scene
        .rig
        .machine
        .publish(&stream, &usage(old_turn, 999))
        .await;
    scene
        .rig
        .machine
        .publish(&stream, &rig::turn_began(old_turn))
        .await;
    scene
        .rig
        .machine
        .publish(&stream, &rig::turn_ended(old_turn, TurnOutcome::Completed))
        .await;
    scene
        .rig
        .machine
        .publish(
            &stream,
            &Event::ItemStarted(ItemStarted {
                turn_id: old_turn,
                native_ref: "late-old-error".into(),
                stream: "main".into(),
                parent_native_ref: None,
                kind: EmittedItemKind::Error,
                payload: orch_link_contract::foreign::ItemPayload::Error(
                    orch_agent_contract::events::ErrorPayload {
                        code: "synthetic.old_observation".into(),
                        message: "Retained old error observation".into(),
                        retryable: false,
                    },
                ),
            }),
        )
        .await;
    // No start exists for this late item. Its stream must not borrow the new lane's running turn.
    scene
        .rig
        .machine
        .publish(
            &stream,
            &rig::message_done("late-old-complete", "Retained late original answer"),
        )
        .await;
    rig::eventually("the actual old-stream report batch reaches its final retained item",||async {
        sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM items WHERE segment_id=$1 AND native_ref='late-old-complete')
          AND EXISTS(SELECT 1 FROM turns WHERE id=$2 AND usage->>'total_tokens'='999')")
            .bind(old_segment).bind(old_turn).fetch_one(scene.rig.db.pool()).await.unwrap_or(false)
    }).await;
    let after = current(&scene, thread).await;
    assert_eq!(after.thread.base.name, before.thread.base.name);
    assert_eq!(after.thread.usage, before.thread.usage);
    assert_eq!(
        (after.thread.state, after.thread.current_turn_id),
        (before.thread.state, Some(running.turn_id))
    );
    assert_eq!(
        after.thread.active_continuation_id,
        Some(accepted.continuation_id)
    );
    assert_eq!(after.projection, before.projection);
    let late:Vec<(String,Id,Option<Id>)>=sqlx::query_as("SELECT native_ref,continuation_id,turn_id FROM items
      WHERE segment_id=$1 AND native_ref IN ('late-old-error','late-old-complete') ORDER BY native_ref")
        .bind(old_segment).fetch_all(scene.rig.db.pool()).await?;
    assert_eq!(
        late,
        vec![
            ("late-old-complete".into(), active, None),
            ("late-old-error".into(), active, Some(old_turn))
        ]
    );
    assert!(
        !after
            .items
            .iter()
            .any(|item| item.segment_id == old_segment && item.seq_created > before.seq)
    );
    let same_activity: (i64, chrono::DateTime<chrono::Utc>) =
        sqlx::query_as("SELECT activity_seq,last_activity_at FROM threads WHERE node_id=$1")
            .bind(thread)
            .fetch_one(scene.rig.db.pool())
            .await?;
    assert_eq!(
        same_activity, activity,
        "an old suffix cannot make the selected history unread or recently active"
    );
    let ledger: Vec<Json> =
        sqlx::query_scalar("SELECT to_jsonb(u) FROM usage_events u WHERE turn_id=$1 ORDER BY id")
            .bind(old_turn)
            .fetch_all(scene.rig.db.pool())
            .await?;
    assert_eq!(
        ledger, old_ledger,
        "a late observed total cannot replace or duplicate the immutable terminal usage obligation"
    );
    assert_eq!(scene.rig.turn_starts(thread).len(), starts);
    scene
        .rig
        .say(
            thread,
            rig::turn_ended(running.turn_id, TurnOutcome::Completed),
        )
        .await;
    scene.rig.wait_turn(running.turn_id, "completed").await;
    Ok(())
}

#[tokio::test]
async fn current_context_reads_the_selected_prefix_while_usage_history_retains_every_actual_turn()
-> Proof {
    use orch_api_contract::context::{
        ThreadContextGet, ThreadContextGetQuery, ThreadTurnUsageList, ThreadTurnUsageListQuery,
    };
    let scene = rig::Scene::new().await;
    let thread = scene.thread().await.base.id;
    let mut turns = Vec::new();
    for (number, total) in [(0_usize, 31_i64), (1, 411)] {
        scene
            .say(
                &scene.ada,
                thread,
                &format!("Actual input {number}"),
                Delivery::Queue,
                &format!("selected-context-{number}"),
            )
            .await
            .ok();
        let start = scene.rig.turn_start(thread, number).await;
        scene
            .rig
            .answer(
                thread,
                &start,
                &format!("context-answer-{number}"),
                &format!("Actual answer {number}"),
            )
            .await;
        scene.rig.say(thread, usage(start.turn_id, total)).await;
        scene
            .rig
            .say(
                thread,
                rig::turn_ended(start.turn_id, TurnOutcome::Completed),
            )
            .await;
        scene.rig.wait_turn(start.turn_id, "completed").await;
        turns.push(start.turn_id);
    }
    rig::eventually(
        "both real terminal context/accounting reports are retained",
        || async {
            sqlx::query_scalar::<_, bool>(
                "SELECT count(*)=2 AND bool_and(context IS NOT NULL AND end_work IS NULL
          AND NOT files_end_due AND NOT files_work_pending) FROM turns WHERE thread_id=$1",
            )
            .bind(thread)
            .fetch_one(scene.rig.db.pool())
            .await
            .unwrap_or(false)
        },
    )
    .await;
    let app = &scene.rig.world.app;
    let original = app
        .call(ThreadContextGet, &[&thread])
        .token(&scene.ada.token)
        .query(&ThreadContextGetQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(original.turn_id, Some(turns[1]));
    assert_eq!(original.fill.as_ref().map(|fill| fill.used), Some(411));
    let ledger: Vec<Json> =
        sqlx::query_scalar("SELECT to_jsonb(u) FROM usage_events u WHERE thread_id=$1 ORDER BY id")
            .bind(thread)
            .fetch_all(scene.rig.db.pool())
            .await?;
    assert_eq!(ledger.len(), 2);
    let view = retained(
        &scene.rig.world.app.app.db,
        thread,
        scene.ada.id(),
        Time::new(scene.rig.world.now()),
    )
    .await?;
    select(
        &scene.rig.world.app.app.db,
        thread,
        view.child,
        Time::new(scene.rig.world.now()),
    )
    .await?;
    let commands = scene.rig.machine.handler.executions().len();
    let selected = app
        .call(ThreadContextGet, &[&thread])
        .token(&scene.ada.token)
        .query(&ThreadContextGetQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(selected.turn_id, Some(turns[0]));
    assert_eq!(selected.fill.as_ref().map(|fill| fill.used), Some(31));
    assert!(
        !selected.can_count,
        "a history with no owned native lane cannot count another lane's context"
    );
    let lifetime = app
        .call(ThreadTurnUsageList, &[&thread])
        .token(&scene.ada.token)
        .query(&ThreadTurnUsageListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(
        lifetime
            .turns
            .iter()
            .map(|turn| turn.turn_id)
            .collect::<Vec<_>>(),
        turns
    );
    assert_eq!(
        lifetime
            .turns
            .iter()
            .map(|turn| turn.usage.map(|usage| usage.total_tokens))
            .collect::<Vec<_>>(),
        vec![Some(31), Some(411)]
    );
    let same: Vec<Json> =
        sqlx::query_scalar("SELECT to_jsonb(u) FROM usage_events u WHERE thread_id=$1 ORDER BY id")
            .bind(thread)
            .fetch_all(scene.rig.db.pool())
            .await?;
    assert_eq!(same, ledger);
    assert_eq!(scene.rig.machine.handler.executions().len(), commands);
    Ok(())
}
