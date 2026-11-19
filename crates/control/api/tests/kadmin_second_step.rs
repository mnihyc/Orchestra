//! A second step an org requires: its work waits for the setup, then for each session to give it; the person's own org and every org that requires nothing stay open; and Members tells only its managers who has one and when they were last seen there.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use chrono::Duration;
use orch_api_contract::auth::{
    MeGet, MeSecondStepVerify, MeSecondStepVerifyBody, MeTotpEnable, MeTotpEnableBody, MeTotpSetup,
    MeTotpSetupBody,
};
use orch_api_contract::common::{ErrorCode, Page};
use orch_api_contract::directory::{Member, MemberSecondStep, OrgMemberList, OrgMemberListQuery};
use orch_api_contract::nodes::{Node, NodeGet, NodeOrSystem};
use orch_api_core::middleware::authentication::{ACCESS_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::Reply;
use orch_db::queries::identity::testing as user_rows;
use orch_settings::Target;
use orch_types::{Id, new_id};
use places::{Who, admin, join, make_org, person};
use serde_json::json;
use support::{PASSWORD, World, step};

async fn read(world: &World, token: &str, id: Id) -> Reply<Node> {
    world.app.call(NodeGet, &[&id]).token(token).send().await
}

/// Sets an authenticator up and switches it on in the session of `token`; the secret is returned.
async fn authenticator(world: &World, token: &str) -> Vec<u8> {
    let reply = world
        .app
        .call(MeTotpSetup, &[])
        .token(token)
        .body(&MeTotpSetupBody {
            password: Some(PASSWORD.to_owned()),
            identity_proof: None,
        })
        .send()
        .await
        .ok();
    let secret = totp_rs::Secret::try_from_base32(&reply.secret)
        .unwrap()
        .as_bytes()
        .to_vec();
    let done = world
        .app
        .call(MeTotpEnable, &[])
        .token(token)
        .body(&MeTotpEnableBody {
            code: world.totp_code(&secret),
        })
        .send()
        .await;
    assert_eq!(done.status, 200, "{:?}", done.body);
    secret
}

/// Another browser session of `who`, which has given no second proof.
async fn another_session(world: &World, who: &Who) -> String {
    let token = new_token(ACCESS_TOKEN_PREFIX);
    let mut conn = world.conn().await;
    user_rows::insert_session(
        &mut conn,
        new_id(),
        who.id(),
        &token_hash(&token),
        world.now() + Duration::days(30),
    )
    .await
    .unwrap();
    token
}

async fn members(world: &World, token: &str, org: Id) -> Page<Member> {
    world
        .app
        .call(OrgMemberList, &[&org])
        .query(&OrgMemberListQuery::default())
        .token(token)
        .send()
        .await
        .ok()
}

async fn require_at(world: &World, org: Id, value: &str) {
    world
        .set_at(Target::Node(org), "auth.second_step", json!(value))
        .await;
}

#[tokio::test]
async fn an_org_that_requires_a_second_step_waits_for_its_setup_while_the_persons_own_org_stays_open()
 {
    let world = World::new().await;
    let root = admin(&world, "root").await;
    let acme = make_org(&world, &root, "Acme", "acme").await.base.id;
    let ada = person(&world, "ada").await;
    join(&world, acme, acme, &ada, "developer", &root).await;
    read(&world, &ada.token, acme).await.ok();

    require_at(&world, acme, "required").await;
    let waiting = read(&world, &ada.token, acme).await.problem();
    assert_eq!(
        (waiting.status, waiting.code),
        (403, ErrorCode::AuthSecondStepSetup)
    );
    assert_eq!(waiting.extra["path"], "/settings/security");
    assert_eq!(waiting.extra["scopes"][0]["id"], json!(acme));
    read(&world, &ada.token, ada.person.personal_org).await.ok();
    let me = world
        .app
        .call(MeGet, &[])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert!(me.second_step_required);
    assert!(matches!(
        me.second_step_sources.as_slice(),
        [NodeOrSystem::Node(org)] if org.id == acme
    ));

    // Setting a step up with its confirming code gives it for this session as well.
    authenticator(&world, &ada.token).await;
    read(&world, &ada.token, acme).await.ok();
}

#[tokio::test]
async fn a_session_that_has_not_given_the_second_step_is_asked_for_it_and_one_code_lets_it_in() {
    let world = World::new().await;
    let root = admin(&world, "root").await;
    let acme = make_org(&world, &root, "Acme", "acme").await.base.id;
    let ada = person(&world, "ada").await;
    join(&world, acme, acme, &ada, "developer", &root).await;
    let secret = authenticator(&world, &ada.token).await;
    require_at(&world, acme, "required").await;

    let later = another_session(&world, &ada).await;
    let asked = read(&world, &later, acme).await.problem();
    assert_eq!(
        (asked.status, asked.code),
        (401, ErrorCode::AuthSecondStepRequired)
    );
    assert_eq!(asked.extra["methods"], json!(["totp", "recovery_code"]));
    read(&world, &later, ada.person.personal_org).await.ok();

    world.advance(step());
    let verify = |token: String, code: String| {
        let world = &world;
        async move {
            world
                .app
                .call(MeSecondStepVerify, &[])
                .token(&token)
                .body(&MeSecondStepVerifyBody {
                    totp: Some(code),
                    email_code: None,
                    recovery_code: None,
                })
                .send()
                .await
        }
    };
    let wrong = verify(later.clone(), "000000".to_owned()).await.problem();
    assert_eq!(wrong.code, ErrorCode::AuthSecondStepInvalid);
    let right = world.totp_code(&secret);
    let me = verify(later.clone(), right.clone()).await.ok();
    assert!(me.second_step_required);
    read(&world, &later, acme).await.ok();

    // The code is spent: it gives no other session its step, and that session still waits.
    let third = another_session(&world, &ada).await;
    let again = verify(third.clone(), right).await.problem();
    assert_eq!(again.code, ErrorCode::AuthSecondStepInvalid);
    assert_eq!(
        read(&world, &third, acme).await.problem().code,
        ErrorCode::AuthSecondStepRequired
    );
}

#[tokio::test]
async fn an_org_that_requires_nothing_stays_open_and_does_not_weaken_the_one_that_does() {
    let world = World::new().await;
    let root = admin(&world, "root").await;
    let acme = make_org(&world, &root, "Acme", "acme").await.base.id;
    let globex = make_org(&world, &root, "Globex", "globex").await.base.id;
    let ada = person(&world, "ada").await;
    join(&world, acme, acme, &ada, "developer", &root).await;
    join(&world, globex, globex, &ada, "developer", &root).await;
    require_at(&world, acme, "required").await;
    require_at(&world, globex, "optional").await;
    assert_eq!(
        read(&world, &ada.token, acme).await.problem().code,
        ErrorCode::AuthSecondStepSetup
    );
    read(&world, &ada.token, globex).await.ok();
    let me = world
        .app
        .call(MeGet, &[])
        .token(&ada.token)
        .send()
        .await
        .ok();
    assert!(
        me.second_step_required,
        "one org that requires it is enough"
    );
}

#[tokio::test]
async fn members_tells_only_whoever_manages_them_who_has_a_second_step_and_when_they_were_last_seen_here()
 {
    let world = World::new().await;
    let root = admin(&world, "root").await;
    let acme = make_org(&world, &root, "Acme", "acme").await.base.id;
    let ada = person(&world, "ada").await;
    // A username has three characters at least.
    let bo = person(&world, "bob").await;
    join(&world, acme, acme, &ada, "developer", &root).await;
    join(&world, acme, acme, &bo, "viewer", &root).await;
    read(&world, &ada.token, acme).await.ok();

    // A member who does not manage members reads neither column, and no value in its place.
    for member in members(&world, &bo.token, acme).await.items {
        assert_eq!(
            (
                member.second_step,
                member.second_step_required,
                member.last_seen_at
            ),
            (None, None, None)
        );
    }

    // Whoever manages them reads each, with the requirement where it applies.
    authenticator(&world, &root.token).await;
    require_at(&world, acme, "required").await;
    let managed = members(&world, &root.token, acme).await;
    let row = managed
        .items
        .iter()
        .find(|m| m.user.id == ada.id())
        .unwrap();
    assert_eq!(row.second_step, Some(MemberSecondStep::Missing));
    assert_eq!(row.second_step_required, Some(true));
    assert!(
        row.last_seen_at.is_some(),
        "she read the org before it required a step"
    );
}
