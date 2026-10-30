//! The mail that signing in writes is delivered through the system's mail server, in the language of the person, and what is secret in the row is blanked.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use chrono::Duration;
use orch_api_core::sign_in::mail::{Addressee, Message, payload};
use orch_db::queries::identity::{self, IssueCode, IssueLink, NewEmailToken};
use orch_db::queries::notifications::channels::ChannelRow;
use orch_notifier_email::EmailNotifier;
use orch_worker_core::channels::Notifiers;
use serde_json::json;
use std::sync::Arc;
use support::mailpit::{LOGIN, Mailpit, Wire};
use support::world::World;

/// The configuration of the system these rows are written under: the same name
/// the worker writes with.
fn the_systems_config() -> orch_config::ServerConfig {
    let mut config = orch_config::ServerConfig::default();
    config.system.name = "Acme Orch".to_owned();
    config.system.public_url = "https://orch.example".to_owned();
    config
}

fn mail_notifiers() -> Notifiers {
    let mut notifiers = Notifiers::new();
    notifiers.register(Arc::new(EmailNotifier::new())).unwrap();
    notifiers
}

/// The system's mail server, which is `server`, with its secrets sealed.
async fn the_systems_mail_server(world: &World, server: &Mailpit) -> ChannelRow {
    world
        .channel(
            support::world::Of::System,
            "email",
            json!({
                "host": "localhost", "port": server.smtp_port.to_string(), "security": "starttls",
                "auth": "auto", "username": LOGIN.0, "from": "Acme Orch <orch@example.org>", "timeout_secs": "10"
            }),
            &[("password", LOGIN.1), ("tls_ca", server.ca_pem().unwrap())],
        )
        .await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_code_written_by_signing_in_is_delivered_to_the_smtp_server_and_blanked_in_the_row() {
    let server = Mailpit::start(Wire::StartTls, Some(LOGIN));
    let world = World::new(mail_notifiers()).await;
    let channel = the_systems_mail_server(&world, &server).await;
    let ada = world.person("ada").await;
    let config = the_systems_config();
    let to = Addressee {
        user_id: ada,
        email: "ada@example.org",
        locale: "en",
    };
    let expires = world.now() + Duration::minutes(10);
    let row = payload(
        &config,
        &to,
        &Message::SignInCode {
            code: "ABCD-1234",
            expires_at: expires,
        },
    );
    {
        let mut conn = world.test.pool.acquire().await.unwrap();
        let written = identity::issue_code(
            &mut conn,
            &IssueCode {
                user_id: ada,
                code_hash: &[7; 32],
                email: "ada@example.org",
                at: world.now(),
                expires_at: expires,
                channel: channel.id,
                payload: &row,
            },
            true,
        )
        .await
        .unwrap();
        assert!(written, "signing in wrote the row");
    }
    assert_eq!(world.rows().await.len(), 1);

    let done = world.outbox().run().await.unwrap();
    assert_eq!((done.sent, done.total()), (1, 1));

    let mail = server.wait_for(1).await;
    assert_eq!(mail.len(), 1);
    assert_eq!(mail[0].subject, "Your code for Acme Orch: ABCD-1234");
    assert_eq!(mail[0].to, ["ada@example.org"]);
    assert_eq!(mail[0].from, "orch@example.org");
    assert!(mail[0].text.contains("ABCD-1234"), "{}", mail[0].text);
    assert!(mail[0].html.contains("ABCD-1234"));

    let rows = world.rows().await;
    assert_eq!((rows[0].state.as_str(), rows[0].attempts), ("sent", 1));
    let kept = rows[0].payload.to_string();
    assert!(
        !kept.contains("ABCD-1234"),
        "the code is not kept in the row: {kept}"
    );
    assert_eq!(rows[0].payload["purpose"], "sign_in_code");
    assert_eq!(rows[0].payload["data"]["code"], "");
    let channel = world.channel_now(channel.id).await;
    assert!(channel.last_ok_at.is_some() && channel.last_error.is_none());

    assert_eq!(
        world.outbox().run().await.unwrap().total(),
        0,
        "a row that is sent is not sent again"
    );
    assert_eq!(server.messages().await.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn each_message_signing_in_writes_is_delivered_in_the_language_of_the_person() {
    let server = Mailpit::start(Wire::Tls, Some(LOGIN));
    let world = World::new(mail_notifiers()).await;
    let mut tls = the_systems_mail_server(&world, &server).await;
    // A server with TLS from the first byte.
    sqlx::query("UPDATE notification_channels SET config = jsonb_set(config, '{security}', '\"tls\"') WHERE id = $1")
        .bind(tls.id)
        .execute(&world.test.pool)
        .await
        .unwrap();
    tls = world.channel_now(tls.id).await;
    let (bo, ada) = (
        world
            .person_with("bob", "zh-CN", "UTC", Some("bob@example.org"))
            .await,
        world.person("ada").await,
    );
    let config = the_systems_config();
    let expires = world.now() + Duration::hours(24);
    let mut conn = world.test.pool.acquire().await.unwrap();
    let token = |n: u8| NewEmailToken {
        id: orch_types::new_id(),
        user_id: bo,
        purpose: "verify_email",
        token_hash: Box::leak(Box::new([n; 32])),
        email: "bob@example.org",
        clear_second_step: false,
        at: world.now(),
        expires_at: expires,
    };
    let verify = payload(
        &config,
        &Addressee {
            user_id: bo,
            email: "bob@example.org",
            locale: "zh-CN",
        },
        &Message::VerifyEmail {
            token: "oem_secret_one",
            expires_at: expires,
        },
    );
    identity::issue_link(
        &mut conn,
        &IssueLink {
            token: token(1),
            channel: tls.id,
            payload: &verify,
        },
        true,
    )
    .await
    .unwrap();
    let notice = payload(
        &config,
        &Addressee {
            user_id: ada,
            email: "ada@example.org",
            locale: "en",
        },
        &Message::AddressChanged {
            new_email: "new@example.org",
        },
    );
    identity::queue_mail(&mut conn, "email", tls.id, &notice, world.now())
        .await
        .unwrap();
    drop(conn);

    let done = world.outbox().run().await.unwrap();
    assert_eq!(done.sent, 2);
    let mail = server.wait_for(2).await;
    let to_bo = mail
        .iter()
        .find(|each| each.to == ["bob@example.org"])
        .unwrap();
    let to_ada = mail
        .iter()
        .find(|each| each.to == ["ada@example.org"])
        .unwrap();
    assert!(to_bo.subject.contains("确认"), "{}", to_bo.subject);
    assert!(
        to_bo
            .text
            .contains("https://orch.example/verify-email/oem_secret_one"),
        "{}",
        to_bo.text
    );
    assert!(to_bo.text.contains("链接只能使用一次"));
    assert!(
        to_ada
            .subject
            .starts_with("The address of your Acme Orch account"),
        "{}",
        to_ada.subject
    );
    for row in world.rows().await {
        assert_eq!(row.state, "sent");
        let kept = row.payload.to_string();
        assert!(!kept.contains("oem_secret_one"), "{kept}");
        assert_eq!(row.payload["blanked"], true);
    }
}
