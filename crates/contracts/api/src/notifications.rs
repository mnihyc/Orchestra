//! The Inbox and its preferences; the channels out of the application that the system, a root org and a person set up, with their texts; a person's routes through them, each off until switched on, with how long a notification stays unread first and how often a message goes; push subscriptions; and the destinations of a node.
//!
//! Everything a person is notified of is written to their Inbox, a
//! [`Notification`] each. What leaves the application is a separate matter:
//! a [`NotificationChannel`] is a way out that the system, a root org or a
//! person sets up, and nothing goes through it to anybody until that person
//! switches their own [`NotificationRoute`] on. A [`Destination`] is where
//! what happens at a node is sent, for nobody in particular. Both are made
//! from a [`Notifier`], a kind of channel or of destination that the system
//! has, whose fields say what its `config` and `secrets` hold.

use std::collections::BTreeMap;

use orch_types::{Field, Id, Json, Seq, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::chats::ChatLevel;
use crate::common::{Page, PageQuery, SubscriptionLevel};
use crate::nodes::NodeRef;
use crate::routes::Access::{Permission, Rule, Session, User};
use crate::users::UserRef;

/// The most characters of the name of a channel or of a destination; the least is one.
pub const NOTIFICATION_NAME_MAX_CHARS: u32 = 80;
/// The fewest minutes a route may wait before it sends, or leave between two messages.
pub const ROUTE_MINUTES_MIN: u32 = 1;
/// The most minutes a route may wait or leave between two messages: a day.
pub const ROUTE_MINUTES_MAX: u32 = 24 * 60;
/// The last minute of a day, counted from local midnight, at which quiet hours may begin or end.
pub const QUIET_MINUTE_MAX: u32 = 24 * 60 - 1;
/// How long the link of a route that has to be linked stays good, in minutes.
pub const ROUTE_LINK_VALID_MINUTES: i64 = 10;
/// The most bytes of one template, which is refused beyond it when it is saved.
pub const TEMPLATE_MAX_BYTES: u32 = 16 * 1024;

/// A synchronous explicit act is one bounded batch, independently of its hourly policy.
pub const PERSON_NOTIFY_MAX_PEOPLE: usize = 200;
/// The header that carries the signature of a webhook's body, `sha256=` and the hex HMAC-SHA256 of
/// the body with the secret, where the webhook has a secret.
pub const WEBHOOK_SIGNATURE_HEADER: &str = "x-orch-signature";

/// A stable block, turn or message at which a person opens an explicit notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PersonNoticeAnchor {
    /// A document block.
    Block(Id),
    /// A turn of a thread's conversation.
    Turn(Id),
    /// A message of a chat.
    Message(Id),
}

/// An explicit request to tell people about something they may read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PersonNotifyBody {
    /// Up to [`PERSON_NOTIFY_MAX_PEOPLE`] identifiers, with each person told once for this act.
    #[schemars(length(max = 200))]
    pub user_ids: Vec<Id>,
    /// The part to open; it must belong to the target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<PersonNoticeAnchor>,
    /// An optional note from the sender.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A narrowly scoped grant of access to a chat, optionally with a notice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatPersonGrantBody {
    /// View or participate, as offered for this chat.
    pub choice: crate::access::AccessChoice,
    /// Tell the person in the same transaction as the grant.
    #[serde(default)]
    pub notify: bool,
    /// A live chat message to open from the notice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<PersonNoticeAnchor>,
}

/// The actions available for a person in the context of a readable thing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PersonNotifyContext {
    /// Whether they may read it, or null when its holders are not visible to the caller.
    pub can_view: Option<bool>,
    /// Whether an explicit notice can be sent to them here.
    pub can_notify: bool,
    /// The access choices the caller can give this person here.
    pub gives: Vec<crate::access::AccessChoice>,
    /// The resolved maximum number of characters in an optional note.
    pub note_chars: i64,
}

/// What a notification is about. Who receives each kind is a rule of the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum NotificationKind {
    /// One scheduled occurrence could not start under its author's current authority.
    #[serde(rename = "schedule.cannot_start")]
    ScheduleCannotStart,
    /// A turn finished.
    #[serde(rename = "turn.completed")]
    TurnCompleted,
    /// A turn failed.
    #[serde(rename = "turn.failed")]
    TurnFailed,
    /// A sub-agent finished, failed or was stopped, where the thread's place asks for it (`notify.subagents`).
    #[serde(rename = "subagent.ended")]
    SubagentEnded,
    /// A goal ended, for the person who asked to be told: it was met, or stopped without being met.
    #[serde(rename = "goal.ended")]
    GoalEnded,
    /// A thread cannot go on until somebody does something.
    #[serde(rename = "thread.blocked")]
    ThreadBlocked,
    /// An approval waits for an answer.
    #[serde(rename = "approval.requested")]
    ApprovalRequested,
    /// An environment asked for a capability.
    #[serde(rename = "capability.requested")]
    CapabilityRequested,
    /// Somebody asked for access.
    #[serde(rename = "access.requested")]
    AccessRequested,
    /// A request for access was decided.
    #[serde(rename = "access.decided")]
    AccessDecided,
    /// An agent reached a milestone.
    #[serde(rename = "milestone")]
    Milestone,
    /// An announcement was sent to a topic or a project.
    #[serde(rename = "announcement")]
    Announcement,
    /// The person was mentioned.
    #[serde(rename = "mention")]
    Mention,
    /// A message came to a chat the person follows.
    #[serde(rename = "chat.message")]
    ChatMessage,
    /// The person was named in a document, or in a comment beside one.
    #[serde(rename = "doc.mention")]
    DocMention,
    /// A person explicitly sent a notice about something the recipient can read.
    #[serde(rename = "person.notify")]
    PersonNotify,
    /// A discussion beside a document was answered.
    #[serde(rename = "doc.comment")]
    DocComment,
    /// A provider ran out of what it allows.
    #[serde(rename = "provider.exhausted")]
    ProviderExhausted,
    /// A provider needs attention.
    #[serde(rename = "provider.attention")]
    ProviderAttention,
    /// A machine went offline while a thread ran on it.
    #[serde(rename = "machine.offline")]
    MachineOffline,
    /// An environment is about to be, or was, stopped for being idle.
    #[serde(rename = "environment.idle_stop")]
    EnvironmentIdleStop,
    /// An environment fell behind, or an update applied at its start was rolled back.
    #[serde(rename = "environment.update")]
    EnvironmentUpdate,
    /// An environment needs attention.
    #[serde(rename = "environment.attention")]
    EnvironmentAttention,
    /// A cap is close to being reached.
    #[serde(rename = "cap.warning")]
    CapWarning,
    /// A cap has been reached.
    #[serde(rename = "cap.reached")]
    CapReached,
    /// A job finished.
    #[serde(rename = "job.finished")]
    JobFinished,
    /// The person was invited.
    #[serde(rename = "invitation")]
    Invitation,
    /// The person's domain was changed.
    #[serde(rename = "member.moved")]
    MemberMoved,
    /// Something about the person's own sign-in needs their attention.
    #[serde(rename = "sign_in.attention")]
    SignInAttention,
    /// A way out of the application that the person switched on stopped reaching them: Inbox only.
    #[serde(rename = "notification.route_attention")]
    NotificationRouteAttention,
}

/// The place a notification is grouped under when the Inbox is read by place, as the caller may
/// read it now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NotificationGroup {
    /// The group's key, which a read of the whole group names.
    pub key: String,
    /// The nearest place the caller may read; `null` for what is about the person alone.
    pub place: Option<NodeRef>,
    /// How many of the group are unread, under the filters of the request.
    pub unread: i64,
    /// How many of the group the caller can act on now.
    pub waiting_for_me: i64,
    /// The highest Inbox sequence the group was counted up to: the cut a read of the group uses.
    pub through_seq: Seq,
}

/// Versioned action data from which a client writes a notification in the person's language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NotificationActionDetails {
    /// The action vocabulary version; this release writes one.
    pub v: u32,
    /// The stable action kind, independent of stored English fallback text.
    pub action: NotificationKind,
    /// Safe action parameters: ids, names, quantities, states and the path already visible to the recipient.
    /// A settled request notice includes its actual approval_state and nullable decided_by;
    /// actor carries that decider's current display name. The same notice id and kind are retained.
    pub parameters: Json,
}

/// One entry of a person's Inbox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Notification {
    /// The notification's identifier.
    pub id: Id,
    /// What it is about.
    pub kind: NotificationKind,
    /// A line saying what happened.
    pub title: String,
    /// More about it; may be empty.
    pub body: String,
    /// The person who caused it, where one was supplied; not a directory of other people.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<UserRef>,
    /// The place it happened in.
    pub node: Option<NodeRef>,
    /// The thread it is about.
    pub thread_id: Option<Id>,
    /// The thread's current name, only as far as this reader may know it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<NodeRef>,
    /// Its current path, with private ancestors redacted; supplied with the Inbox rather than read per row.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<NodeRef>,
    /// The approval it is about.
    pub approval_id: Option<Id>,
    /// The chat it is about.
    pub chat_id: Option<Id>,
    /// The path in the application that opens it.
    pub url: String,
    /// What else the kind carries.
    pub data: Json,
    /// Structured action data; absent for older rows whose stored text remains the fallback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<NotificationActionDetails>,
    /// When it was written; a chat message renews it.
    pub created_at: Time,
    /// When the person read it.
    pub read_at: Option<Time>,
    /// Its place in the person's Inbox, which only grows.
    pub inbox_seq: Seq,
    /// Its group, when the Inbox is read by place; `null` otherwise.
    pub group: Option<NotificationGroup>,
    /// Whether it asks for something the caller can do now.
    pub waiting_for_me: bool,
}

/// Whether a way out that is on can reach the person now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OutsideWayState {
    /// It is on and nothing is wrong with it.
    Ready,
    /// It failed, lost its target or its channel, and says what would repair it.
    NeedsAttention,
}

/// One way out that is on, or that failed, as the summary names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OutsideWay {
    /// The route.
    pub route_id: Id,
    /// The name of its channel, as the person may know it.
    pub name: String,
    /// The notifier of its channel.
    pub kind: String,
    /// Whether it can reach the person now.
    pub state: OutsideWayState,
    /// When its notifier last accepted something for the person: no proof that a device showed it.
    pub last_ok_at: Option<Time>,
}

/// Whether the person is told outside the application, in a scope: their pause and their ways.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OutsideStatus {
    /// The place the summary was asked for; `null` for all of the caller's ways.
    pub scope: Option<NodeRef>,
    /// Until when every way is paused; `null` when no pause holds now.
    pub pause_until: Option<Time>,
    /// The ways that are on and ready, or that need attention, in that scope.
    pub ways: Vec<OutsideWay>,
}

/// Quiet hours, in the person's own time zone, which every route holds still for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct QuietHours {
    /// The minute after local midnight at which they begin, at most [`QUIET_MINUTE_MAX`].
    pub start_minute: u32,
    /// The minute after local midnight at which they end, at most [`QUIET_MINUTE_MAX`].
    pub end_minute: u32,
}

/// What a person chose about being notified, apart from their routes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NotificationPrefs {
    /// Kinds this person chose not to receive in their Inbox or through any route.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled_kinds: Vec<NotificationKind>,
    /// The revision a change must name, so that a change made from a stale view is refused.
    pub revision: i64,
    /// Until when every way out is paused, one moment in UTC; `null` when no pause holds now.
    pub pause_until: Option<Time>,
    /// Whether to be told of approvals they can answer even where they are not subscribed.
    pub approvals_always: bool,
    /// Their level in the chat of a project, topic or thread, until they set one there.
    pub chat_node_level: ChatLevel,
    /// Their level in direct and group chats, until they set one there.
    pub chat_direct_level: ChatLevel,
    /// Hours, in the person's time zone, during which every route holds its messages until the window
    /// ends; `null` for none.
    pub quiet_hours: Option<QuietHours>,
}

/// A text with placeholders, which a channel or a destination sends in place of the shipped one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TemplateText {
    /// The subject, for mail; other notifiers ignore it.
    pub subject: Option<String>,
    /// The body.
    pub body: String,
}

/// What a notifier ships as its texts, in the language of the caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NotifierTemplates {
    /// For one notification.
    pub single: TemplateText,
    /// For several in one message.
    pub digest: TemplateText,
}

/// The texts a channel or a destination has of its own for one language. A form it leaves out
/// sends the shipped text.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LanguageTemplates {
    /// For one notification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub single: Option<TemplateText>,
    /// For several in one message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<TemplateText>,
}

/// The texts of a channel or a destination by language tag, or `*` for every language without
/// its own. A language it does not list sends the shipped texts.
pub type TemplatesByLanguage = BTreeMap<String, LanguageTemplates>;

/// What one of a notifier can be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotifierTarget {
    /// A channel, which reaches a person.
    Person,
    /// A destination, which a node is told at.
    Destination,
}

/// Who may set up a channel of a notifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotifierOwner {
    /// The system, for everybody.
    System,
    /// A root org, for its members.
    Org,
    /// A person, for themself.
    User,
}

/// A kind of channel or of destination that this system has.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Notifier {
    /// Its name, which a channel names as its kind and a destination as its channel.
    pub name: String,
    /// What it is called.
    pub title: String,
    /// What it does, in a sentence.
    pub summary: String,
    /// What one of it can be: a channel, which reaches a person; a destination, which a node is
    /// told at. A webhook is both.
    pub targets: Vec<NotifierTarget>,
    /// What one of it is made from.
    pub fields: Vec<Field>,
    /// For a channel, who may set one up; empty for a destination's notifier.
    pub owners: Vec<NotifierOwner>,
    /// Whether the system and each root org have at most one of it, as with a mail server or a bot.
    pub single: bool,
    /// Whether a person's route through it has to be linked before it can be on, as with a bot.
    pub links: bool,
    /// Its shipped texts, in the caller's language.
    pub templates: NotifierTemplates,
}

/// Whose a channel is.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotificationChannelOwner {
    /// The system's, for everybody.
    System,
    /// A root org's, for its members.
    Org(NodeRef),
    /// The caller's own.
    Me,
}

/// What a route lacks before it can be on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotificationRouteNeeds {
    /// A confirmed address, for mail.
    VerifiedEmail,
    /// A browser that took push.
    Device,
    /// A bot that was started.
    Link,
}

/// The link of a route that has to be linked, while it waits to be pressed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NotificationRouteLink {
    /// The address that opens the bot with a code.
    pub url: String,
    /// When it stops being good.
    pub expires_at: Time,
}

/// How a route fared when it last sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotificationRouteHealth {
    /// Nothing went through it yet.
    Never,
    /// Its notifier accepted the last message.
    Ok,
    /// The last message failed after its tries, or was refused for good.
    Failed,
    /// Push reached some of the person's browsers and not others.
    Partial,
    /// What it sent to is gone: a browser, a chat, an address.
    TargetGone,
    /// Its channel was removed; the route is kept, off, until the person removes it.
    ChannelRemoved,
}

/// The channel of a route as it is kept with the route: what is safe to show, also after the
/// channel is removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NotificationRouteChannel {
    /// The channel's identifier, as it was.
    pub id: Id,
    /// Its notifier.
    pub kind: String,
    /// Its name.
    pub name: String,
    /// The root org it belonged to, as the caller may know it now; `null` for the system's, the
    /// person's own, or a place the caller can no longer see.
    pub org: Option<NodeRef>,
}

/// Why a route stopped reaching its person, without any message, address or token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NotificationRouteFailure {
    /// A word for the reason: `exhausted`, `refused`, `target_gone`, `channel_removed`.
    pub code: String,
    /// The safe reason, as the far side or the system gave it.
    pub message: String,
    /// The names of the person's browsers that push no longer reaches, where it is push.
    pub affected_devices: Vec<String>,
}

/// How one person is told through one channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NotificationRoute {
    /// The route's identifier; `null` only for a route that was never made, which is off.
    pub id: Option<Id>,
    /// The channel; `null` once the channel was removed.
    pub channel_id: Option<Id>,
    /// The channel as it is kept with the route; `null` for a route that was never made.
    pub channel: Option<NotificationRouteChannel>,
    /// Whether it is on. Every route starts off.
    pub enabled: bool,
    /// How it fared when it last sent.
    pub health: NotificationRouteHealth,
    /// When its notifier last accepted a message for the person: no proof that a device showed it.
    pub last_ok_at: Option<Time>,
    /// When it began failing, while it fails.
    pub failed_at: Option<Time>,
    /// Why it fails, while it fails.
    pub failure: Option<NotificationRouteFailure>,
    /// How many unread notifications it would carry now, as far as the person may still read them.
    pub waiting_count: i64,
    /// What is missing before it can be on.
    pub needs: Option<NotificationRouteNeeds>,
    /// A bot's link, while it waits to be pressed.
    pub link: Option<NotificationRouteLink>,
    /// Whom a linked route reaches, as the far side named them: for a bot, the name of whoever
    /// pressed Start, so that a person sees it is theirs.
    pub label: Option<String>,
    /// What to be told of; empty for everything the caller is notified of in the channel's scope.
    pub kinds: Vec<NotificationKind>,
    /// `null`: at once. Otherwise only what is still unread after that many minutes.
    pub unread_delay_minutes: Option<u32>,
    /// `null`: no limit. Otherwise at most one message in that many minutes.
    pub min_interval_minutes: Option<u32>,
    /// When it last sent.
    pub last_sent_at: Option<Time>,
}

/// A way out of the application to a person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NotificationChannel {
    /// The channel's identifier.
    pub id: Id,
    /// The notifier it is made from.
    pub kind: String,
    /// Its name.
    pub name: String,
    /// Whose it is.
    pub owner: NotificationChannelOwner,
    /// What the notifier's fields hold, for whoever may change it; `{}` for everybody else.
    pub config: Json,
    /// The names of its secret fields that have a value, for whoever may change it; `[]` for
    /// everybody else.
    pub secrets_set: Vec<String>,
    /// Its own texts by language; a language it does not list sends the shipped ones.
    pub templates: TemplatesByLanguage,
    /// Whether it sends.
    pub enabled: bool,
    /// When a delivery through it last succeeded.
    pub last_ok_at: Option<Time>,
    /// Why its last delivery failed; cleared by the next that succeeds.
    pub last_error: Option<String>,
    /// Whether the caller may change it.
    pub can_edit: bool,
    /// How many people have a route through it switched on: a number for whoever looks after the
    /// channel, never who they are; `null` for everybody else.
    pub enabled_route_count: Option<i64>,
    /// The caller's own switch for it; off until they say otherwise.
    pub route: NotificationRoute,
}

/// A browser of the caller's that took push, without its endpoint or its keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PushDevice {
    /// The subscription's identifier.
    pub id: Id,
    /// A name for it from what the browser said of itself.
    pub label: String,
    /// The SHA-256 of the endpoint's UTF-8, in lower-case hex: a browser finds itself by it without
    /// the endpoint being shown.
    pub endpoint_hash: String,
    /// When push last reached it.
    pub last_ok_at: Option<Time>,
}

/// How the Inbox is read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotificationGrouping {
    /// Newest first, in the order of days.
    #[default]
    Day,
    /// By the place each is about, what the caller can act on first.
    Place,
}

/// How much of what happens at a node a destination is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DestinationLevel {
    /// Everything a subscriber at `all` would get.
    All,
    /// What a subscriber at `important` would get.
    Important,
}

/// Where what happens at a node is sent, for nobody in particular: a team's room in a chat tool,
/// a build system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Destination {
    /// The destination's identifier.
    pub id: Id,
    /// The notifier it is made from.
    pub channel: String,
    /// Its name.
    pub name: String,
    /// The node it is told what happens at, and below.
    pub node: NodeRef,
    /// What the notifier's fields hold.
    pub config: Json,
    /// The names of its secret fields that have a value.
    pub secrets_set: Vec<String>,
    /// Its own texts by language.
    pub templates: TemplatesByLanguage,
    /// The kinds it is sent; empty for every kind it would be sent.
    pub kinds: Vec<String>,
    /// What a subscriber at that level would get.
    pub level: DestinationLevel,
    /// Whether it sends.
    pub enabled: bool,
    /// When a delivery to it last succeeded.
    pub last_ok_at: Option<Time>,
    /// Why its last delivery failed; cleared by the next that succeeds.
    pub last_error: Option<String>,
    /// When it was made.
    pub created_at: Time,
}

/// One notification as a webhook is posted it: the members of the Inbox row, with `at` for the
/// moment it was written, and whom it is for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WebhookNotification {
    /// The notification's identifier.
    pub id: Id,
    /// What it is about.
    pub kind: NotificationKind,
    /// When it was written.
    pub at: Time,
    /// A line saying what happened.
    pub title: String,
    /// More about it.
    pub body: String,
    /// The path in the application that opens it.
    pub url: String,
    /// The place it happened in.
    pub node: Option<NodeRef>,
    /// The thread it is about.
    pub thread_id: Option<Id>,
    /// The chat it is about.
    pub chat_id: Option<Id>,
    /// What else the kind carries.
    pub data: Json,
    /// Whom it is for: `null` for a node's destination, the person for a channel's webhook.
    pub recipient: Option<UserRef>,
}

/// The body a webhook, as a channel or as a destination, is posted, with the header
/// `X-Orch-Signature: sha256=<hex HMAC-SHA256 of the body with the secret>` where it has a secret.
/// A message that carries several notifications, as a route with a frequency limit sends, has
/// them as `notifications` under the members of the newest one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WebhookDelivery {
    /// The newest notification.
    #[serde(flatten)]
    pub newest: WebhookNotification,
    /// All of those the message carries, where it carries several.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notifications: Option<Vec<WebhookNotification>>,
}

/// Which notifications are wanted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NotificationListQuery {
    /// Only those not read yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unread: Option<bool>,
    /// How many entries, at most [`crate::common::MAX_LIMIT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The `next_cursor` of the page before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Only these kinds: one kind, or several separated by commas, as `mention,doc.mention`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Only those written at or after this moment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<Time>,
    /// By day, newest first (the default), or by place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_by: Option<NotificationGrouping>,
}

/// The scope of the summary.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NotificationSummaryGetQuery {
    /// A place the caller may read: only the ways out that cover it are named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<Id>,
}

/// How many notifications wait, for the badge, and whether the caller is told outside.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NotificationSummaryGetReply {
    /// How many are unread.
    pub unread: i64,
    /// How many approvals the caller can answer wait.
    pub approvals_pending: i64,
    /// The highest Inbox sequence issued to the caller.
    pub seq: Seq,
    /// The caller's pause and their ways out, in the scope asked for.
    pub outside: OutsideStatus,
}

/// Marks notifications read, in exactly one of three forms: what is listed, a node's, a chat's
/// or what is older than a moment; a group up to its cut; or the undoing of a group read by its
/// ticket.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NotificationReadBody {
    /// The group to mark read, as the list named it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_key: Option<String>,
    /// The group's cut, as the list gave it: nothing newer is marked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub through_seq: Option<Seq>,
    /// The ticket a group read gave, to undo it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo_token: Option<String>,
    /// These notifications.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids: Option<Vec<Id>>,
    /// Those of this node; opening a thread sends its identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<Id>,
    /// Those of this chat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<Id>,
    /// Those written before this moment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Time>,
}

/// How many notifications are still unread, what the read changed, and how a group read is undone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NotificationReadReply {
    /// How many are unread now.
    pub unread: i64,
    /// How many rows this read changed.
    pub changed: i64,
    /// For a group read that changed something: the sealed ticket that undoes it.
    pub undo_token: Option<String>,
    /// Until when the ticket undoes it.
    pub undo_expires_at: Option<Time>,
}

/// Changes what a person chose about being notified; what is left out stays.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MeNotificationPrefsSetBody {
    /// Replaces the kinds the person chose not to receive; an empty list enables every kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled_kinds: Option<Vec<NotificationKind>>,
    /// The revision the change was made from; another one is refused as stale.
    pub expected_revision: i64,
    /// Pause every way out until this moment, which must be in the future; `null` resumes.
    #[serde(
        default,
        deserialize_with = "crate::common::absent_or_null",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<Time>")]
    pub pause_until: Option<Option<Time>>,
    /// Whether to be told of approvals they can answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approvals_always: Option<bool>,
    /// Their level in the chat of a project, topic or thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_node_level: Option<ChatLevel>,
    /// Their level in direct and group chats.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_direct_level: Option<ChatLevel>,
    /// Quiet hours; `null` switches them off.
    #[serde(
        default,
        deserialize_with = "crate::common::absent_or_null",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<QuietHours>")]
    pub quiet_hours: Option<Option<QuietHours>>,
}

/// The keys of a browser's push subscription.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PushSubscriptionKeys {
    /// The browser's public key.
    pub p256dh: String,
    /// The secret it shares with the sender.
    pub auth: String,
}

/// A browser of the caller's that takes push. The route of the push channel is what switches push
/// on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MePushSubscriptionCreateBody {
    /// Where the push service is told.
    pub endpoint: String,
    /// The keys that seal what is sent.
    pub keys: PushSubscriptionKeys,
}

/// Takes a browser out of the push subscriptions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MePushSubscriptionDeleteBody {
    /// The endpoint it was subscribed with.
    pub endpoint: String,
}

/// The channels a caller is shown.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NotificationChannelListQuery {
    /// For whoever holds `settings.manage` on that root org: that org's channels, to look after.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<Id>,
}

/// Sets up a channel of the system or of a root org. Its `config` is checked against its
/// notifier's fields, a secret field is write-only, and its notifier looks at it before it is
/// saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NotificationChannelCreateBody {
    /// The notifier it is made from.
    pub kind: String,
    /// Its name, one to [`NOTIFICATION_NAME_MAX_CHARS`] characters.
    pub name: String,
    /// What the notifier's fields ask for, without the secret ones.
    pub config: Json,
    /// The secret fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secrets: Option<BTreeMap<String, String>>,
    /// Its own texts by language.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub templates: Option<TemplatesByLanguage>,
    /// Whether it sends; on unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

/// What a webhook of a person's own is made from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WebhookConfig {
    /// Where it is posted.
    pub url: String,
}

/// The secret of a webhook of a person's own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WebhookSecrets {
    /// What the body is signed with.
    pub secret: String,
}

/// Sets up a webhook of the caller's own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MeNotificationChannelCreateBody {
    /// Its name.
    pub name: String,
    /// Where it is posted.
    pub config: WebhookConfig,
    /// What signs the body, where it is signed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secrets: Option<WebhookSecrets>,
    /// Its own texts by language.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub templates: Option<TemplatesByLanguage>,
}

/// Changes a channel; what is left out stays.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NotificationChannelUpdateBody {
    /// A new name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// New values for the notifier's fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<Json>,
    /// New values for its secret fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secrets: Option<BTreeMap<String, String>>,
    /// New texts by language.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub templates: Option<TemplatesByLanguage>,
    /// Whether it sends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

/// Sends one sample message through a channel.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NotificationChannelTestBody {
    /// For a mail server, the address to send to; otherwise the caller, as their route reaches them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// For push, one browser of the caller's own to send to; every browser of theirs otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<Id>,
}

/// What came of a sample delivery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NotificationTestReply {
    /// Whether the far side took it.
    pub ok: bool,
    /// What the far side answered, when it did not.
    pub error: Option<String>,
}

/// Changes the caller's route through a channel; what is left out stays.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MeNotificationRouteSetBody {
    /// Whether to be told through it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// What to be told of; empty for everything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<NotificationKind>>,
    /// Minutes a notification has to stay unread before it goes out; `null` for at once.
    #[serde(
        default,
        deserialize_with = "crate::common::absent_or_null",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<u32>")]
    pub unread_delay_minutes: Option<Option<u32>>,
    /// Minutes at most one message goes out in; `null` for no limit.
    #[serde(
        default,
        deserialize_with = "crate::common::absent_or_null",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<u32>")]
    pub min_interval_minutes: Option<Option<u32>>,
    /// Send again, as one message, what failed and is still unread; the route must be on and ready.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_failed: Option<bool>,
}

/// Sets up a destination at a node: an org at any level, a project or a topic. `config` and
/// `secrets` are what its notifier's fields ask for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeDestinationCreateBody {
    /// The notifier it is made from; `webhook` unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    /// Its name, one to [`NOTIFICATION_NAME_MAX_CHARS`] characters.
    pub name: String,
    /// What the notifier's fields ask for, without the secret ones.
    pub config: Json,
    /// The secret fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secrets: Option<BTreeMap<String, String>>,
    /// Its own texts by language.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub templates: Option<TemplatesByLanguage>,
    /// The kinds it is sent; empty for every kind it would be sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<String>>,
    /// What it is sent; `important` unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<DestinationLevel>,
    /// Whether it sends; on unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

/// Changes a destination; what is left out stays.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DestinationUpdateBody {
    /// A new name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// New values for the notifier's fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<Json>,
    /// New values for its secret fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secrets: Option<BTreeMap<String, String>>,
    /// New texts by language.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub templates: Option<TemplatesByLanguage>,
    /// The kinds it is sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<String>>,
    /// What it is sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<DestinationLevel>,
    /// Whether it sends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

/// One manual subscription that differs from what this person inherits above this place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NotificationException {
    /// The readable place; entries never include a place the caller cannot view.
    pub node: NodeRef,
    /// Its ancestors, root first, with inaccessible names redacted.
    pub path: Vec<NodeRef>,
    /// The person's explicit level here.
    pub level: SubscriptionLevel,
    /// The level that would apply without this row; Off where none was set above.
    pub default_level: SubscriptionLevel,
    /// The ancestor supplying that level, if any.
    pub default_from: Option<NodeRef>,
}

/// Uses the ordinary node subscription mechanism; null removes this person's explicit row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NotificationExceptionSetBody {
    /// A chosen level, or null to inherit.
    pub level: Option<SubscriptionLevel>,
}

/// The authoritative current page after a change, including newly exposed descendant exceptions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NotificationExceptionChanged {
    /// The changed node's current access and subscription, as its ordinary bell returns.
    pub access: crate::nodes::Access,
    /// The requested exception page after that change.
    pub exceptions: Page<NotificationException>,
    /// The requested page boundary, omitted for the first page, so clients replace that exact page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_cursor: Option<String>,
}

crate::endpoints! {
    /// Give one person the offered access to this chat, optionally notifying them atomically.
    ChatPersonGrant: POST "/chats/{id}/permissions/people/{user_id}", Rule("the caller may grant the chat access"), none, json ChatPersonGrantBody, none, 204;
    /// The caller's active document-notification work, with bounded pages and ordinary job progress.
    DocumentNoticeJobList: GET "/documents/{id}/notification-jobs", Rule("the caller may read the document; only their own jobs are listed"), query PageQuery, none, json Page<crate::workspace::Job>, 200;
    /// Tells the named current readers about this thing once each for this act.
    PersonNotifyCreate: POST "/{object}/{id}/notify", Rule("the sender and each recipient may read the thing"), none, json PersonNotifyBody, none, 204;
    /// Reads the actions available for one person beside this thing.
    PersonNotifyContextGet: GET "/{object}/{id}/notify/people/{user_id}", Rule("the caller may read the thing"), none, none, json PersonNotifyContext, 200;
    /// The caller's manual notification exceptions, by node id, checked against current View.
    MeNotificationExceptionList: GET "/me/notification-exceptions", User, query PageQuery, none, json Page<NotificationException>, 200;
    /// Changes the caller's ordinary node subscription and returns the current exceptions page.
    MeNotificationExceptionSet: PUT "/me/notification-exceptions/{id}", Permission("node.view"), query PageQuery, json NotificationExceptionSetBody, json NotificationExceptionChanged, 200;
    /// Lists the caller's Inbox, newest first.
    NotificationList: GET "/notifications", User, query NotificationListQuery, none, json Page<Notification>, 200;
    /// Counts what waits for the caller, for the badge, with their pause and their ways out in a scope.
    NotificationSummaryGet: GET "/notifications/summary", User, query NotificationSummaryGetQuery, none, json NotificationSummaryGetReply, 200;
    /// Marks notifications of the caller's Inbox read.
    NotificationRead: POST "/notifications/read", User, none, json NotificationReadBody, json NotificationReadReply, 200;
    /// Reads what the caller chose about being notified.
    MeNotificationPrefsGet: GET "/me/notification-prefs", User, none, none, json NotificationPrefs, 200;
    /// Changes what the caller chose about being notified.
    MeNotificationPrefsSet: PUT "/me/notification-prefs", User, none, json MeNotificationPrefsSetBody, json NotificationPrefs, 200;
    /// Subscribes a browser of the caller's to push; the route of the push channel is what switches push on.
    MePushSubscriptionCreate: POST "/me/push-subscriptions", User, none, json MePushSubscriptionCreateBody, none, 204;
    /// Takes a browser out of the caller's push subscriptions.
    MePushSubscriptionDelete: DELETE "/me/push-subscriptions", User, none, json MePushSubscriptionDeleteBody, none, 204;
    /// Lists the caller's own browsers that took push, without their endpoints or keys.
    MePushSubscriptionList: GET "/me/push-subscriptions", User, none, none, json Vec<PushDevice>, 200;
    /// Takes one of the caller's own browsers out of push; the others stay.
    MePushSubscriptionRemove: DELETE "/me/push-subscriptions/{id}", User, none, none, none, 204;
    /// Lists the caller's own routes, with those whose channel was removed until they remove them.
    MeNotificationRouteList: GET "/me/notification-routes", User, none, none, json Vec<NotificationRoute>, 200;
    /// Removes one of the caller's own routes by its identifier: one that is off, or kept after its channel was removed.
    MeNotificationRouteRemove: DELETE "/me/notification-routes/by-id/{route_id}", User, none, none, none, 204;
    /// Lists the kinds of channel and of destination this system has: the built-in ones and each notifier that was installed.
    NotifierList: GET "/notifiers", User, none, none, json Vec<Notifier>, 200;
    /// Lists the ways out that can reach the caller, each with their own route: the system's, those of the root orgs they belong to and their own.
    NotificationChannelList: GET "/notification-channels", Rule("any signed-in caller; with org_id, settings.manage on that root org, to look after its channels; a system admin also gets the system's with their fields"), query NotificationChannelListQuery, none, json Vec<NotificationChannel>, 200;
    /// Sets up a channel of the system: its mail server, its bot, a webhook, push. A system admin.
    AdminNotificationChannelCreate: POST "/admin/notification-channels", Session, none, json NotificationChannelCreateBody, json NotificationChannel, 200;
    /// Sets up a channel of a root org, for its members; a second mail server or bot is refused.
    OrgNotificationChannelCreate: POST "/orgs/{id}/notification-channels", Permission("settings.manage"), none, json NotificationChannelCreateBody, json NotificationChannel, 200;
    /// Sets up a webhook of the caller's own.
    MeNotificationChannelCreate: POST "/me/notification-channels", User, none, json MeNotificationChannelCreateBody, json NotificationChannel, 200;
    /// Changes a channel.
    NotificationChannelUpdate: PATCH "/notification-channels/{id}", Rule("whoever may set that channel up: a system admin for the system's, settings.manage on the root org for an org's, its person for their own"), none, json NotificationChannelUpdateBody, json NotificationChannel, 200;
    /// Deletes a channel, which ends every route through it.
    NotificationChannelDelete: DELETE "/notification-channels/{id}", Rule("whoever may set that channel up: a system admin for the system's, settings.manage on the root org for an org's, its person for their own"), none, none, none, 204;
    /// Sends one sample message through a channel, to the caller as their route reaches them or, for a mail server, to the address given.
    NotificationChannelTest: POST "/notification-channels/{id}/test", Rule("whoever may change the channel, and anybody for a channel their route is linked on"), none, json NotificationChannelTestBody, json NotificationTestReply, 200;
    /// Changes the caller's own switch for a channel.
    MeNotificationRouteSet: PUT "/me/notification-routes/{channel_id}", User, none, json MeNotificationRouteSetBody, json NotificationRoute, 200;
    /// Makes the link that opens a bot with a code, good for ten minutes, for a channel whose notifier links.
    MeNotificationRouteLinkCreate: POST "/me/notification-routes/{channel_id}/link", User, none, none, json NotificationRouteLink, 200;
    /// Switches the caller's route off and unlinks it.
    MeNotificationRouteDelete: DELETE "/me/notification-routes/{channel_id}", User, none, none, none, 204;
    /// Lists the destinations of a node.
    NodeDestinationList: GET "/nodes/{id}/destinations", Permission("settings.manage"), none, none, json Vec<Destination>, 200;
    /// Sets up a destination at a node, which is an org at any level, a project or a topic.
    NodeDestinationCreate: POST "/nodes/{id}/destinations", Permission("settings.manage"), none, json NodeDestinationCreateBody, json Destination, 200;
    /// Changes a destination.
    DestinationUpdate: PATCH "/destinations/{id}", Rule("settings.manage on its node"), none, json DestinationUpdateBody, json Destination, 200;
    /// Deletes a destination.
    DestinationDelete: DELETE "/destinations/{id}", Rule("settings.manage on its node"), none, none, none, 204;
    /// Delivers one sample notification to a destination now, and says what came back.
    DestinationTest: POST "/destinations/{id}/test", Rule("settings.manage on its node"), none, none, json NotificationTestReply, 200;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_explicit_notice_keeps_its_recipient_anchor_and_note() {
        let written = json!({
            "user_ids": ["0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b"],
            "anchor": {"block": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6c"},
            "note": "Please take a look"
        });
        let body: PersonNotifyBody = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(serde_json::to_value(body).unwrap(), written);
        let message = json!({"message": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6d"});
        let anchor: PersonNoticeAnchor = serde_json::from_value(message.clone()).unwrap();
        assert_eq!(serde_json::to_value(anchor).unwrap(), message);
        assert!(
            serde_json::from_value::<PersonNotifyBody>(json!({"user_ids": [], "extra": true}))
                .is_err()
        );
        assert_eq!(
            serde_json::to_value(NotificationKind::PersonNotify).unwrap(),
            "person.notify"
        );
    }

    #[test]
    fn a_channel_of_a_root_org_is_read_with_the_route_of_its_caller() {
        let written = json!({
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b",
            "kind": "telegram",
            "name": "Acme bot",
            "owner": { "org": {
                "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6c", "kind": "org", "name": "Acme", "redacted": false
            } },
            "config": { "bot_username": "acme_orch_bot" },
            "secrets_set": ["bot_token"],
            "templates": { "*": { "single": { "subject": null, "body": "{{title}}\n{{url}}" } } },
            "enabled": true,
            "last_ok_at": "2026-10-08T17:04:05.123Z",
            "last_error": null,
            "can_edit": true,
            "enabled_route_count": 3,
            "route": {
                "id": null,
                "channel_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b",
                "channel": {
                    "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b", "kind": "telegram", "name": "Acme bot", "org": null
                },
                "enabled": false,
                "health": "never",
                "last_ok_at": null,
                "failed_at": null,
                "failure": null,
                "waiting_count": 0,
                "needs": "link",
                "link": { "url": "https://t.me/acme_orch_bot?start=abc", "expires_at": "2026-10-08T17:14:05.123Z" },
                "label": null,
                "kinds": ["turn.failed", "approval.requested"],
                "unread_delay_minutes": 5,
                "min_interval_minutes": null,
                "last_sent_at": null
            }
        });
        let channel: NotificationChannel = serde_json::from_value(written.clone()).unwrap();
        assert!(matches!(channel.owner, NotificationChannelOwner::Org(_)));
        assert_eq!(channel.route.needs, Some(NotificationRouteNeeds::Link));
        assert_eq!(
            channel.route.kinds,
            [
                NotificationKind::TurnFailed,
                NotificationKind::ApprovalRequested
            ]
        );
        assert_eq!(serde_json::to_value(&channel).unwrap(), written);
    }

    #[test]
    fn the_owner_of_a_channel_is_a_word_or_an_org() {
        assert_eq!(
            serde_json::to_value([
                NotificationChannelOwner::System,
                NotificationChannelOwner::Me
            ])
            .unwrap(),
            json!(["system", "me"])
        );
    }

    #[test]
    fn a_kind_of_notification_is_written_with_its_dots() {
        for (kind, written) in [
            (NotificationKind::TurnCompleted, "turn.completed"),
            (
                NotificationKind::EnvironmentIdleStop,
                "environment.idle_stop",
            ),
            (NotificationKind::SignInAttention, "sign_in.attention"),
            (NotificationKind::Milestone, "milestone"),
        ] {
            assert_eq!(serde_json::to_value(kind).unwrap(), json!(written));
            assert_eq!(
                serde_json::from_value::<NotificationKind>(json!(written)).unwrap(),
                kind
            );
        }
    }

    #[test]
    fn quiet_hours_are_switched_off_by_null_and_kept_by_leaving_them_out() {
        let off: MeNotificationPrefsSetBody =
            serde_json::from_value(json!({ "expected_revision": 2, "quiet_hours": null })).unwrap();
        let kept: MeNotificationPrefsSetBody =
            serde_json::from_value(json!({ "expected_revision": 2, "approvals_always": false }))
                .unwrap();
        let set: MeNotificationPrefsSetBody = serde_json::from_value(json!({
            "expected_revision": 2,
            "quiet_hours": { "start_minute": 1320, "end_minute": 420 }
        }))
        .unwrap();
        assert_eq!(off.quiet_hours, Some(None));
        assert_eq!(kept.quiet_hours, None);
        assert_eq!(
            set.quiet_hours,
            Some(Some(QuietHours {
                start_minute: 1320,
                end_minute: 420
            }))
        );
        assert_eq!(
            serde_json::to_value(&off).unwrap(),
            json!({ "expected_revision": 2, "quiet_hours": null })
        );
    }

    #[test]
    fn a_pause_is_set_to_a_moment_cleared_by_null_and_kept_by_leaving_it_out() {
        let resumed: MeNotificationPrefsSetBody =
            serde_json::from_value(json!({ "expected_revision": 4, "pause_until": null })).unwrap();
        let kept: MeNotificationPrefsSetBody =
            serde_json::from_value(json!({ "expected_revision": 4 })).unwrap();
        let paused: MeNotificationPrefsSetBody = serde_json::from_value(
            json!({ "expected_revision": 4, "pause_until": "2026-10-15T06:00:00.000Z" }),
        )
        .unwrap();
        assert_eq!(resumed.pause_until, Some(None));
        assert_eq!(kept.pause_until, None);
        assert!(matches!(paused.pause_until, Some(Some(_))));
        assert!(
            serde_json::from_value::<MeNotificationPrefsSetBody>(json!({ "pause_until": null }))
                .is_err(),
            "a change names the revision it was made from"
        );
    }

    #[test]
    fn a_route_whose_channel_was_removed_keeps_its_snapshot_and_says_why() {
        let kept = json!({
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6d",
            "channel_id": null,
            "channel": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b", "kind": "push", "name": "Push", "org": null },
            "enabled": false,
            "health": "channel_removed",
            "last_ok_at": "2026-10-08T17:04:05.123Z",
            "failed_at": "2026-10-09T08:00:00.000Z",
            "failure": { "code": "channel_removed", "message": "The channel was removed.", "affected_devices": [] },
            "waiting_count": 2,
            "needs": null,
            "link": null,
            "label": null,
            "kinds": [],
            "unread_delay_minutes": null,
            "min_interval_minutes": null,
            "last_sent_at": null
        });
        let route: NotificationRoute = serde_json::from_value(kept.clone()).unwrap();
        assert_eq!(route.health, NotificationRouteHealth::ChannelRemoved);
        assert_eq!(route.channel_id, None);
        assert_eq!(serde_json::to_value(&route).unwrap(), kept);
    }

    #[test]
    fn the_inbox_is_asked_for_by_place_with_its_kinds_and_since() {
        let query: NotificationListQuery = serde_json::from_value(json!({
            "kind": "mention,doc.mention", "since": "2026-10-08T00:00:00.000Z", "group_by": "place"
        }))
        .unwrap();
        assert_eq!(query.group_by, Some(NotificationGrouping::Place));
        assert_eq!(query.kind.as_deref(), Some("mention,doc.mention"));
        assert_eq!(
            serde_json::to_value(NotificationKind::NotificationRouteAttention).unwrap(),
            json!("notification.route_attention")
        );
    }

    #[test]
    fn a_route_is_cleared_of_its_delay_by_null() {
        let body: MeNotificationRouteSetBody = serde_json::from_value(
            json!({ "enabled": true, "unread_delay_minutes": null, "min_interval_minutes": 30 }),
        )
        .unwrap();
        assert_eq!(body.unread_delay_minutes, Some(None));
        assert_eq!(body.min_interval_minutes, Some(Some(30)));
        assert!(
            serde_json::from_value::<MeNotificationRouteSetBody>(json!({ "silence": 1 })).is_err()
        );
    }

    #[test]
    fn a_webhook_carries_the_newest_notification_and_all_of_a_digest() {
        let one = json!({
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b", "kind": "cap.reached",
            "at": "2026-10-08T17:04:05.123Z", "title": "Cap reached", "body": "", "url": "/usage",
            "node": null, "thread_id": null, "chat_id": null, "data": {}, "recipient": null
        });
        let mut digest = one.clone();
        digest["notifications"] = json!([one.clone(), one]);
        let delivery: WebhookDelivery = serde_json::from_value(digest.clone()).unwrap();
        assert_eq!(delivery.newest.kind, NotificationKind::CapReached);
        assert_eq!(delivery.notifications.as_ref().map(Vec::len), Some(2));
        assert_eq!(serde_json::to_value(&delivery).unwrap(), digest);
    }
}
