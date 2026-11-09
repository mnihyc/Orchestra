//! Real cut/job/journal controls for the derived PostgreSQL native witness.
use super::*;
use orch_blob_contract::{
    range::ByteRange,
    store::{BlobError, BlobRead, ByteStream, Stored},
};
use orch_db::queries::thread_history_cuts::native as proofs;

struct ProvedSourceStore {
    inner: Arc<LocalBlobStore>,
    refuse: Mutex<Option<Hash>>,
}

#[async_trait]
impl BlobStore for ProvedSourceStore {
    async fn put(
        &self,
        hash: &Hash,
        size: Option<i64>,
        body: ByteStream,
    ) -> Result<Stored, BlobError> {
        self.inner.put(hash, size, body).await
    }
    async fn get(&self, hash: &Hash, range: Option<ByteRange>) -> Result<BlobRead, BlobError> {
        if self.refuse.lock().unwrap().as_ref() == Some(hash) {
            return Err(BlobError::Io(
                "the proved earlier source must not be read again".into(),
            ));
        }
        self.inner.get(hash, range).await
    }
    async fn exists(&self, hash: &Hash) -> Result<bool, BlobError> {
        self.inner.exists(hash).await
    }
    async fn size(&self, hash: &Hash) -> Result<Option<i64>, BlobError> {
        self.inner.size(hash).await
    }
    async fn delete(&self, hash: &Hash) -> Result<bool, BlobError> {
        self.inner.delete(hash).await
    }
}

async fn gated(rig: &Rig, source: Id) -> Arc<SuccessGate> {
    let slot = Arc::new(Mutex::new(None));
    let capture = slot.clone();
    rig.world
        .restart_api_using(move |inner| {
            let gate = Arc::new(SuccessGate {
                inner,
                source,
                saved: Mutex::new(None),
                waiting: std::sync::atomic::AtomicBool::new(false),
                reached: Notify::new(),
                release: Semaphore::new(0),
            });
            *capture.lock().unwrap() = Some(gate.clone());
            gate
        })
        .await;
    rig.world.engine().attach_blobs(rig.store.clone());

    slot.lock().unwrap().take().unwrap()
}

async fn native_success(gate: &SuccessGate) {
    tokio::time::timeout(common::PATIENCE, async {
        loop {
            let notice = gate.reached.notified();
            if gate.waiting.load(Ordering::Acquire) {
                break;
            }
            notice.await;
        }
    })
    .await
    .expect("the actual native child has succeeded before metadata readback");
}

async fn proof(rig: &Rig, cut: Id) -> proofs::Witness {
    proofs::witness(&mut rig.world.pool.acquire().await.unwrap(), cut)
        .await
        .unwrap()
        .unwrap()
}

async fn released(rig: &Rig, cut: Id) {
    let mut conn = rig.world.pool.acquire().await.unwrap();
    assert!(cuts::cut(&mut conn, cut).await.unwrap().is_none());
    assert!(proofs::witness(&mut conn, cut).await.unwrap().is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_streamed_native_prefix_is_kept_in_postgres_and_child_verification_does_not_reread_its_source()
 {
    let rig = Rig::new(
        Can {
            fork_native: true,
            ..Default::default()
        },
        "fake",
        Can::default(),
    )
    .await;
    let source = rig
        .thread_with_history("Durable native source witness")
        .await;
    rig.say(source, "A later turn outside the selected prefix")
        .await;
    rig.world.run_turn(source, 1, "Later source answer").await;
    accounted(&rig, source).await;
    rig.record_native(source).await;
    let gate = gated(&rig, source).await;
    let store = Arc::new(ProvedSourceStore {
        inner: rig.store.clone(),
        refuse: Mutex::new(None),
    });
    rig.world.engine().attach_blobs(store.clone());
    let mut request = rig.fork();
    request.history = Some(ForkHistory {
        mode: HistoryMode::Range,
        from_turn: Some(1),
        to_turn: Some(1),
    });
    let plan = rig.plan(source, &request).await;
    let job = rig.confirm(source, &plan).await;
    native_success(&gate).await;
    let row = proof(&rig, job.id).await;
    let cut = cuts::cut(&mut rig.world.pool.acquire().await.unwrap(), job.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.boundary, cut.artifacts["native_boundary"]);
    assert_eq!(row.boundary["ordinal"], 1);
    assert_eq!(row.boundary["record_last"], 2);
    assert_eq!(row.prefix_digest.len(), 32);
    assert!(row.prefix_entries > 0);
    assert!(row.prefix_entries < row.boundary["evidence"]["entries"].as_i64().unwrap());
    let mut conn = rig.world.pool.acquire().await.unwrap();
    assert_eq!(
        row.lineage,
        proofs::lineage(&mut conn, job.id)
            .await
            .unwrap()
            .as_bytes()
            .as_slice()
    );
    assert_eq!(
        row.source_identity,
        proofs::identity(&mut conn, job.id)
            .await
            .unwrap()
            .as_bytes()
            .as_slice()
    );
    drop(conn);
    *store.refuse.lock().unwrap() =
        Some(serde_json::from_value(row.boundary["record"].clone()).unwrap());
    gate.release.add_permits(1);
    rig.wait_job(job.id, "succeeded").await;
    let target = rig.new_thread_of(&job);
    let children: Vec<_> = rig
        .opens()
        .into_iter()
        .filter(|open| open.thread_id == target && open.mode == SessionMode::Fork)
        .collect();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].upto_turn_ordinal, Some(1));
    released(&rig, job.id).await;
    drop(gate);
    rig.world.restart_api().await;
    rig.world.engine().attach_blobs(store);
    released(&rig, job.id).await;
    rig.world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_published_native_witness_freezes_its_lineage_and_cannot_be_replaced() {
    let rig = Rig::new(
        Can {
            fork_native: true,
            ..Default::default()
        },
        "fake",
        Can::default(),
    )
    .await;
    let source = rig.thread_with_history("Immutable native witness").await;
    accounted(&rig, source).await;
    let gate = gated(&rig, source).await;
    let plan = rig.plan(source, &rig.fork()).await;
    let job = rig.confirm(source, &plan).await;
    native_success(&gate).await;
    let original = proof(&rig, job.id).await;
    for sql in [
        "UPDATE thread_history_native_witnesses SET prefix_entries=prefix_entries+1 WHERE cut_id=$1",
        "UPDATE thread_history_cut_spans SET seq_offset=seq_offset+1 WHERE cut_id=$1",
        "UPDATE thread_history_cut_rows SET ordinal=ordinal+1 WHERE cut_id=$1 AND kind='turn'",
    ] {
        let error = sqlx::query(sql)
            .bind(job.id)
            .execute(rig.world.pool.pool())
            .await
            .expect_err("a published comparison cannot acquire another retained lineage or digest");
        assert!(matches!(
            error
                .as_database_error()
                .and_then(|error| error.constraint()),
            Some("thread_history_native_witness_identity" | "thread_history_native_lineage_fence")
        ));
    }
    let mut altered = original.clone();
    altered.prefix_entries += 1;
    assert!(
        !proofs::keep(&mut rig.world.pool.acquire().await.unwrap(), &altered)
            .await
            .unwrap()
    );
    assert_eq!(proof(&rig, job.id).await, original);
    eventually("the cut's native receipts are confirmed before normal release", || async {
        rig.world.count("SELECT count(*) FROM thread_history_cut_commands WHERE cut_id=$1 AND NOT settled", job.id).await == 0
    }).await;
    let mut tx = rig.world.pool.begin().await.unwrap();
    cuts::release(&mut tx, job.id).await.unwrap();
    assert!(
        proofs::witness(&mut tx, job.id).await.unwrap().is_none(),
        "the existing cut release owns witness retention too"
    );
    tx.rollback().await.unwrap();
    assert_eq!(proof(&rig, job.id).await, original);
    gate.release.add_permits(1);
    rig.wait_job(job.id, "succeeded").await;
    let target = rig.new_thread_of(&job);
    assert_eq!(
        rig.opens()
            .iter()
            .filter(|open| open.thread_id == target && open.mode == SessionMode::Fork)
            .count(),
        1
    );
    released(&rig, job.id).await;
    drop(gate);
    rig.world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_legacy_retained_boundary_is_filled_once_from_its_pinned_source_without_another_native_child()
 {
    let rig = Rig::new(
        Can {
            fork_native: true,
            ..Default::default()
        },
        "fake",
        Can::default(),
    )
    .await;
    let source = rig.thread_with_history("A pre-witness retained cut").await;
    accounted(&rig, source).await;
    let gate = gated(&rig, source).await;
    let plan = rig.plan(source, &rig.fork()).await;
    let job = rig.confirm(source, &plan).await;
    native_success(&gate).await;
    let original = proof(&rig, job.id).await;
    sqlx::raw_sql(
        "CREATE TABLE native_witness_publications (LIKE thread_history_native_witnesses);
         CREATE FUNCTION observe_native_witness_publication() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN INSERT INTO native_witness_publications SELECT NEW.*; RETURN NEW; END; $$;
         CREATE TRIGGER observe_native_witness_publication AFTER INSERT ON thread_history_native_witnesses
         FOR EACH ROW EXECUTE FUNCTION observe_native_witness_publication();",
    )
    .execute(rig.world.pool.pool())
    .await
    .unwrap();
    // Reconstruct the precise earlier schema checkpoint: the actual original
    // boundary artifact and source blob exist, but its derived witness does not.
    sqlx::query("DELETE FROM thread_history_native_witnesses WHERE cut_id=$1")
        .bind(job.id)
        .execute(rig.world.pool.pool())
        .await
        .unwrap();
    gate.release.add_permits(1);
    rig.wait_job(job.id, "succeeded").await;
    let publications = sqlx::query_as::<_, proofs::Witness>(
        "SELECT cut_id,source_identity,lineage,boundary,prefix_digest,prefix_entries
         FROM native_witness_publications",
    )
    .fetch_all(rig.world.pool.pool())
    .await
    .unwrap();
    assert_eq!(publications, vec![original]);
    released(&rig, job.id).await;
    let target = rig.new_thread_of(&job);
    assert_eq!(
        rig.opens()
            .iter()
            .filter(|open| open.thread_id == target && open.mode == SessionMode::Fork)
            .count(),
        1
    );
    drop(gate);
    rig.world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn another_cuts_native_witness_cannot_activate_an_already_successful_child_or_create_a_replacement()
 {
    let rig = Rig::new(
        Can {
            fork_native: true,
            ..Default::default()
        },
        "fake",
        Can::default(),
    )
    .await;
    let source = rig
        .thread_with_history("Witnesses belong to their cut")
        .await;
    accounted(&rig, source).await;
    let first_gate = gated(&rig, source).await;
    let first_plan = rig.plan(source, &rig.fork()).await;
    let first = rig.confirm(source, &first_plan).await;
    native_success(&first_gate).await;
    let original = proof(&rig, first.id).await;
    first_gate.release.add_permits(1);
    rig.wait_job(first.id, "succeeded").await;
    released(&rig, first.id).await;
    drop(first_gate);
    let gate = gated(&rig, source).await;
    let plan = rig.plan(source, &rig.fork()).await;
    let second = rig.confirm(source, &plan).await;
    native_success(&gate).await;
    let own = proof(&rig, second.id).await;
    assert_eq!(
        own.boundary, original.boundary,
        "the same native bytes are not another cut's lineage proof"
    );
    assert_ne!(own.source_identity, original.source_identity);
    // Explicitly reconstruct damaged derived metadata; no controller route can
    // replace a proof or move a lineage member this way.
    sqlx::query("DELETE FROM thread_history_native_witnesses WHERE cut_id=$1")
        .bind(second.id)
        .execute(rig.world.pool.pool())
        .await
        .unwrap();
    let mut borrowed = original;
    borrowed.cut_id = second.id;
    assert!(
        proofs::keep(&mut rig.world.pool.acquire().await.unwrap(), &borrowed)
            .await
            .unwrap()
    );
    gate.release.add_permits(1);
    let ended = rig.wait_job(second.id, "failed").await;
    assert!(ended.plan.get("commit_done").is_none());
    let target = rig.new_thread_of(&second);
    assert_eq!(
        rig.opens()
            .iter()
            .filter(|open| open.thread_id == target && open.mode == SessionMode::Fork)
            .count(),
        1
    );
    assert!(rig.world.turn_starts(target).is_empty());
    let retained = cuts::cut(&mut rig.world.pool.acquire().await.unwrap(), second.id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        retained
            .artifacts
            .as_object()
            .unwrap()
            .keys()
            .any(|name| name.starts_with("native_result_")),
        "the successful original child is retained for its existing cleanup path"
    );
    drop(gate);
    rig.world.shutdown().await;
}
