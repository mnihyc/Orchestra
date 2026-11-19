//! Native database regressions for archive, saved promises, durable replay and atomic tree acts.

use super::*;
use orch_engine::document_tree as domain;
use orch_types::{Hash, Time};

#[path = "regressions/archive_retention.rs"]
mod archive_retention;
#[path = "regressions/contention.rs"]
mod contention;
#[path = "regressions/replay_rollback.rs"]
mod replay_rollback;

fn request(
    command: domain::Command,
    items: Vec<domain::Item>,
    to: Option<domain::Directory>,
    key: &str,
) -> domain::Request {
    domain::Request {
        command,
        items,
        to,
        name: None,
        request_ref: key.to_owned(),
        version: None,
    }
}

/// Fingerprint every persisted material/authority/receipt field, without dumping whole document
/// bodies in an assertion diagnostic. Only real document-area node sequence stamps are included.
async fn state(world: &World, nodes: &[Id]) -> TestResult<Hash> {
    let value:Json=sqlx::query_scalar("SELECT jsonb_build_object(
        'folders',COALESCE((SELECT jsonb_agg(to_jsonb(c) ORDER BY c.id) FROM document_containers c),'[]'::jsonb),
        'documents',COALESCE((SELECT jsonb_agg(to_jsonb(d) ORDER BY d.id) FROM documents d),'[]'::jsonb),
        'blocks',COALESCE((SELECT jsonb_agg(to_jsonb(b) ORDER BY b.id) FROM document_blocks b),'[]'::jsonb),
        'files',COALESCE((SELECT jsonb_agg(to_jsonb(f) ORDER BY f.id) FROM document_files f),'[]'::jsonb),
        'versions',COALESCE((SELECT jsonb_agg(to_jsonb(v) ORDER BY v.document_id,v.version) FROM document_versions v),'[]'::jsonb),
        'discussions',COALESCE((SELECT jsonb_agg(to_jsonb(d) ORDER BY d.id) FROM document_discussions d),'[]'::jsonb),
        'comments',COALESCE((SELECT jsonb_agg(to_jsonb(c) ORDER BY c.id) FROM document_comments c),'[]'::jsonb),
        'batches',COALESCE((SELECT jsonb_agg(to_jsonb(o) ORDER BY o.document_id,o.author_id,o.client_id) FROM document_ops o),'[]'::jsonb),
        'pages',COALESCE((SELECT jsonb_agg(to_jsonb(r) ORDER BY r.object_kind,r.object_id,r.page,r.profile,r.permission)
            FROM permission_rows r WHERE object_kind IN ('document','document_container')),'[]'::jsonb),
        'grants',COALESCE((SELECT jsonb_agg(to_jsonb(e) ORDER BY e.id) FROM permission_entries e
            WHERE object_kind IN ('document','document_container')),'[]'::jsonb),
        'audit',COALESCE((SELECT jsonb_agg(to_jsonb(a) ORDER BY a.at,a.id) FROM audit_log a
            WHERE object_kind IN ('document','document_container')),'[]'::jsonb),
        'receipts',COALESCE((SELECT jsonb_agg(to_jsonb(o) ORDER BY o.actor_kind,o.actor_id,o.request_ref)
            FROM document_tree_operations o),'[]'::jsonb),
        'scopes',COALESCE((SELECT jsonb_agg(to_jsonb(s) ORDER BY s.node_id) FROM document_tree_scopes s),'[]'::jsonb),
        'nodes',COALESCE((SELECT jsonb_agg(jsonb_build_array(n.id,n.tree_seq) ORDER BY n.id) FROM nodes n WHERE n.id=ANY($1)),'[]'::jsonb),
        'blobs',COALESCE((SELECT jsonb_agg(to_jsonb(b) ORDER BY b.hash) FROM blobs b),'[]'::jsonb))")
        .bind(nodes).fetch_one(world.pool.pool()).await?;
    Ok(Hash::of(&serde_json::to_vec(&value)?))
}

struct Attached {
    _directory: tempfile::TempDir,
    store: std::sync::Arc<orch_blob_local::LocalBlobStore>,
    file: Id,
    hash: Hash,
}

/// A real stored binary and native file row. No external store wait occurs in the transaction.
async fn attach(world: &World, document: Id) -> TestResult<Attached> {
    use orch_blob_contract::store::{BlobStore, stream_of};
    let directory = tempfile::tempdir()?;
    let store = std::sync::Arc::new(
        orch_blob_local::LocalBlobStore::open(directory.path().join("content")).await?,
    );
    let bytes = bytes::Bytes::from_static(b"\0\xffretained document-copy bytes\r\n");
    let hash = Hash::of(&bytes);
    let size = i64::try_from(bytes.len())?;
    let file = new_id();
    store.put(&hash, Some(size), stream_of(vec![bytes])).await?;
    world.engine().attach_blobs(store.clone());
    let mut tx = world.pool.begin().await?;
    documents::lock(&mut tx, document)
        .await?
        .ok_or("source document absent")?;
    orch_db::queries::storage::register_blob(&mut tx, &hash, size, world.clock.now()).await?;
    assert_eq!(
        orch_db::queries::storage::pin_blobs(&mut tx, &[hash]).await?,
        1
    );
    documents::add_file(
        &mut tx,
        &documents::NewFile {
            id: file,
            document,
            blob: hash,
            name: "report.bin",
            media_type: "application/octet-stream",
            size,
            is_text: false,
            added_by: Some(world.ada),
            added_by_thread: None,
            at: world.clock.now(),
        },
    )
    .await?;
    let block = orch_api_contract::documents::NewBlock {
        id: new_id(),
        block_type: orch_api_contract::documents::BlockType::File,
        props: Some(orch_api_contract::documents::BlockProps::File(
            orch_api_contract::documents::FileProps {
                display: None,
                file_id: file,
            },
        )),
        content: None,
        children: None,
    };
    let outcome = documents::batch::apply(
        &mut tx,
        document,
        &orch_documents::versions::Writer::User(world.ada),
        Some(new_id()),
        &[DocumentOp::Insert {
            block,
            parent_id: None,
            after_id: None,
        }],
        documents::batch::Limits {
            max_bytes: 2_097_152,
            max_blocks: 5_000,
        },
        documents::batch::Rules {
            window_secs: 600,
            kept: 200,
            history_max_bytes: 16_777_216,
        },
        world.clock.now(),
    )
    .await?;
    assert!(
        outcome.is_kept(),
        "the native file block must be valid before it is copied"
    );
    documents::touch(&mut tx, document).await?;
    tx.commit().await?;
    Ok(Attached {
        _directory: directory,
        store,
        file,
        hash,
    })
}

async fn pins(world: &World, hash: Hash) -> TestResult<i32> {
    Ok(
        sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash=$1")
            .bind(hash.as_bytes().as_slice())
            .fetch_one(world.pool.pool())
            .await?,
    )
}

async fn remove(
    world: &World,
    actor: Id,
    item: domain::Item,
    key: &str,
) -> TestResult<domain::Receipt> {
    let request = request(domain::Command::Delete, vec![item], None, key);
    let mut tx = world.pool.begin().await?;
    let now = Time::new(world.clock.now());
    let options = domain::Options::for_engine(&world.engine());
    let actor = domain::Actor::Agent { thread: actor };
    let plan = domain::plan_delete_in(&mut tx, actor, &request, now, options).await?;
    let receipt =
        domain::execute_in(&mut tx, actor, &request, Some(&plan.stamp), now, options).await?;
    tx.commit().await?;
    Ok(receipt)
}
