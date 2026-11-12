//! Real HTTP continuation reads preserve actual IDs, frozen bytes and independent scoped read positions.

mod continuation_read_support;
mod places;
mod rig;
mod support;

use continuation_read_support::{Proof, retained, select};
use orch_api_contract::{
    common::ErrorCode,
    personal::{ThreadRead, ThreadReadBody},
    threads::{
        ItemPayload, PromptEditReason, ThreadItemGet, ThreadOpeningGet, ThreadOpeningGetQuery,
        ThreadPromptVersionList, ThreadPromptVersionListQuery, ThreadSyncGet, ThreadSyncGetQuery,
    },
};
use orch_types::Time;

#[tokio::test]
async fn returned_prompt_summaries_are_batched_without_delivered_input_or_versions_fanout() -> Proof
{
    let scene = rig::Scene::new().await;
    let thread = scene.thread().await.base.id;
    scene
        .exchange(thread, 0, "Original first", "First answer")
        .await;
    scene
        .exchange(thread, 1, "Original second", "Second answer")
        .await;
    rig::eventually("both native turns finish terminal follow-up", || async {
        !orch_db::queries::threads::turn_end_work_pending(
            &mut *scene.rig.world.conn().await,
            thread,
        )
        .await
        .unwrap_or(true)
    })
    .await;
    let view = retained(
        &scene.rig.world.app.app.db,
        thread,
        scene.ada.id(),
        Time::new(scene.rig.world.now()),
    )
    .await?;
    let before: (i64, i64) =
        sqlx::query_as("SELECT (SELECT count(*) FROM commands),(SELECT count(*) FROM turns)")
            .fetch_one(scene.rig.db.pool())
            .await?;
    let app = &scene.rig.world.app;
    let opening = app
        .call(ThreadOpeningGet, &[&thread])
        .token(&scene.ada.token)
        .query(&ThreadOpeningGetQuery::default())
        .send()
        .await
        .ok();
    assert!(
        opening.sync.inputs.is_empty(),
        "completed input bodies stay out of a bounded opening"
    );
    let mut actual = opening
        .sync
        .items
        .iter()
        .filter_map(|item| match &item.payload {
            ItemPayload::UserMessage(message) => message.input_id,
            _ => None,
        })
        .collect::<Vec<_>>();
    actual.sort_unstable();
    actual.dedup();
    let mut summaries = opening
        .sync
        .prompt_summaries
        .iter()
        .map(|summary| summary.input_id)
        .collect::<Vec<_>>();
    summaries.sort_unstable();
    assert_eq!(summaries, actual);
    assert_eq!(summaries.len(), 2);
    for summary in &opening.sync.prompt_summaries {
        assert_eq!(
            (summary.group_id, summary.ordinal, summary.total),
            (summary.input_id, 1, 1)
        );
        assert_eq!(
            summary.author.as_ref().map(|author| author.id),
            Some(scene.ada.id())
        );
        assert_eq!(
            summary.edit.reason,
            Some(PromptEditReason::NativeBoundaryUnsupported)
        );
        assert!(
            !summary.edit.available,
            "the ordinary fake adapter does not declare exact native prefixes"
        );
    }
    let child = app
        .call(ThreadOpeningGet, &[&thread])
        .token(&scene.ada.token)
        .query(&ThreadOpeningGetQuery {
            continuation_id: Some(view.child),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(
        child
            .sync
            .items
            .iter()
            .any(|item| item.id == view.prefix_item)
    );
    assert!(
        !child
            .sync
            .prompt_summaries
            .iter()
            .any(|summary| summary.input_id == view.before_input)
    );
    assert!(
        child
            .sync
            .prompt_summaries
            .iter()
            .any(|summary| summary.input_id == view.first_input)
    );
    assert_eq!(
        child
            .sync
            .projection
            .as_ref()
            .map(|projection| projection.active_continuation_id),
        Some(view.root)
    );
    assert_eq!(
        child.sync.thread.active_continuation_id,
        Some(view.root),
        "browsing cannot select execution"
    );
    let frozen = app
        .call(ThreadItemGet, &[&thread, &view.prefix_item])
        .token(&scene.ada.token)
        .query(
            &orch_api_contract::threads::continuations::ThreadReadSelector {
                continuation_id: Some(view.child),
                ..Default::default()
            },
        )
        .send()
        .await
        .ok();
    let mut conn = scene.rig.world.conn().await;
    let seq = orch_db::sequences::next(&mut conn, thread)
        .await?
        .ok_or("missing thread sequence")?;
    sqlx::query(
        "UPDATE items SET payload=jsonb_set(payload,'{text}',to_jsonb($2::text)),seq_updated=$3
        WHERE id=$1",
    )
    .bind(view.prefix_item)
    .bind("Later source revision")
    .bind(seq)
    .execute(&mut *conn)
    .await?;
    drop(conn);
    let reread = app
        .call(ThreadItemGet, &[&thread, &view.prefix_item])
        .token(&scene.ada.token)
        .query(
            &orch_api_contract::threads::continuations::ThreadReadSelector {
                continuation_id: Some(view.child),
                ..Default::default()
            },
        )
        .send()
        .await
        .ok();
    assert_eq!(
        reread.payload, frozen.payload,
        "a source update cannot rewrite inherited bytes"
    );
    let original = app
        .call(ThreadItemGet, &[&thread, &view.prefix_item])
        .token(&scene.ada.token)
        .query(
            &orch_api_contract::threads::continuations::ThreadReadSelector {
                continuation_id: Some(view.root),
                ..Default::default()
            },
        )
        .send()
        .await
        .ok();
    let ItemPayload::AgentMessage(original) = original.payload else {
        return Err("the native answer changed kind".into());
    };
    assert_eq!(original.text, "Later source revision");
    let after: (i64, i64) =
        sqlx::query_as("SELECT (SELECT count(*) FROM commands),(SELECT count(*) FROM turns)")
            .fetch_one(scene.rig.db.pool())
            .await?;
    assert_eq!(
        after, before,
        "read-only arrows/summary/output do not prepare native work or make turns"
    );
    Ok(())
}

#[tokio::test]
async fn an_inactive_read_acknowledgment_never_marks_the_selected_history_read() -> Proof {
    let scene = rig::Scene::new().await;
    let thread = scene.thread().await.base.id;
    scene.exchange(thread, 0, "First", "One").await;
    scene.exchange(thread, 1, "Second", "Two").await;
    rig::eventually(
        "the native follow-up completes before the read cut",
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
    let db = &scene.rig.world.app.app.db;
    let view = retained(db, thread, scene.ada.id(), Time::new(scene.rig.world.now())).await?;
    let first = scene
        .rig
        .world
        .app
        .call(ThreadSyncGet, &[&thread])
        .token(&scene.ada.token)
        .query(&ThreadSyncGetQuery::default())
        .send()
        .await
        .ok();
    select(db, thread, view.child, Time::new(scene.rig.world.now())).await?;
    let stale = scene
        .rig
        .world
        .app
        .call(ThreadRead, &[&thread])
        .token(&scene.ada.token)
        .body(&ThreadReadBody {
            seq: first.seq,
            projection_revision: first
                .projection
                .as_ref()
                .map(|scope| scope.projection_revision),
            ..Default::default()
        })
        .send()
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    let ack = scene
        .rig
        .world
        .app
        .call(ThreadRead, &[&thread])
        .token(&scene.ada.token)
        .body(&ThreadReadBody {
            seq: first.seq,
            continuation_id: Some(view.root),
            projection_revision: Some(1),
        })
        .send()
        .await
        .ok();
    assert_eq!(ack.last_read_seq, first.seq);
    assert_eq!(
        ack.projection
            .as_ref()
            .map(|scope| (scope.viewed_continuation_id, scope.active_continuation_id)),
        Some((view.root, view.child))
    );
    let read: Vec<(orch_types::Id, i64)> = sqlx::query_as(
        "SELECT continuation_id,last_read_seq FROM thread_reads
        WHERE user_id=$1 AND thread_id=$2 ORDER BY continuation_id",
    )
    .bind(scene.ada.id())
    .bind(thread)
    .fetch_all(scene.rig.db.pool())
    .await?;
    assert_eq!(read, vec![(view.root, first.seq)]);
    let versions = scene
        .rig
        .world
        .app
        .call(ThreadPromptVersionList, &[&thread, &view.first_input])
        .token(&scene.cai.token)
        .query(&ThreadPromptVersionListQuery {
            continuation_id: Some(view.child),
            projection_revision: Some(1),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(
        (
            versions.total,
            versions.selected_version,
            versions.rows.len()
        ),
        (1, 1, 1)
    );
    assert!(
        !versions.rows[0].edit.available,
        "a viewer does not borrow the original author identity"
    );
    assert_eq!(
        versions.rows[0].edit.reason,
        Some(PromptEditReason::NotAuthor)
    );
    Ok(())
}
