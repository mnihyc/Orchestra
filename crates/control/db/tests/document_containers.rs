//! Native typed parent/namespace constraints and complete candidate continuation reads.

use orch_db::{
    Conn, Db, DbError,
    queries::{
        documents::{self, containers},
        hierarchy, resources,
    },
};
use orch_testkit::{clock, database::TestDb};
use orch_types::{Id, new_id};
use std::time::Instant;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct Area {
    test: TestDb,
    db: Db,
    user: Id,
    root: Id,
    node: Id,
}

impl Area {
    async fn new() -> TestResult<Self> {
        let started = Instant::now();
        let test = TestDb::fresh().await;
        let inherited_jit: String = sqlx::query_scalar("SELECT current_setting('jit')")
            .fetch_one(&test.pool)
            .await?;
        // These sequential metadata tests need one connection. Use the deployed startup options,
        // including jit=off, rather than silently adopting the fixture server's pool defaults.
        let db = Db::connect(&test.url, 1).await?;
        let mut conn = db.acquire().await?;
        let jit: String = sqlx::query_scalar("SELECT current_setting('jit')")
            .fetch_one(&mut *conn)
            .await?;
        assert_eq!(jit, "off");
        let user = resources::testing::insert_user(&mut conn, "ada").await?;
        let (root, node) = (new_id(), new_id());
        hierarchy::testing::insert_node(&mut conn, root, "org", None, "Office", user).await?;
        hierarchy::testing::insert_node(&mut conn, node, "project", Some(root), "Research", user)
            .await?;
        drop(conn);
        eprintln!(
            "document-container fixture bootstrap {:?}; inherited jit={inherited_jit}; deployed jit={jit}",
            started.elapsed()
        );
        Ok(Self {
            test,
            db,
            user,
            root,
            node,
        })
    }

    async fn folder(
        &self,
        conn: &mut Conn,
        id: Id,
        parent: Option<Id>,
        name: &str,
    ) -> Result<(), DbError> {
        containers::insert(
            conn,
            &containers::NewContainer {
                id,
                node: self.node,
                root: self.root,
                parent,
                owner: self.user,
                thread: None,
                agent: None,
                name,
                now: clock::start(),
            },
        )
        .await
    }

    async fn close(self) -> TestResult {
        let started = Instant::now();
        self.db.pool().close().await;
        self.test.close().await?;
        eprintln!("document-container fixture cleanup {:?}", started.elapsed());
        Ok(())
    }
}

#[tokio::test]
async fn raw_structural_writers_cannot_bypass_cross_kind_names_or_put_a_folder_below_itself()
-> TestResult {
    let area = Area::new().await?;
    let (parent, child, document) = (new_id(), new_id(), new_id());
    let mut tx = area.db.begin().await?;
    area.folder(&mut tx, parent, None, "Research").await?;
    area.folder(&mut tx, child, Some(parent), "Notes").await?;
    containers::insert_document(
        &mut tx,
        &documents::NewDocument {
            id: document,
            node_id: area.node,
            org_id: area.root,
            owner_id: area.user,
            created_by_thread: None,
            created_by_agent: None,
            title: "Plan",
            now: clock::start(),
        },
        Some(child),
        "Plan.md",
    )
    .await?;
    tx.commit().await?;
    let mut tx = area.db.begin().await?;
    let collision = area.folder(&mut tx, new_id(), Some(child), "PLAN.md").await;
    assert!(
        matches!(collision, Err(DbError::Unique { .. })),
        "both native kinds share the resolver's ASCII-folded namespace"
    );
    tx.rollback().await?;
    let mut tx = area.db.begin().await?;
    let cycle = sqlx::query("UPDATE document_containers SET parent_container_id=$2 WHERE id=$1")
        .bind(parent)
        .bind(child)
        .execute(&mut *tx)
        .await;
    assert!(cycle.is_err());
    tx.rollback().await?;
    let mut tx = area.db.begin().await?;
    let wrong_type = sqlx::query("UPDATE documents SET parent_container_id=$1 WHERE id=$1")
        .bind(document)
        .execute(&mut *tx)
        .await;
    assert!(
        wrong_type.is_err(),
        "a text document can never become a directory"
    );
    tx.rollback().await?;
    let mut conn = area.db.acquire().await?;
    let row = containers::entry(&mut conn, "document", document)
        .await?
        .ok_or("document missing")?;
    assert_eq!(row.parent_container_id, Some(child));
    drop(conn);
    area.close().await?;
    Ok(())
}

#[tokio::test]
async fn candidate_continuations_follow_every_page_and_folder_seeds_do_not_add_siblings()
-> TestResult {
    let area = Area::new().await?;
    let (parent, child, sibling) = (new_id(), new_id(), new_id());
    let mut tx = area.db.begin().await?;
    area.folder(&mut tx, parent, None, "Research").await?;
    area.folder(&mut tx, child, Some(parent), "Nested").await?;
    area.folder(&mut tx, sibling, None, "Unshared").await?;
    tx.commit().await?;
    // One more than the native fetch budget proves this is paging, never a whole-node reach cap.
    // Keep every native trigger, parent FK, namespace check and scope stamp active. A 128-row
    // setup batch bounds repeated versions of the one scope row while requiring only 157
    // transactions at the current 20,001-row boundary. This is not a single-writer stress test.
    const SETUP_ROWS_PER_TRANSACTION: i64 = 128;
    let loading = Instant::now();
    let total = containers::TREE_READ_PAGE + 1;
    let mut first = 1_i64;
    let mut inserted = 0_u64;
    while first <= total {
        let last = first
            .saturating_add(SETUP_ROWS_PER_TRANSACTION - 1)
            .min(total);
        let mut tx = area.db.begin().await?;
        inserted+=sqlx::query("INSERT INTO documents(id,node_id,org_id,owner_id,title,created_by,created_at,updated_at,parent_container_id)
            SELECT gen_random_uuid(),$1,$2,$3,'Plan '||n,$3,$4,$4,$5 FROM generate_series($6::bigint,$7::bigint) n")
            .bind(area.node).bind(area.root).bind(area.user).bind(clock::start()).bind(child).bind(first).bind(last)
            .execute(&mut *tx).await?.rows_affected();
        tx.commit().await?;
        first = last + 1;
    }
    assert_eq!(
        i64::try_from(inserted)?,
        total,
        "the fixture still crosses the real native page boundary"
    );
    eprintln!(
        "document-container native fixture inserts: {inserted} documents in {:?}",
        loading.elapsed()
    );
    let mut conn = area.db.acquire().await?;
    let reading = Instant::now();
    let all = containers::candidates(&mut conn, &[area.node], &[], &[]).await?;
    eprintln!(
        "document-container complete node candidate read: {} entries in {:?}",
        all.len(),
        reading.elapsed()
    );
    assert_eq!(i64::try_from(all.len())?, containers::TREE_READ_PAGE + 4);
    let reading = Instant::now();
    let selected = containers::candidates(&mut conn, &[], &[], &[parent]).await?;
    eprintln!(
        "document-container complete folder candidate read: {} entries in {:?}",
        selected.len(),
        reading.elapsed()
    );
    assert_eq!(
        i64::try_from(selected.len())?,
        containers::TREE_READ_PAGE + 3
    );
    assert!(selected.iter().any(|entry| entry.id == child));
    assert!(!selected.iter().any(|entry| entry.id == sibling));
    let leaves: std::collections::HashSet<_> = selected
        .iter()
        .filter(|entry| entry.kind == "document")
        .map(|entry| entry.id)
        .collect();
    assert_eq!(
        i64::try_from(leaves.len())?,
        containers::TREE_READ_PAGE + 1,
        "no duplicate or omitted page boundary"
    );
    // Keep the disposable database alive until every query has completed, then close both pools.
    drop(conn);
    area.close().await?;
    Ok(())
}
