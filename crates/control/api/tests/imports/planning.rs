//! The actual archive, matched conflict rows and accounts form one native planning read.

use super::*;

const OP: &str = "NodeImportPreflight";

fn destination(o: &Office) -> Value {
    rule(OP, "topic", o.topic(), "self", &["topic"], &["read"])
}
fn source(o: &Office) -> Value {
    rule(OP, "account", o.s.ada.id(), "self", &["upload"], &["read"])
}
fn document(o: &Office) -> Value {
    rule(
        OP,
        "document",
        o.document.base.id,
        "self",
        &["document"],
        &["read"],
    )
}
async fn call(o: &Office, token: &str, request: &ImportPreflightRequest) -> RawReply {
    let node = o.topic();
    let call = o.s.rig.world.app.call(NodeImportPreflight, &[&node]);
    call.token(token).body(request).send_bytes().await
}
fn read(reply: RawReply) -> ImportPlan {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn plain(o: &Office, name: &str) -> ImportPreflightRequest {
    let file = upload(o, &o.s.ada, name, b"# Import planning source\n").await;
    ImportPreflightRequest {
        request: asked("scoped-source-plan", &file),
        view: ImportView::default(),
    }
}

#[tokio::test]
async fn preflight_needs_its_own_read_of_the_actual_destination_and_byte_witness_and_creates_nothing()
 {
    let o = Office::new().await;
    let body = plain(&o, "new-planning-source.md").await;
    let before: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM imports),(SELECT count(*) FROM jobs),(SELECT count(*) FROM documents)")
        .fetch_one(o.s.rig.db.pool()).await.unwrap();
    let native = read(call(&o, &o.s.ada.token, &body).await);
    assert_eq!(native.conflicts, 0);
    let full = key(&o, &o.s.ada, vec![destination(&o), source(&o)]).await;
    assert_eq!(read(call(&o, &full.secret, &body).await), native);
    for rules in [
        vec![destination(&o)],
        vec![source(&o)],
        vec![
            rule(
                "NodeImportPreview",
                "topic",
                o.topic(),
                "self",
                &["topic"],
                &["read"],
            ),
            source(&o),
        ],
        vec![
            rule(OP, "topic", o.topic(), "self", &["topic"], &["write"]),
            source(&o),
        ],
        vec![
            rule(OP, "thread", o.topic(), "self", &["thread"], &["read"]),
            source(&o),
        ],
    ] {
        let denied = key(&o, &o.s.ada, rules).await;
        let reply = call(&o, &denied.secret, &body).await;
        assert!(reply.status.is_client_error());
        assert!(!String::from_utf8_lossy(&reply.raw).contains("Import planning source"));
    }
    assert_eq!(sqlx::query_as::<_, (i64, i64, i64)>("SELECT (SELECT count(*) FROM imports),(SELECT count(*) FROM jobs),(SELECT count(*) FROM documents)")
        .fetch_one(o.s.rig.db.pool()).await.unwrap(), before);
}

#[tokio::test]
async fn conflict_counts_and_existing_names_need_this_operations_actual_live_document_read() {
    let o = Office::new().await;
    let body = plain(&o, "Handbook.md").await;
    let native = read(call(&o, &o.s.ada.token, &body).await);
    assert_eq!(native.conflicts, 1);
    assert_eq!(
        native.objects.items[0].existing.as_ref().unwrap().id,
        o.document.base.id
    );
    let full = key(
        &o,
        &o.s.ada,
        vec![destination(&o), source(&o), document(&o)],
    )
    .await;
    assert_eq!(read(call(&o, &full.secret, &body).await), native);
    for extra in [
        Vec::new(),
        vec![rule(
            "DocumentGet",
            "document",
            o.document.base.id,
            "self",
            &["document"],
            &["read"],
        )],
        vec![rule(
            OP,
            "document_container",
            o.document.base.id,
            "self",
            &["document_container"],
            &["read"],
        )],
    ] {
        let mut rules = vec![destination(&o), source(&o)];
        rules.extend(extra);
        let denied = key(&o, &o.s.ada, rules).await;
        let reply = call(&o, &denied.secret, &body).await;
        assert!(reply.status.is_client_error());
        let text = String::from_utf8_lossy(&reply.raw);
        assert!(!text.contains(&o.document.base.id.to_string()));
        assert!(!text.contains("Handbook"));
    }
}

#[tokio::test]
async fn package_plans_bind_actual_matched_accounts_and_preserve_native_pages_and_custody() {
    let o = Office::new().await;
    let bytes = package_of(&o, o.topic()).await;
    let file = upload(&o, &o.s.ada, "planning-package.zip", &bytes).await;
    let body = ImportPreflightRequest {
        request: asked("package-plan", &file),
        view: ImportView {
            limit: Some(1),
            ..Default::default()
        },
    };
    let native = read(call(&o, &o.s.ada.token, &body).await);
    assert_eq!(native.objects.items.len(), 1);
    assert!(native.objects.next_cursor.is_some());
    let mut rules = vec![
        source(&o),
        rule(
            OP,
            "topic",
            o.topic(),
            "subtree",
            &["topic", "thread", "document", "document_container", "chat"],
            &["read"],
        ),
    ];
    let account_ids: std::collections::BTreeSet<_> = native
        .people
        .iter()
        .filter_map(|person| person.user.as_ref().map(|user| user.id))
        .collect();
    assert!(!account_ids.is_empty());
    let missing_accounts = key(&o, &o.s.ada, rules.clone()).await;
    assert!(
        call(&o, &missing_accounts.secret, &body)
            .await
            .status
            .is_client_error()
    );
    for id in account_ids {
        rules.push(rule(OP, "account", id, "self", &["account"], &["read"]));
    }
    let full = key(&o, &o.s.ada, rules).await;
    assert_eq!(read(call(&o, &full.secret, &body).await), native);
    let foreign = upload(
        &o,
        &o.s.cai,
        "foreign-planning.md",
        b"# Foreign private source\n",
    )
    .await;
    let mut foreign_body = body.clone();
    foreign_body.request.source.files = vec![foreign];
    let reply = call(&o, &full.secret, &foreign_body).await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    assert!(!String::from_utf8_lossy(&reply.raw).contains("Foreign private source"));
}

#[tokio::test]
async fn preflight_publication_retains_the_opened_archive_and_original_conflicts_under_current_scope()
 {
    for mode in [
        "unchanged",
        "supplier_removed",
        "unrelated_append",
        "scope_closed",
        "custody_released",
        "moved",
        "expired",
    ] {
        let o = Office::new().await;
        let body = plain(&o, "Handbook.md").await;
        let reader = key(
            &o,
            &o.s.ada,
            vec![destination(&o), source(&o), document(&o)],
        )
        .await;
        let before = read(call(&o, &reader.secret, &body).await);
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
        let request = body.clone();
        let token = reader.secret.clone();
        let future = tokio::spawn(async move {
            let call = app.call(NodeImportPreflight, &[&node]);
            call.token(&token).body(&request).send_bytes().await
        });
        activity_wait(&o, pid, &future).await;
        match mode {
            "supplier_removed" => {
                o.s.rig
                    .world
                    .app
                    .app
                    .engine
                    .blobs()
                    .unwrap()
                    .delete(&body.request.source.files[0].blob)
                    .await
                    .unwrap()
            }
            "unrelated_append" => {
                Office::write_document(&o.s, &o.s.ada, "Added after selection", "new").await;
            }
            "scope_closed" => {
                let scope: KeyScope =
                    serde_json::from_value(json!({"rules":[destination(&o),source(&o)]})).unwrap();
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
                    .bind(body.request.source.files[0].blob.as_bytes().as_slice())
                    .execute(o.s.rig.db.pool())
                    .await
                    .unwrap();
            }
            "moved" => {
                sqlx::query("UPDATE documents SET node_id=$2 WHERE id=$1")
                    .bind(o.document.base.id)
                    .bind(o.s.tree.project.base.id)
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
        match mode {
            "unchanged" | "supplier_removed" | "unrelated_append" => {
                assert_eq!(read(reply), before)
            }
            "expired" => assert_eq!(reply.status, StatusCode::UNAUTHORIZED),
            _ => assert!(reply.status.is_client_error()),
        }
    }
}
