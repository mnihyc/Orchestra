//! Composite authority keeps its own account, organization sources and current native standing.

use super::{checked, key, wait_for_activity};
use crate::{places, support::World, usage_world::team};
use http::StatusCode;
use orch_api_contract::auth::MeAuthorityGet;
use orch_types::{Id, Json};
use serde_json::json;

pub(super) fn rule(operation: &str, kind: &str, id: Id) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":[kind],
        "allow":[{"operation":operation,"actions":["read"]}]})
}
pub(super) fn home(operation: &str, id: Id) -> Json {
    json!({"target":{"kind":"org","id":id,"reach":"subtree"},
        "resources":["org","project","topic","thread","document","document_container","provider","agent","environment","host","machine","template","mcp_server","mount","skill","chat"],
        "allow":[{"operation":operation,"actions":["read"]}]})
}

#[tokio::test]
async fn authority_reads_the_actual_account_and_scopes_ordinary_orgs_with_no_store() {
    let world = World::new().await;
    let t = team(&world).await;
    let native = world
        .app
        .call(MeAuthorityGet, &[])
        .token(&t.viewer.token)
        .send()
        .await
        .ok();
    for extra in [Vec::new(), vec![rule("NodeGet.read", "org", t.acme)]] {
        let token = key(
            &world,
            t.viewer.id(),
            std::iter::once(rule("MeAuthorityGet", "account", t.viewer.id()))
                .chain(extra)
                .collect(),
        )
        .await;
        let reply = world
            .app
            .call(MeAuthorityGet, &[])
            .token(&token)
            .send_bytes()
            .await;
        assert_eq!(reply.headers[http::header::CACHE_CONTROL], "no-store");
        let body = checked(reply, StatusCode::OK);
        assert_eq!(
            body["person"],
            serde_json::to_value(&native.person).unwrap()
        );
        assert_eq!(
            body["admin_mode"],
            serde_json::to_value(&native.admin_mode).unwrap()
        );
        assert_eq!(body["meta"], serde_json::to_value(&native.meta).unwrap());
        assert_eq!(body["orgs"], json!([]));
    }
    let token = key(
        &world,
        t.viewer.id(),
        vec![
            rule("MeAuthorityGet", "account", t.viewer.id()),
            home("MeAuthorityGet", t.acme),
            home("MeAuthorityGet", t.viewer.person.personal_org),
        ],
    )
    .await;
    let full = checked(
        world
            .app
            .call(MeAuthorityGet, &[])
            .token(&token)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(full, serde_json::to_value(native).unwrap());
    assert_eq!(full["orgs"][0]["id"], json!(t.viewer.person.personal_org));
}

#[tokio::test]
async fn authority_never_lends_another_operation_account_ancestry_or_browser_mode() {
    let world = World::new().await;
    let t = team(&world).await;
    for rules in [
        vec![home("MeAuthorityGet", t.acme)],
        vec![
            rule("MeGet", "account", t.viewer.id()),
            home("MeAuthorityGet", t.acme),
        ],
        vec![
            rule("MeAuthorityGet", "account", t.boss.id()),
            home("MeAuthorityGet", t.acme),
        ],
    ] {
        let token = key(&world, t.viewer.id(), rules).await;
        checked(
            world
                .app
                .call(MeAuthorityGet, &[])
                .token(&token)
                .send_bytes()
                .await,
            StatusCode::FORBIDDEN,
        );
    }
    // This browser is already acting as a system admin. A key retains native ordinary reach.
    let token = key(
        &world,
        t.sysadmin.id(),
        vec![
            rule("MeAuthorityGet", "account", t.sysadmin.id()),
            home("MeAuthorityGet", t.acme),
            home("MeAuthorityGet", t.sysadmin.person.personal_org),
        ],
    )
    .await;
    let body = checked(
        world
            .app
            .call(MeAuthorityGet, &[])
            .token(&token)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(body["admin_mode"]["as_admin"], false);
    assert_eq!(body["admin_mode"]["available"], true);
    assert!(
        body["orgs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|org| org["id"] != json!(t.acme))
    );
    let stranger = key(
        &world,
        t.stranger.id(),
        vec![
            rule("MeAuthorityGet", "account", t.stranger.id()),
            home("MeAuthorityGet", t.acme),
        ],
    )
    .await;
    let body = checked(
        world
            .app
            .call(MeAuthorityGet, &[])
            .token(&stranger)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(body["orgs"], json!([]));
}

#[tokio::test]
async fn authority_publication_cannot_keep_an_org_after_its_native_membership_is_lost() {
    let world = World::new().await;
    let t = team(&world).await;
    let token = key(
        &world,
        t.viewer.id(),
        vec![
            rule("MeAuthorityGet", "account", t.viewer.id()),
            home("MeAuthorityGet", t.acme),
        ],
    )
    .await;
    let control = checked(
        world
            .app
            .call(MeAuthorityGet, &[])
            .token(&token)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(control["orgs"][0]["id"], json!(t.acme));
    let mut blocker = world.db.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = world.app.clone();
    let request = tokio::spawn(async move {
        let call = app.call(MeAuthorityGet, &[]).token(&token);
        call.send_bytes().await
    });
    wait_for_activity(&world, pid, &request).await;
    sqlx::query("DELETE FROM org_members WHERE org_id=$1 AND user_id=$2")
        .bind(t.acme)
        .bind(t.viewer.id())
        .execute(&world.db.pool)
        .await
        .unwrap();
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert!(!String::from_utf8_lossy(&reply.raw).contains("\"orgs\""));
    let native = world
        .app
        .call(MeAuthorityGet, &[])
        .token(&t.viewer.token)
        .send()
        .await
        .ok();
    assert!(native.orgs.iter().all(|org| org.base.id != t.acme));
}

#[tokio::test]
async fn authority_publication_checks_the_live_key_deadline_after_the_native_activity_wait() {
    let world = World::new().await;
    let actor = places::person(&world, "authority-expiry").await;
    let token = key(
        &world,
        actor.id(),
        vec![rule("MeAuthorityGet", "account", actor.id())],
    )
    .await;
    checked(
        world
            .app
            .call(MeAuthorityGet, &[])
            .token(&token)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    let mut blocker = world.db.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = world.app.clone();
    let request = tokio::spawn(async move {
        let call = app.call(MeAuthorityGet, &[]).token(&token);
        call.send_bytes().await
    });
    wait_for_activity(&world, pid, &request).await;
    world.advance(chrono::Duration::hours(2));
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    assert!(!String::from_utf8_lossy(&reply.raw).contains("\"person\""));
}
