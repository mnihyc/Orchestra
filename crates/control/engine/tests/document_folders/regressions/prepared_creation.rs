//! Prepared native folder creation keeps the real transaction, first effect and durable replay.

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

async fn material(tx: &mut orch_db::Tx, node: Id, key: &str) -> TestResult<(i64, i64, i64)> {
    Ok(sqlx::query_as(
        "SELECT
        (SELECT count(*) FROM document_containers WHERE node_id=$1),
        (SELECT count(*) FROM audit_log WHERE node_id=$1 AND object_kind='document_container'),
        (SELECT count(*) FROM document_tree_operations WHERE request_ref=$2)",
    )
    .bind(node)
    .bind(key)
    .fetch_one(&mut **tx)
    .await?)
}

#[tokio::test]
async fn prepared_folder_check_precedes_material_and_native_replay_never_calls_it_again()
-> TestResult {
    let area = Area::new(true).await?;
    let mut command = request(
        domain::Command::CreateFolder,
        Vec::new(),
        Some(domain::Directory::NodeDocs(area.topic)),
        "prepared-native-folder",
    );
    command.name = Some("Prepared research".into());
    let now = Time::new(area.world.clock.now());
    let options = domain::Options::for_engine(&area.world.engine());
    let calls = AtomicUsize::new(0);
    let mut tx = area.world.pool.begin().await?;
    let actor = domain::Actor::Agent { thread: area.agent };
    let before = material(&mut tx, area.topic, &command.request_ref).await?;
    let rejected = domain::prepare_folder_creation_in(&mut tx, actor, &command, now).await?;
    assert_eq!(rejected.destination(), command.to);
    assert!(!rejected.is_replay());
    assert_eq!(
        material(&mut tx, area.topic, &command.request_ref).await?,
        before,
        "native preparation must not create a folder, receipt or creation audit"
    );
    let result = domain::apply_folder_creation_in(&mut tx, actor, rejected, now, options, || {
        calls.fetch_add(1, Ordering::SeqCst);
        Err::<(), _>("the caller's exact before-effect refusal")
    })
    .await;
    assert!(matches!(
        result,
        Err("the caller's exact before-effect refusal")
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        material(&mut tx, area.topic, &command.request_ref).await?,
        before
    );
    let allowed = domain::prepare_folder_creation_in(&mut tx, actor, &command, now).await?;
    let receipt = domain::apply_folder_creation_in(&mut tx, actor, allowed, now, options, || {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok::<(), &str>(())
    })
    .await??;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(receipt.entries.len(), 1);
    let identity = receipt.entries[0].item;
    tx.commit().await?;
    let mut replay_tx = area.world.pool.begin().await?;
    let before_replay = material(&mut replay_tx, area.topic, &command.request_ref).await?;
    let replay = domain::prepare_folder_creation_in(&mut replay_tx, actor, &command, now).await?;
    assert!(replay.is_replay());
    let repeated =
        domain::apply_folder_creation_in(&mut replay_tx, actor, replay, now, options, || {
            calls.fetch_add(1, Ordering::SeqCst);
            Err::<(), _>("a replay must never reach the first-effect seam")
        })
        .await??;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(repeated.operation, receipt.operation);
    assert_eq!(repeated.entries[0].item, identity);
    assert_eq!(
        material(&mut replay_tx, area.topic, &command.request_ref).await?,
        before_replay
    );
    replay_tx.commit().await?;
    Ok(())
}

#[tokio::test]
async fn prepared_folder_packet_from_an_ended_transaction_cannot_apply_or_call_the_check()
-> TestResult {
    let area = Area::new(true).await?;
    let mut command = request(
        domain::Command::CreateFolder,
        Vec::new(),
        Some(domain::Directory::NodeDocs(area.topic)),
        "ended-native-preparation",
    );
    command.name = Some("Never created".into());
    let now = Time::new(area.world.clock.now());
    let options = domain::Options::for_engine(&area.world.engine());
    let actor = domain::Actor::Agent { thread: area.agent };
    let calls = AtomicUsize::new(0);
    let mut original = area.world.pool.begin().await?;
    let prepared = domain::prepare_folder_creation_in(&mut original, actor, &command, now).await?;
    original.rollback().await?;
    let mut other = area.world.pool.begin().await?;
    let before = material(&mut other, area.topic, &command.request_ref).await?;
    let result =
        domain::apply_folder_creation_in(&mut other, actor, prepared, now, options, || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok::<(), &str>(())
        })
        .await?;
    assert!(matches!(result, Err(domain::Error::Stale)));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        material(&mut other, area.topic, &command.request_ref).await?,
        before
    );
    other.commit().await?;
    Ok(())
}

async fn text_material(tx: &mut orch_db::Tx, id: Id) -> TestResult<(i64, i64)> {
    Ok(sqlx::query_as(
        "SELECT
        (SELECT count(*) FROM documents WHERE id=$1),
        (SELECT count(*) FROM document_tree_operations WHERE operation_id=$1)",
    )
    .bind(id)
    .fetch_one(&mut **tx)
    .await?)
}

#[tokio::test]
async fn prepared_text_check_precedes_the_native_header_and_stable_id_replay_never_calls_it_again()
-> TestResult {
    let area = Area::new(false).await?;
    let now = Time::new(area.world.clock.now());
    let actor = domain::Actor::Agent { thread: area.agent };
    let id = new_id();
    let parent = domain::Directory::NodeDocs(area.topic);
    let calls = AtomicUsize::new(0);
    let mut tx = area.world.pool.begin().await?;
    let rejected =
        domain::prepare_text_creation_in(&mut tx, actor, parent, id, "Prepared text", None, now)
            .await?;
    assert_eq!(rejected.id(), id);
    assert_eq!(rejected.destination(), parent);
    assert_eq!(rejected.node(), area.topic);
    assert!(!rejected.is_replay());
    assert_eq!(text_material(&mut tx, id).await?, (0, 0));
    domain::require_text_creation_in(&mut tx, actor, &rejected, now).await?;
    let result = domain::apply_text_creation_in(&mut tx, actor, rejected, now, || {
        calls.fetch_add(1, Ordering::SeqCst);
        Err::<(), _>("the caller's exact before-header refusal")
    })
    .await;
    assert!(matches!(
        result,
        Err("the caller's exact before-header refusal")
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(text_material(&mut tx, id).await?, (0, 0));
    let allowed =
        domain::prepare_text_creation_in(&mut tx, actor, parent, id, "Prepared text", None, now)
            .await?;
    let created = domain::apply_text_creation_in(&mut tx, actor, allowed, now, || {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok::<(), &str>(())
    })
    .await??;
    assert!(created.created);
    assert_eq!(created.document.id, id);
    assert_eq!(text_material(&mut tx, id).await?, (1, 1));
    let version: String = sqlx::query_scalar("SELECT xmin::text FROM documents WHERE id=$1")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    let mut replay_tx = area.world.pool.begin().await?;
    let replay = domain::prepare_text_creation_in(
        &mut replay_tx,
        actor,
        parent,
        id,
        "Prepared text",
        None,
        now,
    )
    .await?;
    assert!(replay.is_replay());
    let repeated = domain::apply_text_creation_in(&mut replay_tx, actor, replay, now, || {
        calls.fetch_add(1, Ordering::SeqCst);
        Err::<(), _>("a native header replay must not check a new effect")
    })
    .await??;
    assert!(!repeated.created);
    assert_eq!(repeated.document.id, id);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(text_material(&mut replay_tx, id).await?, (1, 1));
    let after: String = sqlx::query_scalar("SELECT xmin::text FROM documents WHERE id=$1")
        .bind(id)
        .fetch_one(&mut *replay_tx)
        .await?;
    assert_eq!(after, version);
    replay_tx.commit().await?;
    Ok(())
}

#[tokio::test]
async fn prepared_text_packet_from_an_ended_transaction_cannot_create_a_header() -> TestResult {
    let area = Area::new(false).await?;
    let now = Time::new(area.world.clock.now());
    let actor = domain::Actor::Agent { thread: area.agent };
    let id = new_id();
    let calls = AtomicUsize::new(0);
    let mut original = area.world.pool.begin().await?;
    let prepared = domain::prepare_text_creation_in(
        &mut original,
        actor,
        domain::Directory::NodeDocs(area.topic),
        id,
        "Never inserted header",
        None,
        now,
    )
    .await?;
    original.rollback().await?;
    let mut other = area.world.pool.begin().await?;
    let result = domain::apply_text_creation_in(&mut other, actor, prepared, now, || {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok::<(), &str>(())
    })
    .await?;
    assert!(matches!(result, Err(domain::Error::Stale)));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(text_material(&mut other, id).await?, (0, 0));
    other.commit().await?;
    Ok(())
}

#[tokio::test]
async fn native_entry_selection_completes_only_the_chosen_window_ancestry() -> TestResult {
    let area = Area::new(false).await?;
    let alpha = area.folder("alpha/").await?;
    let beta = area.folder("beta/").await?;
    let first = area
        .document("alpha/notes.md", "First native entry")
        .await?;
    let second = area
        .document("beta/notes.md", "Second native entry")
        .await?;
    let now = Time::new(area.world.clock.now());
    let actor = domain::Actor::Agent { thread: area.agent };
    let mut conn = area.world.pool.acquire().await?;
    let first_row = documents::containers::entry(&mut conn, "document", first)
        .await?
        .ok_or("first native row absent")?;
    let second_row = documents::containers::entry(&mut conn, "document", second)
        .await?
        .ok_or("second native row absent")?;
    let expected =
        domain::project_entries_in(&mut conn, actor, vec![first_row.clone()], now, false).await?;
    let mut selected =
        domain::select_entries_in(&mut conn, actor, vec![first_row, second_row], now, false)
            .await?;
    assert_eq!(selected.entries().len(), 2);
    selected.retain(|row| row.entry.item == domain::Item::Document(first));
    let projected = domain::project_selected_in(&mut conn, actor, selected, now, false).await?;
    assert_eq!(projected, expected);
    assert!(projected.ancestors.iter().any(|row| row.id == alpha));
    assert!(!projected.ancestors.iter().any(|row| row.id == beta));
    Ok(())
}

#[tokio::test]
async fn native_entry_selection_never_lends_its_previous_actors_read_standing() -> TestResult {
    let area = Area::new(false).await?;
    let document = area
        .document("private-to-the-reading-agent.md", "Native selected content")
        .await?;
    let other = area.world.thread("Agent without that document grant").await;
    let now = Time::new(area.world.clock.now());
    let mut conn = area.world.pool.acquire().await?;
    let row = documents::containers::entry(&mut conn, "document", document)
        .await?
        .ok_or("native row absent")?;
    let producer = domain::Actor::Agent { thread: area.agent };
    let reader = domain::Actor::Agent { thread: other };
    let selected = domain::select_entries_in(&mut conn, producer, vec![row], now, false).await?;
    assert_eq!(selected.entries().len(), 1);
    let projected = domain::project_selected_in(&mut conn, reader, selected, now, false).await?;
    assert!(projected.entries.is_empty());
    assert!(projected.ancestors.is_empty());
    Ok(())
}

async fn movement_material(
    tx: &mut orch_db::Tx,
    document: Id,
    reference: &str,
) -> TestResult<(Option<Id>, String, i64, i64, i64)> {
    Ok(sqlx::query_as(
        "SELECT parent_container_id,xmin::text,doc_seq,
        (SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='document.moved'),
        (SELECT count(*) FROM document_tree_operations WHERE request_ref=$2)
        FROM documents WHERE id=$1",
    )
    .bind(document)
    .bind(reference)
    .fetch_one(&mut **tx)
    .await?)
}
#[tokio::test]
async fn prepared_document_move_checks_before_update_and_native_replay_never_moves_twice()
-> TestResult {
    let area = Area::new(true).await?;
    let folder = area.folder("move-source/").await?;
    let document = area
        .document("move-source/notes.md", "Native movement source")
        .await?;
    let now = Time::new(area.world.clock.now());
    let options = domain::Options::for_engine(&area.world.engine());
    let actor = domain::Actor::Agent { thread: area.agent };
    let command = request(
        domain::Command::Move,
        vec![domain::Item::Document(document)],
        Some(domain::Directory::NodeDocs(area.topic)),
        "checked-native-movement",
    );
    let calls = AtomicUsize::new(0);
    let mut tx = area.world.pool.begin().await?;
    let before = movement_material(&mut tx, document, &command.request_ref).await?;
    assert_eq!(before.0, Some(folder));
    let rejected = domain::prepare_document_movement_in(&mut tx, actor, &command, now).await?;
    assert_eq!(rejected.document(), Some(document));
    assert_eq!(rejected.destination(), command.to);
    assert!(!rejected.is_replay());
    assert_eq!(
        movement_material(&mut tx, document, &command.request_ref).await?,
        before
    );
    let result = domain::apply_document_movement_in(&mut tx, actor, rejected, now, options, || {
        calls.fetch_add(1, Ordering::SeqCst);
        Err::<(), _>("the caller's exact before-move refusal")
    })
    .await;
    assert!(matches!(
        result,
        Err("the caller's exact before-move refusal")
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        movement_material(&mut tx, document, &command.request_ref).await?,
        before
    );
    let allowed = domain::prepare_document_movement_in(&mut tx, actor, &command, now).await?;
    let receipt = domain::apply_document_movement_in(&mut tx, actor, allowed, now, options, || {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok::<(), &str>(())
    })
    .await??;
    assert_eq!(receipt.entries[0].item, domain::Item::Document(document));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let moved = movement_material(&mut tx, document, &command.request_ref).await?;
    assert_eq!(moved.0, None);
    assert!(moved.2 > before.2);
    assert_eq!(moved.3, before.3 + 1);
    assert_eq!(moved.4, 1);
    tx.commit().await?;
    let mut repeated_tx = area.world.pool.begin().await?;
    let repeated =
        domain::prepare_document_movement_in(&mut repeated_tx, actor, &command, now).await?;
    assert!(repeated.is_replay());
    let replay =
        domain::apply_document_movement_in(&mut repeated_tx, actor, repeated, now, options, || {
            calls.fetch_add(1, Ordering::SeqCst);
            Err::<(), _>("a replay cannot reach the move UPDATE")
        })
        .await??;
    assert_eq!(replay.operation, receipt.operation);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        movement_material(&mut repeated_tx, document, &command.request_ref).await?,
        moved
    );
    repeated_tx.commit().await?;
    Ok(())
}
#[tokio::test]
async fn prepared_document_move_from_an_ended_transaction_refuses_before_callback_or_material()
-> TestResult {
    let area = Area::new(true).await?;
    area.folder("ended-move/").await?;
    let document = area
        .document("ended-move/notes.md", "Unapplied native source")
        .await?;
    let now = Time::new(area.world.clock.now());
    let options = domain::Options::for_engine(&area.world.engine());
    let actor = domain::Actor::Agent { thread: area.agent };
    let command = request(
        domain::Command::Move,
        vec![domain::Item::Document(document)],
        Some(domain::Directory::NodeDocs(area.topic)),
        "ended-movement-preparation",
    );
    let mut original = area.world.pool.begin().await?;
    let prepared =
        domain::prepare_document_movement_in(&mut original, actor, &command, now).await?;
    original.rollback().await?;
    let calls = AtomicUsize::new(0);
    let mut other = area.world.pool.begin().await?;
    let before = movement_material(&mut other, document, &command.request_ref).await?;
    let result =
        domain::apply_document_movement_in(&mut other, actor, prepared, now, options, || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok::<(), &str>(())
        })
        .await?;
    assert!(matches!(result, Err(domain::Error::Stale)));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        movement_material(&mut other, document, &command.request_ref).await?,
        before
    );
    other.commit().await?;
    Ok(())
}

async fn delete_native_text(
    area: &Area,
    document: Id,
    now: Time,
    options: domain::Options,
) -> TestResult {
    let actor = domain::Actor::Agent { thread: area.agent };
    let mut tx = area.world.pool.begin().await?;
    let deletion = request(
        domain::Command::Delete,
        vec![domain::Item::Document(document)],
        None,
        "native-restore-source-deletion",
    );
    let plan = domain::plan_delete_in(&mut tx, actor, &deletion, now, options).await?;
    domain::execute_in(&mut tx, actor, &deletion, Some(&plan.stamp), now, options).await?;
    tx.commit().await?;
    Ok(())
}
async fn restoration_material(
    tx: &mut orch_db::Tx,
    document: Id,
    reference: &str,
) -> TestResult<(bool, String, i64, i64, i64)> {
    Ok(sqlx::query_as(
        "SELECT deleted_at IS NOT NULL,xmin::text,doc_seq,
        (SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='document.restored'),
        (SELECT count(*) FROM document_tree_operations WHERE request_ref=$2)
        FROM documents WHERE id=$1",
    )
    .bind(document)
    .bind(reference)
    .fetch_one(&mut **tx)
    .await?)
}
#[tokio::test]
async fn prepared_document_restore_checks_before_clearing_deletion_and_replay_never_restores_twice()
-> TestResult {
    let area = Area::new(true).await?;
    let document = area
        .document("retained-native-source.md", "Retained content")
        .await?;
    let now = Time::new(area.world.clock.now());
    let options = domain::Options::for_engine(&area.world.engine());
    delete_native_text(&area, document, now, options).await?;
    let actor = domain::Actor::Agent { thread: area.agent };
    let command = request(
        domain::Command::Restore,
        vec![domain::Item::Document(document)],
        None,
        "checked-native-restoration",
    );
    let calls = AtomicUsize::new(0);
    let mut tx = area.world.pool.begin().await?;
    let before = restoration_material(&mut tx, document, &command.request_ref).await?;
    assert!(before.0);
    let rejected =
        domain::prepare_document_restoration_in(&mut tx, actor, &command, now, options).await?;
    assert_eq!(rejected.document(), Some(document));
    assert!(!rejected.is_replay());
    assert!(rejected.until().is_some_and(|until| until > now.get()));
    assert_eq!(
        restoration_material(&mut tx, document, &command.request_ref).await?,
        before
    );
    let result =
        domain::apply_document_restoration_in(&mut tx, actor, rejected, now, options, || {
            calls.fetch_add(1, Ordering::SeqCst);
            Err::<(), _>("the caller's exact before-restore refusal")
        })
        .await;
    assert!(matches!(
        result,
        Err("the caller's exact before-restore refusal")
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        restoration_material(&mut tx, document, &command.request_ref).await?,
        before
    );
    let allowed =
        domain::prepare_document_restoration_in(&mut tx, actor, &command, now, options).await?;
    let receipt =
        domain::apply_document_restoration_in(&mut tx, actor, allowed, now, options, || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok::<(), &str>(())
        })
        .await??;
    assert_eq!(receipt.entries[0].item, domain::Item::Document(document));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let restored = restoration_material(&mut tx, document, &command.request_ref).await?;
    assert!(!restored.0);
    assert!(restored.2 > before.2);
    assert_eq!(restored.3, before.3 + 1);
    assert_eq!(restored.4, 1);
    tx.commit().await?;
    let mut repeated_tx = area.world.pool.begin().await?;
    let repeated =
        domain::prepare_document_restoration_in(&mut repeated_tx, actor, &command, now, options)
            .await?;
    assert!(repeated.is_replay());
    let replay = domain::apply_document_restoration_in(
        &mut repeated_tx,
        actor,
        repeated,
        now,
        options,
        || {
            calls.fetch_add(1, Ordering::SeqCst);
            Err::<(), _>("a restore replay cannot reach another update")
        },
    )
    .await??;
    assert_eq!(replay.operation, receipt.operation);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        restoration_material(&mut repeated_tx, document, &command.request_ref).await?,
        restored
    );
    repeated_tx.commit().await?;
    Ok(())
}
#[tokio::test]
async fn prepared_document_restore_from_an_ended_transaction_refuses_before_callback_or_material()
-> TestResult {
    let area = Area::new(true).await?;
    let document = area
        .document("ended-restoration.md", "Unchanged retained content")
        .await?;
    let now = Time::new(area.world.clock.now());
    let options = domain::Options::for_engine(&area.world.engine());
    delete_native_text(&area, document, now, options).await?;
    let actor = domain::Actor::Agent { thread: area.agent };
    let command = request(
        domain::Command::Restore,
        vec![domain::Item::Document(document)],
        None,
        "ended-restoration-preparation",
    );
    let mut original = area.world.pool.begin().await?;
    let prepared =
        domain::prepare_document_restoration_in(&mut original, actor, &command, now, options)
            .await?;
    original.rollback().await?;
    let calls = AtomicUsize::new(0);
    let mut other = area.world.pool.begin().await?;
    let before = restoration_material(&mut other, document, &command.request_ref).await?;
    let result =
        domain::apply_document_restoration_in(&mut other, actor, prepared, now, options, || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok::<(), &str>(())
        })
        .await?;
    assert!(matches!(result, Err(domain::Error::Stale)));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        restoration_material(&mut other, document, &command.request_ref).await?,
        before
    );
    other.commit().await?;
    Ok(())
}
