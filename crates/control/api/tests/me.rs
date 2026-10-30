//! What a signed-in person does to their own identity: their profile and address, password, second steps, sessions and API tokens.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use chrono::Duration;
use orch_api_contract::auth::{
    ApiTokenCreateBody, AuthEmailCodeSend, AuthEmailCodeSendBody, AuthEmailVerify,
    AuthEmailVerifyBody, AuthLoginBody, AuthRecoveryCheck, AuthRecoveryConfirm,
    AuthRecoveryConfirmBody, AuthRecoveryStart, AuthRecoveryStartBody, MeEmailCodeDisable,
    MeEmailCodeDisableBody, MeEmailCodeEnable, MeEmailCodeEnableBody, MeEmailCodeSend,
    MeEmailCodeSendBody, MeGet, MePasswordChange, MePasswordChangeBody, MeSessionDelete,
    MeSessionList, MeTokenCreate, MeTokenDelete, MeTokenList, MeTotpDisable, MeTotpDisableBody,
    MeTotpEnable, MeTotpEnableBody, MeTotpSetup, MeTotpSetupBody, MeUpdate, MeUpdateBody,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::{NodeGet, NodeKind};
use orch_api_contract::users::{Density, EmailCodeUse, Locale, Theme};
use orch_api_core::testing::Reply;
use orch_settings::Target;
use orch_types::Id;
use serde_json::json;
use sha2::{Digest, Sha256};
use support::{PASSWORD, STRANGER, Signed, World, step};

fn hash(text: &str) -> Vec<u8> {
    Sha256::digest(text.as_bytes()).to_vec()
}

async fn update(
    world: &World,
    signed: &Signed,
    body: &MeUpdateBody,
) -> Reply<orch_api_contract::users::Me> {
    world
        .app
        .call(MeUpdate, &[])
        .token(signed.token())
        .body(body)
        .send()
        .await
}

async fn email_code_of(world: &World, signed: &Signed) -> Vec<EmailCodeUse> {
    world
        .app
        .call(MeGet, &[])
        .token(signed.token())
        .send()
        .await
        .ok()
        .email_code
}

async fn epoch_of(world: &World) -> i64 {
    sqlx::query_scalar("SELECT epoch FROM authz_state")
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}

fn change_address(email: &str, password: Option<&str>) -> MeUpdateBody {
    MeUpdateBody {
        email: Some(Some(email.to_owned())),
        current_password: password.map(str::to_owned),
        ..MeUpdateBody::default()
    }
}

async fn change_password(world: &World, signed: &Signed, current: &str, new: &str) -> Reply<()> {
    world
        .app
        .call(MePasswordChange, &[])
        .token(signed.token())
        .body(&MePasswordChangeBody {
            current_password: current.to_owned(),
            new_password: new.to_owned(),
        })
        .send()
        .await
}

async fn mail_a_code(world: &World, signed: &Signed, password: &str) -> Reply<()> {
    world
        .app
        .call(MeEmailCodeSend, &[])
        .token(signed.token())
        .body(&MeEmailCodeSendBody {
            password: Some(password.to_owned()),
            identity_proof: None,
        })
        .send()
        .await
}

async fn enable_code(world: &World, signed: &Signed, code: &str) -> Reply<()> {
    world
        .app
        .call(MeEmailCodeEnable, &[])
        .token(signed.token())
        .body(&MeEmailCodeEnableBody {
            code: code.to_owned(),
        })
        .send()
        .await
}

async fn create_token(
    world: &World,
    signed: &Signed,
    body: ApiTokenCreateBody,
) -> Reply<orch_api_contract::auth::TokenCreated> {
    world
        .app
        .call(MeTokenCreate, &[])
        .token(signed.token())
        .body(&body)
        .send()
        .await
}

fn named(name: &str) -> ApiTokenCreateBody {
    ApiTokenCreateBody {
        name: name.to_owned(),
        scopes: Some(vec!["*".to_owned()]),
        node_id: None,
        expires_at: None,
        proof: orch_api_contract::auth::SensitiveProof {
            password: Some(support::PASSWORD.to_owned()),
            ..Default::default()
        },
    }
}

#[tokio::test]
async fn a_person_reads_themself_and_changes_what_they_set_for_themself() {
    let world = World::new().await;
    let ada = world.person("ada").await;
    let signed = world.sign_in(&ada).await;
    let me = world
        .app
        .call(MeGet, &[])
        .token(signed.token())
        .send()
        .await
        .ok();
    assert_eq!(me.user.username, "ada");
    assert_eq!(me.email.as_deref(), Some("ada@example.org"));
    assert!(me.email_verified && me.has_password && !me.totp_enabled && !me.is_system_admin);
    assert_eq!(
        (me.locale, me.theme, me.density),
        (Locale::En, Theme::System, Density::Comfortable)
    );
    assert_eq!(
        (me.timezone.as_str(), me.personal_org_id),
        ("UTC", Some(ada.personal_org))
    );

    let changed = update(
        &world,
        &signed,
        &MeUpdateBody {
            display_name: Some("  émile Zola ".to_owned()),
            locale: Some(Locale::ZhCn),
            timezone: Some("Asia/Shanghai".to_owned()),
            theme: Some(Theme::Dark),
            density: Some(Density::Compact),
            ..MeUpdateBody::default()
        },
    )
    .await
    .ok();
    assert_eq!(changed.user.display_name, "émile Zola");
    assert_eq!(changed.user.avatar.text, "É");
    assert_eq!(
        (changed.locale, changed.theme, changed.density),
        (Locale::ZhCn, Theme::Dark, Density::Compact)
    );
    assert_eq!(changed.timezone, "Asia/Shanghai");
    // What was left out stays as it is.
    let kept = update(
        &world,
        &signed,
        &MeUpdateBody {
            theme: Some(Theme::Light),
            ..MeUpdateBody::default()
        },
    )
    .await
    .ok();
    assert_eq!(
        (kept.theme, kept.locale, kept.user.display_name.as_str()),
        (Theme::Light, Locale::ZhCn, "émile Zola")
    );

    let bad_zone = update(
        &world,
        &signed,
        &MeUpdateBody {
            timezone: Some("Mars/Olympus".to_owned()),
            ..MeUpdateBody::default()
        },
    )
    .await
    .problem();
    assert_eq!(bad_zone.code, ErrorCode::RequestInvalid);
    assert_eq!(bad_zone.extra["errors"][0]["path"], "timezone");
    let bad_name = update(
        &world,
        &signed,
        &MeUpdateBody {
            display_name: Some("   ".to_owned()),
            ..MeUpdateBody::default()
        },
    )
    .await
    .problem();
    assert_eq!(bad_name.extra["errors"][0]["path"], "display_name");
    let unknown = world
        .app
        .call(MeUpdate, &[])
        .token(signed.token())
        .raw_body(&json!({ "nickname": "ad" }))
        .send()
        .await
        .problem();
    assert_eq!(unknown.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn a_new_address_changes_nothing_until_it_is_confirmed_and_the_old_one_is_told() {
    let world = World::new().await;
    world.mail_server().await;
    let ada = world.person("ada").await;
    world.person("bob").await;
    let signed = world.sign_in(&ada).await;

    // The password is asked for where there is one, and counted.
    let without = update(&world, &signed, &change_address("new@example.org", None))
        .await
        .problem();
    assert_eq!(without.extra["errors"][0]["path"], "current_password");
    let wrong = update(
        &world,
        &signed,
        &change_address("new@example.org", Some("wrong password")),
    )
    .await
    .problem();
    assert_eq!(wrong.code, ErrorCode::AuthInvalidCredentials);
    let taken = update(
        &world,
        &signed,
        &change_address("BOB@example.org", Some(PASSWORD)),
    )
    .await
    .problem();
    assert_eq!((taken.status, taken.code), (409, ErrorCode::ConflictExists));
    let bad = update(&world, &signed, &change_address("nowhere", Some(PASSWORD)))
        .await
        .problem();
    assert_eq!(bad.extra["errors"][0]["path"], "email");
    assert!(world.outbox().await.is_empty());

    let me = update(
        &world,
        &signed,
        &change_address("new@example.org", Some(PASSWORD)),
    )
    .await
    .ok();
    assert_eq!(me.email.as_deref(), Some("ada@example.org"));
    assert!(me.email_verified);
    assert_eq!(me.email_pending.as_deref(), Some("new@example.org"));
    // A link goes to the new address and a notice to the old.
    let token = world.mails("verify_email", "new@example.org").await;
    assert_eq!(token.len(), 1);
    let notice = world.mails("email_changed", "ada@example.org").await;
    assert_eq!(notice.len(), 1);
    assert_eq!(notice[0]["data"]["new_email"], "new@example.org");
    // Codes and resets go to the confirmed address meanwhile.
    for email in ["new@example.org", "ada@example.org"] {
        world
            .app
            .call(AuthRecoveryStart, &[])
            .body(&AuthRecoveryStartBody {
                email: email.to_owned(),
            })
            .send()
            .await
            .ok();
    }
    assert!(
        world
            .mails("reset_password", "new@example.org")
            .await
            .is_empty()
    );
    assert_eq!(
        world.mails("reset_password", "ada@example.org").await.len(),
        1
    );

    // The link takes its place.
    let secret = token[0]["data"]["token"].as_str().unwrap().to_owned();
    let done = world
        .app
        .call(AuthEmailVerify, &[])
        .body(&AuthEmailVerifyBody { token: secret })
        .send()
        .await;
    assert_eq!(done.status, 204);
    let me = world
        .app
        .call(MeGet, &[])
        .token(signed.token())
        .send()
        .await
        .ok();
    assert_eq!(me.email.as_deref(), Some("new@example.org"));
    assert!(me.email_verified && me.email_pending.is_none());
    assert_eq!(
        world
            .password_login("new@example.org", PASSWORD, STRANGER, "")
            .await
            .status,
        200
    );
    assert_eq!(
        world
            .password_login("ada@example.org", PASSWORD, "198.51.100.5", "")
            .await
            .status,
        401
    );
    assert!(
        world
            .audit()
            .await
            .contains(&"auth.email_changed".to_owned())
    );
}

#[tokio::test]
async fn a_reset_sent_to_a_former_address_stops_working_when_the_new_address_is_confirmed() {
    let world = World::new().await;
    world.mail_server().await;
    let ada = world.person("ada").await;
    let signed = world.sign_in(&ada).await;
    update(
        &world,
        &signed,
        &change_address("new@example.org", Some(PASSWORD)),
    )
    .await
    .ok();
    world
        .app
        .call(AuthRecoveryStart, &[])
        .body(&AuthRecoveryStartBody {
            email: "ada@example.org".to_owned(),
        })
        .send()
        .await
        .ok();
    let old_mail = world.mails("reset_password", "ada@example.org").await;
    let old_reset = old_mail[0]["data"]["token"].as_str().unwrap();
    let verification = world.mails("verify_email", "new@example.org").await;
    let verify_token = verification[0]["data"]["token"].as_str().unwrap();
    world
        .app
        .call(AuthEmailVerify, &[])
        .body(&AuthEmailVerifyBody {
            token: verify_token.to_owned(),
        })
        .send()
        .await
        .ok();
    let check = world
        .app
        .call(AuthRecoveryCheck, &[&old_reset])
        .send()
        .await
        .problem();
    assert_eq!((check.status, check.code), (410, ErrorCode::Gone));
    // Model a recovery issuance that had read the old address before replacement and inserted
    // its link after the cleanup committed: the destination is checked again on use.
    sqlx::query("UPDATE email_tokens SET used_at=NULL WHERE token_hash=$1")
        .bind(hash(old_reset))
        .execute(&world.db.pool)
        .await
        .unwrap();
    let reset = world
        .app
        .call(AuthRecoveryConfirm, &[])
        .body(&AuthRecoveryConfirmBody {
            token: old_reset.to_owned(),
            password: "an attacker password".to_owned(),
        })
        .send()
        .await
        .problem();
    assert_eq!((reset.status, reset.code), (410, ErrorCode::Gone));
    assert_eq!(
        world
            .password_login("new@example.org", PASSWORD, STRANGER, "")
            .await
            .status,
        200
    );
    // The existing send pace is per person, including after their address changes.
    world.advance(Duration::minutes(1) + Duration::seconds(1));
    world
        .app
        .call(AuthRecoveryStart, &[])
        .body(&AuthRecoveryStartBody {
            email: "new@example.org".to_owned(),
        })
        .send()
        .await
        .ok();
    let new_mail = world.mails("reset_password", "new@example.org").await;
    let new_reset = new_mail[0]["data"]["token"].as_str().unwrap();
    assert_eq!(
        world
            .app
            .call(AuthRecoveryConfirm, &[])
            .body(&AuthRecoveryConfirmBody {
                token: new_reset.to_owned(),
                password: "a fresh new password".to_owned(),
            })
            .send()
            .await
            .status,
        204
    );
}

#[tokio::test]
async fn a_second_request_for_an_address_voids_the_first() {
    let world = World::new().await;
    world.mail_server().await;
    let ada = world.person("ada").await;
    let signed = world.sign_in(&ada).await;
    update(
        &world,
        &signed,
        &change_address("first@example.org", Some(PASSWORD)),
    )
    .await
    .ok();
    let me = update(
        &world,
        &signed,
        &change_address("second@example.org", Some(PASSWORD)),
    )
    .await
    .ok();
    assert_eq!(me.email_pending.as_deref(), Some("second@example.org"));
    let first = world.mails("verify_email", "first@example.org").await[0]["data"]["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let second = world.mails("verify_email", "second@example.org").await[0]["data"]["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let call = |token: String| {
        world
            .app
            .call(AuthEmailVerify, &[])
            .body(&AuthEmailVerifyBody { token })
    };
    assert_eq!(call(first).send().await.problem().code, ErrorCode::Gone);
    assert_eq!(call(second).send().await.status, 204);
}

#[tokio::test]
async fn where_the_system_has_no_mail_server_a_new_address_is_stored_as_given_unconfirmed() {
    let world = World::new().await;
    let ada = world.person("ada").await;
    let signed = world.sign_in(&ada).await;
    let me = update(
        &world,
        &signed,
        &change_address("new@example.org", Some(PASSWORD)),
    )
    .await
    .ok();
    assert_eq!(me.email.as_deref(), Some("new@example.org"));
    assert!(!me.email_verified && me.email_pending.is_none());
    assert!(
        world
            .audit()
            .await
            .contains(&"auth.email_changed".to_owned())
    );
    assert!(world.outbox().await.is_empty());
    // The address they have again changes nothing.
    let same = update(&world, &signed, &change_address("NEW@example.org", None))
        .await
        .ok();
    assert_eq!(same.email.as_deref(), Some("new@example.org"));
}

#[tokio::test]
async fn changing_the_password_ends_the_other_sessions_and_keeps_this_one() {
    let world = World::new().await;
    let ada = world.person("ada").await;
    let this = world.sign_in(&ada).await;
    let other = world.sign_in(&ada).await;

    let short = change_password(&world, &this, PASSWORD, "short")
        .await
        .problem();
    assert_eq!(short.extra["errors"][0]["path"], "new_password");
    let same_as_name = change_password(&world, &this, PASSWORD, "ada@example.org")
        .await
        .problem();
    assert_eq!(same_as_name.extra["errors"][0]["path"], "new_password");
    // A whole password that is commonly used is refused by its own code, case folded.
    let common = change_password(&world, &this, PASSWORD, "QwertyUiop")
        .await
        .problem();
    assert_eq!(
        (common.status, common.code),
        (400, ErrorCode::AuthPasswordCommon)
    );
    assert_eq!(common.extra["field"], "new_password");
    assert!(common.detail.contains("commonly used"), "{}", common.detail);
    let wrong = change_password(&world, &this, "wrong password", "a brand new password")
        .await
        .problem();
    assert_eq!(wrong.code, ErrorCode::AuthInvalidCredentials);

    assert_eq!(
        change_password(&world, &this, PASSWORD, "a brand new password")
            .await
            .status,
        204
    );
    assert_eq!(
        world
            .app
            .call(MeGet, &[])
            .token(this.token())
            .send()
            .await
            .status,
        200
    );
    let ended = world
        .app
        .call(MeGet, &[])
        .token(other.token())
        .send()
        .await
        .problem();
    assert_eq!(ended.code, ErrorCode::AuthInvalidToken);
    assert_eq!(
        world
            .password_login("ada", "a brand new password", STRANGER, "")
            .await
            .status,
        200
    );
    assert_eq!(
        world
            .password_login("ada", PASSWORD, "198.51.100.5", "")
            .await
            .status,
        401
    );
    assert!(
        world
            .audit()
            .await
            .contains(&"auth.password_changed".to_owned())
    );
}

#[tokio::test]
async fn a_wrong_password_given_by_a_session_is_counted_for_that_session_alone() {
    let world = World::locking_after(3).await;
    let ada = world.person("ada").await;
    let this = world.sign_in(&ada).await;
    let other = world.sign_in(&ada).await;
    for _ in 0..3 {
        let wrong = change_password(&world, &this, "wrong password", "a brand new password")
            .await
            .problem();
        assert_eq!(wrong.code, ErrorCode::AuthInvalidCredentials);
    }
    // Refused, right though it is, with when to come back.
    let locked = change_password(&world, &this, PASSWORD, "a brand new password").await;
    assert_eq!(locked.status, 423);
    assert_eq!(locked.headers["retry-after"], "10800");
    let problem = locked.problem();
    assert_eq!(problem.code, ErrorCode::AuthLocked);
    assert_eq!(problem.extra["way"], "password");
    // Another session of the same person, and the name's own count, are not held by it.
    assert_eq!(
        change_password(&world, &other, PASSWORD, "a brand new password")
            .await
            .status,
        204
    );
    assert_eq!(
        world
            .password_login("ada", "a brand new password", STRANGER, "")
            .await
            .status,
        200
    );
}

#[tokio::test]
async fn an_authenticator_is_set_up_switched_on_and_switched_off_with_codes_that_work_once() {
    let world = World::new().await;
    let ada = world.person("ada").await;
    let signed = world.sign_in(&ada).await;
    let setup = |password: &str| {
        world
            .app
            .call(MeTotpSetup, &[])
            .token(signed.token())
            .body(&MeTotpSetupBody {
                password: Some(password.to_owned()),
                identity_proof: None,
            })
            .send()
    };
    assert_eq!(
        setup("wrong password").await.problem().code,
        ErrorCode::AuthInvalidCredentials
    );
    let reply = setup(PASSWORD).await;
    assert_eq!(reply.headers["cache-control"], "no-store");
    let first = reply.ok();
    assert!(
        first
            .otpauth_url
            .starts_with("otpauth://totp/Orch:ada?secret="),
        "{}",
        first.otpauth_url
    );
    assert!(first.otpauth_url.contains(&first.secret));
    // Asking again before it is on gives the same secret.
    assert_eq!(setup(PASSWORD).await.ok().secret, first.secret);
    let secret = totp_rs::Secret::try_from_base32(&first.secret)
        .unwrap()
        .as_bytes()
        .to_vec();
    // The secret is kept sealed, under the master key and bound to the person.
    let stored: Vec<u8> = sqlx::query_scalar("SELECT totp_secret FROM users WHERE id = $1")
        .bind(ada.id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(stored.len(), 24 + 20 + 16);
    assert!(
        !stored
            .windows(secret.len())
            .any(|window| window == secret.as_slice())
    );

    let enable = |code: String| {
        world
            .app
            .call(MeTotpEnable, &[])
            .token(signed.token())
            .body(&MeTotpEnableBody { code })
            .send()
    };
    assert_eq!(
        enable("000000".to_owned()).await.problem().code,
        ErrorCode::AuthSecondStepInvalid
    );
    assert_eq!(enable(world.totp_code(&secret)).await.status, 200);
    let me = world
        .app
        .call(MeGet, &[])
        .token(signed.token())
        .send()
        .await
        .ok();
    assert!(me.totp_enabled);
    // Not set up again over one that is on.
    assert_eq!(
        setup(PASSWORD).await.problem().code,
        ErrorCode::ConflictExists
    );
    // The code that switched it on is spent.
    assert_eq!(
        enable(world.totp_code(&secret)).await.problem().code,
        ErrorCode::ConflictExists
    );

    // Switching it off takes the password and a code, fresh ones.
    world.advance(step());
    let disable = |password: &str, code: String| {
        world
            .app
            .call(MeTotpDisable, &[])
            .token(signed.token())
            .body(&MeTotpDisableBody {
                password: password.to_owned(),
                code,
            })
            .send()
    };
    assert_eq!(
        disable("wrong password", world.totp_code(&secret))
            .await
            .problem()
            .code,
        ErrorCode::AuthInvalidCredentials
    );
    assert_eq!(
        disable(PASSWORD, "000000".to_owned()).await.problem().code,
        ErrorCode::AuthSecondStepInvalid
    );
    assert_eq!(
        disable(PASSWORD, world.totp_code(&secret)).await.status,
        204
    );
    let me = world
        .app
        .call(MeGet, &[])
        .token(signed.token())
        .send()
        .await
        .ok();
    assert!(!me.totp_enabled);
    assert_eq!(
        world
            .password_login("ada", PASSWORD, STRANGER, "")
            .await
            .status,
        200
    );
    let audit = world.audit().await;
    assert!(
        audit.contains(&"auth.totp_enabled".to_owned())
            && audit.contains(&"auth.totp_disabled".to_owned())
    );
    // And it can be set up anew, with a secret of its own.
    assert_ne!(setup(PASSWORD).await.ok().secret, first.secret);
}

#[tokio::test]
async fn an_authenticator_is_not_switched_on_without_being_set_up_and_a_session_gets_few_tries_at_its_code()
 {
    let world = World::locking_after(3).await;
    let ada = world.person("ada").await;
    let signed = world.sign_in(&ada).await;
    let enable = |code: &str| {
        world
            .app
            .call(MeTotpEnable, &[])
            .token(signed.token())
            .body(&MeTotpEnableBody {
                code: code.to_owned(),
            })
            .send()
    };
    let nothing = enable("123456").await.problem();
    assert_eq!(nothing.extra["errors"][0]["path"], "code");
    world
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
    for _ in 0..3 {
        assert_eq!(
            enable("000000").await.problem().code,
            ErrorCode::AuthSecondStepInvalid
        );
    }
    let locked = enable("000000").await.problem();
    assert_eq!((locked.status, locked.code), (423, ErrorCode::AuthLocked));
    assert_eq!(locked.extra["way"], "totp");
}

#[tokio::test]
async fn a_mailed_code_becomes_a_second_step_only_where_the_orgs_allow_it_and_the_person_asks() {
    let world = World::new().await;
    world.mail_server().await;
    world
        .set("registration.email_verification", json!("off"))
        .await;
    let unconfirmed = world.person_with("bob", PASSWORD, false).await;
    let bobs = world.sign_in(&unconfirmed).await;
    // No confirmed address, no code.
    let none = mail_a_code(&world, &bobs, PASSWORD).await.problem();
    assert_eq!(
        (none.status, none.code),
        (403, ErrorCode::AuthEmailUnverified)
    );

    let ada = world.person("ada").await;
    let signed = world.sign_in(&ada).await;
    let wrong = mail_a_code(&world, &signed, "wrong password")
        .await
        .problem();
    assert_eq!(wrong.code, ErrorCode::AuthInvalidCredentials);
    assert_eq!(mail_a_code(&world, &signed, PASSWORD).await.status, 202);
    let code = world.mailed_code("ada@example.org").await;
    // The orgs have not allowed it as a second step, as shipped.
    let me = world
        .app
        .call(MeGet, &[])
        .token(signed.token())
        .send()
        .await
        .ok();
    assert!(me.email_code.is_empty());
    let refused = enable_code(&world, &signed, &code).await.problem();
    assert_eq!(
        (refused.status, refused.code),
        (403, ErrorCode::AuthEmailCodeNotAllowed)
    );

    world.set("auth.email_code", json!(["second_step"])).await;
    let me = world
        .app
        .call(MeGet, &[])
        .token(signed.token())
        .send()
        .await
        .ok();
    assert_eq!(me.email_code, vec![EmailCodeUse::SecondStep]);
    assert_eq!(
        enable_code(&world, &signed, "ABCD-1234")
            .await
            .problem()
            .code,
        ErrorCode::AuthSecondStepInvalid
    );
    assert_eq!(enable_code(&world, &signed, &code).await.status, 204);
    let me = world
        .app
        .call(MeGet, &[])
        .token(signed.token())
        .send()
        .await
        .ok();
    assert!(me.email_code_second_step);
    // The code was spent by switching it on.
    assert_eq!(
        enable_code(&world, &signed, &code).await.problem().code,
        ErrorCode::AuthSecondStepInvalid
    );

    // From now on a sign-in asks for it.
    let asked = world
        .password_login("ada", PASSWORD, STRANGER, "")
        .await
        .problem();
    assert_eq!(asked.code, ErrorCode::AuthSecondStepRequired);
    assert_eq!(asked.extra["methods"], json!(["email_code"]));
    world.advance(Duration::seconds(61));
    let again = world
        .app
        .call(AuthEmailCodeSend, &[])
        .body(&AuthEmailCodeSendBody {
            login: "ada".to_owned(),
            password: Some(PASSWORD.to_owned()),
        })
        .send()
        .await;
    assert_eq!(again.status, 202);
    let second = world.mailed_code("ada@example.org").await;
    let reply = world
        .attempt(
            AuthLoginBody {
                login: "ada".to_owned(),
                password: Some(PASSWORD.to_owned()),
                email_code: Some(second),
                totp: None,
                recovery_code: None,
            },
            STRANGER,
            "",
        )
        .await;
    assert_eq!(reply.status, 200);
    assert!(reply.ok().user.email_code_second_step);
}

#[tokio::test]
async fn a_mailed_code_stops_being_a_second_step_with_the_password_and_a_code_and_the_pace_of_codes_holds()
 {
    let world = World::new().await;
    world.mail_server().await;
    world.set("auth.email_code", json!(["second_step"])).await;
    let ada = world.person("ada").await;
    let signed = world.sign_in(&ada).await;
    mail_a_code(&world, &signed, PASSWORD).await.ok();
    let code = world.mailed_code("ada@example.org").await;
    assert_eq!(enable_code(&world, &signed, &code).await.status, 204);

    // One a minute, to this person, whoever asks and from wherever they do.
    let quick = mail_a_code(&world, &signed, PASSWORD).await;
    assert_eq!(quick.status, 429);
    assert!(quick.headers.contains_key("retry-after"));
    world.advance(Duration::seconds(61));
    assert_eq!(mail_a_code(&world, &signed, PASSWORD).await.status, 202);
    let code = world.mailed_code("ada@example.org").await;

    let disable = |password: &str, code: &str| {
        world
            .app
            .call(MeEmailCodeDisable, &[])
            .token(signed.token())
            .body(&MeEmailCodeDisableBody {
                password: password.to_owned(),
                code: code.to_owned(),
            })
            .send()
    };
    assert_eq!(
        disable("wrong password", &code).await.problem().code,
        ErrorCode::AuthInvalidCredentials
    );
    assert_eq!(
        disable(PASSWORD, "ABCD-1234").await.problem().code,
        ErrorCode::AuthSecondStepInvalid
    );
    assert_eq!(disable(PASSWORD, &code).await.status, 204);
    let me = world
        .app
        .call(MeGet, &[])
        .token(signed.token())
        .send()
        .await
        .ok();
    assert!(!me.email_code_second_step);
    assert_eq!(
        world
            .password_login("ada", PASSWORD, STRANGER, "")
            .await
            .status,
        200
    );
    assert!(
        world
            .audit()
            .await
            .contains(&"auth.email_code_disabled".to_owned())
    );
}

#[tokio::test]
async fn a_mailed_second_step_can_be_switched_off_with_a_code_of_the_authenticator() {
    let world = World::new().await;
    world.mail_server().await;
    world.set("auth.email_code", json!(["second_step"])).await;
    let ada = world.person("ada").await;
    let signed = world.sign_in(&ada).await;
    let secret = world.authenticator(&signed).await;
    mail_a_code(&world, &signed, PASSWORD).await.ok();
    let code = world.mailed_code("ada@example.org").await;
    enable_code(&world, &signed, &code).await.ok();
    world.advance(step());
    let off = world
        .app
        .call(MeEmailCodeDisable, &[])
        .token(signed.token())
        .body(&MeEmailCodeDisableBody {
            password: PASSWORD.to_owned(),
            code: world.totp_code(&secret),
        })
        .send()
        .await;
    assert_eq!(off.status, 204);
    let me = world
        .app
        .call(MeGet, &[])
        .token(signed.token())
        .send()
        .await
        .ok();
    assert!(!me.email_code_second_step && me.totp_enabled);
}

#[tokio::test]
async fn what_a_mailed_code_may_be_for_a_person_is_what_every_root_org_of_theirs_allows() {
    let world = World::new().await;
    world
        .set("auth.email_code", json!(["sign_in", "second_step"]))
        .await;
    let ada = world.person("ada").await;
    let signed = world.sign_in(&ada).await;
    // In no org but their own: the system's value.
    assert_eq!(
        email_code_of(&world, &signed).await,
        vec![EmailCodeUse::SignIn, EmailCodeUse::SecondStep]
    );
    // Their own org does not count, whatever it says.
    world
        .set_at(Target::Node(ada.personal_org), "auth.email_code", json!([]))
        .await;
    assert_eq!(
        email_code_of(&world, &signed).await,
        vec![EmailCodeUse::SignIn, EmailCodeUse::SecondStep]
    );

    // Two orgs of theirs, one that allows both and one that allows the second step alone.
    let (acme, globex): (Id, Id) = (orch_types::new_id(), orch_types::new_id());
    {
        let mut conn = world.conn().await;
        for (org, name) in [(acme, "Acme"), (globex, "Globex")] {
            orch_db::queries::hierarchy::testing::insert_node(
                &mut conn, org, "org", None, name, ada.id,
            )
            .await
            .unwrap();
            orch_db::queries::access::testing::insert_member(&mut conn, org, ada.id, org)
                .await
                .unwrap();
        }
    }
    world
        .set_at(
            Target::Node(acme),
            "auth.email_code",
            json!(["sign_in", "second_step"]),
        )
        .await;
    world
        .set_at(
            Target::Node(globex),
            "auth.email_code",
            json!(["second_step"]),
        )
        .await;
    assert_eq!(
        email_code_of(&world, &signed).await,
        vec![EmailCodeUse::SecondStep]
    );
    // One of them allows none.
    world
        .set_at(Target::Node(acme), "auth.email_code", json!([]))
        .await;
    assert!(email_code_of(&world, &signed).await.is_empty());
    // An org that sets nothing follows the system.
    world
        .set_at(
            Target::Node(acme),
            "auth.email_code",
            json!(["second_step"]),
        )
        .await;
    sqlx::query("DELETE FROM settings WHERE node_id = $1 AND key = 'auth.email_code'")
        .bind(globex)
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        email_code_of(&world, &signed).await,
        vec![EmailCodeUse::SecondStep]
    );

    // And none while the address is not confirmed.
    sqlx::query("UPDATE users SET email_verified_at = NULL WHERE id = $1")
        .bind(ada.id)
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert!(email_code_of(&world, &signed).await.is_empty());
}

#[tokio::test]
async fn the_sessions_of_a_person_are_listed_with_the_one_that_asks_and_ended_one_by_one() {
    let world = World::new().await;
    let ada = world.person("ada").await;
    let bob = world.person("bob").await;
    let this = world.sign_in(&ada).await;
    let other = world.sign_in(&ada).await;
    let bobs = world.sign_in(&bob).await;
    let list = |signed: &Signed| {
        world
            .app
            .call(MeSessionList, &[])
            .token(signed.token())
            .send()
    };

    let listed = list(&this).await.ok();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed.iter().filter(|session| session.current).count(), 1);
    let current = listed.iter().find(|session| session.current).unwrap();
    assert_eq!(current.ip.as_deref(), Some("203.0.113.1"));
    assert_eq!(current.user_agent, "tests/1.0");
    assert_eq!(current.expires_at.get(), world.now() + Duration::days(30));
    let other_id = listed.iter().find(|session| !session.current).unwrap().id;
    // Seen from an API token, none of them is the current one.
    let made = create_token(&world, &this, named("ci")).await.ok();
    let by_token = world
        .app
        .call(MeSessionList, &[])
        .token(made.token.as_deref().unwrap())
        .send()
        .await
        .ok();
    assert!(by_token.iter().all(|session| !session.current));

    // Not somebody else's, and not one that is none.
    let end = |id: &dyn std::fmt::Display| {
        world
            .app
            .call(MeSessionDelete, &[id])
            .token(this.token())
            .send()
    };
    let theirs = bobs_session(&world, &bobs).await;
    assert_eq!(end(&theirs).await.problem().code, ErrorCode::NotFound);
    assert_eq!(
        end(&orch_types::new_id()).await.problem().code,
        ErrorCode::NotFound
    );
    assert_eq!(end(&"not-an-id").await.problem().code, ErrorCode::NotFound);
    assert_eq!(
        world
            .app
            .call(MeGet, &[])
            .token(bobs.token())
            .send()
            .await
            .status,
        200
    );

    assert_eq!(end(&other_id).await.status, 204);
    assert_eq!(
        world
            .app
            .call(MeGet, &[])
            .token(other.token())
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthInvalidToken
    );
    assert_eq!(list(&this).await.ok().len(), 1);
    assert_eq!(end(&other_id).await.problem().code, ErrorCode::NotFound);
    assert!(
        world
            .audit()
            .await
            .contains(&"auth.session_revoked".to_owned())
    );
}

async fn bobs_session(world: &World, bobs: &Signed) -> Id {
    world
        .app
        .call(MeSessionList, &[])
        .token(bobs.token())
        .send()
        .await
        .ok()[0]
        .id
}

#[tokio::test]
async fn an_api_token_is_shown_once_stored_as_a_hash_and_made_only_from_a_browser_session() {
    let world = World::new().await;
    let ada = world.person("ada").await;
    let signed = world.sign_in(&ada).await;
    let reply = create_token(&world, &signed, named("  ci  ")).await;
    assert_eq!(reply.headers["cache-control"], "no-store");
    let made = reply.ok();
    assert!(made.token.as_deref().unwrap().starts_with("opat_"));
    assert_eq!(
        (made.info.name.as_str(), made.info.scopes.clone()),
        ("ci", vec!["*".to_owned()])
    );
    assert!(
        made.info.node.is_none()
            && made.info.expires_at.is_none()
            && made.info.last_used_at.is_none()
    );
    // Only the hash is kept, and the listing never shows the secret.
    let stored: i64 =
        sqlx::query_scalar("SELECT count(*) FROM api_tokens WHERE id = $1 AND token_hash = $2")
            .bind(made.info.id)
            .bind(hash(made.token.as_deref().unwrap()))
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(stored, 1);
    let listed = world
        .app
        .call(MeTokenList, &[])
        .token(signed.token())
        .send()
        .await;
    assert!(!String::from_utf8_lossy(&listed.raw).contains(made.token.as_deref().unwrap()));
    assert_eq!(listed.ok().len(), 1);

    // It acts as its person, and its last use is noted.
    let me = world
        .app
        .call(MeGet, &[])
        .token(made.token.as_deref().unwrap())
        .send()
        .await
        .ok();
    assert_eq!(me.user.username, "ada");
    let after = world
        .app
        .call(MeTokenList, &[])
        .token(made.token.as_deref().unwrap())
        .send()
        .await
        .ok();
    assert!(after[0].last_used_at.is_some());
    // But it makes none of its own: it is not a browser session.
    let refused = world
        .app
        .call(MeTokenCreate, &[])
        .token(made.token.as_deref().unwrap())
        .body(&named("more"))
        .send()
        .await
        .problem();
    assert_eq!(
        (refused.status, refused.code),
        (403, ErrorCode::AuthSessionRequired)
    );
    assert!(
        world
            .audit()
            .await
            .contains(&"auth.token_created".to_owned())
    );
}

#[tokio::test]
async fn an_api_token_is_made_with_the_scopes_the_node_and_the_end_it_is_asked_for() {
    let world = World::new().await;
    let ada = world.person("ada").await;
    let bob = world.person("bob").await;
    let signed = world.sign_in(&ada).await;
    let (project, theirs) = (orch_types::new_id(), orch_types::new_id());
    {
        let mut conn = world.conn().await;
        orch_db::queries::hierarchy::testing::insert_node(
            &mut conn,
            project,
            "project",
            Some(ada.personal_org),
            "Web",
            ada.id,
        )
        .await
        .unwrap();
        orch_db::queries::hierarchy::testing::insert_node(
            &mut conn,
            theirs,
            "project",
            Some(bob.personal_org),
            "Private",
            bob.id,
        )
        .await
        .unwrap();
    }
    let ends = world.now() + Duration::days(1);
    let made = create_token(
        &world,
        &signed,
        ApiTokenCreateBody {
            name: "reader".to_owned(),
            scopes: Some(vec!["thread.view".to_owned(), "project.view".to_owned()]),
            node_id: Some(project),
            expires_at: Some(ends.into()),
            proof: orch_api_contract::auth::SensitiveProof {
                password: Some(support::PASSWORD.to_owned()),
                ..Default::default()
            },
        },
    )
    .await
    .ok();
    let mut scopes = made.info.scopes.clone();
    scopes.sort();
    assert_eq!(scopes, ["project.view", "thread.view"]);
    let node = made.info.node.clone().unwrap();
    assert_eq!(
        (node.id, node.kind, node.name.as_deref(), node.redacted),
        (project, NodeKind::Project, Some("Web"), false)
    );
    assert_eq!(made.info.expires_at.map(|at| at.get()), Some(ends));

    let bad = |body: ApiTokenCreateBody| create_token(&world, &signed, body);
    let mut empty = named("x");
    empty.scopes = Some(Vec::new());
    let unscoped = bad(empty).await.ok();
    assert!(unscoped.info.scopes.is_empty());
    assert_eq!(
        world
            .app
            .call(NodeGet, &[&project])
            .token(unscoped.token.as_deref().unwrap())
            .send()
            .await
            .status,
        403,
        "an empty token grants none of its person's project authority"
    );
    let mut unknown = named("x");
    unknown.scopes = Some(vec!["thread.view".to_owned(), "teleport".to_owned()]);
    assert_eq!(
        bad(unknown).await.problem().extra["errors"][0]["path"],
        "scopes"
    );
    let mut past = named("x");
    past.expires_at = Some((world.now() - Duration::hours(1)).into());
    assert_eq!(
        bad(past).await.problem().extra["errors"][0]["path"],
        "expires_at"
    );
    assert_eq!(
        bad(named("")).await.problem().extra["errors"][0]["path"],
        "name"
    );
    // A node they cannot see is none, and so is one that does not exist.
    let mut hidden = named("x");
    hidden.node_id = Some(theirs);
    assert_eq!(bad(hidden).await.problem().code, ErrorCode::NotFound);
    let mut nowhere = named("x");
    nowhere.node_id = Some(orch_types::new_id());
    assert_eq!(bad(nowhere).await.problem().code, ErrorCode::NotFound);

    // It stops working when it ends.
    assert_eq!(
        world
            .app
            .call(MeGet, &[])
            .token(made.token.as_deref().unwrap())
            .send()
            .await
            .status,
        200
    );
    world.advance(Duration::days(1) + Duration::seconds(1));
    let late = world
        .app
        .call(MeGet, &[])
        .token(made.token.as_deref().unwrap())
        .send()
        .await
        .problem();
    assert_eq!(late.code, ErrorCode::AuthInvalidToken);
}

#[tokio::test]
async fn a_revoked_api_token_stops_working_at_once_and_only_its_person_revokes_it() {
    let world = World::new().await;
    let ada = world.person("ada").await;
    let bob = world.person("bob").await;
    let signed = world.sign_in(&ada).await;
    let bobs = world.sign_in(&bob).await;
    let made = create_token(&world, &signed, named("ci")).await.ok();
    let revoke = |by: &Signed, id: Id| {
        world
            .app
            .call(MeTokenDelete, &[&id])
            .token(by.token())
            .send()
    };

    assert_eq!(
        revoke(&bobs, made.info.id).await.problem().code,
        ErrorCode::NotFound
    );
    assert_eq!(
        world
            .app
            .call(MeGet, &[])
            .token(made.token.as_deref().unwrap())
            .send()
            .await
            .status,
        200
    );
    let db = orch_db::Db::from_pool(world.db.pool.clone());
    let mut notices = orch_db::sequences::listen(
        &db,
        &[
            orch_db::sequences::IDENTITY_CHANNEL,
            orch_db::sequences::AUTHZ_CHANNEL,
        ],
    )
    .await
    .unwrap();
    let before = epoch_of(&world).await;
    assert_eq!(revoke(&signed, made.info.id).await.status, 204);
    assert_eq!(
        epoch_of(&world).await,
        before,
        "personal credentials do not change global permissions"
    );
    // Match the existing database notification regression's bounded wait.
    let notice = tokio::time::timeout(std::time::Duration::from_secs(5), notices.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        notice,
        orch_db::sequences::Notice::Heard {
            channel: orch_db::sequences::IDENTITY_CHANNEL.to_owned(),
            payload: ada.id.to_string(),
        }
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), notices.next())
            .await
            .is_err(),
        "there is no global epoch notification"
    );
    let gone = world
        .app
        .call(MeGet, &[])
        .token(made.token.as_deref().unwrap())
        .send()
        .await
        .problem();
    assert_eq!(gone.code, ErrorCode::AuthInvalidToken);
    assert!(
        world
            .app
            .call(MeTokenList, &[])
            .token(signed.token())
            .send()
            .await
            .ok()
            .is_empty()
    );
    assert_eq!(
        revoke(&signed, made.info.id).await.problem().code,
        ErrorCode::NotFound
    );
    assert!(
        world
            .audit()
            .await
            .contains(&"auth.token_revoked".to_owned())
    );
}
