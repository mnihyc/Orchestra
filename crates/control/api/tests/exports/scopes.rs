//! The real archive identity, retained creator custody and current optional name sources.

use super::*;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::exports::{Export, ExportGet};
use orch_api_core::{
    middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash},
    testing::RawReply,
};
use orch_db::queries::identity::{self, NewApiToken};

const OPERATION: &str = "ExportGet";

#[path = "list.rs"]
mod page;

#[path = "removal.rs"]
mod removal;

#[path = "download.rs"]
mod bytes;

struct Key {
    id: Id,
    secret: String,
}

fn rule(
    operation: &str,
    kind: &str,
    id: Id,
    reach: &str,
    resources: &[&str],
    actions: &[&str],
) -> Value {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"resources":resources,
        "allow":[{"operation":operation,"actions":actions}]})
}

fn artifact(export: Id) -> Value {
    rule(OPERATION, "export", export, "self", &["export"], &["read"])
}

fn names(node: Id) -> Value {
    rule(
        OPERATION,
        "org",
        node,
        "subtree",
        &["org", "project", "topic", "thread"],
        &["read"],
    )
}

async fn key(o: &Office, actor: &places::Who, rules: Vec<Value>) -> Key {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    let world = &o.s.rig.world;
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id,
            user_id: actor.id(),
            name: "Retained archive metadata fixture",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    Key { id, secret }
}

async fn call(o: &Office, secret: &str, id: Id) -> RawReply {
    let call = o.s.rig.world.app.call(ExportGet, &[&id]);
    call.token(secret).send_bytes().await
}

async fn shown(o: &Office, secret: &str, id: Id) -> Export {
    let reply = call(o, secret, id).await;
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

#[tokio::test]
async fn export_metadata_uses_the_actual_artifact_and_separately_bounds_native_current_place_names()
{
    let o = Office::new().await;
    let export = o.made(&o.s.ada, o.org(), &everything()).await;
    let full = key(&o, &o.s.ada, vec![artifact(export.id), names(o.org())]).await;
    assert_eq!(shown(&o, &full.secret, export.id).await, export);
    let only_artifact = key(&o, &o.s.ada, vec![artifact(export.id)]).await;
    let mut expected = export.clone();
    let node = expected
        .node
        .as_mut()
        .expect("a real linked native archive");
    assert!(node.name.is_some());
    node.name = None;
    node.redacted = true;
    assert_eq!(shown(&o, &only_artifact.secret, export.id).await, expected);
    for rules in [
        vec![rule(
            "MeExportList",
            "export",
            export.id,
            "self",
            &["export"],
            &["read"],
        )],
        vec![rule(
            OPERATION,
            "export",
            export.id,
            "self",
            &["export"],
            &["manage"],
        )],
        vec![rule(
            OPERATION,
            "import",
            export.id,
            "self",
            &["export"],
            &["read"],
        )],
        vec![names(o.org())],
        vec![rule(
            OPERATION,
            "account",
            o.s.ada.id(),
            "self",
            &["export"],
            &["read"],
        )],
    ] {
        let denied = key(&o, &o.s.ada, rules).await;
        assert_eq!(
            call(&o, &denied.secret, export.id).await.status,
            StatusCode::FORBIDDEN
        );
    }
    let other = key(&o, &o.s.cai, vec![artifact(export.id), names(o.org())]).await;
    assert_eq!(
        call(&o, &other.secret, export.id).await.status,
        StatusCode::NOT_FOUND,
        "artifact scope never supplies another person's private archive custody"
    );
}

#[tokio::test]
async fn export_metadata_keeps_exact_artifact_custody_after_purge_without_old_or_account_ancestry()
{
    let o = Office::new().await;
    let export = o.made(&o.s.ada, o.org(), &everything()).await;
    let linked = key(
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
    assert_eq!(
        shown(&o, &linked.secret, export.id).await.id,
        export.id,
        "the actual linked current placement supplies subtree reach before purge"
    );
    // The existing archive custody fixture uses the exact nullable-FK result of place purge.
    sqlx::query("UPDATE exports SET node_id=NULL WHERE id=$1")
        .bind(export.id)
        .execute(o.s.rig.db.pool())
        .await
        .unwrap();
    let native = o.read(&o.s.ada, export.id).await.ok();
    assert!(native.node.is_none());
    let exact = key(&o, &o.s.ada, vec![artifact(export.id)]).await;
    assert_eq!(shown(&o, &exact.secret, export.id).await, native);
    assert_eq!(
        call(&o, &linked.secret, export.id).await.status,
        StatusCode::FORBIDDEN,
        "request wording and creator membership cannot recreate purged place ancestry"
    );
    let account = key(
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
    assert_eq!(
        call(&o, &account.secret, export.id).await.status,
        StatusCode::FORBIDDEN
    );
}

async fn activity_wait(o: &Office, pid: i32, request: &tokio::task::JoinHandle<RawReply>) {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            assert!(!request.is_finished(), "the actual native archive metadata must reach final activity");
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                .bind(pid).fetch_one(o.s.rig.db.pool()).await.unwrap();
            if blocked { break; } tokio::task::yield_now().await;
        }
    }).await.expect("the exact native archive metadata activity INSERT wait");
}

#[tokio::test]
async fn export_metadata_publication_keeps_the_original_link_and_key_deadline_after_activity_waits()
{
    for mode in ["unchanged", "relinked", "expired"] {
        let o = Office::new().await;
        let export = o.made(&o.s.ada, o.org(), &everything()).await;
        let other = places::make_org(
            &o.s.rig.world,
            &o.s.ada,
            "Other archive place",
            "other-archive",
        )
        .await
        .base
        .id;
        let reader = key(
            &o,
            &o.s.ada,
            vec![artifact(export.id), names(o.org()), names(other)],
        )
        .await;
        assert_eq!(shown(&o, &reader.secret, export.id).await, export);
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
        let secret = reader.secret.clone();
        let id = export.id;
        let request = tokio::spawn(async move {
            let call = app.call(ExportGet, &[&id]);
            call.token(&secret).send_bytes().await
        });
        activity_wait(&o, pid, &request).await;
        match mode {
            "relinked" => {
                sqlx::query("UPDATE exports SET node_id=$2 WHERE id=$1")
                    .bind(id)
                    .bind(other)
                    .execute(o.s.rig.db.pool())
                    .await
                    .unwrap();
                assert_eq!(o.read(&o.s.ada, id).await.ok().node.unwrap().id, other);
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
                "relinked" => StatusCode::FORBIDDEN,
                "expired" => StatusCode::UNAUTHORIZED,
                _ => StatusCode::OK,
            }
        );
    }
}

#[tokio::test]
async fn export_metadata_publication_cannot_replace_original_place_read_with_fresh_redaction() {
    for narrow in [false, true] {
        let o = Office::new().await;
        let export = o.made(&o.s.ada, o.org(), &everything()).await;
        let reader = key(&o, &o.s.ada, vec![artifact(export.id), names(o.org())]).await;
        assert_eq!(shown(&o, &reader.secret, export.id).await, export);
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
        let id = export.id;
        let request = tokio::spawn(async move {
            let call = app.call(ExportGet, &[&id]);
            call.token(&token).send_bytes().await
        });
        activity_wait(&o, pid, &request).await;
        if narrow {
            let scope: KeyScope = serde_json::from_value(json!({"rules":[artifact(id)]})).unwrap();
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
            if narrow {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::OK
            }
        );
        if narrow {
            let current = shown(&o, &reader.secret, id).await;
            assert!(
                current.node.as_ref().unwrap().name.is_none(),
                "a fresh request remains usable with its genuine smaller native form"
            );
        }
    }
}

#[path = "planning.rs"]
mod planning;
