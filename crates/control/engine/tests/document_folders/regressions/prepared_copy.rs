//! Real native copy packets stop before material, bind their actor/transaction and replay once.

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

async fn principal(area: &Area, conn: &mut orch_db::Conn) -> TestResult<orch_authz::Principal> {
    Ok(orch_authz::Principal::load(
        conn,
        orch_authz::Caller::session(area.world.ada),
        Time::new(area.world.clock.now()),
    )
    .await?)
}

type CopyState = (i64, i64, i64, i64, i64, i64, i64, i64, i64);
async fn material(tx: &mut orch_db::Tx, id: Id, key: &str, blob: Hash) -> TestResult<CopyState> {
    Ok(sqlx::query_as("SELECT
        (SELECT count(*) FROM documents WHERE id=$1),
        (SELECT count(*) FROM document_blocks WHERE document_id=$1),
        (SELECT count(*) FROM document_files WHERE document_id=$1),
        (SELECT count(*) FROM document_versions WHERE document_id=$1),
        (SELECT count(*) FROM audit_log WHERE object_kind='document' AND object_id=$1),
        (SELECT count(*) FROM document_tree_operations WHERE request_ref=$2 OR request_ref='text-create:' || $1::text),
        (SELECT count(*) FROM permission_rows WHERE object_kind='document' AND object_id=$1),
        (SELECT count(*) FROM permission_entries WHERE object_kind='document' AND object_id=$1),
        (SELECT pin_count::bigint FROM blobs WHERE hash=$3)")
        .bind(id).bind(key).bind(blob.as_bytes().as_slice()).fetch_one(&mut **tx).await?)
}

fn command(area: &Area, source: Id, label: &str) -> (domain::Request, domain::TextCopy) {
    (
        request(
            domain::Command::Copy,
            vec![domain::Item::Document(source)],
            Some(domain::Directory::NodeDocs(area.topic)),
            label,
        ),
        domain::TextCopy {
            id: new_id(),
            title: Some("Prepared native text copy".into()),
            private: true,
        },
    )
}

#[tokio::test]
async fn prepared_copy_cut_precedes_header_files_history_and_replay_never_repeats_it() -> TestResult
{
    let area = Area::new(true).await?;
    let source = area.document("source.md", "Current source text").await?;
    let attached = attach(&area.world, source).await?;
    let (request, copy) = command(&area, source, "prepared-copy-once");
    let now = Time::new(area.world.clock.now());
    let options = domain::Options::for_engine(&area.world.engine());
    let calls = AtomicUsize::new(0);
    let mut tx = area.world.pool.begin().await?;
    let principal = principal(&area, &mut tx).await?;
    let actor = domain::Actor::Person(&principal);
    let before = material(&mut tx, copy.id, &request.request_ref, attached.hash).await?;
    let subjects =
        domain::prepare_document_copy_sources_in(&mut tx, actor, &request, &copy).await?;
    assert_eq!(subjects.destination(), request.to);
    assert_eq!(subjects.sources().len(), 1);
    assert_eq!(subjects.sources()[0].id, source);
    assert!(!subjects.is_replay());
    assert_eq!(
        material(&mut tx, copy.id, &request.request_ref, attached.hash).await?,
        before
    );
    let rejected =
        domain::prepare_document_copy_material_in(&mut tx, actor, subjects, now, options).await?;
    assert_eq!(rejected.id(), copy.id);
    assert_eq!(rejected.destination(), request.to);
    assert_eq!(rejected.sources().len(), 1);
    assert_eq!(rejected.sources()[0].id, source);
    assert!(!rejected.is_replay());
    assert_eq!(
        material(&mut tx, copy.id, &request.request_ref, attached.hash).await?,
        before
    );
    domain::require_document_copy_in(&mut tx, actor, &rejected, now).await?;
    let result = domain::apply_document_copy_in(&mut tx, actor, rejected, now, options, || {
        calls.fetch_add(1, Ordering::SeqCst);
        Err::<(), _>("the exact native copy cut refused")
    })
    .await;
    assert!(matches!(result, Err("the exact native copy cut refused")));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        material(&mut tx, copy.id, &request.request_ref, attached.hash).await?,
        before
    );
    let ready =
        domain::prepare_document_copy_in(&mut tx, actor, &request, &copy, now, options).await?;
    let receipt = domain::apply_document_copy_in(&mut tx, actor, ready, now, options, || {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok::<(), &str>(())
    })
    .await??;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(receipt.entries.len(), 1);
    assert_eq!(receipt.entries[0].item, domain::Item::Document(copy.id));
    assert_eq!(receipt.copied[0].source, domain::Item::Document(source));
    assert_eq!(
        receipt.copied[0].destination,
        domain::Item::Document(copy.id)
    );
    let copied_file: (Id, Vec<u8>) =
        sqlx::query_as("SELECT id,blob FROM document_files WHERE document_id=$1")
            .bind(copy.id)
            .fetch_one(&mut *tx)
            .await?;
    assert_ne!(copied_file.0, attached.file);
    assert_eq!(
        copied_file.1.as_slice(),
        attached.hash.as_bytes().as_slice()
    );
    let complete = material(&mut tx, copy.id, &request.request_ref, attached.hash).await?;
    assert_eq!(complete.0, 1);
    assert_eq!(complete.2, 1);
    assert!(complete.1 > 0);
    assert_eq!(
        complete.3, 0,
        "initial copied blocks are an open history span, before any version cut"
    );
    let copied = orch_db::queries::documents::get(&mut tx, copy.id)
        .await?
        .ok_or("the copied document is absent")?;
    assert!(!copied.span().is_empty());
    assert_eq!(
        copied.span().authors,
        vec![orch_documents::versions::Writer::User(area.world.ada)]
    );
    assert!(complete.4 > 0);
    assert_eq!(complete.8, before.8 + 1);
    tx.commit().await?;
    let mut tx = area.world.pool.begin().await?;
    let principal = self::principal(&area, &mut tx).await?;
    let actor = domain::Actor::Person(&principal);
    let replay =
        domain::prepare_document_copy_in(&mut tx, actor, &request, &copy, now, options).await?;
    assert!(replay.is_replay());
    assert!(replay.sources().is_empty());
    assert_eq!(replay.replay_items(), &[domain::Item::Document(copy.id)]);
    domain::require_document_copy_in(&mut tx, actor, &replay, now).await?;
    let repeated = domain::apply_document_copy_in(&mut tx, actor, replay, now, options, || {
        calls.fetch_add(1, Ordering::SeqCst);
        Err::<(), _>("replay reached a new copy effect")
    })
    .await??;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(repeated.operation, receipt.operation);
    assert_eq!(
        material(&mut tx, copy.id, &request.request_ref, attached.hash).await?,
        complete
    );
    tx.commit().await?;
    Ok(())
}

#[tokio::test]
async fn prepared_copy_rejects_another_actor_and_transaction_before_material_or_the_cut()
-> TestResult {
    let area = Area::new(true).await?;
    let source = area
        .document("source.md", "Native copy actor source")
        .await?;
    let attached = attach(&area.world, source).await?;
    let (request, copy) = command(&area, source, "prepared-copy-association");
    let now = Time::new(area.world.clock.now());
    let options = domain::Options::for_engine(&area.world.engine());
    let calls = AtomicUsize::new(0);
    let mut tx = area.world.pool.begin().await?;
    let principal = principal(&area, &mut tx).await?;
    let actor = domain::Actor::Person(&principal);
    let before = material(&mut tx, copy.id, &request.request_ref, attached.hash).await?;
    let prepared =
        domain::prepare_document_copy_in(&mut tx, actor, &request, &copy, now, options).await?;
    let wrong_actor = domain::apply_document_copy_in(
        &mut tx,
        domain::Actor::Agent { thread: area.agent },
        prepared,
        now,
        options,
        || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok::<(), &str>(())
        },
    )
    .await?;
    assert!(matches!(wrong_actor, Err(domain::Error::Denied)));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        material(&mut tx, copy.id, &request.request_ref, attached.hash).await?,
        before
    );
    let prepared =
        domain::prepare_document_copy_in(&mut tx, actor, &request, &copy, now, options).await?;
    tx.rollback().await?;
    let mut tx = area.world.pool.begin().await?;
    let wrong_transaction =
        domain::apply_document_copy_in(&mut tx, actor, prepared, now, options, || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok::<(), &str>(())
        })
        .await?;
    assert!(matches!(wrong_transaction, Err(domain::Error::Stale)));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        material(&mut tx, copy.id, &request.request_ref, attached.hash).await?,
        before
    );
    tx.commit().await?;
    Ok(())
}

#[tokio::test]
async fn prepared_copy_late_native_file_failure_rolls_back_header_authority_and_pins() -> TestResult
{
    let area = Area::new(true).await?;
    let source = area
        .document("source.md", "Native copy rollback source")
        .await?;
    let attached = attach(&area.world, source).await?;
    let (request, copy) = command(&area, source, "prepared-copy-late-file");
    let now = Time::new(area.world.clock.now());
    let options = domain::Options::for_engine(&area.world.engine());
    let calls = AtomicUsize::new(0);
    let mut tx = area.world.pool.begin().await?;
    sqlx::raw_sql("CREATE FUNCTION refuse_prepared_copy_file() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.document_id::text=current_setting('orch.test.copy_destination',true) THEN RAISE EXCEPTION 'native copied file refused'; END IF; RETURN NEW; END; $$; CREATE TRIGGER refuse_prepared_copy_file BEFORE INSERT ON document_files FOR EACH ROW EXECUTE FUNCTION refuse_prepared_copy_file();").execute(&mut *tx).await?;
    sqlx::query("SELECT set_config('orch.test.copy_destination',$1,true)")
        .bind(copy.id.to_string())
        .execute(&mut *tx)
        .await?;
    let principal = principal(&area, &mut tx).await?;
    let actor = domain::Actor::Person(&principal);
    let before = material(&mut tx, copy.id, &request.request_ref, attached.hash).await?;
    let prepared =
        domain::prepare_document_copy_in(&mut tx, actor, &request, &copy, now, options).await?;
    let result = domain::apply_document_copy_in(&mut tx, actor, prepared, now, options, || {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok::<(), &str>(())
    })
    .await?;
    assert!(result.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        material(&mut tx, copy.id, &request.request_ref, attached.hash).await?,
        before
    );
    tx.commit().await?;
    Ok(())
}

#[tokio::test]
async fn prepared_copy_subjects_cannot_cross_a_transaction_or_upgrade_provisional_input()
-> TestResult {
    let area = Area::new(true).await?;
    let source = area
        .document("source.md", "Native copy source footprint")
        .await?;
    let attached = attach(&area.world, source).await?;
    let (request, copy) = command(&area, source, "prepared-copy-subject-footprint");
    let now = Time::new(area.world.clock.now());
    let options = domain::Options::for_engine(&area.world.engine());
    let mut tx = area.world.pool.begin().await?;
    let principal = principal(&area, &mut tx).await?;
    let actor = domain::Actor::Person(&principal);
    let before = material(&mut tx, copy.id, &request.request_ref, attached.hash).await?;
    let subjects =
        domain::prepare_document_copy_sources_in(&mut tx, actor, &request, &copy).await?;
    tx.rollback().await?;
    let mut tx = area.world.pool.begin().await?;
    assert!(matches!(
        domain::prepare_document_copy_material_in(&mut tx, actor, subjects, now, options).await,
        Err(domain::Error::Stale)
    ));
    assert_eq!(
        material(&mut tx, copy.id, &request.request_ref, attached.hash).await?,
        before
    );
    tx.commit().await?;
    let mut conn = area.world.pool.acquire().await?;
    let subjects =
        domain::prepare_document_copy_sources_in(&mut conn, actor, &request, &copy).await?;
    drop(conn);
    let mut tx = area.world.pool.begin().await?;
    assert!(matches!(
        domain::prepare_document_copy_material_in(&mut tx, actor, subjects, now, options).await,
        Err(domain::Error::Stale)
    ));
    assert_eq!(
        material(&mut tx, copy.id, &request.request_ref, attached.hash).await?,
        before
    );
    tx.commit().await?;
    Ok(())
}
