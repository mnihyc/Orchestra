//! Stored review custody and native pages under exact current management scope.

use super::*;
use chrono::Duration;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::directory::{OrgOutsiderRemovePlanGet, OrgOutsiderRemovePlanGetQuery};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::identity;
use orch_types::{Json, new_id};
use serde_json::json;

fn rule(operation: &str, org: Id, action: &str) -> Json {
    json!({"target":{"kind":"org","id":org,"reach":"self"},"resources":["org"],
        "allow":[{"operation":operation,"actions":[action]}]})
}
async fn key(at: &Acme, user: Id, rules: Vec<Json>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *at.world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Stored outsider review fixture",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: at.world.now(),
            expires_at: Some(at.world.now() + Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    secret
}
async fn page(
    at: &Acme,
    secret: &str,
    user: Id,
    plan: Id,
    query: &OrgOutsiderRemovePlanGetQuery,
) -> RawReply {
    let call = at
        .world
        .app
        .call(OrgOutsiderRemovePlanGet, &[&at.acme, &user, &plan]);
    call.token(secret).query(query).send_bytes().await
}
fn checked(reply: RawReply) -> OutsideRemovalPlan {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

#[tokio::test]
async fn kept_plan_management_uses_stored_org_custody_and_preserves_native_pages_and_fingerprint() {
    let at = acme().await;
    let kept = preview(&at, &at.olga).await;
    assert!(
        kept.removes.len() >= 2,
        "the native complete review has multiple ways"
    );
    let secret = key(
        &at,
        at.root.id(),
        vec![rule("OrgOutsiderRemovePlanGet", at.acme, "manage")],
    )
    .await;
    let mut cursor = None;
    let mut all = Vec::new();
    loop {
        let query = OrgOutsiderRemovePlanGetQuery {
            cursor: cursor.clone(),
            limit: Some(1),
        };
        let native = checked(page(&at, &at.root.token, at.olga.id(), kept.id, &query).await);
        let current = checked(page(&at, &secret, at.olga.id(), kept.id, &query).await);
        assert_eq!(
            current, native,
            "cached review content remains the native stored snapshot"
        );
        all.extend(current.removes);
        cursor = current.next_cursor;
        if cursor.is_none() {
            assert_eq!(current.reviewed_fingerprint, kept.reviewed_fingerprint);
            break;
        }
        assert!(current.reviewed_fingerprint.is_none());
    }
    assert_eq!(all, kept.removes);
    for rules in [
        vec![rule("OrgOutsiderRemovePlanGet", at.acme, "read")],
        vec![rule(
            "OrgOutsiderRemovePlanGet",
            at.root.person.personal_org,
            "manage",
        )],
        vec![rule("OrgOutsiderRemovePreview", at.acme, "manage")],
    ] {
        let wrong = key(&at, at.root.id(), rules).await;
        let reply = page(&at, &wrong, at.olga.id(), kept.id, &Default::default()).await;
        assert_eq!(reply.status, StatusCode::FORBIDDEN);
        assert!(!String::from_utf8_lossy(&reply.raw).contains("Contractors"));
    }
}

#[tokio::test]
async fn kept_plan_cannot_be_borrowed_by_another_requester_or_a_different_outsider() {
    let at = acme().await;
    let kept = preview(&at, &at.olga).await;
    // The second reader really holds the native ordinary permission; custody is independent.
    access_rows::add_entry(
        &mut *at.world.conn().await,
        &PageKey::new("node", at.acme, "live"),
        "any",
        "members.manage",
        ("user", Some(at.ada.id()), None),
        None,
        at.world.now(),
    )
    .await
    .unwrap();
    let secret = key(
        &at,
        at.ada.id(),
        vec![rule("OrgOutsiderRemovePlanGet", at.acme, "manage")],
    )
    .await;
    let native = page(
        &at,
        &at.ada.token,
        at.olga.id(),
        kept.id,
        &Default::default(),
    )
    .await;
    assert!(
        !native.status.is_success(),
        "the native review belongs to its requester"
    );
    let current = page(&at, &secret, at.olga.id(), kept.id, &Default::default()).await;
    assert_eq!(current.status, native.status);
    assert_eq!(
        serde_json::from_slice::<Json>(&current.raw).unwrap()["code"],
        serde_json::from_slice::<Json>(&native.raw).unwrap()["code"]
    );
    assert!(!String::from_utf8_lossy(&current.raw).contains("Contractors"));
    let owner_key = key(
        &at,
        at.root.id(),
        vec![rule("OrgOutsiderRemovePlanGet", at.acme, "manage")],
    )
    .await;
    let native = page(
        &at,
        &at.root.token,
        at.pat.id(),
        kept.id,
        &Default::default(),
    )
    .await;
    let wrong = page(&at, &owner_key, at.pat.id(), kept.id, &Default::default()).await;
    assert!(!wrong.status.is_success());
    assert_eq!(wrong.status, native.status);
}

#[tokio::test]
async fn kept_review_stays_what_was_saved_and_does_not_replan_live_ways() {
    let at = acme().await;
    let kept = preview(&at, &at.olga).await;
    let secret = key(
        &at,
        at.root.id(),
        vec![rule("OrgOutsiderRemovePlanGet", at.acme, "manage")],
    )
    .await;
    let before = checked(page(&at, &secret, at.olga.id(), kept.id, &Default::default()).await);
    access_rows::add_entry(
        &mut *at.world.conn().await,
        &PageKey::new("node", at.acme, "live"),
        "any",
        "org.view",
        ("user", Some(at.olga.id()), None),
        None,
        at.world.now(),
    )
    .await
    .unwrap();
    let unchanged = checked(page(&at, &secret, at.olga.id(), kept.id, &Default::default()).await);
    assert_eq!(
        unchanged, before,
        "a stored review's native page/fingerprint is not a fresh plan"
    );
    assert!(
        confirm(&at, &at.olga, &kept).await.body.is_err(),
        "the existing native confirmation independently detects changed actual ways"
    );
}

#[tokio::test]
async fn kept_plan_publication_rechecks_its_actual_native_expiry_after_activity_wait() {
    for expire_plan in [false, true] {
        let at = acme().await;
        let kept = preview(&at, &at.olga).await;
        let secret = key(
            &at,
            at.root.id(),
            vec![rule("OrgOutsiderRemovePlanGet", at.acme, "manage")],
        )
        .await;
        let native = checked(page(&at, &secret, at.olga.id(), kept.id, &Default::default()).await);
        let mut blocker = at.world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = at.world.app.clone();
        let (org, user, plan) = (at.acme, at.olga.id(), kept.id);
        let request = tokio::spawn(async move {
            let call = app.call(OrgOutsiderRemovePlanGet, &[&org, &user, &plan]);
            call.token(&secret)
                .query(&OrgOutsiderRemovePlanGetQuery::default())
                .send_bytes()
                .await
        });
        let wait = std::time::Duration::from_secs(30);
        tokio::time::timeout(wait, async {
            loop {
                assert!(
                    !request.is_finished(),
                    "the actual final append must be reached"
                );
                let blocked: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                     AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid))
                     AND query LIKE '%INSERT INTO api_token_activity%')",
                )
                .bind(pid)
                .fetch_one(&at.world.db.pool)
                .await
                .unwrap();
                if blocked {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the real activity append is blocked");
        if expire_plan {
            at.world.advance(Duration::minutes(16));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
        if expire_plan {
            assert!(
                !reply.status.is_success(),
                "the native fifteen-minute review expired while the key remains live"
            );
            assert!(!String::from_utf8_lossy(&reply.raw).contains("Contractors"));
        } else {
            assert_eq!(checked(reply), native);
        }
    }
}
