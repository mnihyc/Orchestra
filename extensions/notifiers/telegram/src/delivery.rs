//! One notification, or several together, written to a person's chat within the length a message takes; a person who stopped the bot found at the next delivery and their route switched off.
//!
//! [`TelegramNotifier`] is the kind of channel that is a Telegram bot. A route
//! through it holds the chat the bot may write to, which the person gave it by
//! pressing Start ([`crate::linking`]). The worker hands over the text finished
//! and no longer than a message takes; the notifier writes it, as plain text, to
//! that chat. Telegram can answer in four ways that matter: it took the message;
//! it asks the bot to slow down, naming how long (the delivery is tried again
//! after that); it is down or unreachable (tried again by the schedule); or it
//! says the chat is gone, because the person blocked the bot or their account is
//! gone, which is the end of the route.
//!
//! It reaches `api.telegram.org` and nothing else.

use crate::bot::{API_BASE, Bot, MAX_MESSAGE_CHARS};
use crate::linking;
use async_trait::async_trait;
use orch_notifier_contract::delivery::{Config, DeliverError, Delivery, LinkPoll};
use orch_notifier_contract::descriptor::{NotifierDescriptor, Owner, Reaches, Target};
use orch_notifier_contract::notifier::Notifier;
use orch_notifier_contract::templates::{BodyKind, TemplatePair, TemplateText};
use orch_plugin_contract::Extensions;
use orch_types::{Field, FieldType, Json};
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Duration;
use url::Url;

/// The name a channel gives as its kind to be a Telegram bot.
pub const NAME: &str = "telegram";

/// The member of the values that holds the bot's name, which is read from
/// Telegram when the token is saved.
pub const BOT_USERNAME: &str = "bot_username";

/// The texts a bot sends until its channel has its own.
#[must_use]
pub fn shipped_texts() -> BTreeMap<String, TemplatePair> {
    let pair = TemplatePair {
        single: TemplateText {
            subject: None,
            body: "{{title}}\n{{body}}\n{{url}}".to_owned(),
        },
        digest: TemplateText {
            subject: None,
            body: "{{#notifications}}{{time}}  {{title}}\n{{url}}\n{{/notifications}}".to_owned(),
        },
    };
    BTreeMap::from([("*".to_owned(), pair)])
}

/// What the notifier declares of itself.
#[must_use]
pub fn descriptor() -> NotifierDescriptor {
    let mut token = Field::new("bot_token", "Bot token", FieldType::Secret);
    token.help = Some("The token BotFather gave the bot.".to_owned());
    NotifierDescriptor {
        name: NAME.to_owned(),
        title: "Telegram".to_owned(),
        summary: "Messages from a bot to the people who started a chat with it.".to_owned(),
        targets: vec![Target::Person],
        fields: vec![token],
        owners: vec![Owner::System, Owner::Org],
        single: true,
        reaches: Reaches::Link,
        body: BodyKind::Text,
        max_chars: Some(MAX_MESSAGE_CHARS),
        templates: shipped_texts(),
        extensions: Extensions::new(),
    }
}

/// The notifier of Telegram bots.
#[derive(Debug, Clone)]
pub struct TelegramNotifier {
    base: Url,
}

impl Default for TelegramNotifier {
    fn default() -> Self {
        Self::new()
    }
}

impl TelegramNotifier {
    /// The notifier of bots at `api.telegram.org`.
    #[must_use]
    #[expect(
        clippy::expect_used,
        reason = "API_BASE is a constant address, and a test parses it"
    )]
    pub fn new() -> Self {
        Self {
            base: Url::parse(API_BASE).expect("API_BASE is an address"),
        }
    }

    /// The notifier of bots at a stand-in for the Bot API, for tests. The program
    /// that composes the worker never calls this: a system reaches
    /// `api.telegram.org` and nothing else.
    #[must_use]
    pub fn against(base: Url) -> Self {
        Self { base }
    }

    fn bot(&self, config: &Config) -> Result<Bot, DeliverError> {
        let token = config
            .secret("bot_token")
            .filter(|token| !token.is_empty())
            .ok_or_else(|| DeliverError::refused("invalid_config", "bot_token is required"))?;
        Ok(Bot::new(Bot::client()?, &self.base, token.expose()))
    }
}

#[async_trait]
impl Notifier for TelegramNotifier {
    fn describe(&self) -> NotifierDescriptor {
        descriptor()
    }

    async fn check(&self, config: &Config) -> Result<Json, DeliverError> {
        let me = self.bot(config)?.get_me().await?;
        let username = me.username.filter(|name| !name.is_empty()).ok_or_else(|| {
            DeliverError::refused("no_username", "Telegram knows this bot by no username")
        })?;
        Ok(json!({ BOT_USERNAME: username }))
    }

    async fn deliver(&self, delivery: Delivery) -> Result<(), DeliverError> {
        let chat = delivery
            .to
            .get("chat_id")
            .and_then(Json::as_i64)
            .ok_or_else(|| DeliverError::gone("no_chat", "the person has not started the bot"))?;
        self.bot(&delivery.config)?
            .send_message(chat, &delivery.body)
            .await
    }

    fn link_url(&self, config: &Config, code: &str) -> Option<String> {
        linking::link_url(config.text(BOT_USERNAME)?, code)
    }

    async fn poll_links(
        &self,
        config: &Config,
        cursor: Option<String>,
        wait: Duration,
    ) -> Result<LinkPoll, DeliverError> {
        linking::poll(&self.bot(config)?, cursor, wait).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_notifier_declares_is_consistent_and_a_bot_is_told_to_write_at_most_what_a_message_takes()
     {
        let declared = descriptor();
        assert_eq!(declared.check(), Ok(()));
        assert!(declared.is_channel() && !declared.is_destination() && declared.is_single());
        assert!(
            declared.offered_to(Owner::System)
                && declared.offered_to(Owner::Org)
                && !declared.offered_to(Owner::User)
        );
        assert_eq!(
            (declared.reaches, declared.max_chars),
            (Reaches::Link, Some(4096))
        );
        assert!(declared.reaches.links_people() && declared.reaches.stores_an_address());
        let fields: Vec<_> = declared
            .fields
            .iter()
            .map(|field| (field.name.as_str(), field.kind.is_secret()))
            .collect();
        assert_eq!(fields, [("bot_token", true)]);
    }

    #[test]
    fn the_bot_api_is_at_a_valid_address_with_no_path_of_its_own() {
        let notifier = TelegramNotifier::new();
        assert_eq!(notifier.base.as_str(), "https://api.telegram.org/");
        assert_eq!(notifier.base.scheme(), "https");
    }

    #[test]
    fn the_link_is_made_from_the_name_the_bot_has_been_read_to_have() {
        let notifier = TelegramNotifier::new();
        let named = Config {
            values: json!({ "bot_username": "orch_bot" }),
            ..Config::default()
        };
        assert_eq!(
            notifier.link_url(&named, "c0de").as_deref(),
            Some("https://t.me/orch_bot?start=c0de")
        );
        assert_eq!(
            notifier.link_url(&Config::default(), "c0de"),
            None,
            "before the token was saved and read"
        );
    }
}
