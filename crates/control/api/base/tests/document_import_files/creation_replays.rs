//! A header keeps the native import aggregate through independent commits and cancellation.
use super::scopes::{destination, key, retained, rule};
use super::*;
use http::StatusCode;
use orch_api_core::testing::RawReply;
use orch_types::{Json, new_id};
use serde_json::json;

async fn send(desk: &Desk, secret: &str, header: &str, input: &DocumentImportRequest) -> RawReply {
    let request = desk
        .world
        .app
        .call(NodeDocumentImport, &[&desk.topic])
        .token(secret)
        .header("Idempotency-Key", header)
        .body(input);
    request.send_bytes().await
}
fn value(reply: RawReply, status: StatusCode) -> Json {
    assert_eq!(
        reply.status,
        status,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn progress(desk: &Desk, header: &str) -> Json {
    sqlx::query_scalar("SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=$2")
        .bind(desk.ada.id())
        .bind(header)
        .fetch_one(&desk.world.db.pool)
        .await
        .unwrap()
}
fn input(files: Vec<(Hash, &str)>, client: &str) -> DocumentImportRequest {
    DocumentImportRequest {
        client_id: client.into(),
        files: files
            .into_iter()
            .map(|(blob, name)| DocumentImportRequestFile {
                blob,
                name: name.into(),
                format: Format::Markdown,
            })
            .collect(),
    }
}

#[tokio::test]
async fn a_completed_import_header_keeps_only_native_identity_and_reopens_current_content() {
    let desk = Desk::new().await;
    let blob = upload(
        &desk,
        &desk.ada,
        "source-name.md",
        b"# Original import title\n\nNative bytes.\n",
    )
    .await;
    let body = input(vec![(blob, "source-name.md")], "stable-native-import");
    let header = new_id().to_string();
    let first = value(
        send(&desk, &desk.ada.token, &header, &body).await,
        StatusCode::OK,
    );
    let id: Id = serde_json::from_value(first["entries"][0]["document"]["id"].clone()).unwrap();
    let record = progress(&desk, &header).await;
    assert_eq!(record["outcome"]["id"], json!(desk.topic));
    assert_eq!(
        record["outcome"]["data"]["client_id"],
        json!(body.client_id)
    );
    assert_eq!(record["outcome"]["data"]["documents"], json!([id]));
    assert_eq!(record["outcome"]["data"]["complete"], true);
    assert!(!record.to_string().contains("source-name.md"));
    assert!(!record.to_string().contains("Original import title"));
    sqlx::query("UPDATE documents SET title='Current imported title' WHERE id=$1")
        .bind(id)
        .execute(&desk.world.db.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM completed_uploads WHERE user_id=$1 AND hash=$2")
        .bind(desk.ada.id())
        .bind(blob.as_bytes().as_slice())
        .execute(&desk.world.db.pool)
        .await
        .unwrap();
    let read = key(
        &desk,
        desk.ada.id(),
        vec![rule(
            "NodeDocumentImport",
            "document",
            id,
            "self",
            &["document"],
            &["read"],
        )],
    )
    .await;
    let replay = value(
        send(&desk, &read.secret, &header, &body).await,
        StatusCode::OK,
    );
    assert_eq!(replay["entries"][0]["document"]["id"], json!(id));
    assert_eq!(
        replay["entries"][0]["document"]["title"],
        "Current imported title"
    );
    let wrong = key(
        &desk,
        desk.ada.id(),
        vec![rule(
            "DocumentGet.read",
            "document",
            id,
            "self",
            &["document"],
            &["read"],
        )],
    )
    .await;
    let receipt = value(
        send(&desk, &wrong.secret, &header, &body).await,
        StatusCode::OK,
    );
    assert_eq!(receipt["projection"], "receipt");
    assert_eq!(receipt["accepted_document_ids"], json!([id]));
    assert!(!receipt.to_string().contains("Current imported title"));
    let mut changed = body.clone();
    changed.files[0].name = "different.md".into();
    let mismatch = value(
        send(&desk, &read.secret, &header, &changed).await,
        StatusCode::CONFLICT,
    );
    assert_eq!(mismatch["code"], "conflict.idempotency_mismatch");
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action='document.created' AND object_id=$1",
    )
    .bind(id)
    .fetch_one(&desk.world.db.pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    sqlx::query("UPDATE documents SET deleted_at=$2 WHERE id=$1")
        .bind(id)
        .bind(desk.world.now())
        .execute(&desk.world.db.pool)
        .await
        .unwrap();
    let removed = value(
        send(&desk, &read.secret, &header, &body).await,
        StatusCode::OK,
    );
    assert_eq!(removed["projection"], "receipt");
    assert_eq!(removed["accepted_document_ids"], json!([id]));
    let live: bool = sqlx::query_scalar("SELECT deleted_at IS NULL FROM documents WHERE id=$1")
        .bind(id)
        .fetch_one(&desk.world.db.pool)
        .await
        .unwrap();
    assert!(!live, "a completed header never recreates a removed entry");
}

#[tokio::test]
async fn cancellation_after_one_native_commit_resumes_only_the_remaining_import_entries() {
    let desk = Desk::new().await;
    let first = upload(
        &desk,
        &desk.ada,
        "first.md",
        b"# First committed import\n\nKept.\n",
    )
    .await;
    let picture = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, PICTURE);
    let second = upload(
        &desk,
        &desk.ada,
        "second.md",
        format!("# Second committed import\n\n![image](data:image/png;base64,{picture})\n")
            .as_bytes(),
    )
    .await;
    let body = input(
        vec![(first, "first.md"), (second, "second.md")],
        "partial-header-import",
    );
    let header = new_id().to_string();
    let actor = key(
        &desk,
        desk.ada.id(),
        vec![destination(&desk, false), retained(&desk)],
    )
    .await;
    let mut blocker = desk.world.db.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE document_files IN SHARE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = desk.world.app.clone();
    let node = desk.topic;
    let secret = actor.secret.clone();
    let copy = body.clone();
    let key_header = header.clone();
    let task = tokio::spawn(async move {
        let request = app
            .call(NodeDocumentImport, &[&node])
            .token(&secret)
            .header("Idempotency-Key", &key_header)
            .body(&copy);
        request.send_bytes().await
    });
    super::scopes::wait(&desk, pid, "%INSERT INTO document_files%", &task).await;
    let id: Id = sqlx::query_scalar(
        "SELECT id FROM documents WHERE node_id=$1 AND title='First committed import'",
    )
    .bind(node)
    .fetch_one(&desk.world.db.pool)
    .await
    .unwrap();
    let xmin: String = sqlx::query_scalar("SELECT xmin::text FROM documents WHERE id=$1")
        .bind(id)
        .fetch_one(&desk.world.db.pool)
        .await
        .unwrap();
    let before = progress(&desk, &header).await;
    assert_eq!(before["outcome"]["data"]["documents"], json!([id]));
    assert_eq!(before["outcome"]["data"]["complete"], false);
    let owner = before["owner"].clone();
    let recorded_at: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT created_at FROM idempotency_keys WHERE user_id=$1 AND key=$2")
            .bind(desk.ada.id())
            .bind(&header)
            .fetch_one(&desk.world.db.pool)
            .await
            .unwrap();
    task.abort();
    assert!(matches!(task.await, Err(error) if error.is_cancelled()));
    blocker.commit().await.unwrap();
    let mut renamed = body.clone();
    renamed.files[1].name = "different-second.md".into();
    let mut other_import = body.clone();
    other_import.client_id = "another-native-import".into();
    for changed in [renamed, other_import] {
        let refused = value(
            send(&desk, &actor.secret, &header, &changed).await,
            StatusCode::CONFLICT,
        );
        assert_eq!(refused["code"], "conflict.idempotency_mismatch");
        assert_eq!(
            progress(&desk, &header).await,
            before,
            "a different body cannot claim or replace partial progress"
        );
        let unfinished: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM documents WHERE node_id=$1 AND title='Second committed import'",
        )
        .bind(node)
        .fetch_one(&desk.world.db.pool)
        .await
        .unwrap();
        assert_eq!(unfinished, 0, "a mismatched header never imports the rest");
    }
    desk.world.advance(chrono::Duration::minutes(1));
    sqlx::query("DELETE FROM completed_uploads WHERE user_id=$1 AND hash=$2")
        .bind(desk.ada.id())
        .bind(first.as_bytes().as_slice())
        .execute(&desk.world.db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
        .bind(actor.id)
        .bind(desk.world.now())
        .execute(&desk.world.db.pool)
        .await
        .unwrap();
    let read_only = key(
        &desk,
        desk.ada.id(),
        vec![rule(
            "NodeDocumentImport",
            "document",
            id,
            "self",
            &["document"],
            &["read"],
        )],
    )
    .await;
    let held = value(
        send(&desk, &read_only.secret, &header, &body).await,
        StatusCode::OK,
    );
    assert_eq!(held["projection"], "receipt");
    assert_eq!(held["accepted_document_ids"], json!([id]));
    assert_eq!(held["complete"], false);
    let unfinished: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM documents WHERE node_id=$1 AND title='Second committed import'",
    )
    .bind(node)
    .fetch_one(&desk.world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        unfinished, 0,
        "a resumed header never borrows the earlier writer's authority"
    );
    let fresh = key(
        &desk,
        desk.ada.id(),
        vec![destination(&desk, false), retained(&desk)],
    )
    .await;
    let result = value(
        send(&desk, &fresh.secret, &header, &body).await,
        StatusCode::OK,
    );
    assert_eq!(result["entries"][0]["document_id"], json!(id));
    assert!(result["entries"][1]["document_id"].is_string());
    let after = progress(&desk, &header).await;
    assert_eq!(after["outcome"]["id"], json!(node));
    assert_eq!(
        after["outcome"]["data"]["documents"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(after["outcome"]["data"]["complete"], true);
    assert_ne!(
        after["owner"], owner,
        "resume fences the earlier request owner"
    );
    let still_recorded_at: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT created_at FROM idempotency_keys WHERE user_id=$1 AND key=$2")
            .bind(desk.ada.id())
            .bind(&header)
            .fetch_one(&desk.world.db.pool)
            .await
            .unwrap();
    assert_eq!(
        recorded_at, still_recorded_at,
        "retry never extends the header window"
    );
    let now: String = sqlx::query_scalar("SELECT xmin::text FROM documents WHERE id=$1")
        .bind(id)
        .fetch_one(&desk.world.db.pool)
        .await
        .unwrap();
    assert_eq!(xmin, now, "the accepted first document was not rewritten");
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action='document.created' AND node_id=$1",
    )
    .bind(node)
    .fetch_one(&desk.world.db.pool)
    .await
    .unwrap();
    assert_eq!(count, 2, "only the remaining entry created a document");
}

#[tokio::test]
async fn an_import_retry_keeps_its_total_byte_allowance_and_original_source_order() {
    use orch_api_contract::settings::{NodeSettingSet, NodeSettingSetBody};
    let desk = Desk::new().await;
    let a = b"# A\n\nx\n";
    let b = b"# B\n\ny\n";
    let (a_source, _) = super::scopes::source_document(&desk, a).await;
    let (b_source, _) = super::scopes::source_document(&desk, b).await;
    for bytes in [a, b] {
        sqlx::query("DELETE FROM completed_uploads WHERE user_id=$1 AND hash=$2")
            .bind(desk.ada.id())
            .bind(Hash::of(bytes).as_bytes().as_slice())
            .execute(&desk.world.db.pool)
            .await
            .unwrap();
    }
    async fn maximum(desk: &Desk, bytes: i64) {
        let request = desk
            .world
            .app
            .call(
                NodeSettingSet,
                &[&desk.project, &"sharing.snapshot_max_bytes"],
            )
            .token(&desk.ada.token)
            .body(&NodeSettingSetBody {
                value: json!(bytes),
                locked: None,
                expected_revision: None,
            });
        let _saved = request.send().await.ok();
    }
    maximum(&desk, 10).await;
    let body = input(
        vec![(Hash::of(a), "a.md"), (Hash::of(b), "b.md")],
        "source-order-and-bytes",
    );
    let header = new_id().to_string();
    let first = key(
        &desk,
        desk.ada.id(),
        vec![
            destination(&desk, false),
            rule(
                "NodeDocumentImport",
                "document",
                b_source,
                "self",
                &["document"],
                &["read"],
            ),
        ],
    )
    .await;
    let partial = value(
        send(&desk, &first.secret, &header, &body).await,
        StatusCode::OK,
    );
    assert!(partial["entries"][0]["document_id"].is_null());
    let b_id: Id = serde_json::from_value(partial["entries"][1]["document_id"].clone()).unwrap();
    let fresh = key(
        &desk,
        desk.ada.id(),
        vec![
            destination(&desk, false),
            rule(
                "NodeDocumentImport",
                "document",
                a_source,
                "self",
                &["document"],
                &["read"],
            ),
        ],
    )
    .await;
    let limited = value(
        send(&desk, &fresh.secret, &header, &body).await,
        StatusCode::OK,
    );
    assert!(
        limited["entries"][0]["document_id"].is_null(),
        "the accepted source still counts against the whole import allowance"
    );
    assert!(limited["entries"][0]["error"].is_string());
    assert_eq!(limited["entries"][1]["document_id"], json!(b_id));
    assert_eq!(
        progress(&desk, &header).await["outcome"]["data"]["source_bytes"],
        json!({"1":7})
    );
    maximum(&desk, 14).await;
    let completed = value(
        send(&desk, &fresh.secret, &header, &body).await,
        StatusCode::OK,
    );
    let a_id: Id = serde_json::from_value(completed["entries"][0]["document_id"].clone()).unwrap();
    assert_eq!(completed["entries"][1]["document_id"], json!(b_id));
    let record = progress(&desk, &header).await;
    assert_eq!(record["outcome"]["data"]["documents"], json!([a_id, b_id]));
    assert_eq!(
        record["outcome"]["data"]["source_bytes"],
        json!({"0":7,"1":7})
    );
    assert_eq!(record["outcome"]["data"]["complete"], true);
    let read = key(
        &desk,
        desk.ada.id(),
        vec![
            rule(
                "NodeDocumentImport",
                "document",
                a_id,
                "self",
                &["document"],
                &["read"],
            ),
            rule(
                "NodeDocumentImport",
                "document",
                b_id,
                "self",
                &["document"],
                &["read"],
            ),
        ],
    )
    .await;
    let replay = value(
        send(&desk, &read.secret, &header, &body).await,
        StatusCode::OK,
    );
    assert_eq!(replay["entries"][0]["document"]["id"], json!(a_id));
    assert_eq!(replay["entries"][1]["document"]["id"], json!(b_id));
}
