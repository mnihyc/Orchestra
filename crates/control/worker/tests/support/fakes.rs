//! A notifier that answers as it is told to and remembers what it was given.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(dead_code, reason = "each test file uses some of what is here")]

use async_trait::async_trait;
use orch_notifier_contract::delivery::{Config, DeliverError, Delivery, LinkPoll};
use orch_notifier_contract::descriptor::{NotifierDescriptor, Owner, Reaches, Target};
use orch_notifier_contract::notifier::Notifier;
use orch_notifier_contract::templates::{BodyKind, TemplatePair, TemplateText};
use orch_types::{Field, FieldType, Json};
use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// A notifier whose answers are scripted.
pub struct Scripted {
    name: String,
    reaches: Reaches,
    body: BodyKind,
    results: Mutex<VecDeque<Result<(), DeliverError>>>,
    delivered: Mutex<Vec<Delivery>>,
    checked: Mutex<Result<Json, DeliverError>>,
    polls: Mutex<VecDeque<Result<LinkPoll, DeliverError>>>,
    polled: Mutex<Vec<Option<String>>>,
    working: AtomicUsize,
    at_once: AtomicUsize,
    pause: Mutex<Duration>,
}

impl Scripted {
    /// A notifier named `name` that reaches people as `reaches` says, and takes
    /// everything until it is told otherwise.
    pub fn new(name: &str, reaches: Reaches) -> std::sync::Arc<Self> {
        Self::with_body(name, reaches, BodyKind::Text)
    }

    /// The same, for a medium that is JSON.
    pub fn with_body(name: &str, reaches: Reaches, body: BodyKind) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            name: name.to_owned(),
            reaches,
            body,
            results: Mutex::default(),
            delivered: Mutex::default(),
            checked: Mutex::new(Ok(Json::Null)),
            polls: Mutex::default(),
            polled: Mutex::default(),
            working: AtomicUsize::new(0),
            at_once: AtomicUsize::new(0),
            pause: Mutex::new(Duration::ZERO),
        })
    }

    /// What the next deliveries answer, in turn; after them it takes everything.
    pub fn answer(&self, answers: impl IntoIterator<Item = Result<(), DeliverError>>) {
        self.results.lock().unwrap().extend(answers);
    }

    /// What the next readings of who followed a link answer.
    pub fn followers(&self, polls: impl IntoIterator<Item = Result<LinkPoll, DeliverError>>) {
        self.polls.lock().unwrap().extend(polls);
    }

    /// What a check of a channel answers from now on.
    pub fn check_answers(&self, answer: Result<Json, DeliverError>) {
        *self.checked.lock().unwrap() = answer;
    }

    /// How long a delivery, or a reading of who followed a link, takes.
    pub fn takes(&self, pause: Duration) {
        *self.pause.lock().unwrap() = pause;
    }

    /// What it was given to deliver, in order.
    pub fn delivered(&self) -> Vec<Delivery> {
        self.delivered.lock().unwrap().clone()
    }

    /// How many deliveries it has had.
    pub fn deliveries(&self) -> usize {
        self.delivered.lock().unwrap().len()
    }

    /// The cursors it was asked who followed a link since.
    pub fn polled(&self) -> Vec<Option<String>> {
        self.polled.lock().unwrap().clone()
    }

    /// The most deliveries it had in hand at one moment.
    pub fn most_at_once(&self) -> usize {
        self.at_once.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Notifier for Scripted {
    fn describe(&self) -> NotifierDescriptor {
        let text = |subject: Option<&str>, body: &str| TemplateText {
            subject: subject.map(str::to_owned),
            body: body.to_owned(),
        };
        let json = self.body == BodyKind::Json;
        let mut token = Field::new("token", "Token", FieldType::Secret);
        token.required = false;
        let mut url = Field::new("url", "URL", FieldType::Text);
        url.required = false;
        NotifierDescriptor {
            name: self.name.clone(),
            title: self.name.clone(),
            summary: "A notifier for tests.".to_owned(),
            targets: vec![Target::Person, Target::Destination],
            fields: vec![url, token],
            owners: vec![Owner::System, Owner::Org, Owner::User],
            single: false,
            reaches: self.reaches,
            body: self.body,
            max_chars: None,
            templates: BTreeMap::from([(
                "*".to_owned(),
                TemplatePair {
                    single: text(
                        Some("[{{org.name}}] {{title}}"),
                        if json {
                            "{\"text\": \"{{title}}\"}"
                        } else {
                            "{{title}}\n{{body}}\n{{url}}"
                        },
                    ),
                    digest: text(
                        Some("[{{org.name}}] {{count}} unread"),
                        if json {
                            "{\"count\": \"{{count}}\"}"
                        } else {
                            "{{#notifications}}{{title}}\n{{/notifications}}"
                        },
                    ),
                },
            )]),
            extensions: orch_plugin_contract::Extensions::new(),
        }
    }

    async fn check(&self, _: &Config) -> Result<Json, DeliverError> {
        self.checked.lock().unwrap().clone()
    }

    async fn deliver(&self, delivery: Delivery) -> Result<(), DeliverError> {
        let now = self.working.fetch_add(1, Ordering::SeqCst) + 1;
        self.at_once.fetch_max(now, Ordering::SeqCst);
        let pause = *self.pause.lock().unwrap();
        if !pause.is_zero() {
            tokio::time::sleep(pause).await;
        }
        self.delivered.lock().unwrap().push(delivery);
        let answer = self.results.lock().unwrap().pop_front().unwrap_or(Ok(()));
        self.working.fetch_sub(1, Ordering::SeqCst);
        answer
    }

    fn link_url(&self, _: &Config, code: &str) -> Option<String> {
        self.reaches
            .links_people()
            .then(|| format!("https://chat.example/start?code={code}"))
    }

    async fn poll_links(
        &self,
        _: &Config,
        cursor: Option<String>,
        _: Duration,
    ) -> Result<LinkPoll, DeliverError> {
        self.polled.lock().unwrap().push(cursor);
        let pause = *self.pause.lock().unwrap();
        if !pause.is_zero() {
            tokio::time::sleep(pause).await;
        }
        self.polls
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Ok(LinkPoll::default()))
    }
}
