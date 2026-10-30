//! HTTP delivery, with a text of the channel's own rendered as the body where it has one, and retryable outcomes.
//!
//! A webhook is an address that is posted to, for a tool that tells people
//! itself. What is posted is the body of the notification (or, for a message
//! that tells several, the newest one's members with all of them beside as
//! `notifications`), or, where the channel or the destination has a text of
//! its own, what that renders, which is how a webhook is fitted to the format a
//! chat tool expects. With a secret, the body is signed
//! ([`crate::signature`]). The address is held to the list of private hosts
//! ([`crate::targets`]) on the address it connects to and again after every
//! redirect, which is followed with the same post.
//!
//! What the far side answers decides the outcome: a success takes the delivery;
//! a request to wait, a server error or no answer is tried again, after the
//! time the far side named where it named one; an address that no longer exists
//! (404, 410) is gone, which switches the route or the destination off; any
//! other refusal fails the delivery.

use crate::signature;
use crate::targets::{self, Target};
use async_trait::async_trait;
use orch_notifier_contract::delivery::{Config, DeliverError, Delivery, WebhookBody};
use orch_notifier_contract::descriptor::{NotifierDescriptor, Owner, Reaches, Target as Kind};
use orch_notifier_contract::notifier::Notifier;
use orch_notifier_contract::templates::{BodyKind, TemplatePair, TemplateText};
use orch_plugin_contract::Extensions;
use orch_types::{Field, FieldType, Json};
use reqwest::header::{CONTENT_TYPE, LOCATION, RETRY_AFTER};
use std::collections::BTreeMap;
use std::time::Duration;
use url::Url;

/// The name a channel gives as its kind to be a webhook.
pub const NAME: &str = "webhook";

/// How long a connection may take to be made: long enough for a slow network,
/// short enough that an address nobody answers at is given up on quickly.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a whole post, an answer included, may take.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// How many redirects a post follows: a webhook behind an address that
/// moved, or from `http` to `https`, needs one or two; more is a loop.
pub const MAX_REDIRECTS: usize = 3;

/// The longest wait the far side may ask for, in seconds: a day, so that an
/// answer cannot park a delivery for good.
pub const MAX_RETRY_AFTER_SECS: u32 = 86_400;

/// How much of an answer is quoted in an error: enough to say why, not enough
/// to fill a column with a page.
const QUOTED_BYTES: usize = 300;

/// What the program calls itself to the far side.
const USER_AGENT: &str = "orch-webhook";

/// The texts the notifier ships: an example to start an editor from, since a
/// webhook with no text of its own posts the notification as it is.
#[must_use]
pub fn shipped_texts() -> BTreeMap<String, TemplatePair> {
    let pair = TemplatePair {
        single: TemplateText {
            subject: None,
            body: "{\"text\": \"{{title}}\\n{{body}}\\n{{url}}\"}".to_owned(),
        },
        digest: TemplateText {
            subject: None,
            body: "{\"text\": \"{{count}} unread\\n{{#notifications}}{{time}}  {{title}}\\n{{/notifications}}\"}"
                .to_owned(),
        },
    };
    BTreeMap::from([("*".to_owned(), pair)])
}

/// What the notifier declares of itself.
#[must_use]
pub fn descriptor() -> NotifierDescriptor {
    let mut url = Field::new("url", "Address", FieldType::Text);
    url.help = Some("Where what happens is posted, as https://hooks.example/….".to_owned());
    let mut secret = Field::new("secret", "Secret", FieldType::Secret);
    secret.required = false;
    secret.help = Some(
        "With a secret, every post carries X-Orch-Signature: sha256=, the HMAC-SHA256 of its body under it."
            .to_owned(),
    );
    NotifierDescriptor {
        name: NAME.to_owned(),
        title: "Webhook".to_owned(),
        summary: "Posts what happened, as JSON, to an address, signed where there is a secret."
            .to_owned(),
        targets: vec![Kind::Person, Kind::Destination],
        fields: vec![url, secret],
        owners: vec![Owner::System, Owner::Org, Owner::User],
        single: false,
        reaches: Reaches::Channel,
        body: BodyKind::Json,
        max_chars: None,
        templates: shipped_texts(),
        extensions: Extensions::new(),
    }
}

/// The notifier of webhooks.
#[derive(Debug, Clone, Copy, Default)]
pub struct WebhookNotifier;

impl WebhookNotifier {
    /// A notifier for any number of webhooks: each call is given the values of
    /// the one it is for.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

fn address_of(config: &Config) -> Result<Url, DeliverError> {
    let text = config
        .text("url")
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| DeliverError::refused("invalid_config", "url is required"))?;
    targets::parse(text)
}

/// The body to post: the text the channel rendered, or else the body of the
/// notification.
fn body_of(delivery: &Delivery) -> Result<String, DeliverError> {
    if !delivery.body.is_empty() {
        return Ok(delivery.body.clone());
    }
    let body = WebhookBody::of(&delivery.notifications)
        .ok_or_else(|| DeliverError::refused("nothing_to_post", "the message tells nothing"))?;
    serde_json::to_string(&body)
        .map_err(|error| DeliverError::refused("invalid_message", &error.to_string()))
}

/// What a failure to reach the address means: it may pass.
fn unreachable(error: reqwest::Error) -> DeliverError {
    let error = error.without_url();
    if error.is_timeout() {
        DeliverError::temporary("timeout", "the address did not answer in time")
    } else if error.is_connect() {
        DeliverError::temporary(
            "unreachable",
            &format!("the address cannot be connected to: {error}"),
        )
    } else {
        DeliverError::temporary("network", &format!("the post failed: {error}"))
    }
}

/// A short, one-line piece of what the far side answered.
fn quoted(chunk: Option<&[u8]>) -> String {
    let text = String::from_utf8_lossy(chunk.unwrap_or_default());
    let text: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    text.chars().take(QUOTED_BYTES).collect()
}

/// What an answer comes to.
fn outcome(
    status: reqwest::StatusCode,
    retry_after: Option<u32>,
    said: &str,
) -> Result<(), DeliverError> {
    let code = format!("http_{}", status.as_u16());
    let message = if said.is_empty() {
        format!("the address answered {status}")
    } else {
        format!("the address answered {status}: {said}")
    };
    match status.as_u16() {
        200..=299 => Ok(()),
        404 | 410 => Err(DeliverError::gone(&code, &message)),
        408 | 425 | 429 | 500..=599 => Err(match retry_after {
            Some(seconds) => {
                DeliverError::after(&code, &message, seconds.min(MAX_RETRY_AFTER_SECS))
            }
            None => DeliverError::temporary(&code, &message),
        }),
        _ => Err(DeliverError::refused(&code, &message)),
    }
}

fn client(target: &Target) -> Result<reqwest::Client, DeliverError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .user_agent(USER_AGENT)
        .no_proxy()
        .resolve_to_addrs(&target.host, &target.addresses)
        .build()
        .map_err(|error| DeliverError::refused("client", &error.without_url().to_string()))
}

/// What a post carries besides its body.
struct Post<'a> {
    body: &'a str,
    signature: Option<String>,
    delivery: &'a Delivery,
}

/// Posts to the address, and to each address it redirects to, each looked at
/// before a connection is made.
async fn post(url: Url, post: &Post<'_>, allowed: Option<&[String]>) -> Result<(), DeliverError> {
    let mut url = url;
    for hop in 0..=MAX_REDIRECTS {
        let target = targets::resolve(&url, allowed).await?;
        let mut request = client(&target)?
            .post(url.clone())
            .header(CONTENT_TYPE, "application/json")
            .header("X-Orch-Delivery", post.delivery.id.to_string())
            .header("X-Orch-Attempt", post.delivery.attempt.to_string())
            .body(post.body.to_owned());
        if let Some(signature) = &post.signature {
            request = request.header(signature::HEADER, signature);
        }
        let mut response = request.send().await.map_err(unreachable)?;
        let status = response.status();
        if status.is_redirection() {
            let next = response
                .headers()
                .get(LOCATION)
                .and_then(|location| location.to_str().ok())
                .and_then(|location| url.join(location).ok());
            match next {
                Some(next) if hop < MAX_REDIRECTS => {
                    url = next;
                    continue;
                }
                Some(_) => {
                    return Err(DeliverError::refused(
                        "too_many_redirects",
                        &format!("the address redirected more than {MAX_REDIRECTS} times"),
                    ));
                }
                None => {
                    return outcome(status, None, "a redirect with nowhere to go");
                }
            }
        }
        let retry_after = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u32>().ok());
        let said = if status.is_success() {
            String::new()
        } else {
            quoted(response.chunk().await.ok().flatten().as_deref())
        };
        return outcome(status, retry_after, &said);
    }
    Err(DeliverError::refused(
        "too_many_redirects",
        "the address redirected too often",
    ))
}

#[async_trait]
impl Notifier for WebhookNotifier {
    fn describe(&self) -> NotifierDescriptor {
        descriptor()
    }

    async fn check(&self, config: &Config) -> Result<Json, DeliverError> {
        let url = address_of(config)?;
        targets::resolve(&url, config.allowed_private_hosts.as_deref()).await?;
        Ok(Json::Null)
    }

    async fn deliver(&self, delivery: Delivery) -> Result<(), DeliverError> {
        let url = address_of(&delivery.config)?;
        let body = body_of(&delivery)?;
        let signature = delivery
            .config
            .secret("secret")
            .filter(|secret| !secret.is_empty())
            .map(|secret| signature::sign(secret.expose().as_bytes(), body.as_bytes()));
        let allowed = delivery.config.allowed_private_hosts.clone();
        post(
            url,
            &Post {
                body: &body,
                signature,
                delivery: &delivery,
            },
            allowed.as_deref(),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;

    #[test]
    fn what_the_notifier_declares_is_consistent_and_its_example_text_renders_to_json() {
        let declared = descriptor();
        assert_eq!(declared.check(), Ok(()));
        assert!(declared.is_channel() && declared.is_destination() && !declared.is_single());
        assert!(
            declared.offered_to(Owner::User),
            "a person's own are webhooks"
        );
        assert_eq!(declared.reaches, Reaches::Channel);
        assert_eq!(declared.body, BodyKind::Json);
        let names: Vec<_> = declared
            .fields
            .iter()
            .map(|field| field.name.as_str())
            .collect();
        assert_eq!(names, ["url", "secret"]);
    }

    #[test]
    fn the_answer_of_the_far_side_decides_what_becomes_of_the_delivery() {
        let outcome_of =
            |code: u16, wait: Option<u32>| outcome(StatusCode::from_u16(code).unwrap(), wait, "");
        assert_eq!(outcome_of(200, None), Ok(()));
        assert_eq!(outcome_of(204, None), Ok(()));
        for gone in [404, 410] {
            assert!(outcome_of(gone, None).unwrap_err().gone, "{gone}");
        }
        for again in [408, 425, 429, 500, 502, 503, 504] {
            let error = outcome_of(again, None).unwrap_err();
            assert!(error.retryable && !error.gone, "{again}");
        }
        for for_good in [400, 401, 403, 405, 413, 422, 301] {
            let error = outcome_of(for_good, None).unwrap_err();
            assert!(!error.retryable && !error.gone, "{for_good}");
        }
        let slow = outcome_of(429, Some(120)).unwrap_err();
        assert_eq!((slow.retry_after_secs, slow.retryable), (Some(120), true));
        let parked = outcome_of(503, Some(10_000_000)).unwrap_err();
        assert_eq!(
            parked.retry_after_secs,
            Some(MAX_RETRY_AFTER_SECS),
            "a wait is bounded"
        );
        assert!(
            outcome(StatusCode::BAD_REQUEST, None, "invalid_payload")
                .unwrap_err()
                .message
                .contains("invalid_payload")
        );
    }

    #[test]
    fn what_is_posted_is_the_text_of_the_channel_or_else_the_notification_with_the_rest_beside_it()
    {
        let delivery = |body: &str, titles: &[&str]| -> Delivery {
            let notifications: Vec<_> = titles
                .iter()
                .enumerate()
                .map(|(n, title)| serde_json::json!({
                    "id": format!("0199abcd-1234-7000-8000-0000000000{n:02}"), "kind": "turn.completed",
                    "at": "2026-10-11T10:00:00.000Z", "title": title, "body": "all green", "url": "https://o.example/x",
                    "node": null, "thread_id": null, "chat_id": null, "data": {}, "recipient": null }))
                .collect();
            serde_json::from_value(serde_json::json!({
                "id": "0199abcd-1234-7000-8000-0000000000d1", "attempt": 1, "config": {}, "to": {}, "recipient": null,
                "language": "en", "subject": null, "body": body, "notifications": notifications
            }))
            .unwrap()
        };
        assert_eq!(
            body_of(&delivery("{\"text\":\"hi\"}", &["A"])).unwrap(),
            "{\"text\":\"hi\"}"
        );
        let one: Json = serde_json::from_str(&body_of(&delivery("", &["A"])).unwrap()).unwrap();
        assert_eq!(one["title"], "A");
        assert!(
            one.get("notifications").is_none(),
            "a message of one is that notification alone"
        );
        let several: Json =
            serde_json::from_str(&body_of(&delivery("", &["A", "B", "C"])).unwrap()).unwrap();
        assert_eq!(several["title"], "C", "the newest one's members");
        assert_eq!(several["notifications"].as_array().unwrap().len(), 3);
        assert_eq!(
            body_of(&delivery("", &[])).unwrap_err().code,
            "nothing_to_post"
        );
    }
}
