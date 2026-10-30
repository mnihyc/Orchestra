//! A webhook is posted to, signed, held to the list of private hosts, and what the far side answers decides what becomes of the delivery.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use orch_notifier_contract::delivery::{Delivery, Next, Saved, after_delivery, on_check};
use orch_notifier_contract::notifier::Notifier;
use orch_notifier_webhook::{WebhookNotifier, signature};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A request the stand-in saw.
#[derive(Debug, Clone)]
struct Seen {
    path: String,
    headers: HeaderMap,
    body: Vec<u8>,
}

type Log = Arc<Mutex<Vec<Seen>>>;

/// What the stand-in answers, by path.
async fn answer(State(log): State<Log>, uri: Uri, headers: HeaderMap, body: Bytes) -> Response {
    log.lock().unwrap().push(Seen {
        path: uri.path().to_owned(),
        headers: headers.clone(),
        body: body.to_vec(),
    });
    let query: std::collections::BTreeMap<String, String> = uri
        .query()
        .map(|query| {
            url::form_urlencoded::parse(query.as_bytes())
                .into_owned()
                .collect()
        })
        .unwrap_or_default();
    match uri.path() {
        "/ok" => StatusCode::NO_CONTENT.into_response(),
        "/slow-down" => (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, "7")],
            "slow down",
        )
            .into_response(),
        "/boom" => (StatusCode::INTERNAL_SERVER_ERROR, "boom").into_response(),
        "/gone" => (StatusCode::GONE, "no such hook").into_response(),
        "/missing" => (StatusCode::NOT_FOUND, "unknown webhook").into_response(),
        "/bad" => (
            StatusCode::BAD_REQUEST,
            "invalid_payload\n  the text is missing",
        )
            .into_response(),
        "/forbidden" => StatusCode::FORBIDDEN.into_response(),
        "/redirect" => {
            let to = query.get("to").cloned().unwrap_or_default();
            (
                StatusCode::TEMPORARY_REDIRECT,
                [(header::LOCATION, HeaderValue::from_str(&to).unwrap())],
            )
                .into_response()
        }
        "/loop" => (
            StatusCode::TEMPORARY_REDIRECT,
            [(header::LOCATION, "/loop")],
        )
            .into_response(),
        _ => StatusCode::IM_A_TEAPOT.into_response(),
    }
}

/// Starts the stand-in on a port from 41000 to 41999.
async fn stand_in() -> (u16, Log) {
    let log = Log::default();
    let app = Router::new().fallback(answer).with_state(Arc::clone(&log));
    let seed = u64::from(std::process::id()).wrapping_mul(2_654_435_761);
    for attempt in 0..400_u64 {
        let port = 41_000 + u16::try_from(seed.wrapping_add(attempt * 7_919) % 1_000).unwrap();
        if let Ok(listener) = tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            return (port, log);
        }
    }
    panic!("no free port from 41000 to 41999");
}

fn delivery(url: &str, secret: Option<&str>, body: &str, allowed: Value) -> Delivery {
    serde_json::from_value(json!({
        "id": "0199abcd-1234-7000-8000-0000000000d1", "attempt": 2,
        "config": { "values": { "url": url }, "secrets": secret.map_or_else(|| json!({}), |s| json!({ "secret": s })),
                    "allowed_private_hosts": allowed },
        "to": {}, "recipient": null, "language": "en", "subject": null, "body": body,
        "notifications": [ { "id": "0199abcd-1234-7000-8000-000000000001", "kind": "turn.completed",
            "at": "2026-10-11T10:00:00.000Z", "title": "Turn 3 finished", "body": "all green",
            "url": "https://orch.example/thread/9", "node": null, "thread_id": null, "chat_id": null,
            "data": { "turn": 3 }, "recipient": null } ]
    }))
    .unwrap()
}

const SCHEDULE: [Duration; 2] = [Duration::from_secs(60), Duration::from_secs(300)];

fn next(result: &Result<(), orch_notifier_contract::delivery::DeliverError>) -> Next {
    after_delivery(result, 1, &SCHEDULE)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_post_carries_the_body_of_the_notification_and_a_signature_the_receiver_can_check() {
    let (port, log) = stand_in().await;
    let url = format!("http://127.0.0.1:{port}/ok");
    let result = WebhookNotifier::new()
        .deliver(delivery(&url, Some("s3cr3t"), "", json!(["127.0.0.1"])))
        .await;
    assert_eq!(result, Ok(()));
    let seen = log.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].headers[header::CONTENT_TYPE], "application/json");
    let sent = seen[0]
        .headers
        .get(signature::HEADER)
        .expect("a webhook with a secret signs")
        .to_str()
        .unwrap();
    assert!(sent.starts_with("sha256="));
    assert!(
        signature::verify(b"s3cr3t", &seen[0].body, sent),
        "the receiver checks the bytes it got"
    );
    assert!(!signature::verify(b"other", &seen[0].body, sent));
    assert_eq!(
        seen[0].headers["x-orch-delivery"],
        "0199abcd-1234-7000-8000-0000000000d1"
    );
    assert_eq!(seen[0].headers["x-orch-attempt"], "2");
    let body: Value = serde_json::from_slice(&seen[0].body).unwrap();
    for member in [
        "id",
        "kind",
        "at",
        "title",
        "body",
        "url",
        "node",
        "thread_id",
        "chat_id",
        "data",
        "recipient",
    ] {
        assert!(body.get(member).is_some(), "{member}");
    }
    assert_eq!(body["kind"], "turn.completed");
    assert_eq!(body["data"]["turn"], 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_secret_nothing_is_signed_and_a_text_of_the_channels_own_is_posted_as_it_was_rendered()
 {
    let (port, log) = stand_in().await;
    let url = format!("http://127.0.0.1:{port}/ok");
    let text = r#"{"text": "Turn 3 finished\nall green"}"#;
    WebhookNotifier::new()
        .deliver(delivery(&url, None, text, Value::Null))
        .await
        .unwrap();
    let seen = log.lock().unwrap().clone();
    assert!(seen[0].headers.get(signature::HEADER).is_none());
    assert_eq!(String::from_utf8(seen[0].body.clone()).unwrap(), text);
}

#[tokio::test(flavor = "multi_thread")]
async fn what_the_far_side_answers_decides_what_becomes_of_the_delivery() {
    let (port, _) = stand_in().await;
    let post = |path: &str| {
        let url = format!("http://127.0.0.1:{port}{path}");
        async move {
            WebhookNotifier::new()
                .deliver(delivery(&url, None, "", Value::Null))
                .await
        }
    };
    let slow = post("/slow-down").await;
    assert_eq!(
        next(&slow),
        Next::RetryAfter(Duration::from_secs(7)),
        "the time the far side named"
    );
    assert_eq!(
        next(&post("/boom").await),
        Next::RetryAfter(Duration::from_secs(60))
    );
    assert_eq!(
        next(&post("/gone").await),
        Next::Gone,
        "a hook that was deleted"
    );
    assert_eq!(next(&post("/missing").await), Next::Gone);
    let bad = post("/bad").await;
    assert_eq!(next(&bad), Next::Failed);
    assert!(
        bad.unwrap_err()
            .message
            .contains("invalid_payload the text is missing"),
        "what the far side said, on one line"
    );
    assert_eq!(next(&post("/forbidden").await), Next::Failed);
    let closed = (41_000..42_000_u16)
        .rev()
        .find(|port| std::net::TcpListener::bind(("127.0.0.1", *port)).is_ok())
        .unwrap();
    let down = WebhookNotifier::new()
        .deliver(delivery(
            &format!("http://127.0.0.1:{closed}/x"),
            None,
            "",
            Value::Null,
        ))
        .await;
    assert!(matches!(next(&down), Next::RetryAfter(_)), "{down:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_private_address_is_refused_before_anything_is_sent_unless_the_list_names_it() {
    let (port, log) = stand_in().await;
    let url = format!("http://127.0.0.1:{port}/ok");
    let notifier = WebhookNotifier::new();
    for allowed in [json!([]), json!(["hooks.example"]), json!(["10.0.0.0/8"])] {
        let result = notifier
            .deliver(delivery(&url, None, "", allowed.clone()))
            .await;
        let error = result.unwrap_err();
        assert_eq!(
            (error.code.as_str(), error.retryable, error.gone),
            ("address_not_allowed", false, false),
            "{allowed}"
        );
    }
    assert!(log.lock().unwrap().is_empty(), "the far side saw nothing");
    for listed in [json!(["127.0.0.1"]), json!(["127.0.0.0/8"]), Value::Null] {
        notifier
            .deliver(delivery(&url, None, "", listed))
            .await
            .unwrap();
    }
    assert_eq!(log.lock().unwrap().len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_redirect_is_looked_at_again_before_it_is_followed() {
    let (port, log) = stand_in().await;
    let to = format!("http://127.0.0.1:{port}/ok");
    let through = format!(
        "http://localhost:{port}/redirect?to={}",
        url::form_urlencoded::byte_serialize(to.as_bytes()).collect::<String>()
    );
    let notifier = WebhookNotifier::new();

    // The first address is listed, where the redirect leads is not.
    let refused = notifier
        .deliver(delivery(&through, Some("k"), "", json!(["localhost"])))
        .await
        .unwrap_err();
    assert_eq!(refused.code, "address_not_allowed");
    assert!(refused.message.contains("127.0.0.1"), "{}", refused.message);
    {
        let seen = log.lock().unwrap();
        assert_eq!(seen.len(), 1, "only the redirect itself was asked");
        assert_eq!(seen[0].path, "/redirect");
    }

    // Both are listed: the post is made again, to where it leads, with the same body and signature.
    notifier
        .deliver(delivery(
            &through,
            Some("k"),
            "",
            json!(["localhost", "127.0.0.1"]),
        ))
        .await
        .unwrap();
    let seen = log.lock().unwrap().clone();
    assert_eq!(seen.len(), 3);
    assert_eq!(seen[2].path, "/ok");
    assert_eq!(seen[1].body, seen[2].body);
    assert!(signature::verify(
        b"k",
        &seen[2].body,
        seen[2]
            .headers
            .get(signature::HEADER)
            .unwrap()
            .to_str()
            .unwrap()
    ));

    let looping = format!("http://127.0.0.1:{port}/loop");
    let error = notifier
        .deliver(delivery(&looping, None, "", Value::Null))
        .await
        .unwrap_err();
    assert_eq!(
        (error.code.as_str(), error.retryable),
        ("too_many_redirects", false)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_webhook_is_looked_at_when_it_is_saved_without_posting_anything() {
    let (port, log) = stand_in().await;
    let notifier = WebhookNotifier::new();
    let config = |url: &str, allowed: Value| -> orch_notifier_contract::delivery::Config {
        serde_json::from_value(
            json!({ "values": { "url": url }, "secrets": {}, "allowed_private_hosts": allowed }),
        )
        .unwrap()
    };
    let here = format!("http://127.0.0.1:{port}/ok");
    assert_eq!(
        on_check(notifier.check(&config(&here, json!(["127.0.0.1"]))).await),
        Saved::Kept(Value::Null)
    );
    let Saved::Refused(error) = on_check(notifier.check(&config(&here, json!([]))).await) else {
        panic!("a private address must refuse the save");
    };
    assert_eq!(error.code, "address_not_allowed");
    for bad in ["ftp://x.example", "not an address", ""] {
        let Saved::Refused(error) = on_check(notifier.check(&config(bad, Value::Null)).await)
        else {
            panic!("{bad:?} must refuse the save");
        };
        assert!(
            error.code == "invalid_url" || error.code == "invalid_config",
            "{bad:?}: {}",
            error.code
        );
    }
    assert!(log.lock().unwrap().is_empty(), "a check delivers nothing");
}
