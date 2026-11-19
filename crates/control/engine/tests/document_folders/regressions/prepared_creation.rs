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
    let rejected =
        domain::prepare_folder_creation_in(&mut tx, actor, &command, now, options).await?;
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
    let allowed =
        domain::prepare_folder_creation_in(&mut tx, actor, &command, now, options).await?;
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
    let replay =
        domain::prepare_folder_creation_in(&mut replay_tx, actor, &command, now, options).await?;
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
    let prepared =
        domain::prepare_folder_creation_in(&mut original, actor, &command, now, options).await?;
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
