//! The ways out of the application as the tests of channels, routes and destinations have them: notifiers that answer as they are told, a far side that records what it is posted, and a world with the people who stand in different places towards them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(dead_code, reason = "each test file uses some of what is here")]

use crate::places::{self, Who, admin, join, make_org, person};
use crate::support::World;
use async_trait::async_trait;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use orch_api_contract::common::Problem;
use orch_api_contract::notifications::{
    NotificationChannel, NotificationChannelCreateBody, NotificationChannelList,
    NotificationChannelListQuery, OrgNotificationChannelCreate,
};
use orch_db::secrets::MasterKey;
use orch_notifier_contract::delivery::{Config, DeliverError, Delivery, LinkPoll};
use orch_notifier_contract::descriptor::NotifierDescriptor;
use orch_notifier_contract::notifier::Notifier;
use orch_types::{Id, Json};
use orch_worker_core::channels::Notifiers;
use serde_json::json;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// The host of a mail server whose password the far side refuses for good.
pub const REFUSING_HOST: &str = "wrong-password.example";
/// The host of a mail server that cannot be reached today.
pub const DOWN_HOST: &str = "down.example";
/// The token of a bot that Telegram does not know.
pub const UNKNOWN_TOKEN: &str = "unknown-token";
/// The address a mail server bounces.
pub const BOUNCING: &str = "gone@example.org";
/// The key every test seals its secrets with.
pub const MASTER: [u8; 32] = [7; 32];

/// What a scripted notifier was asked to deliver.
#[derive(Debug, Clone)]
pub struct Sent {
    /// The notifier.
    pub notifier: String,
    /// Where the person is on it.
    pub to: Json,
    /// The finished text.
    pub body: String,
    /// The language it is in.
    pub language: String,
    /// The names of the secrets the delivery was given.
    pub secrets: Vec<String>,
    /// The private hosts the channel was held to; none where it was not.
    pub allowed: Option<Vec<String>>,
}

static SENT: Mutex<Vec<Sent>> = Mutex::new(Vec::new());

/// Everything the scripted notifiers delivered in this test binary.
pub fn sent() -> Vec<Sent> {
    SENT.lock().unwrap().clone()
}

/// A notifier that answers as the values of the channel tell it to.
struct Scripted {
    descriptor: NotifierDescriptor,
}

fn field(name: &str, kind: &str, required: bool) -> Json {
    json!({ "name": name, "label": name, "type": kind, "required": required, "help": null,
            "choices": null, "default": null, "max_bytes": null })
}

fn texts() -> Json {
    json!({ "*": {
        "single": { "subject": "[{{org.name}}] {{title}}", "body": "{{title}}\n{{body}}\n{{url}}" },
        "digest": { "subject": "{{count}} unread",
                    "body": "{{#notifications}}{{time}}  {{title}}\n{{url}}\n{{/notifications}}" } },
        "zh-CN": {
        "single": { "subject": "[{{org.name}}] {{title}}", "body": "{{title}}\n{{body}}\n{{url}}" },
        "digest": { "subject": "{{count}} 条未读",
                    "body": "{{#notifications}}{{time}}  {{title}}\n{{url}}\n{{/notifications}}" } } })
}

fn scripted(name: &str, fields: Vec<Json>, owners: &[&str], reaches: &str) -> Scripted {
    let descriptor: NotifierDescriptor = serde_json::from_value(json!({
        "name": name, "title": name, "summary": format!("A scripted {name}."),
        "targets": ["person"], "fields": fields, "owners": owners, "single": true,
        "reaches": reaches, "body": "text", "max_chars": null, "templates": texts()
    }))
    .unwrap();
    Scripted { descriptor }
}

#[async_trait]
impl Notifier for Scripted {
    fn describe(&self) -> NotifierDescriptor {
        self.descriptor.clone()
    }

    async fn check(&self, config: &Config) -> Result<Json, DeliverError> {
        let host = config.text("host").unwrap_or_default();
        if host == REFUSING_HOST {
            return Err(DeliverError::refused(
                "535",
                "535 5.7.8 authentication failed: wrong password",
            ));
        }
        if host == DOWN_HOST {
            return Err(DeliverError::temporary(
                "unreachable",
                "connection to down.example refused",
            ));
        }
        match config.secret("bot_token") {
            Some(token) if token.expose() == UNKNOWN_TOKEN => {
                return Err(DeliverError::refused("401", "Unauthorized"));
            }
            Some(_) => return Ok(json!({ "bot_username": "acme_orch_bot" })),
            None => {}
        }
        Ok(Json::Null)
    }

    async fn deliver(&self, delivery: Delivery) -> Result<(), DeliverError> {
        SENT.lock().unwrap().push(Sent {
            notifier: self.descriptor.name.clone(),
            to: delivery.to.clone(),
            body: delivery.body.clone(),
            language: delivery.language.clone(),
            secrets: delivery.config.secrets.keys().cloned().collect(),
            allowed: delivery.config.allowed_private_hosts.clone(),
        });
        if delivery.to["email"] == BOUNCING {
            return Err(DeliverError::refused("550", "550 5.1.1 no such user"));
        }
        Ok(())
    }

    fn link_url(&self, config: &Config, code: &str) -> Option<String> {
        let bot = config.text("bot_username")?;
        Some(format!("https://t.me/{bot}?start={code}"))
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

/// The notifiers the tests of this binary have: a mail server, a bot and push that answer as they are
/// told, and the real webhook.
pub fn notifiers() -> Notifiers {
    let mut notifiers = Notifiers::new();
    notifiers
        .register(Arc::new(scripted(
            "email",
            vec![
                field("host", "text", true),
                field("port", "text", false),
                field("password", "secret", true),
                field("from", "text", true),
            ],
            &["system", "org"],
            "email",
        )))
        .unwrap();
    notifiers
        .register(Arc::new(scripted(
            "telegram",
            vec![field("bot_token", "secret", true)],
            &["system", "org"],
            "link",
        )))
        .unwrap();
    let mut push = scripted("push", vec![], &["system"], "devices");
    push.descriptor.single = true;
    notifiers.register(Arc::new(push)).unwrap();
    notifiers
        .register(Arc::new(orch_notifier_webhook::WebhookNotifier::new()))
        .unwrap();
    notifiers
}

/// Gives the requests of this test binary the notifiers and the key they seal with.
pub fn install() {
    orch_api_core::routes::notifications::install_notifiers(notifiers());
    orch_api_core::routes::install_master_key(&MasterKey::new(MASTER));
}

/// One post a far side took.
#[derive(Debug, Clone)]
pub struct Hit {
    /// What it was posted.
    pub body: String,
    /// The signature header, where there was one.
    pub signature: Option<String>,
}

/// A far side on this machine that takes posts and answers as it was told.
pub struct FarSide {
    /// Its address, a web address on a loopback.
    pub url: String,
    /// What it was posted.
    pub hits: Arc<Mutex<Vec<Hit>>>,
}

struct Answering {
    status: StatusCode,
    text: String,
    hits: Arc<Mutex<Vec<Hit>>>,
}

async fn take(
    State(answering): State<Arc<Answering>>,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, String) {
    answering.hits.lock().unwrap().push(Hit {
        body: String::from_utf8_lossy(&body).into_owned(),
        signature: headers
            .get("x-orch-signature")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
    });
    (answering.status, answering.text.clone())
}

impl FarSide {
    /// A far side that answers every post with `status` and `text`.
    pub async fn answering(status: u16, text: &str) -> Self {
        let hits = Arc::new(Mutex::new(Vec::new()));
        let answering = Arc::new(Answering {
            status: StatusCode::from_u16(status).unwrap(),
            text: text.to_owned(),
            hits: Arc::clone(&hits),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/hook", listener.local_addr().unwrap());
        let router = axum::Router::new().fallback(take).with_state(answering);
        tokio::spawn(async move {
            axum::serve(listener, router).await.ok();
        });
        Self { url, hits }
    }

    /// A far side that takes everything.
    pub async fn taking() -> Self {
        Self::answering(200, "ok").await
    }

    /// How many posts it took.
    pub fn count(&self) -> usize {
        self.hits.lock().unwrap().len()
    }
}

/// The people and the place of the tests of ways out: Ada is the admin of the system, Bea is the first owner and
/// admin of the org Acme and Cai is a developer of it, Dee is a viewer of it, Eve is a member of a department of
/// it and Fay is a stranger.
pub struct Outside {
    pub world: World,
    pub ada: Who,
    pub bea: Who,
    pub cai: Who,
    pub dee: Who,
    pub eve: Who,
    pub fay: Who,
    /// The root org Acme.
    pub acme: Id,
    /// A department of it.
    pub research: Id,
}

impl Outside {
    pub async fn new() -> Self {
        let world = World::new().await;
        install();
        let ada = admin(&world, "ada").await;
        let bea = person(&world, "bea").await;
        let cai = person(&world, "cai").await;
        let dee = person(&world, "dee").await;
        let eve = person(&world, "eve").await;
        let fay = person(&world, "fay").await;
        let org = make_org(&world, &bea, "Acme", "acme").await;
        let acme = org.base.id;
        join(&world, acme, acme, &cai, "developer", &bea).await;
        join(&world, acme, acme, &dee, "viewer", &bea).await;
        let research = places::make_department(&world, &bea, acme, "Research", "research")
            .await
            .base
            .id;
        join(&world, acme, research, &eve, "developer", &bea).await;
        Self {
            world,
            ada,
            bea,
            cai,
            dee,
            eve,
            fay,
            acme,
            research,
        }
    }

    /// The body that sets up a webhook channel pointing at `url`.
    pub fn webhook(name: &str, url: &str, secret: Option<&str>) -> NotificationChannelCreateBody {
        NotificationChannelCreateBody {
            kind: "webhook".to_owned(),
            name: name.to_owned(),
            config: json!({ "url": url }),
            secrets: secret.map(|secret| {
                std::collections::BTreeMap::from([("secret".to_owned(), secret.to_owned())])
            }),
            templates: None,
            enabled: None,
        }
    }

    /// The body that sets up a mail server at `host`.
    pub fn mail_server(host: &str) -> NotificationChannelCreateBody {
        NotificationChannelCreateBody {
            kind: "email".to_owned(),
            name: "Mail".to_owned(),
            config: json!({ "host": host, "from": "orch@example.org" }),
            secrets: Some(std::collections::BTreeMap::from([(
                "password".to_owned(),
                "hunter2-the-password".to_owned(),
            )])),
            templates: None,
            enabled: None,
        }
    }

    /// The body that sets up a bot.
    pub fn bot(token: &str) -> NotificationChannelCreateBody {
        NotificationChannelCreateBody {
            kind: "telegram".to_owned(),
            name: "Bot".to_owned(),
            config: json!({}),
            secrets: Some(std::collections::BTreeMap::from([(
                "bot_token".to_owned(),
                token.to_owned(),
            )])),
            templates: None,
            enabled: None,
        }
    }

    /// `by` sets up a channel of the root org `org`; the channel, or what refused it.
    pub async fn org_channel(
        &self,
        by: &Who,
        org: Id,
        body: &NotificationChannelCreateBody,
    ) -> Result<NotificationChannel, Problem> {
        let reply = self
            .world
            .app
            .call(OrgNotificationChannelCreate, &[&org])
            .token(&by.token)
            .body(body)
            .send()
            .await;
        reply.body
    }

    /// The channels that reach `by`, each with their own route.
    pub async fn channels(&self, by: &Who) -> Vec<NotificationChannel> {
        self.world
            .app
            .call(NotificationChannelList, &[])
            .token(&by.token)
            .query(&NotificationChannelListQuery::default())
            .send()
            .await
            .ok()
    }

    /// The row of a table that a test reads by one identifier.
    pub async fn count(&self, sql: &str, id: Id) -> i64 {
        sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
            .bind(id)
            .fetch_one(&self.world.db.pool)
            .await
            .unwrap()
    }
}

/// The OnceLock that keeps the notifiers is the process's; tests call `install` in every world.
#[allow(
    dead_code,
    reason = "kept for the tests that want to see it was called"
)]
static INSTALLED: OnceLock<()> = OnceLock::new();
