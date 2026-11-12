//! The read and write options doors never lend one another their scope.
use super::*;

type Proof = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn option_scope_refusals_preserve_native_content_options_and_refresh_work() -> Proof {
    let desk = Desk::new().await;
    let document = desk
        .make(&desk.ada, "Options permission source")
        .await
        .base
        .id;
    let target = SummaryTarget::Document { id: document };
    let read = key(
        &desk,
        rule("document", document, "DocumentSummaryOptionsGet", "read"),
        chrono::Duration::hours(1),
    )
    .await;
    let write = key(
        &desk,
        rule("document", document, "DocumentSummaryOptionsSet", "write"),
        chrono::Duration::hours(1),
    )
    .await;
    let mut conn = desk.world.conn().await;
    let original = documents::get(&mut conn, document)
        .await?
        .ok_or("the actual document is absent")?;
    drop(conn);
    let before: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM resource_summaries),(SELECT count(*) FROM resource_summary_requests),(SELECT count(*) FROM model_inputs)")
        .fetch_one(&desk.world.db.pool).await?;
    for refused in [
        desk.world
            .app
            .call(DocumentSummaryOptionsGet, &[&document])
            .token(&write)
            .send_bytes()
            .await,
        desk.world
            .app
            .call(DocumentSummaryOptionsSet, &[&document])
            .token(&read)
            .body(&body(json!({"automatic":false,"output_chars":1100}), 0))
            .send_bytes()
            .await,
    ] {
        assert_eq!(refused.status, StatusCode::FORBIDDEN);
        assert_eq!(refused.problem().code, ErrorCode::AuthzDenied);
        let mut conn = desk.world.conn().await;
        assert_eq!(
            documents::get(&mut conn, document).await?.as_ref(),
            Some(&original)
        );
        assert!(summaries::get(&mut conn, target, None).await?.is_none());
        drop(conn);
        let after: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM resource_summaries),(SELECT count(*) FROM resource_summary_requests),(SELECT count(*) FROM model_inputs)")
            .fetch_one(&desk.world.db.pool).await?;
        assert_eq!(after, before);
    }
    let changed = desk
        .world
        .app
        .call(DocumentSummaryOptionsSet, &[&document])
        .token(&write)
        .body(&body(json!({"automatic":false,"output_chars":1100}), 0))
        .send_bytes()
        .await;
    assert_eq!(changed.status, StatusCode::OK);
    let receipt: Json = serde_json::from_slice(&changed.raw)?;
    assert_eq!(receipt["projection"], "receipt");
    assert_eq!(receipt["revision"], 1);
    let shown = desk
        .world
        .app
        .call(DocumentSummaryOptionsGet, &[&document])
        .token(&read)
        .send()
        .await
        .ok();
    assert_eq!(shown.target, target);
    assert_eq!(shown.revision, 1);
    assert_eq!(shown.options.output_chars.map(|n| n.get()), Some(1100));
    let mut conn = desk.world.conn().await;
    assert_eq!(
        documents::get(&mut conn, document).await?.as_ref(),
        Some(&original)
    );
    drop(conn);
    let work: (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM resource_summary_requests),(SELECT count(*) FROM model_inputs)")
        .fetch_one(&desk.world.db.pool).await?;
    assert_eq!(work, (before.1, before.2));
    Ok(())
}
