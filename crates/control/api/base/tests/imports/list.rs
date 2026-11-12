//! Scope participates in the native narrow Import history before its output window.

use super::*;
use orch_api_contract::{common::Page, imports::ImportLine};

const OPERATION: &str = "MeImportList";

fn rules(ids: &[Id], node: Option<Id>) -> Vec<Value> {
    let mut selected = ids
        .iter()
        .map(|id| rule(OPERATION, "import", *id, "self", &["import"], &["read"]))
        .collect::<Vec<_>>();
    if let Some(node) = node {
        selected.push(rule(
            OPERATION,
            "org",
            node,
            "subtree",
            &["org", "project", "topic", "thread"],
            &["read"],
        ));
    }
    selected
}

async fn make(o: &Office, client: &str) -> Import {
    let file = upload(o, &o.s.ada, &format!("{client}.md"), b"# History source\n").await;
    imported(o, &o.s.ada, o.topic(), &asked(client, &file)).await
}

async fn call(o: &Office, token: &str, query: &PageQuery) -> RawReply {
    let call = o.s.rig.world.app.call(MeImportList, &[]);
    call.token(token).query(query).send_bytes().await
}

async fn shown(o: &Office, token: &str, query: &PageQuery) -> Page<ImportLine> {
    let reply = call(o, token, query).await;
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

#[tokio::test]
async fn import_history_filters_before_native_window_lookahead_and_cursor_with_full_native_lines() {
    let o = Office::new().await;
    for name in ["history-a", "history-b", "history-c"] {
        make(&o, name).await;
    }
    let native = shown(&o, &o.s.ada.token, &PageQuery::default()).await;
    assert_eq!(native.items.len(), 3);
    let ids = native.items.iter().map(|row| row.id).collect::<Vec<_>>();
    let all = key(&o, &o.s.ada, rules(&ids, Some(o.org()))).await;
    let query = PageQuery {
        limit: Some(1),
        cursor: None,
    };
    assert_eq!(
        shown(&o, &all.secret, &query).await,
        shown(&o, &o.s.ada.token, &query).await
    );
    let oldest = key(&o, &o.s.ada, rules(&ids[2..], Some(o.org()))).await;
    let last = shown(&o, &oldest.secret, &query).await;
    assert_eq!(
        last.items,
        vec![native.items[2].clone()],
        "excluded newer Imports do not consume the one-row scope window"
    );
    assert!(last.next_cursor.is_none());
    let two = key(&o, &o.s.ada, rules(&ids[1..], Some(o.org()))).await;
    let first = shown(&o, &two.secret, &query).await;
    assert_eq!(first.items, vec![native.items[1].clone()]);
    assert!(first.next_cursor.is_some());
    let next = shown(
        &o,
        &two.secret,
        &PageQuery {
            limit: Some(1),
            cursor: first.next_cursor,
        },
    )
    .await;
    assert_eq!(next.items, vec![native.items[2].clone()]);
    assert!(next.next_cursor.is_none());
    let empty = key(&o, &o.s.ada, rules(&[new_id()], None)).await;
    let empty = shown(&o, &empty.secret, &query).await;
    assert!(empty.items.is_empty() && empty.next_cursor.is_none());
    let wrong = key(&o, &o.s.ada, vec![artifact("ImportGet", ids[0])]).await;
    assert_eq!(
        call(&o, &wrong.secret, &query).await.status,
        StatusCode::FORBIDDEN
    );
    let invalid = call(
        &o,
        &all.secret,
        &PageQuery {
            limit: Some(1),
            cursor: Some("invalid".into()),
        },
    )
    .await;
    assert_eq!(invalid.status, StatusCode::BAD_REQUEST);
    let problem: serde_json::Value = serde_json::from_slice(&invalid.raw).unwrap();
    assert_eq!(problem["code"], "request.invalid");
    assert_eq!(problem["errors"][0]["path"], "cursor");
}

#[tokio::test]
async fn import_history_keeps_native_creator_custody_and_real_link_or_absence_without_account_ancestry()
 {
    let o = Office::new().await;
    let first = make(&o, "history-purged").await;
    let second = make(&o, "history-linked").await;
    let foreign = make(&o, "history-foreign").await;
    sqlx::query("UPDATE imports SET user_id=$2 WHERE id=$1")
        .bind(foreign.id)
        .bind(o.s.cai.id())
        .execute(o.s.rig.db.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE imports SET node_id=NULL WHERE id=$1")
        .bind(first.id)
        .execute(o.s.rig.db.pool())
        .await
        .unwrap();
    let reader = key(
        &o,
        &o.s.ada,
        rules(&[first.id, second.id, foreign.id], None),
    )
    .await;
    let result = shown(&o, &reader.secret, &PageQuery::default()).await;
    assert_eq!(result.items.len(), 2);
    assert!(!result.items.iter().any(|row| row.id == foreign.id));
    assert!(
        result
            .items
            .iter()
            .find(|row| row.id == first.id)
            .unwrap()
            .node
            .is_none()
    );
    assert!(
        result
            .items
            .iter()
            .find(|row| row.id == second.id)
            .unwrap()
            .node
            .as_ref()
            .unwrap()
            .name
            .is_none()
    );
    for (kind, id, reach, expected) in [
        ("org", o.org(), "subtree", vec![second.id]),
        ("account", o.s.ada.id(), "self", Vec::new()),
    ] {
        let reader = key(
            &o,
            &o.s.ada,
            vec![rule(OPERATION, kind, id, reach, &["import"], &["read"])],
        )
        .await;
        assert_eq!(
            shown(&o, &reader.secret, &PageQuery::default())
                .await
                .items
                .iter()
                .map(|row| row.id)
                .collect::<Vec<_>>(),
            expected
        );
    }
}

#[tokio::test]
async fn import_history_publication_preserves_old_rows_after_native_window_changes_and_refuses_scope_loss()
 {
    for mode in ["unchanged", "newer", "narrowed"] {
        let o = Office::new().await;
        make(&o, "history-old-a").await;
        make(&o, "history-old-b").await;
        let native = shown(&o, &o.s.ada.token, &PageQuery::default()).await;
        let ids = native.items.iter().map(|row| row.id).collect::<Vec<_>>();
        let reader = key(
            &o,
            &o.s.ada,
            vec![rule(
                OPERATION,
                "org",
                o.org(),
                "subtree",
                &["import", "org", "project", "topic", "thread"],
                &["read"],
            )],
        )
        .await;
        let query = PageQuery {
            limit: Some(1),
            cursor: None,
        };
        let before = shown(&o, &reader.secret, &query).await;
        assert_eq!(before.items, vec![native.items[0].clone()]);
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
        let token = reader.secret.clone();
        let request = tokio::spawn(async move {
            let call = app.call(MeImportList, &[]);
            call.token(&token).query(&query).send_bytes().await
        });
        activity_wait(&o, pid, &request).await;
        if mode == "narrowed" {
            let scope: KeyScope =
                serde_json::from_value(json!({"rules":rules(&ids[1..],Some(o.org()))})).unwrap();
            assert!(scope.validate().is_empty());
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(reader.id)
                .bind(json!(scope))
                .execute(o.s.rig.db.pool())
                .await
                .unwrap();
        } else if mode == "newer" {
            // Native history already uses this real row's stored creation time in its ordering.
            // Change the other existing import's native position while the old response waits.
            sqlx::query("UPDATE imports SET created_at=created_at+interval '1 day' WHERE id=$1")
                .bind(ids[1])
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
            if mode == "narrowed" {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::OK
            }
        );
        if mode != "narrowed" {
            let old: Page<ImportLine> = serde_json::from_slice(&reply.raw).unwrap();
            assert_eq!(old, before, "publication must retain the old selected body");
        }
        if mode != "unchanged" {
            let current = shown(
                &o,
                &reader.secret,
                &PageQuery {
                    limit: Some(1),
                    cursor: None,
                },
            )
            .await;
            assert_eq!(current.items[0].id, ids[1]);
        }
    }
}
