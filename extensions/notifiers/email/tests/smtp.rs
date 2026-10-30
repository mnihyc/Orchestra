//! Mail through a real SMTP server in every combination of security and sign-in, and what a server's refusals come to.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support {
    pub mod mailpit;
}

use orch_notifier_contract::delivery::{Delivery, Next, Saved, after_delivery, on_check};
use orch_notifier_contract::notifier::Notifier;
use orch_notifier_email::EmailNotifier;
use serde_json::{Value, json};
use std::time::Duration;
use support::mailpit::{LOGIN, Mailpit, Wire};

fn security(wire: Wire) -> &'static str {
    match wire {
        Wire::Plain => "none",
        Wire::StartTls => "starttls",
        Wire::Tls => "tls",
    }
}

/// The values of a channel that reaches `server`.
fn values(server: &Mailpit, wire: Wire, auth: &str, signed_in: bool) -> Value {
    let mut values = json!({
        "host": "localhost", "port": server.smtp_port.to_string(), "security": security(wire),
        "auth": auth, "from": "Orch <orch@example.org>", "reply_to": "help@example.org",
        "hello_name": "orch.example.org", "timeout_secs": "10"
    });
    if signed_in {
        values["username"] = json!(LOGIN.0);
    }
    values
}

fn secrets(server: &Mailpit, signed_in: bool, with_authority: bool) -> Value {
    let mut secrets = serde_json::Map::new();
    if signed_in {
        secrets.insert("password".into(), json!(LOGIN.1));
    }
    if let (true, Some(pem)) = (with_authority, server.ca_pem()) {
        secrets.insert("tls_ca".into(), json!(pem));
    }
    Value::Object(secrets)
}

fn delivery(id: &str, config: Value, to: &str) -> Delivery {
    serde_json::from_value(json!({
        "id": id, "attempt": 1, "config": config,
        "to": { "email": to },
        "recipient": { "id": "0199abcd-1234-7000-8000-0000000000b1", "kind": "human", "username": "ada",
                       "display_name": "Ada Lovelace", "avatar": { "text": "A", "color": 2 } },
        "language": "en", "subject": "[Acme] Turn 3 finished",
        "body": "Turn 3 finished\nall green & <fast>\nhttps://orch.example/thread/9", "notifications": []
    }))
    .unwrap()
}

fn config(values: Value, secrets: Value, allowed: Value) -> Value {
    json!({ "values": values, "secrets": secrets, "allowed_private_hosts": allowed })
}

/// Mail goes through a server of this wire mode that asks for no sign-in and
/// through one that does, by every way of signing in.
async fn every_way_in(wire: Wire) {
    let open = Mailpit::start(wire, None);
    let closed = Mailpit::start(wire, Some(LOGIN));
    let notifier = EmailNotifier::new();
    let mut expected = Vec::new();

    for (n, auth) in ["none", "auto"].into_iter().enumerate() {
        let id = format!("0199abcd-1234-7000-8000-00000000010{n}");
        let config = config(
            values(&open, wire, auth, false),
            secrets(&open, false, true),
            Value::Null,
        );
        notifier
            .deliver(delivery(&id, config, "ada@example.org"))
            .await
            .unwrap_or_else(|error| panic!("{wire:?} without sign-in, auth {auth}: {error}"));
        expected.push(id);
    }
    let taken = open.wait_for(2).await;
    assert_eq!(taken.len(), 2);

    for (n, auth) in ["auto", "plain", "login"].into_iter().enumerate() {
        let id = format!("0199abcd-1234-7000-8000-00000000020{n}");
        let config = config(
            values(&closed, wire, auth, true),
            secrets(&closed, true, true),
            Value::Null,
        );
        notifier
            .deliver(delivery(&id, config, "ada@example.org"))
            .await
            .unwrap_or_else(|error| panic!("{wire:?} signed in, auth {auth}: {error}"));
        expected.push(id);
    }
    let taken_after_sign_in = closed.wait_for(3).await;
    assert_eq!(
        taken_after_sign_in.len(),
        3,
        "{wire:?}: each way of signing in sent one"
    );

    for message in taken.iter().chain(&taken_after_sign_in) {
        assert_eq!(message.subject, "[Acme] Turn 3 finished");
        assert_eq!(message.from, "orch@example.org");
        assert_eq!(message.to, ["ada@example.org"]);
        assert_eq!(message.reply_to, ["help@example.org"]);
        assert!(
            message.text.contains("Turn 3 finished") && message.text.contains("all green & <fast>")
        );
        assert!(
            message.html.contains("all green &amp; &lt;fast&gt;")
                && message
                    .html
                    .contains("<a href=\"https://orch.example/thread/9\">"),
            "the HTML part is made from the text: {}",
            message.html
        );
    }
    let mut ids: Vec<String> = taken
        .iter()
        .chain(&taken_after_sign_in)
        .map(|message| message.message_id.clone())
        .collect();
    ids.sort();
    let mut wanted: Vec<String> = expected
        .iter()
        .map(|id| format!("<{id}@example.org>"))
        .collect();
    wanted.sort();
    assert_eq!(ids, wanted, "a message is named after its delivery");

    // Looking at either server when its channel is saved connects, secures and
    // signs in, and sends nothing.
    let checked = notifier
        .check(
            &serde_json::from_value(config(
                values(&closed, wire, "auto", true),
                secrets(&closed, true, true),
                Value::Null,
            ))
            .unwrap(),
        )
        .await;
    assert_eq!(on_check(checked), Saved::Kept(Value::Null));
    assert_eq!(closed.messages().await.len(), 3, "a check sends nothing");
}

#[tokio::test(flavor = "multi_thread")]
async fn mail_is_taken_over_a_plain_connection_with_and_without_signing_in() {
    every_way_in(Wire::Plain).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mail_is_taken_after_starttls_with_and_without_signing_in() {
    every_way_in(Wire::StartTls).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mail_is_taken_over_tls_from_the_first_byte_with_and_without_signing_in() {
    every_way_in(Wire::Tls).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_password_refuses_the_save_with_the_servers_own_words_and_fails_a_delivery_at_once()
{
    let server = Mailpit::start(Wire::StartTls, Some(LOGIN));
    let mut bad = secrets(&server, true, true);
    bad["password"] = json!("not-the-password");
    let channel = config(
        values(&server, Wire::StartTls, "auto", true),
        bad,
        Value::Null,
    );
    let checked = EmailNotifier::new()
        .check(&serde_json::from_value(channel.clone()).unwrap())
        .await;
    let Saved::Refused(error) = on_check(checked) else {
        panic!("a wrong password must refuse the save");
    };
    assert_eq!(error.code, "authentication_failed");
    assert!(!error.retryable);
    assert!(
        error.message.contains("535") || error.message.to_lowercase().contains("auth"),
        "{}",
        error.message
    );

    let result = EmailNotifier::new()
        .deliver(delivery(
            "0199abcd-1234-7000-8000-000000000300",
            channel,
            "ada@example.org",
        ))
        .await;
    assert_eq!(
        after_delivery(&result, 1, &[Duration::from_secs(60)]),
        Next::Failed
    );
    assert!(server.messages().await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_with_a_private_authority_is_trusted_only_when_the_channel_names_it() {
    for wire in [Wire::StartTls, Wire::Tls] {
        let server = Mailpit::start(wire, None);
        let notifier = EmailNotifier::new();
        let without = config(
            values(&server, wire, "none", false),
            secrets(&server, false, false),
            Value::Null,
        );
        let checked = notifier
            .check(&serde_json::from_value(without.clone()).unwrap())
            .await;
        let shown = format!("{checked:?}");
        let Saved::Refused(error) = on_check(checked) else {
            panic!("{wire:?}: a certificate nobody trusted must refuse the save, got {shown}");
        };
        assert_eq!(error.code, "tls", "{wire:?}");
        let result = notifier
            .deliver(delivery(
                "0199abcd-1234-7000-8000-000000000400",
                without,
                "ada@example.org",
            ))
            .await;
        assert!(
            matches!(
                after_delivery(&result, 1, &[Duration::from_secs(60)]),
                Next::RetryAfter(_)
            ),
            "{wire:?}: once saved, a certificate that cannot be checked is tried again, not failed: {result:?}"
        );
        assert!(
            server.messages().await.is_empty(),
            "{wire:?}: nothing was sent to a server nobody trusted"
        );

        let with = config(
            values(&server, wire, "none", false),
            secrets(&server, false, true),
            Value::Null,
        );
        notifier
            .deliver(delivery(
                "0199abcd-1234-7000-8000-000000000401",
                with,
                "ada@example.org",
            ))
            .await
            .unwrap();
        assert_eq!(server.wait_for(1).await.len(), 1);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn nothing_is_sent_in_the_clear_when_the_channel_asks_for_starttls_and_the_server_has_none() {
    let server = Mailpit::start(Wire::Plain, Some(LOGIN));
    let channel = config(
        values(&server, Wire::Plain, "auto", true),
        secrets(&server, true, false),
        Value::Null,
    );
    let mut asking = channel.clone();
    asking["values"]["security"] = json!("starttls");
    let result = EmailNotifier::new()
        .deliver(delivery(
            "0199abcd-1234-7000-8000-000000000500",
            asking,
            "ada@example.org",
        ))
        .await;
    let error = result.unwrap_err();
    assert_eq!(error.code, "unsupported", "{error}");
    assert!(!error.retryable);
    assert!(server.messages().await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_channel_of_an_org_reaches_a_private_address_only_where_the_list_names_it() {
    let server = Mailpit::start(Wire::Plain, None);
    let notifier = EmailNotifier::new();
    let make = |allowed: Value| {
        config(
            values(&server, Wire::Plain, "none", false),
            secrets(&server, false, false),
            allowed,
        )
    };

    for refused in [
        json!([]),
        json!(["mail.example.org"]),
        json!(["10.0.0.0/8"]),
    ] {
        let channel = make(refused.clone());
        let checked = notifier
            .check(&serde_json::from_value(channel.clone()).unwrap())
            .await;
        let Saved::Refused(error) = on_check(checked) else {
            panic!("{refused}: loopback is private");
        };
        assert_eq!(error.code, "address_not_allowed");
        let result = notifier
            .deliver(delivery(
                "0199abcd-1234-7000-8000-000000000600",
                channel,
                "ada@example.org",
            ))
            .await;
        assert_eq!(result.unwrap_err().code, "address_not_allowed");
    }
    assert!(server.messages().await.is_empty());

    for listed in [
        json!(["localhost"]),
        json!(["127.0.0.0/8"]),
        json!(["127.0.0.1", "10.0.0.0/8"]),
    ] {
        notifier
            .deliver(delivery(
                "0199abcd-1234-7000-8000-000000000601",
                make(listed.clone()),
                "ada@example.org",
            ))
            .await
            .unwrap_or_else(|error| panic!("{listed}: {error}"));
    }
    assert_eq!(server.wait_for(3).await.len(), 3);
    // The system's own channels are not held to the list.
    notifier
        .deliver(delivery(
            "0199abcd-1234-7000-8000-000000000602",
            make(Value::Null),
            "ada@example.org",
        ))
        .await
        .unwrap();
}

mod scripted {
    //! A scripted SMTP server that answers each command as it is told to.
    use std::sync::Arc;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    /// What it answers.
    #[derive(Clone)]
    pub struct Script {
        pub banner: &'static str,
        pub mail: &'static str,
        pub rcpt: &'static str,
        pub data_end: &'static str,
    }

    pub const FINE: Script = Script {
        banner: "220 scripted ESMTP",
        mail: "250 2.1.0 ok",
        rcpt: "250 2.1.5 ok",
        data_end: "250 2.0.0 queued",
    };

    /// Listens on a port from 41000 to 41999 and answers by the script.
    pub async fn serve(script: Script) -> u16 {
        let seed = u64::from(std::process::id()).wrapping_mul(2_654_435_761);
        for attempt in 0..400_u64 {
            let start = 41_000 + u16::try_from(seed.wrapping_add(attempt * 7_919) % 1_000).unwrap();
            let Ok(listener) = TcpListener::bind(("127.0.0.1", start)).await else {
                continue;
            };
            let script = Arc::new(script);
            tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        return;
                    };
                    let script = Arc::clone(&script);
                    tokio::spawn(async move {
                        let (read, mut write) = stream.into_split();
                        let mut lines = BufReader::new(read).lines();
                        if script.banner.is_empty() {
                            // A server that never says anything.
                            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                            return;
                        }
                        let _ = write
                            .write_all(format!("{}\r\n", script.banner).as_bytes())
                            .await;
                        if !script.banner.starts_with("220") {
                            return;
                        }
                        let mut in_data = false;
                        while let Ok(Some(line)) = lines.next_line().await {
                            let upper = line.to_ascii_uppercase();
                            let reply = if in_data {
                                if line == "." {
                                    in_data = false;
                                    script.data_end.to_owned()
                                } else {
                                    continue;
                                }
                            } else if upper.starts_with("EHLO") {
                                "250-scripted\r\n250 8BITMIME".to_owned()
                            } else if upper.starts_with("MAIL") {
                                script.mail.to_owned()
                            } else if upper.starts_with("RCPT") {
                                script.rcpt.to_owned()
                            } else if upper.starts_with("DATA") {
                                in_data = true;
                                "354 go ahead".to_owned()
                            } else if upper.starts_with("QUIT") {
                                let _ = write.write_all(b"221 bye\r\n").await;
                                return;
                            } else {
                                "250 ok".to_owned()
                            };
                            if write
                                .write_all(format!("{reply}\r\n").as_bytes())
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }
                    });
                }
            });
            return start;
        }
        panic!("no free port from 41000 to 41999");
    }
}

fn plain_channel(port: u16, timeout: &str) -> Value {
    config(
        json!({ "host": "127.0.0.1", "port": port.to_string(), "security": "none", "auth": "none",
                "from": "orch@example.org", "timeout_secs": timeout }),
        json!({}),
        Value::Null,
    )
}

async fn after(
    script: scripted::Script,
) -> (
    Result<(), orch_notifier_contract::delivery::DeliverError>,
    Next,
) {
    let port = scripted::serve(script).await;
    let result = EmailNotifier::new()
        .deliver(delivery(
            "0199abcd-1234-7000-8000-000000000700",
            plain_channel(port, "5"),
            "ada@example.org",
        ))
        .await;
    let next = after_delivery(&result, 1, &[Duration::from_secs(60)]);
    (result, next)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_takes_the_message_ends_the_delivery() {
    let (result, next) = after(scripted::FINE).await;
    assert_eq!(result, Ok(()));
    assert_eq!(next, Next::Taken);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_mailbox_the_server_says_does_not_exist_is_a_recipient_that_is_gone() {
    let (result, next) = after(scripted::Script {
        rcpt: "550 5.1.1 <ada@example.org>: Recipient address rejected: User unknown",
        ..scripted::FINE
    })
    .await;
    assert_eq!(next, Next::Gone, "{result:?}");
    let error = result.unwrap_err();
    assert!(
        error.message.contains("User unknown"),
        "the server's own words are kept: {}",
        error.message
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_about_the_sender_or_the_message_fails_the_delivery_without_ending_the_route() {
    for script in [
        scripted::Script {
            mail: "550 5.7.1 sender not allowed",
            ..scripted::FINE
        },
        scripted::Script {
            rcpt: "554 5.7.1 relay access denied",
            ..scripted::FINE
        },
        scripted::Script {
            data_end: "554 5.6.0 message rejected as spam",
            ..scripted::FINE
        },
        scripted::Script {
            rcpt: "550 no such user here",
            ..scripted::FINE
        },
    ] {
        let (result, next) = after(script).await;
        assert_eq!(next, Next::Failed, "{result:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_asks_to_wait_or_is_down_is_tried_again() {
    for script in [
        scripted::Script {
            rcpt: "450 4.2.0 greylisted, try again later",
            ..scripted::FINE
        },
        scripted::Script {
            banner: "421 4.3.2 service shutting down",
            ..scripted::FINE
        },
        scripted::Script {
            data_end: "452 4.3.1 insufficient system storage",
            ..scripted::FINE
        },
    ] {
        let (result, next) = after(script).await;
        assert!(matches!(next, Next::RetryAfter(_)), "{result:?}");
    }
    // Nobody is listening.
    let closed = (41_000..42_000_u16)
        .rev()
        .find(|port| std::net::TcpListener::bind(("127.0.0.1", *port)).is_ok())
        .unwrap();
    let result = EmailNotifier::new()
        .deliver(delivery(
            "0199abcd-1234-7000-8000-000000000701",
            plain_channel(closed, "5"),
            "ada@example.org",
        ))
        .await;
    assert!(
        matches!(
            after_delivery(&result, 1, &[Duration::from_secs(60)]),
            Next::RetryAfter(_)
        ),
        "{result:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_says_nothing_is_given_up_on_after_the_time_the_channel_allows() {
    let port = scripted::serve(scripted::Script {
        banner: "",
        ..scripted::FINE
    })
    .await;
    let started = std::time::Instant::now();
    let result = EmailNotifier::new()
        .deliver(delivery(
            "0199abcd-1234-7000-8000-000000000702",
            plain_channel(port, "1"),
            "ada@example.org",
        ))
        .await;
    let error = result.unwrap_err();
    assert_eq!((error.code.as_str(), error.retryable), ("timeout", true));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}
