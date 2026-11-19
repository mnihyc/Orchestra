//! Selected exports, bounded catch-up titles and opaque raw diagnostics cannot expose another suffix.

mod continuation_read_support;
mod places;
mod rig;
mod support;

use continuation_read_support::{Proof, retained, select};
use orch_api_contract::{
    common::ErrorCode,
    threads::{
        ExportFormat, ItemPayload, ThreadCatchupGet, ThreadCatchupGetQuery, ThreadExport,
        ThreadExportJson, ThreadExportQuery, ThreadRawList, ThreadRawListQuery,
    },
};
use orch_db::queries::threads;
use orch_types::{Id, Time, new_id};
use serde_json::json;

fn query(format: ExportFormat, view: Id) -> ThreadExportQuery {
    ThreadExportQuery {
        continuation_id: Some(view),
        projection_revision: Some(1),
        format,
        detail: None,
        from: None,
        to: None,
        include: None,
    }
}

async fn settled(scene: &rig::Scene, thread: Id) {
    rig::eventually(
        "all actual native end/files work settles before retaining an export cut",
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
}

async fn work(scene: &rig::Scene) -> Proof<(i64, i64, usize)> {
    let (commands, turns): (i64, i64) =
        sqlx::query_as("SELECT (SELECT count(*) FROM commands),(SELECT count(*) FROM turns)")
            .fetch_one(scene.rig.db.pool())
            .await?;
    Ok((
        commands,
        turns,
        scene.rig.machine.handler.executions().len(),
    ))
}

#[tokio::test]
async fn json_markdown_and_common_shared_renderer_export_only_the_retained_frozen_history() -> Proof
{
    let scene = rig::Scene::new().await;
    let thread = scene.thread().await.base.id;
    scene
        .exchange(thread, 0, "Original first prompt", "Frozen first answer")
        .await;
    scene
        .exchange(
            thread,
            1,
            "Excluded second prompt",
            "Excluded second answer",
        )
        .await;
    settled(&scene, thread).await;
    let view = retained(
        &scene.rig.world.app.app.db,
        thread,
        scene.ada.id(),
        Time::new(scene.rig.world.now()),
    )
    .await?;
    // The ordinary source's later revision must not replace the inherited bytes in any renderer.
    let mut tx = scene.rig.db.begin().await?;
    let seq = orch_db::sequences::next(&mut tx, thread)
        .await?
        .ok_or("thread sequence missing")?;
    sqlx::query("UPDATE items SET payload=jsonb_set(payload,'{text}',to_jsonb($2::text)),seq_updated=$3 WHERE id=$1")
        .bind(view.prefix_item).bind("Later source answer").bind(seq).execute(&mut *tx).await?;
    tx.commit().await?;
    let before = work(&scene).await?;
    let app = &scene.rig.world.app;
    let bytes = app
        .call(ThreadExport, &[&thread])
        .token(&scene.ada.token)
        .query(&query(ExportFormat::Json, view.child))
        .send_bytes()
        .await;
    assert_eq!(bytes.status, http::StatusCode::OK);
    let child: ThreadExportJson = serde_json::from_slice(&bytes.raw)?;
    assert_eq!(
        child
            .projection
            .as_ref()
            .map(|scope| (scope.viewed_continuation_id, scope.active_continuation_id)),
        Some((view.child, view.root))
    );
    assert_eq!(
        child
            .turns
            .iter()
            .map(|turn| turn.ordinal)
            .collect::<Vec<_>>(),
        vec![1]
    );
    let inputs: Vec<_> = child
        .items
        .iter()
        .filter_map(|item| match &item.payload {
            ItemPayload::UserMessage(message) => message.input_id,
            _ => None,
        })
        .collect();
    assert_eq!(inputs, vec![view.first_input]);
    assert!(!inputs.contains(&view.before_input));
    let answer = child
        .items
        .iter()
        .find(|item| item.id == view.prefix_item)
        .ok_or("inherited answer missing")?;
    assert!(
        matches!(&answer.payload,ItemPayload::AgentMessage(answer) if answer.text=="Frozen first answer")
    );
    let bytes = app
        .call(ThreadExport, &[&thread])
        .token(&scene.ada.token)
        .query(&query(ExportFormat::Json, view.root))
        .send_bytes()
        .await;
    assert_eq!(bytes.status, http::StatusCode::OK);
    let original: ThreadExportJson = serde_json::from_slice(&bytes.raw)?;
    assert_eq!(
        original
            .turns
            .iter()
            .map(|turn| turn.ordinal)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert!(original.items.iter().any(|item|matches!(&item.payload,ItemPayload::AgentMessage(answer) if answer.text=="Later source answer")));
    let bytes = app
        .call(ThreadExport, &[&thread])
        .token(&scene.ada.token)
        .query(&query(ExportFormat::Markdown, view.child))
        .send_bytes()
        .await;
    assert_eq!(bytes.status, http::StatusCode::OK);
    let markdown = String::from_utf8(bytes.raw.to_vec())?;
    assert!(markdown.contains("Original first prompt") && markdown.contains("Frozen first answer"));
    assert!(!markdown.contains("Excluded second") && !markdown.contains("Later source answer"));
    select(
        &scene.rig.world.app.app.db,
        thread,
        view.child,
        Time::new(scene.rig.world.now()),
    )
    .await?;
    let mut conn = scene.rig.world.conn().await;
    let shared = orch_engine::written::read_conversation(
        &mut conn,
        thread,
        (None, None),
        orch_engine::written::Detail::Normal,
        None,
    )
    .await?
    .ok_or("selected written conversation missing")?;
    assert!(shared.text.contains("Frozen first answer"));
    assert!(
        !shared.text.contains("Excluded second") && !shared.text.contains("Later source answer")
    );
    let ranges = orch_db::queries::shares::turns_between(&mut conn, thread, 1, 2).await?;
    assert_eq!(
        ranges.iter().map(|turn| turn.ordinal).collect::<Vec<_>>(),
        vec![1]
    );
    let old_shared = orch_db::queries::shares::items_of_turns(
        &mut conn,
        thread,
        &original
            .turns
            .iter()
            .map(|turn| turn.id)
            .collect::<Vec<_>>(),
    )
    .await?;
    assert!(
        old_shared.iter().any(
            |item| item.id == view.prefix_item && item.payload["text"] == "Frozen first answer"
        )
    );
    assert!(
        !old_shared
            .iter()
            .any(|item| item.payload.to_string().contains("Excluded second")
                || item.payload["text"] == "Later source answer")
    );
    assert_eq!(
        orch_db::queries::shares::last_turn(&mut conn, thread).await?,
        1
    );
    drop(conn);
    assert_eq!(
        work(&scene).await?,
        before,
        "reading/exporting/browsing creates no command, paid turn or native execution"
    );
    Ok(())
}

/// Explicit persisted reader fixtures, not fabricated native tool execution or model-history evidence.
async fn titles(scene: &rig::Scene, thread: Id, word: &str) -> Proof<(Id, Id)> {
    let mut tx = scene.rig.db.begin().await?;
    let segment = threads::thread(&mut tx, thread)
        .await?
        .ok_or("thread missing")?
        .current_segment_id
        .ok_or("segment missing")?;
    let mut found = Vec::new();
    for (kind, payload) in [
        (
            "tool_call",
            json!({"tool":{"kind":"orch","tool":"announce","arguments":{"category":"milestone","title":word},"result":null},
            "title":word,"started_at":Time::new(scene.rig.world.now()),"duration_ms":0,
            "raw":{"name":"announce","input":{"inline":"{}","total_len":2,"truncated":false,"blob":null},"output":null,"is_error":false}}),
        ),
        (
            "announcement",
            json!({"announcement_id":new_id(),"category":"notice","to_thread":false,"title":word,
            "body":word,"severity":"info","from":{"thread":{"id":thread,"name":"Synthetic reader fixture"}}}),
        ),
    ] {
        let seq = orch_db::sequences::next(&mut tx, thread)
            .await?
            .ok_or("sequence missing")?;
        let id = new_id();
        let row = threads::insert_item(
            &mut tx,
            &threads::NewItem {
                id,
                thread_id: thread,
                segment_id: segment,
                turn_id: None,
                stream: "main",
                parent_item_id: None,
                kind,
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
        .ok_or("reader title insertion failed")?;
        assert_eq!(row.id, id);
        found.push(id);
    }
    tx.commit().await?;
    Ok((found[0], found[1]))
}

#[tokio::test]
async fn catchup_counts_one_history_and_pages_both_title_lists_without_losing_either_cursor()
-> Proof {
    let scene = rig::Scene::new().await;
    let thread = scene.thread().await.base.id;
    scene.exchange(thread, 0, "First", "One").await;
    settled(&scene, thread).await;
    let mut expected = Vec::new();
    for name in ["Earlier one", "Earlier two", "Earlier three"] {
        expected.push(titles(&scene, thread, name).await?);
    }
    scene.exchange(thread, 1, "Excluded second", "Two").await;
    settled(&scene, thread).await;
    let excluded = titles(&scene, thread, "Excluded title").await?;
    let view = retained(
        &scene.rig.world.app.app.db,
        thread,
        scene.ada.id(),
        Time::new(scene.rig.world.now()),
    )
    .await?;
    let before = work(&scene).await?;
    let mut request = ThreadCatchupGetQuery {
        continuation_id: Some(view.child),
        projection_revision: Some(1),
        since: 0,
        limit: Some(1),
        ..Default::default()
    };
    let app = &scene.rig.world.app;
    let mut milestones = Vec::new();
    let mut announcements = Vec::new();
    for page in 0..3 {
        let response = app
            .call(ThreadCatchupGet, &[&thread])
            .token(&scene.ada.token)
            .query(&request)
            .send()
            .await
            .ok();
        assert_eq!(response.turns_completed, 1);
        assert_eq!((response.turns_failed, response.approvals_pending), (0, 0));
        assert_eq!(response.milestones.len(), 1);
        assert_eq!(response.announcements.len(), 1);
        milestones.push(response.milestones[0].item_id);
        announcements.push(response.announcements[0].item_id);
        assert_eq!(response.milestones_next_after.is_some(), page < 2);
        assert_eq!(response.announcements_next_after.is_some(), page < 2);
        request.milestones_after = response.milestones_next_after;
        request.announcements_after = response.announcements_next_after;
    }
    assert_eq!(
        milestones,
        expected.iter().map(|pair| pair.0).collect::<Vec<_>>()
    );
    assert_eq!(
        announcements,
        expected.iter().map(|pair| pair.1).collect::<Vec<_>>()
    );
    assert!(!milestones.contains(&excluded.0) && !announcements.contains(&excluded.1));
    let root = app
        .call(ThreadCatchupGet, &[&thread])
        .token(&scene.ada.token)
        .query(&ThreadCatchupGetQuery {
            continuation_id: Some(view.root),
            since: 0,
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(root.turns_completed, 2);
    assert_eq!((root.milestones.len(), root.announcements.len()), (4, 4));
    assert_eq!(work(&scene).await?, before);
    Ok(())
}

#[tokio::test]
async fn raw_protocol_requires_its_actual_owning_history_instead_of_leaking_an_excluded_suffix()
-> Proof {
    let scene = rig::Scene::new().await;
    let thread = scene.thread().await.base.id;
    scene.exchange(thread, 0, "First", "One").await;
    scene.exchange(thread, 1, "Second", "Two").await;
    settled(&scene, thread).await;
    let view = retained(
        &scene.rig.world.app.app.db,
        thread,
        scene.ada.id(),
        Time::new(scene.rig.world.now()),
    )
    .await?;
    let segment = scene.rig.segment_of(thread).await;
    scene
        .rig
        .say(
            thread,
            orch_link_contract::events::Event::RawFrame(orch_link_contract::events::RawFrame {
                n: 0,
                direction: "out".into(),
                payload: json!({"synthetic":"whole session including the excluded suffix"}),
            }),
        )
        .await;
    rig::eventually("the actual admitted raw frame is retained", || async {
        sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM raw_events WHERE segment_id=$1)")
            .bind(segment)
            .fetch_one(scene.rig.db.pool())
            .await
            .unwrap_or(false)
    })
    .await;
    let before = work(&scene).await?;
    let refused = scene
        .rig
        .world
        .app
        .call(ThreadRawList, &[&thread])
        .token(&scene.ada.token)
        .query(&ThreadRawListQuery {
            segment_id: segment,
            continuation_id: Some(view.child),
            ..Default::default()
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestValueRefused);
    assert_eq!(
        refused.extra.get("continuation_id"),
        Some(&json!(view.root))
    );
    let original = scene
        .rig
        .world
        .app
        .call(ThreadRawList, &[&thread])
        .token(&scene.ada.token)
        .query(&ThreadRawListQuery {
            segment_id: segment,
            continuation_id: Some(view.root),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(original.events.len(), 1);
    assert_eq!(
        original.events[0].payload,
        json!({"synthetic":"whole session including the excluded suffix"})
    );
    assert_eq!(
        original
            .projection
            .as_ref()
            .map(|scope| scope.viewed_continuation_id),
        Some(view.root)
    );
    assert_eq!(work(&scene).await?, before);
    Ok(())
}

#[tokio::test]
async fn archive_preflight_and_shared_turn_bounds_count_the_selected_frozen_payload_and_files()
-> Proof {
    use orch_api_contract::exports::{ExportRequest, NodeExportPreflight};
    let scene = rig::Scene::new().await;
    let thread = scene.thread().await.base.id;
    scene
        .exchange(thread, 0, "Retained prompt", "Retained answer")
        .await;
    scene
        .exchange(
            thread,
            1,
            "Excluded prompt",
            &"Excluded large answer ".repeat(1000),
        )
        .await;
    settled(&scene, thread).await;
    let mut files = Vec::new();
    for bytes in [
        b"retained binary\0\xff".as_slice(),
        b"excluded attachment",
        b"new source-only attachment",
    ] {
        let store = &scene.rig.world.app.app.blobs;
        let meta = store
            .put_bytes(bytes::Bytes::copy_from_slice(bytes))
            .await?;
        let mut conn = scene.rig.world.conn().await;
        store
            .register(&mut conn, meta, scene.rig.world.now())
            .await?;
        files.push(
            json!({"type":"file","path":format!(".orch/uploads/{}.bin",files.len()),
            "name":format!("{}.bin",files.len()),"size":meta.size,"blob":meta.hash.to_string()}),
        );
    }
    // Persisted attachment projection setup; real upload/delivery is proved separately.
    let mut tx = scene.rig.db.begin().await?;
    for ordinal in [1_i32, 2] {
        let seq = orch_db::sequences::next(&mut tx, thread)
            .await?
            .ok_or("thread sequence missing")?;
        sqlx::query("UPDATE items i SET payload=jsonb_set(i.payload,'{parts}',(i.payload->'parts') || $3::jsonb),seq_updated=$4
          FROM turns t WHERE t.thread_id=$1 AND t.ordinal=$2 AND i.turn_id=t.id AND i.kind='user_message'")
            .bind(thread).bind(ordinal).bind(json!([files[usize::try_from(ordinal-1)?].clone()])).bind(seq)
            .execute(&mut *tx).await?;
    }
    tx.commit().await?;
    let view = retained(
        &scene.rig.world.app.app.db,
        thread,
        scene.ada.id(),
        Time::new(scene.rig.world.now()),
    )
    .await?;
    let request: ExportRequest = serde_json::from_value(json!({"what":"all","files":true,
        "parts":{"conversations":true,"chats":false,"documents":false}}))?;
    let app = &scene.rig.world.app;
    let source = app
        .call(NodeExportPreflight, &[&thread])
        .token(&scene.ada.token)
        .body(&request)
        .send()
        .await
        .ok();
    assert_eq!((source.counts.conversations, source.counts.files), (1, 2));
    select(
        &scene.rig.world.app.app.db,
        thread,
        view.child,
        Time::new(scene.rig.world.now()),
    )
    .await?;
    let frozen = app
        .call(NodeExportPreflight, &[&thread])
        .token(&scene.ada.token)
        .body(&request)
        .send()
        .await
        .ok();
    assert_eq!((frozen.counts.conversations, frozen.counts.files), (1, 1));
    assert!(source.bytes > frozen.bytes);
    let mut tx = scene.rig.db.begin().await?;
    let seq = orch_db::sequences::next(&mut tx, thread)
        .await?
        .ok_or("thread sequence missing")?;
    sqlx::query("UPDATE items i SET payload=jsonb_set(i.payload,'{parts}',(i.payload->'parts') || $2::jsonb),seq_updated=$3
      FROM turns t WHERE t.thread_id=$1 AND t.ordinal=1 AND i.turn_id=t.id AND i.kind='user_message'")
        .bind(thread).bind(json!([files[2].clone()])).bind(seq).execute(&mut *tx).await?;
    tx.commit().await?;
    let before = work(&scene).await?;
    let after = app
        .call(NodeExportPreflight, &[&thread])
        .token(&scene.ada.token)
        .body(&request)
        .send()
        .await
        .ok();
    assert_eq!(
        (after.bytes, after.counts.files),
        (frozen.bytes, 1),
        "a later source revision cannot change a retained export's estimate/files"
    );
    let mut conn = scene.rig.world.conn().await;
    let bounds = orch_db::queries::shares::turn_totals(&mut conn, &[thread]).await?;
    assert_eq!(bounds.get(&thread).map(|entry| entry.0), Some(1));
    drop(conn);
    select(
        &scene.rig.world.app.app.db,
        thread,
        view.root,
        Time::new(scene.rig.world.now()),
    )
    .await?;
    let original = app
        .call(NodeExportPreflight, &[&thread])
        .token(&scene.ada.token)
        .body(&request)
        .send()
        .await
        .ok();
    assert_eq!(original.counts.files, 3);
    assert_eq!(
        work(&scene).await?,
        before,
        "aggregate reads never cause native work"
    );
    Ok(())
}
