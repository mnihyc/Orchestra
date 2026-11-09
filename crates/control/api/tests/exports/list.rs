//! Scope participates before the native archive page, look-ahead and cursor.

use super::*;
use orch_api_contract::{
    common::{Page, PageQuery},
    exports::MeExportList,
};

const OPERATION: &str = "MeExportList";

fn rules(ids: &[Id], node: Option<Id>) -> Vec<Value> {
    let mut rules = ids
        .iter()
        .map(|id| rule(OPERATION, "export", *id, "self", &["export"], &["read"]))
        .collect::<Vec<_>>();
    if let Some(node) = node {
        rules.push(rule(
            OPERATION,
            "org",
            node,
            "subtree",
            &["org", "project", "topic", "thread"],
            &["read"],
        ));
    }
    rules
}

async fn call(o: &Office, token: &str, query: &PageQuery) -> RawReply {
    let call = o.s.rig.world.app.call(MeExportList, &[]);
    call.token(token).query(query).send_bytes().await
}

async fn shown(o: &Office, token: &str, query: &PageQuery) -> Page<Export> {
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
async fn export_page_filters_before_native_window_lookahead_and_cursor_and_preserves_full_bodies() {
    let o = Office::new().await;
    for _ in 0..3 {
        o.made(&o.s.ada, o.org(), &everything()).await;
    }
    let native = o.list(&o.s.ada).await;
    assert_eq!(native.len(), 3);
    let ids = native.iter().map(|row| row.id).collect::<Vec<_>>();
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
    let selected = shown(&o, &oldest.secret, &query).await;
    assert_eq!(
        selected.items,
        vec![native[2].clone()],
        "two excluded newer rows cannot consume the one-row output window"
    );
    assert!(
        selected.next_cursor.is_none(),
        "scope-excluded look-ahead does not invent another page"
    );
    let two = key(&o, &o.s.ada, rules(&ids[1..], Some(o.org()))).await;
    let first = shown(&o, &two.secret, &query).await;
    assert_eq!(first.items, vec![native[1].clone()]);
    let second = shown(
        &o,
        &two.secret,
        &PageQuery {
            limit: Some(1),
            cursor: first.next_cursor.clone(),
        },
    )
    .await;
    assert!(first.next_cursor.is_some());
    assert_eq!(second.items, vec![native[2].clone()]);
    assert!(second.next_cursor.is_none());
    let empty = key(&o, &o.s.ada, rules(&[new_id()], None)).await;
    let page = shown(&o, &empty.secret, &query).await;
    assert!(page.items.is_empty() && page.next_cursor.is_none());
    let unrelated = key(&o, &o.s.ada, vec![artifact(ids[0])]).await;
    assert_eq!(
        call(&o, &unrelated.secret, &query).await.status,
        StatusCode::FORBIDDEN,
        "ExportGet cannot lend the list operation's Read"
    );
}

#[tokio::test]
async fn export_page_uses_real_owner_rows_and_current_links_without_creator_account_ancestry() {
    let o = Office::new().await;
    let first = o.made(&o.s.ada, o.org(), &everything()).await;
    let second = o.made(&o.s.ada, o.org(), &everything()).await;
    let foreign = orch_db::queries::exports::insert(
        &mut *o.s.rig.world.conn().await,
        new_id(),
        o.s.cai.id(),
        o.org(),
        &json!(everything()),
        o.s.rig.world.now(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE exports SET node_id=NULL WHERE id=$1")
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
    let page = shown(&o, &reader.secret, &PageQuery::default()).await;
    assert_eq!(page.items.len(), 2);
    assert!(
        !page.items.iter().any(|row| row.id == foreign.id),
        "scope cannot supply somebody else's ordinary archive custody"
    );
    assert!(
        page.items
            .iter()
            .find(|row| row.id == first.id)
            .unwrap()
            .node
            .is_none()
    );
    assert!(
        page.items
            .iter()
            .find(|row| row.id == second.id)
            .unwrap()
            .node
            .as_ref()
            .unwrap()
            .name
            .is_none()
    );
    let subtree = key(
        &o,
        &o.s.ada,
        vec![rule(
            OPERATION,
            "org",
            o.org(),
            "subtree",
            &["export"],
            &["read"],
        )],
    )
    .await;
    let page = shown(&o, &subtree.secret, &PageQuery::default()).await;
    assert_eq!(
        page.items.iter().map(|row| row.id).collect::<Vec<_>>(),
        vec![second.id]
    );
    let fake = key(
        &o,
        &o.s.ada,
        vec![rule(
            OPERATION,
            "account",
            o.s.ada.id(),
            "self",
            &["export"],
            &["read"],
        )],
    )
    .await;
    assert!(
        shown(&o, &fake.secret, &PageQuery::default())
            .await
            .items
            .is_empty(),
        "the real empty scope selection does not invent creator Account ancestry"
    );
}

#[tokio::test]
async fn export_page_publication_cannot_replace_an_old_selected_artifact_with_another_allowed_row()
{
    for mode in ["unchanged", "narrowed", "reordered"] {
        let o = Office::new().await;
        for _ in 0..2 {
            o.made(&o.s.ada, o.org(), &everything()).await;
        }
        let native = o.list(&o.s.ada).await;
        let ids = native.iter().map(|row| row.id).collect::<Vec<_>>();
        let reader = key(&o, &o.s.ada, rules(&ids, Some(o.org()))).await;
        let query = PageQuery {
            limit: Some(1),
            cursor: None,
        };
        let before = shown(&o, &reader.secret, &query).await;
        assert_eq!(before.items, vec![native[0].clone()]);
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
            let call = app.call(MeExportList, &[]);
            call.token(&token).query(&query).send_bytes().await
        });
        activity_wait(&o, pid, &request).await;
        if mode == "narrowed" {
            let scope: KeyScope =
                serde_json::from_value(json!({"rules":rules(&ids[1..], Some(o.org()))})).unwrap();
            assert!(scope.validate().is_empty());
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(reader.id)
                .bind(json!(scope))
                .execute(o.s.rig.db.pool())
                .await
                .unwrap();
        } else if mode == "reordered" {
            sqlx::query("UPDATE exports SET created_at=created_at+interval '1 day' WHERE id=$1")
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
        if mode == "narrowed" {
            let fresh = shown(
                &o,
                &reader.secret,
                &PageQuery {
                    limit: Some(1),
                    cursor: None,
                },
            )
            .await;
            assert_eq!(
                fresh.items,
                vec![native[1].clone()],
                "a fresh native scoped page remains useful"
            );
        } else {
            let old: Page<Export> = serde_json::from_slice(&reply.raw).unwrap();
            assert_eq!(old, before, "publication retains the original native page");
            if mode == "reordered" {
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
}
