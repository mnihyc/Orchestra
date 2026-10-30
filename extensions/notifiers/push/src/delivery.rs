//! Delivery to a person's browsers, and a subscription removed when it has expired.
//!
//! [`PushNotifier`] is the one channel of Web Push, whose keys are the
//! system's. A person's route through it reaches each browser of theirs that
//! took push; the worker makes one delivery for each, with the browser's
//! address in `to` (its endpoint at the push service and the two keys it gave),
//! and the notifier encrypts the content to that browser's key, signs it with
//! the system's VAPID key and posts it to the endpoint. A push service says
//! whether it took the message; when it says the subscription no longer exists
//! (404, 410), or what is stored of it can never work, the browser is gone and
//! the worker removes it.
//!
//! The endpoint is an address a browser reported, so it is held to the list of
//! private hosts like the address of any webhook a person set up, and, unlike
//! the system's other channels, the system's own is held to it too: a person
//! must not be able to point the worker at a machine behind the system.

use crate::encryption;
use crate::payload::{self, BODY_CHARS};
use crate::vapid::Vapid;
use async_trait::async_trait;
use orch_notifier_contract::delivery::{Config, DeliverError, Delivery};
use orch_notifier_contract::descriptor::{NotifierDescriptor, Owner, Reaches, Target};
use orch_notifier_contract::notifier::Notifier;
use orch_notifier_contract::outbound;
use orch_notifier_contract::templates::{BodyKind, TemplatePair, TemplateText};
use orch_plugin_contract::Extensions;
use orch_types::{Clock, Json, SystemClock};
use reqwest::header::RETRY_AFTER;
use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use url::{Host, Url};

/// The name a channel gives as its kind to be Web Push.
pub const NAME: &str = "push";

/// How long a push service keeps a message for a browser that is offline: a
/// day, after which the Inbox is where a person catches up.
pub const TTL_SECS: u32 = 86_400;

/// How long a connection may take to be made.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a whole post may take.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The longest wait a push service may ask for, in seconds: a day.
pub const MAX_RETRY_AFTER_SECS: u32 = 86_400;

/// The texts the push channel sends: the subject is the title a notification
/// shows, the body is its text.
#[must_use]
pub fn shipped_texts() -> BTreeMap<String, TemplatePair> {
    let pair = |digest_subject: &str| TemplatePair {
        single: TemplateText {
            subject: Some("{{title}}".to_owned()),
            body: "{{body}}".to_owned(),
        },
        digest: TemplateText {
            subject: Some(digest_subject.to_owned()),
            body: "{{#notifications}}{{title}}\n{{/notifications}}".to_owned(),
        },
    };
    BTreeMap::from([
        ("*".to_owned(), pair("{{count}} unread")),
        ("zh-CN".to_owned(), pair("{{count}} 条未读")),
    ])
}

/// What the notifier declares of itself.
#[must_use]
pub fn descriptor() -> NotifierDescriptor {
    NotifierDescriptor {
        name: NAME.to_owned(),
        title: "Web Push".to_owned(),
        summary: "Notifications to the browsers of a person that took push.".to_owned(),
        targets: vec![Target::Person],
        fields: Vec::new(),
        owners: vec![Owner::System],
        single: true,
        reaches: Reaches::Devices,
        body: BodyKind::Text,
        max_chars: Some(BODY_CHARS),
        templates: shipped_texts(),
        extensions: Extensions::new(),
    }
}

/// The notifier of Web Push.
#[derive(Debug, Clone)]
pub struct PushNotifier {
    vapid: Vapid,
    clock: Arc<dyn Clock>,
}

impl PushNotifier {
    /// A notifier that signs with the system's key.
    #[must_use]
    pub fn new(vapid: Vapid) -> Self {
        Self::with_clock(vapid, Arc::new(SystemClock))
    }

    /// A notifier whose signature expiration follows the supplied clock.
    #[must_use]
    pub fn with_clock(vapid: Vapid, clock: Arc<dyn Clock>) -> Self {
        Self { vapid, clock }
    }

    /// The public half of the system's key: what a browser subscribes with.
    #[must_use]
    pub fn public_key(&self) -> &str {
        self.vapid.public_key()
    }
}

/// A browser, as the worker addresses it.
struct Subscription<'a> {
    endpoint: &'a str,
    public_key: &'a str,
    auth: &'a str,
}

fn subscription_of(to: &Json) -> Option<Subscription<'_>> {
    let endpoint = to.get("endpoint")?.as_str()?;
    let keys = to.get("keys")?;
    Some(Subscription {
        endpoint,
        public_key: keys.get("p256dh")?.as_str()?,
        auth: keys.get("auth")?.as_str()?,
    })
}

/// The addresses of the endpoint that a connection may go to. The system's own
/// channel is held to the list as well: the endpoint came from a person.
async fn addresses(
    url: &Url,
    allowed: Option<&[String]>,
) -> Result<(String, Vec<SocketAddr>), DeliverError> {
    let held = allowed.unwrap_or(&[]);
    let host = url
        .host()
        .ok_or_else(|| DeliverError::gone("bad_subscription", "the endpoint names no host"))?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| DeliverError::gone("bad_subscription", "the endpoint names no port"))?;
    let (name, found): (String, Vec<SocketAddr>) = match host {
        Host::Ipv4(address) => (
            address.to_string(),
            vec![SocketAddr::new(IpAddr::V4(address), port)],
        ),
        Host::Ipv6(address) => (
            address.to_string(),
            vec![SocketAddr::new(IpAddr::V6(address), port)],
        ),
        Host::Domain(domain) => {
            let looked_up = tokio::net::lookup_host((domain, port))
                .await
                .map_err(|error| {
                    DeliverError::temporary(
                        "unreachable",
                        &format!("{domain} cannot be looked up: {error}"),
                    )
                })?;
            (domain.to_owned(), looked_up.collect())
        }
    };
    let mut vetted = Vec::new();
    let mut refusal = None;
    for address in found {
        match outbound::may_connect(&name, address.ip(), Some(held)) {
            Ok(()) => vetted.push(address),
            Err(refused) => refusal = Some(refused),
        }
    }
    match (vetted.is_empty(), refusal) {
        (true, Some(refused)) => Err(DeliverError::refused(
            "address_not_allowed",
            &refused.to_string(),
        )),
        (true, None) => Err(DeliverError::temporary(
            "unreachable",
            &format!("{name} has no address"),
        )),
        _ => Ok((name, vetted)),
    }
}

/// What a push service's answer comes to (RFC 8030): it took the message
/// (`201`); the subscription is gone (`404`, `410`); the service asks for time
/// or fails (`429`, `5xx`); or it refuses for good, as for a message too large
/// (`413`), a request it cannot read (`400`) or a sender it does not accept
/// (`401`, `403`).
fn outcome(status: reqwest::StatusCode, retry_after: Option<u32>) -> Result<(), DeliverError> {
    let code = format!("http_{}", status.as_u16());
    let message = format!("the push service answered {status}");
    match status.as_u16() {
        200..=299 => Ok(()),
        404 | 410 => Err(DeliverError::gone(&code, &message)),
        408 | 429 | 500..=599 => Err(match retry_after {
            Some(seconds) => {
                DeliverError::after(&code, &message, seconds.min(MAX_RETRY_AFTER_SECS))
            }
            None => DeliverError::temporary(&code, &message),
        }),
        401 | 403 => Err(DeliverError::refused(
            "vapid_rejected",
            &format!("{message}: the sender was not accepted"),
        )),
        _ => Err(DeliverError::refused(&code, &message)),
    }
}

fn unreachable(error: reqwest::Error) -> DeliverError {
    let error = error.without_url();
    if error.is_timeout() {
        DeliverError::temporary("timeout", "the push service did not answer in time")
    } else {
        DeliverError::temporary(
            "unreachable",
            &format!("the push service cannot be reached: {error}"),
        )
    }
}

#[async_trait]
impl Notifier for PushNotifier {
    fn describe(&self) -> NotifierDescriptor {
        descriptor()
    }

    async fn check(&self, _: &Config) -> Result<Json, DeliverError> {
        Ok(Json::Null)
    }

    async fn deliver(&self, delivery: Delivery) -> Result<(), DeliverError> {
        let subscription = subscription_of(&delivery.to).ok_or_else(|| {
            DeliverError::gone(
                "bad_subscription",
                "the browser's address lacks its endpoint or its keys",
            )
        })?;
        let endpoint = Url::parse(subscription.endpoint).map_err(|_| {
            DeliverError::gone("bad_subscription", "the endpoint is not an address")
        })?;
        if !matches!(endpoint.scheme(), "http" | "https") {
            return Err(DeliverError::gone(
                "bad_subscription",
                "the endpoint is not a web address",
            ));
        }
        let (host, vetted) =
            addresses(&endpoint, delivery.config.allowed_private_hosts.as_deref()).await?;

        let content = serde_json::to_vec(&payload::content_of(&delivery))
            .map_err(|error| DeliverError::refused("message", &error.to_string()))?;
        let body = encryption::seal(subscription.public_key, subscription.auth, &content)?;
        let authorization = self
            .vapid
            .sign_for(&endpoint, self.clock.as_ref())
            .map_err(|error| DeliverError::refused("message", &error.to_string()))?;

        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .user_agent("orch-push")
            .no_proxy()
            .resolve_to_addrs(&host, &vetted)
            .build()
            .map_err(|error| DeliverError::refused("client", &error.without_url().to_string()))?;
        let mut post = client
            .post(endpoint)
            .header("content-encoding", "aes128gcm")
            .header("content-type", "application/octet-stream")
            .header("ttl", TTL_SECS)
            .header("authorization", authorization);
        if let Some(topic) = payload::topic_of(&delivery) {
            post = post.header("topic", topic);
        }
        let response = post.body(body).send().await.map_err(unreachable)?;
        let retry_after = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u32>().ok());
        outcome(response.status(), retry_after)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;

    #[test]
    fn what_the_notifier_declares_is_consistent_and_it_is_the_one_channel_of_the_system() {
        let declared = descriptor();
        assert_eq!(declared.check(), Ok(()));
        assert!(
            declared.offered_to(Owner::System)
                && !declared.offered_to(Owner::Org)
                && !declared.offered_to(Owner::User)
        );
        assert!(declared.is_single() && declared.fields.is_empty());
        assert_eq!(declared.reaches, Reaches::Devices);
        assert!(declared.reaches.one_delivery_per_device());
    }

    #[test]
    fn the_answer_of_a_push_service_decides_what_becomes_of_the_message() {
        let answer =
            |code: u16, wait: Option<u32>| outcome(StatusCode::from_u16(code).unwrap(), wait);
        assert_eq!(answer(201, None), Ok(()));
        for gone in [404, 410] {
            assert!(answer(gone, None).unwrap_err().gone, "{gone}");
        }
        for again in [429, 500, 502, 503] {
            assert!(answer(again, None).unwrap_err().retryable, "{again}");
        }
        assert_eq!(
            answer(429, Some(30)).unwrap_err().retry_after_secs,
            Some(30)
        );
        assert_eq!(
            answer(503, Some(9_999_999)).unwrap_err().retry_after_secs,
            Some(MAX_RETRY_AFTER_SECS)
        );
        for for_good in [400, 401, 403, 413] {
            let error = answer(for_good, None).unwrap_err();
            assert!(!error.retryable && !error.gone, "{for_good}");
        }
        assert_eq!(answer(403, None).unwrap_err().code, "vapid_rejected");
    }

    #[test]
    fn what_a_browser_gave_that_can_never_work_is_a_browser_that_is_gone() {
        assert!(
            subscription_of(
                &serde_json::json!({ "endpoint": "https://x", "keys": { "p256dh": "a" } })
            )
            .is_none()
        );
        assert!(subscription_of(&serde_json::json!({})).is_none());
    }
}
