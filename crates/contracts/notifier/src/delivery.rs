//! What is delivered to which target, one notification or several together, and the outcome: taken, to be retried and when, or gone.
//!
//! The engine decides who is told what, and writes a row of the outbox for each delivery. The worker renders
//! the texts, with every value escaped for the notifier's body, and hands the notifier a **delivery**: a
//! finished text, the address of the route it goes to, and beside the text the notifications it was
//! rendered from, for a notifier that wants to build its own message. A notifier delivers, and never reads
//! the database. Secret fields are unsealed in the worker for the length of one call, and given with it.
//!
//! What a delivery came to is one of four things ([`Next`]): it was taken; it is to be tried again, and when;
//! it failed for good; or its target is gone. A far side's refusal says which ([`DeliverError`]); the rules
//! that turn it into the next step of the outbox row are [`after_delivery`], and the rule for what a check
//! of a saved channel decides is [`on_check`].

use std::collections::BTreeMap;
use std::time::Duration;

use orch_plugin_contract::Secret;
use orch_plugin_contract::refs::{NodeRef, UserRef};
use orch_types::fields::{FieldError, validate};
use orch_types::{Field, Id, Json, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The pauses between the tries of a delivery that may be tried again: one minute, five minutes, fifteen
/// minutes, an hour and six hours, and then it has failed. This is the default of `[worker] outbox_retry_secs`.
pub const DEFAULT_RETRY_SCHEDULE: [Duration; 5] = [
    Duration::from_secs(60),
    Duration::from_secs(5 * 60),
    Duration::from_secs(15 * 60),
    Duration::from_secs(60 * 60),
    Duration::from_secs(6 * 60 * 60),
];

/// The kinds of notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum NotificationKind {
    /// One scheduled input could not start under its author's current authority.
    #[serde(rename = "schedule.cannot_start")]
    ScheduleCannotStart,
    /// A turn finished.
    #[serde(rename = "turn.completed")]
    TurnCompleted,
    /// A turn failed.
    #[serde(rename = "turn.failed")]
    TurnFailed,
    /// A goal ended.
    #[serde(rename = "goal.ended")]
    GoalEnded,
    /// A thread is blocked.
    #[serde(rename = "thread.blocked")]
    ThreadBlocked,
    /// An approval waits for an answer.
    #[serde(rename = "approval.requested")]
    ApprovalRequested,
    /// An environment asks for a capability.
    #[serde(rename = "capability.requested")]
    CapabilityRequested,
    /// Access was asked for.
    #[serde(rename = "access.requested")]
    AccessRequested,
    /// Access was decided.
    #[serde(rename = "access.decided")]
    AccessDecided,
    /// An agent marked a milestone.
    #[serde(rename = "milestone")]
    Milestone,
    /// An announcement was delivered.
    #[serde(rename = "announcement")]
    Announcement,
    /// A person was mentioned.
    #[serde(rename = "mention")]
    Mention,
    /// A message was written in a chat.
    #[serde(rename = "chat.message")]
    ChatMessage,
    /// A person was named in a document.
    #[serde(rename = "doc.mention")]
    DocMention,
    /// A person explicitly sent a notice about a readable thing.
    #[serde(rename = "person.notify")]
    PersonNotify,
    /// A discussion beside a document was answered.
    #[serde(rename = "doc.comment")]
    DocComment,
    /// A provider is used up.
    #[serde(rename = "provider.exhausted")]
    ProviderExhausted,
    /// A provider needs its managers' attention.
    #[serde(rename = "provider.attention")]
    ProviderAttention,
    /// A machine went offline.
    #[serde(rename = "machine.offline")]
    MachineOffline,
    /// An environment was stopped for being idle, or is about to be.
    #[serde(rename = "environment.idle_stop")]
    EnvironmentIdleStop,
    /// An environment fell behind.
    #[serde(rename = "environment.update")]
    EnvironmentUpdate,
    /// An environment needs its managers' attention.
    #[serde(rename = "environment.attention")]
    EnvironmentAttention,
    /// A cap's use passed its warning.
    #[serde(rename = "cap.warning")]
    CapWarning,
    /// A cap was reached.
    #[serde(rename = "cap.reached")]
    CapReached,
    /// A job finished.
    #[serde(rename = "job.finished")]
    JobFinished,
    /// A person was invited.
    #[serde(rename = "invitation")]
    Invitation,
    /// A member was moved to another org.
    #[serde(rename = "member.moved")]
    MemberMoved,
    /// Something about a person's own sign-in.
    #[serde(rename = "sign_in.attention")]
    SignInAttention,
}

/// One notification, as the body of a webhook carries it and as a notifier is given what a text was
/// rendered from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NotificationBody {
    /// Its id.
    pub id: Id,
    /// Its kind.
    pub kind: NotificationKind,
    /// When it happened.
    pub at: Time,
    /// Its title.
    pub title: String,
    /// Its first line.
    pub body: String,
    /// The address to open.
    pub url: String,
    /// The place it happened at.
    pub node: Option<NodeRef>,
    /// The thread it is about.
    pub thread_id: Option<Id>,
    /// The chat it is about.
    pub chat_id: Option<Id>,
    /// Whatever else its kind carries.
    pub data: Json,
    /// Whom it is for; `null` for a node's destination, which is told for nobody in particular.
    pub recipient: Option<UserRef>,
}

/// The body a webhook posts when it has no text of its own: the newest notification's members, and where the
/// message carries several, all of them as `notifications`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WebhookBody {
    /// The newest notification.
    #[serde(flatten)]
    pub newest: NotificationBody,
    /// Every notification of the message, when it carries several.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notifications: Option<Vec<NotificationBody>>,
}

impl WebhookBody {
    /// The body of a message that carries `notifications`, oldest first; `None` for a message with none. A
    /// message of one is that notification's members alone.
    #[must_use]
    pub fn of(notifications: &[NotificationBody]) -> Option<Self> {
        let newest = notifications.last()?.clone();
        Some(Self {
            newest,
            notifications: (notifications.len() > 1).then(|| notifications.to_vec()),
        })
    }
}

/// The values of a channel or a destination as they are handed over for one call.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Config {
    /// The notifier's declared fields, without the secret ones.
    #[serde(default)]
    pub values: Json,
    /// The secret fields, unsealed for the length of this call.
    #[serde(default)]
    pub secrets: BTreeMap<String, Secret>,
    /// The hosts on a loopback, link-local or private address that this channel may reach
    /// (`outbound.allowed_private_hosts`): names or ranges. `None`: it is not held to the list, as the
    /// system's own channels are not.
    #[serde(default)]
    pub allowed_private_hosts: Option<Vec<String>>,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("values", &self.values)
            .field("secrets", &self.secrets.keys().collect::<Vec<_>>())
            .field("allowed_private_hosts", &self.allowed_private_hosts)
            .finish()
    }
}

impl Config {
    /// Splits what a person gave for the notifier's `fields` into the values and the secrets: a field of a
    /// secret kind goes to `secrets` (a text field as the text, a map by its names), and the rest stay.
    /// What is given is checked against the fields first, every fault returned.
    pub fn from_answer(
        fields: &[Field],
        answer: &Json,
        allowed_private_hosts: Option<Vec<String>>,
    ) -> Result<Self, Vec<FieldError>> {
        validate(fields, answer)?;
        let mut values = serde_json::Map::new();
        let mut secrets = BTreeMap::new();
        if let Json::Object(given) = answer {
            for (name, value) in given {
                let secret_field = fields
                    .iter()
                    .find(|field| &field.name == name)
                    .is_some_and(|field| field.kind.is_secret());
                match (secret_field, value) {
                    (_, Json::Null) => {}
                    (true, Json::String(text)) => {
                        secrets.insert(name.clone(), Secret::new(text.as_str()));
                    }
                    (true, Json::Object(map)) => {
                        for (key, value) in map {
                            if let Some(text) = value.as_str() {
                                secrets.insert(format!("{name}.{key}"), Secret::new(text));
                            }
                        }
                    }
                    _ => {
                        values.insert(name.clone(), value.clone());
                    }
                }
            }
        }
        Ok(Self {
            values: Json::Object(values),
            secrets,
            allowed_private_hosts,
        })
    }

    /// The text of a declared field, from the values.
    #[must_use]
    pub fn text(&self, field: &str) -> Option<&str> {
        self.values.get(field).and_then(Json::as_str)
    }

    /// The text of a secret field.
    #[must_use]
    pub fn secret(&self, field: &str) -> Option<&Secret> {
        self.secrets.get(field)
    }
}

/// One delivery: the finished text, and where it goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Delivery {
    /// The outbox row's id; it is the same across the tries, so a notifier can make a delivery idempotent.
    pub id: Id,
    /// Which try this is; the first is 1.
    pub attempt: u32,
    /// The channel's or the destination's values and secrets.
    pub config: Config,
    /// The route's address: where the recipient is on this channel; `{}` where it has none.
    pub to: Json,
    /// Whom it is for; `null` for a destination.
    pub recipient: Option<UserRef>,
    /// The language the text is in.
    pub language: String,
    /// The subject, for mail.
    pub subject: Option<String>,
    /// The text, rendered by the worker from the templates, every value escaped for the notifier's body.
    pub body: String,
    /// The notifications it was rendered from.
    pub notifications: Vec<NotificationBody>,
}

/// Who followed an address a notifier made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LinkStarted {
    /// The code the person came with.
    pub code: String,
    /// What the notifier learned: for a bot, the chat it may write to.
    pub address: Json,
    /// Whom that is, as the far side names them, so that the person sees that the route is theirs.
    pub label: String,
}

/// The answer to asking who followed an address.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LinkPoll {
    /// Those who did since the cursor.
    pub started: Vec<LinkStarted>,
    /// Where the next reading continues; kept on the channel's row.
    pub cursor: Option<String>,
}

/// Why a call did not succeed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeliverError {
    /// A code.
    pub code: String,
    /// What the far side said, or happened, in a sentence.
    pub message: String,
    /// Whether trying again may help.
    #[serde(default)]
    pub retryable: bool,
    /// How long the far side asked to wait, in seconds.
    #[serde(default)]
    pub retry_after_secs: Option<u32>,
    /// The target no longer exists, or no longer takes this sender.
    #[serde(default)]
    pub gone: bool,
}

impl DeliverError {
    /// A refusal for good.
    #[must_use]
    pub fn refused(code: &str, message: &str) -> Self {
        Self {
            code: code.to_owned(),
            message: message.to_owned(),
            retryable: false,
            retry_after_secs: None,
            gone: false,
        }
    }

    /// A failure that may pass, such as a far side that cannot be reached now.
    #[must_use]
    pub fn temporary(code: &str, message: &str) -> Self {
        Self {
            retryable: true,
            ..Self::refused(code, message)
        }
    }

    /// A failure that passes after `seconds`, as the far side named.
    #[must_use]
    pub fn after(code: &str, message: &str, seconds: u32) -> Self {
        Self {
            retry_after_secs: Some(seconds),
            ..Self::temporary(code, message)
        }
    }

    /// The target is gone.
    #[must_use]
    pub fn gone(code: &str, message: &str) -> Self {
        Self {
            gone: true,
            ..Self::refused(code, message)
        }
    }

    /// The answer of a notifier that does not do what was asked (a call to `poll_links` of one that does
    /// not link people).
    #[must_use]
    pub fn unsupported(what: &str) -> Self {
        Self::refused("unsupported", &format!("this notifier does not {what}"))
    }
}

impl std::fmt::Display for DeliverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for DeliverError {}

/// What becomes of a delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// The far side took it.
    Taken,
    /// Try again after this long.
    RetryAfter(Duration),
    /// It fails for good: the row is `failed`.
    Failed,
    /// The target is gone: the route or the destination is switched off, or the device's subscription is
    /// removed.
    Gone,
}

/// What becomes of a delivery after its `attempts`-th try (the first try is 1), given the schedule of
/// pauses.
///
/// `Ok` means the far side took it. A `gone` error switches the target off, whatever else it says. An error
/// that is not `retryable` fails the row at once. One that is is tried again after the pause the far side
/// named, or after the schedule's pause for this try; when the schedule has none left, it has failed.
#[must_use]
pub fn after_delivery(
    result: &Result<(), DeliverError>,
    attempts: u32,
    schedule: &[Duration],
) -> Next {
    let Err(error) = result else {
        return Next::Taken;
    };
    if error.gone {
        return Next::Gone;
    }
    if !error.retryable {
        return Next::Failed;
    }
    let index = usize::try_from(attempts.saturating_sub(1)).unwrap_or(usize::MAX);
    match schedule.get(index) {
        None => Next::Failed,
        Some(pause) => Next::RetryAfter(
            error
                .retry_after_secs
                .map_or(*pause, |seconds| Duration::from_secs(u64::from(seconds))),
        ),
    }
}

/// What looking at a channel's values decides when they are saved or tested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Saved {
    /// They are saved, with what the far side said of itself kept beside them (a bot's name).
    Kept(Json),
    /// What the far side refuses for good, a password or a token, refuses the save, with what it answered.
    Refused(DeliverError),
    /// The far side cannot be reached now: a server that is down today is not a wrong entry, so the values are
    /// saved, with the error shown on the channel.
    KeptWithError(DeliverError),
}

/// What the answer of `check` decides.
#[must_use]
pub fn on_check(result: Result<Json, DeliverError>) -> Saved {
    match result {
        Ok(remote) => Saved::Kept(remote),
        Err(error) if error.retryable => Saved::KeptWithError(error),
        Err(error) => Saved::Refused(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::FieldType;
    use serde_json::json;

    fn notification(n: u8) -> NotificationBody {
        serde_json::from_value(json!({
            "id": format!("0199abcd-1234-7000-8000-0000000000{n:02}"), "kind": "turn.completed", "at": "2026-10-11T10:00:00.000Z",
            "title": format!("Turn {n} finished"), "body": "all green", "url": "https://orch.example/inbox/1",
            "node": { "id": "0199abcd-1234-7000-8000-0000000000a1", "kind": "thread", "name": "Fix login", "redacted": false },
            "thread_id": "0199abcd-1234-7000-8000-0000000000a1", "chat_id": null, "data": { "turn": 3 },
            "recipient": { "id": "0199abcd-1234-7000-8000-0000000000b1", "kind": "human", "username": "bo",
                           "display_name": "Bo", "avatar": { "text": "B", "color": 2 } }
        }))
        .unwrap()
    }

    #[test]
    fn a_notification_reads_as_the_webhook_body_of_the_spec_and_the_kinds_are_written_with_dots() {
        let one = notification(1);
        assert_eq!(one.kind, NotificationKind::TurnCompleted);
        let written = serde_json::to_value(&one).unwrap();
        for member in [
            "id",
            "kind",
            "at",
            "title",
            "body",
            "url",
            "node",
            "thread_id",
            "chat_id",
            "data",
            "recipient",
        ] {
            assert!(written.get(member).is_some(), "{member}");
        }
        assert_eq!(written["kind"], "turn.completed");
        assert_eq!(
            serde_json::to_value(NotificationKind::SignInAttention).unwrap(),
            json!("sign_in.attention")
        );
        assert_eq!(
            serde_json::to_value(NotificationKind::EnvironmentIdleStop).unwrap(),
            json!("environment.idle_stop")
        );
    }

    #[test]
    fn a_message_of_several_notifications_has_them_under_the_newest_ones_members() {
        let all = [notification(1), notification(2), notification(3)];
        let body = WebhookBody::of(&all).unwrap();
        let written = serde_json::to_value(&body).unwrap();
        assert_eq!(
            written["title"], "Turn 3 finished",
            "the newest one's members"
        );
        assert_eq!(written["notifications"].as_array().unwrap().len(), 3);
        let single = serde_json::to_value(WebhookBody::of(&all[..1]).unwrap()).unwrap();
        assert!(
            single.get("notifications").is_none(),
            "a message of one is that notification's members alone"
        );
        assert!(WebhookBody::of(&[]).is_none());
        let back: WebhookBody = serde_json::from_value(written).unwrap();
        assert_eq!(back, body);
    }

    #[test]
    fn what_a_person_gave_is_split_into_the_values_and_the_secrets_after_it_is_checked() {
        let mut url = Field::new("url", "URL", FieldType::Text);
        url.required = true;
        let mut secret = Field::new("secret", "Secret", FieldType::Secret);
        secret.required = false;
        let fields = [url, secret];
        let config = Config::from_answer(
            &fields,
            &json!({ "url": "https://hooks.example/x", "secret": "s3cr3t" }),
            None,
        )
        .unwrap();
        assert_eq!(config.text("url"), Some("https://hooks.example/x"));
        assert_eq!(
            config.values.get("secret"),
            None,
            "a secret is not among the values"
        );
        assert_eq!(config.secret("secret").map(Secret::expose), Some("s3cr3t"));
        assert!(!format!("{config:?}").contains("s3cr3t"));
        let faults = Config::from_answer(&fields, &json!({ "secret": "x" }), None).unwrap_err();
        assert_eq!(faults[0].path, "url", "the field is named");
        let bare = Config::from_answer(
            &fields,
            &json!({ "url": "https://hooks.example/x" }),
            Some(vec!["10.0.0.0/8".into()]),
        )
        .unwrap();
        assert!(bare.secrets.is_empty());
        assert_eq!(
            bare.allowed_private_hosts.as_deref(),
            Some(&["10.0.0.0/8".to_owned()][..])
        );
    }

    #[test]
    fn a_secret_map_is_split_by_its_names() {
        let mut headers = Field::new("headers", "Headers", FieldType::SecretMap);
        headers.required = false;
        let config = Config::from_answer(
            &[headers],
            &json!({ "headers": { "x-key": "k1", "x-other": "k2" } }),
            None,
        )
        .unwrap();
        assert_eq!(
            config.secret("headers.x-key").map(Secret::expose),
            Some("k1")
        );
        assert_eq!(
            config.secret("headers.x-other").map(Secret::expose),
            Some("k2")
        );
    }

    #[test]
    fn a_delivery_reads_as_the_worker_writes_it() {
        let delivery: Delivery = serde_json::from_value(json!({
            "id": "0199abcd-1234-7000-8000-0000000000d1", "attempt": 2,
            "config": { "values": { "url": "https://hooks.example/x" }, "secrets": { "secret": "s" }, "allowed_private_hosts": null },
            "to": {}, "recipient": null, "language": "en", "subject": null, "body": "{\"text\":\"Turn 1 finished\"}",
            "notifications": []
        }))
        .unwrap();
        assert_eq!(delivery.attempt, 2);
        assert!(
            delivery.config.allowed_private_hosts.is_none(),
            "None: not held to the list"
        );
        assert!(!format!("{delivery:?}").contains("\"s\""));
    }

    #[test]
    fn a_taken_delivery_is_done_and_a_gone_target_is_switched_off_whatever_else_is_said() {
        let schedule = DEFAULT_RETRY_SCHEDULE;
        assert_eq!(after_delivery(&Ok(()), 1, &schedule), Next::Taken);
        let gone = DeliverError {
            retryable: true,
            ..DeliverError::gone("chat_not_found", "the chat is gone")
        };
        assert_eq!(after_delivery(&Err(gone), 1, &schedule), Next::Gone);
        assert_eq!(
            after_delivery(
                &Err(DeliverError::refused("rejected", "bad request")),
                1,
                &schedule
            ),
            Next::Failed
        );
    }

    #[test]
    fn a_delivery_that_may_be_tried_again_is_retried_on_the_schedule_and_then_fails() {
        let schedule = DEFAULT_RETRY_SCHEDULE;
        let error = Err(DeliverError::temporary("unreachable", "connection refused"));
        let pauses: Vec<_> = (1..=6)
            .map(|attempt| after_delivery(&error, attempt, &schedule))
            .collect();
        assert_eq!(
            pauses,
            [
                Next::RetryAfter(Duration::from_secs(60)),
                Next::RetryAfter(Duration::from_secs(300)),
                Next::RetryAfter(Duration::from_secs(900)),
                Next::RetryAfter(Duration::from_secs(3600)),
                Next::RetryAfter(Duration::from_secs(21_600)),
                Next::Failed
            ]
        );
        assert_eq!(
            after_delivery(&error, 1, &[]),
            Next::Failed,
            "no schedule, no retry"
        );
    }

    #[test]
    fn a_time_the_far_side_named_is_honoured_in_place_of_the_schedule() {
        let schedule = DEFAULT_RETRY_SCHEDULE;
        let slow_down = Err(DeliverError::after("rate_limited", "too many requests", 7));
        assert_eq!(
            after_delivery(&slow_down, 1, &schedule),
            Next::RetryAfter(Duration::from_secs(7))
        );
        assert_eq!(
            after_delivery(&slow_down, 3, &schedule),
            Next::RetryAfter(Duration::from_secs(7))
        );
        assert_eq!(
            after_delivery(&slow_down, 6, &schedule),
            Next::Failed,
            "the tries are still counted"
        );
    }

    #[test]
    fn what_a_check_decides_depends_on_whether_the_far_side_may_come_back() {
        assert_eq!(
            on_check(Ok(json!({ "bot_username": "orch_bot" }))),
            Saved::Kept(json!({ "bot_username": "orch_bot" }))
        );
        let wrong_token = DeliverError::refused("unauthorized", "the token is not valid");
        assert_eq!(
            on_check(Err(wrong_token.clone())),
            Saved::Refused(wrong_token)
        );
        let down = DeliverError::temporary("unreachable", "the server is down today");
        assert_eq!(on_check(Err(down.clone())), Saved::KeptWithError(down));
    }

    #[test]
    fn a_poll_of_who_followed_a_link_reads_as_the_spec_writes_it() {
        let poll: LinkPoll = serde_json::from_value(json!({
            "started": [ { "code": "abc123", "address": { "chat_id": 4242 }, "label": "Dana" } ], "cursor": "9001"
        }))
        .unwrap();
        assert_eq!(poll.started[0].label, "Dana");
        assert_eq!(LinkPoll::default().cursor, None);
        assert_eq!(DeliverError::unsupported("link people").code, "unsupported");
    }
}
