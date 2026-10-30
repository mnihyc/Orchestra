//! What a kind of channel does: describe itself, check a channel when it is saved, deliver, and, for a kind that links people, make a link and ask who followed it.
//!
//! The worker hosts the notifiers. **The engine decides who is told what**, and writes the rows; **the worker
//! renders**, with every value escaped for the notifier's `body`; **the notifier sends**, and never reads the
//! database. Secret fields are unsealed in the worker for the length of one call and given to the notifier
//! with it.
//!
//! **Linking is one procedure for every notifier whose `reaches` is `link`.** The person asks for a link: the
//! server makes a code of 128 random bits, keeps its hash on the route for ten minutes, and gives out
//! [`Notifier::link_url`]. While a code of a channel is waiting, and only then, the worker calls
//! [`Notifier::poll_links`] for that channel, with the cursor it keeps on the channel's row. A code that comes
//! back and is still good gives the route its address and its label. Nothing listens: a notifier is asked, so
//! no address of the system has to be reachable from outside.
//!
//! An error that says `retryable` is retried, or after the time the far side named; one that does not fails the
//! row at once; `gone` switches the route or the destination off, or removes a device's subscription
//! ([`crate::delivery::after_delivery`]). An installed notifier opens its own connections, so it is given the
//! list of private hosts it may reach with every call ([`crate::delivery::Config`]), and refuses the others
//! ([`crate::outbound`]).

use std::time::Duration;

use async_trait::async_trait;
use orch_types::Json;

use crate::delivery::{Config, DeliverError, Delivery, LinkPoll};
use crate::descriptor::NotifierDescriptor;

/// One implementation per notifier, used by the worker.
#[async_trait]
pub trait Notifier: Send + Sync + 'static {
    /// What it is: its identity, whom it reaches, its fields, and the texts it ships.
    fn describe(&self) -> NotifierDescriptor;

    /// Looks at the values of a channel or a destination when they are saved or tested, and returns what the
    /// far side says of itself, to be kept with them (a bot's name). Delivers nothing. An error that is not
    /// `retryable` refuses the save; one that is leaves it saved, with the error shown
    /// ([`crate::delivery::on_check`]).
    async fn check(&self, config: &Config) -> Result<Json, DeliverError>;

    /// One delivery. `Ok` means the far side took it.
    async fn deliver(&self, delivery: Delivery) -> Result<(), DeliverError>;

    /// Where `reaches` is `link`: the address a person opens to be linked, for a code. `None` for a notifier
    /// that does not link people.
    fn link_url(&self, config: &Config, code: &str) -> Option<String> {
        let _ = (config, code);
        None
    }

    /// Where `reaches` is `link`: who followed such an address since `cursor`. It may wait up to `wait` before
    /// it answers that nobody did.
    async fn poll_links(
        &self,
        config: &Config,
        cursor: Option<String>,
        wait: Duration,
    ) -> Result<LinkPoll, DeliverError> {
        let _ = (config, cursor, wait);
        Err(DeliverError::unsupported("link people"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delivery::LinkStarted;
    use crate::templates::{Item, Template, Values};
    use futures::executor::block_on;
    use serde_json::json;
    use std::sync::Mutex;

    /// A notifier that links people by a code.
    struct Bot {
        sent: Mutex<Vec<String>>,
        followers: Mutex<Vec<LinkStarted>>,
    }

    fn descriptor() -> NotifierDescriptor {
        serde_json::from_value(json!({
            "name": "bot", "title": "A bot", "summary": "Writes to the people who started a chat with it.",
            "targets": ["person"], "fields": [], "owners": ["system"], "single": true, "reaches": "link", "body": "text",
            "max_chars": 4096,
            "templates": { "*": { "single": { "subject": null, "body": "{{title}}" },
                                  "digest": { "subject": null, "body": "{{#notifications}}{{title}}\n{{/notifications}}" } } }
        }))
        .unwrap()
    }

    #[async_trait]
    impl Notifier for Bot {
        fn describe(&self) -> NotifierDescriptor {
            descriptor()
        }
        async fn check(&self, config: &Config) -> Result<Json, DeliverError> {
            match config.secret("bot_token") {
                Some(token) if token.expose() == "good" => {
                    Ok(json!({ "bot_username": "orch_bot" }))
                }
                Some(_) => Err(DeliverError::refused(
                    "unauthorized",
                    "the token is not valid",
                )),
                None => Err(DeliverError::refused("missing", "no token")),
            }
        }
        async fn deliver(&self, delivery: Delivery) -> Result<(), DeliverError> {
            if delivery.to.get("chat_id").is_none() {
                return Err(DeliverError::gone(
                    "no_chat",
                    "the person has not started the bot",
                ));
            }
            self.sent.lock().unwrap().push(delivery.body);
            Ok(())
        }
        fn link_url(&self, _: &Config, code: &str) -> Option<String> {
            Some(format!("https://t.me/orch_bot?start={code}"))
        }
        async fn poll_links(
            &self,
            _: &Config,
            cursor: Option<String>,
            _: Duration,
        ) -> Result<LinkPoll, DeliverError> {
            let started = std::mem::take(&mut *self.followers.lock().unwrap());
            let next = cursor.map_or(1, |c| c.parse::<i64>().unwrap_or(0) + 1);
            Ok(LinkPoll {
                started,
                cursor: Some(next.to_string()),
            })
        }
    }

    /// A notifier that links nobody: it takes the defaults.
    struct Mailer;

    #[async_trait]
    impl Notifier for Mailer {
        fn describe(&self) -> NotifierDescriptor {
            let mut descriptor = descriptor();
            descriptor.reaches = crate::descriptor::Reaches::Email;
            descriptor
        }
        async fn check(&self, _: &Config) -> Result<Json, DeliverError> {
            Ok(Json::Null)
        }
        async fn deliver(&self, _: Delivery) -> Result<(), DeliverError> {
            Ok(())
        }
    }

    fn delivery(to: Json, body: &str) -> Delivery {
        serde_json::from_value(json!({
            "id": "0199abcd-1234-7000-8000-0000000000d1", "attempt": 1, "config": {}, "to": to, "recipient": null,
            "language": "en", "subject": null, "body": body, "notifications": []
        }))
        .unwrap()
    }

    #[test]
    fn a_notifier_that_links_people_makes_a_link_for_a_code_and_says_who_followed_it() {
        let bot = Bot {
            sent: Mutex::new(vec![]),
            followers: Mutex::new(vec![LinkStarted {
                code: "c0de".into(),
                address: json!({ "chat_id": 42 }),
                label: "Dana".into(),
            }]),
        };
        let config = Config::default();
        assert_eq!(
            bot.link_url(&config, "c0de").as_deref(),
            Some("https://t.me/orch_bot?start=c0de")
        );
        let first = block_on(bot.poll_links(&config, None, Duration::from_secs(25))).unwrap();
        assert_eq!(first.started.len(), 1);
        assert_eq!(first.cursor.as_deref(), Some("1"));
        let second =
            block_on(bot.poll_links(&config, first.cursor, Duration::from_secs(25))).unwrap();
        assert!(second.started.is_empty(), "nobody else followed it");
        assert_eq!(second.cursor.as_deref(), Some("2"));
        assert!(bot.describe().reaches.links_people());
    }

    #[test]
    fn a_notifier_that_does_not_link_people_has_no_link_and_says_it_does_not() {
        let mailer = Mailer;
        assert_eq!(mailer.link_url(&Config::default(), "code"), None);
        let error =
            block_on(mailer.poll_links(&Config::default(), None, Duration::ZERO)).unwrap_err();
        assert_eq!(error.code, "unsupported");
        assert!(!mailer.describe().reaches.links_people());
    }

    #[test]
    fn what_a_check_finds_decides_the_save_and_a_delivery_to_a_gone_target_says_so() {
        use crate::delivery::{Saved, on_check};
        let bot = Bot {
            sent: Mutex::new(vec![]),
            followers: Mutex::new(vec![]),
        };
        let with_token = |token: &str| {
            let mut config = Config::default();
            config
                .secrets
                .insert("bot_token".into(), orch_plugin_contract::Secret::new(token));
            config
        };
        assert_eq!(
            on_check(block_on(bot.check(&with_token("good")))),
            Saved::Kept(json!({ "bot_username": "orch_bot" }))
        );
        assert!(matches!(
            on_check(block_on(bot.check(&with_token("bad")))),
            Saved::Refused(_)
        ));
        assert_eq!(
            block_on(bot.deliver(delivery(json!({ "chat_id": 42 }), "hello"))),
            Ok(())
        );
        assert_eq!(bot.sent.lock().unwrap().as_slice(), ["hello"]);
        let gone = block_on(bot.deliver(delivery(json!({}), "hello"))).unwrap_err();
        assert!(gone.gone);
    }

    #[test]
    fn the_worker_renders_the_text_and_the_notifier_is_given_it_finished() {
        let texts = descriptor().templates;
        let single = &texts["*"].single;
        let template = Template::parse(&single.body).unwrap();
        let values = Values {
            items: vec![Item {
                title: "Turn finished".into(),
                ..Item::default()
            }],
            recipient_name: "Bo".into(),
            org_name: "Acme".into(),
        };
        let body = template.render(&values, crate::templates::Escape::Text);
        let bot = Bot {
            sent: Mutex::new(vec![]),
            followers: Mutex::new(vec![]),
        };
        block_on(bot.deliver(delivery(json!({ "chat_id": 1 }), &body))).unwrap();
        assert_eq!(bot.sent.lock().unwrap().as_slice(), ["Turn finished"]);
    }
}
