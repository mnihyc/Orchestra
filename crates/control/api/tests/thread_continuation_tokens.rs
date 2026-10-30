//! Actual scoped-token input/version routes and single-cut retained viewed-version status.

mod continuation_diagnostics;
mod places;
mod rig;
mod support;

use orch_api_contract::{
    auth::{ApiTokenCreateBody, MeTokenCreate, SensitiveProof},
    common::ErrorCode,
    threads::{
        ContentPart, Delivery, PromptEditReason, ThreadContinuationStatus, ThreadInputCreate,
        ThreadInputCreateBody, ThreadOpeningGet, ThreadOpeningGetQuery, ThreadPromptVersionCreate,
        ThreadPromptVersionCreateBody, ThreadPromptVersionList, ThreadPromptVersionListQuery,
        ThreadSync, ThreadSyncGet, ThreadSyncGetQuery,
    },
};
use orch_types::Id;
use serde_json::json;
use std::{
    collections::HashSet,
    sync::{Arc, Mutex, PoisonError},
};

type Proof<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

async fn scene(
    running: bool,
) -> Proof<(
    rig::Scene,
    Id,
    Id,
    orch_agent_contract::history::NativePrefixSource,
)> {
    continuation_diagnostics::install()?;
    let scene = rig::Scene::new().await;
    let thread = scene.thread().await.base.id;
    scene
        .exchange(
            thread,
            0,
            "Actual original prompt",
            "Actual original answer",
        )
        .await;
    rig::eventually(
        "native accounting settles before token admission assertions",
        || async {
            !orch_db::queries::threads::turn_end_work_pending(
                &mut *scene.rig.world.conn().await,
                thread,
            )
            .await
            .unwrap_or(true)
        },
    )
    .await;
    // Admission is exercised through real routes; no supervisor consumes its positive control's input/job.
    if !running {
        scene.rig.engine.stop();
    }
    sqlx::query("UPDATE agents SET descriptor=jsonb_set(descriptor,'{capabilities,native_input_prefix}','\"turn_start\"') WHERE id=$1")
        .bind(scene.on.agent).execute(scene.rig.db.pool()).await?;
    let input: Id = sqlx::query_scalar(
        "SELECT input_id FROM turns WHERE thread_id=$1 ORDER BY ordinal LIMIT 1",
    )
    .bind(thread)
    .fetch_one(scene.rig.db.pool())
    .await?;
    let original = native(&scene, thread, input).await?;
    Ok((scene, thread, input, original))
}

async fn token(scene: &rig::Scene, thread: Id, scopes: &[&str]) -> Proof<String> {
    scene
        .rig
        .world
        .app
        .call(MeTokenCreate, &[])
        .token(&scene.ada.token)
        .body(&ApiTokenCreateBody {
            name: format!("Scoped continuation {}", scopes.len()),
            scopes: Some(scopes.iter().map(|scope| (*scope).to_owned()).collect()),
            node_id: Some(thread),
            expires_at: None,
            proof: SensitiveProof {
                password: Some(support::PASSWORD.into()),
                ..Default::default()
            },
        })
        .send()
        .await
        .ok()
        .token
        .ok_or_else(|| "new synthetic token has no returned secret".into())
}

async fn current(scene: &rig::Scene, thread: Id) -> ThreadSync {
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

fn edit(thread: &ThreadSync, client: &str) -> Proof<ThreadPromptVersionCreateBody> {
    let projection = thread
        .projection
        .as_ref()
        .ok_or("modern read has no continuation scope")?;
    Ok(ThreadPromptVersionCreateBody {
        client_id: client.into(),
        source_continuation_id: projection.viewed_continuation_id,
        content: vec![ContentPart::Text {
            text: "Revised actual prompt".into(),
        }],
        expected_active_continuation_id: projection.active_continuation_id,
        expected_execution_revision: projection.execution_revision,
        expected_queue_revision: thread.thread.queue.revision,
        active_turn: None,
    })
}

async fn counts(scene: &rig::Scene, thread: Id) -> Proof<(i64, i64, i64, i64, usize)> {
    let (inputs,turns,versions,operations):(i64,i64,i64,i64)=sqlx::query_as("SELECT
        (SELECT count(*) FROM thread_inputs WHERE thread_id=$1),(SELECT count(*) FROM turns WHERE thread_id=$1),
        (SELECT count(*) FROM thread_continuations WHERE thread_id=$1),(SELECT count(*) FROM thread_continuation_operations WHERE thread_id=$1)")
        .bind(thread).fetch_one(scene.rig.db.pool()).await?;
    Ok((
        inputs,
        turns,
        versions,
        operations,
        scene.rig.seen.all().len(),
    ))
}

/// Generated persisted reader scale, not fabricated model rounds or paid native history.
async fn pad_reader_tail(scene: &rig::Scene, thread: Id, turn: Id) -> Proof {
    use orch_db::queries::threads;
    let mut tx = scene.rig.db.begin().await?;
    let segment: Id =
        sqlx::query_scalar("SELECT segment_id FROM turns WHERE thread_id=$1 AND id=$2")
            .bind(thread)
            .bind(turn)
            .fetch_one(&mut *tx)
            .await?;
    // More than the default200-item tail, with no extra user inputs or native turns.
    for number in 0..220 {
        let text = format!("Generated later reader item {number}");
        let payload = json!({"text":text,"text_len":text.len(),"phase":null});
        let seq = orch_db::sequences::next(&mut tx, thread)
            .await?
            .ok_or("reader sequence absent")?;
        threads::insert_item(
            &mut tx,
            &threads::NewItem {
                id: orch_types::new_id(),
                thread_id: thread,
                segment_id: segment,
                turn_id: Some(turn),
                stream: "main",
                parent_item_id: None,
                kind: "agent_message",
                status: "completed",
                author_id: None,
                payload: &payload,
                native_ref: None,
                search_text: None,
                at: scene.rig.world.now(),
            },
            seq,
        )
        .await?
        .ok_or("generated reader item absent")?;
    }
    tx.commit().await?;
    Ok(())
}

#[tokio::test]
async fn ordinary_input_and_prompt_edit_require_provider_scope_without_narrowing_passive_version_reads()
-> Proof {
    let (scene, thread, input, _source) = scene(false).await?;
    // Where the place refuses what is written without use of the thread's model (rather than holding
    // it, as shipped), a credential without the provider's scope is refused before anything is made.
    sqlx::query("INSERT INTO settings (node_id,key,value) VALUES ($1,'thread.inputs_without_use','\"refused\"'::jsonb)")
        .bind(scene.tree.topic.base.id)
        .execute(&scene.rig.world.db.pool)
        .await?;
    let narrow = token(&scene, thread, &["thread.view", "thread.interact"]).await?;
    let view = token(&scene, thread, &["thread.view"]).await?;
    let allowed = token(
        &scene,
        thread,
        &[
            "thread.view",
            "thread.interact",
            "provider.use",
            "environment.use",
        ],
    )
    .await?;
    let before = counts(&scene, thread).await?;
    let ordinary = ThreadInputCreateBody {
        plan: false,
        content: vec![ContentPart::Text {
            text: "Ordinary scoped input".into(),
        }],
        delivery: Delivery::Queue,
        client_id: "ordinary-provider-scope".into(),
        bypass: None,
        native_action: None,
    };
    let refused = scene
        .rig
        .world
        .app
        .call(ThreadInputCreate, &[&thread])
        .token(&narrow)
        .body(&ordinary)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra.get("required"), Some(&json!("provider.use")));
    let request = edit(&current(&scene, thread).await, "edit-provider-scope")?;
    let refused = scene
        .rig
        .world
        .app
        .call(ThreadPromptVersionCreate, &[&thread, &input])
        .token(&narrow)
        .body(&request)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra.get("required"), Some(&json!("provider.use")));
    assert_eq!(
        counts(&scene, thread).await?,
        before,
        "missing provider.use creates no input, turn, version, operation or native command"
    );
    let read = scene
        .rig
        .world
        .app
        .call(ThreadSyncGet, &[&thread])
        .token(&narrow)
        .query(&ThreadSyncGetQuery::default())
        .send()
        .await
        .ok();
    let summary = read
        .prompt_summaries
        .iter()
        .find(|summary| summary.input_id == input)
        .ok_or("actual prompt summary missing")?;
    assert_eq!(summary.edit.reason, Some(PromptEditReason::AccessRequired));
    assert!(!summary.edit.available);
    let passive = scene
        .rig
        .world
        .app
        .call(ThreadPromptVersionList, &[&thread, &input])
        .token(&view)
        .query(&ThreadPromptVersionListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!((passive.total, passive.rows.len()), (1, 1));
    assert!(!passive.rows[0].edit.available);
    assert_eq!(
        counts(&scene, thread).await?,
        before,
        "passive reads add no provider-use requirement or runtime work"
    );
    let accepted = scene
        .rig
        .world
        .app
        .call(ThreadInputCreate, &[&thread])
        .token(&allowed)
        .body(&ordinary)
        .send()
        .await;
    assert_eq!(accepted.status, http::StatusCode::CREATED);
    let accepted = accepted.ok();
    assert_eq!(
        accepted.input.state,
        orch_api_contract::threads::InputState::Pending
    );
    let request = edit(&current(&scene, thread).await, "edit-with-provider-scope")?;
    let accepted = scene
        .rig
        .world
        .app
        .call(ThreadPromptVersionCreate, &[&thread, &input])
        .token(&allowed)
        .body(&request)
        .send()
        .await;
    assert_eq!(accepted.status, http::StatusCode::ACCEPTED);
    let receipt = accepted.ok();
    assert!(receipt.input_id.is_some());
    let after = counts(&scene, thread).await?;
    assert_eq!(
        after,
        (before.0 + 2, before.1, before.2 + 1, before.3 + 1, before.4)
    );
    Ok(())
}

#[tokio::test]
async fn deep_linked_pending_and_failed_versions_supply_their_own_retained_status_without_neighbor_requests()
-> Proof {
    let (scene, thread, input, _source) = scene(false).await?;
    let before = current(&scene, thread).await;
    let accepted = scene
        .rig
        .world
        .app
        .call(ThreadPromptVersionCreate, &[&thread, &input])
        .token(&scene.ada.token)
        .body(&edit(&before, "viewed-pending-version")?)
        .send()
        .await
        .ok();
    let query = ThreadOpeningGetQuery {
        continuation_id: Some(accepted.continuation_id),
        ..Default::default()
    };
    // Fresh opening only: no prior versions GET or client click cache supplies these facts.
    let opening = scene
        .rig
        .world
        .app
        .call(ThreadOpeningGet, &[&thread])
        .token(&scene.cai.token)
        .query(&query)
        .send()
        .await
        .ok();
    let viewed = opening
        .sync
        .viewed_prompt
        .as_ref()
        .ok_or("viewed version status absent from a fresh pending opening")?;
    assert_eq!(
        (viewed.group_id, viewed.ordinal, viewed.total),
        (input, 2, 2)
    );
    assert_eq!(
        (viewed.input_id, viewed.continuation_id),
        (
            accepted.input_id.ok_or("edited input absent")?,
            accepted.continuation_id
        )
    );
    assert_eq!(viewed.status, ThreadContinuationStatus::Preparing);
    assert_eq!(
        viewed.item_id, None,
        "pending words have no delivered timeline anchor"
    );
    assert_eq!(viewed.job.as_ref().map(|job| job.id), Some(accepted.job.id));
    assert_eq!(
        opening.sync.thread.active_continuation_id,
        before.thread.active_continuation_id
    );
    assert!(opening.sync.items.iter().all(|item|!matches!(&item.payload,orch_api_contract::threads::ItemPayload::UserMessage(message)
        if message.input_id==accepted.input_id)),"pending revised words are never presented as delivered history");
    assert!(
        opening
            .sync
            .inputs
            .iter()
            .any(|pending| Some(pending.id) == accepted.input_id
                && pending.state == orch_api_contract::threads::InputState::Pending)
    );
    // Read projection fixture: reconstruct a terminal refusal and actual job retention,
    // not a successful native preparation or a second execution path.
    let mut tx = scene.rig.db.begin().await?;
    let job = orch_db::queries::jobs::job(&mut tx, accepted.job.id)
        .await?
        .ok_or("preparation job absent")?;
    assert!(orch_db::queries::jobs::finish(&mut tx,job.id,"failed",&job.progress,None,
        Some(&json!({"code":"synthetic.preparation_refused","message":"The synthetic source refused its exact checkpoint."})),scene.rig.world.now()).await?);
    sqlx::query("UPDATE thread_continuations SET status='failed' WHERE id=$1")
        .bind(accepted.continuation_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE thread_continuation_operations SET status='failed' WHERE id=$1")
        .bind(accepted.operation_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM jobs WHERE id=$1")
        .bind(accepted.job.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let fresh = scene
        .rig
        .world
        .app
        .call(ThreadOpeningGet, &[&thread])
        .token(&scene.cai.token)
        .query(&query)
        .send()
        .await
        .ok();
    let failed = fresh
        .sync
        .viewed_prompt
        .as_ref()
        .ok_or("failed viewed status was lost with its retained job")?;
    assert_eq!(failed.status, ThreadContinuationStatus::Failed);
    assert_eq!(
        failed.item_id, None,
        "failed preparation cannot invent a delivered anchor"
    );
    assert_eq!(
        failed.job.as_ref().map(|job| (job.id, job.state)),
        Some((accepted.job.id, orch_api_contract::nodes::JobState::Failed))
    );
    assert_eq!(
        (
            failed.group_id,
            failed.input_id,
            failed.ordinal,
            failed.total
        ),
        (viewed.group_id, viewed.input_id, 2, 2)
    );
    assert_eq!(
        fresh.sync.projection, opening.sync.projection,
        "progress/failure does not change read projection identity"
    );
    assert_eq!(
        fresh.sync.thread.active_continuation_id,
        before.thread.active_continuation_id
    );
    assert!(
        fresh
            .sync
            .inputs
            .iter()
            .any(|pending| Some(pending.id) == accepted.input_id)
    );
    let root = scene
        .rig
        .world
        .app
        .call(ThreadOpeningGet, &[&thread])
        .token(&scene.cai.token)
        .query(&ThreadOpeningGetQuery::default())
        .send()
        .await
        .ok();
    assert!(
        root.sync.viewed_prompt.is_none(),
        "an original root does not acquire a child edit from a shared group"
    );
    Ok(())
}

#[tokio::test]
async fn first_and_middle_version_neighbors_keep_actual_item_anchors_outside_a_large_opening_tail_without_execution()
-> Proof {
    use orch_api_contract::threads::{ItemPayload, ThreadItemList, ThreadItemListQuery};
    use orch_db::queries::threads;
    for ordinal in [1_i32, 2] {
        let (scene, thread, _, _) = scene(true).await?;
        scene
            .exchange(thread, 1, "Actual middle prompt", "Actual middle answer")
            .await;
        scene
            .exchange(thread, 2, "Actual later prompt", "Actual later answer")
            .await;
        rig::eventually(
            "all actual native input/accounting work settles before generated reader padding",
            || async {
                !threads::turn_end_work_pending(&mut *scene.rig.world.conn().await, thread)
                    .await
                    .unwrap_or(true)
            },
        )
        .await;
        scene.rig.engine.stop();
        let (input,item):(Id,Id)=sqlx::query_as("SELECT t.input_id,i.id FROM turns t JOIN items i ON i.turn_id=t.id
          AND i.kind='user_message' AND i.native_ref='user:'||t.input_id::text WHERE t.thread_id=$1 AND t.ordinal=$2")
            .bind(thread).bind(ordinal).fetch_one(scene.rig.db.pool()).await?;
        native(&scene, thread, input).await?;
        let turn: Id = sqlx::query_scalar("SELECT id FROM turns WHERE thread_id=$1 AND ordinal=3")
            .bind(thread)
            .fetch_one(scene.rig.db.pool())
            .await?;
        pad_reader_tail(&scene, thread, turn).await?;
        let baseline = current(&scene, thread).await;
        let app = &scene.rig.world.app;
        let opening = app
            .call(ThreadOpeningGet, &[&thread])
            .token(&scene.cai.token)
            .query(&ThreadOpeningGetQuery::default())
            .send()
            .await
            .ok();
        assert!(
            !opening.sync.items.iter().any(|row| row.id == item),
            "the actual early prompt lies outside the bounded tail"
        );
        assert!(
            opening.sync.inputs.is_empty(),
            "opening cannot load delivered input bodies to find its anchor"
        );
        let accepted = app
            .call(ThreadPromptVersionCreate, &[&thread, &input])
            .token(&scene.ada.token)
            .body(&edit(
                &baseline,
                &format!("large-history-version-{ordinal}"),
            )?)
            .send()
            .await
            .ok();
        let before = counts(&scene, thread).await?;
        let source_scope = baseline
            .projection
            .as_ref()
            .ok_or("the source history scope is absent")?;
        let original = app
            .call(ThreadPromptVersionList, &[&thread, &input])
            .token(&scene.cai.token)
            .query(&ThreadPromptVersionListQuery {
                before: Some(2),
                limit: Some(1),
                continuation_id: Some(source_scope.viewed_continuation_id),
                projection_revision: Some(source_scope.projection_revision),
                ..Default::default()
            })
            .send()
            .await
            .ok();
        assert_eq!(
            (
                original.total,
                original.rows.len(),
                original.rows[0].ordinal
            ),
            (2, 1, 1)
        );
        assert_eq!(
            (original.rows[0].input_id, original.rows[0].item_id),
            (input, Some(item))
        );
        assert!(
            !original.rows[0].edit.available,
            "passive neighboring reads never borrow the author's act"
        );
        let pending = app
            .call(ThreadPromptVersionList, &[&thread, &input])
            .token(&scene.cai.token)
            .query(&ThreadPromptVersionListQuery {
                after: Some(1),
                limit: Some(1),
                continuation_id: Some(source_scope.viewed_continuation_id),
                projection_revision: Some(source_scope.projection_revision),
                ..Default::default()
            })
            .send()
            .await
            .ok();
        assert_eq!(pending.rows.len(), 1);
        assert_eq!(Some(pending.rows[0].input_id), accepted.input_id);
        assert_eq!(pending.rows[0].item_id, None);
        assert_eq!(pending.rows[0].status, ThreadContinuationStatus::Preparing);
        let own = app
            .call(ThreadOpeningGet, &[&thread])
            .token(&scene.cai.token)
            .query(&ThreadOpeningGetQuery {
                continuation_id: Some(accepted.continuation_id),
                ..Default::default()
            })
            .send()
            .await
            .ok();
        assert_eq!(
            own.sync
                .viewed_prompt
                .as_ref()
                .and_then(|prompt| prompt.item_id),
            None
        );
        assert_eq!(
            own.sync
                .viewed_prompt
                .as_ref()
                .map(|prompt| prompt.input_id),
            accepted.input_id
        );
        let anchored = app
            .call(ThreadItemList, &[&thread])
            .token(&scene.cai.token)
            .query(&ThreadItemListQuery {
                around: original.rows[0].item_id,
                continuation_id: Some(original.rows[0].continuation_id),
                projection_revision: Some(1),
                limit: Some(5),
                ..Default::default()
            })
            .send()
            .await
            .ok();
        assert!(anchored.items.iter().any(|row|row.id==item && matches!(&row.payload,ItemPayload::UserMessage(message) if message.input_id==Some(input))));
        assert_eq!(
            anchored
                .projection
                .as_ref()
                .map(|scope| scope.viewed_continuation_id),
            Some(original.rows[0].continuation_id)
        );
        assert_eq!(
            current(&scene, thread).await.thread.active_continuation_id,
            baseline.thread.active_continuation_id
        );
        assert_eq!(
            counts(&scene, thread).await?,
            before,
            "version browsing and positioned loads cannot issue native work or another operation/input/turn"
        );
    }
    Ok(())
}

#[tokio::test]
async fn an_actually_delivered_revised_input_gives_its_own_anchor_in_fresh_opening_and_adjacent_read_only_versions()
-> Proof {
    use orch_api_contract::threads::{ItemPayload, ThreadItemList, ThreadItemListQuery};
    use orch_link_contract::events::TurnOutcome;
    let (scene, thread, input, _original) = scene(true).await?;
    let baseline = current(&scene, thread).await;
    let original_item: Id = sqlx::query_scalar(
        "SELECT id FROM items WHERE thread_id=$1 AND kind='user_message'
      AND native_ref='user:'||$2::uuid::text",
    )
    .bind(thread)
    .bind(input)
    .fetch_one(scene.rig.db.pool())
    .await?;
    let receipt = scene
        .rig
        .world
        .app
        .call(ThreadPromptVersionCreate, &[&thread, &input])
        .token(&scene.ada.token)
        .body(&edit(&baseline, "actually-delivered-version-anchor")?)
        .send()
        .await;
    assert_eq!(receipt.status, http::StatusCode::ACCEPTED);
    let receipt = receipt.ok();
    let revised = receipt
        .input_id
        .ok_or("the explicitly revised input identity is absent")?;
    let start = scene.rig.turn_start(thread, 1).await;
    let delivered: Id = sqlx::query_scalar("SELECT input_id FROM turns WHERE id=$1")
        .bind(start.turn_id)
        .fetch_one(scene.rig.db.pool())
        .await?;
    assert_eq!(delivered, revised);
    scene
        .rig
        .answer(
            thread,
            &start,
            "actual-revised-answer",
            "Answer to the explicitly revised words",
        )
        .await;
    scene
        .rig
        .say(
            thread,
            rig::turn_ended(start.turn_id, TurnOutcome::Completed),
        )
        .await;
    scene.rig.wait_turn(start.turn_id, "completed").await;
    rig::eventually(
        "the actual revised turn and its terminal obligations settle",
        || async {
            !orch_db::queries::threads::turn_end_work_pending(
                &mut *scene.rig.world.conn().await,
                thread,
            )
            .await
            .unwrap_or(true)
        },
    )
    .await;
    pad_reader_tail(&scene, thread, start.turn_id).await?;
    let item: Id = sqlx::query_scalar(
        "SELECT id FROM items WHERE thread_id=$1 AND turn_id=$2 AND kind='user_message'
      AND native_ref='user:'||$3::uuid::text AND payload->>'input_id'=$3::uuid::text",
    )
    .bind(thread)
    .bind(start.turn_id)
    .bind(revised)
    .fetch_one(scene.rig.db.pool())
    .await?;
    assert_ne!(item, original_item);
    let before = counts(&scene, thread).await?;
    let app = &scene.rig.world.app;
    let opening = app
        .call(ThreadOpeningGet, &[&thread])
        .token(&scene.cai.token)
        .query(&ThreadOpeningGetQuery {
            continuation_id: Some(receipt.continuation_id),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let viewed = opening
        .sync
        .viewed_prompt
        .as_ref()
        .ok_or("the delivered selected version has no metadata")?;
    assert_eq!(
        (
            viewed.input_id,
            viewed.item_id,
            viewed.ordinal,
            viewed.total
        ),
        (revised, Some(item), 2, 2)
    );
    assert_eq!(viewed.status, ThreadContinuationStatus::Ready);
    assert_eq!(
        opening.sync.thread.active_continuation_id,
        Some(receipt.continuation_id)
    );
    assert!(
        !opening.sync.items.iter().any(|row| row.id == item),
        "the viewed prompt's own anchor is supplied even outside its bounded tail"
    );
    assert!(
        opening.sync.inputs.is_empty(),
        "delivered words are not duplicated as pending-input bodies"
    );
    let positioned = app
        .call(ThreadItemList, &[&thread])
        .token(&scene.cai.token)
        .query(&ThreadItemListQuery {
            around: viewed.item_id,
            continuation_id: Some(viewed.continuation_id),
            projection_revision: Some(1),
            limit: Some(5),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(positioned.items.iter().any(|row|row.id==item && matches!(&row.payload,ItemPayload::UserMessage(message) if message.input_id==Some(revised))));
    let versions = app
        .call(ThreadPromptVersionList, &[&thread, &revised])
        .token(&scene.cai.token)
        .query(&ThreadPromptVersionListQuery {
            continuation_id: Some(receipt.continuation_id),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(
        versions
            .rows
            .iter()
            .map(|row| (row.input_id, row.item_id, row.ordinal))
            .collect::<Vec<_>>(),
        vec![(input, Some(original_item), 1), (revised, Some(item), 2)]
    );
    assert!(versions.rows.iter().all(|row| !row.edit.available));
    let original = app
        .call(ThreadOpeningGet, &[&thread])
        .token(&scene.cai.token)
        .query(&ThreadOpeningGetQuery {
            continuation_id: baseline.thread.active_continuation_id,
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(original.sync.viewed_prompt.is_none());
    assert_eq!(
        original.sync.thread.active_continuation_id,
        Some(receipt.continuation_id),
        "reading the old version cannot activate it"
    );
    assert_eq!(counts(&scene, thread).await?, before);
    assert_eq!(
        scene.rig.turn_starts(thread).len(),
        2,
        "only the original and explicitly revised words have a native turn"
    );
    Ok(())
}

/// This synthetic native program implements the reported first-input contract through
/// the real machine link/journal. It is not a vendor model-history exactness proof.
async fn native(
    scene: &rig::Scene,
    thread: Id,
    input: Id,
) -> Proof<orch_agent_contract::history::NativePrefixSource> {
    use orch_agent_contract::history::{NativeInputPrefix, NativePrefixOpened, NativePrefixSource};
    use orch_link_contract::commands::{
        Command, CommandError, CommandKind, CommandResult, Empty, ErrorCode, SessionOpenResult,
    };
    let (segment,native,turn):(Id,String,Id)=sqlx::query_as("SELECT s.id,s.native_session_ref,t.id FROM threads h
        JOIN segments s ON s.id=h.current_segment_id JOIN turns t ON t.segment_id=s.id AND t.input_id=$2 WHERE h.node_id=$1")
        .bind(thread).bind(input).fetch_one(scene.rig.db.pool()).await?;
    let original = NativePrefixSource {
        thread_id: thread,
        segment_id: segment,
        native_session_ref: native,
    };
    let descriptor: serde_json::Value =
        sqlx::query_scalar("SELECT descriptor FROM agents WHERE id=$1")
            .bind(scene.on.agent)
            .fetch_one(scene.rig.db.pool())
            .await?;
    let agent = scene.on.agent;
    let provider = scene.on.provider;
    scene
        .rig
        .machine
        .handler
        .on(CommandKind::AgentStart, move |command| {
            let Command::AgentStart(body) = command else {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    "wrong native command",
                ));
            };
            if body.agent_id != agent || body.provider_id != provider {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    "another synthetic program was requested",
                ));
            }
            Ok(CommandResult::AgentStart(
                orch_link_contract::commands::AgentStartResult {
                    reused: true,
                    harness_version: "1.0.0".into(),
                    descriptor: serde_json::from_value(descriptor.clone()).map_err(|error| {
                        CommandError::new(ErrorCode::Internal, error.to_string())
                    })?,
                },
            ))
        });
    let current = Arc::new(Mutex::new(Some(original.clone())));
    let known = Arc::new(Mutex::new(HashSet::from([original
        .native_session_ref
        .clone()])));
    let prefix = NativeInputPrefix {
        harness_kind: "fake".into(),
        source: original.clone(),
        before_input_ref: turn.to_string(),
        fingerprint: orch_types::Hash::of(b"synthetic scoped native checkpoint"),
        empty: true,
    };
    let expected = prefix.clone();
    let slot = current.clone();
    scene
        .rig
        .machine
        .handler
        .on(CommandKind::SessionPrefixInspect, move |command| {
            let Command::SessionPrefixInspect(body) = command else {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    "wrong native command",
                ));
            };
            if body.source != expected.source
                || body.before_input_ref != expected.before_input_ref
                || slot.lock().unwrap_or_else(PoisonError::into_inner).as_ref()
                    != Some(&expected.source)
            {
                return Err(CommandError::new(
                    ErrorCode::Busy,
                    "The exact native source is not idle here.",
                ));
            }
            Ok(CommandResult::SessionPrefixInspect(expected.clone()))
        });
    let slot = current.clone();
    let registry = known.clone();
    scene
        .rig
        .machine
        .handler
        .on(CommandKind::SessionPrefixOpen, move |command| {
            let Command::SessionPrefixOpen(body) = command else {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    "wrong native command",
                ));
            };
            let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
            if body.prefix != prefix || slot.is_some() {
                return Err(CommandError::new(
                    ErrorCode::Busy,
                    "Another native lane occupies this thread.",
                ));
            }
            let child = format!("scoped-native-child-{}", body.session.segment_id);
            registry
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(child.clone());
            *slot = Some(NativePrefixSource {
                thread_id: thread,
                segment_id: body.session.segment_id,
                native_session_ref: child.clone(),
            });
            Ok(CommandResult::SessionPrefixOpen(NativePrefixOpened {
                prefix: body.prefix.clone(),
                session: orch_agent_contract::session::SessionOpened {
                    native_session_ref: child,
                    resumed: false,
                    import_report: None,
                    instruction_sources: Vec::new(),
                    approval_reviewer: None,
                },
            }))
        });
    let slot = current.clone();
    scene
        .rig
        .machine
        .handler
        .on(CommandKind::SessionContinuationClose, move |command| {
            let Command::SessionContinuationClose(body) = command else {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    "wrong native command",
                ));
            };
            let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
            if slot.as_ref().is_some_and(|held| held != &body.source) {
                return Err(CommandError::new(
                    ErrorCode::Busy,
                    "The close names another native lane.",
                ));
            }
            *slot = None;
            Ok(CommandResult::SessionContinuationClose(Empty {}))
        });
    let refresh_current = current.clone();
    let refresh_known = known.clone();
    scene
        .rig
        .machine
        .handler
        .on(CommandKind::SessionContinuationResume, move |command| {
            let Command::SessionContinuationResume(body) = command else {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    "wrong native command",
                ));
            };
            let mut slot = current.lock().unwrap_or_else(PoisonError::into_inner);
            if !known
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains(&body.source.native_session_ref)
                || slot.as_ref().is_some_and(|held| {
                    held != &body.source
                        && body.replace_segment_id != Some(held.segment_id)
                        && body.replace_retained_source.as_ref() != Some(held)
                })
            {
                return Err(CommandError::new(
                    ErrorCode::Busy,
                    "The exact retained native lane is unavailable.",
                ));
            }
            *slot = Some(NativePrefixSource {
                thread_id: thread,
                segment_id: body.session.segment_id,
                native_session_ref: body.source.native_session_ref.clone(),
            });
            Ok(CommandResult::SessionContinuationResume(
                SessionOpenResult {
                    native_session_ref: body.source.native_session_ref.clone(),
                    resumed: true,
                    import_report: None,
                    instruction_sources: Vec::new(),
                    approval_reviewer: None,
                },
            ))
        });
    let current = refresh_current;
    let known = refresh_known;
    scene
        .rig
        .machine
        .handler
        .on(CommandKind::SessionOpenFenced, move |command| {
            let Command::SessionOpenFenced(body) = command else {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    "wrong native command",
                ));
            };
            let held = current.lock().unwrap_or_else(PoisonError::into_inner);
            let held = held.as_ref().ok_or_else(|| {
                CommandError::new(ErrorCode::Busy, "No synthetic native lane is prepared.")
            })?;
            if body.thread_id != thread
                || body.session.agent_id != agent
                || body.session.segment_id != held.segment_id
                || body.session.native_session_ref.as_deref()
                    != Some(held.native_session_ref.as_str())
                || body.predecessor.as_ref() != Some(held)
                || !known
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .contains(&held.native_session_ref)
            {
                return Err(CommandError::new(
                    ErrorCode::Busy,
                    "The synthetic session refresh names another prepared lane.",
                ));
            }
            Ok(CommandResult::SessionOpenFenced(SessionOpenResult {
                native_session_ref: held.native_session_ref.clone(),
                resumed: true,
                import_report: None,
                instruction_sources: Vec::new(),
                approval_reviewer: None,
            }))
        });
    Ok(original)
}

#[tokio::test]
async fn narrowing_only_provider_use_before_effects_or_activation_keeps_original_and_frozen_grants_truthful()
-> Proof {
    use orch_link_contract::commands::CommandKind;
    for after_creation in [false, true] {
        let (scene, thread, input, original) = scene(true).await?;
        let secret = token(
            &scene,
            thread,
            &[
                "thread.view",
                "thread.interact",
                "provider.use",
                "environment.use",
            ],
        )
        .await?;
        let token:Id=sqlx::query_scalar("SELECT id FROM api_tokens WHERE user_id=$1 AND scopes @> ARRAY['provider.use']::text[]")
            .bind(scene.ada.id()).fetch_one(scene.rig.db.pool()).await?;
        let baseline = current(&scene, thread).await;
        let native_before = scene.rig.machine.handler.executions().len();
        let hold = if after_creation {
            CommandKind::SessionPrefixOpen
        } else {
            CommandKind::SessionPrefixInspect
        };
        scene.rig.machine.handler.hold(hold);
        let accepted = scene
            .rig
            .world
            .app
            .call(ThreadPromptVersionCreate, &[&thread, &input])
            .token(&secret)
            .body(&edit(
                &baseline,
                if after_creation {
                    "narrow-before-activation"
                } else {
                    "narrow-before-effects"
                },
            )?)
            .send()
            .await
            .ok();
        let commands = orch_engine::continuations::recipe::Commands::of(accepted.operation_id);
        let held = if after_creation {
            commands.open_prefix
        } else {
            commands.preinspect
        };
        tokio::time::timeout(rig::PATIENCE, scene.rig.machine.handler.started(held)).await?;
        let frozen: serde_json::Value = sqlx::query_scalar(
            "SELECT plan#>'{recipe,authorization_scope,scopes}' FROM jobs WHERE id=$1",
        )
        .bind(accepted.operation_id)
        .fetch_one(scene.rig.db.pool())
        .await?;
        assert!(
            frozen
                .as_array()
                .is_some_and(|scopes| scopes.contains(&json!("provider.use")))
        );
        sqlx::query(
            "UPDATE api_tokens SET scopes=ARRAY['thread.view','thread.interact'] WHERE id=$1",
        )
        .bind(token)
        .execute(scene.rig.db.pool())
        .await?;
        scene.rig.machine.handler.release();
        rig::eventually(
            "the revoked provider scope ends the job and confirms original native recovery",
            || async {
                sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS(SELECT 1 FROM jobs WHERE id=$1 AND state='failed'
                AND error->>'code'='authz.denied')",
                )
                .bind(accepted.job.id)
                .fetch_one(scene.rig.db.pool())
                .await
                .unwrap_or(false)
            },
        )
        .await;
        let failed = current(&scene, thread).await;
        assert_eq!(
            failed.thread.active_continuation_id,
            baseline.thread.active_continuation_id
        );
        assert_eq!(
            failed.thread.execution_revision,
            baseline.thread.execution_revision
        );
        assert_eq!(failed.thread.current_turn_id, None);
        let original_ref: String =
            sqlx::query_scalar("SELECT native_session_ref FROM segments WHERE id=$1")
                .bind(original.segment_id)
                .fetch_one(scene.rig.db.pool())
                .await?;
        assert_eq!(original_ref, original.native_session_ref);
        let turns: i64 = sqlx::query_scalar("SELECT count(*) FROM turns WHERE thread_id=$1")
            .bind(thread)
            .fetch_one(scene.rig.db.pool())
            .await?;
        assert_eq!(
            turns, 1,
            "neither preparation nor token revocation creates an autonomous turn"
        );
        let revised = accepted.input_id.ok_or("revised input absent")?;
        let row: (Id, String) =
            sqlx::query_as("SELECT continuation_id,state FROM thread_inputs WHERE id=$1")
                .bind(revised)
                .fetch_one(scene.rig.db.pool())
                .await?;
        assert_eq!(row, (accepted.continuation_id, "pending".into()));
        let executions = scene.rig.machine.handler.executions();
        let prefix = executions
            .iter()
            .filter(|run| run.kind == CommandKind::SessionPrefixOpen)
            .count();
        assert_eq!(prefix, usize::from(after_creation));
        if !after_creation {
            assert!(executions[native_before..].iter().all(|run| !matches!(
                run.kind,
                CommandKind::SessionContinuationClose | CommandKind::SessionContinuationResume
            )));
        } else {
            let creation: serde_json::Value = sqlx::query_scalar(
                "SELECT native_creation_result FROM thread_continuations WHERE id=$1",
            )
            .bind(accepted.continuation_id)
            .fetch_one(scene.rig.db.pool())
            .await?;
            assert!(
                creation["session"]["native_session_ref"]
                    .as_str()
                    .is_some_and(|native| native.starts_with("scoped-native-child-"))
            );
            assert_eq!(scene.rig.machine.handler.runs_of(commands.open_prefix), 1);
            assert!(
                executions
                    .iter()
                    .any(|run| run.kind == CommandKind::SessionContinuationResume)
            );
        }
        let viewed = scene
            .rig
            .world
            .app
            .call(ThreadSyncGet, &[&thread])
            .token(&secret)
            .query(&ThreadSyncGetQuery {
                continuation_id: Some(accepted.continuation_id),
                ..Default::default()
            })
            .send()
            .await
            .ok();
        assert_eq!(
            viewed.viewed_prompt.as_ref().map(|prompt| prompt.status),
            Some(ThreadContinuationStatus::Failed)
        );
        assert_eq!(
            viewed.thread.active_continuation_id,
            baseline.thread.active_continuation_id
        );
    }
    Ok(())
}
