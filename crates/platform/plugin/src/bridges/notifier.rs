//! External Notifier bridge.
//!
//! [`NotifierBridge`] is a [`Notifier`] whose implementation is a program. The engine decides who is told what,
//! the worker renders the texts, and the notifier only delivers; it never reads the database. The worker gives it
//! the values of the channel and its secrets, unsealed for the length of one call, and so they are in the request
//! that the program receives and nowhere else.
//!
//! | Method | Required | Parameters | Result |
//! |--------|----------|------------|--------|
//! | `describe` | yes | none | the notifier's descriptor |
//! | `check` | yes | `config`: the values, the secrets and the private hosts allowed | whatever the far side says of itself, as it is (a bot's name), or `null` |
//! | `deliver` | yes | the delivery: `id`, `attempt`, `config`, `to`, `recipient`, `language`, `subject`, `body`, `notifications` | none |
//! | `link_url` | for a notifier that links people | `config`, `code` | `url`, which may be `null` |
//! | `poll_links` | for a notifier that links people | `config`, `cursor` (left out for the first reading), `wait_secs` | the reading: `started` and `cursor` |
//!
//! A program refuses with an error whose members are `code`, `message`, `retryable`, and, as the far side asked,
//! `retry_after_secs`, or `gone` for a target that no longer exists. A notifier that links nobody does not declare
//! the last two methods, and is asked for neither: it has no link, and a request to poll answers that it does not
//! link people. `poll_links` may wait up to `wait_secs` before it answers that nobody followed a link, so its
//! deadline is that and the usual one after it.

use std::time::Duration;

use async_trait::async_trait;
use orch_notifier_contract::delivery::{Config, DeliverError, Delivery, LinkPoll};
use orch_notifier_contract::descriptor::NotifierDescriptor;
use orch_notifier_contract::notifier::Notifier;
use orch_plugin_contract::PluginKind;
use orch_plugin_contract::lifecycle::DEFAULT_DEADLINE;
use orch_types::Json;
use serde_json::json;

use super::{BridgeError, Described, member, object, params, result};
use crate::requests::CallError;
use crate::supervisor::Plugin;

const CHECK: &str = "check";
const DELIVER: &str = "deliver";
const LINK_URL: &str = "link_url";
const POLL_LINKS: &str = "poll_links";

/// A notifier that is a program.
pub struct NotifierBridge {
    plugin: Plugin,
    descriptor: Described<NotifierDescriptor>,
}

impl NotifierBridge {
    /// The notifier that `plugin` is. The plugin must be of the kind and have said hello.
    pub fn new(plugin: Plugin) -> Result<Self, BridgeError> {
        Ok(Self {
            descriptor: Described::new(plugin.clone(), PluginKind::Notifier)?,
            plugin,
        })
    }
}

/// The refusal for what the program answered. A program that is down, or does not answer, is a far side that
/// cannot be reached now: the delivery is tried again on the schedule.
fn refusal(error: CallError) -> DeliverError {
    match error {
        CallError::Failed(remote) => DeliverError {
            code: remote.code,
            message: remote.message,
            retryable: remote.retryable,
            retry_after_secs: remote.retry_after_secs,
            gone: remote.gone,
        },
        CallError::Unavailable { .. } | CallError::Lost { .. } | CallError::Timeout { .. } => {
            DeliverError::temporary(error.code(), &error.to_string())
        }
        CallError::Cancelled | CallError::Malformed { .. } => {
            DeliverError::refused(error.code(), &error.to_string())
        }
    }
}

#[async_trait]
impl Notifier for NotifierBridge {
    fn describe(&self) -> NotifierDescriptor {
        (*self.descriptor.get()).clone()
    }

    async fn check(&self, config: &Config) -> Result<Json, DeliverError> {
        let answer = async {
            let sent = params(CHECK, &json!({ "config": config }))?;
            self.plugin.call(CHECK, sent).await
        };
        answer.await.map_err(refusal)
    }

    async fn deliver(&self, delivery: Delivery) -> Result<(), DeliverError> {
        let answer = async {
            let sent = params(DELIVER, &delivery)?;
            self.plugin.call(DELIVER, sent).await
        };
        answer.await.map(|_| ()).map_err(refusal)
    }

    fn link_url(&self, config: &Config, code: &str) -> Option<String> {
        if !self.plugin.supports(LINK_URL) {
            return None;
        }
        let asked = params(LINK_URL, &json!({ "config": config, "code": code }))
            .and_then(|sent| self.plugin.start_call(LINK_URL, sent))
            .and_then(crate::requests::Call::wait_blocking)
            .and_then(|answer| member::<Option<String>>(LINK_URL, answer, "url"));
        match asked {
            Ok(url) => url,
            Err(error) => {
                tracing::warn!(plugin = self.plugin.name(), %error, "the plugin made no link");
                None
            }
        }
    }

    async fn poll_links(
        &self,
        config: &Config,
        cursor: Option<String>,
        wait: Duration,
    ) -> Result<LinkPoll, DeliverError> {
        if !self.plugin.supports(POLL_LINKS) {
            return Err(DeliverError::unsupported("link people"));
        }
        // The program may keep the request for as long as it was told to wait, and then has the usual time.
        let deadline = wait.saturating_add(DEFAULT_DEADLINE);
        let wait_secs = wait.as_secs() + u64::from(wait.subsec_nanos() > 0);
        let answer = async {
            let sent = object([
                ("config", params(POLL_LINKS, config)?),
                ("cursor", params(POLL_LINKS, &cursor)?),
                ("wait_secs", Json::from(wait_secs)),
            ]);
            let call = self.plugin.start_call_within(POLL_LINKS, sent, deadline)?;
            result(POLL_LINKS, call.await?)
        };
        answer.await.map_err(refusal)
    }
}
