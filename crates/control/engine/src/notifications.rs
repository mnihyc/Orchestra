//! Recipient selection; the channels a notification may go out through and each person's routes, with what waits for an unread delay or a frequency limit; what a node's destination is sent; and transactional inbox and outbox creation.
//!
//! **Who is told.** Every kind of notification has a rule of the system for
//! its recipients. For what happens in a thread they are its *subscribers*:
//! the people whose nearest subscription on the way up from the thread has a
//! level the kind is told at, and who hold the right to view it. An approval
//! goes to the people who can decide it: those who hold `thread.manage`, and
//! who either follow the thread at `all` or `important` or asked to be told of
//! every approval they can answer. Nobody is told of what they did
//! themselves.
//!
//! **What is written.** All of it in the transaction of the event, so that a
//! notification exists exactly when the event does: a row in each recipient's
//! Inbox, written once for the event however often it is told; for each route
//! of that recipient that is on, takes the kind, covers the place and has
//! neither an unread delay nor a frequency limit, one row in the outbox; and,
//! for each destination of the node or of a node above it that takes the
//! kind, one row in the outbox, once. A route with a delay or a limit is
//! served later by the worker, from the Inbox. The Inbox row is written
//! whether or not any route carries it.
//!
//! **Following.** Owning a thread, or sending an input to it, makes a person
//! follow it at `all` by an automatic row, which a row of their own at that
//! thread always replaces.
//!
//! The events of the engine call the functions below once each; the other
//! parts that tell people (a job that ended, a cap, a provider) call
//! [`tell`] and [`tell_destinations`] with the recipients their kind has.

use crate::engine::Engine;
use crate::error::EngineError;
use crate::ids;
use crate::turns::TurnEnd;
use crate::unit::Unit;
use chrono::{DateTime, Utc};
use orch_access_contract::permissions::{NodePermission, Permission};
use orch_authz::engine::{Frame, Graph};
use orch_authz::{Caller, Policy, Principal, holders, nodes};
use orch_db::Conn;
use orch_db::queries::approvals::ApprovalRow;
use orch_db::queries::notifications::channels::DestinationRow;
use orch_db::queries::notifications::outbox::{Payload, Snapshot};
use orch_db::queries::notifications::people::{self, NewNotification};
use orch_db::queries::notifications::{events, inbox, routes, subscribers};
use orch_db::queries::threads::{self, StateChange, ThreadRow, TurnRow, thread_state, turn_state};
use orch_settings::Place;
use orch_types::{Id, Json, Seq, Time};
use serde_json::json;
use std::collections::{BTreeSet, HashMap};

mod operations;
pub use operations::{
    NotificationOperation, OperationRecipient, attach_operation, extend_operation_audience,
    notify_operation, operation_audience, operation_default, operation_thread_audience,
};

tokio::task_local! {static NOTIFICATION_OPERATION:Option<Id>;}

/// Runs an operation's effects without their ordinary per-resource notifications.
pub async fn operation_scope<T>(id: Option<Id>, work: impl std::future::Future<Output = T>) -> T {
    NOTIFICATION_OPERATION.scope(id, work).await
}

/// The kind of notification, as the contract names it.
pub use orch_api_contract::notifications::NotificationKind as Kind;

/// The setting that says whether the system's mail server may carry the mail
/// of a root org that has none of its own.
const MAIL_SHARED: &str = "mail.shared";

/// A subscription level, as the schema writes it.
mod level {
    /// Told everything.
    pub const ALL: &str = "all";
    /// Told what matters.
    pub const IMPORTANT: &str = "important";
}

/// Who a kind of notification reaches among those who follow the place it
/// happened at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// Subscribers at `all`.
    All,
    /// Subscribers at `all` or `important`.
    AllOrImportant,
}

impl Reach {
    /// Whether a subscriber at `level` is told.
    #[must_use]
    pub fn takes(self, level: &str) -> bool {
        match self {
            Self::All => level == level::ALL,
            Self::AllOrImportant => level == level::ALL || level == level::IMPORTANT,
        }
    }
}

/// How far a kind reaches among subscribers, where the kind is told to them.
/// An announcement reaches `important` subscribers only when it is important.
#[must_use]
pub fn reach_of(kind: Kind, important: bool) -> Option<Reach> {
    match kind {
        Kind::TurnCompleted | Kind::SubagentEnded | Kind::Milestone | Kind::EnvironmentIdleStop => {
            Some(Reach::All)
        }
        Kind::TurnFailed | Kind::ThreadBlocked | Kind::ProviderExhausted | Kind::CapReached => {
            Some(Reach::AllOrImportant)
        }
        Kind::Announcement => Some(if important {
            Reach::AllOrImportant
        } else {
            Reach::All
        }),
        _ => None,
    }
}

/// How far a kind reaches among destinations, which are told what a subscriber
/// at their level would be: the kinds a node's destination is ever sent, and
/// the lowest level that is sent each. A kind addressed to one person, or said
/// in a chat, is never sent to a destination.
#[must_use]
pub fn destination_reach(kind: Kind, important: bool) -> Option<Reach> {
    match kind {
        Kind::ApprovalRequested | Kind::CapabilityRequested => Some(Reach::AllOrImportant),
        Kind::TurnCompleted
        | Kind::SubagentEnded
        | Kind::TurnFailed
        | Kind::ThreadBlocked
        | Kind::Milestone
        | Kind::EnvironmentIdleStop
        | Kind::ProviderExhausted
        | Kind::CapReached
        | Kind::Announcement => reach_of(kind, important),
        _ => None,
    }
}

/// The kind as the word the schema and the wire write.
#[must_use]
pub fn word(kind: Kind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// Something worth telling that happened.
#[derive(Debug, Clone)]
pub struct Event {
    /// What it is.
    pub kind: Kind,
    /// The node it happened at.
    pub node: Option<Id>,
    /// The thread it is about.
    pub thread: Option<Id>,
    /// The approval it is about.
    pub approval: Option<Id>,
    /// A line saying what happened.
    pub title: String,
    /// A first line more.
    pub body: String,
    /// What else the kind carries. A member `actor` names who did it and a
    /// member `path` is the in-app path that opens it.
    pub data: Json,
    /// What tells this event from the others of its kind, so that a person's
    /// Inbox has one row for it however often it is told.
    pub source: String,
    /// Who did it, who is not told of it.
    pub actor: Option<Id>,
    /// When it happened.
    pub at: DateTime<Utc>,
}

impl Event {
    /// The actor already involved in this event, without discovering or enumerating other people.
    fn actor_data(&self) -> Json {
        let mut data = self.data.clone();
        if let Some(actor) = self.actor
            && let Some(object) = data.as_object_mut()
        {
            object.entry("actor").or_insert_with(|| json!(actor));
        }
        data
    }
    /// The key under which a person's Inbox holds this event once.
    #[must_use]
    pub fn dedupe_key(&self) -> String {
        format!("{}:{}", word(self.kind), self.source)
    }

    fn snapshot(&self, row: Option<Id>) -> Snapshot {
        Snapshot {
            id: row,
            kind: word(self.kind),
            at: Time::new(self.at),
            title: self.title.clone(),
            body: self.body.clone(),
            node_id: self.node,
            thread_id: self.thread,
            approval_id: self.approval,
            chat_id: None,
            data: people::notification_data(&word(self.kind), &self.actor_data()),
        }
    }
}

/// Whom an event told and where it was sent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Told {
    /// The people whose Inbox got a row for it now.
    pub people: Vec<Id>,
    /// The destinations that will be sent it.
    pub destinations: Vec<Id>,
}

/// The people who hold `permission` on a node and are not suspended there,
/// found once for the node, however many people follow it.
async fn holders_of(
    conn: &mut Conn,
    node: Id,
    permission: Option<Permission>,
    at: DateTime<Utc>,
) -> Result<Vec<Id>, EngineError> {
    let Some(owner) = subscribers::owner_of(conn, node).await? else {
        return Ok(Vec::new());
    };
    let principal = Principal::load(
        conn,
        Caller::session(owner).with_admin_mode(false),
        Time::new(at),
    )
    .await?;
    let graph = Graph::load(conn, &[node]).await?;
    let Some(site) = graph.site_of(node) else {
        return Ok(Vec::new());
    };
    let wanted = match permission {
        Some(permission) => permission,
        None => match site
            .kind
            .node()
            .and_then(|kind| NodePermission::View.on(kind))
        {
            Some(view) => view,
            None => return Ok(Vec::new()),
        },
    };
    let mut asked = vec![wanted];
    asked.extend(wanted.held_through());
    let frame = Frame::load(
        conn,
        &principal,
        Policy::default(),
        graph,
        std::slice::from_ref(&site),
        &asked,
    )
    .await?;
    Ok(holders::who_holds_for_notifications(conn, &frame, &site, wanted).await?)
}

/// The people a kind of notification reaches among those who follow a node:
/// the subscribers whose nearest subscription is at a level it takes, who
/// hold the right to view the node, without the one who did it.
pub async fn subscribers_of(
    conn: &mut Conn,
    node: Id,
    reach: Reach,
    actor: Option<Id>,
    at: DateTime<Utc>,
) -> Result<Vec<Id>, EngineError> {
    let followers: Vec<Id> = subscribers::followers_of(conn, node)
        .await?
        .into_iter()
        .filter(|follower| reach.takes(&follower.level) && Some(follower.user_id) != actor)
        .map(|follower| follower.user_id)
        .collect();
    if followers.is_empty() {
        return Ok(followers);
    }
    let viewers: BTreeSet<Id> = holders_of(conn, node, None, at)
        .await?
        .into_iter()
        .collect();
    Ok(followers
        .into_iter()
        .filter(|user| viewers.contains(user))
        .collect())
}

/// The people who can decide what the agent of a thread asks and are to be
/// told of it: those who hold `thread.manage` on it and either follow it at
/// `all` or `important` or asked to be told of every approval they can answer.
/// Nobody else, whoever they are and however they follow the thread.
pub async fn deciders_of(
    conn: &mut Conn,
    thread: Id,
    actor: Option<Id>,
    at: DateTime<Utc>,
) -> Result<Vec<Id>, EngineError> {
    Box::pin(deciders_holding(
        conn,
        thread,
        actor,
        at,
        Permission::ThreadManage,
    ))
    .await
}

/// As [`deciders_of`], for a request that those holding `permission` decide: a plan, which the place lets its
/// writers answer unless it keeps that to managers.
pub async fn deciders_holding(
    conn: &mut Conn,
    thread: Id,
    actor: Option<Id>,
    at: DateTime<Utc>,
    permission: Permission,
) -> Result<Vec<Id>, EngineError> {
    let managers: Vec<Id> = orch_authz::approvals::permission_deciders(
        conn,
        orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ObjectKind::Thread,
            id: thread,
        },
        permission,
        Time::new(at),
    )
    .await?
    .into_iter()
    .filter(|user| Some(*user) != actor)
    .collect();
    Ok(orch_authz::approvals::owners_to_tell(
        conn,
        orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ObjectKind::Thread,
            id: thread,
        },
        &managers,
        Time::new(at),
    )
    .await?)
}

fn mention_names(content: &Json) -> Vec<String> {
    let mut names = BTreeSet::new();
    for part in content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|part| part["type"] == "text")
    {
        let Some(text) = part.get("text").and_then(Json::as_str) else {
            continue;
        };
        for (offset, _) in text.char_indices().filter(|(_, ch)| *ch == '@') {
            if text[..offset].chars().next_back().is_some_and(|previous| {
                previous.is_alphanumeric() || matches!(previous, '_' | '-' | '.' | '/' | '@')
            }) {
                continue;
            }
            let name: String = text[offset + '@'.len_utf8()..]
                .chars()
                .take_while(|ch| ch.is_ascii_alphanumeric() || matches!(*ch, '_' | '-'))
                .collect();
            if !name.is_empty() {
                names.insert(name.to_ascii_lowercase());
            }
        }
    }
    names.into_iter().collect()
}

/// Newly authored input mentions notify current readable recipients even when their thread bell is off.
/// Only literal text parts participate; names inside forwarded material are not fresh mentions.
pub async fn thread_mentions(
    conn: &mut Conn,
    thread: &ThreadRow,
    input: &threads::InputRow,
    at: DateTime<Utc>,
) -> Result<(), EngineError> {
    let names = mention_names(&input.content);
    if names.is_empty() {
        return Ok(());
    }
    let named = people::mentioned_people(conn, &names).await?;
    if named.is_empty() {
        return Ok(());
    }
    let viewers: BTreeSet<Id> = holders_of(conn, thread.node_id, Some(Permission::ThreadView), at)
        .await?
        .into_iter()
        .collect();
    let recipients: Vec<Id> = named
        .into_iter()
        .filter(|user| viewers.contains(user))
        .collect();
    let event = Event {
        kind: Kind::Mention,
        node: Some(thread.node_id),
        thread: Some(thread.node_id),
        approval: None,
        title: format!("You were mentioned in {}", shown(&thread.name)),
        body: String::new(),
        data: json!({"input_id":input.id,"path":format!("/thread/{}", thread.node_id)}),
        source: input.id.to_string(),
        actor: input.author_id,
        at,
    };
    tell(conn, &event, &recipients, Some(thread.org_id)).await?;
    Ok(())
}

/// Provider managers and followers of a thread actually blocked by its exhausted state, once per episode.
pub async fn provider_exhausted(
    conn: &mut Conn,
    provider: Id,
    thread: Option<Id>,
    at: DateTime<Utc>,
) -> Result<(), EngineError> {
    let Some(row) = events::exhausted_provider(conn, provider).await? else {
        return Ok(());
    };
    let mut recipients =
        crate::provider_fact_reads::managers(conn, provider, row.owner_id, at).await?;
    if let Some(thread) = thread {
        recipients.extend(subscribers_of(conn, thread, Reach::AllOrImportant, None, at).await?);
    }
    let event = Event {
        kind: Kind::ProviderExhausted,
        node: Some(thread.unwrap_or(row.home_id)),
        thread,
        approval: None,
        title: format!("{}: its allowance is used up", row.name),
        body: String::new(),
        data: json!({"provider":provider,"provider_name":row.name,"path":format!("/providers/{provider}")}),
        source: format!("{}:{}", row.id, row.status_at.timestamp_micros()),
        actor: None,
        at,
    };
    tell(conn, &event, &recipients, Some(row.org_id)).await?;
    if thread.is_some() {
        tell_destinations(conn, &event, true).await?;
    }
    Ok(())
}

/// A renewal job failed or timed out: current managers are told atomically with its final job state.
pub async fn provider_renewal_failed(
    conn: &mut Conn,
    provider: Id,
    job: Id,
    why: &str,
    at: DateTime<Utc>,
) -> Result<(), EngineError> {
    let Some(row) = orch_db::queries::resources::provider_runtime::state(conn, provider).await?
    else {
        return Ok(());
    };
    let managers = crate::provider_fact_reads::managers(conn, provider, row.owner_id, at).await?;
    let event = Event {
        kind: Kind::ProviderAttention,
        node: Some(row.home_id),
        thread: None,
        approval: None,
        title: format!("{}: its sign-in could not be renewed", row.name),
        body: events::first_line(why),
        data: json!({"provider":provider,"provider_name":row.name,"job_id":job,"reason":"renewal_failed","path":format!("/providers/{provider}")}),
        source: format!("{provider}:renewal_job:{job}"),
        actor: None,
        at,
    };
    tell(conn, &event, &managers, Some(row.org_id)).await?;
    Ok(())
}

/// A capability request tells exactly its current approvers who follow the thread or want all approvals.
pub async fn capability_requested(
    conn: &mut Conn,
    thread: &ThreadRow,
    approval: Id,
    environment: Id,
    label: &str,
    approvers: &[Id],
    at: DateTime<Utc>,
) -> Result<(), EngineError> {
    let event = Event {
        kind: Kind::CapabilityRequested,
        node: Some(thread.node_id),
        thread: Some(thread.node_id),
        approval: Some(approval),
        title: "A capability request needs a decision".to_owned(),
        body: label.to_owned(),
        data: json!({"approval_id":approval,"environment_id":environment,"capability":label,"path":"/approvals"}),
        source: approval.to_string(),
        actor: None,
        at,
    };
    tell(conn, &event, approvers, Some(thread.org_id)).await?;
    Ok(())
}

/// Writes the event to the Inbox of each of these people, and for each row
/// that was written, to the outbox for each of their routes that tells it at
/// once. A person whose Inbox has a row for this event already is not told
/// again. `place_org` is the root org the event happened in, which decides
/// which channels carry it.
pub async fn tell(
    conn: &mut Conn,
    event: &Event,
    people: &[Id],
    place_org: Option<Id>,
) -> Result<Vec<Id>, EngineError> {
    if NOTIFICATION_OPERATION
        .try_with(|id| id.is_some())
        .unwrap_or(false)
    {
        return Ok(Vec::new());
    }
    let mail_shared = orch_settings::get::<bool>(conn, Place::System, MAIL_SHARED)
        .await?
        .unwrap_or(true);
    let key = event.dedupe_key();
    let kind = word(event.kind);
    let data = event.actor_data();
    // In one order, so that two events that tell the same people at the same
    // moment do not wait for each other's rows. A service identity is told
    // nothing.
    let candidates: Vec<Id> = people
        .iter()
        .copied()
        .filter(|user| Some(*user) != event.actor)
        .collect();
    let ordered: BTreeSet<Id> = subscribers::people_among(conn, &candidates)
        .await?
        .into_iter()
        .collect();
    let mut told = Vec::new();
    for user in ordered {
        let written = people::announce(
            conn,
            &NewNotification {
                user_id: user,
                kind: &kind,
                node_id: event.node,
                thread_id: event.thread,
                approval_id: event.approval,
                chat_id: None,
                title: &event.title,
                body: &event.body,
                data: &data,
                dedupe_key: &key,
                at: event.at,
            },
        )
        .await?;
        let Some(written) = written else {
            continue;
        };
        routes::tell_routes(
            conn,
            user,
            &event.snapshot(Some(written.id)),
            place_org,
            mail_shared,
            event.at,
        )
        .await?;
        told.push(user);
    }
    Ok(told)
}

/// Whether any node of `path` below the `from`th is restricted: what happens
/// there is not for a destination of a node above it.
async fn restricted_below(
    conn: &mut Conn,
    principal: &Principal,
    path: &[Id],
    from: usize,
    known: &mut HashMap<Id, bool>,
) -> Result<bool, EngineError> {
    for node in path.iter().skip(from + 1) {
        let restricted = match known.get(node) {
            Some(restricted) => *restricted,
            None => {
                let found = nodes::is_restricted(conn, principal, *node, Policy::default()).await?;
                known.insert(*node, found);
                found
            }
        };
        if restricted {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether a destination takes an event: its level reaches the kind, and its
/// own list of kinds, where it has one, names it.
fn destination_takes(destination: &DestinationRow, kind: Kind, important: bool) -> bool {
    let Some(needed) = destination_reach(kind, important) else {
        return false;
    };
    let level_allows = match destination.level.as_str() {
        level::ALL => true,
        other => needed.takes(other),
    };
    let wanted = word(kind);
    level_allows && (destination.kinds.is_empty() || destination.kinds.contains(&wanted))
}

/// Sends the event to every destination of its node, or of a node above it,
/// that takes it, once each: not what happened in a node below the
/// destination's own whose page is restricted, not what people said in a chat
/// and not a kind addressed to one person. `important` says an announcement
/// is. Says which destinations will be sent it.
pub async fn tell_destinations(
    conn: &mut Conn,
    event: &Event,
    important: bool,
) -> Result<Vec<Id>, EngineError> {
    let Some(node) = event.node else {
        return Ok(Vec::new());
    };
    if destination_reach(event.kind, important).is_none() {
        return Ok(Vec::new());
    }
    let path = subscribers::path_of(conn, node).await?;
    let candidates: Vec<DestinationRow> = subscribers::destinations_on(conn, &path)
        .await?
        .into_iter()
        .filter(|destination| destination_takes(destination, event.kind, important))
        .collect();
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let owner = subscribers::owner_of(conn, node).await?;
    let principal = match owner {
        Some(owner) => Some(
            Principal::load(
                conn,
                Caller::session(owner).with_admin_mode(false),
                Time::new(event.at),
            )
            .await?,
        ),
        None => None,
    };
    let key = event.dedupe_key();
    let payload = Payload::of(&[event.snapshot(None)]).to_json();
    let mut known: HashMap<Id, bool> = HashMap::new();
    let mut sent = Vec::new();
    for destination in candidates {
        let at_depth = path.iter().position(|each| *each == destination.node_id);
        if let (Some(depth), Some(principal)) = (at_depth, principal.as_ref())
            && restricted_below(conn, principal, &path, depth, &mut known).await?
        {
            continue;
        }
        let row = ids::derived(
            "destination.told",
            &[destination.id.as_bytes().as_slice(), key.as_bytes()],
        );
        if events::tell_destination_once(
            conn,
            row,
            destination.id,
            &destination.channel,
            &payload,
            event.at,
        )
        .await?
        {
            sent.push(destination.id);
        }
    }
    Ok(sent)
}

/// Tells an event to the people who follow its node at the levels its kind
/// takes, and to the destinations that take it.
async fn tell_followers(
    conn: &mut Conn,
    event: &Event,
    place_org: Id,
    important: bool,
) -> Result<Told, EngineError> {
    tell_followers_except(conn, event, place_org, important, None).await
}

/// An already-watching author needs no successful-completion notice; all other recipient rules stay.
async fn tell_followers_except(
    conn: &mut Conn,
    event: &Event,
    place_org: Id,
    important: bool,
    except: Option<Id>,
) -> Result<Told, EngineError> {
    let (Some(node), Some(reach)) = (event.node, reach_of(event.kind, important)) else {
        return Ok(Told::default());
    };
    let mut people = subscribers_of(conn, node, reach, event.actor, event.at).await?;
    people.retain(|person| Some(*person) != except);
    let told = tell(conn, event, &people, Some(place_org)).await?;
    let destinations = tell_destinations(conn, event, important).await?;
    Ok(Told {
        people: told,
        destinations,
    })
}

/// Renews one ordinary document notice and queues its currently enabled immediate routes.
pub async fn tell_document_notice(
    conn: &mut Conn,
    root: Id,
    notice: &NewNotification<'_>,
) -> Result<(), EngineError> {
    let Some(announced) = orch_db::queries::documents::tell(conn, notice).await? else {
        return Ok(());
    };
    let snapshot = Snapshot {
        id: Some(announced.id),
        kind: notice.kind.to_owned(),
        at: Time::new(notice.at),
        title: notice.title.to_owned(),
        body: notice.body.to_owned(),
        node_id: notice.node_id,
        thread_id: notice.thread_id,
        approval_id: notice.approval_id,
        chat_id: notice.chat_id,
        data: notice.data.clone(),
    };
    let mail_shared = orch_settings::get::<bool>(conn, Place::System, MAIL_SHARED)
        .await?
        .unwrap_or(true);
    routes::tell_routes(
        conn,
        notice.user_id,
        &snapshot,
        Some(root),
        mail_shared,
        notice.at,
    )
    .await?;
    Ok(())
}

/// Writes one explicit person notice and queues that person's enabled routes.
pub async fn tell_person_notice(
    conn: &mut Conn,
    root: Option<Id>,
    notice: &NewNotification<'_>,
) -> Result<(), EngineError> {
    let Some(announced) = people::announce(conn, notice).await? else {
        return Ok(());
    };
    let snapshot = Snapshot {
        id: Some(announced.id),
        kind: notice.kind.to_owned(),
        at: Time::new(notice.at),
        title: notice.title.to_owned(),
        body: notice.body.to_owned(),
        node_id: notice.node_id,
        thread_id: notice.thread_id,
        approval_id: notice.approval_id,
        chat_id: notice.chat_id,
        data: notice.data.clone(),
    };
    let mail_shared = orch_settings::get::<bool>(conn, Place::System, MAIL_SHARED)
        .await?
        .unwrap_or(true);
    routes::tell_routes(
        conn,
        notice.user_id,
        &snapshot,
        root,
        mail_shared,
        notice.at,
    )
    .await?;
    Ok(())
}

/// A thread's name, as a notification says it.
fn shown(name: &str) -> &str {
    if name.is_empty() { "A thread" } else { name }
}

/// A sub-agent ended (its card says it finished, failed or was stopped): where the thread's place asks for it
/// (`notify.subagents`, off as shipped), the people who follow the thread are told, as of a turn's end. The agent
/// that started it is told by its own program either way.
pub(crate) async fn subagent_ended(
    unit: &mut Unit<'_>,
    thread: &ThreadRow,
    item: Id,
    payload: &Json,
) -> Result<(), EngineError> {
    let wanted = Box::pin(orch_settings::get::<bool>(
        &mut *unit.conn,
        Place::Node(thread.node_id),
        orch_settings_contract::registry::keys::NOTIFY_SUBAGENTS,
    ))
    .await?
    .unwrap_or(false);
    if !wanted {
        return Ok(());
    }
    let tool = payload.get("tool").unwrap_or(&Json::Null);
    let word = |member: &str| {
        tool.get(member)
            .and_then(Json::as_str)
            .filter(|text| !text.trim().is_empty())
    };
    let name = word("name")
        .or_else(|| word("role"))
        .or_else(|| payload.get("title").and_then(Json::as_str))
        .unwrap_or("a sub-agent");
    let state = word("state").unwrap_or_default();
    let ended = match state {
        "completed" => "finished",
        "failed" => "failed",
        _ => "was stopped",
    };
    let event = Event {
        kind: Kind::SubagentEnded,
        node: Some(thread.node_id),
        thread: Some(thread.node_id),
        approval: None,
        title: format!("{}: {name} {ended}", shown(&thread.name)),
        body: word("prompt")
            .and_then(|prompt| prompt.lines().next())
            .unwrap_or_default()
            .to_owned(),
        data: json!({ "item_id": item, "state": state, "model": word("model") }),
        source: format!("{item}:{state}"),
        actor: None,
        at: unit.now(),
    };
    tell_followers_except(&mut *unit.conn, &event, thread.org_id, false, None).await?;
    Ok(())
}

/// A turn ended: the people who follow its thread are told that it completed
/// or failed. A turn the person interrupted is told to nobody, since they did
/// it, and a turn that failed before it began is not told either: its input
/// is tried again, or the thread is blocked, and that is what is told.
pub(crate) async fn turn_ended(
    unit: &mut Unit<'_>,
    thread: &ThreadRow,
    turn: &TurnRow,
    end: &TurnEnd,
    watching_author: Option<Id>,
) -> Result<(), EngineError> {
    if end
        .error
        .as_ref()
        .and_then(|error| error.pointer("/recovery/automatic"))
        .and_then(Json::as_bool)
        == Some(true)
    {
        return Ok(());
    }
    let (kind, title, body) = match end.state {
        turn_state::COMPLETED => (
            Kind::TurnCompleted,
            format!("{}: turn completed", shown(&thread.name)),
            events::last_agent_line(&mut *unit.conn, turn.id)
                .await?
                .unwrap_or_default(),
        ),
        turn_state::FAILED if turn.state != turn_state::STARTING => (
            Kind::TurnFailed,
            format!("{}: turn failed", shown(&thread.name)),
            end.error
                .as_ref()
                .and_then(|error| error.get("message"))
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_owned(),
        ),
        _ => return Ok(()),
    };
    let mut data = json!({ "turn_id": turn.id, "turn": turn.ordinal });
    if let Some(code) = end
        .error
        .as_ref()
        .and_then(|error| error.get("code"))
        .and_then(Json::as_str)
        && let Some(map) = data.as_object_mut()
    {
        map.insert("code".to_owned(), json!(code));
    }
    let event = Event {
        kind,
        node: Some(thread.node_id),
        thread: Some(thread.node_id),
        approval: None,
        title,
        body,
        data,
        source: turn.id.to_string(),
        actor: None,
        at: unit.now(),
    };
    tell_followers_except(
        &mut *unit.conn,
        &event,
        thread.org_id,
        false,
        watching_author,
    )
    .await?;
    Ok(())
}

/// Says why a thread cannot go on, in a sentence.
fn blocked_sentence(reason: &str, detail: Option<&str>) -> String {
    match detail {
        Some(detail) if !detail.is_empty() => detail.to_owned(),
        _ => reason.replace('_', " "),
    }
}

/// Changes the state of a thread as [`threads::set_state`] does, and, where
/// the change blocks a thread that was not blocked for the same reason, tells
/// the people who follow it. The answer says whether the state changed.
pub(crate) async fn set_state(
    unit: &mut Unit<'_>,
    thread: Id,
    from: &[&str],
    to: &StateChange<'_>,
    seq: Seq,
) -> Result<bool, EngineError> {
    let lost:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM threads t LEFT JOIN environments e ON e.id=t.environment_id LEFT JOIN providers p ON p.id=t.provider_id WHERE t.node_id=$1 AND (e.lost_at IS NOT NULL OR p.runtime_unavailable IS NOT NULL))").bind(thread).fetch_one(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
    if lost
        && matches!(
            to.reason,
            Some("environment_unavailable" | "provider_unavailable")
        )
    {
        return Ok(threads::set_state(&mut *unit.conn, thread, from, to, seq).await?);
    }
    let before = threads::thread(&mut *unit.conn, thread).await?;
    let changed = threads::set_state(&mut *unit.conn, thread, from, to, seq).await?;
    if !changed || to.state != thread_state::BLOCKED {
        return Ok(changed);
    }
    let Some(before) = before else {
        return Ok(changed);
    };
    let again = before.state == thread_state::BLOCKED
        && before.blocked_reason.as_deref() == to.reason
        && before.blocked_detail.as_deref() == to.detail;
    if !again {
        let reason = to.reason.unwrap_or_default();
        if reason == "provider_exhausted"
            && let Some(provider) = before.provider_id
        {
            let at = unit.now();
            provider_exhausted(&mut *unit.conn, provider, Some(thread), at).await?;
        }
        let event = Event {
            kind: Kind::ThreadBlocked,
            node: Some(thread),
            thread: Some(thread),
            approval: None,
            title: format!("{} is blocked", shown(&before.name)),
            body: blocked_sentence(reason, to.detail),
            data: json!({ "reason": reason, "detail": to.detail }),
            source: format!("{thread}:{seq}"),
            actor: None,
            at: unit.now(),
        };
        tell_followers(&mut *unit.conn, &event, before.org_id, false).await?;
    }
    Ok(changed)
}

/// A harness asked for an approval: the people who can decide it are told, as
/// they are told of nothing else, whether or not they follow the thread, when
/// they asked for that.
pub(crate) async fn approval_asked(
    unit: &mut Unit<'_>,
    thread: Id,
    approval: &ApprovalRow,
) -> Result<(), EngineError> {
    let Some(row) = threads::thread(&mut *unit.conn, thread).await? else {
        return Ok(());
    };
    let event = Event {
        kind: Kind::ApprovalRequested,
        node: Some(thread),
        thread: Some(thread),
        approval: Some(approval.id),
        title: format!("{} asks for approval", shown(&row.name)),
        body: approval.title.clone(),
        data: json!({ "approval_id": approval.id }),
        source: approval.id.to_string(),
        actor: None,
        at: unit.now(),
    };
    let permission = Box::pin(crate::approvals::deciding_permission(
        &mut *unit.conn,
        thread,
        &approval.request,
    ))
    .await?;
    let object = orch_access_contract::decisions::ObjectRef {
        kind: orch_access_contract::permissions::ObjectKind::Thread,
        id: thread,
    };
    let eligible = orch_authz::approvals::permission_deciders(
        &mut *unit.conn,
        object,
        permission,
        Time::new(event.at),
    )
    .await?;
    let people = orch_authz::approvals::request_recipients(
        &mut *unit.conn,
        object,
        &eligible,
        approval.turn_id,
        Time::new(event.at),
    )
    .await?;
    tell(&mut *unit.conn, &event, &people, Some(row.org_id)).await?;
    Ok(())
}

/// An approval is not waiting any more, decided or withdrawn: what asked
/// people to decide it is marked read, since nobody has anything left to do
/// about it and a message that went out later would ask for what is done.
pub(crate) async fn approval_settled(unit: &mut Unit<'_>, approval: Id) -> Result<(), EngineError> {
    let now = unit.now();
    inbox::settle_approval(&mut *unit.conn, approval, now).await?;
    Ok(())
}

/// A person owns a thread, or sent an input to it: from now on they follow it
/// at `all`, unless they already chose a level at that thread.
pub(crate) async fn follow_thread(
    conn: &mut Conn,
    person: Id,
    thread: Id,
    at: DateTime<Utc>,
) -> Result<(), EngineError> {
    subscribers::follow(conn, person, thread, at).await?;
    Ok(())
}

impl Engine {
    /// A machine lost its session while turns ran on it: the person it belongs
    /// to is told, once for the turns that were under way, with the threads
    /// they were. A machine the system runs belongs to nobody, and a machine
    /// that goes while nothing runs on it tells nobody.
    pub(crate) async fn tell_machine_offline(
        &self,
        machine: Id,
        affected: &[Id],
    ) -> Result<(), EngineError> {
        if affected.is_empty() {
            return Ok(());
        }
        let mut tx = self.inner.db.begin().await?;
        let Some(row) = events::machine(&mut tx, machine).await? else {
            return Ok(());
        };
        let Some(owner) = row.owner_id else {
            return Ok(());
        };
        let mut names: Vec<String> = Vec::new();
        let mut turns: Vec<Id> = Vec::new();
        for thread in affected {
            if let Some(thread) = threads::thread(&mut tx, *thread).await? {
                names.push(thread.name);
                turns.extend(thread.current_turn_id);
            }
        }
        // The turns that were under way are what the event is: the same turns
        // cut off again are not told again.
        turns.sort_unstable();
        let under_way = turns.first().map_or_else(String::new, ToString::to_string);
        let Some(first) = names.first().cloned() else {
            return Ok(());
        };
        let others = names.len() - 1;
        let body = if others == 0 {
            format!("{} was running on it", shown(&first))
        } else {
            format!("{} and {others} more were running on it", shown(&first))
        };
        let event = Event {
            kind: Kind::MachineOffline,
            node: None,
            thread: None,
            approval: None,
            title: format!("{} went offline", row.name),
            body,
            data: json!({ "machine_id": machine, "path": format!("/resources/machines/{machine}") }),
            source: format!("{machine}:{under_way}"),
            actor: None,
            at: self.inner.clock.now(),
        };
        tell(&mut tx, &event, &[owner], None).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(())
    }
}

/// The words a kind of job is told as.
fn job_phrase(kind: &str) -> &'static str {
    match kind {
        "snapshot" => "snapshot",
        "thread_fork" => "fork",
        "environment_move" => "move",
        "environment_recreate" => "update",
        "environment_provision" => "setup",
        "environment_remove" => "removal",
        "provider_provision" => "provider setup",
        "provider_recreate" => "provider update",
        "capability_change" => "capability change",
        "delivery" => "delivery",
        "export" => "export",
        "import" => "import",
        _ => "job",
    }
}

/// An announcement or milestone reaches the sender's eligible followers and the node's destinations.
pub async fn announcement(
    conn: &mut Conn,
    row: &threads::tools::AnnouncementRow,
) -> Result<Told, EngineError> {
    let milestone = row.category == "milestone";
    let kind = if milestone {
        Kind::Milestone
    } else {
        Kind::Announcement
    };
    let important = row.severity == "important";
    let Some(reach) = reach_of(kind, important) else {
        return Ok(Told::default());
    };
    let node = row.from_thread_id.unwrap_or(row.topic_id);
    let org = subscribers::path_of(conn, node).await?.first().copied();
    let recipients = subscribers_of(conn, node, reach, row.from_user_id, row.created_at).await?;
    let path = row.from_thread_id.map_or_else(
        || format!("/topic/{}", row.topic_id),
        |thread| format!("/thread/{thread}"),
    );
    let event = Event {
        kind,
        node: Some(node),
        thread: row.from_thread_id,
        approval: None,
        title: row.title.clone(),
        body: events::first_line(&row.body),
        data: json!({"announcement_id":row.id,"category":row.category,"severity":row.severity,"path":path}),
        source: row.group_id.unwrap_or(row.id).to_string(),
        actor: row.from_user_id,
        at: row.created_at,
    };
    Ok(Told {
        people: tell(conn, &event, &recipients, org).await?,
        destinations: tell_destinations(conn, &event, important).await?,
    })
}

/// An environment is to be stopped for idleness at `stops_at`: its owner, and the owners and the followers at
/// `all` of the threads bound to it, are told once for each stretch without activity (activity starts a new
/// stretch, and the next warning is a new one).
pub async fn environment_idle_warning(
    conn: &mut Conn,
    environment: &orch_db::queries::environments::hosting::EnvironmentState,
    stops_at: DateTime<Utc>,
    at: DateTime<Utc>,
) -> Result<Vec<Id>, EngineError> {
    let mut recipients = vec![environment.owner_id];
    let reach = reach_of(Kind::EnvironmentIdleStop, false).unwrap_or(Reach::All);
    for (thread, owner) in
        orch_db::queries::environments::routes::bound_threads(conn, environment.id).await?
    {
        recipients.push(owner);
        recipients.extend(subscribers_of(conn, thread, reach, None, at).await?);
    }
    recipients.sort_unstable();
    recipients.dedup();
    let since = environment
        .last_activity_at
        .unwrap_or(environment.created_at);
    let event = Event {
        kind: Kind::EnvironmentIdleStop,
        node: Some(environment.home_node_id),
        thread: None,
        approval: None,
        title: format!(
            "{} stops soon: nothing has happened in it for a while",
            environment.name
        ),
        body: String::new(),
        data: json!({
            "environment_id": environment.id, "environment_name": environment.name,
            "stops_at": stops_at, "warning": true,
            "path": format!("/environments/{}", environment.id),
        }),
        source: format!("{}:{}:warning", environment.id, since.timestamp_micros()),
        actor: None,
        at,
    };
    tell(conn, &event, &recipients, Some(environment.org_id)).await
}

/// A temporary environment's current owner has a full hour to keep or export it.
pub async fn temporary_environment_warning(
    conn: &mut Conn,
    environment: &orch_db::queries::environments::hosting::EnvironmentState,
    expires_at: DateTime<Utc>,
    at: DateTime<Utc>,
) -> Result<Vec<Id>, EngineError> {
    let event = Event {
        kind: Kind::EnvironmentAttention,
        node: Some(environment.home_node_id),
        thread: None,
        approval: None,
        title: format!(
            "{}: temporary environment will be removed",
            environment.name
        ),
        body: "Export the workspace files or choose Keep before removal.".to_owned(),
        data: json!({"environment_id":environment.id,"subject":environment.name,"reason":"temporary",
            "expires_at":expires_at,"path":format!("/resources/environments/{}",environment.id)}),
        source: format!(
            "temporary:{}:{}:{}",
            environment.id,
            environment.owner_id,
            expires_at.timestamp_micros()
        ),
        actor: None,
        at,
    };
    tell(
        conn,
        &event,
        &[environment.owner_id],
        Some(environment.org_id),
    )
    .await
}

/// A job ended, succeeded or failed: the person who asked for it is told. A
/// job that was cancelled, or is not over, tells nobody.
pub async fn job_finished(
    conn: &mut Conn,
    job: Id,
    at: DateTime<Utc>,
) -> Result<Told, EngineError> {
    crate::recovery::release_history_pins(conn, job, at).await?;
    let full = orch_db::queries::jobs::job(conn, job).await?;
    if let Some(row) = &full {
        crate::jobs::capability_change::finished(conn, row, at).await?;
    }
    if let Some(row) = &full
        && row.plan.get("provider_recovery").is_some()
        && matches!(row.state.as_str(), "failed" | "cancelled" | "lost")
        && let Some(provider) = row.provider_id
    {
        sqlx::query("UPDATE providers SET runtime_unavailable='Its re-creation did not finish',status='offline',status_detail='Its re-creation did not finish',revision=revision+1,updated_at=$2 WHERE id=$1 AND NOT EXISTS(SELECT 1 FROM runtimes WHERE provider_id=$1 AND successor_of IS NULL AND state='running')").bind(provider).bind(at).execute(&mut *conn).await.map_err(orch_db::DbError::from)?;
    }
    if let Some(full) = &full
        && operations::operation_job_finished(conn, full, at).await?
    {
        return Ok(Told::default());
    }
    let Some(row) = events::job(conn, job).await? else {
        return Ok(Told::default());
    };
    // Preview counts are followed in the plan; they are not a completed change to someone's work.
    if row.kind == crate::jobs::measurement::KIND {
        return Ok(Told::default());
    }
    // Keep the archive link in the completion transaction: a generic notice would win its dedupe key,
    // and leaving the notice to a later hook could lose it after a crash.
    if row.kind == orch_db::queries::jobs::kind::EXPORT
        && let Some(creator) = row.created_by
        && let Some(export) = orch_db::queries::jobs::job(conn, job)
            .await?
            .and_then(|job| {
                job.plan
                    .get("export")
                    .and_then(Json::as_str)
                    .and_then(|id| id.parse::<Id>().ok())
            })
    {
        return export_finished(conn, job, export, creator, at).await;
    }
    let outcome = match row.state.as_str() {
        "succeeded" => "finished",
        "failed" => "failed",
        _ => return Ok(Told::default()),
    };
    let Some(creator) = row.created_by else {
        return Ok(Told::default());
    };
    let subject = row.subject.clone().unwrap_or_default();
    let phrase = job_phrase(&row.kind);
    let title = if subject.is_empty() {
        format!("The {phrase} {outcome}")
    } else {
        format!("{subject}: {phrase} {outcome}")
    };
    let body = row
        .error
        .as_ref()
        .map(|error| {
            if row.kind == crate::jobs::capability_change::KIND {
                return error
                    .get("message")
                    .and_then(Json::as_str)
                    .unwrap_or("The capability change did not finish")
                    .to_owned();
            }
            orch_api_contract::messages::for_failure(
                error
                    .get("code")
                    .and_then(Json::as_str)
                    .unwrap_or("internal"),
            )
        })
        .unwrap_or_default();
    let place_org = match row.node_id {
        Some(node) => subscribers::path_of(conn, node).await?.first().copied(),
        None => None,
    };
    let thread = match row.node_id {
        Some(node) if threads::thread(conn, node).await?.is_some() => Some(node),
        _ => None,
    };
    let exported_snapshot = if row.kind == orch_db::queries::jobs::kind::SNAPSHOT {
        orch_db::queries::jobs::job(conn, job)
            .await?
            .filter(|job| job.plan.get("workspace_export").and_then(Json::as_bool) == Some(true))
            .and_then(|job| job.result)
            .and_then(|result| {
                result
                    .get("snapshot_id")
                    .and_then(Json::as_str)
                    .and_then(|id| id.parse::<Id>().ok())
            })
    } else {
        None
    };
    let mut data = json!({"job_id":job,"job":row.kind,"state":row.state,"subject":row.subject,
        "error_code":row.error.as_ref().and_then(|error|error.get("code")).and_then(Json::as_str)});
    let approval = full
        .as_ref()
        .and_then(|job| job.plan.get("approval"))
        .and_then(Json::as_str)
        .and_then(|id| id.parse::<Id>().ok());
    if approval.is_some() {
        data["path"] = json!("/approvals");
    }
    if let Some(snapshot) = exported_snapshot {
        data["snapshot_id"] = json!(snapshot);
        data["workspace_export"] = json!(true);
    }
    let event = Event {
        kind: Kind::JobFinished,
        node: row.node_id,
        thread,
        approval,
        title,
        body,
        data,
        source: job.to_string(),
        actor: None,
        at,
    };
    let people = tell(conn, &event, &[creator], place_org).await?;
    Ok(Told {
        people,
        destinations: Vec::new(),
    })
}

/// An archive of what a person may read in a place was written, or was not: the person who asked is told, with the
/// link to the archive when it is ready. One that was cancelled tells nobody.
pub async fn export_finished(
    conn: &mut Conn,
    job: Id,
    export: Id,
    creator: Id,
    at: DateTime<Utc>,
) -> Result<Told, EngineError> {
    let Some(row) = events::job(conn, job).await? else {
        return Ok(Told::default());
    };
    let ready = match row.state.as_str() {
        "succeeded" => true,
        "failed" => false,
        _ => return Ok(Told::default()),
    };
    let named = orch_db::queries::exports::get(conn, export).await?;
    let place = match named.as_ref().and_then(|each| each.node_id) {
        Some(node) => orch_db::queries::threads::node_name(conn, node)
            .await?
            .unwrap_or_default(),
        None => String::new(),
    };
    let title = match (ready, place.is_empty()) {
        (true, true) => "The export finished".to_owned(),
        (true, false) => format!("{place}: export finished"),
        (false, true) => "The export failed".to_owned(),
        (false, false) => format!("{place}: export failed"),
    };
    let body = if ready {
        "The archive is ready to download.".to_owned()
    } else {
        row.error
            .as_ref()
            .and_then(|error| error.get("message"))
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let event = Event {
        kind: Kind::JobFinished,
        node: None,
        thread: None,
        approval: None,
        title,
        body,
        data: json!({
            "job_id": job, "job": "export", "state": row.state, "export_id": export,
            "path": "/settings/exports",
        }),
        source: job.to_string(),
        actor: None,
        at,
    };
    let people = tell(conn, &event, &[creator], None).await?;
    Ok(Told {
        people,
        destinations: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn destination(level: &str, kinds: &[&str]) -> DestinationRow {
        DestinationRow {
            id: orch_types::new_id(),
            node_id: orch_types::new_id(),
            channel: "webhook".to_owned(),
            name: "Ops".to_owned(),
            config: json!({}),
            templates: json!({}),
            kinds: kinds.iter().map(|kind| (*kind).to_owned()).collect(),
            level: level.to_owned(),
            enabled: true,
            last_ok_at: None,
            last_error: None,
            created_by: orch_types::new_id(),
            created_at: chrono::DateTime::UNIX_EPOCH,
            updated_at: chrono::DateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn a_turn_that_completed_is_told_at_all_only_and_a_failure_at_important_too() {
        assert!(reach_of(Kind::TurnCompleted, false).is_some_and(|reach| reach.takes("all")));
        assert!(
            reach_of(Kind::TurnCompleted, false).is_some_and(|reach| !reach.takes("important"))
        );
        assert!(reach_of(Kind::TurnFailed, false).is_some_and(|reach| reach.takes("important")));
        assert!(reach_of(Kind::ThreadBlocked, false).is_some_and(|reach| !reach.takes("off")));
        assert!(
            reach_of(Kind::Mention, false).is_none(),
            "said to one person"
        );
    }

    #[test]
    fn an_announcement_reaches_important_subscribers_only_when_it_is_important() {
        assert!(reach_of(Kind::Announcement, false).is_some_and(|reach| !reach.takes("important")));
        assert!(reach_of(Kind::Announcement, true).is_some_and(|reach| reach.takes("important")));
    }

    #[test]
    fn a_destination_is_sent_what_a_subscriber_at_its_level_would_be_and_nothing_said_to_one_person()
     {
        let at_important = destination("important", &[]);
        let at_all = destination("all", &[]);
        assert!(!destination_takes(
            &at_important,
            Kind::TurnCompleted,
            false
        ));
        assert!(destination_takes(&at_all, Kind::TurnCompleted, false));
        assert!(destination_takes(&at_important, Kind::TurnFailed, false));
        assert!(destination_takes(
            &at_important,
            Kind::ApprovalRequested,
            false
        ));
        assert!(!destination_takes(&at_important, Kind::Announcement, false));
        assert!(destination_takes(&at_important, Kind::Announcement, true));
        for kind in [
            Kind::Mention,
            Kind::ChatMessage,
            Kind::AccessRequested,
            Kind::AccessDecided,
            Kind::Invitation,
            Kind::JobFinished,
            Kind::SignInAttention,
        ] {
            assert!(!destination_takes(&at_all, kind, true), "{kind:?}");
        }
    }

    #[test]
    fn a_destination_with_a_list_of_kinds_is_sent_those_and_no_others() {
        let narrowed = destination("all", &["turn.failed"]);
        assert!(destination_takes(&narrowed, Kind::TurnFailed, false));
        assert!(!destination_takes(&narrowed, Kind::TurnCompleted, false));
    }

    #[test]
    fn a_kind_is_written_with_its_dots_and_a_block_with_its_detail_or_its_reason() {
        assert_eq!(word(Kind::TurnCompleted), "turn.completed");
        assert_eq!(word(Kind::EnvironmentIdleStop), "environment.idle_stop");
        assert_eq!(
            blocked_sentence("agent_offline", Some("the machine is offline")),
            "the machine is offline"
        );
        assert_eq!(blocked_sentence("quota_exceeded", None), "quota exceeded");
    }
}
