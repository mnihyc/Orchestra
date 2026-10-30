//! API tokens made narrow and with a fresh proof, within the longest life their places admit, answered again by their key without their secret, and refused by an org.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use chrono::Duration;
use orch_api_contract::auth::{
    ApiTokenCreateBody, MeGet, MeTokenCreate, MeTokenList, OrgTokenList, OrgTokenListQuery,
    OrgTokenRefuse, SensitiveProof, TokenCreated,
};
use orch_api_contract::common::{ErrorCode, IDEMPOTENCY_HEADER};
use orch_api_contract::nodes::{DeletedInspectionQuery, NodeGet};
use orch_api_core::testing::Reply;
use orch_settings::Target;
use orch_types::Time;
use serde_json::json;
use support::{PASSWORD, Signed, World, step};

fn body(scopes: Option<&[&str]>, ends_in_days: Option<i64>, world: &World) -> ApiTokenCreateBody {
    ApiTokenCreateBody {
        name: "automation".to_owned(),
        scopes: scopes.map(|scopes| scopes.iter().map(|scope| (*scope).to_owned()).collect()),
        node_id: None,
        expires_at: ends_in_days.map(|days| Time::new(world.now() + Duration::days(days))),
        proof: SensitiveProof {
            password: Some(PASSWORD.to_owned()),
            ..SensitiveProof::default()
        },
    }
}

async fn make(
    world: &World,
    signed: &Signed,
    body: &ApiTokenCreateBody,
    key: Option<&str>,
) -> Reply<TokenCreated> {
    let call = world.app.call(MeTokenCreate, &[]).token(signed.token());
    let call = match key {
        Some(key) => call.header(IDEMPOTENCY_HEADER, key),
        None => call,
    };
    call.body(body).send().await
}

async fn read_org(
    world: &World,
    token: &str,
    org: orch_types::Id,
) -> Reply<orch_api_contract::nodes::Node> {
    world
        .app
        .call(NodeGet, &[&org])
        .query(&DeletedInspectionQuery::default())
        .token(token)
        .send()
        .await
}

#[tokio::test]
async fn a_token_made_without_scopes_may_use_nothing_and_everything_is_asked_for_by_name() {
    let world = World::new().await;
    let ada = world.person("ada").await;
    let signed = world.sign_in(&ada).await;
    let narrow = make(&world, &signed, &body(None, None, &world), None)
        .await
        .ok();
    assert!(narrow.info.scopes.is_empty());
    let refused = read_org(&world, narrow.token.as_deref().unwrap(), ada.personal_org)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    let whole = make(&world, &signed, &body(Some(&["*"]), None, &world), None)
        .await
        .ok();
    assert_eq!(whole.info.scopes, ["*"]);
    read_org(&world, whole.token.as_deref().unwrap(), ada.personal_org)
        .await
        .ok();
}

#[tokio::test]
async fn a_token_is_made_only_with_a_fresh_password_and_the_second_step_in_the_same_request() {
    let world = World::new().await;
    let ada = world.person("ada").await;
    let signed = world.sign_in(&ada).await;
    let mut without = body(Some(&["*"]), None, &world);
    without.proof = SensitiveProof::default();
    let missing = make(&world, &signed, &without, None).await.problem();
    assert_eq!(
        (missing.status, missing.code),
        (403, ErrorCode::AuthReauthenticationRequired)
    );
    let mut wrong = body(Some(&["*"]), None, &world);
    wrong.proof.password = Some("not my password at all".to_owned());
    let refused = make(&world, &signed, &wrong, None).await.problem();
    assert_eq!(refused.code, ErrorCode::AuthInvalidCredentials);
    // An identity provider's proof does not stand for a password the person has.
    let mut instead = body(Some(&["*"]), None, &world);
    instead.proof = SensitiveProof {
        identity_proof: Some("whatever".to_owned()),
        ..SensitiveProof::default()
    };
    let replaced = make(&world, &signed, &instead, None).await.problem();
    assert_eq!(replaced.code, ErrorCode::AuthReauthenticationRequired);
    let listed = world
        .app
        .call(MeTokenList, &[])
        .token(signed.token())
        .send()
        .await
        .ok();
    assert!(listed.is_empty(), "nothing was made");

    // With an authenticator, the password alone is not enough, and a code of it completes the proof.
    let secret = world.authenticator(&signed).await;
    world.advance(step());
    let asked = make(&world, &signed, &body(Some(&["*"]), None, &world), None)
        .await
        .problem();
    assert_eq!(asked.code, ErrorCode::AuthSecondStepRequired);
    assert_eq!(asked.extra["methods"], json!(["totp", "recovery_code"]));
    let mut bad_code = body(Some(&["*"]), None, &world);
    bad_code.proof.totp = Some("000000".to_owned());
    assert_eq!(
        make(&world, &signed, &bad_code, None).await.problem().code,
        ErrorCode::AuthSecondStepInvalid
    );
    let mut with_code = body(Some(&["*"]), None, &world);
    with_code.proof.totp = Some(world.totp_code(&secret));
    let made = make(&world, &signed, &with_code, None).await.ok();
    assert!(made.token.is_some() && !made.already_created);
    // The same code is spent: it does not make a second token.
    assert_eq!(
        make(&world, &signed, &with_code, None).await.problem().code,
        ErrorCode::AuthSecondStepInvalid
    );
}

#[tokio::test]
async fn the_same_creation_asked_again_by_its_key_answers_what_was_made_without_the_secret() {
    let world = World::new().await;
    let ada = world.person("ada").await;
    let signed = world.sign_in(&ada).await;
    let key = "4f0d8c1e-0d9a-4f53-9a5e-2b0c3d4e5f60";
    let first = make(
        &world,
        &signed,
        &body(Some(&["*"]), None, &world),
        Some(key),
    )
    .await
    .ok();
    assert!(first.token.is_some() && !first.already_created);
    // Asked again, even with a proof that would now be wrong: what was made, and no secret.
    let mut again = body(Some(&["*"]), None, &world);
    again.proof.password = Some("not my password at all".to_owned());
    let repeat = make(&world, &signed, &again, Some(key)).await.ok();
    assert_eq!(repeat.token, None);
    assert!(repeat.already_created);
    assert_eq!(repeat.info.id, first.info.id);
    let listed = world
        .app
        .call(MeTokenList, &[])
        .token(signed.token())
        .send()
        .await
        .ok();
    assert_eq!(listed.len(), 1, "no second token");
}

#[tokio::test]
async fn a_token_lives_no_longer_than_its_places_admit_where_it_is_made_and_where_it_reaches() {
    let world = World::new().await;
    let ada = world.person("ada").await;
    let signed = world.sign_in(&ada).await;
    world.set("tokens.max_days", json!(30)).await;
    let forever = make(&world, &signed, &body(Some(&["*"]), None, &world), None)
        .await
        .problem();
    assert_eq!(
        (forever.status, forever.code),
        (403, ErrorCode::TokenLifetimeExceeded)
    );
    assert_eq!(forever.extra["max_days"], 30);
    let month = make(&world, &signed, &body(Some(&["*"]), Some(10), &world), None)
        .await
        .ok();
    let token = month.token.unwrap();
    read_org(&world, &token, ada.personal_org).await.ok();
    // An org that admits less keeps it out there, from the next request on; elsewhere it works.
    world
        .set_at(Target::Node(ada.personal_org), "tokens.max_days", json!(5))
        .await;
    let kept_out = read_org(&world, &token, ada.personal_org).await.problem();
    assert_eq!(kept_out.code, ErrorCode::TokenLifetimeExceeded);
    assert_eq!(kept_out.extra["max_days"], 5);
    world.app.call(MeGet, &[]).token(&token).send().await.ok();
}

#[tokio::test]
async fn an_org_refuses_a_token_there_and_the_token_works_elsewhere() {
    let world = World::new().await;
    let ada = world.person("ada").await;
    let signed = world.sign_in(&ada).await;
    let made = make(&world, &signed, &body(Some(&["*"]), None, &world), None)
        .await
        .ok();
    let token = made.token.unwrap();
    let listed = world
        .app
        .call(OrgTokenList, &[&ada.personal_org])
        .query(&OrgTokenListQuery::default())
        .token(signed.token())
        .send()
        .await
        .ok();
    let row = listed
        .items
        .iter()
        .find(|row| row.info.id == made.info.id)
        .unwrap();
    assert!(!row.refused_here && row.lifetime_allowed && !row.service);
    assert!(
        !serde_json::to_string(&listed).unwrap().contains(&token),
        "no secret"
    );

    let refused = world
        .app
        .call(OrgTokenRefuse, &[&ada.personal_org, &made.info.id])
        .token(signed.token())
        .send()
        .await;
    assert_eq!(refused.status, 204);
    let kept_out = read_org(&world, &token, ada.personal_org).await.problem();
    assert_eq!(kept_out.code, ErrorCode::TokenRefusedHere);
    // Not revoked: the person's own requests through no place still answer it.
    world.app.call(MeGet, &[]).token(&token).send().await.ok();
    let again = world
        .app
        .call(OrgTokenList, &[&ada.personal_org])
        .query(&OrgTokenListQuery::default())
        .token(signed.token())
        .send()
        .await
        .ok();
    assert!(
        again
            .items
            .iter()
            .any(|row| row.info.id == made.info.id && row.refused_here)
    );
    assert!(
        world
            .audit()
            .await
            .contains(&"auth.token_refused".to_owned())
    );
}
