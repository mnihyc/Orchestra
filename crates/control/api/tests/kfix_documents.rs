//! Document history counts distinct changed blocks through edits, version cuts and restores.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod support;

use chrono::Duration;
use desk::{Desk, rewrite, said};
use orch_api_contract::documents::{
    DocumentChangesGet, DocumentChangesGetQuery, DocumentSeen, DocumentSeenBody,
    DocumentVersionList, DocumentVersionListQuery, DocumentVersionRestore,
};
use orch_db::queries::documents::history;
use orch_types::Id;
use std::collections::BTreeSet;

/// The idle interval shipped for cutting a document version.
const IDLE_SECS: i64 = 120;

async fn cut_idle(desk: &Desk, document: Id) -> i32 {
    desk.world.advance(Duration::seconds(IDLE_SECS));
    let mut conn = desk.world.db.pool.acquire().await.unwrap();
    history::cut_if_left_alone(&mut conn, document, desk.world.now(), IDLE_SECS)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn repeated_edits_count_two_distinct_blocks_in_the_open_and_cut_history_and_since_seen() {
    let desk = Desk::new().await;
    let made = desk.make(&desk.ada, "Plan").await;
    let document = made.base.id;
    let blocks = desk
        .paragraphs(&desk.ada, document, &["alpha", "beta", "untouched"])
        .await;
    assert_eq!(cut_idle(&desk, document).await, 1);
    desk.world.advance(Duration::seconds(1));
    let seq = desk.sync(&desk.ada, document).await.seq;
    desk.world
        .app
        .call(DocumentSeen, &[&document])
        .token(&desk.ada.token)
        .body(&DocumentSeenBody { seq })
        .send()
        .await
        .ok();

    let unrelated = desk.make(&desk.ada, "Another document").await;
    desk.paragraphs(&desk.ada, unrelated.base.id, &["unrelated writing"])
        .await;
    let mut alpha = blocks[0].clone();
    for words in ["alpha once", "alpha twice", "alpha three times"] {
        alpha = desk
            .write(&desk.ada, document, vec![rewrite(&alpha, words)])
            .await
            .ok()
            .blocks
            .remove(0);
    }
    desk.write(
        &desk.ada,
        document,
        vec![
            rewrite(&alpha, "alpha four times"),
            rewrite(&alpha, "alpha finally"),
            rewrite(&blocks[1], "beta once"),
        ],
    )
    .await
    .ok();
    let versions = desk
        .world
        .app
        .call(DocumentVersionList, &[&document])
        .token(&desk.ada.token)
        .query(&DocumentVersionListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(versions.items[0].version, None);
    assert_eq!(versions.items[0].changed.updated, 2);
    assert_eq!(versions.items[0].changed.inserted, 0);
    assert_eq!(versions.items[0].changed.moved, 0);
    assert_eq!(versions.items[0].changed.removed, 0);

    let changed_ids = BTreeSet::from([blocks[0].id, blocks[1].id]);
    let since = desk
        .world
        .app
        .call(DocumentChangesGet, &[&document])
        .token(&desk.ada.token)
        .query(&DocumentChangesGetQuery {
            inspect_deleted: None,
            from: "seen".into(),
            to: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(since.from, 1);
    assert_eq!(since.blocks.len(), 2);
    assert_eq!(
        since
            .blocks
            .iter()
            .map(|block| block.id)
            .collect::<BTreeSet<_>>(),
        changed_ids
    );

    assert_eq!(cut_idle(&desk, document).await, 2);
    let versions = desk
        .world
        .app
        .call(DocumentVersionList, &[&document])
        .token(&desk.ada.token)
        .query(&DocumentVersionListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(versions.items[0].version, Some(2));
    assert_eq!(versions.items[0].changed.updated, 2);
    let span = desk
        .world
        .app
        .call(DocumentChangesGet, &[&document])
        .token(&desk.ada.token)
        .query(&DocumentChangesGetQuery {
            inspect_deleted: None,
            from: "1".into(),
            to: Some(2),
        })
        .send()
        .await
        .ok();
    assert_eq!(span.blocks.len(), 2);
    assert_eq!(
        span.blocks
            .iter()
            .map(|block| block.id)
            .collect::<BTreeSet<_>>(),
        changed_ids
    );

    let restored = desk
        .world
        .app
        .call(DocumentVersionRestore, &[&document, &1])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    let restored_alpha = restored
        .blocks
        .iter()
        .find(|block| block.id == alpha.id)
        .unwrap();
    assert_eq!(said(restored_alpha), "alpha");
    let edited = desk
        .write(
            &desk.ada,
            document,
            vec![rewrite(restored_alpha, "after restore")],
        )
        .await
        .ok();
    desk.write(
        &desk.ada,
        document,
        vec![rewrite(&edited.blocks[0], "after restore again")],
    )
    .await
    .ok();
    let versions = desk
        .world
        .app
        .call(DocumentVersionList, &[&document])
        .token(&desk.ada.token)
        .query(&DocumentVersionListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(versions.items[0].version, None);
    assert_eq!(
        versions.items[0].changed.updated, 2,
        "restored blocks and later edits share one span"
    );
    assert_eq!(
        desk.words(&desk.ada, document).await,
        ["after restore again", "beta", "untouched"]
    );
}
