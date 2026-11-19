//! Scheduled document history preserves live content, recent uploads and every retained file reference.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use chrono::Duration;
use orch_api_contract::documents::{
    BlockContent, BlockProps, BlockType, DocumentOp, FileProps, Inline, NewBlock,
};
use orch_config::ServerConfig;
use orch_db::queries::{
    documents::{self, batch, history},
    hierarchy::testing::insert_node,
    identity::testing::insert_user,
    settings, storage,
};
use orch_db::{Conn, Db};
use orch_documents::versions::Writer;
use orch_testkit::{clock, database::TestDb};
use orch_types::{Clock, Hash, Id, ManualClock, new_id};
use orch_worker_core::{document_purge, document_versions, schedule::Job};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio_util::sync::CancellationToken;
use tracing::instrument::WithSubscriber;

struct World {
    test: TestDb,
    db: Db,
    clock: ManualClock,
    user: Id,
    topic: Id,
    document: Id,
}
impl World {
    async fn new() -> Self {
        let test = TestDb::fresh().await;
        let clock = clock::manual();
        let (user, org, project, topic, document) =
            (new_id(), new_id(), new_id(), new_id(), new_id());
        let mut conn = test.pool.acquire().await.unwrap();
        insert_user(&mut conn, user, "writer").await.unwrap();
        insert_node(&mut conn, org, "org", None, "Acme", user)
            .await
            .unwrap();
        insert_node(&mut conn, project, "project", Some(org), "Work", user)
            .await
            .unwrap();
        insert_node(&mut conn, topic, "topic", Some(project), "Notes", user)
            .await
            .unwrap();
        documents::insert(
            &mut conn,
            &documents::NewDocument {
                id: document,
                node_id: topic,
                org_id: org,
                owner_id: user,
                created_by_thread: None,
                created_by_agent: None,
                title: "Handbook",
                now: clock.now(),
            },
        )
        .await
        .unwrap();
        drop(conn);
        let db = Db::from_pool(test.pool.clone());
        Self {
            test,
            db,
            clock,
            user,
            topic,
            document,
        }
    }
    async fn edit(&self, ops: &[DocumentOp]) -> i64 {
        let mut tx = self.db.begin().await.unwrap();
        let result = batch::apply(
            &mut tx,
            self.document,
            &Writer::User(self.user),
            Some(new_id()),
            ops,
            batch::Limits {
                max_bytes: 2 * 1024 * 1024,
                max_blocks: 5000,
            },
            batch::Rules {
                window_secs: 600,
                kept: 200,
                history_max_bytes: 16 * 1024 * 1024,
            },
            self.clock.now(),
        )
        .await
        .unwrap();
        let batch::Outcome::Applied(done) = result else {
            panic!("the edit must apply: {result:?}");
        };
        tx.commit().await.unwrap();
        done.seq
    }
    async fn file(&self, id: Id, name: &str, hash: Hash) {
        let mut conn = self.test.pool.acquire().await.unwrap();
        storage::register_blob(&mut conn, &hash, 8, self.clock.now())
            .await
            .unwrap();
        storage::pin_blobs(&mut conn, &[hash]).await.unwrap();
        documents::add_file(
            &mut conn,
            &documents::NewFile {
                id,
                document: self.document,
                blob: hash,
                name,
                media_type: "application/octet-stream",
                size: 8,
                is_text: false,
                added_by: Some(self.user),
                added_by_thread: None,
                at: self.clock.now(),
            },
        )
        .await
        .unwrap();
    }
}

fn put(id: Id) -> DocumentOp {
    DocumentOp::Insert {
        block: NewBlock {
            id,
            block_type: BlockType::Paragraph,
            props: None,
            content: Some(BlockContent::Inline(vec![Inline::Text {
                text: "kept words".into(),
                marks: None,
                href: None,
                color: None,
                background: None,
            }])),
            children: None,
        },
        parent_id: None,
        after_id: None,
    }
}

#[tokio::test]
async fn the_scheduled_idle_cut_is_visible_once_and_a_running_edit_restarts_the_idle_time() {
    let world = World::new().await;
    let block = new_id();
    let seq = world.edit(&[put(block)]).await;
    world.clock.advance(Duration::seconds(119));
    assert_eq!(
        document_versions::run(&world.db, &world.clock, &ServerConfig::default())
            .await
            .unwrap(),
        Some(0)
    );
    world
        .edit(&[DocumentOp::Update {
            id: block,
            base_seq: seq,
            block_type: None,
            props: None,
            content: Some(BlockContent::Inline(vec![])),
        }])
        .await;
    world.clock.advance(Duration::seconds(119));
    assert_eq!(
        document_versions::run(&world.db, &world.clock, &ServerConfig::default())
            .await
            .unwrap(),
        Some(0)
    );
    world.clock.advance(Duration::seconds(1));
    assert_eq!(
        document_versions::run(&world.db, &world.clock, &ServerConfig::default())
            .await
            .unwrap(),
        Some(1)
    );
    assert_eq!(
        document_versions::run(&world.db, &world.clock, &ServerConfig::default())
            .await
            .unwrap(),
        Some(0)
    );
    let mut conn = world.test.pool.acquire().await.unwrap();
    let shown = history::list(&mut conn, world.document, None, None, 50)
        .await
        .unwrap();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].cause, "idle");
    assert_eq!(
        history::snapshot(&mut conn, world.document, shown[0].version)
            .await
            .unwrap()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        documents::live_blocks(&mut conn, world.document)
            .await
            .unwrap()
            .len(),
        1
    );
}

async fn pins(conn: &mut Conn, hash: Hash) -> i32 {
    storage::blob(conn, &hash).await.unwrap().unwrap().pin_count
}

#[tokio::test]
async fn daily_purge_keeps_new_uploads_live_and_version_files_and_releases_each_unused_pin_once() {
    let world = World::new().await;
    let hash = Hash::from_bytes([31; 32]);
    let (live, history_file, unused, new_upload) = (new_id(), new_id(), new_id(), new_id());
    for (id, name) in [
        (live, "live.bin"),
        (history_file, "history.bin"),
        (unused, "unused.bin"),
    ] {
        world.file(id, name, hash).await;
    }
    let (live_block, old_block) = (new_id(), new_id());
    let file_block = |id, file| DocumentOp::Insert {
        block: NewBlock {
            id,
            block_type: BlockType::File,
            props: Some(BlockProps::File(FileProps {
                display: None,
                file_id: file,
            })),
            content: None,
            children: None,
        },
        parent_id: None,
        after_id: None,
    };
    world
        .edit(&[
            file_block(live_block, live),
            file_block(old_block, history_file),
        ])
        .await;
    world.clock.advance(Duration::seconds(120));
    document_versions::run(&world.db, &world.clock, &ServerConfig::default())
        .await
        .unwrap();
    world.edit(&[DocumentOp::Remove { id: old_block }]).await;
    world.clock.advance(Duration::days(31));
    world.file(new_upload, "just-uploaded.bin", hash).await;
    let done = document_purge::run(
        &world.db,
        &world.clock,
        &ServerConfig::default(),
        &CancellationToken::new(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(done.files, 1, "only the old unused association can go");
    let mut conn = world.test.pool.acquire().await.unwrap();
    assert!(
        documents::file(&mut conn, world.document, unused)
            .await
            .unwrap()
            .is_none()
    );
    for id in [live, history_file, new_upload] {
        assert!(
            documents::file(&mut conn, world.document, id)
                .await
                .unwrap()
                .is_some()
        );
    }
    assert_eq!(pins(&mut conn, hash).await, 3);
    assert_eq!(
        documents::live_blocks(&mut conn, world.document)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        history::snapshot(&mut conn, world.document, 1)
            .await
            .unwrap()
            .unwrap()
            .len(),
        2,
        "the removed file is recoverable from the kept version"
    );
    drop(conn);
    let again = document_purge::run(
        &world.db,
        &world.clock,
        &ServerConfig::default(),
        &CancellationToken::new(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(again.files, 0);
    // Once its final retained version goes, the removed file releases exactly its own pin.
    document_versions::run(&world.db, &world.clock, &ServerConfig::default())
        .await
        .unwrap();
    let mut conn = world.test.pool.acquire().await.unwrap();
    let one = serde_json::json!(1);
    settings::put(
        &mut conn,
        Some(world.topic),
        "docs.versions_kept",
        &settings::Stored {
            value: &one,
            locked: false,
            by: Some(world.user),
            at: world.clock.now(),
        },
        None,
    )
    .await
    .unwrap();
    drop(conn);
    assert_eq!(
        document_purge::run(
            &world.db,
            &world.clock,
            &ServerConfig::default(),
            &CancellationToken::new()
        )
        .await
        .unwrap()
        .unwrap()
        .files,
        1
    );
    let mut conn = world.test.pool.acquire().await.unwrap();
    assert!(
        documents::file(&mut conn, world.document, history_file)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(pins(&mut conn, hash).await, 2);
    let shown = history::list(&mut conn, world.document, None, None, 50)
        .await
        .unwrap();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].version, 2);
    assert_eq!(
        history::snapshot(&mut conn, world.document, 2)
            .await
            .unwrap()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn a_second_scheduler_cannot_cut_or_purge_while_the_same_job_is_running() {
    let world = World::new().await;
    world.edit(&[put(new_id())]).await;
    world.clock.advance(Duration::minutes(3));
    // Either job holds a lease, which the API's run of the same job takes too, not a transaction held open.
    let versions = orch_db::queries::job_leases::Lease::new(
        Job::DocumentVersions.key(),
        "the API",
        Duration::minutes(10),
    );
    assert!(versions.hold(&world.db, world.clock.now()).await.unwrap());
    assert_eq!(
        document_versions::run(&world.db, &world.clock, &ServerConfig::default())
            .await
            .unwrap(),
        None
    );
    versions.release(&world.db).await.unwrap();
    assert_eq!(
        document_versions::run(&world.db, &world.clock, &ServerConfig::default())
            .await
            .unwrap(),
        Some(1)
    );
    let other = orch_db::queries::job_leases::Lease::new(
        Job::DocumentPurge.key(),
        "another worker",
        Duration::minutes(10),
    );
    assert!(other.hold(&world.db, world.clock.now()).await.unwrap());
    assert_eq!(
        document_purge::run(
            &world.db,
            &world.clock,
            &ServerConfig::default(),
            &CancellationToken::new()
        )
        .await
        .unwrap(),
        None
    );
    other.release(&world.db).await.unwrap();
    assert!(
        document_purge::run(
            &world.db,
            &world.clock,
            &ServerConfig::default(),
            &CancellationToken::new()
        )
        .await
        .unwrap()
        .is_some()
    );
}

#[derive(Default)]
struct PurgeCounts {
    settings: AtomicUsize,
    pages: AtomicUsize,
}

#[derive(Clone, Default)]
struct PurgeReads(Arc<PurgeCounts>);

impl tracing::Subscriber for PurgeReads {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.target() == "sqlx::query"
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        if event.metadata().target() != "sqlx::query" {
            return;
        }
        #[derive(Default)]
        struct Sql(String);
        impl tracing::field::Visit for Sql {
            fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                if matches!(field.name(), "sql" | "db.statement") {
                    self.0.push_str(value);
                }
            }
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                if matches!(field.name(), "sql" | "db.statement") {
                    self.0.push_str(&format!("{value:?}"));
                }
            }
        }
        let mut sql = Sql::default();
        event.record(&mut sql);
        let sql: String = sql.0.split_whitespace().collect();
        if sql.contains("FROMsettings") {
            self.0.settings.fetch_add(1, Ordering::Relaxed);
        }
        if sql.contains("SELECTid,node_idFROMdocuments") {
            self.0.pages.fetch_add(1, Ordering::Relaxed);
        }
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

async fn maintenance_documents(world: &World, total: i64) {
    // Keep native namespace/scope triggers while bounding repeated scope-row versions
    // within one setup transaction, as ordinary document creation does.
    const SETUP_ROWS: i64 = 128;
    let mut first = 1;
    while first < total {
        let last = (first + SETUP_ROWS - 1).min(total - 1);
        let mut tx = world.test.pool.begin().await.unwrap();
        let added = sqlx::query(
            "WITH added AS (
                 INSERT INTO documents(id,node_id,org_id,owner_id,title,created_by,created_at,updated_at)
                 SELECT gen_random_uuid(),d.node_id,d.org_id,d.owner_id,'Maintenance '||g,d.owner_id,$4,$4
                   FROM documents d CROSS JOIN generate_series($2::bigint,$3::bigint) g WHERE d.id=$1
                 RETURNING id
             )
             INSERT INTO document_ops(document_id,author_id,client_id,seq,created_at)
             SELECT id,$5,gen_random_uuid(),0,$6 FROM added",
        ).bind(world.document).bind(first).bind(last).bind(world.clock.now()).bind(world.user)
            .bind(world.clock.now() - Duration::days(2)).execute(&mut *tx).await.unwrap().rows_affected();
        assert_eq!(added, u64::try_from(last - first + 1).unwrap());
        tx.commit().await.unwrap();
        first = last + 1;
    }
    sqlx::query("INSERT INTO document_ops(document_id,author_id,client_id,seq,created_at) VALUES($1,$2,$3,0,$4)")
        .bind(world.document).bind(world.user).bind(new_id()).bind(world.clock.now()-Duration::days(2))
        .execute(&world.test.pool).await.unwrap();
    let actual: i64 = sqlx::query_scalar("SELECT count(*) FROM documents")
        .fetch_one(&world.test.pool)
        .await
        .unwrap();
    assert_eq!(actual, total);
}

#[tokio::test]
async fn a_page_of_documents_with_one_home_loads_its_settings_once() {
    let world = World::new().await;
    maintenance_documents(&world, 200).await;
    let reads = PurgeReads::default();
    let done = document_purge::run(
        &world.db,
        &world.clock,
        &ServerConfig::default(),
        &CancellationToken::new(),
    )
    .with_subscriber(reads.clone())
    .await
    .unwrap()
    .unwrap();
    assert_eq!(done.batches, 200);
    assert_eq!(
        reads.0.settings.load(Ordering::Relaxed),
        1,
        "one real settings read for the shared home"
    );
    assert_eq!(
        reads.0.pages.load(Ordering::Relaxed),
        2,
        "one full page and its empty continuation"
    );
}

#[tokio::test]
async fn cancelling_a_hundred_thousand_document_purge_finishes_only_its_current_page() {
    let world = World::new().await;
    maintenance_documents(&world, 100_000).await;
    let file = new_id();
    let hash = Hash::from_bytes([51; 32]);
    world.file(file, "not-yet-collected.bin", hash).await;
    world.clock.advance(Duration::days(2));
    let mut conn = world.test.pool.acquire().await.unwrap();
    sqlx::raw_sql(
        "CREATE TABLE observed_purge_pages (n integer NOT NULL);
         CREATE FUNCTION observe_purge_page() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN
             PERFORM pg_advisory_xact_lock(87123);
             INSERT INTO observed_purge_pages VALUES(1);
             RETURN NULL;
         END; $$;
         CREATE TRIGGER observe_purge_page AFTER DELETE ON document_ops
             FOR EACH STATEMENT EXECUTE FUNCTION observe_purge_page()",
    )
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);
    let mut blocked = world.test.pool.begin().await.unwrap();
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocked)
        .await
        .unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(87123)")
        .execute(&mut *blocked)
        .await
        .unwrap();
    let stop = CancellationToken::new();
    let request_stop = stop.clone();
    let db = world.db.clone();
    let clock = world.clock.clone();
    let reads = PurgeReads::default();
    let observed = reads.clone();
    let running = tokio::spawn(async move {
        document_purge::run(&db, &clock, &ServerConfig::default(), &request_stop)
            .with_subscriber(observed)
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                    AND pg_blocking_pids(pid) @> ARRAY[$1]::integer[])",
            )
            .bind(blocker)
            .fetch_one(&world.test.pool)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the first page reaches its actual delete before cancellation");
    stop.cancel();
    blocked.commit().await.unwrap();
    let done = tokio::time::timeout(std::time::Duration::from_secs(10), running)
        .await
        .expect("a cancelled pass finishes its current page")
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(done.batches, 200, "the current page commits completely");
    assert_eq!(
        done.files, 0,
        "cancellation cannot start the later file sweep"
    );
    assert_eq!(
        reads.0.pages.load(Ordering::Relaxed),
        1,
        "no second candidate page is read"
    );
    assert_eq!(reads.0.settings.load(Ordering::Relaxed), 1);
    let pages: i64 = sqlx::query_scalar("SELECT count(*) FROM observed_purge_pages")
        .fetch_one(&world.test.pool)
        .await
        .unwrap();
    assert_eq!(pages, 1);
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM document_ops")
        .fetch_one(&world.test.pool)
        .await
        .unwrap();
    assert_eq!(left, 99_800, "later pages are left for the next run");
    let mut conn = world.test.pool.acquire().await.unwrap();
    assert!(
        documents::file(&mut conn, world.document, file)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(pins(&mut conn, hash).await, 1);
    drop(conn);
    let next = orch_db::queries::job_leases::Lease::new(
        Job::DocumentPurge.key(),
        "next scheduler",
        Duration::minutes(10),
    );
    assert!(
        next.hold(&world.db, world.clock.now()).await.unwrap(),
        "shutdown releases the run lease"
    );
    next.release(&world.db).await.unwrap();
}
