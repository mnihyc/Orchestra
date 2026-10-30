//! Recipient delivery and failure outcomes: taken, to be retried, or an address that is gone.
//!
//! [`EmailNotifier`] is the kind of channel that is a mail server. It sends a
//! finished text to the confirmed address of one person, which the worker
//! gives it as `to.email`, and says what became of it: the server took the
//! message; the server or the network cannot be relied on now (the delivery
//! is retried); the server refused for good (it fails); or the server says
//! that the recipient's mailbox does not exist or takes no mail (the address is
//! gone, and so is the person's route through this server).
//!
//! The message id is made from the delivery's identifier, which is the same
//! across the tries, so that a server or a reader that sees a message twice
//! can tell it is one.

use crate::render::{self, Composed};
use crate::smtp::{self, SmtpSettings, Step};
use async_trait::async_trait;
use lettre::message::{Mailbox, Message, MultiPart};
use orch_notifier_contract::delivery::{Config, DeliverError, Delivery};
use orch_notifier_contract::descriptor::{NotifierDescriptor, Owner, Reaches, Target};
use orch_notifier_contract::notifier::Notifier;
use orch_notifier_contract::templates::{BodyKind, TemplatePair, TemplateText};
use orch_plugin_contract::Extensions;
use orch_types::Json;
use std::collections::BTreeMap;

/// The name a channel gives as its kind to be a mail server.
pub const NAME: &str = "email";

fn pair(
    single_subject: &str,
    single_body: &str,
    digest_subject: &str,
    digest_body: &str,
) -> TemplatePair {
    TemplatePair {
        single: TemplateText {
            subject: Some(single_subject.to_owned()),
            body: single_body.to_owned(),
        },
        digest: TemplateText {
            subject: Some(digest_subject.to_owned()),
            body: digest_body.to_owned(),
        },
    }
}

/// The texts a mail server sends until its channel has its own: the title,
/// the first line and the address of a notification; and, for several, a line
/// for each.
#[must_use]
pub fn shipped_texts() -> BTreeMap<String, TemplatePair> {
    let single_body = "{{title}}\n{{body}}\n{{url}}";
    let digest_body = "{{#notifications}}{{time}}  {{title}}\n{{url}}\n{{/notifications}}";
    BTreeMap::from([
        (
            "*".to_owned(),
            pair(
                "[{{org.name}}] {{title}}",
                single_body,
                "[{{org.name}}] {{count}} unread",
                digest_body,
            ),
        ),
        (
            "zh-CN".to_owned(),
            pair(
                "[{{org.name}}] {{title}}",
                single_body,
                "[{{org.name}}] {{count}} 条未读",
                digest_body,
            ),
        ),
    ])
}

/// What the notifier declares of itself.
#[must_use]
pub fn descriptor() -> NotifierDescriptor {
    NotifierDescriptor {
        name: NAME.to_owned(),
        title: "Email".to_owned(),
        summary: "Mail from an SMTP server, to the confirmed address of a person.".to_owned(),
        targets: vec![Target::Person],
        fields: smtp::fields(),
        owners: vec![Owner::System, Owner::Org],
        single: true,
        reaches: Reaches::Email,
        body: BodyKind::Text,
        max_chars: None,
        templates: shipped_texts(),
        extensions: Extensions::new(),
    }
}

/// The notifier of a mail server.
#[derive(Debug, Clone, Copy, Default)]
pub struct EmailNotifier;

impl EmailNotifier {
    /// A notifier for any number of mail servers: each call is given the
    /// values of the one it is for.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

/// The mailbox a delivery goes to: the address the worker gave, with the name
/// of the person where it knows one.
fn recipient(delivery: &Delivery) -> Result<Mailbox, DeliverError> {
    let address = delivery
        .to
        .get("email")
        .and_then(Json::as_str)
        .map(str::trim)
        .filter(|address| !address.is_empty())
        .ok_or_else(|| {
            DeliverError::gone(
                "no_address",
                "the person has no confirmed address to send to",
            )
        })?;
    let address: lettre::Address = address.parse().map_err(|_| {
        DeliverError::gone("no_address", "the address of the person is not an address")
    })?;
    let name = delivery
        .recipient
        .as_ref()
        .map(|person| person.display_name.trim().to_owned())
        .filter(|name| !name.is_empty());
    Ok(Mailbox::new(name, address))
}

/// The message, ready to be handed to a server.
fn message(
    settings: &SmtpSettings,
    delivery: &Delivery,
    to: Mailbox,
    mail: &Composed,
) -> Result<Message, DeliverError> {
    let mut builder = Message::builder()
        .from(settings.from.clone())
        .to(to)
        .subject(mail.subject.clone())
        .message_id(Some(format!(
            "<{}@{}>",
            delivery.id,
            settings.from.email.domain()
        )))
        .date_now();
    if let Some(reply_to) = &settings.reply_to {
        builder = builder.reply_to(reply_to.clone());
    }
    builder
        .multipart(MultiPart::alternative_plain_html(
            mail.text.clone(),
            mail.html.clone(),
        ))
        .map_err(|error| DeliverError::refused("invalid_message", &error.to_string()))
}

/// Hands a message to the server and says what became of it.
async fn send(
    settings: &SmtpSettings,
    allowed: Option<&[String]>,
    message: &Message,
) -> Result<(), DeliverError> {
    let mut open = smtp::open(settings, allowed, Step::Open).await?;
    let sent = tokio::time::timeout(
        settings.timeout,
        open.connection
            .send(message.envelope(), &message.formatted()),
    )
    .await;
    match sent {
        Ok(Ok(_)) => {
            let _ = tokio::time::timeout(settings.timeout, open.connection.quit()).await;
            Ok(())
        }
        Ok(Err(error)) => {
            open.connection.abort().await;
            Err(smtp::classify(&error, Step::Send))
        }
        Err(_) => {
            open.connection.abort().await;
            Err(DeliverError::temporary(
                "timeout",
                &format!(
                    "{} did not take the message in {} s",
                    settings.host,
                    settings.timeout.as_secs()
                ),
            ))
        }
    }
}

#[async_trait]
impl Notifier for EmailNotifier {
    fn describe(&self) -> NotifierDescriptor {
        descriptor()
    }

    async fn check(&self, config: &Config) -> Result<Json, DeliverError> {
        let settings = SmtpSettings::from_config(config).map_err(|fault| fault.refusal())?;
        smtp::check(&settings, config.allowed_private_hosts.as_deref()).await?;
        Ok(Json::Null)
    }

    async fn deliver(&self, delivery: Delivery) -> Result<(), DeliverError> {
        let settings =
            SmtpSettings::from_config(&delivery.config).map_err(|fault| fault.refusal())?;
        let to = recipient(&delivery)?;
        let mail = render::compose(&delivery);
        let message = message(&settings, &delivery, to, &mail)?;
        send(
            &settings,
            delivery.config.allowed_private_hosts.as_deref(),
            &message,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn what_the_notifier_declares_is_consistent_and_ships_both_forms_in_both_languages() {
        let declared = descriptor();
        assert_eq!(declared.check(), Ok(()));
        assert!(declared.offered_to(Owner::System) && declared.offered_to(Owner::Org));
        assert!(
            !declared.offered_to(Owner::User),
            "a person's own are webhooks"
        );
        assert!(declared.is_single() && !declared.is_destination());
        assert_eq!(declared.reaches, Reaches::Email);
        assert!(declared.templates.contains_key("*") && declared.templates.contains_key("zh-CN"));
        let names: Vec<_> = declared
            .fields
            .iter()
            .map(|field| field.name.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "host",
                "port",
                "security",
                "auth",
                "username",
                "password",
                "from",
                "reply_to",
                "hello_name",
                "tls_ca",
                "timeout_secs"
            ]
        );
        assert!(
            declared
                .fields
                .iter()
                .filter(|field| field.kind.is_secret())
                .count()
                == 2
        );
    }

    fn delivery(to: serde_json::Value) -> Delivery {
        serde_json::from_value(json!({
            "id": "0199abcd-1234-7000-8000-0000000000d1", "attempt": 1,
            "config": { "values": { "host": "smtp.example.org", "from": "Orch <orch@example.org>" } },
            "to": to, "recipient": { "id": "0199abcd-1234-7000-8000-0000000000b1", "kind": "human",
                "username": "ada", "display_name": "Ada Lovelace", "avatar": { "text": "A", "color": 2 } },
            "language": "en", "subject": "Hello", "body": "Hi there\nhttps://orch.example/x", "notifications": []
        }))
        .unwrap()
    }

    #[test]
    fn a_delivery_without_an_address_to_send_to_says_that_the_person_is_gone() {
        for to in [
            json!({}),
            json!({ "email": "" }),
            json!({ "email": "not an address" }),
        ] {
            let error = recipient(&delivery(to)).unwrap_err();
            assert!(error.gone && !error.retryable, "{error}");
        }
        let mailbox = recipient(&delivery(json!({ "email": "ada@example.org" }))).unwrap();
        assert_eq!(mailbox.to_string(), "Ada Lovelace <ada@example.org>");
    }

    #[test]
    fn a_message_is_made_once_for_every_try_with_the_id_of_the_delivery() {
        let delivery = delivery(json!({ "email": "ada@example.org" }));
        let settings = SmtpSettings::from_config(&delivery.config).unwrap();
        let mail = render::compose(&delivery);
        let to = recipient(&delivery).unwrap();
        let one = message(&settings, &delivery, to.clone(), &mail).unwrap();
        let text = String::from_utf8(one.formatted()).unwrap();
        assert!(
            text.contains("Message-ID: <0199abcd-1234-7000-8000-0000000000d1@example.org>"),
            "{text}"
        );
        assert!(text.contains("multipart/alternative"));
        assert!(text.contains("text/plain") && text.contains("text/html"));
        assert!(text.contains("Subject: Hello"));
    }
}
