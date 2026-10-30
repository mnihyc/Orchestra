//! `orchctl channel`: a channel of the system made from a file of its fields with its secrets sealed as they are stored, and a sample sent through it whose answer is the far side's own.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::any;
use orch_db::secrets::MasterKey;
use orch_notifier_webhook::signature;
use orch_types::{Id, Json};
use orch_worker_core::channels::secret_binding;
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use support::smtp::{NO_SUCH_MAILBOX, Smtp};
use support::{Deployment, admin, person};
use tokio::net::TcpListener;

/// The master key the deployment's secrets directory holds.
fn master() -> MasterKey {
    MasterKey::new([7; 32])
}

const HOOK_SECRET: &str = "hook-signing-secret";
const MAIL_PASSWORD: &str = "mail-password-value";

/// A request a stand-in receiver saw.
#[derive(Debug, Clone)]
struct Seen {
    path: String,
    headers: HeaderMap,
    body: Vec<u8>,
}

/// A place a webhook posts to, which takes everything and remembers it.
struct Receiver {
    url: String,
    seen: Arc<Mutex<Vec<Seen>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Receiver {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn receiver() -> Receiver {
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let router = Router::new()
        .fallback(any(
            |State(seen): State<Arc<Mutex<Vec<Seen>>>>,
             uri: axum::http::Uri,
             headers: HeaderMap,
             body: Bytes| async move {
                seen.lock().unwrap().push(Seen {
                    path: uri.path().to_owned(),
                    headers,
                    body: body.to_vec(),
                });
                StatusCode::NO_CONTENT
            },
        ))
        .with_state(Arc::clone(&seen));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Receiver { url, seen, task }
}

fn write(world: &Deployment, name: &str, fields: &Json) -> PathBuf {
    let path = world.dir().join(name);
    std::fs::write(&path, fields.to_string()).unwrap();
    path
}

async fn channels(world: &Deployment) -> Vec<(Id, String, String, Json, Option<String>, Id)> {
    sqlx::query_as(
        "SELECT id, kind, name, config, last_error, created_by FROM notification_channels
          WHERE org_id IS NULL AND user_id IS NULL ORDER BY created_at, id",
    )
    .fetch_all(world.pool())
    .await
    .unwrap()
}

async fn secrets(world: &Deployment) -> Vec<(String, Vec<u8>, Id)> {
    sqlx::query_as(
        "SELECT name, sealed, owner_id FROM secrets WHERE owner_kind = 'notification_channel' ORDER BY name",
    )
    .fetch_all(world.pool())
    .await
    .unwrap()
}

/// Whether `needle` is in any row that a person with the database could read.
async fn readable_anywhere(world: &Deployment, needle: &str) -> bool {
    let found: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM notification_channels WHERE notification_channels::text LIKE '%' || $1 || '%')
              + (SELECT count(*) FROM audit_log WHERE audit_log::text LIKE '%' || $1 || '%')
              + (SELECT count(*) FROM outbox WHERE outbox::text LIKE '%' || $1 || '%')
              + (SELECT count(*) FROM secrets WHERE secrets::text LIKE '%' || $1 || '%')",
    )
    .bind(needle)
    .fetch_one(world.pool())
    .await
    .unwrap();
    found > 0
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_webhook_is_put_from_a_file_with_its_secret_sealed_and_a_sample_to_it_is_signed_by_that_secret()
 {
    let world = Deployment::new().await;
    let boss = admin(&world.db(), "boss").await;
    let hook = receiver().await;
    let file = write(
        &world,
        "hook.json",
        &json!({ "url": format!("{}/build", hook.url), "secret": HOOK_SECRET }),
    );

    let put = world
        .run(&[
            "channel",
            "put",
            "webhook",
            "--file",
            file.to_str().unwrap(),
            "--name",
            "Build room",
        ])
        .await;

    let said = put.succeeded();
    assert!(
        said.contains("made the webhook channel \"Build room\"") && said.contains("secret"),
        "{said}"
    );
    let made = channels(&world).await;
    assert_eq!(made.len(), 1);
    let (id, kind, name, config, last_error, created_by) = &made[0];
    assert_eq!((kind.as_str(), name.as_str()), ("webhook", "Build room"));
    assert_eq!(config, &json!({ "url": format!("{}/build", hook.url) }));
    assert_eq!(*last_error, None);
    assert_eq!(*created_by, boss, "set up by the first system admin");
    let stored = secrets(&world).await;
    assert_eq!(stored.len(), 1);
    assert_eq!((stored[0].0.as_str(), stored[0].2), ("secret", *id));
    assert!(
        !contains(&stored[0].1, HOOK_SECRET.as_bytes()),
        "the value is not in the row"
    );
    assert!(
        !readable_anywhere(&world, HOOK_SECRET).await,
        "the secret is in no row of the database in the clear"
    );
    assert_eq!(
        master()
            .open(
                &stored[0].1,
                &secret_binding("notification_channel", *id, "secret")
            )
            .unwrap(),
        HOOK_SECRET.as_bytes(),
        "and it opens with the master key, for its own channel"
    );
    let trail: Vec<(String, String)> =
        sqlx::query_as("SELECT action, actor_kind FROM audit_log ORDER BY at, id")
            .fetch_all(world.pool())
            .await
            .unwrap();
    assert_eq!(trail, [("channel.created".to_owned(), "system".to_owned())]);

    let test = world.run(&["channel", "test", "webhook"]).await;

    assert!(
        test.succeeded().contains("the far side took it"),
        "{}",
        test.out
    );
    let seen = hook.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].path, "/build");
    let sent = seen[0]
        .headers
        .get(signature::HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap();
    assert!(
        signature::verify(HOOK_SECRET.as_bytes(), &seen[0].body, sent),
        "the secret that was sealed is the one that signs"
    );
    let body: Json = serde_json::from_slice(&seen[0].body).unwrap();
    assert_eq!(body["title"], "Test message from Test Orch");
    assert_eq!(
        body["recipient"],
        Json::Null,
        "a sample is for nobody in particular"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn putting_a_channel_again_replaces_it_and_its_secrets_instead_of_adding_one() {
    let world = Deployment::new().await;
    admin(&world.db(), "boss").await;
    let first = receiver().await;
    let second = receiver().await;
    let signed = write(
        &world,
        "signed.json",
        &json!({ "url": first.url, "secret": HOOK_SECRET }),
    );
    let plain = write(&world, "plain.json", &json!({ "url": second.url }));
    world
        .run(&[
            "channel",
            "put",
            "webhook",
            "--file",
            signed.to_str().unwrap(),
        ])
        .await
        .succeeded();
    let id = channels(&world).await[0].0;

    let again = world
        .run(&[
            "channel",
            "put",
            "webhook",
            "--file",
            plain.to_str().unwrap(),
        ])
        .await;

    assert!(
        again
            .succeeded()
            .starts_with("replaced the webhook channel")
    );
    let now = channels(&world).await;
    assert_eq!(now.len(), 1, "one channel, replaced");
    assert_eq!(now[0].0, id);
    assert_eq!(now[0].3, json!({ "url": second.url }));
    assert!(
        secrets(&world).await.is_empty(),
        "a secret the file leaves out is removed"
    );
    world.run(&["channel", "test", "webhook"]).await.succeeded();
    assert!(first.seen.lock().unwrap().is_empty());
    let seen = second.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert!(
        seen[0].headers.get(signature::HEADER).is_none(),
        "and nothing signs now"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn several_webhooks_are_told_apart_by_name() {
    let world = Deployment::new().await;
    admin(&world.db(), "boss").await;
    let alpha = receiver().await;
    let beta = receiver().await;
    for (name, hook) in [("alpha", &alpha), ("beta", &beta)] {
        let file = write(&world, &format!("{name}.json"), &json!({ "url": hook.url }));
        world
            .run(&[
                "channel",
                "put",
                "webhook",
                "--file",
                file.to_str().unwrap(),
                "--name",
                name,
            ])
            .await
            .succeeded();
    }

    let which = world.run(&["channel", "test", "webhook"]).await;
    which.refused_with("several webhook channels (alpha, beta): say which with --name");
    let named = world
        .run(&["channel", "test", "webhook", "--name", "beta"])
        .await;

    named.succeeded();
    assert!(alpha.seen.lock().unwrap().is_empty());
    assert_eq!(beta.seen.lock().unwrap().len(), 1);
    let file = write(&world, "again.json", &json!({ "url": beta.url }));
    world
        .run(&[
            "channel",
            "put",
            "webhook",
            "--file",
            file.to_str().unwrap(),
        ])
        .await
        .refused_with("say which with --name");
    assert_eq!(channels(&world).await.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_mail_server_is_put_with_its_password_sealed_and_a_sample_goes_through_it() {
    let world = Deployment::new().await;
    admin(&world.db(), "boss").await;
    let smtp = Smtp::start(Some(MAIL_PASSWORD)).await;
    let file = write(
        &world,
        "mail.json",
        &json!({
            "host": "127.0.0.1", "port": smtp.port.to_string(), "security": "none", "auth": "plain",
            "username": support::smtp::USER, "password": MAIL_PASSWORD, "from": "orch@example.org"
        }),
    );

    let put = world
        .run(&["channel", "put", "email", "--file", file.to_str().unwrap()])
        .await;

    assert!(
        put.succeeded().contains("made the email channel"),
        "{}",
        put.err
    );
    assert!(
        !readable_anywhere(&world, MAIL_PASSWORD).await,
        "the password of a mail server is in no row in the clear"
    );
    let stored = secrets(&world).await;
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].0, "password");
    let id = channels(&world).await[0].0;
    assert_eq!(
        master()
            .open(
                &stored[0].1,
                &secret_binding("notification_channel", id, "password")
            )
            .unwrap(),
        MAIL_PASSWORD.as_bytes()
    );

    let test = world
        .run(&["channel", "test", "email", "--to", "ops@example.org"])
        .await;

    assert!(
        test.succeeded()
            .contains("to ops@example.org: the far side took it"),
        "{}",
        test.out
    );
    let told = smtp.told();
    assert!(
        told.iter()
            .any(|line| line.contains("MAIL FROM:<orch@example.org>")),
        "{told:?}"
    );
    assert!(
        told.iter()
            .any(|line| line.contains("RCPT TO:<ops@example.org>")),
        "{told:?}"
    );
    assert!(
        told.iter()
            .any(|line| line.starts_with("DATA:") && line.contains("Test message from Test Orch")),
        "{told:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn what_the_mail_server_answers_is_printed_when_it_does_not_take_the_sample() {
    let world = Deployment::new().await;
    admin(&world.db(), "boss").await;
    let smtp = Smtp::start(None).await;
    let file = write(
        &world,
        "mail.json",
        &json!({ "host": "127.0.0.1", "port": smtp.port.to_string(), "security": "none", "auth": "none", "from": "orch@example.org" }),
    );
    world
        .run(&["channel", "put", "email", "--file", file.to_str().unwrap()])
        .await
        .succeeded();

    let refused = world
        .run(&["channel", "test", "email", "--to", "nobody@example.org"])
        .await;

    let line = refused.refused_with("the far side did not take the sample");
    assert!(
        line.contains("550") && line.contains("5.1.1 Recipient address rejected: User unknown"),
        "the server's own words: {line}"
    );
    assert!(NO_SUCH_MAILBOX.contains("User unknown"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_password_refuses_the_save_and_leaves_nothing_behind() {
    let world = Deployment::new().await;
    admin(&world.db(), "boss").await;
    let smtp = Smtp::start(Some(MAIL_PASSWORD)).await;
    let file = write(
        &world,
        "mail.json",
        &json!({
            "host": "127.0.0.1", "port": smtp.port.to_string(), "security": "none", "auth": "plain",
            "username": support::smtp::USER, "password": "not-the-password", "from": "orch@example.org"
        }),
    );

    let put = world
        .run(&["channel", "put", "email", "--file", file.to_str().unwrap()])
        .await;

    let line = put.refused_with("nothing was saved");
    assert!(
        line.contains("authentication"),
        "what the server said: {line}"
    );
    assert!(!line.contains("not-the-password"));
    assert!(channels(&world).await.is_empty());
    assert!(secrets(&world).await.is_empty());
    assert!(
        !readable_anywhere(&world, "not-the-password").await,
        "nor is the password anywhere"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_mail_server_that_is_down_is_kept_with_its_error_and_the_person_is_told() {
    let world = Deployment::new().await;
    admin(&world.db(), "boss").await;
    // A port nothing listens on: the one a closed listener held.
    let closed = {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap().port()
    };
    let file = write(
        &world,
        "mail.json",
        &json!({ "host": "127.0.0.1", "port": closed.to_string(), "security": "none", "auth": "none", "from": "orch@example.org", "timeout_secs": "2" }),
    );

    let put = world
        .run(&["channel", "put", "email", "--file", file.to_str().unwrap()])
        .await;

    assert!(put.succeeded().contains("made the email channel"));
    assert!(put.err.contains("could not be reached"), "{}", put.err);
    let made = channels(&world).await;
    assert_eq!(made.len(), 1);
    assert!(
        made[0].4.is_some(),
        "the channel carries why: {:?}",
        made[0].4
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_put_that_cannot_be_done_is_refused_in_one_line_that_says_what_to_do() {
    let world = Deployment::new().await;
    let hook = receiver().await;
    let file = write(&world, "hook.json", &json!({ "url": hook.url }));
    let path = file.to_str().unwrap();

    // Nobody to set it up yet.
    world
        .run(&["channel", "put", "webhook", "--file", path])
        .await
        .refused_with("no administrator yet");
    admin(&world.db(), "boss").await;

    let kinds = world
        .run(&["channel", "put", "carrier-pigeon", "--file", path])
        .await;
    let line = kinds.refused_with("carrier-pigeon is no kind of channel");
    assert!(line.contains("email") && line.contains("webhook"), "{line}");
    world
        .run(&[
            "channel",
            "put",
            "webhook",
            "--file",
            "/no/such/fields.json",
        ])
        .await
        .refused_with("cannot read /no/such/fields.json");
    let broken = world.dir().join("broken.json");
    std::fs::write(&broken, "{ not json").unwrap();
    world
        .run(&[
            "channel",
            "put",
            "webhook",
            "--file",
            broken.to_str().unwrap(),
        ])
        .await
        .refused_with("is not JSON");
    let list = world.dir().join("list.json");
    std::fs::write(&list, "[1, 2]").unwrap();
    world
        .run(&[
            "channel",
            "put",
            "webhook",
            "--file",
            list.to_str().unwrap(),
        ])
        .await
        .refused_with("not an object of fields");
    let missing = write(&world, "empty.json", &json!({}));
    let incomplete = world
        .run(&[
            "channel",
            "put",
            "email",
            "--file",
            missing.to_str().unwrap(),
        ])
        .await;
    let line = incomplete.refused_with("the fields do not fit email");
    assert!(line.contains("host"), "it names the field: {line}");
    world
        .run(&["channel", "put", "webhook", "--file", path, "--name", ""])
        .await
        .refused_with("a name is 1 to 80 characters");
    assert!(channels(&world).await.is_empty(), "nothing was made");

    let mut no_key = world.command();
    no_key.env("ORCH_SECRETS_DIR", "/no/such/secrets");
    support::run(no_key, &["channel", "put", "webhook", "--file", path])
        .await
        .refused_with("master key");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_directory_of_the_secrets_is_the_one_given_even_after_the_command() {
    let world = Deployment::new().await;
    admin(&world.db(), "boss").await;
    let hook = receiver().await;
    let file = write(
        &world,
        "hook.json",
        &json!({ "url": hook.url, "secret": HOOK_SECRET }),
    );
    // Another directory holds another master key than the deployment's own.
    let other = world.dir().join("other-secrets");
    std::fs::create_dir(&other).unwrap();
    std::fs::write(
        other.join("orch_master_key"),
        format!("{}\n", "0a".repeat(32)),
    )
    .unwrap();
    let mut command = world.command();
    command.env_remove("ORCH_SECRETS_DIR");

    let put = support::run(
        command,
        &[
            "channel",
            "put",
            "webhook",
            "--file",
            file.to_str().unwrap(),
            "--secrets-dir",
            other.to_str().unwrap(),
        ],
    )
    .await;

    put.succeeded();
    let sealed = secrets(&world).await;
    let id = channels(&world).await[0].0;
    let binding = secret_binding("notification_channel", id, "secret");
    assert!(
        master().open(&sealed[0].1, &binding).is_err(),
        "it is not sealed under the key of the deployment's own directory"
    );
    assert_eq!(
        MasterKey::new([0x0a; 32])
            .open(&sealed[0].1, &binding)
            .unwrap(),
        HOOK_SECRET.as_bytes(),
        "it is sealed under the key of the directory that was given"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sample_that_cannot_be_sent_from_the_host_says_why() {
    let world = Deployment::new().await;
    admin(&world.db(), "boss").await;
    let hook = receiver().await;
    let file = write(&world, "hook.json", &json!({ "url": hook.url }));
    person(&world.db(), "ada").await;

    world
        .run(&["channel", "test", "email", "--to", "ops@example.org"])
        .await
        .refused_with("the system has no email channel");
    world
        .run(&["channel", "test", "webhook"])
        .await
        .refused_with("the system has no webhook channel");
    world
        .run(&[
            "channel",
            "put",
            "webhook",
            "--file",
            file.to_str().unwrap(),
        ])
        .await
        .succeeded();
    world
        .run(&["channel", "test", "webhook", "--to", "somebody@example.org"])
        .await
        .refused_with("--to has no meaning");
    world
        .run(&["channel", "test", "email"])
        .await
        .refused_with("give it with --to");
    world
        .run(&["channel", "test", "telegram"])
        .await
        .refused_with("that person's own settings");
    assert!(
        hook.seen.lock().unwrap().is_empty(),
        "nothing was sent for those"
    );
}
