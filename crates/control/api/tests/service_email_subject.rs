//! Service identities can edit their profile but cannot acquire human email state.
#![allow(clippy::unwrap_used, reason = "a test fails by panicking")]

mod places;
mod support;

use chrono::Duration;
use orch_api_contract::auth::{MeGet, MeUpdate};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::directory::{OrgServiceAccountCreate, OrgServiceAccountCreateBody};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::identity;
use orch_types::new_id;
use serde_json::json;
use support::World;

#[tokio::test]
async fn a_legacy_service_key_cannot_attach_email_or_partially_edit_its_profile() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let org = places::make_org(&world, &owner, "Office", "office").await;
    let service = world
        .app
        .call(OrgServiceAccountCreate, &[&org.base.id])
        .token(&owner.token)
        .body(&OrgServiceAccountCreateBody {
            username: "build-service".to_owned(),
            display_name: "Original service".to_owned(),
        })
        .send()
        .await
        .ok();
    let token = new_token(API_TOKEN_PREFIX);
    let hash = token_hash(&token);
    let mut conn = world.conn().await;
    identity::create_api_token(
        &mut conn,
        &identity::NewApiToken {
            id: new_id(),
            user_id: service.user.id,
            name: "Existing legacy service key",
            token_hash: &hash,
            scopes: &["*".to_owned()],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + Duration::days(1)),
        },
    )
    .await
    .unwrap();
    let original = identity::user_by_id(&mut conn, service.user.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(original.kind, "service");
    assert!(
        original.email.is_none()
            && original.password_hash.is_none()
            && original.totp_secret.is_none()
            && original.email_code_at.is_none()
    );
    drop(conn);
    world.app.call(MeGet, &[]).token(&token).send().await.ok();
    let profile = world
        .app
        .call(MeUpdate, &[])
        .token(&token)
        .raw_body(&json!({"display_name":"A valid service profile edit"}))
        .send()
        .await;
    assert_eq!(
        profile.status, 200,
        "the ordinary service-profile path is admitted"
    );
    let refused = world
        .app
        .call(MeUpdate, &[])
        .token(&token)
        .raw_body(&json!({
            "email":"unsupported-service-address@example.org","display_name":"Must stay unapplied"
        }))
        .send()
        .await;
    assert_eq!(
        refused.status, 400,
        "a service identity cannot acquire a human address"
    );
    assert_eq!(refused.problem().code, ErrorCode::RequestInvalid);
    let mut conn = world.conn().await;
    let actual = identity::user_by_id(&mut conn, service.user.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(actual.display_name, "A valid service profile edit");
    assert!(
        actual.email.is_none()
            && actual.password_hash.is_none()
            && actual.totp_secret.is_none()
            && actual.email_code_at.is_none()
    );
    assert!(
        identity::live_sessions(&mut conn, actual.id, world.now())
            .await
            .unwrap()
            .is_empty()
    );
}
