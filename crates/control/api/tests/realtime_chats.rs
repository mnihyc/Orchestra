//! The chat feed on the realtime socket, tested with a real WebSocket client against a real database: the rows of a chat in sequence, none missed across a reconnect, membership lost closing the subscription, typing, an admin who only inspects, and the news that reaches a person without a subscription.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod chat_world;
mod places;
mod realtime_support;
mod support;

use chat_world::{Room, text};
use chrono::Duration as Span;
use orch_api_contract::chats::{
    ChatMemberRemove, ChatMessageDelete, ChatMessagePinSet, ChatMessageReactionAdd,
    ChatMessageUpdate, ChatMessageUpdateBody, ChatRead, ChatReadBody, ChatUpdate, ChatUpdateBody,
};
use orch_config::ServerConfig;
use orch_db::Db;
use orch_types::{Clock, Id};
use realtime_support::{Client, Server, quick};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

/// A realtime server on the room's database, which reads the room's clock; the sweep is a minute
/// away, so that what arrives arrives by the announcement.
async fn serve(room: &Room) -> Server {
    let clock: Arc<dyn Clock> = Arc::new(room.world.app.clock.clone());
    let db = Db::from_pool(room.world.db.pool.clone());
    let mut tuning = quick(&ServerConfig::default());
    tuning.sweep_every = Duration::from_secs(60);
    Server::over(&db, &clock, tuning).await
}

async fn follow(
    server: &Server,
    who: &places::Who,
    id: i64,
    feed: &str,
    since: Option<i64>,
) -> Client {
    let mut client = Client::signed_in(server.addr, &who.token).await;
    client.sub(id, feed, since).await;
    client
}

fn messages(frame: &Value) -> Vec<Value> {
    frame["data"]["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn words(message: &Value) -> String {
    message["parts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|part| part["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------- rows in sequence

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_subscriber_is_sent_each_message_and_each_change_of_one_as_the_messages_row_in_sequence()
{
    let room = Room::new().await;
    let server = serve(&room).await;
    let feed = format!("chat:{}", room.chat);
    let mut bea = follow(&server, &room.bea, 1, &feed, None).await;
    let subscribed = bea.expect("subscribed").await;
    assert_eq!(
        (subscribed["mode"].as_str(), subscribed["seq"].as_i64()),
        (Some("delta"), Some(0))
    );
    let presence = bea.expect("presence").await;
    assert_eq!(presence["users"], json!([]));

    let one = room.say(&room.ada, room.chat, "one").await;
    let first = bea.expect("sync").await;
    assert_eq!(first["feed"], feed.as_str());
    assert_eq!(
        (first["prev"].as_i64(), first["seq"].as_i64()),
        (Some(0), Some(one.seq_created))
    );
    assert_eq!(words(&messages(&first)[0]), "one");
    assert!(
        first["data"].get("chat").is_none(),
        "the chat itself did not change"
    );

    // An edit, a reaction, a pin and a deletion are each the row of the message.
    let mut last = first["seq"].as_i64().unwrap();
    let mut rows = Vec::new();
    room.world
        .app
        .call(ChatMessageUpdate, &[&room.chat, &one.id])
        .token(&room.ada.token)
        .body(&ChatMessageUpdateBody {
            parts: vec![text("one, edited")],
        })
        .send()
        .await
        .ok();
    rows.push(bea.expect("sync").await);
    room.world
        .app
        .call(ChatMessageReactionAdd, &[&room.chat, &one.id, &"x"])
        .token(&room.cai.token)
        .send()
        .await
        .ok();
    rows.push(bea.expect("sync").await);
    room.world
        .app
        .call(ChatMessagePinSet, &[&room.chat, &one.id])
        .token(&room.cai.token)
        .send()
        .await
        .ok();
    rows.push(bea.expect("sync").await);
    room.world
        .app
        .call(ChatMessageDelete, &[&room.chat, &one.id])
        .token(&room.ada.token)
        .send()
        .await
        .ok();
    rows.push(bea.expect("sync").await);
    for frame in &rows {
        assert_eq!(
            frame["prev"].as_i64(),
            Some(last),
            "each frame begins where the one before ended"
        );
        last = frame["seq"].as_i64().unwrap();
        let row = &messages(frame)[0];
        assert_eq!(row["id"], one.id.to_string());
        assert_eq!(row["seq_updated"].as_i64(), Some(last));
    }
    assert_eq!(words(&messages(&rows[0])[0]), "one, edited");
    assert!(messages(&rows[0])[0]["edited_at"].is_string());
    assert_eq!(messages(&rows[1])[0]["reactions"][0]["emoji"], "x");
    assert!(messages(&rows[2])[0]["pinned"]["by"]["username"] == "cai");
    let gone = &messages(&rows[3])[0];
    assert_eq!(gone["deleted"], true);
    assert_eq!(gone["parts"], json!([]));
    assert_eq!(gone["reactions"], json!([]));
    assert!(gone["pinned"].is_null());
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reconnect_from_a_sequence_is_sent_every_message_it_missed_in_order_and_none_twice() {
    let room = Room::new().await;
    let server = serve(&room).await;
    let feed = format!("chat:{}", room.chat);
    let seen = room.say(&room.ada, room.chat, "seen").await;
    let mut first = follow(&server, &room.bea, 1, &feed, Some(seen.seq_created)).await;
    assert_eq!(first.expect("subscribed").await["seq"], seen.seq_created);
    first.expect("presence").await;
    drop(first);
    // While she is away.
    let missed: Vec<Id> = vec![
        room.say(&room.ada, room.chat, "two").await.id,
        room.say(&room.cai, room.chat, "three").await.id,
        room.say(&room.ada, room.chat, "four").await.id,
    ];
    let mut back = follow(&server, &room.bea, 1, &feed, Some(seen.seq_created)).await;
    back.expect("subscribed").await;
    back.expect("presence").await;
    let catch_up = back.expect("sync").await;
    let ids: Vec<String> = messages(&catch_up)
        .iter()
        .map(|message| message["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        ids,
        missed.iter().map(ToString::to_string).collect::<Vec<_>>()
    );
    assert_eq!(catch_up["prev"], seen.seq_created);
    assert!(
        back.is_quiet_for(Duration::from_millis(300)).await,
        "and nothing twice"
    );
    // A position beyond the chat's is no position: the chat is loaded again.
    let mut ahead = follow(&server, &room.bea, 2, &feed, Some(9_999)).await;
    assert_eq!(ahead.expect("subscribed").await["mode"], "reload");
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rename_and_a_new_member_arrive_as_the_chat_itself_for_each_subscriber() {
    let room = Room::new().await;
    let server = serve(&room).await;
    let group = room
        .group(&room.bea, &[&room.cai], Some("Release"))
        .await
        .base
        .id;
    let feed = format!("chat:{group}");
    let mut cai = follow(&server, &room.cai, 1, &feed, None).await;
    cai.expect("subscribed").await;
    cai.expect("presence").await;
    room.world
        .app
        .call(ChatUpdate, &[&group])
        .token(&room.bea.token)
        .body(&ChatUpdateBody {
            name: Some("Launch".to_owned()),
        })
        .send()
        .await
        .ok();
    let frame = cai.expect("sync").await;
    assert_eq!(frame["data"]["chat"]["name"], "Launch");
    assert_eq!(frame["data"]["chat"]["can"]["send"], true, "as she sees it");
    let line = &messages(&frame)[0];
    assert_eq!(line["kind"], "system");
    assert_eq!(line["system"]["event"], "renamed");
    server.stop().await;
}

// ---------------------------------------------------------------- who may follow

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_chat_may_be_followed_by_those_who_read_it_and_by_nobody_else() {
    let room = Room::new().await;
    let server = serve(&room).await;
    let direct = room.direct(&room.bea, &room.cai).await.base.id;
    for (who, chat, code) in [
        (&room.gus, direct, "forbidden"),
        (&room.eve, room.chat, "forbidden"),
        (&room.bea, orch_types::new_id(), "not_found"),
    ] {
        let mut client = follow(&server, who, 1, &format!("chat:{chat}"), None).await;
        let refused = client.expect("error").await;
        assert_eq!(
            (refused["code"].as_str(), refused["id"].as_i64()),
            (Some(code), Some(1))
        );
    }
    let mut member = follow(&server, &room.cai, 1, &format!("chat:{direct}"), None).await;
    assert_eq!(member.expect("subscribed").await["mode"], "delta");
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn membership_lost_closes_the_subscription_with_revoked_before_any_further_row() {
    let room = Room::new().await;
    let server = serve(&room).await;
    let group = room
        .group(&room.bea, &[&room.cai, &room.gus], Some("Release"))
        .await
        .base
        .id;
    let feed = format!("chat:{group}");
    let mut gus = follow(&server, &room.gus, 1, &feed, None).await;
    gus.expect("subscribed").await;
    gus.expect("presence").await;
    room.say(&room.bea, group, "welcome").await;
    gus.expect("sync").await;

    room.world
        .app
        .call(ChatMemberRemove, &[&group, &room.gus.id()])
        .token(&room.bea.token)
        .send()
        .await
        .ok();
    room.say(&room.bea, group, "he is out").await;
    let closed = gus.until("closed").await;
    let last = closed.last().unwrap();
    assert_eq!(
        (last["reason"].as_str(), last["id"].as_i64()),
        (Some("revoked"), Some(1))
    );
    assert!(
        closed
            .iter()
            .all(|frame| frame["op"] != "sync" || words(&messages(frame)[0]) != "he is out"),
        "the message after the removal is not sent to him"
    );
    assert!(gus.is_quiet_for(Duration::from_millis(400)).await);

    // The chat of a node: the page of the node stops giving the right to read it.
    let mut cai = follow(&server, &room.cai, 2, &format!("chat:{}", room.chat), None).await;
    cai.expect("subscribed").await;
    cai.expect("presence").await;
    {
        let mut conn = room.world.conn().await;
        sqlx::query("UPDATE node_roles SET removed_at = now() WHERE subject_id = $1")
            .bind(room.cai.id())
            .execute(&mut *conn)
            .await
            .unwrap();
        orch_authz::epoch::bump(&mut conn).await.unwrap();
    }
    let revoked = cai.until("closed").await;
    assert_eq!(revoked.last().unwrap()["reason"], "revoked");
    server.stop().await;
}

// ---------------------------------------------------------------- typing and presence

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn typing_is_shown_to_the_other_subscribers_only_and_one_frame_in_three_seconds_is_let_through()
 {
    let room = Room::new().await;
    let server = serve(&room).await;
    let feed = format!("chat:{}", room.chat);
    let mut ada = follow(&server, &room.ada, 1, &feed, None).await;
    let mut bea = follow(&server, &room.bea, 7, &feed, None).await;
    for client in [&mut ada, &mut bea] {
        client.expect("subscribed").await;
        client.expect("presence").await;
    }
    ada.send(json!({ "op": "typing", "feed": feed })).await;
    let event = bea.expect("event").await;
    assert_eq!(event["id"], 7);
    assert_eq!(event["feed"], feed.as_str());
    assert_eq!(event["data"]["what"], "typing");
    assert_eq!(event["data"]["user"]["username"], "ada");
    assert!(
        ada.is_quiet_for(Duration::from_millis(300)).await,
        "never shown to the sender"
    );

    // More than one frame in three seconds is dropped; the next after them is let through.
    ada.send(json!({ "op": "typing", "feed": feed })).await;
    assert!(bea.is_quiet_for(Duration::from_millis(300)).await);
    room.world.advance(Span::seconds(4));
    ada.send(json!({ "op": "typing", "feed": feed })).await;
    assert_eq!(bea.expect("event").await["data"]["user"]["username"], "ada");

    // Typing in a chat the connection does not follow, or in no chat, is ignored.
    ada.send(json!({ "op": "typing", "feed": format!("chat:{}", orch_types::new_id()) }))
        .await;
    ada.send(json!({ "op": "typing", "feed": "user" })).await;
    assert!(bea.is_quiet_for(Duration::from_millis(300)).await);
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_admin_who_only_inspects_a_chat_is_left_out_of_its_presence_cannot_type_and_leaves_one_audit_row()
 {
    let room = Room::new().await;
    let server = serve(&room).await;
    let direct = room.direct(&room.bea, &room.cai).await.base.id;
    let feed = format!("chat:{direct}");
    let mut bea = follow(&server, &room.bea, 1, &feed, None).await;
    bea.expect("subscribed").await;
    bea.expect("presence").await;
    bea.send(json!({ "op": "visible", "feed": feed, "visible": true }))
        .await;
    let looking = bea.expect("presence").await;
    assert_eq!(looking["users"].as_array().unwrap().len(), 1);

    let mut dee = follow(&server, &room.dee, 3, &feed, None).await;
    assert_eq!(dee.expect("subscribed").await["mode"], "delta");
    let list = dee.expect("presence").await;
    assert_eq!(
        list["users"].as_array().unwrap().len(),
        1,
        "she sees who looks"
    );
    dee.send(json!({ "op": "visible", "feed": feed, "visible": true }))
        .await;
    dee.send(json!({ "op": "typing", "feed": feed })).await;
    assert!(
        bea.is_quiet_for(Duration::from_millis(400)).await,
        "neither her looking nor her typing is shown to the people in the chat"
    );
    // She does see what is said.
    room.say(&room.bea, direct, "private").await;
    assert_eq!(words(&messages(&dee.expect("sync").await)[0]), "private");
    let rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'chat.inspected' AND object_id = $1 AND actor_id = $2",
    )
    .bind(direct)
    .bind(room.dee.id())
    .fetch_one(&room.world.db.pool)
    .await
    .unwrap();
    assert_eq!(rows, 1);
    server.stop().await;
}

// ---------------------------------------------------------------- news without a subscription

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_node_chats_last_message_reaches_the_org_feed_of_those_who_view_the_node() {
    let room = Room::new().await;
    let server = serve(&room).await;
    let mut bea = follow(&server, &room.bea, 1, &format!("org:{}", room.root), None).await;
    bea.expect("subscribed").await;
    let message = room.say(&room.ada, room.chat, "news").await;
    let tree = bea.expect("sync").await;
    let nodes = tree["data"]["nodes"].as_array().unwrap();
    let topic = nodes
        .iter()
        .find(|node| node["id"] == room.topic.to_string())
        .unwrap();
    assert_eq!(topic["chat"]["id"], room.chat.to_string());
    assert_eq!(topic["chat"]["seq"].as_i64(), Some(message.seq_created));
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_direct_message_a_mention_and_a_read_reach_the_persons_own_feed_without_a_chat_subscription()
 {
    let room = Room::new().await;
    let server = serve(&room).await;
    let mut cai = follow(&server, &room.cai, 1, "user", None).await;
    cai.expect("subscribed").await;
    let direct = room.direct(&room.bea, &room.cai).await.base.id;
    let first = cai.until("event").await;
    let created = first.last().unwrap();
    assert_eq!(
        created["data"]["what"], "chat",
        "a new chat arrives the same way"
    );
    assert_eq!(created["data"]["chat_id"], direct.to_string());

    let message = room.say(&room.bea, direct, "lunch?").await;
    let mut heard = Vec::new();
    while !heard.iter().any(|frame: &Value| frame["op"] == "sync")
        || !heard.iter().any(|frame: &Value| {
            frame["data"]["what"] == "chat" && frame["data"]["seq"] == message.seq_created
        })
    {
        heard.push(cai.frame().await);
    }
    let notified = heard.iter().find(|frame| frame["op"] == "sync").unwrap();
    assert_eq!(notified["data"]["notifications"][0]["kind"], "chat.message");
    assert_eq!(
        notified["data"]["notifications"][0]["chat_id"],
        direct.to_string()
    );

    // Named in the chat of a node: how the state stands now.
    room.say(&room.ada, room.chat, "@cai have a look").await;
    let mut named = None;
    for _ in 0..6 {
        let frame = cai.frame().await;
        if frame["data"]["what"] == "chat_state" {
            named = Some(frame);
            break;
        }
    }
    let state = &named.expect("told how their state stands")["data"]["state"];
    assert_eq!(
        (state["chat_id"].as_str(), state["mentions_unread"].as_i64()),
        (Some(room.chat.to_string().as_str()), Some(1))
    );

    // Reading on another screen reaches this one.
    room.world
        .app
        .call(ChatRead, &[&room.chat])
        .token(&room.cai.token)
        .body(&ChatReadBody {
            seq: 5,
            read_revision: 0,
            unread_from: None,
        })
        .send()
        .await
        .ok();
    let mut read = None;
    for _ in 0..6 {
        let frame = cai.frame().await;
        if frame["data"]["what"] == "chat_state" {
            read = Some(frame);
            break;
        }
    }
    let state = &read.expect("their other screen is told")["data"]["state"];
    assert_eq!(state["mentions_unread"], 0);
    assert!(state["last_read_seq"].as_i64().unwrap() > 0);
    server.stop().await;
}
