//! The Telegram notifier against a stand-in for the Bot API: it reads who the bot is, writes within the length a message takes, finds out who pressed Start, and tells a person who stopped the bot from a bot that is merely slow.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use axum::Json as AxumJson;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use orch_notifier_contract::delivery::{Config, Delivery, Next, Saved, after_delivery, on_check};
use orch_notifier_contract::notifier::Notifier;
use orch_notifier_telegram::TelegramNotifier;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use url::Url;

/// What the stand-in was asked, and what it will say of the updates.
#[derive(Default)]
struct World {
    calls: Vec<(String, String, Value)>,
    updates: Vec<Value>,
}

type Shared = Arc<Mutex<World>>;

fn ok(result: Value) -> Response {
    AxumJson(json!({ "ok": true, "result": result })).into_response()
}

fn refused(status: u16, words: &str, parameters: Value) -> Response {
    (
        StatusCode::from_u16(status).unwrap(),
        AxumJson(json!({ "ok": false, "error_code": status, "description": words, "parameters": parameters })),
    )
        .into_response()
}

/// `POST /bot<token>/<method>`, as the Bot API answers it.
async fn api(
    State(shared): State<Shared>,
    Path((bot, method)): Path<(String, String)>,
    AxumJson(body): AxumJson<Value>,
) -> Response {
    let token = bot.strip_prefix("bot").unwrap_or_default().to_owned();
    shared
        .lock()
        .unwrap()
        .calls
        .push((token.clone(), method.clone(), body.clone()));
    if token != "123:good" {
        return refused(401, "Unauthorized", json!(null));
    }
    match method.as_str() {
        "getMe" => {
            ok(json!({ "id": 123, "is_bot": true, "first_name": "Orch", "username": "orch_bot" }))
        }
        "sendMessage" => match body["chat_id"].as_i64().unwrap_or_default() {
            1 => ok(json!({ "message_id": 9 })),
            2 => refused(403, "Forbidden: bot was blocked by the user", json!(null)),
            3 => refused(400, "Bad Request: chat not found", json!(null)),
            4 => refused(
                429,
                "Too Many Requests: retry after 5",
                json!({ "retry_after": 5 }),
            ),
            5 => refused(502, "Bad Gateway", json!(null)),
            _ => ok(json!({ "message_id": 10 })),
        },
        "getUpdates" => {
            let offset = body["offset"].as_i64().unwrap_or(0);
            let mut world = shared.lock().unwrap();
            // Asking from an offset confirms what is before it.
            world
                .updates
                .retain(|update| update["update_id"].as_i64().unwrap_or(0) >= offset);
            ok(Value::Array(world.updates.clone()))
        }
        _ => refused(404, "Not Found", json!(null)),
    }
}

async fn stand_in() -> (Url, Shared) {
    let shared = Shared::default();
    let app = Router::new()
        .route("/{bot}/{method}", post(api))
        .with_state(Arc::clone(&shared));
    let seed = u64::from(std::process::id()).wrapping_mul(2_654_435_761);
    for attempt in 0..400_u64 {
        let port = 41_000 + u16::try_from(seed.wrapping_add(attempt * 7_919) % 1_000).unwrap();
        if let Ok(listener) = tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            return (
                Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap(),
                shared,
            );
        }
    }
    panic!("no free port from 41000 to 41999");
}

fn config(token: &str, username: Option<&str>) -> Config {
    serde_json::from_value(json!({
        "values": username.map_or_else(|| json!({}), |name| json!({ "bot_username": name })),
        "secrets": { "bot_token": token }
    }))
    .unwrap()
}

fn delivery(chat: Value, body: &str) -> Delivery {
    serde_json::from_value(json!({
        "id": "0199abcd-1234-7000-8000-0000000000d1", "attempt": 1,
        "config": { "values": {}, "secrets": { "bot_token": "123:good" } },
        "to": chat, "recipient": null, "language": "en", "subject": null, "body": body, "notifications": []
    }))
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_bot_is_asked_who_it_is_when_the_token_is_saved_and_a_token_that_is_not_one_refuses_the_save()
 {
    let (base, shared) = stand_in().await;
    let notifier = TelegramNotifier::against(base);
    let checked = on_check(notifier.check(&config("123:good", None)).await);
    assert_eq!(checked, Saved::Kept(json!({ "bot_username": "orch_bot" })));
    let Saved::Refused(error) = on_check(notifier.check(&config("123:wrong", None)).await) else {
        panic!("a token that is not a bot's refuses the save");
    };
    assert_eq!(
        (error.code.as_str(), error.retryable),
        ("unauthorized", false)
    );
    assert!(
        !error.message.contains("123:wrong"),
        "the token is in the address, and never in an error: {}",
        error.message
    );
    assert_eq!(shared.lock().unwrap().calls[0].1, "getMe");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_is_written_as_plain_text_to_the_chat_and_no_more_than_a_message_takes() {
    let (base, shared) = stand_in().await;
    let notifier = TelegramNotifier::against(base);
    notifier
        .deliver(delivery(
            json!({ "chat_id": 1, "label": "Dana" }),
            "Turn 3 finished\nall green",
        ))
        .await
        .unwrap();
    let long = "x".repeat(5_000);
    notifier
        .deliver(delivery(json!({ "chat_id": 1 }), &long))
        .await
        .unwrap();
    let calls = shared.lock().unwrap().calls.clone();
    assert_eq!(calls[0].0, "123:good");
    assert_eq!(calls[0].1, "sendMessage");
    assert_eq!(calls[0].2["chat_id"], 1);
    assert_eq!(calls[0].2["text"], "Turn 3 finished\nall green");
    assert!(
        calls[0].2.get("parse_mode").is_none(),
        "plain text: nothing in it is read as formatting"
    );
    assert_eq!(calls[1].2["text"].as_str().unwrap().chars().count(), 4096);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_person_who_stopped_the_bot_is_gone_and_a_bot_that_is_slowed_down_is_waited_for() {
    let (base, _) = stand_in().await;
    let notifier = TelegramNotifier::against(base);
    let schedule = [Duration::from_secs(60)];
    let next = |result| after_delivery(&result, 1, &schedule);
    assert_eq!(
        next(
            notifier
                .deliver(delivery(json!({ "chat_id": 1 }), "x"))
                .await
        ),
        Next::Taken
    );
    assert_eq!(
        next(
            notifier
                .deliver(delivery(json!({ "chat_id": 2 }), "x"))
                .await
        ),
        Next::Gone,
        "blocked the bot"
    );
    assert_eq!(
        next(
            notifier
                .deliver(delivery(json!({ "chat_id": 3 }), "x"))
                .await
        ),
        Next::Gone,
        "chat not found"
    );
    assert_eq!(
        next(
            notifier
                .deliver(delivery(json!({ "chat_id": 4 }), "x"))
                .await
        ),
        Next::RetryAfter(Duration::from_secs(5)),
        "the time Telegram named"
    );
    assert_eq!(
        next(
            notifier
                .deliver(delivery(json!({ "chat_id": 5 }), "x"))
                .await
        ),
        Next::RetryAfter(Duration::from_secs(60))
    );
    let no_chat = notifier
        .deliver(delivery(json!({}), "x"))
        .await
        .unwrap_err();
    assert!(
        no_chat.gone,
        "a route that was never linked has nowhere to write"
    );
    let blocked = notifier
        .deliver(delivery(json!({ "chat_id": 2 }), "x"))
        .await
        .unwrap_err();
    assert!(
        blocked.message.contains("blocked by the user"),
        "Telegram's own words are kept: {}",
        blocked.message
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_bot_is_read_for_who_pressed_start_from_where_the_last_reading_ended() {
    let (base, shared) = stand_in().await;
    let notifier = TelegramNotifier::against(base);
    shared.lock().unwrap().updates = vec![
        json!({ "update_id": 100, "message": { "chat": { "id": 7, "type": "private" }, "from": { "id": 7, "first_name": "Dana", "last_name": "Scully" }, "text": "/start AbCd-1234_x" } }),
        json!({ "update_id": 101, "message": { "chat": { "id": 8, "type": "private" }, "from": { "id": 8, "first_name": "Fox" }, "text": "good morning" } }),
        json!({ "update_id": 102, "edited_message": { "chat": { "id": 8, "type": "private" } } }),
    ];
    let config = config("123:good", Some("orch_bot"));
    let first = notifier
        .poll_links(&config, None, Duration::from_secs(25))
        .await
        .unwrap();
    assert_eq!(first.started.len(), 1);
    assert_eq!(first.started[0].code, "AbCd-1234_x");
    assert_eq!(first.started[0].address, json!({ "chat_id": 7 }));
    assert_eq!(first.started[0].label, "Dana Scully");
    assert_eq!(
        first.cursor.as_deref(),
        Some("103"),
        "past the newest update, which also confirms them"
    );

    shared.lock().unwrap().updates.push(
        json!({ "update_id": 103, "message": { "chat": { "id": 9, "type": "private" }, "from": { "id": 9, "first_name": "Walter" }, "text": "/start ZZ99" } }),
    );
    let second = notifier
        .poll_links(&config, first.cursor.clone(), Duration::from_secs(25))
        .await
        .unwrap();
    assert_eq!(second.started.len(), 1, "only what came after");
    assert_eq!(second.started[0].code, "ZZ99");
    assert_eq!(second.cursor.as_deref(), Some("104"));

    let third = notifier
        .poll_links(&config, second.cursor.clone(), Duration::ZERO)
        .await
        .unwrap();
    assert!(third.started.is_empty());
    assert_eq!(
        third.cursor.as_deref(),
        Some("104"),
        "nothing new: the cursor stays"
    );

    let readings: Vec<_> = shared
        .lock()
        .unwrap()
        .calls
        .iter()
        .filter(|call| call.1 == "getUpdates")
        .map(|call| call.2.clone())
        .collect();
    assert_eq!(readings.len(), 3);
    assert!(
        readings[0].get("offset").is_none()
            && readings[1]["offset"] == 103
            && readings[2]["offset"] == 104
    );
    assert_eq!(readings[0]["timeout"], 25, "one waiting request");
    assert_eq!(readings[0]["allowed_updates"], json!(["message"]));
    assert_eq!(readings[2]["timeout"], 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bot_that_cannot_be_reached_is_tried_again_and_the_token_is_not_in_what_is_said() {
    let closed = (41_000..42_000_u16)
        .rev()
        .find(|port| std::net::TcpListener::bind(("127.0.0.1", *port)).is_ok())
        .unwrap();
    let notifier =
        TelegramNotifier::against(Url::parse(&format!("http://127.0.0.1:{closed}/")).unwrap());
    let error = notifier
        .deliver(delivery(json!({ "chat_id": 1 }), "x"))
        .await
        .unwrap_err();
    assert!(error.retryable && !error.gone, "{error}");
    assert!(!error.message.contains("123:good"), "{}", error.message);
    let error = notifier.check(&config("123:good", None)).await.unwrap_err();
    assert!(
        error.retryable,
        "a bot that cannot be reached now does not refuse the save"
    );
    assert!(!format!("{error:?}").contains("123:good"));
}
