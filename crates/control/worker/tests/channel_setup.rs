//! A channel of the system put from its fields, with its secrets sealed, and a sample sent through it with nobody behind it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use async_trait::async_trait;
use orch_db::audit::{Actor, Origin};
use orch_db::queries::notifications::channels::owner;
use orch_notifier_contract::delivery::{Config, DeliverError, Delivery, LinkPoll};
use orch_notifier_contract::descriptor::{NotifierDescriptor, Owner, Reaches, Target};
use orch_notifier_contract::notifier::Notifier;
use orch_notifier_contract::templates::{BodyKind, TemplatePair, TemplateText};
use orch_types::{Field, FieldType, Id, Json};
use orch_worker_core::channel_setup::{Pick, Put, PutError, SystemChannel, find, put, send_sample};
use orch_worker_core::channels::{Notifiers, SaveFault, Toolbox, secret_binding};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use support::fakes::Scripted;
use support::world::World;

const SECRET: &str = "s3cr3t-token-value";

/// A bot: the system has one, and it writes to a chat that a person linked.
struct Bot;

#[async_trait]
impl Notifier for Bot {
    fn describe(&self) -> NotifierDescriptor {
        let text = |body: &str| TemplateText {
            subject: None,
            body: body.to_owned(),
        };
        let mut token = Field::new("bot_token", "Token", FieldType::Secret);
        token.required = true;
        NotifierDescriptor {
            name: "bot".to_owned(),
            title: "Bot".to_owned(),
            summary: "A bot for tests.".to_owned(),
            targets: vec![Target::Person],
            fields: vec![token],
            owners: vec![Owner::System, Owner::Org],
            single: true,
            reaches: Reaches::Link,
            body: BodyKind::Text,
            max_chars: None,
            templates: BTreeMap::from([(
                "*".to_owned(),
                TemplatePair {
                    single: text("{{title}}"),
                    digest: text("{{count}}"),
                },
            )]),
            extensions: orch_plugin_contract::Extensions::new(),
        }
    }

    async fn check(&self, _: &Config) -> Result<Json, DeliverError> {
        Ok(json!({ "bot_username": "orch_bot" }))
    }

    async fn deliver(&self, _: Delivery) -> Result<(), DeliverError> {
        Ok(())
    }

    async fn poll_links(
        &self,
        _: &Config,
        _: Option<String>,
        _: Duration,
    ) -> Result<LinkPoll, DeliverError> {
        Ok(LinkPoll::default())
    }
}

struct Setup {
    world: World,
    hook: Arc<Scripted>,
    mail: Arc<Scripted>,
}

async fn setup() -> Setup {
    let hook = Scripted::new("hook", Reaches::Channel);
    let mail = Scripted::new("mail", Reaches::Email);
    let mut notifiers = Notifiers::new();
    notifiers.register(hook.clone()).unwrap();
    notifiers.register(mail.clone()).unwrap();
    notifiers.register(Arc::new(Bot)).unwrap();
    Setup {
        world: World::new(notifiers).await,
        hook,
        mail,
    }
}

async fn put_as(
    world: &World,
    kind: &str,
    name: Option<&str>,
    fields: &Json,
) -> Result<Put, PutError> {
    put(
        &world.db(),
        &world.master,
        &world.notifiers,
        &SystemChannel {
            kind,
            name,
            fields,
            created_by: world.admin,
        },
        Actor::System,
        &Origin::default(),
        world.now(),
    )
    .await
}

async fn secrets_of(world: &World, channel: Id) -> Vec<(String, Vec<u8>)> {
    sqlx::query_as(
        "SELECT name, sealed FROM secrets WHERE owner_kind = 'notification_channel' AND owner_id = $1 ORDER BY name",
    )
    .bind(channel)
    .fetch_all(&world.test.pool)
    .await
    .unwrap()
}

async fn count_of(world: &World, table: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
        .fetch_one(&world.test.pool)
        .await
        .unwrap()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[tokio::test]
async fn a_secret_field_is_sealed_as_it_is_stored_and_opens_only_for_its_own_channel() {
    let Setup { world, .. } = setup().await;
    let fields = json!({ "url": "https://hooks.example/a", "token": SECRET });

    let made = put_as(&world, "hook", None, &fields).await.unwrap();

    assert!(made.created);
    assert_eq!(
        made.channel.config,
        json!({ "url": "https://hooks.example/a" })
    );
    assert!(made.channel.enabled && made.channel.is_the_systems());
    assert_eq!(made.channel.created_by, world.admin);
    let secrets = secrets_of(&world, made.channel.id).await;
    assert_eq!(secrets.len(), 1);
    let (name, sealed) = &secrets[0];
    assert_eq!(name, "token");
    assert!(
        !contains(sealed, SECRET.as_bytes()),
        "the value is not in the row"
    );
    let binding = secret_binding(owner::CHANNEL, made.channel.id, "token");
    assert_eq!(
        world.master.open(sealed, &binding).unwrap(),
        SECRET.as_bytes()
    );
    let elsewhere = secret_binding(owner::CHANNEL, orch_types::new_id(), "token");
    assert!(
        world.master.open(sealed, &elsewhere).is_err(),
        "copied to another channel it opens nothing"
    );
    // The value is in no other row: not the channel, not the audit trail.
    let elsewhere: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM notification_channels WHERE notification_channels::text LIKE '%' || $1 || '%')
              + (SELECT count(*) FROM audit_log WHERE audit_log::text LIKE '%' || $1 || '%')",
    )
    .bind(SECRET)
    .fetch_one(&world.test.pool)
    .await
    .unwrap();
    assert_eq!(elsewhere, 0);
}

#[tokio::test]
async fn putting_again_replaces_the_channel_and_its_secrets_and_switches_it_on() {
    let Setup { world, .. } = setup().await;
    let first = put_as(
        &world,
        "hook",
        None,
        &json!({ "url": "https://hooks.example/a", "token": "old-value" }),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE notification_channels SET enabled = false, last_error = 'was down'")
        .execute(&world.test.pool)
        .await
        .unwrap();

    let again = put_as(
        &world,
        "hook",
        None,
        &json!({ "url": "https://hooks.example/b", "token": SECRET }),
    )
    .await
    .unwrap();

    assert!(!again.created);
    assert_eq!(again.channel.id, first.channel.id, "one channel, replaced");
    assert_eq!(count_of(&world, "notification_channels").await, 1);
    assert_eq!(
        again.channel.config,
        json!({ "url": "https://hooks.example/b" })
    );
    assert!(again.channel.enabled, "a channel that is put is in use");
    assert_eq!(
        again.channel.last_error, None,
        "what was wrong is looked at anew"
    );
    let secrets = secrets_of(&world, again.channel.id).await;
    assert_eq!(secrets.len(), 1);
    let binding = secret_binding(owner::CHANNEL, again.channel.id, "token");
    assert_eq!(
        world.master.open(&secrets[0].1, &binding).unwrap(),
        SECRET.as_bytes()
    );
}

#[tokio::test]
async fn a_secret_that_the_new_fields_leave_out_is_removed() {
    let Setup { world, .. } = setup().await;
    let first = put_as(
        &world,
        "hook",
        None,
        &json!({ "url": "https://hooks.example/a", "token": SECRET }),
    )
    .await
    .unwrap();
    assert_eq!(secrets_of(&world, first.channel.id).await.len(), 1);

    let again = put_as(
        &world,
        "hook",
        None,
        &json!({ "url": "https://hooks.example/a" }),
    )
    .await
    .unwrap();

    assert!(secrets_of(&world, again.channel.id).await.is_empty());
    assert!(again.secrets.is_empty());
}

#[tokio::test]
async fn what_the_far_side_refuses_for_good_refuses_the_save_and_what_cannot_be_reached_is_kept_with_its_error()
 {
    let Setup { world, hook, .. } = setup().await;
    let fields = json!({ "url": "https://hooks.example/a", "token": SECRET });

    hook.check_answers(Err(DeliverError::refused(
        "unauthorized",
        "the token is not valid",
    )));
    let refused = put_as(&world, "hook", None, &fields).await.unwrap_err();
    match refused {
        PutError::Refused(error) => assert_eq!(error.message, "the token is not valid"),
        other => panic!("not refused: {other:?}"),
    }
    assert_eq!(count_of(&world, "notification_channels").await, 0);
    assert_eq!(count_of(&world, "secrets").await, 0);
    assert_eq!(
        count_of(&world, "audit_log").await,
        0,
        "a refusal leaves no line"
    );

    hook.check_answers(Err(DeliverError::temporary(
        "unreachable",
        "the server is down today",
    )));
    let kept = put_as(&world, "hook", None, &fields).await.unwrap();
    assert_eq!(
        kept.unreachable
            .as_ref()
            .map(|error| error.message.as_str()),
        Some("the server is down today")
    );
    assert_eq!(
        kept.channel.last_error.as_deref(),
        Some("the server is down today"),
        "a server that is down today is not a wrong entry"
    );
    assert_eq!(count_of(&world, "notification_channels").await, 1);
}

#[tokio::test]
async fn what_the_far_side_says_of_itself_is_kept_beside_the_fields_and_a_kind_there_is_not_is_refused()
 {
    let Setup { world, .. } = setup().await;

    let bot = put_as(&world, "bot", None, &json!({ "bot_token": SECRET }))
        .await
        .unwrap();
    assert_eq!(bot.channel.config, json!({ "bot_username": "orch_bot" }));
    assert_eq!(
        bot.channel.name, "Bot",
        "named for its notifier unless a name is given"
    );

    let unknown = put_as(&world, "carrier-pigeon", None, &json!({})).await;
    assert!(matches!(
        unknown,
        Err(PutError::Fault(SaveFault::UnknownKind(kind))) if kind == "carrier-pigeon"
    ));
    let missing = put_as(&world, "hook", None, &json!({ "url": 5 })).await;
    let Err(PutError::Fault(SaveFault::Fields(faults))) = missing else {
        panic!("a field that does not fit must be refused by name");
    };
    assert_eq!(faults[0].path, "url");
    let long = "x".repeat(81);
    let named = put_as(&world, "hook", Some(&long), &json!({})).await;
    assert!(matches!(named, Err(PutError::Name { max: 80 })));
}

#[tokio::test]
async fn a_system_has_one_bot_whatever_it_is_called_and_as_many_webhooks_as_it_names() {
    let Setup { world, .. } = setup().await;
    let bot = json!({ "bot_token": SECRET });
    let first = put_as(&world, "bot", Some("Team bot"), &bot).await.unwrap();
    let second = put_as(&world, "bot", Some("Another name"), &bot)
        .await
        .unwrap();
    assert_eq!(second.channel.id, first.channel.id);
    assert_eq!(second.channel.name, "Another name", "the same bot, renamed");

    let url = |path: &str| json!({ "url": format!("https://hooks.example/{path}") });
    let alpha = put_as(&world, "hook", Some("alpha"), &url("a"))
        .await
        .unwrap();
    let beta = put_as(&world, "hook", Some("beta"), &url("b"))
        .await
        .unwrap();
    assert_ne!(alpha.channel.id, beta.channel.id);
    let ambiguous = put_as(&world, "hook", None, &url("c")).await.unwrap_err();
    assert!(
        matches!(&ambiguous, PutError::Several { names, .. } if names == "alpha, beta"),
        "{ambiguous}"
    );

    let changed = put_as(&world, "hook", Some("beta"), &url("changed"))
        .await
        .unwrap();
    assert_eq!(changed.channel.id, beta.channel.id);
    assert!(!changed.created);
    let mut conn = world.test.pool.acquire().await.unwrap();
    let kept = find(&mut conn, &world.notifiers, "hook", Some("alpha"))
        .await
        .unwrap();
    assert!(
        matches!(&kept, Pick::One(row) if row.config == url("a")),
        "{kept:?}"
    );
    assert!(matches!(
        find(&mut conn, &world.notifiers, "hook", None).await.unwrap(),
        Pick::Several(names) if names.len() == 2
    ));
    assert_eq!(
        find(&mut conn, &world.notifiers, "hook", Some("gamma"))
            .await
            .unwrap(),
        Pick::Nothing
    );
}

#[tokio::test]
async fn a_bot_that_is_put_again_forgets_where_the_last_reading_of_its_followers_ended() {
    let Setup { world, .. } = setup().await;
    let bot = json!({ "bot_token": SECRET });
    let first = put_as(&world, "bot", None, &bot).await.unwrap();
    sqlx::query("UPDATE notification_channels SET link_cursor = '41'")
        .execute(&world.test.pool)
        .await
        .unwrap();

    let again = put_as(&world, "bot", None, &bot).await.unwrap();

    assert_eq!(again.channel.id, first.channel.id);
    assert_eq!(
        again.channel.link_cursor, None,
        "the new token may be another bot, whose updates are numbered afresh"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn two_operators_putting_the_systems_one_bot_at_the_same_moment_make_one_and_replace_it() {
    let Setup { world, .. } = setup().await;
    let bot = json!({ "bot_token": SECRET });

    let (first, second) = tokio::join!(
        put_as(&world, "bot", Some("first"), &bot),
        put_as(&world, "bot", Some("second"), &bot)
    );

    let (first, second) = (first.unwrap(), second.unwrap());
    assert_eq!(first.channel.id, second.channel.id, "one bot, not two");
    assert_eq!(
        [first.created, second.created]
            .iter()
            .filter(|made| **made)
            .count(),
        1,
        "the one that came second replaced what the first made"
    );
    assert_eq!(count_of(&world, "notification_channels").await, 1);
    assert_eq!(secrets_of(&world, first.channel.id).await.len(), 1);
}

#[tokio::test]
async fn a_put_is_in_the_trail_under_the_actor_given_with_the_names_of_the_fields_and_never_a_value()
 {
    let Setup { world, .. } = setup().await;
    let fields = json!({ "url": "https://hooks.example/a", "token": SECRET });
    put_as(&world, "hook", None, &fields).await.unwrap();
    put_as(&world, "hook", None, &fields).await.unwrap();

    let lines: Vec<(String, String, bool, Json)> = sqlx::query_as(
        "SELECT action, actor_kind, history, details FROM audit_log ORDER BY at, id",
    )
    .fetch_all(&world.test.pool)
    .await
    .unwrap();

    assert_eq!(lines.len(), 2);
    assert_eq!(
        (lines[0].0.as_str(), lines[0].1.as_str()),
        ("channel.created", "system")
    );
    assert_eq!(lines[1].0, "channel.updated");
    assert_eq!(lines[0].3["kind"], "hook");
    assert_eq!(lines[0].3["secrets"], json!(["token"]));
    assert_eq!(lines[0].3["fields"], json!(["url"]));
}

#[tokio::test]
async fn a_sample_has_nobody_behind_it_and_goes_to_the_address_given() {
    let Setup { world, hook, mail } = setup().await;
    let mail_channel = put_as(&world, "mail", None, &json!({ "url": "smtp.example" }))
        .await
        .unwrap()
        .channel;
    let hook_channel = put_as(
        &world,
        "hook",
        None,
        &json!({ "url": "https://hooks.example/a" }),
    )
    .await
    .unwrap()
    .channel;
    let surroundings = world.surroundings();
    let tools = Toolbox {
        master: &world.master,
        notifiers: &world.notifiers,
        surroundings: &surroundings,
    };
    let mut conn = world.test.pool.acquire().await.unwrap();

    let sent = send_sample(
        &mut conn,
        &tools,
        &mail_channel,
        Some("ops@example.org"),
        world.now(),
    )
    .await
    .unwrap();
    let posted = send_sample(&mut conn, &tools, &hook_channel, None, world.now())
        .await
        .unwrap();

    assert!(sent.ok && posted.ok);
    let delivered = mail.delivered();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].to, json!({ "email": "ops@example.org" }));
    assert_eq!(
        delivered[0].recipient, None,
        "a sample is for nobody in particular"
    );
    assert_eq!(delivered[0].language, "en");
    assert!(
        delivered[0]
            .subject
            .as_deref()
            .is_some_and(|subject| subject.contains("Test message from Acme Orch")),
        "{:?}",
        delivered[0].subject
    );
    assert_eq!(
        hook.delivered()[0].to,
        json!({}),
        "a webhook posts to its own address"
    );
}

#[tokio::test]
async fn a_sample_that_the_far_side_does_not_take_comes_back_with_what_it_answered() {
    let Setup { world, mail, .. } = setup().await;
    let channel = put_as(&world, "mail", None, &json!({}))
        .await
        .unwrap()
        .channel;
    mail.answer([Err(DeliverError::refused(
        "mailbox",
        "550 5.1.1 no such mailbox",
    ))]);
    let surroundings = world.surroundings();
    let tools = Toolbox {
        master: &world.master,
        notifiers: &world.notifiers,
        surroundings: &surroundings,
    };
    let mut conn = world.test.pool.acquire().await.unwrap();

    let outcome = send_sample(
        &mut conn,
        &tools,
        &channel,
        Some("nobody@example.org"),
        world.now(),
    )
    .await
    .unwrap();

    assert!(!outcome.ok);
    assert_eq!(outcome.error.as_deref(), Some("550 5.1.1 no such mailbox"));
}

#[tokio::test]
async fn a_sample_needs_an_address_for_a_mail_server_and_has_nobody_to_write_to_for_a_bot() {
    let Setup { world, mail, .. } = setup().await;
    let mail_channel = put_as(&world, "mail", None, &json!({}))
        .await
        .unwrap()
        .channel;
    let bot_channel = put_as(&world, "bot", None, &json!({ "bot_token": SECRET }))
        .await
        .unwrap()
        .channel;
    let surroundings = world.surroundings();
    let tools = Toolbox {
        master: &world.master,
        notifiers: &world.notifiers,
        surroundings: &surroundings,
    };
    let mut conn = world.test.pool.acquire().await.unwrap();

    let no_address = send_sample(&mut conn, &tools, &mail_channel, None, world.now())
        .await
        .unwrap();
    let bot = send_sample(&mut conn, &tools, &bot_channel, Some("42"), world.now())
        .await
        .unwrap();

    assert!(!no_address.ok && !bot.ok);
    assert!(no_address.error.unwrap().contains("address"));
    assert!(bot.error.unwrap().contains("person"));
    assert_eq!(mail.deliveries(), 0, "nothing was sent");
}
