//! Multi-chunk edits retain current block state and exact counters while refusing and rolling back later conflicts.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_api_contract::documents::{
    BlockContent, BlockType, DOCUMENT_OPS_MAX, DocumentOp, Inline, NewBlock,
};
use orch_db::{
    Db,
    queries::{
        documents::{self, batch},
        hierarchy, resources,
    },
};
use orch_documents::versions::Writer;
use orch_testkit::{clock, database::TestDb};
use orch_types::{Id, new_id};

const LIMITS: batch::Limits = batch::Limits {
    max_bytes: 1_000_000,
    max_blocks: 10_000,
};
const RULES: batch::Rules = batch::Rules {
    window_secs: 3600,
    kept: 10,
    history_max_bytes: 10_000_000,
};
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

fn text(value: &str) -> BlockContent {
    BlockContent::Inline(vec![Inline::Text {
        text: value.into(),
        marks: None,
        href: None,
        color: None,
        background: None,
    }])
}

fn creation(id: Id, after: Option<Id>) -> DocumentOp {
    DocumentOp::Insert {
        block: NewBlock {
            id,
            block_type: BlockType::Paragraph,
            props: None,
            content: Some(text("before")),
            children: None,
        },
        parent_id: None,
        after_id: after,
    }
}

#[tokio::test]
async fn later_chunks_use_earlier_changes_and_count_only_live_content() -> TestResult {
    let test = TestDb::fresh().await;
    let db = Db::from_pool(test.pool.clone());
    let now = clock::start();
    let (root, document) = (new_id(), new_id());
    let mut tx = db.begin().await?;
    let user = resources::testing::insert_user(&mut tx, "chunk_writer").await?;
    hierarchy::testing::insert_node(&mut tx, root, "org", None, "Office", user).await?;
    documents::insert(
        &mut tx,
        &documents::NewDocument {
            id: document,
            node_id: root,
            org_id: root,
            owner_id: user,
            created_by_thread: None,
            created_by_agent: None,
            title: "Chunks",
            now,
        },
    )
    .await?;
    let most = usize::try_from(DOCUMENT_OPS_MAX)?;
    let ids: Vec<Id> = (0..most).map(|_| new_id()).collect();
    let mut ops: Vec<_> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| creation(*id, i.checked_sub(1).map(|j| ids[j])))
        .collect();
    for id in &ids {
        ops.push(DocumentOp::Update {
            id: *id,
            base_seq: 1,
            block_type: None,
            props: None,
            content: Some(text("after, with more bytes")),
        });
    }
    // The third chunk names and removes state established by the first two.
    ops.push(DocumentOp::Move {
        id: ids[0],
        parent_id: Some(ids[most - 1]),
        after_id: None,
    });
    ops.push(DocumentOp::Remove { id: ids[most - 1] });
    let final_id = new_id();
    ops.push(creation(final_id, None));
    let outcome = batch::apply_chunks(
        &mut tx,
        document,
        &Writer::User(user),
        &ops,
        LIMITS,
        RULES,
        now,
    )
    .await?;
    let batch::Outcome::Applied(applied) = outcome else {
        panic!("{outcome:?}")
    };
    assert_eq!(applied.seq, 3);
    let current = documents::get(&mut tx, document).await?.unwrap();
    let counted: (i64,i64) = sqlx::query_as(
        "SELECT count(*)::bigint,coalesce(sum(octet_length(props::text)+octet_length(content::text)),0)::bigint FROM document_blocks WHERE document_id=$1 AND removed_at IS NULL"
    ).bind(document).fetch_one(&mut *tx).await?;
    assert_eq!(counted.0, i64::try_from(most)? - 1);
    assert_eq!((i64::from(current.block_count), current.bytes), counted);
    let updated = documents::blocks_by_ids(&mut tx, document, &[ids[1], final_id]).await?;
    assert_eq!(updated.len(), 2);
    assert!(updated.iter().any(|row| row.id == ids[1]
        && row.text == "after, with more bytes"
        && row.seq_updated == 2));
    assert!(
        updated
            .iter()
            .any(|row| row.id == final_id && row.text == "before" && row.seq_updated == 3)
    );
    tx.commit().await?;
    db.pool().close().await;
    test.close().await?;
    Ok(())
}

#[tokio::test]
async fn a_later_stale_or_oversize_chunk_requires_the_entire_edit_to_roll_back() -> TestResult {
    let test = TestDb::fresh().await;
    let db = Db::from_pool(test.pool.clone());
    let now = clock::start();
    let (root, document) = (new_id(), new_id());
    let user;
    {
        let mut tx = db.begin().await?;
        user = resources::testing::insert_user(&mut tx, "chunk_writer").await?;
        hierarchy::testing::insert_node(&mut tx, root, "org", None, "Office", user).await?;
        documents::insert(
            &mut tx,
            &documents::NewDocument {
                id: document,
                node_id: root,
                org_id: root,
                owner_id: user,
                created_by_thread: None,
                created_by_agent: None,
                title: "Rollback",
                now,
            },
        )
        .await?;
        tx.commit().await?;
    }
    let most = usize::try_from(DOCUMENT_OPS_MAX)?;
    let ids: Vec<Id> = (0..most).map(|_| new_id()).collect();
    let content = text("before");
    let checked =
        orch_documents::blocks::check(BlockType::Paragraph, None, Some(&content)).unwrap();
    let bytes: i64 = sqlx::query_scalar(
        "SELECT (octet_length($1::jsonb::text)+octet_length($2::jsonb::text))::bigint",
    )
    .bind(&checked.props)
    .bind(checked.content_json())
    .fetch_one(db.pool())
    .await?;
    // Exactly one first chunk fits; a later valid-sized block crosses this document's byte budget.
    let max_bytes = bytes * i64::try_from(most)? + 1_000;
    for stale in [true, false] {
        let mut tx = db.begin().await?;
        let mut ops: Vec<_> = ids.iter().map(|id| creation(*id, None)).collect();
        ops.push(DocumentOp::Update {
            id: ids[0],
            base_seq: if stale { 0 } else { 1 },
            block_type: None,
            props: None,
            content: Some(text(&"x".repeat(20_000))),
        });
        let limits = if stale {
            LIMITS
        } else {
            batch::Limits {
                max_bytes,
                ..LIMITS
            }
        };
        let outcome = batch::apply_chunks(
            &mut tx,
            document,
            &Writer::User(user),
            &ops,
            limits,
            RULES,
            now,
        )
        .await?;
        if stale {
            assert!(
                matches!(outcome, batch::Outcome::Stale { .. }),
                "{outcome:?}"
            );
        } else {
            assert!(
                matches!(outcome, batch::Outcome::TooLarge { .. }),
                "{outcome:?}"
            );
        }
        let sequence: i64 = sqlx::query_scalar("SELECT doc_seq FROM documents WHERE id=$1")
            .bind(document)
            .fetch_one(&mut *tx)
            .await?;
        assert_eq!(
            sequence,
            if stale { 1 } else { 2 },
            "the first chunk was accepted before the later refusal"
        );
        tx.rollback().await?;
        let mut conn = db.acquire().await?;
        let current = documents::get(&mut conn, document).await?.unwrap();
        assert_eq!(
            (current.doc_seq, current.block_count, current.bytes),
            (0, 0, 0)
        );
        assert!(
            documents::blocks_by_ids(&mut conn, document, &ids)
                .await?
                .is_empty()
        );
    }
    db.pool().close().await;
    test.close().await?;
    Ok(())
}
