//! The ways out of the application over HTTP: the channels that reach a person, set up by the system, by a root org and by a person, checked against their notifier and the far side, kept with their secrets sealed and never returned, changed, removed and tried.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod outside;
mod places;
mod support;

use orch_api_contract::common::{ErrorCode, Problem};
use orch_api_contract::notifications::{
    AdminNotificationChannelCreate, LanguageTemplates, MeNotificationChannelCreate,
    MeNotificationChannelCreateBody, NotificationChannel, NotificationChannelCreateBody,
    NotificationChannelDelete, NotificationChannelList, NotificationChannelListQuery,
    NotificationChannelOwner, NotificationChannelTest, NotificationChannelTestBody,
    NotificationChannelUpdate, NotificationChannelUpdateBody, NotificationTestReply, NotifierList,
    OrgNotificationChannelCreate, TemplateText, WebhookConfig, WebhookSecrets,
};
use orch_api_core::testing::Reply;
use orch_db::secrets::MasterKey;
use orch_types::Id;
use orch_worker_core::channels::secret_binding;
use outside::{BOUNCING, DOWN_HOST, FarSide, MASTER, Outside, REFUSING_HOST, UNKNOWN_TOKEN, sent};
use places::Who;
use serde_json::json;
use std::collections::BTreeMap;

const PASSWORD: &str = "hunter2-the-password";

async fn make_system(
    o: &Outside,
    by: &Who,
    body: &NotificationChannelCreateBody,
) -> Reply<NotificationChannel> {
    o.world
        .app
        .call(AdminNotificationChannelCreate, &[])
        .token(&by.token)
        .body(body)
        .send()
        .await
}

async fn make_org(
    o: &Outside,
    by: &Who,
    org: Id,
    body: &NotificationChannelCreateBody,
) -> Reply<NotificationChannel> {
    o.world
        .app
        .call(OrgNotificationChannelCreate, &[&org])
        .token(&by.token)
        .body(body)
        .send()
        .await
}

async fn make_own(
    o: &Outside,
    by: &Who,
    name: &str,
    url: &str,
    secret: Option<&str>,
) -> Reply<NotificationChannel> {
    o.world
        .app
        .call(MeNotificationChannelCreate, &[])
        .token(&by.token)
        .body(&MeNotificationChannelCreateBody {
            name: name.to_owned(),
            config: WebhookConfig {
                url: url.to_owned(),
            },
            secrets: secret.map(|secret| WebhookSecrets {
                secret: secret.to_owned(),
            }),
            templates: None,
        })
        .send()
        .await
}

async fn change(
    o: &Outside,
    by: &Who,
    id: Id,
    body: &NotificationChannelUpdateBody,
) -> Reply<NotificationChannel> {
    o.world
        .app
        .call(NotificationChannelUpdate, &[&id])
        .token(&by.token)
        .body(body)
        .send()
        .await
}

async fn remove(o: &Outside, by: &Who, id: Id) -> Reply<()> {
    o.world
        .app
        .call(NotificationChannelDelete, &[&id])
        .token(&by.token)
        .send()
        .await
}

async fn try_it(o: &Outside, by: &Who, id: Id, to: Option<&str>) -> Reply<NotificationTestReply> {
    o.world
        .app
        .call(NotificationChannelTest, &[&id])
        .token(&by.token)
        .body(&NotificationChannelTestBody {
            to: to.map(str::to_owned),
            device_id: None,
        })
        .send()
        .await
}

/// The members of a refusal that name what was wrong, with what was said of each.
fn faults(problem: &Problem) -> Vec<(String, String)> {
    problem
        .extra
        .get("errors")
        .and_then(|errors| errors.as_array())
        .into_iter()
        .flatten()
        .map(|fault| {
            (
                fault["path"].as_str().unwrap().to_owned(),
                fault["message"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn paths(problem: &Problem) -> Vec<String> {
    faults(problem).into_iter().map(|(path, _)| path).collect()
}

#[tokio::test]
async fn a_system_admin_sets_up_the_systems_mail_server_and_nobody_else_does() {
    let o = Outside::new().await;

    let made = make_system(&o, &o.ada, &Outside::mail_server("smtp.example"))
        .await
        .ok();
    assert_eq!((made.kind.as_str(), made.name.as_str()), ("email", "Mail"));
    assert_eq!(made.owner, NotificationChannelOwner::System);
    assert!(made.can_edit && made.enabled);
    assert_eq!(made.config["host"], "smtp.example");
    assert_eq!(made.secrets_set, ["password"]);
    assert_eq!(made.last_error, None);
    assert!(!made.route.enabled, "every route starts off");

    for who in [&o.bea, &o.cai, &o.fay] {
        let refused = make_system(&o, who, &Outside::mail_server("other.example"))
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::AuthzDenied);
        assert_eq!(refused.extra["required"], "system.admin");
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM notification_channels")
        .fetch_one(&o.world.db.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn a_secret_of_a_channel_cannot_be_read_back_by_anyone() {
    let o = Outside::new().await;
    let made = make_system(&o, &o.ada, &Outside::mail_server("smtp.example"))
        .await
        .ok();
    org_channel(&o).await;

    // Nothing any caller is given holds it: not the one who set it up, not the org's admin, not a member.
    let mut said = serde_json::to_string(&made).unwrap();
    for who in [&o.ada, &o.bea, &o.cai, &o.dee, &o.fay] {
        said.push_str(&serde_json::to_string(&o.channels(who).await).unwrap());
    }
    assert!(!said.contains(PASSWORD), "{said}");

    // It is in one row, sealed to the channel and its name, and opens only there.
    let rows: Vec<(String, Vec<u8>)> = sqlx::query_as(
        "SELECT name, sealed FROM secrets WHERE owner_kind = 'notification_channel' AND owner_id = $1",
    )
    .bind(made.id)
    .fetch_all(&o.world.db.pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "password");
    assert!(
        !rows[0]
            .1
            .windows(PASSWORD.len())
            .any(|window| window == PASSWORD.as_bytes())
    );
    let key = MasterKey::new(MASTER);
    assert_eq!(
        key.open(
            &rows[0].1,
            &secret_binding("notification_channel", made.id, "password")
        )
        .unwrap(),
        PASSWORD.as_bytes()
    );
    assert!(
        key.open(
            &rows[0].1,
            &secret_binding("notification_channel", made.id, "other")
        )
        .is_err()
    );

    // The trail of audit says which kind and which fields, never a value.
    let trail: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT action, details FROM audit_log WHERE object_id = $1 ORDER BY at, id",
    )
    .bind(made.id)
    .fetch_all(&o.world.db.pool)
    .await
    .unwrap();
    assert_eq!(trail.len(), 1);
    assert_eq!(trail[0].0, "channel.created");
    assert_eq!(trail[0].1["kind"], "email");
    assert!(!trail[0].1.to_string().contains(PASSWORD));
}

/// An org's mail server, set up by its admin.
async fn org_channel(o: &Outside) -> NotificationChannel {
    make_org(
        o,
        &o.bea,
        o.acme,
        &Outside::mail_server("smtp.acme.example"),
    )
    .await
    .ok()
}

#[tokio::test]
async fn a_second_mail_server_or_bot_is_refused_for_the_system_and_for_a_root_org_and_a_second_webhook_is_not()
 {
    let o = Outside::new().await;
    make_system(&o, &o.ada, &Outside::mail_server("smtp.example"))
        .await
        .ok();
    make_system(&o, &o.ada, &Outside::bot("good-token"))
        .await
        .ok();
    for body in [
        Outside::mail_server("again.example"),
        Outside::bot("good-token"),
    ] {
        let refused = make_system(&o, &o.ada, &body).await.problem();
        assert_eq!(refused.code, ErrorCode::ConflictExists, "{body:?}");
    }
    org_channel(&o).await;
    let refused = make_org(
        &o,
        &o.bea,
        o.acme,
        &Outside::mail_server("again.acme.example"),
    )
    .await
    .problem();
    assert_eq!(refused.code, ErrorCode::ConflictExists);
    // The refusal does not wait for what the far side would have said.
    let refused = make_org(&o, &o.bea, o.acme, &Outside::mail_server(REFUSING_HOST))
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::ConflictExists);
    // Each root org has its own, and any number of webhooks.
    for name in ["Hook one", "Hook two"] {
        make_org(
            &o,
            &o.bea,
            o.acme,
            &Outside::webhook(name, "https://hooks.example/a", None),
        )
        .await
        .ok();
    }
}

#[tokio::test]
async fn a_channel_of_a_root_org_needs_settings_manage_there_and_not_an_org_inside_it_nor_a_developer()
 {
    let o = Outside::new().await;
    let made = make_org(
        &o,
        &o.bea,
        o.acme,
        &Outside::mail_server("smtp.acme.example"),
    )
    .await
    .ok();
    match &made.owner {
        NotificationChannelOwner::Org(org) => {
            assert_eq!((org.id, org.name.as_deref()), (o.acme, Some("Acme")));
        }
        other => panic!("{other:?}"),
    }
    assert!(made.can_edit);

    let refused = make_org(&o, &o.cai, o.acme, &Outside::bot("good-token"))
        .await
        .problem();
    assert_eq!(
        refused.code,
        ErrorCode::AuthzDenied,
        "a developer does not manage the org's settings"
    );
    let refused = make_org(&o, &o.fay, o.acme, &Outside::bot("good-token"))
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    let inside = make_org(&o, &o.bea, o.research, &Outside::bot("good-token"))
        .await
        .problem();
    assert_eq!(
        inside.code,
        ErrorCode::RequestInvalid,
        "an org inside another has none of its own"
    );
    assert_eq!(paths(&inside), ["id"]);
    let nowhere = make_org(
        &o,
        &o.bea,
        orch_types::new_id(),
        &Outside::bot("good-token"),
    )
    .await
    .problem();
    assert_eq!(nowhere.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn a_person_sets_up_a_webhook_of_their_own_which_nobody_else_sees_or_changes() {
    let o = Outside::new().await;
    let far = FarSide::taking().await;
    // The address is on this machine: the list of private hosts lets it be.
    o.world
        .set("outbound.allowed_private_hosts", json!(["127.0.0.1"]))
        .await;

    let made = make_own(&o, &o.cai, "My hook", &far.url, Some("shh-its-mine"))
        .await
        .ok();
    assert_eq!(made.owner, NotificationChannelOwner::Me);
    assert_eq!(made.kind, "webhook");
    assert_eq!(made.secrets_set, ["secret"]);
    assert!(made.can_edit);
    assert!(
        !serde_json::to_string(&made)
            .unwrap()
            .contains("shh-its-mine")
    );

    let mine: Vec<_> = o
        .channels(&o.cai)
        .await
        .into_iter()
        .map(|each| each.id)
        .collect();
    assert_eq!(mine, [made.id]);
    assert!(
        o.channels(&o.dee).await.is_empty(),
        "somebody else's own channel is not in their list"
    );
    for who in [&o.dee, &o.bea, &o.ada] {
        assert_eq!(
            change(&o, who, made.id, &NotificationChannelUpdateBody::default())
                .await
                .problem()
                .code,
            ErrorCode::NotFound
        );
        assert_eq!(
            remove(&o, who, made.id).await.problem().code,
            ErrorCode::NotFound
        );
        assert_eq!(
            try_it(&o, who, made.id, None).await.problem().code,
            ErrorCode::NotFound
        );
    }
    // A mail server or a bot is not what a person sets up for themself.
    let own_mail = o
        .world
        .app
        .call(MeNotificationChannelCreate, &[])
        .token(&o.cai.token)
        .raw_body(&json!({ "name": "x", "config": { "host": "smtp.example" } }))
        .send()
        .await
        .problem();
    assert_eq!(own_mail.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn what_the_far_side_refuses_for_good_refuses_the_save_and_one_that_is_down_is_kept_with_the_reason()
 {
    let o = Outside::new().await;
    let refused = make_system(&o, &o.ada, &Outside::mail_server(REFUSING_HOST))
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    assert!(
        faults(&refused)[0].1.contains("wrong password"),
        "what it answered: {refused:?}"
    );
    let bad_bot = make_system(&o, &o.ada, &Outside::bot(UNKNOWN_TOKEN))
        .await
        .problem();
    assert!(faults(&bad_bot)[0].1.contains("Unauthorized"));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM notification_channels")
        .fetch_one(&o.world.db.pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "nothing was kept");

    let down = make_system(&o, &o.ada, &Outside::mail_server(DOWN_HOST))
        .await
        .ok();
    assert_eq!(
        down.last_error.as_deref(),
        Some("connection to down.example refused"),
        "a server that is down today is not a wrong entry"
    );
    let bot = make_org(&o, &o.bea, o.acme, &Outside::bot("good-token"))
        .await
        .ok();
    assert_eq!(
        bot.config["bot_username"], "acme_orch_bot",
        "what the far side said of itself is kept"
    );
}

#[tokio::test]
async fn the_fields_of_a_channel_are_checked_against_its_notifier_naming_each() {
    let o = Outside::new().await;
    let mut body = Outside::mail_server("smtp.example");
    body.config = json!({ "port": 587, "extra": "x" });
    body.secrets = None;
    let refused = make_system(&o, &o.ada, &body).await.problem();
    let named = paths(&refused);
    for path in [
        "config.host",
        "config.from",
        "config.port",
        "config.extra",
        "secrets.password",
    ] {
        assert!(named.contains(&path.to_owned()), "{path} in {named:?}");
    }

    // A secret is given as one and nowhere else.
    let mut in_config = Outside::mail_server("smtp.example");
    in_config.config = json!({ "host": "smtp.example", "from": "a@b.example", "password": "x" });
    assert_eq!(
        paths(&make_system(&o, &o.ada, &in_config).await.problem()),
        ["config.password"]
    );
    let mut wrong_place = Outside::mail_server("smtp.example");
    wrong_place.secrets = Some(BTreeMap::from([
        ("password".to_owned(), "x".to_owned()),
        ("host".to_owned(), "elsewhere".to_owned()),
    ]));
    assert_eq!(
        paths(&make_system(&o, &o.ada, &wrong_place).await.problem()),
        ["secrets.host"]
    );

    let mut unknown = Outside::mail_server("smtp.example");
    unknown.kind = "carrier-pigeon".to_owned();
    assert_eq!(
        paths(&make_system(&o, &o.ada, &unknown).await.problem()),
        ["kind"]
    );
    // Push is the system's alone; a bot is not what a person sets up.
    let push = NotificationChannelCreateBody {
        kind: "push".to_owned(),
        name: "Push".to_owned(),
        config: json!({}),
        secrets: None,
        templates: None,
        enabled: None,
    };
    assert_eq!(
        paths(&make_org(&o, &o.bea, o.acme, &push).await.problem()),
        ["kind"]
    );
    make_system(&o, &o.ada, &push).await.ok();
    let mut nameless = Outside::bot("good-token");
    nameless.name = "  ".to_owned();
    assert_eq!(
        paths(&make_system(&o, &o.ada, &nameless).await.problem()),
        ["name"]
    );
    nameless.name = "n".repeat(81);
    assert_eq!(
        paths(&make_system(&o, &o.ada, &nameless).await.problem()),
        ["name"]
    );
}

fn text(body: &str) -> TemplateText {
    TemplateText {
        subject: None,
        body: body.to_owned(),
    }
}

fn single(body: &str) -> BTreeMap<String, LanguageTemplates> {
    BTreeMap::from([(
        "*".to_owned(),
        LanguageTemplates {
            single: Some(text(body)),
            digest: None,
        },
    )])
}

#[tokio::test]
async fn the_texts_of_a_channel_are_checked_when_they_are_saved_naming_what_is_wrong() {
    let o = Outside::new().await;
    let far = FarSide::taking().await;
    let at = |templates| {
        let mut body = Outside::webhook("Hook", &far.url, None);
        body.templates = Some(templates);
        body
    };
    // A placeholder outside the list.
    let refused = make_system(&o, &o.ada, &at(single("{{password}}")))
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    // The fault places the text; its explanation lists placeholders written with dots, which a
    // public problem keeps to the catalogue's sentence.
    assert_eq!(paths(&refused), ["templates.*.single"]);
    assert_eq!(
        faults(&refused)[0].1,
        orch_api_contract::messages::for_code(ErrorCode::RequestInvalid)
    );
    // A section that is not closed, a text over 16 KiB, and a webhook's text that is no JSON.
    for texts in [
        single("{{#notifications}}{{title}}"),
        single(&"x".repeat(17 * 1024)),
        single("{{title}} finished"),
    ] {
        assert_eq!(
            make_system(&o, &o.ada, &at(texts)).await.problem().code,
            ErrorCode::RequestInvalid
        );
    }
    // One that is JSON when rendered is kept, and shown back with the channel.
    let kept = make_system(
        &o,
        &o.ada,
        &at(single("{\"text\": \"{{title}}: {{body}}\"}")),
    )
    .await
    .ok();
    assert_eq!(
        kept.templates["*"].single.as_ref().unwrap().body,
        "{\"text\": \"{{title}}: {{body}}\"}"
    );
}

#[tokio::test]
async fn a_host_on_a_private_address_is_refused_for_an_orgs_and_a_persons_channel_and_taken_once_listed()
 {
    let o = Outside::new().await;
    let far = FarSide::taking().await;

    // The system's own channels are not held to the list.
    let systems = make_system(&o, &o.ada, &Outside::webhook("System hook", &far.url, None))
        .await
        .ok();
    assert!(try_it(&o, &o.ada, systems.id, None).await.ok().ok);
    assert_eq!(far.count(), 1);

    let refused = make_org(
        &o,
        &o.bea,
        o.acme,
        &Outside::webhook("Acme hook", &far.url, None),
    )
    .await
    .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    // The fault places the address; its explanation names a setting, which a public problem keeps
    // to the catalogue's sentence.
    assert_eq!(paths(&refused), ["config"], "{refused:?}");
    assert_eq!(
        faults(&refused)[0].1,
        orch_api_contract::messages::for_code(ErrorCode::RequestInvalid)
    );
    assert_eq!(
        make_own(&o, &o.cai, "Mine", &far.url, None)
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );

    o.world
        .set("outbound.allowed_private_hosts", json!(["127.0.0.1"]))
        .await;
    let taken = make_org(
        &o,
        &o.bea,
        o.acme,
        &Outside::webhook("Acme hook", &far.url, None),
    )
    .await
    .ok();
    make_own(&o, &o.cai, "Mine", &far.url, None).await.ok();
    assert!(try_it(&o, &o.bea, taken.id, None).await.ok().ok);
}

#[tokio::test]
async fn a_channel_is_changed_by_whoever_may_set_it_up_and_a_secret_stays_unless_it_is_replaced() {
    let o = Outside::new().await;
    let made = org_channel(&o).await;
    let before: Vec<u8> = sqlx::query_scalar("SELECT sealed FROM secrets WHERE owner_id = $1")
        .bind(made.id)
        .fetch_one(&o.world.db.pool)
        .await
        .unwrap();

    let renamed = change(
        &o,
        &o.bea,
        made.id,
        &NotificationChannelUpdateBody {
            name: Some("Acme mail".to_owned()),
            ..NotificationChannelUpdateBody::default()
        },
    )
    .await
    .ok();
    assert_eq!(renamed.name, "Acme mail");
    assert_eq!(
        renamed.config["host"], "smtp.acme.example",
        "what is left out stays"
    );

    // New fields are looked at, with the secret it has.
    let moved = change(
        &o,
        &o.bea,
        made.id,
        &NotificationChannelUpdateBody {
            config: Some(json!({ "host": "mail2.acme.example", "from": "orch@acme.example" })),
            ..NotificationChannelUpdateBody::default()
        },
    )
    .await
    .ok();
    assert_eq!(moved.config["host"], "mail2.acme.example");
    assert_eq!(moved.secrets_set, ["password"]);
    let after: Vec<u8> = sqlx::query_scalar("SELECT sealed FROM secrets WHERE owner_id = $1")
        .bind(made.id)
        .fetch_one(&o.world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        before, after,
        "the password was not given again, so it is the one it was"
    );

    // A host the far side refuses for good refuses the change; nothing changes.
    let refused = change(
        &o,
        &o.bea,
        made.id,
        &NotificationChannelUpdateBody {
            config: Some(json!({ "host": REFUSING_HOST, "from": "orch@acme.example" })),
            ..NotificationChannelUpdateBody::default()
        },
    )
    .await
    .problem();
    assert!(faults(&refused)[0].1.contains("wrong password"));
    let host: String =
        sqlx::query_scalar("SELECT config ->> 'host' FROM notification_channels WHERE id = $1")
            .bind(made.id)
            .fetch_one(&o.world.db.pool)
            .await
            .unwrap();
    assert_eq!(host, "mail2.acme.example");

    // A new password replaces the old one; an empty one cannot take away what the server needs.
    change(
        &o,
        &o.bea,
        made.id,
        &NotificationChannelUpdateBody {
            secrets: Some(BTreeMap::from([(
                "password".to_owned(),
                "another-password".to_owned(),
            )])),
            ..NotificationChannelUpdateBody::default()
        },
    )
    .await
    .ok();
    let replaced: Vec<u8> = sqlx::query_scalar("SELECT sealed FROM secrets WHERE owner_id = $1")
        .bind(made.id)
        .fetch_one(&o.world.db.pool)
        .await
        .unwrap();
    assert_ne!(before, replaced);
    let emptied = change(
        &o,
        &o.bea,
        made.id,
        &NotificationChannelUpdateBody {
            secrets: Some(BTreeMap::from([("password".to_owned(), String::new())])),
            ..NotificationChannelUpdateBody::default()
        },
    )
    .await
    .problem();
    assert_eq!(paths(&emptied), ["secrets.password"]);

    // Switched off, it is shown so.
    let off = change(
        &o,
        &o.bea,
        made.id,
        &NotificationChannelUpdateBody {
            enabled: Some(false),
            ..NotificationChannelUpdateBody::default()
        },
    )
    .await
    .ok();
    assert!(!off.enabled);
    let trail: i64 = o
        .count(
            "SELECT count(*) FROM audit_log WHERE object_id = $1 AND action = 'channel.updated'",
            made.id,
        )
        .await;
    assert!(trail >= 4, "every change is in the trail");
}

#[tokio::test]
async fn a_secret_that_is_not_required_is_taken_away_by_an_empty_value_and_the_texts_alone_are_changed_without_the_far_side()
 {
    let o = Outside::new().await;
    let far = FarSide::taking().await;
    let made = make_system(
        &o,
        &o.ada,
        &Outside::webhook("Hook", &far.url, Some("sign-with-this")),
    )
    .await
    .ok();
    assert_eq!(made.secrets_set, ["secret"]);
    let unsigned = change(
        &o,
        &o.ada,
        made.id,
        &NotificationChannelUpdateBody {
            secrets: Some(BTreeMap::from([("secret".to_owned(), String::new())])),
            ..NotificationChannelUpdateBody::default()
        },
    )
    .await
    .ok();
    assert!(unsigned.secrets_set.is_empty());

    let texts = change(
        &o,
        &o.ada,
        made.id,
        &NotificationChannelUpdateBody {
            templates: Some(single("{\"text\": \"{{title}}\"}")),
            ..NotificationChannelUpdateBody::default()
        },
    )
    .await
    .ok();
    assert_eq!(
        texts.templates["*"].single.as_ref().unwrap().body,
        "{\"text\": \"{{title}}\"}"
    );
    let bad = change(
        &o,
        &o.ada,
        made.id,
        &NotificationChannelUpdateBody {
            templates: Some(single("{{password}}")),
            ..NotificationChannelUpdateBody::default()
        },
    )
    .await
    .problem();
    assert_eq!(paths(&bad), ["templates.*.single"]);
}

#[tokio::test]
async fn a_channel_is_changed_and_removed_only_by_whoever_may_set_it_up() {
    let o = Outside::new().await;
    let systems = make_system(&o, &o.ada, &Outside::bot("good-token"))
        .await
        .ok();
    let orgs = org_channel(&o).await;
    let rename = NotificationChannelUpdateBody {
        name: Some("Renamed".to_owned()),
        ..NotificationChannelUpdateBody::default()
    };
    // The system's: its admin alone; an org's admin does not.
    for who in [&o.bea, &o.cai, &o.fay] {
        assert_eq!(
            change(&o, who, systems.id, &rename).await.problem().extra["required"],
            "system.admin"
        );
        assert_eq!(
            remove(&o, who, systems.id).await.problem().code,
            ErrorCode::AuthzDenied
        );
    }
    // An org's: whoever holds settings.manage there; a developer of the org does not, nor does a stranger.
    for who in [&o.cai, &o.dee, &o.fay] {
        assert_eq!(
            change(&o, who, orgs.id, &rename).await.problem().code,
            ErrorCode::AuthzDenied
        );
        assert_eq!(
            remove(&o, who, orgs.id).await.problem().code,
            ErrorCode::AuthzDenied
        );
    }
    change(&o, &o.bea, orgs.id, &rename).await.ok();
    change(&o, &o.ada, systems.id, &rename).await.ok();
    assert_eq!(
        remove(&o, &o.bea, systems.id).await.problem().code,
        ErrorCode::AuthzDenied
    );
    assert_eq!(remove(&o, &o.bea, orgs.id).await.status, 204);
    assert_eq!(remove(&o, &o.ada, systems.id).await.status, 204);
    assert_eq!(
        remove(&o, &o.ada, systems.id).await.problem().code,
        ErrorCode::NotFound
    );
}

#[tokio::test]
async fn deleting_a_channel_ends_every_route_through_it_and_removes_its_secrets() {
    let o = Outside::new().await;
    let far = FarSide::taking().await;
    o.world
        .set("outbound.allowed_private_hosts", json!(["127.0.0.1"]))
        .await;
    let made = make_org(
        &o,
        &o.bea,
        o.acme,
        &Outside::webhook("Acme hook", &far.url, Some("signing")),
    )
    .await
    .ok();
    for who in [&o.cai, &o.dee] {
        let on = o
            .world
            .app
            .call(
                orch_api_contract::notifications::MeNotificationRouteSet,
                &[&made.id],
            )
            .token(&who.token)
            .body(
                &orch_api_contract::notifications::MeNotificationRouteSetBody {
                    enabled: Some(true),
                    ..Default::default()
                },
            )
            .send()
            .await
            .ok();
        assert!(on.enabled);
    }
    assert_eq!(
        o.count(
            "SELECT count(*) FROM notification_routes WHERE channel_id = $1",
            made.id
        )
        .await,
        2
    );
    assert_eq!(remove(&o, &o.bea, made.id).await.status, 204);
    assert_eq!(
        o.count(
            "SELECT count(*) FROM notification_routes WHERE channel_id = $1",
            made.id
        )
        .await,
        0
    );
    assert_eq!(
        o.count("SELECT count(*) FROM secrets WHERE owner_id = $1", made.id)
            .await,
        0
    );
    assert!(o.channels(&o.cai).await.is_empty());
    assert_eq!(
        o.count(
            "SELECT count(*) FROM audit_log WHERE object_id = $1 AND action = 'channel.deleted'",
            made.id
        )
        .await,
        1
    );
}

#[tokio::test]
async fn a_test_returns_what_the_far_side_answered_when_it_does_not_take_the_message() {
    let o = Outside::new().await;
    let broken = FarSide::answering(500, "the build system is on fire").await;
    let made = make_system(
        &o,
        &o.ada,
        &Outside::webhook("Hook", &broken.url, Some("signing")),
    )
    .await
    .ok();
    let tried = try_it(&o, &o.ada, made.id, None).await.ok();
    assert!(!tried.ok);
    assert!(tried.error.as_deref().unwrap().contains("500"), "{tried:?}");
    assert_eq!(broken.count(), 1, "one sample message went through it now");
    let hit = broken.hits.lock().unwrap()[0].clone();
    assert!(hit.signature.as_deref().unwrap().starts_with("sha256="));
    let posted: serde_json::Value = serde_json::from_str(&hit.body).unwrap();
    assert_eq!(posted["kind"], "milestone");
    assert!(
        posted["title"]
            .as_str()
            .unwrap()
            .starts_with("Test message from")
    );
    // The reason is kept on the channel for whoever looks after it.
    let seen = o.channels(&o.ada).await;
    assert!(seen[0].last_error.as_deref().unwrap().contains("500"));
    assert_eq!(seen[0].last_ok_at, None);
}

#[tokio::test]
async fn slow_channel_and_destination_tests_leave_the_only_connection_available() {
    use axum::extract::State;
    use orch_api_contract::notifications::{
        DestinationTest, NodeDestinationCreate, NodeDestinationCreateBody,
    };
    use orch_api_core::registry::Registry;
    use orch_api_core::testing::TestApp;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::Notify;

    // Shorter than the notifier deadline; the endpoint is gated, so no timing of remote I/O is assumed.
    const RESPONSIVE: Duration = Duration::from_secs(2);
    let o = Outside::new().await;
    o.world
        .set("outbound.allowed_private_hosts", json!(["127.0.0.1"]))
        .await;
    let gates = Arc::new((Notify::new(), Notify::new()));
    let router = axum::Router::new()
        .fallback(|State(gates): State<Arc<(Notify, Notify)>>| async move {
            gates.0.notify_one();
            gates.1.notified().await;
            "ok"
        })
        .with_state(Arc::clone(&gates));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hook", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let channel = make_system(&o, &o.ada, &Outside::webhook("Slow hook", &url, None))
        .await
        .ok();
    let destination = o
        .world
        .app
        .call(NodeDestinationCreate, &[&o.acme])
        .token(&o.bea.token)
        .body(&NodeDestinationCreateBody {
            channel: None,
            name: "Slow destination".to_owned(),
            config: json!({"url": url}),
            secrets: None,
            templates: None,
            kinds: None,
            level: None,
            enabled: None,
        })
        .send()
        .await
        .ok();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with((*o.world.db.pool.connect_options()).clone())
        .await
        .unwrap();
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let app = TestApp::new(
        orch_db::Db::from_pool(pool.clone()),
        o.world.app.clock.clone(),
        registry,
    );
    for (is_channel, id, token) in [
        (true, channel.id, o.ada.token.clone()),
        (false, destination.id, o.bea.token.clone()),
    ] {
        let caller = app.clone();
        let pending = async move {
            if is_channel {
                caller
                    .call(NotificationChannelTest, &[&id])
                    .token(&token)
                    .body(&NotificationChannelTestBody::default())
                    .send()
                    .await
                    .ok()
            } else {
                caller
                    .call(DestinationTest, &[&id])
                    .token(&token)
                    .send()
                    .await
                    .ok()
            }
        };
        let check_pool = async {
            tokio::time::timeout(RESPONSIVE, gates.0.notified())
                .await
                .expect("the endpoint was called");
            let available: i32 =
                tokio::time::timeout(RESPONSIVE, sqlx::query_scalar("SELECT 1").fetch_one(&pool))
                    .await
                    .expect("a pending send must release the connection")
                    .unwrap();
            assert_eq!(available, 1);
            gates.1.notify_one();
        };
        let (reply, ()) = tokio::join!(pending, check_pool);
        assert!(reply.ok);
    }
    server.abort();
    pool.close().await;
}

#[tokio::test]
async fn a_test_that_is_taken_clears_the_reason_and_names_the_person_for_a_channels_webhook() {
    let o = Outside::new().await;
    let far = FarSide::taking().await;
    let made = make_system(&o, &o.ada, &Outside::webhook("Hook", &far.url, None))
        .await
        .ok();
    sqlx::query("UPDATE notification_channels SET last_error = 'it was down' WHERE id = $1")
        .bind(made.id)
        .execute(&o.world.db.pool)
        .await
        .unwrap();
    assert!(try_it(&o, &o.ada, made.id, None).await.ok().ok);
    let seen = o.channels(&o.ada).await;
    assert_eq!(seen[0].last_error, None);
    assert!(seen[0].last_ok_at.is_some());
    // A webhook is told whom the message is for.
    let posted: serde_json::Value =
        serde_json::from_str(&far.hits.lock().unwrap()[0].body).unwrap();
    assert_eq!(posted["recipient"]["username"], "ada");
}

#[tokio::test]
async fn a_test_is_for_whoever_looks_after_a_channel_and_for_a_person_with_a_route_through_it() {
    let o = Outside::new().await;
    let far = FarSide::taking().await;
    let made = make_system(&o, &o.ada, &Outside::webhook("Hook", &far.url, None))
        .await
        .ok();
    // Dee has no route through it and does not look after it.
    let refused = try_it(&o, &o.dee, made.id, None).await.problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(far.count(), 0);
    // Once she has a route, the sample reaches her as her route reaches her; what she sees of the channel is no more.
    o.world
        .app
        .call(
            orch_api_contract::notifications::MeNotificationRouteSet,
            &[&made.id],
        )
        .token(&o.dee.token)
        .body(&orch_api_contract::notifications::MeNotificationRouteSetBody::default())
        .send()
        .await
        .ok();
    assert!(try_it(&o, &o.dee, made.id, None).await.ok().ok);
    let posted: serde_json::Value =
        serde_json::from_str(&far.hits.lock().unwrap()[0].body).unwrap();
    assert_eq!(posted["recipient"]["username"], "dee");
    // An address to send to is for whoever looks after the channel.
    let to = try_it(&o, &o.dee, made.id, Some("anybody@example.org"))
        .await
        .problem();
    assert_eq!(to.code, ErrorCode::RequestInvalid);
    assert_eq!(paths(&to), ["to"]);
}

#[tokio::test]
async fn a_mail_server_is_tried_by_sending_to_the_address_given_and_says_what_the_server_answered()
{
    let o = Outside::new().await;
    let made = make_system(&o, &o.ada, &Outside::mail_server("smtp.example"))
        .await
        .ok();
    let taken = try_it(&o, &o.ada, made.id, Some("ops@example.org"))
        .await
        .ok();
    assert!(taken.ok, "{taken:?}");
    assert!(
        sent()
            .iter()
            .any(|each| each.notifier == "email" && each.to["email"] == "ops@example.org")
    );
    let bounced = try_it(&o, &o.ada, made.id, Some(BOUNCING)).await.ok();
    assert!(!bounced.ok);
    assert!(bounced.error.as_deref().unwrap().contains("550 5.1.1"));
    // Without an address there is nobody to write to.
    let nobody = try_it(&o, &o.ada, made.id, None).await.ok();
    assert!(
        !nobody.ok
            || sent()
                .iter()
                .any(|each| each.to["email"] == "ada@example.org")
    );
}

#[tokio::test]
async fn the_channels_that_reach_a_person_are_the_systems_their_root_orgs_and_their_own() {
    let o = Outside::new().await;
    assert!(
        o.channels(&o.fay).await.is_empty(),
        "a system with no mail server has no such channel in the list"
    );
    let far = FarSide::taking().await;
    o.world
        .set("outbound.allowed_private_hosts", json!(["127.0.0.1"]))
        .await;
    let systems = make_system(&o, &o.ada, &Outside::mail_server("smtp.example"))
        .await
        .ok();
    let orgs = org_channel(&o).await;
    let own = make_own(&o, &o.cai, "Mine", &far.url, None).await.ok();

    let ids =
        |channels: &[NotificationChannel]| channels.iter().map(|each| each.id).collect::<Vec<_>>();
    assert_eq!(
        ids(&o.channels(&o.fay).await),
        [systems.id],
        "a stranger is reached by the system's alone"
    );
    assert_eq!(ids(&o.channels(&o.dee).await), [systems.id, orgs.id]);
    assert_eq!(
        ids(&o.channels(&o.eve).await),
        [systems.id, orgs.id],
        "a member of a department is a member of the root org"
    );
    assert_eq!(
        ids(&o.channels(&o.cai).await),
        [systems.id, orgs.id, own.id]
    );

    // A member reads what a channel is and nothing of how it is made.
    let seen = o.channels(&o.dee).await;
    assert!(
        seen.iter()
            .all(|each| !each.can_edit && each.secrets_set.is_empty())
    );
    assert!(
        seen.iter()
            .all(|each| each.config == json!({}) && each.last_error.is_none())
    );
    // Its admin reads the fields.
    let admins = o.channels(&o.bea).await;
    assert_eq!(admins[1].config["host"], "smtp.acme.example");
    assert!(admins[1].can_edit && !admins[0].can_edit);
    // A system admin has the system's with their fields.
    let mine = o.channels(&o.ada).await;
    assert_eq!(mine[0].config["host"], "smtp.example");
    assert!(mine[0].can_edit);
}

#[tokio::test]
async fn where_an_org_may_not_use_the_systems_mail_server_the_system_has_no_such_channel_for_its_people()
 {
    let o = Outside::new().await;
    let systems = make_system(&o, &o.ada, &Outside::mail_server("smtp.example"))
        .await
        .ok();
    let orgs = org_channel(&o).await;
    let ids =
        |channels: &[NotificationChannel]| channels.iter().map(|each| each.id).collect::<Vec<_>>();
    assert_eq!(
        ids(&o.channels(&o.fay).await),
        [systems.id],
        "mail.shared is on as shipped"
    );

    o.world.set("mail.shared", json!(false)).await;
    assert_eq!(
        ids(&o.channels(&o.fay).await),
        Vec::<Id>::new(),
        "she belongs to no org that has a server of its own"
    );
    assert_eq!(
        ids(&o.channels(&o.dee).await),
        [orgs.id],
        "her org's own mail server is a way out and the system's is not"
    );
    assert_eq!(
        ids(&o.channels(&o.ada).await),
        [systems.id],
        "the admin who set it up reads it"
    );
}

#[tokio::test]
async fn the_channels_of_a_root_org_are_listed_to_look_after_for_whoever_may_and_for_nobody_else() {
    let o = Outside::new().await;
    let systems = make_system(&o, &o.ada, &Outside::mail_server("smtp.example"))
        .await
        .ok();
    let orgs = org_channel(&o).await;
    let listed = |who: &Who, org: Id| {
        let o = &o;
        let token = who.token.clone();
        async move {
            o.world
                .app
                .call(NotificationChannelList, &[])
                .token(&token)
                .query(&NotificationChannelListQuery { org_id: Some(org) })
                .send()
                .await
        }
    };
    let theirs = listed(&o.bea, o.acme).await.ok();
    assert_eq!(
        theirs.iter().map(|each| each.id).collect::<Vec<_>>(),
        [orgs.id]
    );
    assert_eq!(theirs[0].config["host"], "smtp.acme.example");
    assert_ne!(theirs[0].id, systems.id);
    for who in [&o.cai, &o.dee, &o.fay] {
        assert_eq!(
            listed(who, o.acme).await.problem().code,
            ErrorCode::AuthzDenied
        );
    }
    assert_eq!(
        listed(&o.bea, o.research).await.problem().code,
        ErrorCode::RequestInvalid
    );
}

#[tokio::test]
async fn the_kinds_of_channel_this_system_has_are_listed_with_their_texts_in_the_callers_language()
{
    let o = Outside::new().await;
    let listed = o
        .world
        .app
        .call(NotifierList, &[])
        .token(&o.cai.token)
        .send()
        .await
        .ok();
    let names: Vec<_> = listed.iter().map(|each| each.name.as_str()).collect();
    assert_eq!(names, ["email", "push", "telegram", "webhook"]);
    let by = |name: &str| listed.iter().find(|each| each.name == name).unwrap();
    assert!(by("telegram").links && by("telegram").single && !by("webhook").links);
    assert_eq!(
        by("webhook").targets.len(),
        2,
        "a webhook is both a channel and a destination"
    );
    assert!(
        by("webhook")
            .fields
            .iter()
            .any(|field| field.name == "secret")
    );
    assert_eq!(
        by("email").templates.digest.subject.as_deref(),
        Some("{{count}} unread")
    );

    sqlx::query("UPDATE users SET locale = 'zh-CN' WHERE id = $1")
        .bind(o.cai.id())
        .execute(&o.world.db.pool)
        .await
        .unwrap();
    let chinese = o
        .world
        .app
        .call(NotifierList, &[])
        .token(&o.cai.token)
        .send()
        .await
        .ok();
    let email = chinese.iter().find(|each| each.name == "email").unwrap();
    assert_eq!(
        email.templates.digest.subject.as_deref(),
        Some("{{count}} 条未读")
    );
}

#[tokio::test]
async fn nobody_who_is_not_signed_in_sees_or_sets_up_a_way_out() {
    let o = Outside::new().await;
    let id = orch_types::new_id();
    let signed_out = [
        o.world.app.call(NotifierList, &[]).send().await.problem(),
        o.world
            .app
            .call(NotificationChannelList, &[])
            .query(&NotificationChannelListQuery::default())
            .send()
            .await
            .problem(),
        o.world
            .app
            .call(AdminNotificationChannelCreate, &[])
            .body(&Outside::mail_server("smtp.example"))
            .send()
            .await
            .problem(),
        o.world
            .app
            .call(OrgNotificationChannelCreate, &[&o.acme])
            .body(&Outside::mail_server("smtp.example"))
            .send()
            .await
            .problem(),
        o.world
            .app
            .call(MeNotificationChannelCreate, &[])
            .body(&MeNotificationChannelCreateBody {
                name: "x".to_owned(),
                config: WebhookConfig {
                    url: "https://x.example".to_owned(),
                },
                secrets: None,
                templates: None,
            })
            .send()
            .await
            .problem(),
        o.world
            .app
            .call(NotificationChannelUpdate, &[&id])
            .body(&NotificationChannelUpdateBody::default())
            .send()
            .await
            .problem(),
        o.world
            .app
            .call(NotificationChannelDelete, &[&id])
            .send()
            .await
            .problem(),
        o.world
            .app
            .call(NotificationChannelTest, &[&id])
            .body(&NotificationChannelTestBody::default())
            .send()
            .await
            .problem(),
    ];
    for problem in signed_out {
        assert_eq!(problem.code, ErrorCode::AuthRequired);
    }
}
