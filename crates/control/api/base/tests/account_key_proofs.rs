//! Explicit account-key proof preserves subject validity, actual confirmations and session identities.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use chrono::Duration;
use orch_access_contract::key_scope::{
    KeyAction, KeyIdentityKind, KeyOperation, KeyReach, KeyRule, KeyScope, KeyTarget, KeyTargetKind,
};
use orch_api_contract::auth::{
    AuthLogin, AuthLoginBody, MeEmailCodeDisable, MeEmailCodeEnable, MeEmailCodeSend, MeGet,
    MePasswordChange, MeRecoveryCodesGet, MeRecoveryCodesRegenerate, MeSessionDelete,
    MeSessionList, MeSessionOthersDelete, MeTotpDisable, MeTotpEnable, MeTotpSetup, MeUpdate,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::directory::{OrgServiceAccountCreate, OrgServiceAccountCreateBody};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::identity;
use orch_types::{Id, new_id};
use serde_json::{Value, json};
use support::World;

fn rule(kind: KeyIdentityKind, id: Id, operations: &[(&str, &[KeyAction])]) -> KeyRule {
    KeyRule {
        target: KeyTarget {
            kind: KeyTargetKind::Identity(kind),
            id: Some(id),
            reach: KeyReach::SelfOnly,
        },
        resources: Some(vec![KeyTargetKind::Identity(kind)]),
        allow: operations
            .iter()
            .map(|(operation, actions)| KeyOperation {
                operation: (*operation).to_owned(),
                actions: actions.to_vec(),
            })
            .collect(),
    }
}

async fn key(world: &World, user: Id, rules: Vec<KeyRule>) -> String {
    let token = new_token(API_TOKEN_PREFIX);
    let hash = token_hash(&token);
    let scope = serde_json::to_value(KeyScope {
        rules,
        ..KeyScope::default()
    })
    .unwrap();
    let mut conn = world.conn().await;
    identity::create_api_token_scoped(
        &mut conn,
        &identity::NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Account control",
            token_hash: &hash,
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + Duration::days(1)),
        },
        Some(&scope),
    )
    .await
    .unwrap();
    token
}

fn value(reply: &RawReply, status: u16) -> Value {
    assert_eq!(reply.status.as_u16(), status);
    serde_json::from_slice(&reply.raw).unwrap()
}

#[tokio::test]
async fn an_admitted_human_key_changes_its_password_without_a_browser_or_redundant_password() {
    let world = World::new().await;
    let ada = world.person("ada").await;
    let signed = world.sign_in(&ada).await;
    let read = key(
        &world,
        ada.id,
        vec![rule(
            KeyIdentityKind::Account,
            ada.id,
            &[("MeGet", &[KeyAction::Read])],
        )],
    )
    .await;
    let wrong = world
        .app
        .call(MePasswordChange, &[])
        .token(&read)
        .raw_body(&json!({"new_password":"another long valid phrase"}))
        .send()
        .await
        .problem();
    assert_eq!(wrong.code, ErrorCode::AuthzDenied);
    world
        .app
        .call(MeGet, &[])
        .token(signed.token())
        .send()
        .await
        .ok();
    let allowed = key(
        &world,
        ada.id,
        vec![rule(
            KeyIdentityKind::Account,
            ada.id,
            &[
                ("MePasswordChange", &[KeyAction::Manage]),
                ("MeGet", &[KeyAction::Read]),
            ],
        )],
    )
    .await;
    let changed = world
        .app
        .call(MePasswordChange, &[])
        .token(&allowed)
        .raw_body(&json!({"new_password":"another long valid phrase"}))
        .send()
        .await;
    assert_eq!(changed.status, 204);
    assert_eq!(
        world
            .app
            .call(MeGet, &[])
            .token(signed.token())
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthInvalidToken
    );
    world.app.call(MeGet, &[]).token(&allowed).send().await.ok();
    world
        .app
        .call(AuthLogin, &[])
        .body(&AuthLoginBody {
            login: ada.username,
            password: Some("another long valid phrase".to_owned()),
            email_code: None,
            totp: None,
            recovery_code: None,
        })
        .send()
        .await
        .ok();
}

#[tokio::test]
async fn key_credential_management_still_requires_new_factor_and_disable_confirmations() {
    let world = World::new().await;
    let ada = world.person("ada").await;
    let token = key(
        &world,
        ada.id,
        vec![rule(
            KeyIdentityKind::Account,
            ada.id,
            &[
                ("MeTotpSetup", &[KeyAction::Manage]),
                ("MeTotpEnable", &[KeyAction::Manage]),
                ("MeTotpDisable", &[KeyAction::Manage]),
                ("MeRecoveryCodesGet", &[KeyAction::Read]),
                ("MeRecoveryCodesRegenerate", &[KeyAction::Manage]),
                ("MeGet", &[KeyAction::Read]),
            ],
        )],
    )
    .await;
    let setup = world
        .app
        .call(MeTotpSetup, &[])
        .token(&token)
        .raw_body(&json!({}))
        .send_bytes()
        .await;
    let setup = value(&setup, 200);
    let secret = totp_rs::Secret::try_from_base32(setup["secret"].as_str().unwrap())
        .unwrap()
        .as_bytes()
        .to_vec();
    let wrong = world
        .app
        .call(MeTotpEnable, &[])
        .token(&token)
        .raw_body(&json!({"code":"not-a-code"}))
        .send()
        .await
        .problem();
    assert_eq!(wrong.code, ErrorCode::AuthSecondStepInvalid);
    let before = world.app.call(MeGet, &[]).token(&token).send().await.ok();
    assert!(!before.totp_enabled);
    let enabled = world
        .app
        .call(MeTotpEnable, &[])
        .token(&token)
        .raw_body(&json!({"code":world.totp_code(&secret)}))
        .send()
        .await;
    assert_eq!(enabled.status, 200);
    let status = world
        .app
        .call(MeRecoveryCodesGet, &[])
        .token(&token)
        .send()
        .await
        .ok();
    assert_eq!(status.remaining, 10);
    assert!(
        !status.recent_recovery,
        "a key never invents a browser recovery allowance"
    );
    let one = world
        .app
        .call(MeRecoveryCodesRegenerate, &[])
        .token(&token)
        .raw_body(&json!({}))
        .send_bytes()
        .await;
    let one = value(&one, 200);
    assert_eq!(one["codes"].as_array().unwrap().len(), 10);
    let next = world
        .app
        .call(MeRecoveryCodesRegenerate, &[])
        .token(&token)
        .raw_body(&json!({}))
        .send_bytes()
        .await;
    let next = value(&next, 200);
    assert_eq!(next["codes"].as_array().unwrap().len(), 10);
    let old = world
        .app
        .call(MeTotpDisable, &[])
        .token(&token)
        .raw_body(&json!({"code":one["codes"][0]}))
        .send()
        .await
        .problem();
    assert_eq!(
        old.code,
        ErrorCode::AuthSecondStepInvalid,
        "replacement invalidates the old complete batch"
    );
    let missing = world
        .app
        .call(MeTotpDisable, &[])
        .token(&token)
        .raw_body(&json!({}))
        .send()
        .await
        .problem();
    assert_eq!(
        missing.code,
        ErrorCode::RequestInvalid,
        "an admitted key does not waive the disable confirmation"
    );
    world.advance(support::step());
    let disabled = world
        .app
        .call(MeTotpDisable, &[])
        .token(&token)
        .raw_body(&json!({"code":world.totp_code(&secret)}))
        .send()
        .await;
    assert_eq!(disabled.status, 204);
    assert!(
        !world
            .app
            .call(MeGet, &[])
            .token(&token)
            .send()
            .await
            .ok()
            .totp_enabled
    );
}

#[tokio::test]
async fn key_mail_management_keeps_new_address_and_mailbox_confirmation_separate() {
    let world = World::new().await;
    world.mail_server().await;
    let ada = world.person("ada").await;
    let token = key(
        &world,
        ada.id,
        vec![rule(
            KeyIdentityKind::Account,
            ada.id,
            &[
                ("MeUpdate.security", &[KeyAction::Manage, KeyAction::Read]),
                ("MeGet", &[KeyAction::Read]),
                ("MeEmailCodeSend", &[KeyAction::Manage]),
                ("MeEmailCodeEnable", &[KeyAction::Manage]),
                ("MeEmailCodeDisable", &[KeyAction::Manage]),
            ],
        )],
    )
    .await;
    let pending = world
        .app
        .call(MeUpdate, &[])
        .token(&token)
        .raw_body(&json!({"email":"new@example.org"}))
        .send()
        .await;
    assert_eq!(pending.status, 200);
    assert_eq!(
        world
            .app
            .call(MeGet, &[])
            .token(&token)
            .send()
            .await
            .ok()
            .email
            .as_deref(),
        Some(ada.email.as_str()),
        "key authentication does not confirm a newly requested address"
    );
    let sent = world
        .app
        .call(MeEmailCodeSend, &[])
        .token(&token)
        .raw_body(&json!({}))
        .send()
        .await;
    assert_eq!(sent.status, 202);
    let code = world.mailed_code(&ada.email).await;
    // A key does not bypass the existing policy that disables email second steps by default.
    let refused = world
        .app
        .call(MeEmailCodeEnable, &[])
        .token(&token)
        .raw_body(&json!({"code":"wrong"}))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthEmailCodeNotAllowed);
    world.set("auth.email_code", json!(["second_step"])).await;
    let bad = world
        .app
        .call(MeEmailCodeEnable, &[])
        .token(&token)
        .raw_body(&json!({"code":"wrong"}))
        .send()
        .await
        .problem();
    assert_eq!(bad.code, ErrorCode::AuthSecondStepInvalid);
    let on = world
        .app
        .call(MeEmailCodeEnable, &[])
        .token(&token)
        .raw_body(&json!({"code":code}))
        .send()
        .await;
    assert_eq!(on.status, 204);
    world.advance(Duration::minutes(2));
    world
        .app
        .call(MeEmailCodeSend, &[])
        .token(&token)
        .raw_body(&json!({}))
        .send()
        .await
        .ok();
    let fresh = world.mailed_code(&ada.email).await;
    let bad = world
        .app
        .call(MeEmailCodeDisable, &[])
        .token(&token)
        .raw_body(&json!({"code":"wrong"}))
        .send()
        .await
        .problem();
    assert_eq!(bad.code, ErrorCode::AuthSecondStepInvalid);
    assert!(
        world
            .app
            .call(MeGet, &[])
            .token(&token)
            .send()
            .await
            .ok()
            .email_code_second_step
    );
    let off = world
        .app
        .call(MeEmailCodeDisable, &[])
        .token(&token)
        .raw_body(&json!({"code":fresh}))
        .send()
        .await;
    assert_eq!(off.status, 204);
    assert!(
        !world
            .app
            .call(MeGet, &[])
            .token(&token)
            .send()
            .await
            .ok()
            .email_code_second_step
    );
}

#[tokio::test]
async fn a_key_revokes_actual_owned_sessions_and_never_invents_a_session_to_keep() {
    let world = World::new().await;
    let ada = world.person("ada").await;
    let bob = world.person("bob").await;
    let old_one = world.sign_in(&ada).await;
    world.sign_in(&ada).await;
    let old = world
        .app
        .call(MeSessionList, &[])
        .token(old_one.token())
        .send()
        .await
        .ok()
        .items;
    assert_eq!(old.len(), 2);
    let expiry = old
        .iter()
        .map(|session| session.expires_at.get())
        .max()
        .unwrap();
    world.advance(expiry - world.now() + Duration::seconds(1));
    let first = world.sign_in(&ada).await;
    let second = world.sign_in(&ada).await;
    let third = world.sign_in(&ada).await;
    let bobs = world.sign_in(&bob).await;
    let sessions = world
        .app
        .call(MeSessionList, &[])
        .token(first.token())
        .send()
        .await
        .ok()
        .items;
    assert_eq!(
        sessions.len(),
        3,
        "the retained expired subjects are absent from the live display"
    );
    let narrow = key(
        &world,
        ada.id,
        sessions
            .iter()
            .map(|session| {
                rule(
                    KeyIdentityKind::Session,
                    session.id,
                    &[(
                        "MeSessionOthersDelete",
                        &[KeyAction::Manage, KeyAction::Read],
                    )],
                )
            })
            .collect(),
    )
    .await;
    let denied = world
        .app
        .call(MeSessionOthersDelete, &[])
        .token(&narrow)
        .send()
        .await
        .problem();
    assert_eq!(
        denied.code,
        ErrorCode::AuthzDenied,
        "scope admission includes expired-but-unrevoked subjects too"
    );
    for browser in [first.token(), second.token(), third.token()] {
        world.app.call(MeGet, &[]).token(browser).send().await.ok();
    }
    // The actual Account-subtree witness covers only this account's credentials,
    // not every owned resource or another person's sessions.
    let mut bulk = rule(
        KeyIdentityKind::Account,
        ada.id,
        &[
            ("MeSessionDelete", &[KeyAction::Manage]),
            (
                "MeSessionOthersDelete",
                &[KeyAction::Manage, KeyAction::Read],
            ),
        ],
    );
    bulk.target.reach = KeyReach::Subtree;
    bulk.resources = Some(vec![KeyTargetKind::Identity(KeyIdentityKind::Session)]);
    let token = key(
        &world,
        ada.id,
        vec![
            bulk,
            rule(
                KeyIdentityKind::Account,
                ada.id,
                &[("MeGet", &[KeyAction::Read])],
            ),
        ],
    )
    .await;
    let expired = world
        .app
        .call(MeSessionDelete, &[&old[0].id])
        .token(&token)
        .send()
        .await;
    assert_eq!(
        expired.status, 204,
        "single-session revocation keeps the same retained-subject semantics"
    );
    let other = sessions.iter().find(|session| !session.current).unwrap().id;
    assert_eq!(
        world
            .app
            .call(MeSessionDelete, &[&other])
            .token(&token)
            .send()
            .await
            .status,
        204
    );
    let bulk = world
        .app
        .call(MeSessionOthersDelete, &[])
        .token(&token)
        .send_bytes()
        .await;
    assert_eq!(value(&bulk, 200), json!([]));
    for browser in [first.token(), second.token(), third.token()] {
        assert_eq!(
            world
                .app
                .call(MeGet, &[])
                .token(browser)
                .send()
                .await
                .problem()
                .code,
            ErrorCode::AuthInvalidToken
        );
    }
    world
        .app
        .call(MeGet, &[])
        .token(bobs.token())
        .send()
        .await
        .ok();
    world.app.call(MeGet, &[]).token(&token).send().await.ok();
    let mut conn = world.conn().await;
    assert!(
        identity::unrevoked_session_subjects(&mut conn, ada.id)
            .await
            .unwrap()
            .is_empty(),
        "expired and live selected subjects were both revoked"
    );
    drop(conn);
    let repeated = world
        .app
        .call(MeSessionOthersDelete, &[])
        .token(&token)
        .send_bytes()
        .await;
    assert_eq!(
        value(&repeated, 200),
        json!([]),
        "an explicitly admitted empty collection needs no fictional Session"
    );
    let foreign = world
        .app
        .call(MeSessionList, &[])
        .token(bobs.token())
        .send()
        .await
        .ok()
        .items[0]
        .id;
    let claimed = key(
        &world,
        ada.id,
        vec![rule(
            KeyIdentityKind::Session,
            foreign,
            &[("MeSessionDelete", &[KeyAction::Manage])],
        )],
    )
    .await;
    let no_owner = world
        .app
        .call(MeSessionDelete, &[&foreign])
        .token(&claimed)
        .send()
        .await
        .problem();
    assert_eq!(
        no_owner.code,
        ErrorCode::NotFound,
        "a selected target never creates ordinary ownership"
    );
    world
        .app
        .call(MeGet, &[])
        .token(bobs.token())
        .send()
        .await
        .ok();
}

#[tokio::test]
async fn service_keys_keep_profile_authority_without_acquiring_human_sign_in_state() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let org = places::make_org(&world, &owner, "Office", "office").await;
    let service = world
        .app
        .call(OrgServiceAccountCreate, &[&org.base.id])
        .token(&owner.token)
        .body(&OrgServiceAccountCreateBody {
            username: "build-service".to_owned(),
            display_name: "Build service".to_owned(),
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("this ordinary control requires the full native creation response");
    let user = service.user.id;
    let token = key(
        &world,
        user,
        vec![rule(
            KeyIdentityKind::Account,
            user,
            &[
                ("MeGet", &[KeyAction::Read]),
                ("MePasswordChange", &[KeyAction::Manage]),
                ("MeTotpSetup", &[KeyAction::Manage]),
                ("MeEmailCodeSend", &[KeyAction::Manage]),
                ("MeUpdate.profile", &[KeyAction::Write, KeyAction::Read]),
                ("MeUpdate.security", &[KeyAction::Manage, KeyAction::Read]),
            ],
        )],
    )
    .await;
    let password = world
        .app
        .call(MePasswordChange, &[])
        .token(&token)
        .raw_body(&json!({"new_password":"not a supported service password"}))
        .send()
        .await
        .problem();
    assert_eq!(password.code, ErrorCode::RequestInvalid);
    let totp = world
        .app
        .call(MeTotpSetup, &[])
        .token(&token)
        .raw_body(&json!({}))
        .send()
        .await
        .problem();
    assert_eq!(totp.code, ErrorCode::RequestInvalid);
    let code = world
        .app
        .call(MeEmailCodeSend, &[])
        .token(&token)
        .raw_body(&json!({}))
        .send()
        .await
        .problem();
    assert_eq!(code.code, ErrorCode::RequestInvalid);
    let email = world
        .app
        .call(MeUpdate, &[])
        .token(&token)
        .raw_body(&json!({"email":"not-for-service@example.org"}))
        .send()
        .await
        .problem();
    assert_eq!(email.code, ErrorCode::RequestInvalid);
    let updated = world
        .app
        .call(MeUpdate, &[])
        .token(&token)
        .raw_body(&json!({"display_name":"Service profile remains editable"}))
        .send()
        .await;
    assert_eq!(updated.status, 200);
    let mut conn = world.conn().await;
    let actual = identity::user_by_id(&mut conn, user)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(actual.kind, "service");
    assert_eq!(actual.display_name, "Service profile remains editable");
    assert!(
        actual.password_hash.is_none()
            && actual.email.is_none()
            && actual.totp_secret.is_none()
            && actual.totp_enabled_at.is_none()
            && actual.email_code_at.is_none()
    );
    assert!(
        identity::live_sessions(&mut conn, user, world.now())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        identity::api_token_by_hash(&mut conn, &token_hash(&token), world.now())
            .await
            .unwrap()
            .is_some(),
        "the valid service key remains usable"
    );
}
