//! Administrator messages share native scope authority and full/receipt projection.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
mod admin_support;
mod places;
mod rig;
mod support;

#[path = "admin_messages/banner.rs"]
mod banner;
#[path = "admin_messages/reading.rs"]
mod reading;
#[path = "admin_messages/replays.rs"]
mod replays;

use chrono::Duration;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::admin_messages::{
    AdminMessageCreate, AdminMessageCreateBody, AdminMessageCreateResponse, OrgMessageCreate,
};
use orch_api_contract::common::ErrorCode;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::identity;
use orch_types::{Id, Json, new_id};
use serde_json::json;
use std::collections::BTreeSet;
use support::World;

async fn key(world: &World, user: Id, rules: Vec<Json>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Administrator message control",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + Duration::hours(1)),
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    secret
}
fn org_rule(operation: &str, org: Id, actions: &[&str]) -> Json {
    json!({"target":{"kind":"org","id":org,"reach":"self"},"resources":["org"],"allow":[{"operation":operation,"actions":actions}]})
}
fn system_rule(actions: &[&str]) -> Json {
    json!({"target":{"kind":"system","reach":"self"},"resources":["system"],"allow":[{"operation":"AdminMessageCreate","actions":actions}]})
}
fn body() -> AdminMessageCreateBody {
    AdminMessageCreateBody {
        title: "A change for members".into(),
        body: "## What changed\n\nPlease read the new rules.".into(),
        banner_until: None,
    }
}
async fn org_send(world: &World, secret: &str, org: Id, body: &AdminMessageCreateBody) -> RawReply {
    let request = world
        .app
        .call(OrgMessageCreate, &[&org])
        .token(secret)
        .body(body)
        .send_bytes();
    request.await
}
async fn count(world: &World) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM admin_messages")
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}

async fn full_org_send(
    world: &World,
    who: &places::Who,
    org: Id,
) -> orch_api_contract::admin_messages::AdminMessage {
    let answer = org_send(world, &who.token, org, &body()).await;
    assert_eq!(
        answer.status.as_u16(),
        201,
        "{}",
        String::from_utf8_lossy(&answer.raw)
    );
    match serde_json::from_slice::<AdminMessageCreateResponse>(&answer.raw).unwrap() {
        AdminMessageCreateResponse::Full(row) => *row,
        AdminMessageCreateResponse::Receipt(_) => {
            panic!("the ordinary sender receives the accepted message")
        }
    }
}
async fn recipients(world: &World, message: Id) -> BTreeSet<Id> {
    sqlx::query_scalar::<_, Id>("SELECT user_id FROM admin_message_recipients WHERE message_id=$1")
        .bind(message)
        .fetch_all(&world.db.pool)
        .await
        .unwrap()
        .into_iter()
        .collect()
}

#[tokio::test]
async fn an_org_audience_follows_membership_domains_and_keeps_the_accepted_people_not_guests() {
    let world = World::new().await;
    let cast = admin_support::cast(&world).await;
    let department = places::make_department(&world, &cast.boss, cast.acme, "Engineering", "eng")
        .await
        .base
        .id;
    let nested = places::make_department(&world, &cast.boss, department, "Tools", "tools")
        .await
        .base
        .id;
    let sibling = places::make_department(&world, &cast.boss, cast.acme, "Operations", "ops")
        .await
        .base
        .id;
    let root_member = places::person(&world, "root-member").await;
    let member = places::person(&world, "dept-member").await;
    let child_member = places::person(&world, "child-member").await;
    let sibling_member = places::person(&world, "sibling-member").await;
    let guest = places::person(&world, "guest-reader").await;
    for (person, domain) in [
        (&root_member, cast.acme),
        (&member, department),
        (&child_member, nested),
        (&sibling_member, sibling),
    ] {
        places::join(&world, cast.acme, domain, person, "viewer", &cast.boss).await;
    }
    places::give_role(&world, department, &guest, "viewer", &cast.boss).await;
    let orch_api_contract::nodes::Node::Org(guest_view) =
        places::read(&world, &guest, department).await
    else {
        panic!("the guest reads an actual organization")
    };
    assert_eq!(
        guest_view.base.id, department,
        "a real readable guest is still no member"
    );
    let request = world
        .app
        .call(
            orch_api_contract::directory::OrgServiceAccountCreate,
            &[&department],
        )
        .token(&cast.boss.token)
        .body(&orch_api_contract::directory::OrgServiceAccountCreateBody {
            username: "message-bot".into(),
            display_name: "Message bot".into(),
        })
        .send();
    let service = request
        .await
        .ok()
        .into_full()
        .expect("the ordinary service-account fixture");
    identity::testing::suspend(&mut *world.conn().await, child_member.id())
        .await
        .unwrap();
    let expected = BTreeSet::from([cast.boss.id(), member.id(), child_member.id()]);
    let preview = orch_db::queries::admin_messages::audience_count(
        &mut *world.conn().await,
        Some(department),
        cast.boss.id(),
    )
    .await
    .unwrap();
    assert_eq!(preview, i64::try_from(expected.len()).unwrap());
    let message = full_org_send(&world, &cast.boss, department).await;
    assert_eq!(recipients(&world, message.id).await, expected);
    assert_eq!(message.recipient_count, preview);
    assert!(!recipients(&world, message.id).await.contains(&guest.id()));
    assert!(
        !recipients(&world, message.id)
            .await
            .contains(&service.user.id),
        "an org-owned service identity alone is not a membership"
    );
    sqlx::query("UPDATE org_members SET domain_id=$3 WHERE org_id=$1 AND user_id=$2")
        .bind(cast.acme)
        .bind(member.id())
        .bind(sibling)
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        recipients(&world, message.id).await,
        expected,
        "later moves cannot rewrite the accepted audience"
    );
    let next = full_org_send(&world, &cast.boss, department).await;
    assert_eq!(
        recipients(&world, next.id).await,
        BTreeSet::from([cast.boss.id(), child_member.id()])
    );
    let root = full_org_send(&world, &cast.boss, cast.acme).await;
    assert_eq!(
        recipients(&world, root.id).await,
        BTreeSet::from([
            cast.boss.id(),
            root_member.id(),
            member.id(),
            child_member.id(),
            sibling_member.id()
        ])
    );
}

#[tokio::test]
async fn a_system_audience_is_every_active_person_including_people_outside_the_org() {
    let world = World::new().await;
    let cast = admin_support::cast(&world).await;
    let suspended = places::person(&world, "suspended-reader").await;
    identity::testing::suspend(&mut *world.conn().await, suspended.id())
        .await
        .unwrap();
    let request = world
        .app
        .call(
            orch_api_contract::directory::OrgServiceAccountCreate,
            &[&cast.acme],
        )
        .token(&cast.boss.token)
        .body(&orch_api_contract::directory::OrgServiceAccountCreateBody {
            username: "system-message-bot".into(),
            display_name: "Message bot".into(),
        })
        .send();
    let service = request
        .await
        .ok()
        .into_full()
        .expect("the ordinary service-account fixture");
    let request = world
        .app
        .call(AdminMessageCreate, &[])
        .token(&cast.root.token)
        .body(&body())
        .send();
    let AdminMessageCreateResponse::Full(message) = request.await.ok() else {
        panic!("the ordinary system sender reads the message")
    };
    let active: BTreeSet<Id> =
        sqlx::query_scalar::<_, Id>("SELECT id FROM users WHERE kind='human' AND status='active'")
            .fetch_all(&world.db.pool)
            .await
            .unwrap()
            .into_iter()
            .collect();
    assert!(
        active.contains(&cast.ada.id()),
        "system recipients need no team-org membership"
    );
    assert_eq!(recipients(&world, message.id).await, active);
    assert!(
        !recipients(&world, message.id)
            .await
            .contains(&service.user.id)
    );
    assert!(
        !recipients(&world, message.id)
            .await
            .contains(&suspended.id())
    );
    let mut after = None;
    let mut paged = Vec::new();
    loop {
        let page = orch_db::queries::admin_messages::recipient_page(
            &mut *world.conn().await,
            message.id,
            after,
            2,
        )
        .await
        .unwrap();
        if page.is_empty() {
            break;
        }
        after = page.last().copied();
        paged.extend(page);
    }
    assert_eq!(paged.iter().copied().collect::<BTreeSet<_>>(), active);
    assert_eq!(
        paged.len(),
        active.len(),
        "restartable recipient pages neither duplicate nor omit"
    );
}

async fn immediate_mail_route(world: &World, who: &places::Who) -> Id {
    use orch_db::queries::notifications::{channels, routes};
    let mut conn = world.conn().await;
    sqlx::query("UPDATE users SET email=$2,email_verified_at=$3 WHERE id=$1")
        .bind(who.id())
        .bind(format!("{}@example.test", who.person.username))
        .bind(world.now())
        .execute(&mut *conn)
        .await
        .unwrap();
    let channel = channels::insert_channel(
        &mut conn,
        &channels::NewChannel {
            id: new_id(),
            org_id: None,
            user_id: Some(who.id()),
            kind: "email",
            name: "Message route",
            config: &json!({}),
            templates: &json!({}),
            enabled: true,
            created_by: who.id(),
            at: world.now(),
        },
    )
    .await
    .unwrap();
    routes::put_route(
        &mut conn,
        new_id(),
        who.id(),
        channel.id,
        &routes::RouteChange {
            enabled: Some(true),
            kinds: None,
            unread_delay_minutes: Some(None),
            min_interval_minutes: Some(None),
        },
        world.now(),
    )
    .await
    .unwrap()
    .id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn administrator_messages_use_the_existing_batched_notice_job_and_mandatory_inbox_with_optional_channels()
 {
    use orch_db::queries::notifications::{people, routes};
    let world = World::new().await;
    let cast = admin_support::cast(&world).await;
    let off = places::person(&world, "mail-off-reader").await;
    let on = places::person(&world, "mail-on-reader").await;
    for who in [&off, &on] {
        places::join(&world, cast.acme, cast.acme, who, "viewer", &cast.boss).await;
    }
    let off_route = immediate_mail_route(&world, &off).await;
    let on_route = immediate_mail_route(&world, &on).await;
    sqlx::query("INSERT INTO notification_prefs(user_id,disabled_kinds) VALUES($1,$2) ON CONFLICT(user_id) DO UPDATE SET disabled_kinds=EXCLUDED.disabled_kinds")
        .bind(off.id()).bind(json!(["admin_message"])).execute(&world.db.pool).await.unwrap();
    let mut conn = world.conn().await;
    for index in 0..orch_engine::jobs::document_notices::BATCH + 3 {
        let user = new_id();
        identity::testing::insert_user(&mut conn, user, &format!("message-member{index:04}"))
            .await
            .unwrap();
        orch_db::queries::access::testing::insert_member(&mut conn, cast.acme, user, cast.acme)
            .await
            .unwrap();
    }
    drop(conn);
    let message = full_org_send(&world, &cast.boss, cast.acme).await;
    assert!(
        message.recipient_count
            > i64::try_from(orch_engine::jobs::document_notices::BATCH).unwrap()
    );
    let job = message
        .delivery_job_id
        .expect("the accepted message owns its durable notice job");
    let mut tx = world.app.app.db.begin().await.unwrap();
    assert_eq!(
        orch_engine::jobs::document_notices::enqueue_message(&mut tx, message.id)
            .await
            .unwrap(),
        job
    );
    tx.commit().await.unwrap();
    world.app.app.engine.start();
    rig::eventually("the administrator message fan-out finishes", || async {
        let state: String = sqlx::query_scalar("SELECT state FROM jobs WHERE id=$1")
            .bind(job)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        state == "succeeded"
    })
    .await;
    let (processed, percent): (i64, i64) = sqlx::query_as(
        "SELECT (plan->>'processed')::bigint,(progress->>'percent')::bigint FROM jobs WHERE id=$1",
    )
    .bind(job)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(processed, message.recipient_count);
    assert_eq!(percent, 100);
    let delivered: BTreeSet<Id> = sqlx::query_scalar(
        "SELECT user_id FROM notifications WHERE kind='admin_message' AND data->>'message_id'=$1",
    )
    .bind(message.id.to_string())
    .fetch_all(&world.db.pool)
    .await
    .unwrap()
    .into_iter()
    .collect();
    assert_eq!(delivered, recipients(&world, message.id).await);
    let materialized:i64=sqlx::query_scalar("SELECT count(*) FROM admin_message_recipients WHERE message_id=$1 AND delivered_at IS NOT NULL")
        .bind(message.id).fetch_one(&world.db.pool).await.unwrap();
    assert_eq!(materialized, message.recipient_count);
    let duplicate:i64=sqlx::query_scalar("SELECT count(*) FROM (SELECT user_id FROM notifications WHERE kind='admin_message' AND data->>'message_id'=$1 GROUP BY user_id HAVING count(*)<>1) AS duplicates")
        .bind(message.id.to_string()).fetch_one(&world.db.pool).await.unwrap();
    assert_eq!(duplicate, 0);
    for (route, wanted) in [(off_route, 0_i64), (on_route, 1_i64)] {
        let queued: i64 = sqlx::query_scalar("SELECT count(*) FROM outbox WHERE route_id=$1")
            .bind(route)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        assert_eq!(
            queued, wanted,
            "the external-kind choice never suppresses the Inbox item"
        );
    }
    let request = world
        .app
        .call(orch_api_contract::notifications::NotificationList, &[])
        .token(&off.token)
        .query(&orch_api_contract::notifications::NotificationListQuery::default())
        .send();
    let inbox = request.await.ok();
    let notice = inbox
        .items
        .iter()
        .find(|notice| {
            notice.kind == orch_api_contract::notifications::NotificationKind::AdminMessage
        })
        .expect("the mandatory Inbox item is visible");
    assert_eq!(notice.url, format!("/messages/{}", message.id));
    assert_eq!(notice.body, body().body);
    assert_eq!(
        serde_json::to_value(orch_notifier_contract::delivery::NotificationKind::AdminMessage)
            .unwrap(),
        json!("admin_message")
    );
    // A queued outward snapshot is filtered by the person's CURRENT choice too.
    sqlx::query("INSERT INTO notification_prefs(user_id,disabled_kinds) VALUES($1,$2) ON CONFLICT(user_id) DO UPDATE SET disabled_kinds=EXCLUDED.disabled_kinds")
        .bind(on.id()).bind(json!(["admin_message"])).execute(&world.db.pool).await.unwrap();
    assert!(
        people::outward_kinds(&mut *world.conn().await, on.id(), &["admin_message".into()])
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        routes::immediate_routes(
            &mut *world.conn().await,
            on.id(),
            "admin_message",
            Some(cast.acme),
            true
        )
        .await
        .unwrap()
        .is_empty()
    );
    assert!(delivered.contains(&off.id()) && delivered.contains(&on.id()));
    world.app.app.engine.stop();
}

#[tokio::test]
async fn organization_send_binds_its_native_scope_and_keeps_current_read_optional() {
    let world = World::new().await;
    let cast = admin_support::cast(&world).await;
    let other = places::make_org(&world, &cast.boss, "Other", "other")
        .await
        .base
        .id;
    for rules in [
        vec![org_rule("OrgMessageCreate", cast.acme, &["read"])],
        vec![org_rule("OrgMessageCreate", other, &["manage"])],
        vec![org_rule("NodeGet", cast.acme, &["read"])],
    ] {
        let secret = key(&world, cast.boss.id(), rules).await;
        let denied = org_send(&world, &secret, cast.acme, &body()).await;
        assert_eq!(denied.problem().code, ErrorCode::AuthzDenied);
        assert_eq!(count(&world).await, 0);
    }
    let outsider = key(
        &world,
        cast.ada.id(),
        vec![org_rule("OrgMessageCreate", cast.acme, &["manage", "read"])],
    )
    .await;
    assert_eq!(
        org_send(&world, &outsider, cast.acme, &body())
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    assert_eq!(
        count(&world).await,
        0,
        "a scope rule never grants native members.manage"
    );
    let write_only = key(
        &world,
        cast.boss.id(),
        vec![org_rule("OrgMessageCreate", cast.acme, &["manage"])],
    )
    .await;
    let answer = org_send(&world, &write_only, cast.acme, &body()).await;
    assert_eq!(
        answer.status.as_u16(),
        201,
        "{}",
        String::from_utf8_lossy(&answer.raw)
    );
    let receipt: Json = serde_json::from_slice(&answer.raw).unwrap();
    assert_eq!(receipt["projection"], "receipt");
    assert!(receipt["message_id"].is_string());
    assert!(receipt.get("body").is_none() && receipt.get("sender").is_none());
    assert_eq!(count(&world).await, 1);
    let ordinary = world
        .app
        .call(OrgMessageCreate, &[&cast.acme])
        .token(&cast.boss.token)
        .body(&body())
        .send();
    let ordinary = ordinary.await.ok();
    let AdminMessageCreateResponse::Full(message) = ordinary else {
        panic!("ordinary native creation has its full response")
    };
    assert_eq!(message.title, body().title);
    assert_eq!(message.body, body().body);
    assert_eq!(message.sender.id, cast.boss.id());
    assert_eq!(message.org.as_ref().unwrap().id, cast.acme);
    assert!(message.recipient_count > 0);
    let rows = admin_support::lines_of(&world, "message.sent").await;
    assert_eq!(
        rows.len(),
        2,
        "one audit row per accepted send, no refused send"
    );
    assert!(
        rows.iter()
            .all(|row| row.actor_id == Some(cast.boss.id()) && row.node_id == Some(cast.acme))
    );
    let readable = key(
        &world,
        cast.boss.id(),
        vec![org_rule("OrgMessageCreate", cast.acme, &["manage", "read"])],
    )
    .await;
    let full = org_send(&world, &readable, cast.acme, &body()).await;
    assert_eq!(
        full.status.as_u16(),
        201,
        "{}",
        String::from_utf8_lossy(&full.raw)
    );
    let full: Json = serde_json::from_slice(&full.raw).unwrap();
    assert_eq!(full["body"], body().body);
    assert_eq!(full["sender"]["id"], cast.boss.id().to_string());
    assert_eq!(count(&world).await, 3);
}

#[tokio::test]
async fn system_send_keeps_system_standing_and_the_native_browser_admin_switch() {
    let world = World::new().await;
    let cast = admin_support::cast(&world).await;
    let outsider = key(
        &world,
        cast.boss.id(),
        vec![system_rule(&["manage", "read"])],
    )
    .await;
    let denied = world
        .app
        .call(AdminMessageCreate, &[])
        .token(&outsider)
        .body(&body())
        .send_bytes();
    assert_eq!(denied.await.problem().code, ErrorCode::AuthzDenied);
    places::admin_mode(&world, &cast.root, false).await;
    let off = world
        .app
        .call(AdminMessageCreate, &[])
        .token(&cast.root.token)
        .body(&body())
        .send_bytes();
    assert_eq!(off.await.problem().code, ErrorCode::AuthzDenied);
    assert_eq!(count(&world).await, 0);
    places::admin_mode(&world, &cast.root, true).await;
    let allowed = world
        .app
        .call(AdminMessageCreate, &[])
        .token(&cast.root.token)
        .body(&body())
        .send();
    let AdminMessageCreateResponse::Full(message) = allowed.await.ok() else {
        panic!("an ordinary system administrator receives the native message")
    };
    assert!(message.org.is_none());
    assert_eq!(message.body, body().body);
    assert_eq!(count(&world).await, 1);
    let read_only = key(&world, cast.root.id(), vec![system_rule(&["read"])]).await;
    let denied = world
        .app
        .call(AdminMessageCreate, &[])
        .token(&read_only)
        .body(&body())
        .send_bytes();
    assert_eq!(denied.await.problem().code, ErrorCode::AuthzDenied);
    assert_eq!(count(&world).await, 1);
    let write_only = key(&world, cast.root.id(), vec![system_rule(&["manage"])]).await;
    let receipt = world
        .app
        .call(AdminMessageCreate, &[])
        .token(&write_only)
        .body(&body())
        .send_bytes();
    let receipt = receipt.await;
    assert_eq!(
        receipt.status.as_u16(),
        201,
        "{}",
        String::from_utf8_lossy(&receipt.raw)
    );
    let receipt: Json = serde_json::from_slice(&receipt.raw).unwrap();
    assert_eq!(receipt["projection"], "receipt");
    assert!(receipt.get("body").is_none());
    assert_eq!(count(&world).await, 2);
}
