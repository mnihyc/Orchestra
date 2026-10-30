//! A reset link made by hand, for somebody whom no mail reaches: it works once and for half an hour, may switch a lost second step off, and the trail says who made it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use chrono::Duration;
use orch_api_contract::auth::{
    AuthRecoveryCheck, AuthRecoveryConfirm, AuthRecoveryConfirmBody, MeGet,
};
use orch_api_core::middleware::authentication::token_hash;
use orch_api_core::sign_in::reset_links::{self, Made};
use orch_api_core::testing::Reply;
use orch_db::audit::{Actor, Origin};
use orch_db::queries::identity;
use support::{PASSWORD, Person, World};

const NEW_PASSWORD: &str = "a new password, long enough";

#[tokio::test]
async fn checking_a_reset_link_leaves_it_unspent_and_unknown_used_expired_or_other_purpose_links_are_gone()
 {
    let world = World::new().await;
    let ada = world.person("ada").await;
    let signed = world.sign_in(&ada).await;
    let made = link_for(&world, &ada, false, Actor::System).await;
    let secret = secret_of(&made);
    let before = world.audit().await;
    for _ in 0..2 {
        let checked = world.app.call(AuthRecoveryCheck, &[&secret]).send().await;
        assert_eq!(checked.status, 204);
        assert_eq!(checked.headers[http::header::CACHE_CONTROL], "no-store");
        assert!(!checked.headers.contains_key(http::header::SET_COOKIE));
    }
    assert_eq!(
        world.audit().await,
        before,
        "checking consumes and changes nothing"
    );
    world
        .app
        .call(MeGet, &[])
        .token(signed.token())
        .send()
        .await
        .ok();
    use_link(&world, &made, NEW_PASSWORD).await.ok();
    let used = world
        .app
        .call(AuthRecoveryCheck, &[&secret])
        .send()
        .await
        .problem();
    assert_eq!(
        (used.status, used.code),
        (410, orch_api_contract::common::ErrorCode::Gone)
    );

    let expired = link_for(&world, &ada, false, Actor::System).await;
    let verification = "a-confirmation-token-is-not-a-reset-token";
    identity::create_email_token(
        &mut *world.conn().await,
        &identity::NewEmailToken {
            id: orch_types::new_id(),
            user_id: ada.id,
            purpose: "verify_email",
            token_hash: &token_hash(verification),
            email: "ada@example.test",
            clear_second_step: false,
            at: world.now(),
            expires_at: world.now() + Duration::hours(1),
        },
    )
    .await
    .unwrap();
    world.advance(Duration::minutes(31));
    for token in [
        "unknown-reset-token".to_owned(),
        secret_of(&expired),
        verification.to_owned(),
    ] {
        let refused = world
            .app
            .call(AuthRecoveryCheck, &[&token])
            .send()
            .await
            .problem();
        assert_eq!((refused.status, refused.code), (used.status, used.code));
        assert_eq!(
            refused.detail, used.detail,
            "the refusal reveals no account or token state"
        );
    }
}

async fn link_for(world: &World, person: &Person, clear_second_step: bool, by: Actor) -> Made {
    let mut tx = world.db.pool.begin().await.unwrap();
    let user = identity::user_by_id(&mut tx, person.id)
        .await
        .unwrap()
        .unwrap();
    let made = reset_links::make(
        &mut tx,
        &world.app.app.config,
        &user,
        clear_second_step,
        by,
        &Origin::default(),
        world.now(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    made
}

fn secret_of(made: &Made) -> String {
    made.url.rsplit('/').next().unwrap().to_owned()
}

async fn use_link(world: &World, made: &Made, password: &str) -> Reply<()> {
    world
        .app
        .call(AuthRecoveryConfirm, &[])
        .body(&AuthRecoveryConfirmBody {
            token: secret_of(made),
            password: password.to_owned(),
        })
        .send()
        .await
}

#[tokio::test]
async fn a_link_made_by_hand_sets_a_new_password_once_and_ends_the_sessions_there_were() {
    let world = World::new().await;
    let ada = world.person("ada").await;
    let signed = world.sign_in(&ada).await;
    let made = link_for(&world, &ada, false, Actor::System).await;

    let first = use_link(&world, &made, NEW_PASSWORD).await;
    let again = use_link(&world, &made, "another password, long enough").await;

    assert_eq!(first.status, 204);
    assert!(again.status.is_client_error(), "a link is good for one use");
    let with_new = world
        .password_login("ada", NEW_PASSWORD, "203.0.113.1", "")
        .await;
    assert!(with_new.status.is_success(), "the new password signs in");
    let with_old = world
        .password_login("ada", PASSWORD, "203.0.113.1", "")
        .await;
    assert!(!with_old.status.is_success(), "the old one does not");
    let old_session = world
        .app
        .call(MeGet, &[])
        .token(signed.token())
        .send()
        .await;
    assert_eq!(old_session.status, 401, "a session that was open is ended");
}

#[tokio::test]
async fn a_link_is_good_for_half_an_hour_and_not_for_a_minute_more() {
    let world = World::new().await;
    let early = world.person("early").await;
    let late = world.person("late").await;
    let for_early = link_for(&world, &early, false, Actor::System).await;
    let for_late = link_for(&world, &late, false, Actor::System).await;
    assert_eq!(
        for_early.expires_at - world.now(),
        Duration::minutes(30),
        "the answer says when"
    );

    world.advance(Duration::minutes(29));
    assert_eq!(use_link(&world, &for_early, NEW_PASSWORD).await.status, 204);
    world.advance(Duration::minutes(2));
    assert!(
        use_link(&world, &for_late, NEW_PASSWORD)
            .await
            .status
            .is_client_error()
    );
}

#[tokio::test]
async fn the_address_is_on_the_origin_of_the_system_and_only_the_hash_of_its_secret_is_kept() {
    let world = World::new().await;
    let ada = world.person("ada").await;

    let made = link_for(&world, &ada, false, Actor::System).await;

    assert!(
        made.url.starts_with("https://orch.example.com/reset/oem_"),
        "{}",
        made.url
    );
    let secret = secret_of(&made);
    let stored: Vec<(Vec<u8>, String, bool)> =
        sqlx::query_as("SELECT token_hash, purpose, clear_second_step FROM email_tokens")
            .fetch_all(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(
        stored,
        vec![(token_hash(&secret), "reset_password".to_owned(), false)]
    );
    let trail: String =
        sqlx::query_scalar("SELECT string_agg(summary || details::text, ' ') FROM audit_log")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert!(!trail.contains(&secret), "the trail never holds a secret");
}

#[tokio::test]
async fn a_link_made_for_somebody_who_lost_their_second_step_switches_it_off_when_it_is_used() {
    let world = World::new().await;
    let kept = world.person("kept").await;
    let lost = world.person("lost").await;
    for person in [&kept, &lost] {
        let signed = world.sign_in(person).await;
        world.authenticator(&signed).await;
    }
    let plain = link_for(&world, &kept, false, Actor::System).await;
    let clearing = link_for(&world, &lost, true, Actor::System).await;

    use_link(&world, &plain, NEW_PASSWORD).await.ok();
    use_link(&world, &clearing, NEW_PASSWORD).await.ok();

    let mut conn = world.conn().await;
    let kept = identity::user_by_id(&mut conn, kept.id)
        .await
        .unwrap()
        .unwrap();
    let lost = identity::user_by_id(&mut conn, lost.id)
        .await
        .unwrap()
        .unwrap();
    assert!(kept.totp_on(), "an ordinary link leaves the second step");
    assert!(!lost.totp_on(), "a link made for it switches it off");
    assert!(
        world
            .audit()
            .await
            .contains(&"auth.second_step_cleared".to_owned())
    );
}

#[tokio::test]
async fn the_making_of_a_link_is_in_the_trail_under_the_actor_that_made_it() {
    let world = World::new().await;
    let boss = world.person("boss").await;
    let ada = world.person("ada").await;
    link_for(&world, &ada, true, Actor::System).await;
    link_for(&world, &ada, false, Actor::User(boss.id)).await;

    let lines: Vec<(String, String, Option<orch_types::Id>, serde_json::Value)> = sqlx::query_as(
        "SELECT action, actor_kind, actor_id, details FROM audit_log WHERE action = 'auth.password_reset' ORDER BY at, id",
    )
    .fetch_all(&world.db.pool)
    .await
    .unwrap();

    assert_eq!(lines.len(), 2);
    assert_eq!((lines[0].1.as_str(), lines[0].2), ("system", None));
    assert_eq!(lines[0].3["stage"], "link_made");
    assert_eq!(lines[0].3["clear_second_step"], true);
    assert_eq!((lines[1].1.as_str(), lines[1].2), ("user", Some(boss.id)));
    assert_eq!(lines[1].3["clear_second_step"], false);
}
