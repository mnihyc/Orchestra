//! Authenticator backups are issued once, second factors only, atomically spent and replaceable without the lost phone.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use chrono::Duration;
use orch_api_contract::auth::{
    ApiTokenCreateBody, AuthLoginBody, MeGet, MeRecoveryCodesGet, MeRecoveryCodesRegenerate,
    MeRecoveryCodesRegenerateBody, MeSecondStepVerify, MeSecondStepVerifyBody, MeTokenCreate,
    MeTotpDisable, MeTotpDisableBody, MeTotpEnable, MeTotpEnableBody, MeTotpSetup, MeTotpSetupBody,
    RecoveryCodeBatch, SensitiveProof, SignInProof,
};
use orch_api_contract::common::ErrorCode;
use orch_api_core::sign_in::recovery_codes::{hash, normalize};
use orch_db::queries::{identity, recovery_codes};
use support::{PASSWORD, STRANGER, Signed, World};

async fn enable(world: &World, signed: &Signed) -> RecoveryCodeBatch {
    let setup = world
        .app
        .call(MeTotpSetup, &[])
        .token(signed.token())
        .body(&MeTotpSetupBody {
            password: Some(PASSWORD.to_owned()),
            identity_proof: None,
        })
        .send()
        .await
        .ok();
    let secret = totp_rs::Secret::try_from_base32(&setup.secret)
        .unwrap()
        .as_bytes()
        .to_vec();
    let reply = world
        .app
        .call(MeTotpEnable, &[])
        .token(signed.token())
        .body(&MeTotpEnableBody {
            code: world.totp_code(&secret),
        })
        .send()
        .await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.headers.get("cache-control").unwrap(), "no-store");
    let reply = reply.ok();
    assert!(reply.me.totp_enabled);
    reply.recovery_codes
}

fn login(name: &str, password: Option<&str>, code: &str) -> AuthLoginBody {
    AuthLoginBody {
        login: name.to_owned(),
        password: password.map(str::to_owned),
        email_code: None,
        totp: None,
        recovery_code: Some(code.to_owned()),
    }
}

fn replacement(password: &str, code: Option<&str>) -> MeRecoveryCodesRegenerateBody {
    MeRecoveryCodesRegenerateBody {
        proof: SensitiveProof {
            password: Some(password.to_owned()),
            recovery_code: code.map(str::to_owned),
            ..SensitiveProof::default()
        },
    }
}

#[tokio::test]
async fn activation_issues_ten_codes_once_and_ordinary_reads_return_only_metadata() {
    let world = World::new().await;
    let person = world.person("ada").await;
    let signed = world.sign_in(&person).await;
    let batch = enable(&world, &signed).await;
    assert_eq!(batch.codes.len(), 10);
    assert_eq!(batch.status.remaining, 10);
    assert!(batch.status.methods.contains(&SignInProof::RecoveryCode));
    let metadata = world
        .app
        .call(MeRecoveryCodesGet, &[])
        .token(signed.token())
        .send()
        .await;
    let text = std::str::from_utf8(&metadata.raw).unwrap();
    for code in &batch.codes {
        assert!(!text.contains(code));
        assert!(!format!("{batch:?}").contains(code));
    }
    assert!(!text.contains("code_hash"));
    assert_eq!(metadata.ok().remaining, 10);
    let stored: Vec<Vec<u8>> =
        sqlx::query_scalar("SELECT code_hash FROM second_step_recovery_codes WHERE user_id=$1")
            .bind(person.id)
            .fetch_all(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(stored.len(), 10);
    for code in &batch.codes {
        assert!(stored.contains(&hash(person.id, &normalize(code).unwrap())));
    }
    assert!(stored.iter().all(|value| value.len() == 32));
    let me = world
        .app
        .call(MeGet, &[])
        .token(signed.token())
        .send()
        .await;
    for code in &batch.codes {
        assert!(!std::str::from_utf8(&me.raw).unwrap().contains(code));
    }
}

#[tokio::test]
async fn a_recovery_code_is_never_a_first_proof_and_a_wrong_password_does_not_spend_it() {
    let world = World::new().await;
    let person = world.person("ada").await;
    let signed = world.sign_in(&person).await;
    let batch = enable(&world, &signed).await;
    let code = &batch.codes[0];
    assert_eq!(
        world
            .attempt(login("ada", None, code), STRANGER, "")
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );
    assert_eq!(
        world
            .attempt(login("ada", Some("wrong password"), code), STRANGER, "")
            .await
            .problem()
            .code,
        ErrorCode::AuthInvalidCredentials
    );
    assert_eq!(
        world
            .app
            .call(MeRecoveryCodesGet, &[])
            .token(signed.token())
            .send()
            .await
            .ok()
            .remaining,
        10
    );
    let typed = code.to_lowercase().replace('-', " ");
    assert_eq!(
        world
            .attempt(login("ada", Some(PASSWORD), &typed), STRANGER, "")
            .await
            .status,
        200
    );
    assert_eq!(
        world
            .attempt(login("ada", Some(PASSWORD), code), STRANGER, "")
            .await
            .problem()
            .code,
        ErrorCode::AuthSecondStepInvalid
    );
}

#[tokio::test]
async fn two_simultaneous_sign_ins_cannot_spend_one_code_twice() {
    let world = World::new().await;
    let person = world.person("ada").await;
    let signed = world.sign_in(&person).await;
    let batch = enable(&world, &signed).await;
    let (one, two) = tokio::join!(
        world.attempt(
            login("ada", Some(PASSWORD), &batch.codes[0]),
            "198.51.100.18",
            ""
        ),
        world.attempt(
            login("ada", Some(PASSWORD), &batch.codes[0]),
            "198.51.100.19",
            ""
        )
    );
    let statuses = [one.status.as_u16(), two.status.as_u16()];
    assert_eq!(statuses.iter().filter(|status| **status == 200).count(), 1);
    assert_eq!(statuses.iter().filter(|status| **status == 401).count(), 1);
}

#[tokio::test]
async fn codes_belong_to_one_person_and_stop_working_when_the_authenticator_is_disabled() {
    let world = World::new().await;
    let ada = world.person("ada").await;
    let bob = world.person("bob").await;
    let signed = world.sign_in(&ada).await;
    let bob_session = world.sign_in(&bob).await;
    let batch = enable(&world, &signed).await;
    enable(&world, &bob_session).await;
    assert_eq!(
        world
            .attempt(login("bob", Some(PASSWORD), &batch.codes[0]), STRANGER, "")
            .await
            .problem()
            .code,
        ErrorCode::AuthSecondStepInvalid
    );
    let off = world
        .app
        .call(MeTotpDisable, &[])
        .token(signed.token())
        .body(&MeTotpDisableBody {
            password: PASSWORD.to_owned(),
            code: batch.codes[0].clone(),
        })
        .send()
        .await;
    assert_eq!(off.status, 204);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM second_step_recovery_codes WHERE user_id=$1")
            .bind(ada.id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
    let next = enable(&world, &signed).await;
    assert_eq!(next.status.remaining, 10);
    assert_eq!(
        world
            .attempt(login("ada", Some(PASSWORD), &batch.codes[1]), STRANGER, "")
            .await
            .problem()
            .code,
        ErrorCode::AuthSecondStepInvalid
    );
}

#[tokio::test]
async fn replacing_codes_needs_fresh_proof_and_invalidates_the_whole_old_batch_atomically() {
    let world = World::new().await;
    let person = world.person("ada").await;
    let signed = world.sign_in(&person).await;
    let batch = enable(&world, &signed).await;
    let bad = world
        .app
        .call(MeRecoveryCodesRegenerate, &[])
        .token(signed.token())
        .body(&replacement("wrong password", Some(&batch.codes[0])))
        .send()
        .await;
    assert_eq!(bad.problem().code, ErrorCode::AuthInvalidCredentials);
    let reply = world
        .app
        .call(MeRecoveryCodesRegenerate, &[])
        .token(signed.token())
        .body(&replacement(PASSWORD, Some(&batch.codes[0])))
        .send()
        .await;
    assert_eq!(reply.headers.get("cache-control").unwrap(), "no-store");
    let next = reply.ok();
    assert_eq!(next.status.remaining, 10);
    assert_eq!(
        world
            .attempt(login("ada", Some(PASSWORD), &batch.codes[1]), STRANGER, "")
            .await
            .problem()
            .code,
        ErrorCode::AuthSecondStepInvalid
    );
    assert_eq!(
        world
            .attempt(login("ada", Some(PASSWORD), &next.codes[0]), STRANGER, "")
            .await
            .status,
        200
    );
}

#[tokio::test]
async fn an_existing_browser_can_verify_its_second_step_with_a_saved_code_only_once() {
    let world = World::new().await;
    let person = world.person("ada").await;
    let signed = world.sign_in(&person).await;
    let batch = enable(&world, &signed).await;
    let proof = MeSecondStepVerifyBody {
        recovery_code: Some(batch.codes[0].clone()),
        ..MeSecondStepVerifyBody::default()
    };
    assert_eq!(
        world
            .app
            .call(MeSecondStepVerify, &[])
            .token(signed.token())
            .body(&proof)
            .send()
            .await
            .status,
        200
    );
    assert_eq!(
        world
            .app
            .call(MeSecondStepVerify, &[])
            .token(signed.token())
            .body(&proof)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthSecondStepInvalid
    );
}

#[tokio::test]
async fn a_rolled_back_spend_leaves_the_saved_code_usable() {
    let world = World::new().await;
    let person = world.person("ada").await;
    let signed = world.sign_in(&person).await;
    let batch = enable(&world, &signed).await;
    let hashed = hash(person.id, &normalize(&batch.codes[0]).unwrap());
    let mut tx = world.db.pool.begin().await.unwrap();
    assert!(
        recovery_codes::spend(&mut tx, person.id, &hashed, world.now())
            .await
            .unwrap()
    );
    tx.rollback().await.unwrap();
    assert_eq!(
        world
            .attempt(login("ada", Some(PASSWORD), &batch.codes[0]), STRANGER, "")
            .await
            .status,
        200
    );
}

#[tokio::test]
async fn the_last_code_can_replace_its_batch_in_that_browser_but_cannot_authorize_a_token() {
    let world = World::new().await;
    let person = world.person("ada").await;
    let signed = world.sign_in(&person).await;
    let batch = enable(&world, &signed).await;
    {
        let mut conn = world.conn().await;
        for code in batch.codes.iter().take(9) {
            assert!(
                recovery_codes::spend(
                    &mut conn,
                    person.id,
                    &hash(person.id, &normalize(code).unwrap()),
                    world.now()
                )
                .await
                .unwrap()
            );
        }
    }
    let recovered = Signed::of(
        world
            .attempt(login("ada", Some(PASSWORD), &batch.codes[9]), STRANGER, "")
            .await,
    );
    let status = world
        .app
        .call(MeRecoveryCodesGet, &[])
        .token(recovered.token())
        .send()
        .await
        .ok();
    assert_eq!(status.remaining, 0);
    assert!(status.recent_recovery);
    assert!(
        !world
            .app
            .call(MeRecoveryCodesGet, &[])
            .token(signed.token())
            .send()
            .await
            .ok()
            .recent_recovery
    );
    let token = world
        .app
        .call(MeTokenCreate, &[])
        .token(recovered.token())
        .body(&ApiTokenCreateBody {
            name: "cannot use recovery management".to_owned(),
            scopes: Some(vec!["thread.view".to_owned()]),
            proof: replacement(PASSWORD, None).proof,
            node_id: None,
            expires_at: None,
        })
        .send()
        .await;
    assert_eq!(token.problem().code, ErrorCode::AuthSecondStepRequired);
    let new = world
        .app
        .call(MeRecoveryCodesRegenerate, &[])
        .token(recovered.token())
        .body(&replacement(PASSWORD, None))
        .send()
        .await
        .ok();
    assert_eq!(new.codes.len(), 10);
    assert!(!new.status.recent_recovery);
    assert_eq!(
        world
            .app
            .call(MeRecoveryCodesRegenerate, &[])
            .token(recovered.token())
            .body(&replacement(PASSWORD, None))
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthSecondStepRequired
    );
}

#[tokio::test]
async fn a_recovery_management_allowance_expires_and_does_not_survive_a_replaced_authenticator() {
    let world = World::new().await;
    let person = world.person("ada").await;
    let signed = world.sign_in(&person).await;
    let batch = enable(&world, &signed).await;
    let recovered = Signed::of(
        world
            .attempt(login("ada", Some(PASSWORD), &batch.codes[0]), STRANGER, "")
            .await,
    );
    world.advance(Duration::minutes(6));
    assert_eq!(
        world
            .app
            .call(MeRecoveryCodesRegenerate, &[])
            .token(recovered.token())
            .body(&replacement(PASSWORD, None))
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthSecondStepRequired
    );
    let recovered = Signed::of(
        world
            .attempt(login("ada", Some(PASSWORD), &batch.codes[1]), STRANGER, "")
            .await,
    );
    {
        let mut conn = world.conn().await;
        identity::disable_totp(&mut conn, person.id, world.now())
            .await
            .unwrap();
    }
    enable(&world, &recovered).await;
    assert!(
        !world
            .app
            .call(MeRecoveryCodesGet, &[])
            .token(recovered.token())
            .send()
            .await
            .ok()
            .recent_recovery
    );
}

#[tokio::test]
async fn app_and_recovery_attempts_share_the_same_second_factor_limit() {
    let world = World::locking_after(2).await;
    let person = world.person("ada").await;
    let signed = world.sign_in(&person).await;
    let batch = enable(&world, &signed).await;
    let mut app_code = login("ada", Some(PASSWORD), "");
    app_code.recovery_code = None;
    app_code.totp = Some("not an app code".to_owned());
    assert_eq!(
        world
            .attempt(app_code, "198.51.100.11", "")
            .await
            .problem()
            .code,
        ErrorCode::AuthSecondStepInvalid
    );
    let wrong = world
        .attempt(
            login("ada", Some(PASSWORD), "not a recovery code"),
            "198.51.100.12",
            "",
        )
        .await;
    assert_eq!(wrong.problem().code, ErrorCode::AuthSecondStepInvalid);
    assert_eq!(
        world
            .attempt(
                login("ada", Some(PASSWORD), &batch.codes[0]),
                "198.51.100.13",
                ""
            )
            .await
            .problem()
            .code,
        ErrorCode::AuthLocked
    );
    assert_eq!(
        world
            .app
            .call(MeRecoveryCodesGet, &[])
            .token(signed.token())
            .send()
            .await
            .ok()
            .remaining,
        10
    );
}

#[tokio::test]
async fn a_personal_api_token_cannot_read_or_replace_its_persons_recovery_codes() {
    let world = World::new().await;
    let person = world.person("ada").await;
    let signed = world.sign_in(&person).await;
    let token = world
        .app
        .call(MeTokenCreate, &[])
        .token(signed.token())
        .body(&ApiTokenCreateBody {
            name: "automation".to_owned(),
            scopes: Some(vec!["*".to_owned()]),
            node_id: None,
            expires_at: None,
            proof: SensitiveProof {
                password: Some(PASSWORD.to_owned()),
                ..SensitiveProof::default()
            },
        })
        .send()
        .await
        .ok()
        .token
        .unwrap();
    let batch = enable(&world, &signed).await;
    assert_eq!(
        world
            .app
            .call(MeRecoveryCodesGet, &[])
            .token(&token)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthSessionRequired
    );
    assert_eq!(
        world
            .app
            .call(MeRecoveryCodesRegenerate, &[])
            .token(&token)
            .body(&replacement(PASSWORD, Some(&batch.codes[0])))
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthSessionRequired
    );
}
