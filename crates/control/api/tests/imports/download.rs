//! Real streamed report headers, page waits, original sources and released completed pages.

use super::*;
use futures::StreamExt as _;
use orch_api_contract::imports::{
    ImportReportDownload, ImportReportDownloadQuery, ImportReportFormat,
};

const OPERATION: &str = "ImportReportDownload";

fn rules(o: &Office, id: Id, named: bool) -> Vec<Value> {
    let mut values = vec![artifact(OPERATION, id)];
    if named {
        values.push(rule(
            OPERATION,
            "org",
            o.org(),
            "subtree",
            &["org", "project", "topic", "thread"],
            &["read"],
        ));
    }
    values
}

async fn call(o: &Office, token: &str, id: Id, format: ImportReportFormat) -> RawReply {
    let call = o.s.rig.world.app.call(ImportReportDownload, &[&id]);
    call.token(token)
        .query(&ImportReportDownloadQuery {
            format: Some(format),
        })
        .send_bytes()
        .await
}

#[tokio::test]
async fn report_download_keeps_native_json_csv_headers_creator_and_exact_operation_without_live_name_read()
 {
    let (o, import) = scene().await;
    let reader = key(&o, &o.s.ada, rules(&o, import.id, true)).await;
    for format in [ImportReportFormat::Json, ImportReportFormat::Csv] {
        let native = call(&o, &o.s.ada.token, import.id, format).await;
        let scoped = call(&o, &reader.secret, import.id, format).await;
        assert_eq!(native.status, StatusCode::OK);
        assert_eq!(
            scoped.status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&scoped.raw)
        );
        assert_eq!(scoped.raw, native.raw);
        for name in [
            "content-type",
            "content-disposition",
            "cache-control",
            "x-content-type-options",
        ] {
            assert_eq!(scoped.headers[name], native.headers[name]);
        }
        assert_eq!(scoped.headers["cache-control"], "private, no-store");
        if format == ImportReportFormat::Json {
            let body: Value = serde_json::from_slice(&scoped.raw).unwrap();
            assert_eq!(body["import"], json!(import));
            assert_eq!(body["rows"], json!(report(&o, &o.s.ada, import.id).await));
        } else {
            assert_eq!(
                String::from_utf8(scoped.raw.to_vec()).unwrap(),
                orch_engine::imports::csv_of(&report(&o, &o.s.ada, import.id).await)
            );
        }
    }
    let root = key(&o, &o.s.ada, rules(&o, import.id, false)).await;
    let reply = call(&o, &root.secret, import.id, ImportReportFormat::Json).await;
    assert_eq!(reply.status, StatusCode::OK);
    let body: Value = serde_json::from_slice(&reply.raw).unwrap();
    assert!(body["import"]["node"]["name"].is_null());
    assert_eq!(body["import"]["node"]["redacted"], true);
    assert_eq!(body["rows"], json!(report(&o, &o.s.ada, import.id).await));
    let wrong = key(&o, &o.s.ada, vec![artifact("ImportReportList", import.id)]).await;
    assert_eq!(
        call(&o, &wrong.secret, import.id, ImportReportFormat::Csv)
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let other = key(&o, &o.s.cai, vec![artifact(OPERATION, import.id)]).await;
    assert_eq!(
        call(&o, &other.secret, import.id, ImportReportFormat::Json)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn report_download_releases_delivered_native_pages_before_the_next_piece() {
    let (o, import) = scene().await;
    let original = report(&o, &o.s.ada, import.id).await;
    // One record beyond the native 1000-record report page proves that a delivered
    // page does not remain in a growing request history and gate unrelated later bytes.
    let rows = (0..1_001)
        .map(|position| {
            let mut row = original[0].clone();
            row.position = position;
            row.source_id = new_id();
            orch_engine::imports::stored_row(&row)
        })
        .collect::<Vec<_>>();
    orch_db::queries::imports::add_report_rows(&mut *o.s.rig.world.conn().await, import.id, &rows)
        .await
        .unwrap();
    let reader = key(&o, &o.s.ada, rules(&o, import.id, true)).await;
    let call = o.s.rig.world.app.call(ImportReportDownload, &[&import.id]);
    let response = call
        .token(&reader.secret)
        .query(&ImportReportDownloadQuery {
            format: Some(ImportReportFormat::Json),
        })
        .send_response()
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    let head = body.next().await.unwrap().unwrap();
    assert!(String::from_utf8_lossy(&head).contains("\"rows\":["));
    let first = body.next().await.unwrap().unwrap();
    let first = serde_json::from_slice::<Vec<ImportReportRow>>(
        &[b"[".as_slice(), first.as_ref(), b"]".as_slice()].concat(),
    )
    .unwrap();
    assert_eq!(first.len(), 1_000);
    assert_eq!(first[0].position, 0);
    assert_eq!(first.last().unwrap().position, 999);
    sqlx::query("UPDATE import_report_rows SET row=jsonb_set(row,'{name}',to_jsonb('changed-after-delivery.md'::text)) WHERE import_id=$1 AND position=0")
        .bind(import.id).execute(o.s.rig.db.pool()).await.unwrap();
    let next = body.next().await.unwrap().unwrap();
    let text = std::str::from_utf8(&next).unwrap();
    let last: ImportReportRow = serde_json::from_str(text.strip_prefix(',').unwrap()).unwrap();
    assert_eq!(
        last.position, 1_000,
        "only the newly selected native page remains a delivery source"
    );
    assert_eq!(body.next().await.unwrap().unwrap(), b"]}".as_slice());
    assert!(body.next().await.is_none());
}

#[tokio::test]
async fn report_download_keeps_the_actual_key_through_a_native_page_query_wait() {
    for expire in [false, true] {
        let (o, import) = scene().await;
        let reader = key(&o, &o.s.ada, rules(&o, import.id, true)).await;
        let call = o.s.rig.world.app.call(ImportReportDownload, &[&import.id]);
        let response = call
            .token(&reader.secret)
            .query(&ImportReportDownloadQuery {
                format: Some(ImportReportFormat::Json),
            })
            .send_response()
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        let mut body = response.into_body().into_data_stream();
        assert!(body.next().await.unwrap().is_ok());
        let mut blocker = o.s.rig.db.pool().begin().await.unwrap();
        sqlx::query("LOCK TABLE import_report_rows IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let next = tokio::spawn(async move { body.next().await });
        tokio::time::timeout(std::time::Duration::from_secs(30),async {
            loop {
                assert!(!next.is_finished(),"the actual native next-page query must wait");
                let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%FROM import_report_rows%')")
                    .bind(pid).fetch_one(o.s.rig.db.pool()).await.unwrap();
                if waiting {break;} tokio::task::yield_now().await;
            }
        }).await.expect("the actual native report page SELECT wait");
        if expire {
            o.s.rig.world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let bytes = tokio::time::timeout(std::time::Duration::from_secs(30), next)
            .await
            .unwrap()
            .unwrap()
            .expect("the native page or its terminal source failure");
        assert_eq!(
            bytes.is_err(),
            expire,
            "expired original keys expose no new report page bytes"
        );
    }
}

#[tokio::test]
async fn report_download_publication_keeps_the_original_header_source_after_activity_waits() {
    for mode in ["unchanged", "replaced", "narrowed"] {
        let (o, import) = scene().await;
        let reader = key(&o, &o.s.ada, rules(&o, import.id, true)).await;
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
        let id = import.id;
        let token = reader.secret.clone();
        let request = tokio::spawn(async move {
            let call = app.call(ImportReportDownload, &[&id]);
            call.token(&token)
                .query(&ImportReportDownloadQuery {
                    format: Some(ImportReportFormat::Json),
                })
                .send_bytes()
                .await
        });
        activity_wait(&o, pid, &request).await;
        if mode == "replaced" {
            sqlx::query("UPDATE imports SET request=jsonb_set(request,'{client_id}',to_jsonb('replacement-request'::text)) WHERE id=$1")
                .bind(id).execute(o.s.rig.db.pool()).await.unwrap();
        } else if mode == "narrowed" {
            let scope: KeyScope =
                serde_json::from_value(json!({"rules":rules(&o,id,false)})).unwrap();
            assert!(scope.validate().is_empty());
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(reader.id)
                .bind(json!(scope))
                .execute(o.s.rig.db.pool())
                .await
                .unwrap();
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            reply.status,
            if mode == "unchanged" {
                StatusCode::OK
            } else {
                StatusCode::FORBIDDEN
            }
        );
        if mode == "narrowed" {
            let fresh = call(&o, &reader.secret, id, ImportReportFormat::Json).await;
            assert_eq!(fresh.status, StatusCode::OK);
            let value: Value = serde_json::from_slice(&fresh.raw).unwrap();
            assert!(value["import"]["node"]["name"].is_null());
        }
    }
}
