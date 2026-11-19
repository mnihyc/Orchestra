//! A person's routes through the channels that reach them over HTTP: off until they switch one on, what it needs before it can be on, the kinds and the waits they chose, the link of a bot, and ending a route.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod outside;
mod places;
mod support;

use chrono::Duration;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::notifications::{
    MeNotificationRouteDelete, MeNotificationRouteLinkCreate, MeNotificationRouteSet,
    MeNotificationRouteSetBody, MePushSubscriptionCreate, MePushSubscriptionCreateBody,
    NotificationChannel, NotificationChannelCreateBody, NotificationChannelDelete,
    NotificationKind, NotificationRoute, NotificationRouteNeeds, PushSubscriptionKeys,
};
use orch_api_core::middleware::authentication::{ACCESS_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::Reply;
use orch_db::queries::identity::testing as user_rows;
use orch_db::queries::notifications::routes as route_rows;
use orch_types::{Id, new_id};
use orch_worker_core::bot_links::hash_code;
use outside::{FarSide, Outside};
use places::Who;
use serde_json::json;
use sqlx::postgres::PgListener;
use std::time::Duration as Wait;

async fn set(
    o: &Outside,
    by: &Who,
    channel: Id,
    body: &MeNotificationRouteSetBody,
) -> Reply<NotificationRoute> {
    o.world
        .app
        .call(MeNotificationRouteSet, &[&channel])
        .token(&by.token)
        .body(body)
        .send()
        .await
}

fn on() -> MeNotificationRouteSetBody {
    MeNotificationRouteSetBody {
        enabled: Some(true),
        ..MeNotificationRouteSetBody::default()
    }
}

async fn route_of(o: &Outside, by: &Who, channel: Id) -> NotificationRoute {
    o.channels(by)
        .await
        .into_iter()
        .find(|each| each.id == channel)
        .unwrap()
        .route
}

async fn system_webhook(o: &Outside) -> (NotificationChannel, FarSide) {
    let far = FarSide::taking().await;
    let made = o
        .world
        .app
        .call(
            orch_api_contract::notifications::AdminNotificationChannelCreate,
            &[],
        )
        .token(&o.ada.token)
        .body(&Outside::webhook("Hook", &far.url, None))
        .send()
        .await
        .ok();
    (made, far)
}

async fn system_channel(o: &Outside, body: &NotificationChannelCreateBody) -> NotificationChannel {
    o.world
        .app
        .call(
            orch_api_contract::notifications::AdminNotificationChannelCreate,
            &[],
        )
        .token(&o.ada.token)
        .body(body)
        .send()
        .await
        .ok()
}

/// A person whose address is not confirmed, with a session of their own, who is a viewer in Acme.
async fn unconfirmed(o: &Outside, name: &str) -> Who {
    let person = o.world.person_with(name, support::PASSWORD, false).await;
    let token = new_token(ACCESS_TOKEN_PREFIX);
    let mut conn = o.world.conn().await;
    user_rows::insert_session(
        &mut conn,
        new_id(),
        person.id,
        &token_hash(&token),
        o.world.now() + Duration::days(400),
    )
    .await
    .unwrap();
    drop(conn);
    let who = Who { person, token };
    places::join(&o.world, o.acme, o.acme, &who, "viewer", &o.bea).await;
    who
}

#[tokio::test]
async fn a_route_is_off_until_its_person_switches_it_on_and_nobody_switches_one_on_for_another() {
    let o = Outside::new().await;
    let (hook, _far) = system_webhook(&o).await;
    for who in [&o.ada, &o.cai, &o.dee] {
        let route = route_of(&o, who, hook.id).await;
        assert!(!route.enabled, "every route starts off");
        assert_eq!(route.needs, None, "a webhook needs nothing of its person");
    }
    assert_eq!(
        o.count(
            "SELECT count(*) FROM notification_routes WHERE channel_id = $1",
            hook.id
        )
        .await,
        0,
        "none is made until its person changes it"
    );

    let switched = set(&o, &o.cai, hook.id, &on()).await.ok();
    assert!(switched.enabled);
    assert!(route_of(&o, &o.cai, hook.id).await.enabled);
    // Others' stay as they were, whatever the channel says.
    assert!(!route_of(&o, &o.dee, hook.id).await.enabled);
    assert!(!route_of(&o, &o.ada, hook.id).await.enabled);
    // An admin's call changes the admin's own.
    set(&o, &o.ada, hook.id, &on()).await.ok();
    assert!(!route_of(&o, &o.dee, hook.id).await.enabled);
    let rows: Vec<(Id, bool)> = sqlx::query_as(
        "SELECT user_id, enabled FROM notification_routes WHERE channel_id = $1 ORDER BY enabled, user_id",
    )
    .bind(hook.id)
    .fetch_all(&o.world.db.pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|(_, enabled)| *enabled));
}

#[tokio::test]
async fn a_route_that_cannot_reach_its_person_yet_says_what_it_needs_and_cannot_be_on() {
    let o = Outside::new().await;
    let mail = system_channel(&o, &Outside::mail_server("smtp.example")).await;
    let push = system_channel(
        &o,
        &NotificationChannelCreateBody {
            kind: "push".to_owned(),
            name: "Push".to_owned(),
            config: json!({}),
            secrets: None,
            templates: None,
            enabled: None,
        },
    )
    .await;
    let bot = system_channel(&o, &Outside::bot("good-token")).await;
    let gus = unconfirmed(&o, "gus").await;

    // Mail goes to a confirmed address.
    assert_eq!(
        route_of(&o, &gus, mail.id).await.needs,
        Some(NotificationRouteNeeds::VerifiedEmail)
    );
    let refused = set(&o, &gus, mail.id, &on()).await.problem();
    assert_eq!(refused.code, ErrorCode::RouteNotReady);
    assert_eq!(refused.extra["needs"], "verified_email");
    assert!(!route_of(&o, &gus, mail.id).await.enabled);
    sqlx::query("UPDATE users SET email_verified_at = now() WHERE id = $1")
        .bind(gus.id())
        .execute(&o.world.db.pool)
        .await
        .unwrap();
    assert!(set(&o, &gus, mail.id, &on()).await.ok().enabled);
    assert_eq!(route_of(&o, &gus, mail.id).await.needs, None);

    // Push goes to a browser that took it.
    assert_eq!(
        set(&o, &o.cai, push.id, &on()).await.problem().extra["needs"],
        "device"
    );
    let mut point = [7_u8; 65];
    point[0] = 4;
    use base64::Engine as _;
    let browser = MePushSubscriptionCreateBody {
        endpoint: "https://push.example/send/xyz".to_owned(),
        keys: PushSubscriptionKeys {
            p256dh: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(point),
            auth: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([9_u8; 16]),
        },
    };
    let added = o
        .world
        .app
        .call(MePushSubscriptionCreate, &[])
        .token(&o.cai.token)
        .body(&browser)
        .send()
        .await;
    assert_eq!(added.status, 204);
    assert!(set(&o, &o.cai, push.id, &on()).await.ok().enabled);

    // A bot writes to somebody who started a chat with it.
    assert_eq!(
        set(&o, &o.cai, bot.id, &on()).await.problem().extra["needs"],
        "link"
    );
}

#[tokio::test]
async fn the_link_of_a_bot_is_good_for_ten_minutes_and_only_the_hash_of_its_code_is_kept() {
    let o = Outside::new().await;
    let bot = o
        .org_channel(&o.bea, o.acme, &Outside::bot("good-token"))
        .await
        .unwrap();
    let link = |who: &Who| {
        let o = &o;
        let token = who.token.clone();
        let id = bot.id;
        async move {
            o.world
                .app
                .call(MeNotificationRouteLinkCreate, &[&id])
                .token(&token)
                .send()
                .await
        }
    };
    let first = link(&o.cai).await.ok();
    let code = first
        .url
        .strip_prefix("https://t.me/acme_orch_bot?start=")
        .expect("the address opens the bot with a code")
        .to_owned();
    assert_eq!(
        first.expires_at.get(),
        o.world.now() + Duration::minutes(10)
    );
    let (hash, expires): (Vec<u8>, chrono::DateTime<chrono::Utc>) = sqlx::query_as(
        "SELECT link_hash, link_expires_at FROM notification_routes WHERE user_id = $1",
    )
    .bind(o.cai.id())
    .fetch_one(&o.world.db.pool)
    .await
    .unwrap();
    assert_eq!(hash, hash_code(&code).to_vec());
    assert!(
        !hash
            .windows(code.len())
            .any(|window| window == code.as_bytes())
    );
    assert_eq!(expires, first.expires_at.get());

    // Asking again makes another code and the first no longer counts.
    let second = link(&o.cai).await.ok();
    assert_ne!(second.url, first.url);
    let mut conn = o.world.conn().await;
    let route: Id = sqlx::query_scalar("SELECT id FROM notification_routes WHERE user_id = $1")
        .bind(o.cai.id())
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let address = json!({ "chat_id": 42, "label": "Cai Lee" });
    assert!(
        !route_rows::complete_link(&mut conn, route, &hash_code(&code), &address, o.world.now())
            .await
            .unwrap(),
        "the first code was replaced"
    );
    // The second code, pressed in time, links the route, which then says whose it is.
    let code2 = second.url.rsplit('=').next().unwrap().to_owned();
    assert!(
        route_rows::complete_link(
            &mut conn,
            route,
            &hash_code(&code2),
            &address,
            o.world.now()
        )
        .await
        .unwrap()
    );
    drop(conn);
    let linked = route_of(&o, &o.cai, bot.id).await;
    assert_eq!(linked.needs, None);
    assert_eq!(linked.label.as_deref(), Some("Cai Lee"));
    assert!(set(&o, &o.cai, bot.id, &on()).await.ok().enabled);
}

#[tokio::test]
async fn a_code_that_comes_back_after_ten_minutes_matches_nothing() {
    let o = Outside::new().await;
    let bot = o
        .org_channel(&o.bea, o.acme, &Outside::bot("good-token"))
        .await
        .unwrap();
    let made = o
        .world
        .app
        .call(MeNotificationRouteLinkCreate, &[&bot.id])
        .token(&o.cai.token)
        .send()
        .await
        .ok();
    let code = made.url.rsplit('=').next().unwrap().to_owned();
    o.world.advance(Duration::minutes(11));
    let mut conn = o.world.conn().await;
    let route: Id = sqlx::query_scalar("SELECT id FROM notification_routes WHERE user_id = $1")
        .bind(o.cai.id())
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert!(
        !route_rows::complete_link(
            &mut conn,
            route,
            &hash_code(&code),
            &json!({ "chat_id": 1 }),
            o.world.now()
        )
        .await
        .unwrap()
    );
}

#[tokio::test]
async fn a_link_is_made_only_for_a_channel_that_links_people_that_is_in_use_and_that_reaches_the_person()
 {
    let o = Outside::new().await;
    let (hook, _far) = system_webhook(&o).await;
    let bot = o
        .org_channel(&o.bea, o.acme, &Outside::bot("good-token"))
        .await
        .unwrap();
    let link = |who: &Who, channel: Id| {
        let o = &o;
        let token = who.token.clone();
        async move {
            o.world
                .app
                .call(MeNotificationRouteLinkCreate, &[&channel])
                .token(&token)
                .send()
                .await
        }
    };
    let not_a_bot = link(&o.cai, hook.id).await.problem();
    assert_eq!(not_a_bot.code, ErrorCode::RequestInvalid);
    // A stranger to the org has no bot of the org's to be linked to.
    assert_eq!(
        link(&o.fay, bot.id).await.problem().code,
        ErrorCode::NotFound
    );
    assert_eq!(
        link(&o.cai, new_id()).await.problem().code,
        ErrorCode::NotFound
    );
    // A bot that is switched off is not asked who followed its links.
    o.world
        .app
        .call(
            orch_api_contract::notifications::NotificationChannelUpdate,
            &[&bot.id],
        )
        .token(&o.bea.token)
        .body(
            &orch_api_contract::notifications::NotificationChannelUpdateBody {
                enabled: Some(false),
                ..Default::default()
            },
        )
        .send()
        .await
        .ok();
    assert_eq!(
        link(&o.cai, bot.id).await.problem().code,
        ErrorCode::RequestInvalid
    );
}

#[tokio::test]
async fn a_route_keeps_the_kinds_the_wait_and_the_limit_its_person_chose() {
    let o = Outside::new().await;
    let (hook, _far) = system_webhook(&o).await;
    let chosen = set(
        &o,
        &o.cai,
        hook.id,
        &MeNotificationRouteSetBody {
            enabled: Some(true),
            kinds: Some(vec![
                NotificationKind::TurnFailed,
                NotificationKind::ApprovalRequested,
            ]),
            unread_delay_minutes: Some(Some(15)),
            min_interval_minutes: Some(Some(30)),
            retry_failed: None,
        },
    )
    .await
    .ok();
    assert_eq!(
        chosen.kinds,
        [
            NotificationKind::TurnFailed,
            NotificationKind::ApprovalRequested
        ]
    );
    assert_eq!(
        (chosen.unread_delay_minutes, chosen.min_interval_minutes),
        (Some(15), Some(30))
    );
    // What is left out stays; null takes a wait away.
    let later = set(
        &o,
        &o.cai,
        hook.id,
        &MeNotificationRouteSetBody {
            unread_delay_minutes: Some(None),
            ..MeNotificationRouteSetBody::default()
        },
    )
    .await
    .ok();
    assert!(later.enabled);
    assert_eq!(later.kinds, chosen.kinds);
    assert_eq!(
        (later.unread_delay_minutes, later.min_interval_minutes),
        (None, Some(30))
    );
    let everything = set(
        &o,
        &o.cai,
        hook.id,
        &MeNotificationRouteSetBody {
            kinds: Some(vec![]),
            ..MeNotificationRouteSetBody::default()
        },
    )
    .await
    .ok();
    assert!(everything.kinds.is_empty());
    // A wait is from one minute to a day.
    for (delay, path) in [(0, "unread_delay_minutes"), (1441, "unread_delay_minutes")] {
        let refused = set(
            &o,
            &o.cai,
            hook.id,
            &MeNotificationRouteSetBody {
                unread_delay_minutes: Some(Some(delay)),
                ..MeNotificationRouteSetBody::default()
            },
        )
        .await
        .problem();
        assert_eq!(refused.code, ErrorCode::RequestInvalid);
        assert!(serde_json::to_string(&refused).unwrap().contains(path));
    }
    let limit = set(
        &o,
        &o.cai,
        hook.id,
        &MeNotificationRouteSetBody {
            min_interval_minutes: Some(Some(5000)),
            ..MeNotificationRouteSetBody::default()
        },
    )
    .await
    .problem();
    assert_eq!(limit.code, ErrorCode::RequestInvalid);
    let unknown = o
        .world
        .app
        .call(MeNotificationRouteSet, &[&hook.id])
        .token(&o.cai.token)
        .raw_body(&json!({ "kinds": ["turn.exploded"] }))
        .send()
        .await
        .problem();
    assert_eq!(unknown.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn ending_a_route_switches_it_off_and_forgets_where_it_was_linked() {
    let o = Outside::new().await;
    let bot = o
        .org_channel(&o.bea, o.acme, &Outside::bot("good-token"))
        .await
        .unwrap();
    o.world
        .app
        .call(MeNotificationRouteLinkCreate, &[&bot.id])
        .token(&o.cai.token)
        .send()
        .await
        .ok();
    let mut conn = o.world.conn().await;
    let (route, hash): (Id, Vec<u8>) =
        sqlx::query_as("SELECT id, link_hash FROM notification_routes WHERE user_id = $1")
            .bind(o.cai.id())
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert!(
        route_rows::complete_link(
            &mut conn,
            route,
            &hash,
            &json!({ "chat_id": 42, "label": "Cai Lee" }),
            o.world.now()
        )
        .await
        .unwrap()
    );
    drop(conn);
    assert!(set(&o, &o.cai, bot.id, &on()).await.ok().enabled);

    let ended = o
        .world
        .app
        .call(MeNotificationRouteDelete, &[&bot.id])
        .token(&o.cai.token)
        .send()
        .await;
    assert_eq!(ended.status, 204);
    let after = route_of(&o, &o.cai, bot.id).await;
    assert!(!after.enabled);
    assert_eq!(after.needs, Some(NotificationRouteNeeds::Link));
    assert_eq!(after.label, None);
    // Ending a route that was never made is no error.
    let again = o
        .world
        .app
        .call(MeNotificationRouteDelete, &[&bot.id])
        .token(&o.dee.token)
        .send()
        .await;
    assert_eq!(again.status, 204);
}

#[tokio::test]
async fn a_route_is_made_only_through_a_channel_that_reaches_the_person() {
    let o = Outside::new().await;
    let bot = o
        .org_channel(&o.bea, o.acme, &Outside::bot("good-token"))
        .await
        .unwrap();
    assert_eq!(
        set(&o, &o.fay, bot.id, &on()).await.problem().code,
        ErrorCode::NotFound
    );
    assert_eq!(
        set(&o, &o.cai, new_id(), &on()).await.problem().code,
        ErrorCode::NotFound
    );
    let delete = o
        .world
        .app
        .call(MeNotificationRouteDelete, &[&bot.id])
        .token(&o.fay.token)
        .send()
        .await
        .problem();
    assert_eq!(delete.code, ErrorCode::NotFound);
    assert_eq!(
        o.count(
            "SELECT count(*) FROM notification_routes WHERE channel_id = $1",
            bot.id
        )
        .await,
        0
    );
}

/// The next news for people that is no row of any feed, as the database announces it.
async fn next_news(listener: &mut PgListener) -> serde_json::Value {
    loop {
        let heard = tokio::time::timeout(Wait::from_secs(10), listener.recv())
            .await
            .expect("the news arrives")
            .unwrap();
        if heard.payload().starts_with('{') {
            return serde_json::from_str(heard.payload()).unwrap();
        }
    }
}

#[tokio::test]
async fn the_screens_of_a_person_are_told_when_a_bot_is_started_a_route_is_ended_or_a_channel_is_removed()
 {
    let o = Outside::new().await;
    let bot = o
        .org_channel(&o.bea, o.acme, &Outside::bot("good-token"))
        .await
        .unwrap();
    let mut listener = PgListener::connect(&o.world.db.url).await.unwrap();
    listener.listen("orch_user").await.unwrap();
    o.world
        .app
        .call(MeNotificationRouteLinkCreate, &[&bot.id])
        .token(&o.cai.token)
        .send()
        .await
        .ok();
    let mut conn = o.world.conn().await;
    let (route, hash): (Id, Vec<u8>) =
        sqlx::query_as("SELECT id, link_hash FROM notification_routes WHERE user_id = $1")
            .bind(o.cai.id())
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    // Pressing Start in the bot is the worker's, which links the route.
    assert!(
        route_rows::complete_link(
            &mut conn,
            route,
            &hash,
            &json!({ "chat_id": 42, "label": "Cai Lee" }),
            o.world.now()
        )
        .await
        .unwrap()
    );
    drop(conn);
    let news = next_news(&mut listener).await;
    assert_eq!(
        news["event"],
        json!({ "what": "route", "channel_id": bot.id })
    );
    assert_eq!(news["users"], json!([o.cai.id()]));
    // It is news the feed of the person's screens reads.
    assert_eq!(
        serde_json::from_value::<orch_realtime_contract::feeds::UserEvent>(news["event"].clone())
            .unwrap(),
        orch_realtime_contract::feeds::UserEvent::Route { channel_id: bot.id }
    );

    // Removing the bot ends the routes through it, and their people are told.
    o.world
        .app
        .call(NotificationChannelDelete, &[&bot.id])
        .token(&o.bea.token)
        .send()
        .await;
    let news = next_news(&mut listener).await;
    assert_eq!(
        news["event"],
        json!({ "what": "route", "channel_id": bot.id })
    );
    assert_eq!(news["users"], json!([o.cai.id()]));
}

#[tokio::test]
async fn nobody_who_is_not_signed_in_changes_a_route() {
    let o = Outside::new().await;
    let id = new_id();
    let problems = [
        o.world
            .app
            .call(MeNotificationRouteSet, &[&id])
            .body(&on())
            .send()
            .await
            .problem(),
        o.world
            .app
            .call(MeNotificationRouteLinkCreate, &[&id])
            .send()
            .await
            .problem(),
        o.world
            .app
            .call(MeNotificationRouteDelete, &[&id])
            .send()
            .await
            .problem(),
    ];
    for problem in problems {
        assert_eq!(problem.code, ErrorCode::AuthRequired);
    }
}

#[tokio::test]
async fn a_routes_minutes_outside_the_contracts_bounds_are_refused_at_their_field_and_never_reach_the_database()
 {
    use orch_api_contract::notifications::{ROUTE_MINUTES_MAX, ROUTE_MINUTES_MIN};
    let o = Outside::new().await;
    let (hook, _far) = system_webhook(&o).await;
    for (field, minutes) in [
        ("unread_delay_minutes", ROUTE_MINUTES_MIN - 1),
        ("unread_delay_minutes", ROUTE_MINUTES_MAX + 1),
        ("min_interval_minutes", ROUTE_MINUTES_MIN - 1),
        ("min_interval_minutes", u32::MAX),
    ] {
        let mut body = MeNotificationRouteSetBody::default();
        if field == "unread_delay_minutes" {
            body.unread_delay_minutes = Some(Some(minutes));
        } else {
            body.min_interval_minutes = Some(Some(minutes));
        }
        let refused = set(&o, &o.cai, hook.id, &body).await;
        assert_eq!(refused.status, 400, "{field} = {minutes}");
        let problem = refused.problem();
        assert_eq!(problem.code, ErrorCode::RequestInvalid);
        assert_eq!(problem.extra["errors"][0]["path"], field);
        assert_eq!(
            problem.extra["errors"][0]["message"],
            "is from 1 to 1440 minutes, or null"
        );
    }
    let edges = set(
        &o,
        &o.cai,
        hook.id,
        &MeNotificationRouteSetBody {
            unread_delay_minutes: Some(Some(ROUTE_MINUTES_MIN)),
            min_interval_minutes: Some(Some(ROUTE_MINUTES_MAX)),
            ..MeNotificationRouteSetBody::default()
        },
    )
    .await
    .ok();
    assert_eq!(
        (edges.unread_delay_minutes, edges.min_interval_minutes),
        (Some(ROUTE_MINUTES_MIN), Some(ROUTE_MINUTES_MAX))
    );
}
