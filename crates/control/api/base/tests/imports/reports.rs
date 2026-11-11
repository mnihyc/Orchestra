//! Actual persisted report rows, their native position cursor and optional place references.

use super::*;
use orch_api_contract::common::Page;

const OPERATION: &str = "ImportReportList";

fn rules(o: &Office, id: Id, named: bool) -> Vec<Value> {
    let mut rows = vec![artifact(OPERATION, id)];
    if named {
        rows.push(rule(
            OPERATION,
            "org",
            o.org(),
            "subtree",
            &["org", "project", "topic", "thread"],
            &["read"],
        ));
    }
    rows
}

async fn call(o: &Office, token: &str, id: Id, query: &ImportReportListQuery) -> RawReply {
    let call = o.s.rig.world.app.call(ImportReportList, &[&id]);
    call.token(token).query(query).send_bytes().await
}

async fn shown(
    o: &Office,
    token: &str,
    id: Id,
    query: &ImportReportListQuery,
) -> Page<ImportReportRow> {
    let reply = call(o, token, id, query).await;
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

async fn stored_place_reference(o: &Office, import: &Import) {
    // Populate the native persisted report's optional reference with the real ordinary
    // import destination. Its source/file words and actual made-object identities stay.
    sqlx::query(
        "UPDATE import_report_rows SET row=jsonb_set(row,'{made,node}',$2)
         WHERE import_id=$1 AND row->'made' <> 'null'::jsonb",
    )
    .bind(import.id)
    .bind(json!(import.node))
    .execute(o.s.rig.db.pool())
    .await
    .unwrap();
}

#[tokio::test]
async fn import_report_keeps_saved_content_native_filters_and_cursor_with_independent_place_name_read()
 {
    let (o, import) = scene().await;
    stored_place_reference(&o, &import).await;
    let query = ImportReportListQuery::default();
    let native = shown(&o, &o.s.ada.token, import.id, &query).await;
    assert_eq!(native.items.len(), 2);
    assert!(native.items.iter().all(|row| {
        row.made
            .as_ref()
            .unwrap()
            .node
            .as_ref()
            .unwrap()
            .name
            .is_some()
    }));
    let reader = key(&o, &o.s.ada, rules(&o, import.id, true)).await;
    assert_eq!(shown(&o, &reader.secret, import.id, &query).await, native);
    let root = key(&o, &o.s.ada, rules(&o, import.id, false)).await;
    let mut expected = native.clone();
    for row in &mut expected.items {
        let node = row.made.as_mut().unwrap().node.as_mut().unwrap();
        node.name = None;
        node.redacted = true;
    }
    assert_eq!(shown(&o, &root.secret, import.id, &query).await, expected);
    assert_eq!(expected.items[0].name, native.items[0].name);
    assert_eq!(expected.items[0].path, native.items[0].path);
    let first_query = ImportReportListQuery {
        limit: Some(1),
        kind: Some(PackageKind::Document),
        outcome: Some(ImportOutcome::Created),
        ..Default::default()
    };
    let first = shown(&o, &reader.secret, import.id, &first_query).await;
    assert_eq!(
        first,
        shown(&o, &o.s.ada.token, import.id, &first_query).await
    );
    assert_eq!(first.items, vec![native.items[0].clone()]);
    assert_eq!(first.next_cursor, Some(first.items[0].position.to_string()));
    let second = shown(
        &o,
        &reader.secret,
        import.id,
        &ImportReportListQuery {
            cursor: first.next_cursor,
            ..first_query
        },
    )
    .await;
    assert_eq!(second.items, vec![native.items[1].clone()]);
    assert!(second.next_cursor.is_none());
    let invalid = call(
        &o,
        &reader.secret,
        import.id,
        &ImportReportListQuery {
            cursor: Some("invalid".into()),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(invalid.status, StatusCode::BAD_REQUEST);
    let problem: serde_json::Value = serde_json::from_slice(&invalid.raw).unwrap();
    assert_eq!(problem["code"], "request.invalid");
    assert_eq!(problem["errors"][0]["path"], "cursor");
}

#[tokio::test]
async fn import_report_requires_its_exact_operation_typed_artifact_and_native_creator_after_purge()
{
    let (o, import) = scene().await;
    let query = ImportReportListQuery::default();
    for selected in [
        artifact("ImportGet", import.id),
        rule(
            OPERATION,
            "export",
            import.id,
            "self",
            &["import"],
            &["read"],
        ),
        rule(
            OPERATION,
            "import",
            import.id,
            "self",
            &["import"],
            &["manage"],
        ),
        rule(
            OPERATION,
            "account",
            o.s.ada.id(),
            "self",
            &["import"],
            &["read"],
        ),
    ] {
        let denied = key(&o, &o.s.ada, vec![selected]).await;
        assert_eq!(
            call(&o, &denied.secret, import.id, &query).await.status,
            StatusCode::FORBIDDEN
        );
    }
    let other = key(&o, &o.s.cai, vec![artifact(OPERATION, import.id)]).await;
    assert_eq!(
        call(&o, &other.secret, import.id, &query).await.status,
        StatusCode::NOT_FOUND
    );
    let linked = key(
        &o,
        &o.s.ada,
        vec![rule(
            OPERATION,
            "org",
            o.org(),
            "subtree",
            &["import"],
            &["read"],
        )],
    )
    .await;
    assert_eq!(
        shown(&o, &linked.secret, import.id, &query).await,
        shown(&o, &o.s.ada.token, import.id, &query).await
    );
    sqlx::query("UPDATE imports SET node_id=NULL WHERE id=$1")
        .bind(import.id)
        .execute(o.s.rig.db.pool())
        .await
        .unwrap();
    let exact = key(&o, &o.s.ada, vec![artifact(OPERATION, import.id)]).await;
    assert_eq!(
        shown(&o, &exact.secret, import.id, &query).await,
        shown(&o, &o.s.ada.token, import.id, &query).await
    );
    assert_eq!(
        call(&o, &linked.secret, import.id, &query).await.status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn import_report_publication_rechecks_original_records_names_and_key_without_reopening_the_window()
 {
    for mode in ["unchanged", "appended", "replaced", "narrowed", "expired"] {
        let (o, import) = scene().await;
        stored_place_reference(&o, &import).await;
        let reader = key(&o, &o.s.ada, rules(&o, import.id, true)).await;
        let query = ImportReportListQuery::default();
        let before = shown(&o, &reader.secret, import.id, &query).await;
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
            let call = app.call(ImportReportList, &[&id]);
            call.token(&token).query(&query).send_bytes().await
        });
        activity_wait(&o, pid, &request).await;
        match mode {
            "replaced" => {
                sqlx::query("UPDATE import_report_rows SET row=jsonb_set(row,'{name}',to_jsonb('replacement-source.md'::text)) WHERE import_id=$1 AND position=$2")
                    .bind(id).bind(before.items[0].position).execute(o.s.rig.db.pool()).await.unwrap();
            }
            "appended" => {
                let mut row = before.items[0].clone();
                row.position = before.items.iter().map(|row| row.position).max().unwrap() + 1;
                row.source_id = new_id();
                orch_db::queries::imports::add_report_rows(
                    &mut *o.s.rig.world.conn().await,
                    id,
                    &[orch_engine::imports::stored_row(&row)],
                )
                .await
                .unwrap();
            }
            "narrowed" => {
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
            "expired" => o.s.rig.world.advance(chrono::Duration::hours(2)),
            _ => {}
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            reply.status,
            match mode {
                "replaced" | "narrowed" => StatusCode::FORBIDDEN,
                "expired" => StatusCode::UNAUTHORIZED,
                _ => StatusCode::OK,
            }
        );
        if mode == "unchanged" || mode == "appended" {
            let old: Page<ImportReportRow> = serde_json::from_slice(&reply.raw).unwrap();
            assert_eq!(old, before);
        }
        if mode == "appended" {
            assert_eq!(
                shown(&o, &reader.secret, id, &ImportReportListQuery::default())
                    .await
                    .items
                    .len(),
                3
            );
        } else if mode == "narrowed" {
            assert!(
                shown(&o, &reader.secret, id, &ImportReportListQuery::default())
                    .await
                    .items
                    .iter()
                    .all(|row| row
                        .made
                        .as_ref()
                        .unwrap()
                        .node
                        .as_ref()
                        .unwrap()
                        .name
                        .is_none())
            );
        }
    }
}
