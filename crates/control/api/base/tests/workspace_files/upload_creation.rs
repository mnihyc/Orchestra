//! Native accepted uploads replay their current job after the original writer has ended.
use super::*;
use orch_api_core::testing::RawReply;

async fn send(
    site: &Site,
    token: &str,
    id: Id,
    act: Id,
    body: &EnvironmentUploadWriteBody,
) -> RawReply {
    let call = site
        .app()
        .call(EnvironmentUploadWrite, &[&site.environment, &id])
        .token(token)
        .header("Idempotency-Key", &act.to_string())
        .query(&ThroughThreadQuery::default())
        .body(body);
    call.send_bytes().await
}
fn accepted(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::ACCEPTED,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn one_upload(completed: bool) {
    let site = live_site().await;
    let blob = browser_upload(&site, b"one actual uploaded file").await;
    let before_pins = upload_pins(&site, blob).await;
    let writer = keys::scoped(
        &site,
        site.ada.id(),
        vec![keys::rule(
            "EnvironmentUploadWrite",
            "environment",
            site.environment,
            "self",
            &["write"],
        )],
    )
    .await;
    let id = new_id();
    let act = new_id();
    let body = EnvironmentUploadWriteBody {
        path: "web/header-upload.txt".into(),
        blob,
        expected_hash: Some("none".into()),
        upload_batch: new_id(),
    };
    let receipt = accepted(send(&site, &writer.secret, id, act, &body).await);
    assert_eq!(receipt, json!({"projection":"receipt","job_id":id}));
    assert_eq!(upload_pins(&site, blob).await, before_pins + 1);
    let record: (Json, String) = sqlx::query_as(
        "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
    )
    .bind(site.ada.id())
    .bind(act.to_string())
    .fetch_one(&site.world.db.pool)
    .await
    .unwrap();
    assert_eq!(record.0["outcome"]["kind"], "workspace_upload_job");
    assert_eq!(record.0["outcome"]["id"], json!(id));
    assert!(record.0["outcome"]["data"].is_null());
    if completed {
        site.app().app.engine.start();
        eventually("the one real native upload write", || async {
            upload_job(&site, id).await.state == JobState::Succeeded
        })
        .await;
        assert_eq!(site.sent(CommandKind::FsWrite), 1);
        assert_eq!(upload_pins(&site, blob).await, before_pins);
        site.app().app.engine.stop();
    }
    keys::revoke(&site, &writer).await;
    site.set_remote_write(false).await;
    let job_before: (Json, String) =
        sqlx::query_as("SELECT to_jsonb(j),xmin::text FROM jobs j WHERE id=$1")
            .bind(id)
            .fetch_one(&site.world.db.pool)
            .await
            .unwrap();
    let reader = keys::scoped(
        &site,
        site.ada.id(),
        vec![keys::read("EnvironmentUploadWrite", site.environment)],
    )
    .await;
    let current = accepted(send(&site, &reader.secret, id, act, &body).await);
    assert_eq!(current["id"], json!(id));
    assert_eq!(
        current["state"],
        if completed { "succeeded" } else { "queued" }
    );
    if completed {
        assert_eq!(current["result"]["path"], body.path);
    }
    assert!(current.get("projection").is_none());
    let neighbour = keys::scoped(
        &site,
        site.ada.id(),
        vec![keys::read("EnvironmentFileStat", site.environment)],
    )
    .await;
    assert_eq!(
        accepted(send(&site, &neighbour.secret, id, act, &body).await),
        receipt
    );
    let changed = EnvironmentUploadWriteBody {
        path: "web/another.txt".into(),
        ..body.clone()
    };
    let refused = send(&site, &reader.secret, id, act, &changed).await;
    assert_eq!(refused.status, StatusCode::CONFLICT);
    assert_eq!(
        refused.problem().code,
        ErrorCode::ConflictIdempotencyMismatch
    );
    let after: (Json, String) = sqlx::query_as(
        "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
    )
    .bind(site.ada.id())
    .bind(act.to_string())
    .fetch_one(&site.world.db.pool)
    .await
    .unwrap();
    assert_eq!(record, after);
    let job_after: (Json, String) =
        sqlx::query_as("SELECT to_jsonb(j),xmin::text FROM jobs j WHERE id=$1")
            .bind(id)
            .fetch_one(&site.world.db.pool)
            .await
            .unwrap();
    assert_eq!(
        job_before, job_after,
        "readback never changes or requeues the native job"
    );
    assert_eq!(
        site.sent(CommandKind::FsWrite),
        if completed { 1 } else { 0 }
    );
    assert_eq!(
        upload_pins(&site, blob).await,
        before_pins + if completed { 0 } else { 1 }
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE environment_id=$1 AND plan->'workspace_upload' IS NOT NULL")
        .bind(site.environment).fetch_one(&site.world.db.pool).await.unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn an_upload_header_reads_its_one_queued_job_without_repinning_or_requiring_spent_write() {
    one_upload(false).await;
}

#[tokio::test]
async fn an_upload_header_reads_the_one_native_written_result_without_writing_or_auditing_again() {
    one_upload(true).await;
}
