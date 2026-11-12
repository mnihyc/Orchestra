//! Host creation credentials remain one-use receipts and retain their real HTTP issuer.
use super::*;
use orch_access_contract::key_scope::KeyScope;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::identity::{self, NewApiToken};
struct Key {
    id: Id,
    secret: String,
}
fn scope(home: Id, operation: &str, action: &str) -> serde_json::Value {
    json!({"rules":[{"target":{"kind":"org","id":home,"reach":"self"},"resources":["host"],"allow":[{"operation":operation,"actions":[action]}]}]})
}
async fn key(world: &World, owner: Id, policy: serde_json::Value, seconds: i64) -> Key {
    let policy: KeyScope = serde_json::from_value(policy).unwrap();
    assert!(policy.validate().is_empty());
    let key = Key {
        id: orch_types::new_id(),
        secret: new_token(API_TOKEN_PREFIX),
    };
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id: key.id,
            user_id: owner,
            name: "Host enrollment",
            token_hash: &token_hash(&key.secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::seconds(seconds)),
        },
        Some(&json!(policy)),
    )
    .await
    .unwrap();
    key
}
async fn issue(world: &World, secret: &str, home: Id) -> orch_api_core::testing::RawReply {
    let request = world
        .app
        .call(HostEnrollmentTokenCreate, &[])
        .token(secret)
        .body(&HostEnrollmentTokenCreateBody {
            home_id: Some(home),
            name_hint: Some("Original queued node".into()),
            access: Some(NewResourceAccess::OnlyMe),
            delegate_policy: Some(false),
        });
    request.send_bytes().await
}
#[tokio::test]
async fn host_enrollment_requires_its_own_home_manage_and_returns_only_the_created_credential() {
    let world = World::new().await;
    let c = company(&world).await;
    for (op, action) in [("HostGet", "manage"), ("HostEnrollmentTokenCreate", "read")] {
        let policy = scope(c.acme, op, action);
        // HostGet supports Read, so a wrong-operation credential is still syntactically valid.
        let policy = if op == "HostGet" {
            scope(c.acme, op, "read")
        } else {
            policy
        };
        let key = key(&world, c.boss.id(), policy, 3600).await;
        assert_eq!(
            issue(&world, &key.secret, c.acme).await.status,
            http::StatusCode::FORBIDDEN
        );
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM enrollment_tokens")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
    let key = key(
        &world,
        c.boss.id(),
        scope(c.acme, "HostEnrollmentTokenCreate", "manage"),
        3600,
    )
    .await;
    let reply = issue(&world, &key.secret, c.acme).await;
    assert_eq!(
        reply.status,
        http::StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let made: orch_api_contract::resources::EnrollmentToken =
        serde_json::from_slice(&reply.raw).unwrap();
    assert!(!made.token.is_empty());
    assert!(made.commands.linux.contains("install-node.sh"));
    let value: serde_json::Value = serde_json::from_slice(&reply.raw).unwrap();
    assert!(value.get("home").is_none());
    assert!(value.get("host").is_none());
    let id = enrolled_host(&world, &made.token, "Created once").await;
    let (owner, home): (Option<Id>, Option<Id>) =
        sqlx::query_as("SELECT owner_id,home_id FROM hosts WHERE id=$1")
            .bind(id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!((owner, home), (Some(c.boss.id()), Some(c.acme)));
    assert!(
        world
            .app
            .app
            .link
            .certificates()
            .enroll(
                &host_enrollment_request(&made.token, "Replayed"),
                &orch_db::audit::Origin::default()
            )
            .await
            .is_err()
    );
}
#[tokio::test]
async fn native_host_redemption_keeps_original_bearer_deadline_scope_and_reservation_options() {
    for change in [
        "none",
        "rotated",
        "scope",
        "expired_then_extended",
        "home",
        "access",
        "roles",
        "packet",
    ] {
        let world = World::new().await;
        let c = company(&world).await;
        let key = key(
            &world,
            c.boss.id(),
            scope(c.acme, "HostEnrollmentTokenCreate", "manage"),
            if change == "expired_then_extended" {
                5
            } else {
                3600
            },
        )
        .await;
        let reply = issue(&world, &key.secret, c.acme).await;
        assert_eq!(
            reply.status,
            http::StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&reply.raw)
        );
        let made: orch_api_contract::resources::EnrollmentToken =
            serde_json::from_slice(&reply.raw).unwrap();
        let hash = hex::decode(made.receipt.as_ref().unwrap()).unwrap();
        let (required, origin): (bool, serde_json::Value) = sqlx::query_as(
            "SELECT issuer_required,issuer_origin FROM enrollment_tokens WHERE token_hash=$1",
        )
        .bind(&hash)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
        assert!(required);
        assert!(origin.is_object());
        assert!(!origin.to_string().contains(&key.secret));
        match change {
            "rotated" => {
                sqlx::query("UPDATE api_tokens SET token_hash=$2 WHERE id=$1")
                    .bind(key.id)
                    .bind(token_hash("synthetic replacement issuer"))
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            "scope" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(key.id)
                    .bind(json!({"rules":[]}))
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            "expired_then_extended" => {
                world.advance(chrono::Duration::seconds(6));
                sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
                    .bind(key.id)
                    .bind(world.now() + chrono::Duration::hours(1))
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            "home" => {
                sqlx::query("UPDATE enrollment_tokens SET org_id=NULL WHERE token_hash=$1")
                    .bind(&hash)
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            "access" => {
                sqlx::query("UPDATE enrollment_tokens SET access='home' WHERE token_hash=$1")
                    .bind(&hash)
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            "roles" => {
                sqlx::query(
                    "UPDATE enrollment_tokens SET roles=ARRAY[]::text[] WHERE token_hash=$1",
                )
                .bind(&hash)
                .execute(&world.db.pool)
                .await
                .unwrap();
            }
            "packet" => {
                sqlx::query(
                    "UPDATE enrollment_tokens SET issuer_origin='{}'::jsonb WHERE token_hash=$1",
                )
                .bind(&hash)
                .execute(&world.db.pool)
                .await
                .unwrap();
            }
            _ => {}
        }
        let result = world
            .app
            .app
            .link
            .certificates()
            .enroll(
                &host_enrollment_request(&made.token, "Native queued host"),
                &orch_db::audit::Origin::default(),
            )
            .await;
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM hosts")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        if change == "none" {
            assert!(result.is_ok());
            assert_eq!(count, 1);
        } else {
            assert!(result.is_err(), "{change}");
            assert_eq!(count, 0);
            let used: bool = sqlx::query_scalar(
                "SELECT used_at IS NOT NULL FROM enrollment_tokens WHERE token_hash=$1",
            )
            .bind(&hash)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
            assert!(!used);
        }
    }
}
