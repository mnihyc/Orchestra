//! A person's time zone: a new person's follows the browser they came from, a zone they chose stays on every device, and a device's report that arrives after a choice changes nothing.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_api_contract::admin::{AdminUserCreate, AdminUserCreateBody};
use orch_api_contract::auth::{
    AdminModeSetBody, AuthRecoveryConfirm, AuthRecoveryConfirmBody, AuthResult, MeAdminModeSet,
    MeUpdate, MeUpdateBody, SetupCreate, SetupCreateBody,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::users::{Me, TimezoneMode};
use orch_api_core::testing::Reply;
use support::{PASSWORD, World};

/// The first administrator, made from a browser that gave `zone`.
async fn setup(world: &World, zone: Option<&str>) -> Reply<AuthResult> {
    world
        .app
        .call(SetupCreate, &[])
        .body(&SetupCreateBody {
            username: "root".to_owned(),
            display_name: "Root".to_owned(),
            password: PASSWORD.to_owned(),
            email: None,
            timezone: zone.map(str::to_owned),
        })
        .send()
        .await
}

async fn change(world: &World, token: &str, body: MeUpdateBody) -> Reply<Me> {
    world
        .app
        .call(MeUpdate, &[])
        .token(token)
        .body(&body)
        .send()
        .await
}

/// What a browser sends on its own when the zone it is in is not the one the person has.
fn report(zone: &str) -> MeUpdateBody {
    MeUpdateBody {
        timezone: Some(zone.to_owned()),
        timezone_mode: Some(TimezoneMode::Device),
        timezone_report: Some(true),
        ..MeUpdateBody::default()
    }
}

fn zone_of(me: &Me) -> (&str, TimezoneMode) {
    (me.timezone.as_str(), me.timezone_mode)
}

#[tokio::test]
async fn a_new_person_follows_the_zone_of_the_browser_they_came_from() {
    let world = World::new().await;
    let made = setup(&world, Some("Asia/Shanghai")).await.ok();
    assert_eq!(zone_of(&made.user), ("Asia/Shanghai", TimezoneMode::Device));
    // They sign in from a laptop in another zone, which reports it: the zone follows.
    let moved = change(&world, &made.access_token, report("Europe/Berlin"))
        .await
        .ok();
    assert_eq!(zone_of(&moved), ("Europe/Berlin", TimezoneMode::Device));
}

#[tokio::test]
async fn without_a_zone_from_the_browser_a_new_person_has_utc_until_one_is_reported() {
    let world = World::new().await;
    let made = setup(&world, None).await.ok();
    assert_eq!(zone_of(&made.user), ("UTC", TimezoneMode::Device));
    let reported = change(&world, &made.access_token, report("America/Sao_Paulo"))
        .await
        .ok();
    assert_eq!(
        zone_of(&reported),
        ("America/Sao_Paulo", TimezoneMode::Device)
    );
}

#[tokio::test]
async fn a_zone_the_person_chose_is_not_overwritten_by_a_device_report_that_arrives_later() {
    let world = World::new().await;
    let made = setup(&world, Some("Asia/Shanghai")).await.ok();
    let chosen = change(
        &world,
        &made.access_token,
        MeUpdateBody {
            timezone: Some("America/New_York".to_owned()),
            ..MeUpdateBody::default()
        },
    )
    .await
    .ok();
    assert_eq!(zone_of(&chosen), ("America/New_York", TimezoneMode::Fixed));
    // A report the browser sent before it learnt of the choice arrives now, and changes nothing.
    let late = change(&world, &made.access_token, report("Asia/Tokyo"))
        .await
        .ok();
    assert_eq!(zone_of(&late), ("America/New_York", TimezoneMode::Fixed));
    // Following the device again lets the next report through.
    let back = change(
        &world,
        &made.access_token,
        MeUpdateBody {
            timezone_mode: Some(TimezoneMode::Device),
            ..MeUpdateBody::default()
        },
    )
    .await
    .ok();
    assert_eq!(zone_of(&back), ("America/New_York", TimezoneMode::Device));
    let followed = change(&world, &made.access_token, report("Asia/Tokyo"))
        .await
        .ok();
    assert_eq!(zone_of(&followed), ("Asia/Tokyo", TimezoneMode::Device));
}

#[tokio::test]
async fn an_unknown_zone_is_refused_and_a_report_without_a_zone_is_no_report() {
    let world = World::new().await;
    let refused = setup(&world, Some("Mars/Olympus_Mons")).await.problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    // Nothing was made: the first administrator can still be set up.
    let made = setup(&world, Some("UTC")).await.ok();
    let unknown = change(&world, &made.access_token, report("Nowhere/Land"))
        .await
        .problem();
    assert_eq!(unknown.code, ErrorCode::RequestInvalid);
    let empty = change(
        &world,
        &made.access_token,
        MeUpdateBody {
            timezone_report: Some(true),
            ..MeUpdateBody::default()
        },
    )
    .await
    .problem();
    assert_eq!(empty.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn a_person_made_without_a_browser_starts_in_device_mode_and_can_deliberately_fix_utc() {
    let world = World::new().await;
    let person = world.person("earlier").await;
    let (zone, mode): (String, String) =
        sqlx::query_as("SELECT timezone, timezone_mode FROM users WHERE id = $1")
            .bind(person.id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!((zone.as_str(), mode.as_str()), ("UTC", "device"));
    let signed = world.sign_in(&person).await;
    assert_eq!(zone_of(&signed.result.user), ("UTC", TimezoneMode::Device));
    // UTC chosen explicitly is fixed even though its text equals the initial fallback.
    let chosen = change(
        &world,
        signed.token(),
        MeUpdateBody {
            timezone: Some("UTC".to_owned()),
            ..MeUpdateBody::default()
        },
    )
    .await
    .ok();
    assert_eq!(zone_of(&chosen), ("UTC", TimezoneMode::Fixed));
    let late = change(&world, signed.token(), report("Europe/Berlin"))
        .await
        .ok();
    assert_eq!(zone_of(&late), ("UTC", TimezoneMode::Fixed));
}

#[tokio::test]
async fn an_admins_creation_and_password_invitation_follow_the_new_persons_device() {
    let world = World::new().await;
    let root = setup(&world, None).await.ok();
    world
        .app
        .call(MeAdminModeSet, &[])
        .token(&root.access_token)
        .body(&AdminModeSetBody { as_admin: true })
        .send()
        .await
        .ok();
    for (username, email) in [
        ("made_elsewhere", None),
        ("invited_elsewhere", Some("invited@example.org")),
    ] {
        let made = world
            .app
            .call(AdminUserCreate, &[])
            .token(&root.access_token)
            .body(&AdminUserCreateBody {
                username: username.to_owned(),
                display_name: username.to_owned(),
                email: email.map(str::to_owned),
                is_system_admin: None,
                personal_org: None,
            })
            .send()
            .await
            .ok();
        let stored: (String, String) =
            sqlx::query_as("SELECT timezone, timezone_mode FROM users WHERE id=$1")
                .bind(made.user.base.id)
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
        assert_eq!(stored, ("UTC".to_owned(), "device".to_owned()));
        world
            .app
            .call(AuthRecoveryConfirm, &[])
            .body(&AuthRecoveryConfirmBody {
                token: made.reset_url.rsplit('/').next().unwrap().to_owned(),
                password: PASSWORD.to_owned(),
            })
            .send()
            .await
            .ok();
        let signed = world
            .password_login(username, PASSWORD, "203.0.113.1", "")
            .await
            .ok();
        assert_eq!(
            zone_of(&signed.user),
            ("UTC", TimezoneMode::Device),
            "setting the invited person's first password does not select UTC for them"
        );
        let reported = change(&world, &signed.access_token, report("Asia/Singapore"))
            .await
            .ok();
        assert_eq!(zone_of(&reported), ("Asia/Singapore", TimezoneMode::Device));
    }
}
