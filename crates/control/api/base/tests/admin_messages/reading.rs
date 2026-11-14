//! Received content and both sent lists use their own current source and native scope gate.
use super::*;
#[path = "reading/permission_rules.rs"]
mod permission_rules;
use http::StatusCode;
use orch_api_contract::{
    admin_messages::{AdminMessageList, MessageGet, OrgMessageList},
    common::PageQuery,
};

fn system_read(operation: &str) -> Json {
    json!({"target":{"kind":"system","reach":"self"},"allow":[{"operation":operation,"actions":["read"]}]})
}
async fn get(world: &World, token: &str, message: Id) -> RawReply {
    let call = world.app.call(MessageGet, &[&message]).token(token);
    call.send_bytes().await
}
async fn sent(
    world: &World,
    token: &str,
    org: Option<Id>,
    cursor: Option<String>,
    limit: u32,
) -> RawReply {
    let query = PageQuery {
        cursor,
        limit: Some(limit),
    };
    if let Some(org) = org {
        let call = world
            .app
            .call(OrgMessageList, &[&org])
            .token(token)
            .query(&query);
        call.send_bytes().await
    } else {
        let call = world
            .app
            .call(AdminMessageList, &[])
            .token(token)
            .query(&query);
        call.send_bytes().await
    }
}
fn value(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

fn phase_count(log: &orch_api_core::testing::Boundaries, phase: &str) -> usize {
    let prefix = format!("MessageGet:{phase} ");
    log.all(std::time::Instant::now())
        .iter()
        .filter(|line| line.starts_with(&prefix))
        .count()
}

#[tokio::test]
async fn a_stable_message_read_checks_its_source_only_at_the_transport_cuts_even_when_repeats_are_zero()
 {
    use orch_api_core::testing::{Boundaries, follow};
    for repeats in [0, 3] {
        let world = World::new().await;
        let c = admin_support::cast(&world).await;
        let message = full_org_send(&world, &c.boss, c.acme).await;
        world.set("api.read_repeats", json!(repeats)).await;
        let log = Boundaries::new();
        let answer = follow(log.clone(), get(&world, &c.boss.token, message.id)).await;
        assert_eq!(answer.status, StatusCode::OK);
        assert_eq!(
            phase_count(&log, "selected.refresh"),
            2,
            "one source check for the header and one after the produced data frame"
        );
        assert_eq!(phase_count(&log, "selected.repeat_render"), 0);
    }
}

#[tokio::test]
async fn a_message_that_keeps_changing_is_answered_with_current_content_instead_of_an_access_denial()
 {
    use orch_api_core::testing::{Boundaries, follow};
    let world = World::new().await;
    let c = admin_support::cast(&world).await;
    let message = full_org_send(&world, &c.boss, c.acme).await;
    let log = Boundaries::new();
    let (pool, id) = (world.db.pool.clone(), message.id);
    log.hold(move |operation, phase| {
        let pool = pool.clone();
        Box::pin(async move {
            if operation == "MessageGet" && phase == "selected.refresh" {
                sqlx::query("UPDATE admin_messages SET title=title || ' updated' WHERE id=$1")
                    .bind(id)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
        })
    });
    let answer = follow(log.clone(), get(&world, &c.boss.token, message.id)).await;
    let shown = value(answer);
    assert!(shown["title"].as_str().unwrap().starts_with(&message.title));
    assert!(phase_count(&log, "selected.refresh") >= 2);
}

#[tokio::test]
async fn a_changed_message_with_rebuilding_disabled_says_to_read_again_without_rebuilding_a_discarded_reply()
 {
    use orch_api_core::testing::{Boundaries, follow};
    let world = World::new().await;
    let c = admin_support::cast(&world).await;
    let message = full_org_send(&world, &c.boss, c.acme).await;
    world.set("api.read_repeats", json!(0)).await;
    let log = Boundaries::new();
    let (pool, id) = (world.db.pool.clone(), message.id);
    log.hold(move |operation, phase| {
        let pool = pool.clone();
        Box::pin(async move {
            if operation == "MessageGet" && phase == "selected.settle" {
                sqlx::query("UPDATE admin_messages SET title=title || ' updated' WHERE id=$1")
                    .bind(id)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
        })
    });
    let answer = follow(log.clone(), get(&world, &c.boss.token, message.id)).await;
    assert_eq!(answer.status, StatusCode::CONFLICT);
    assert_eq!(answer.problem().code, ErrorCode::ConflictStaleRevision);
    assert_eq!(phase_count(&log, "selected.repeat_render"), 0);
}

#[tokio::test]
async fn a_message_is_readable_by_its_accepted_recipient_or_a_current_scope_sender_under_its_own_read()
 {
    let world = World::new().await;
    let c = admin_support::cast(&world).await;
    let recipient = places::person(&world, "message-reader").await;
    places::join(&world, c.acme, c.acme, &recipient, "viewer", &c.boss).await;
    let message = full_org_send(&world, &c.boss, c.acme).await;
    let reader = key(
        &world,
        recipient.id(),
        vec![org_rule("MessageGet", c.acme, &["read"])],
    )
    .await;
    let shown = value(get(&world, &reader, message.id).await);
    assert_eq!(shown["id"], json!(message.id));
    assert_eq!(shown["title"], message.title);
    assert_eq!(shown["body"], message.body);
    assert_eq!(shown["sender"]["id"], json!(c.boss.id()));
    sqlx::query("DELETE FROM org_members WHERE user_id=$1 AND org_id=$2")
        .bind(recipient.id())
        .bind(c.acme)
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        value(get(&world, &reader, message.id).await),
        shown,
        "a delivered message belongs to the accepted audience, not current membership"
    );
    assert_eq!(
        get(&world, &c.ada.token, message.id).await.status,
        StatusCode::NOT_FOUND
    );
    places::give_role(&world, c.acme, &c.ada, "admin", &c.boss).await;
    assert!(!recipients(&world, message.id).await.contains(&c.ada.id()));
    let sender_read = key(
        &world,
        c.ada.id(),
        vec![org_rule("MessageGet", c.acme, &["read"])],
    )
    .await;
    assert_eq!(
        value(get(&world, &sender_read, message.id).await)["body"],
        message.body
    );
    let neighbour = key(
        &world,
        recipient.id(),
        vec![org_rule("OrgMessageList", c.acme, &["read"])],
    )
    .await;
    assert_eq!(
        get(&world, &neighbour, message.id).await.problem().code,
        ErrorCode::AuthzDenied
    );
}

#[tokio::test]
async fn org_sent_pages_are_bounded_summaries_with_exact_scope_account_cursors_and_current_sender_authority()
 {
    let world = World::new().await;
    let c = admin_support::cast(&world).await;
    let other = places::make_org(&world, &c.boss, "Other messages", "other-messages")
        .await
        .base
        .id;
    full_org_send(&world, &c.boss, other).await;
    let mut ids = Vec::new();
    for _ in 0..3 {
        ids.push(full_org_send(&world, &c.boss, c.acme).await.id);
    }
    ids.sort_by(|a, b| b.cmp(a));
    let reader = key(
        &world,
        c.boss.id(),
        vec![
            org_rule("OrgMessageList", c.acme, &["read"]),
            org_rule("OrgMessageList", other, &["read"]),
        ],
    )
    .await;
    let first = value(sent(&world, &reader, Some(c.acme), None, 1).await);
    assert_eq!(first["items"].as_array().unwrap().len(), 1);
    assert_eq!(first["items"][0]["id"], json!(ids[0]));
    assert!(first["items"][0].get("body").is_none());
    assert!(first["items"][0]["recipient_count"].as_i64().unwrap() > 0);
    assert_eq!(
        first["audience_count"],
        json!(
            orch_db::queries::admin_messages::audience_count(
                &mut *world.conn().await,
                Some(c.acme),
                c.boss.id()
            )
            .await
            .unwrap()
        )
    );
    let cursor = first["next_cursor"].as_str().unwrap().to_owned();
    let newer = full_org_send(&world, &c.boss, c.acme).await;
    sqlx::query("UPDATE admin_messages SET created_at=$2 WHERE id=$1")
        .bind(newer.id)
        .bind(world.now() + Duration::seconds(1))
        .execute(&world.db.pool)
        .await
        .unwrap();
    let next = value(sent(&world, &reader, Some(c.acme), Some(cursor.clone()), 2).await);
    assert_eq!(
        next["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| serde_json::from_value::<Id>(row["id"].clone()).unwrap())
            .collect::<Vec<_>>(),
        ids[1..]
    );
    assert!(next["next_cursor"].is_null());
    assert_eq!(
        sent(&world, &reader, Some(other), Some(cursor.clone()), 1)
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
    places::give_role(&world, c.acme, &c.ada, "admin", &c.boss).await;
    let another = key(
        &world,
        c.ada.id(),
        vec![org_rule("OrgMessageList", c.acme, &["read"])],
    )
    .await;
    assert_eq!(
        sent(&world, &another, Some(c.acme), Some(cursor), 1)
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        sent(&world, &c.ada.token, Some(other), None, 1)
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    let neighbour = key(
        &world,
        c.boss.id(),
        vec![org_rule("OrgMessageCreate", c.acme, &["read"])],
    )
    .await;
    assert_eq!(
        sent(&world, &neighbour, Some(c.acme), None, 1)
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
}

#[tokio::test]
async fn system_messages_are_readable_in_the_inbox_but_the_sent_list_keeps_current_system_administration()
 {
    let world = World::new().await;
    let c = admin_support::cast(&world).await;
    let call = world
        .app
        .call(AdminMessageCreate, &[])
        .token(&c.root.token)
        .body(&body());
    let AdminMessageCreateResponse::Full(message) = call.send().await.ok() else {
        panic!("ordinary system sender")
    };
    let reader = key(&world, c.ada.id(), vec![system_read("MessageGet")]).await;
    assert_eq!(
        value(get(&world, &reader, message.id).await)["body"],
        message.body
    );
    let denied = key(&world, c.ada.id(), vec![system_read("AdminMessageList")]).await;
    assert_eq!(
        sent(&world, &denied, None, None, 2).await.problem().code,
        ErrorCode::AuthzDenied
    );
    let admin = key(&world, c.root.id(), vec![system_read("AdminMessageList")]).await;
    let listed = value(sent(&world, &admin, None, None, 2).await);
    assert_eq!(listed["items"][0]["id"], json!(message.id));
    assert!(listed["items"][0]["org"].is_null());
    assert!(listed["next_cursor"].is_null());
    assert_eq!(listed["audience_count"], json!(message.recipient_count));
    places::admin_mode(&world, &c.root, false).await;
    assert_eq!(
        sent(&world, &c.root.token, None, None, 2)
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    assert_eq!(
        value(get(&world, &c.root.token, message.id).await)["id"],
        json!(message.id),
        "received content does not borrow the admin switch"
    );
}
