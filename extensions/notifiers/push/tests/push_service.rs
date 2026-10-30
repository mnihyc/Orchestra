//! A push goes to a browser encrypted to its key and signed with the system's VAPID key, is held to the list of private hosts, and a subscription the push service no longer knows is gone.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use orch_notifier_contract::delivery::{Delivery, Next, after_delivery};
use orch_notifier_contract::notifier::Notifier;
use orch_notifier_push::{PushNotifier, Vapid};
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, VerifyingKey};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone)]
struct Seen {
    path: String,
    headers: HeaderMap,
    body: Vec<u8>,
}

type Log = Arc<Mutex<Vec<Seen>>>;

async fn answer(State(log): State<Log>, uri: Uri, headers: HeaderMap, body: Bytes) -> Response {
    log.lock().unwrap().push(Seen {
        path: uri.path().to_owned(),
        headers,
        body: body.to_vec(),
    });
    match uri.path() {
        "/push/ok" => (
            StatusCode::CREATED,
            [(header::LOCATION, "/push/ok/message")],
        )
            .into_response(),
        "/push/gone" => (
            StatusCode::GONE,
            "push subscription has unsubscribed or expired",
        )
            .into_response(),
        "/push/missing" => StatusCode::NOT_FOUND.into_response(),
        "/push/slow" => {
            (StatusCode::TOO_MANY_REQUESTS, [(header::RETRY_AFTER, "30")]).into_response()
        }
        "/push/boom" => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        "/push/denied" => StatusCode::FORBIDDEN.into_response(),
        _ => StatusCode::IM_A_TEAPOT.into_response(),
    }
}

async fn push_service() -> (u16, Log) {
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

/// A browser: the key pair and authentication secret it subscribed with.
struct Browser {
    components: ece::EcKeyComponents,
    auth: [u8; 16],
    p256dh: String,
}

fn browser() -> Browser {
    let (pair, auth) = ece::generate_keypair_and_auth_secret().unwrap();
    let components = pair.raw_components().unwrap();
    Browser {
        p256dh: URL_SAFE_NO_PAD.encode(components.public_key()),
        components,
        auth,
    }
}

fn the_systems_key() -> Vapid {
    let key = rcgen::KeyPair::generate().unwrap();
    Vapid::read(&key.serialize_pem(), "https://orch.example").unwrap()
}

fn delivery(endpoint: &str, browser: &Browser, allowed: Value) -> Delivery {
    serde_json::from_value(json!({
        "id": "0199abcd-1234-7000-8000-0000000000d1", "attempt": 1,
        "config": { "values": {}, "secrets": {}, "allowed_private_hosts": allowed },
        "to": { "id": "0199abcd-1234-7000-8000-0000000000e1", "endpoint": endpoint,
                "keys": { "p256dh": browser.p256dh, "auth": URL_SAFE_NO_PAD.encode(browser.auth) } },
        "recipient": null, "language": "en", "subject": "Approve the deploy?", "body": "Fix login",
        "notifications": [ { "id": "0199abcd-1234-7000-8000-000000000001", "kind": "approval.requested",
            "at": "2026-10-11T10:00:00.000Z", "title": "Approve the deploy?", "body": "Fix login",
            "url": "https://orch.example/thread/9", "node": null, "thread_id": null, "chat_id": null,
            "data": {}, "recipient": null } ]
    }))
    .unwrap()
}

const SCHEDULE: [Duration; 1] = [Duration::from_secs(60)];

#[tokio::test(flavor = "multi_thread")]
async fn a_push_is_encrypted_to_the_browser_signed_by_the_system_and_says_what_the_inbox_row_says()
{
    let (port, log) = push_service().await;
    let vapid = the_systems_key();
    let public = URL_SAFE_NO_PAD.decode(vapid.public_key()).unwrap();
    let notifier = PushNotifier::new(vapid);
    let browser = browser();
    let endpoint = format!("http://127.0.0.1:{port}/push/ok");
    notifier
        .deliver(delivery(&endpoint, &browser, json!(["127.0.0.1"])))
        .await
        .unwrap();

    let seen = log.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    let (headers, body) = (&seen[0].headers, &seen[0].body);
    assert_eq!(headers["content-encoding"], "aes128gcm");
    assert_eq!(headers["ttl"], "86400");
    assert_eq!(
        headers["topic"], "0199abcd123470008000000000000001",
        "a repeat of this notification replaces it"
    );

    // What the browser reads.
    let plain = ece::decrypt(&browser.components, &browser.auth, body).unwrap();
    let content: Value = serde_json::from_slice(&plain).unwrap();
    assert_eq!(content["title"], "Approve the deploy?");
    assert_eq!(content["body"], "Fix login");
    assert_eq!(content["url"], "https://orch.example/thread/9");
    assert_eq!(content["kind"], "approval.requested");
    assert_eq!(content["count"], 1);

    // Who sent it: the system's key signed a token for this push service.
    let authorization = headers["authorization"].to_str().unwrap();
    let rest = authorization
        .strip_prefix("vapid ")
        .expect("a VAPID authorization");
    let field = |name: &str| {
        rest.split(", ")
            .find_map(|part| part.strip_prefix(&format!("{name}=")))
            .unwrap_or_else(|| panic!("{name} in {authorization}"))
            .to_owned()
    };
    assert_eq!(field("k"), URL_SAFE_NO_PAD.encode(&public));
    let token = field("t");
    let parts: Vec<&str> = token.split('.').collect();
    assert_eq!(parts.len(), 3);
    let claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
    assert_eq!(claims["aud"], format!("http://127.0.0.1:{port}"));
    assert_eq!(claims["sub"], "https://orch.example");
    assert!(
        claims["exp"].as_u64().unwrap() > chrono_now() + 3_000,
        "the token is good for a while"
    );
    let signature = Signature::from_slice(&URL_SAFE_NO_PAD.decode(parts[2]).unwrap()).unwrap();
    VerifyingKey::from_sec1_bytes(&public)
        .unwrap()
        .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature)
        .expect("the token is signed with the system's key");
}

fn chrono_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_subscription_the_push_service_no_longer_knows_is_gone_and_a_service_that_is_slow_or_down_is_tried_again()
 {
    let (port, _) = push_service().await;
    let notifier = PushNotifier::new(the_systems_key());
    let browser = browser();
    let post = |path: &str| {
        let endpoint = format!("http://127.0.0.1:{port}/push/{path}");
        let delivery = delivery(&endpoint, &browser, json!(["127.0.0.1"]));
        let notifier = notifier.clone();
        async move { notifier.deliver(delivery).await }
    };
    assert_eq!(after_delivery(&post("ok").await, 1, &SCHEDULE), Next::Taken);
    assert_eq!(
        after_delivery(&post("gone").await, 1, &SCHEDULE),
        Next::Gone,
        "expired or unsubscribed"
    );
    assert_eq!(
        after_delivery(&post("missing").await, 1, &SCHEDULE),
        Next::Gone
    );
    assert_eq!(
        after_delivery(&post("slow").await, 1, &SCHEDULE),
        Next::RetryAfter(Duration::from_secs(30))
    );
    assert_eq!(
        after_delivery(&post("boom").await, 1, &SCHEDULE),
        Next::RetryAfter(Duration::from_secs(60))
    );
    let denied = post("denied").await;
    assert_eq!(after_delivery(&denied, 1, &SCHEDULE), Next::Failed);
    assert_eq!(denied.unwrap_err().code, "vapid_rejected");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_endpoint_on_a_private_network_is_refused_even_for_the_systems_own_channel_unless_listed()
 {
    let (port, log) = push_service().await;
    let notifier = PushNotifier::new(the_systems_key());
    let browser = browser();
    let endpoint = format!("http://127.0.0.1:{port}/push/ok");
    for held in [Value::Null, json!([]), json!(["10.0.0.0/8"])] {
        let error = notifier
            .deliver(delivery(&endpoint, &browser, held.clone()))
            .await
            .unwrap_err();
        assert_eq!(
            (error.code.as_str(), error.retryable, error.gone),
            ("address_not_allowed", false, false),
            "{held}"
        );
    }
    assert!(
        log.lock().unwrap().is_empty(),
        "the push service saw nothing"
    );
    notifier
        .deliver(delivery(&endpoint, &browser, json!(["127.0.0.0/8"])))
        .await
        .unwrap();
    assert_eq!(log.lock().unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn what_is_stored_of_a_browser_that_can_never_work_makes_the_browser_gone() {
    let (port, log) = push_service().await;
    let notifier = PushNotifier::new(the_systems_key());
    let good = browser();
    let endpoint = format!("http://127.0.0.1:{port}/push/ok");
    let mut keys_broken = delivery(&endpoint, &good, json!(["127.0.0.1"]));
    keys_broken.to["keys"]["p256dh"] = json!("garbage");
    let error = notifier.deliver(keys_broken).await.unwrap_err();
    assert!(error.gone, "{error}");
    let mut no_endpoint = delivery(&endpoint, &good, json!(["127.0.0.1"]));
    no_endpoint.to = json!({});
    assert!(notifier.deliver(no_endpoint).await.unwrap_err().gone);
    let mut odd = delivery("ftp://x.example/y", &good, Value::Null);
    odd.config.allowed_private_hosts = Some(vec![]);
    assert!(notifier.deliver(odd).await.unwrap_err().gone);
    assert!(log.lock().unwrap().is_empty());
    let _ = &log.lock().unwrap().first().map(|each| each.path.clone());
}
