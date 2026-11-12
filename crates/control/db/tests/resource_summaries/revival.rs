use super::*;

async fn failed(
    world: &World,
    ordinal: i64,
    recovery: Option<(&str, chrono::DateTime<chrono::Utc>)>,
) -> TestResult<summaries::Request> {
    let id = new_id();
    let now = clock::start();
    let mut tx = world.db.begin().await?;
    let title = format!("Recovery source {ordinal}");
    documents::insert(
        &mut tx,
        &documents::NewDocument {
            id,
            node_id: world.topic,
            org_id: world.root,
            owner_id: world.ada,
            created_by_thread: None,
            created_by_agent: None,
            title: &title,
            now,
        },
    )
    .await?;
    let cache = summaries::ensure(&mut tx, SummaryTarget::Document { id }, None, now)
        .await?
        .ok_or("cache missing")?;
    let request = new(summaries::enqueue(
        &mut tx,
        cache.id,
        Origin::Automatic { writer: world.ada },
        &json!({"ordinal":ordinal}),
        &json!({"original":ordinal}),
        now,
        now,
    )
    .await?)?;
    sqlx::query("UPDATE resource_summary_requests SET state='failed',finished_at=$2,prepared=$3 WHERE id=$1")
        .bind(request.id).bind(now).bind(json!({"retained":ordinal})).execute(&mut *tx).await?;
    if let Some((task, due)) = recovery {
        sqlx::query("INSERT INTO model_call_recovery(node_id,purpose,call_key,reference,failure,rule,cursor,state,due_at,context,updated_at) VALUES($1,'summary',$2,$3,'{}','{}','{}','scheduled',$4,$5,$6)")
            .bind(world.topic).bind(request.id).bind(new_id()).bind(due)
            .bind(json!({"task":task})).bind(now + Duration::seconds(ordinal))
            .execute(&mut *tx).await?;
    }
    tx.commit().await?;
    let mut conn = world.db.acquire().await?;
    Ok(summaries::request(&mut conn, request.id)
        .await?
        .ok_or("request missing")?)
}

#[tokio::test]
async fn revival_drains_bounded_due_pages_without_replacing_original_work() -> TestResult {
    let world = World::new().await?;
    let now = clock::start() + Duration::seconds(20);
    let mut expected = Vec::new();
    for index in 0..5 {
        expected.push(failed(&world, index, Some(("resource_summary", now))).await?);
    }
    let no_recovery = failed(&world, 10, None).await?;
    let other_task = failed(&world, 11, Some(("another_task", now))).await?;
    let future = failed(
        &world,
        12,
        Some(("resource_summary", now + Duration::hours(1))),
    )
    .await?;
    let mut promoted = Vec::new();
    for size in [2, 2, 1] {
        let mut tx = world.db.begin().await?;
        let page = summaries::revive(&mut tx, now, 2).await?;
        assert_eq!(page.len(), size);
        assert_eq!(
            page.iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>(),
            expected[promoted.len()..promoted.len() + size]
                .iter()
                .map(|request| request.id)
                .collect()
        );
        promoted.extend(page);
        tx.commit().await?;
    }
    let mut tx = world.db.begin().await?;
    assert!(summaries::revive(&mut tx, now, 2).await?.is_empty());
    tx.commit().await?;
    assert_eq!(
        promoted
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>(),
        expected.iter().map(|request| request.id).collect()
    );
    let mut conn = world.db.acquire().await?;
    for original in expected {
        let current = summaries::request(&mut conn, original.id)
            .await?
            .ok_or("request missing")?;
        assert_eq!(current.state, "queued");
        assert_eq!(current.arguments, original.arguments);
        assert_eq!(current.accepted_source, original.accepted_source);
        assert_eq!(current.prepared, original.prepared);
        assert_eq!(current.accepted_revision, original.accepted_revision);
        assert_eq!(current.paying_user, original.paying_user);
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM model_call_recovery WHERE call_key=$1")
                .bind(original.id)
                .fetch_one(&mut *conn)
                .await?;
        assert_eq!(count, 1);
    }
    for request in [no_recovery, other_task, future] {
        assert_eq!(
            summaries::request(&mut conn, request.id)
                .await?
                .ok_or("request missing")?
                .state,
            "failed"
        );
    }
    drop(conn);
    world.close().await
}

#[tokio::test]
async fn revival_cannot_replace_a_more_recent_refresh() -> TestResult {
    let world = World::new().await?;
    let now = clock::start() + Duration::seconds(20);
    let old = failed(&world, 0, Some(("resource_summary", now))).await?;
    let mut tx = world.db.begin().await?;
    let newer = new(summaries::enqueue(
        &mut tx,
        old.summary_id,
        Origin::Automatic { writer: world.ada },
        &json!({"newer":true}),
        &json!({"newer":true}),
        now,
        now,
    )
    .await?)?;
    tx.commit().await?;
    let mut tx = world.db.begin().await?;
    assert!(summaries::revive(&mut tx, now, 2).await?.is_empty());
    tx.commit().await?;
    let mut conn = world.db.acquire().await?;
    assert_eq!(
        summaries::request(&mut conn, old.id)
            .await?
            .ok_or("old missing")?
            .state,
        "failed"
    );
    assert_eq!(
        summaries::request(&mut conn, newer.id)
            .await?
            .ok_or("new missing")?
            .state,
        "queued"
    );
    drop(conn);
    world.close().await
}

#[tokio::test]
async fn a_locked_summary_does_not_starve_the_rest_of_a_revival_page() -> TestResult {
    let world = World::new().await?;
    let now = clock::start() + Duration::seconds(20);
    let first = failed(&world, 0, Some(("resource_summary", now))).await?;
    let second = failed(&world, 1, Some(("resource_summary", now))).await?;
    let mut held = world.db.begin().await?;
    sqlx::query("SELECT id FROM resource_summaries WHERE id=$1 FOR UPDATE")
        .bind(first.summary_id)
        .execute(&mut *held)
        .await?;
    let mut tx = world.db.begin().await?;
    assert_eq!(summaries::revive(&mut tx, now, 1).await?, vec![second.id]);
    tx.commit().await?;
    held.rollback().await?;
    let mut tx = world.db.begin().await?;
    assert_eq!(summaries::revive(&mut tx, now, 1).await?, vec![first.id]);
    tx.commit().await?;
    world.close().await
}
