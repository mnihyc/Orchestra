//! The native uploaded-source inspector keeps its real destination and byte custody bounds.

use super::*;
use orch_api_contract::imports::{ImportPreview, ImportPreviewRequest, NodeImportPreview};

const OPERATION: &str = "NodeImportPreview";

fn rules(o: &Office, destination: bool, bytes: bool) -> Vec<Value> {
    let mut values = Vec::new();
    if destination {
        values.push(rule(
            OPERATION,
            "topic",
            o.topic(),
            "self",
            &["topic"],
            &["read"],
        ));
    }
    if bytes {
        values.push(rule(
            OPERATION,
            "account",
            o.s.ada.id(),
            "self",
            &["upload"],
            &["read"],
        ));
    }
    values
}

async fn scene_preview() -> (Office, ImportPreviewRequest) {
    let o = Office::new().await;
    let file = upload(
        &o,
        &o.s.ada,
        "scoped-preview.md",
        b"# Scoped preview\n\nActual stored source.\n",
    )
    .await;
    let request = asked("source-preview", &file);
    let plan = plan(&o, &o.s.ada, o.topic(), &request).await.ok();
    let object = plan
        .objects
        .items
        .iter()
        .find(|object| object.kind == PackageKind::Document)
        .unwrap()
        .source_id;
    (
        o,
        ImportPreviewRequest {
            source: request.source,
            object,
        },
    )
}

async fn call(o: &Office, token: &str, request: &ImportPreviewRequest) -> RawReply {
    let node = o.topic();
    let call = o.s.rig.world.app.call(NodeImportPreview, &[&node]);
    call.token(token).body(request).send_bytes().await
}

#[tokio::test]
async fn native_source_preview_needs_its_actual_destination_and_upload_custody_and_creates_nothing()
{
    let (o, request) = scene_preview().await;
    let reader = key(&o, &o.s.ada, rules(&o, true, true)).await;
    let counts = sqlx::query_as::<_, (i64, i64)>(
        "SELECT (SELECT count(*) FROM imports),(SELECT count(*) FROM documents)",
    )
    .fetch_one(o.s.rig.db.pool())
    .await
    .unwrap();
    let native = call(&o, &o.s.ada.token, &request).await;
    assert_eq!(native.status, StatusCode::OK);
    let scoped = call(&o, &reader.secret, &request).await;
    assert_eq!(
        scoped.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&scoped.raw)
    );
    let body: ImportPreview = serde_json::from_slice(&scoped.raw).unwrap();
    assert_eq!(
        body,
        serde_json::from_slice::<ImportPreview>(&native.raw).unwrap()
    );
    assert!(body.text.contains("Actual stored source."));
    for (destination, bytes) in [(true, false), (false, true)] {
        let denied = key(&o, &o.s.ada, rules(&o, destination, bytes)).await;
        assert!(
            call(&o, &denied.secret, &request)
                .await
                .status
                .is_client_error()
        );
    }
    let wrong = key(&o, &o.s.ada, vec![artifact("ImportGet", new_id())]).await;
    assert_eq!(
        call(&o, &wrong.secret, &request).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        sqlx::query_as::<_, (i64, i64)>(
            "SELECT (SELECT count(*) FROM imports),(SELECT count(*) FROM documents)"
        )
        .fetch_one(o.s.rig.db.pool())
        .await
        .unwrap(),
        counts
    );
}

#[tokio::test]
async fn native_source_preview_keeps_actual_ownership_and_never_treats_a_source_uuid_as_a_live_object()
 {
    let (o, request) = scene_preview().await;
    let reader = key(&o, &o.s.ada, rules(&o, true, true)).await;
    let mut wrong_object = request.clone();
    wrong_object.object = o.document.base.id;
    assert_eq!(
        call(&o, &reader.secret, &wrong_object).await.status,
        StatusCode::NOT_FOUND
    );
    let foreign = upload(
        &o,
        &o.s.cai,
        "foreign-preview.md",
        b"# Private source of another person\n",
    )
    .await;
    let mut request = request;
    request.source.files = vec![foreign];
    let denied = call(&o, &reader.secret, &request).await;
    assert_eq!(denied.status, StatusCode::NOT_FOUND);
    assert!(!String::from_utf8_lossy(&denied.raw).contains("Private source"));
}

#[tokio::test]
async fn native_source_preview_publication_keeps_original_custody_and_never_reopens_the_supplier() {
    for mode in [
        "unchanged",
        "physical_source_removed",
        "custody_released",
        "scope_closed",
        "expired",
    ] {
        let (o, request) = scene_preview().await;
        let reader = key(&o, &o.s.ada, rules(&o, true, true)).await;
        let before = call(&o, &reader.secret, &request).await;
        assert_eq!(before.status, StatusCode::OK);
        let mut blocker = o.s.rig.db.pool().begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = o.s.rig.world.app.clone();
        let node = o.topic();
        let body = request.clone();
        let token = reader.secret.clone();
        let future = tokio::spawn(async move {
            let call = app.call(NodeImportPreview, &[&node]);
            call.token(&token).body(&body).send_bytes().await
        });
        activity_wait(&o, pid, &future).await;
        match mode {
            "physical_source_removed" => {
                o.s.rig
                    .world
                    .app
                    .app
                    .engine
                    .blobs()
                    .unwrap()
                    .delete(&request.source.files[0].blob)
                    .await
                    .unwrap();
            }
            "scope_closed" => {
                let scope: KeyScope =
                    serde_json::from_value(json!({"rules":rules(&o,true,false)})).unwrap();
                assert!(scope.validate().is_empty());
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(reader.id)
                    .bind(json!(scope))
                    .execute(o.s.rig.db.pool())
                    .await
                    .unwrap();
            }
            "custody_released" => {
                sqlx::query("DELETE FROM completed_uploads WHERE user_id=$1 AND hash=$2")
                    .bind(o.s.ada.id())
                    .bind(request.source.files[0].blob.as_bytes().as_slice())
                    .execute(o.s.rig.db.pool())
                    .await
                    .unwrap();
            }
            "expired" => o.s.rig.world.advance(chrono::Duration::hours(2)),
            _ => {}
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), future)
            .await
            .unwrap()
            .unwrap();
        if mode == "unchanged" || mode == "physical_source_removed" {
            assert_eq!(reply.status, StatusCode::OK);
            assert_eq!(
                reply.raw, before.raw,
                "publication retains actual opened source text without supplier IO"
            );
        } else if mode == "expired" {
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
        } else {
            assert!(reply.status.is_client_error());
        }
    }
}
