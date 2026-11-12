use super::*;

#[tokio::test]
async fn target_index_serves_shared_and_personal_invalidation_without_touching_another_target()
-> TestResult {
    let world = World::new().await?;
    let target = SummaryTarget::Document { id: world.document };
    let mut caches = Vec::new();
    for viewer in [None, Some(world.ada), Some(world.bob)] {
        caches.push(world.cache(target, viewer).await?);
    }
    let other = world
        .cache(SummaryTarget::Topic { id: world.topic }, None)
        .await?;
    let mut tx = world.db.begin().await?;
    // This checks index applicability on a tiny fixture, not a measured production plan choice.
    sqlx::query("SET LOCAL enable_seqscan=off")
        .execute(&mut *tx)
        .await?;
    let plan: Json = sqlx::query_scalar(
        "EXPLAIN (FORMAT JSON) UPDATE resource_summaries \
         SET dirty_revision=dirty_revision+1 WHERE kind='document' \
         AND COALESCE(node_id,document_id,chat_id)=$1",
    )
    .bind(world.document)
    .fetch_one(&mut *tx)
    .await?;
    assert!(
        plan.to_string().contains("resource_summaries_target_idx"),
        "{plan}"
    );
    summaries::changed(&mut tx, target, "content_changed", clock::start()).await?;
    for cache in caches {
        let dirty: i64 =
            sqlx::query_scalar("SELECT dirty_revision FROM resource_summaries WHERE id=$1")
                .bind(cache.id)
                .fetch_one(&mut *tx)
                .await?;
        assert_eq!(dirty, cache.dirty_revision + 1);
    }
    let dirty: i64 =
        sqlx::query_scalar("SELECT dirty_revision FROM resource_summaries WHERE id=$1")
            .bind(other.id)
            .fetch_one(&mut *tx)
            .await?;
    assert_eq!(dirty, other.dirty_revision);
    tx.rollback().await?;
    world.close().await
}
