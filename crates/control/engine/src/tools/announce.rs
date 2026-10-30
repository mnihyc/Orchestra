//! What an agent tells the others: a notice to the threads of its topic or, where allowed, of its project, by the delivery, fallback and wake settings; or a milestone, which goes to the people who follow its thread unless it says the topic.
//!
//! An announcement is what somebody on the work tells the others. It has a **category** (a notice, which the
//! other agents need in order to act, or a milestone, a point of progress for the people who follow the thread)
//! and an **audience** (the followers of the sending thread, its topic, or its project). To the followers it is a
//! row and a notification, and enters no thread. To a topic it is delivered into the other threads, each by what
//! it is doing, and by the topic's settings (`broadcast.*`): a thread at work is interrupted, steered or left
//! until its next turn; an idle one is started or told at its next turn; one that is blocked is told then. What
//! each thread got is written on the line the announcement leaves in it (`delivered_as`), so that what was done is
//! counted from what was left and no count is kept anywhere else.
//!
//! **An announcement is made once.** Its identifier is derived from the call (or the request) that made it, and
//! so is everything it puts into a thread, so that the same call asked again finds what it made and makes
//! nothing more. Everything it does to the threads of a topic is done in one transaction, with their rows taken in
//! the order of their identifiers, so that two announcements at once never wait for each other.
//!
//! **A person's announcement** (`POST /nodes/{id}/announcements`) is the same steps with a
//! person's name in place of a thread's, a choice of `all` or `active` threads, and any value of `delivery`,
//! `fallback` and `wake_idle`. It needs `topic.interact` (or `project.manage`), and reaches into a running turn,
//! by steering or by interrupting, only on the threads where the sender holds `thread.interact`: the others get
//! it at their next turn, and the result counts them.
//!
//! **An agent's announcement** reaches no further than its topic unless `broadcast.agent_scope` is `project`,
//! chooses its own `delivery` and `wake_idle` only where `broadcast.agent_options` lists them, and may send at most
//! `broadcast.rate_per_hour` of them, with what it hands to single threads, in a rolling hour. What it received as
//! an announcement it is told not to announce again.
//!
//! **Not here.** The people who follow a thread are told by the notifications; the row is made and the thread's
//! attention moved, and `tell_followers` is the one place that calls them.

use crate::engine::Engine;
use crate::error::EngineError;
use crate::ids;
use crate::inputs::Arrival;
use crate::tools::definitions::{BODY_MAX_CHARS, Code, Refusal};
use crate::tools::{Answer, CallContext, ToolSettings, definition_of, setting};
use crate::unit::Unit;
use chrono::{DateTime, Duration, Utc};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::permissions::Permission;
use orch_api_contract::threads::{
    AnnouncementCategory, AnnouncementFrom, AnnouncementSeverity, ThreadMention,
};
use orch_api_contract::topics::{
    Announcement, AnnouncementAudience, AnnouncementDelivered, AnnouncementDelivery,
    AnnouncementFallback, AnnouncementFile, AnnouncementTargets, AnnouncementThreadTarget,
};
use orch_api_contract::users::UserRef;
use orch_authz::{Act, Caller, Principal, nodes};
use orch_db::audit::{Actor, Entry, Origin, append};
use orch_db::queries::hierarchy;
use orch_db::queries::threads::{self, ThreadRow, thread_state, tools as rows, turn_state};
use orch_settings::{Place as SettingsPlace, Snapshot, Wanted};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json, Time};
use serde::Deserialize;
use serde_json::json;
use std::collections::{BTreeMap, HashSet};

/// How long an hour is, for the rate of what an agent sends into other threads.
const RATE_PERIOD: Duration = Duration::hours(1);

/// The words of `delivered_as`, which a line of an announcement carries in the thread it reached.
const INTERRUPTED: &str = "interrupted";
const STEERED: &str = "steered";
const QUEUED: &str = "queued";
const DEFERRED: &str = "deferred";

/// The key of the text that replaces the words of an announcement in `agent.messages`.
pub const MESSAGE_KEY: &str = "announcement";

// ---------------------------------------------------------------- how a topic delivers

/// The settings of a topic that say how an announcement reaches its threads.
#[derive(Debug, Clone, PartialEq)]
pub struct Broadcast {
    /// What happens to a thread that has an active turn.
    pub delivery: AnnouncementDelivery,
    /// Used instead of `steer` when the receiving agent cannot be steered.
    pub fallback: AnnouncementFallback,
    /// Whether an idle thread is started.
    pub wake_idle: bool,
    /// Which of `interrupt`, `steer`, `next_turn`, `wake_idle` an agent may choose itself.
    pub agent_options: Vec<String>,
    /// How many an agent may send in a rolling hour.
    pub rate_per_hour: i64,
    /// `topic`, `project` or `org`: how far an agent's announcement may reach.
    pub agent_scope: String,
    /// The texts Orch says to agents here.
    pub messages: BTreeMap<String, String>,
}

impl Broadcast {
    /// Reads the settings as they hold at a topic.
    pub async fn load(conn: &mut orch_db::Conn, topic: Id) -> Result<Self, EngineError> {
        let wanted = [
            definition_of(keys::BROADCAST_DELIVERY)?,
            definition_of(keys::BROADCAST_FALLBACK)?,
            definition_of(keys::BROADCAST_WAKE_IDLE)?,
            definition_of(keys::BROADCAST_AGENT_OPTIONS)?,
            definition_of(keys::BROADCAST_RATE_PER_HOUR)?,
            definition_of(keys::BROADCAST_AGENT_SCOPE)?,
            definition_of(keys::AGENT_MESSAGES)?,
        ];
        let snapshot =
            Snapshot::load(conn, SettingsPlace::Node(topic), Wanted::Keys(&wanted)).await?;
        let word = |key: &str| -> Result<Option<String>, EngineError> { setting(&snapshot, key) };
        let delivery = match word(keys::BROADCAST_DELIVERY)?.as_deref() {
            Some("interrupt") => AnnouncementDelivery::Interrupt,
            Some("next_turn") => AnnouncementDelivery::NextTurn,
            _ => AnnouncementDelivery::Steer,
        };
        let fallback = match word(keys::BROADCAST_FALLBACK)?.as_deref() {
            Some("interrupt") => AnnouncementFallback::Interrupt,
            _ => AnnouncementFallback::NextTurn,
        };
        Ok(Self {
            delivery,
            fallback,
            wake_idle: setting(&snapshot, keys::BROADCAST_WAKE_IDLE)?.unwrap_or(false),
            agent_options: setting(&snapshot, keys::BROADCAST_AGENT_OPTIONS)?.unwrap_or_default(),
            rate_per_hour: setting(&snapshot, keys::BROADCAST_RATE_PER_HOUR)?.unwrap_or_default(),
            agent_scope: word(keys::BROADCAST_AGENT_SCOPE)?.unwrap_or_else(|| "topic".to_owned()),
            messages: setting(&snapshot, keys::AGENT_MESSAGES)?.unwrap_or_default(),
        })
    }
}

fn delivery_word(delivery: AnnouncementDelivery) -> &'static str {
    match delivery {
        AnnouncementDelivery::Interrupt => "interrupt",
        AnnouncementDelivery::Steer => "steer",
        AnnouncementDelivery::NextTurn => "next_turn",
    }
}

fn fallback_word(fallback: AnnouncementFallback) -> &'static str {
    match fallback {
        AnnouncementFallback::NextTurn => "next_turn",
        AnnouncementFallback::Interrupt => "interrupt",
    }
}

fn category_word(category: AnnouncementCategory) -> &'static str {
    match category {
        AnnouncementCategory::Notice => "notice",
        AnnouncementCategory::Milestone => "milestone",
    }
}

fn severity_word(severity: AnnouncementSeverity) -> &'static str {
    match severity {
        AnnouncementSeverity::Info => "info",
        AnnouncementSeverity::Important => "important",
    }
}

// ---------------------------------------------------------------- what is sent, and by whom

/// Who sends.
#[derive(Debug, Clone)]
pub enum Sender {
    /// The agent of a thread, who may always reach the threads of its own topic.
    Agent(Box<ThreadRow>),
    /// A person, with the name that heads the text.
    Person {
        /// Who.
        user: Id,
        /// The name shown.
        name: String,
    },
}

impl Sender {
    fn name(&self) -> &str {
        match self {
            Self::Agent(thread) => &thread.name,
            Self::Person { name, .. } => name,
        }
    }

    fn thread(&self) -> Option<Id> {
        match self {
            Self::Agent(thread) => Some(thread.node_id),
            Self::Person { .. } => None,
        }
    }

    /// The person who answers for an interrupt: the person, or the first owner of the agent's thread.
    fn responsible(&self) -> Id {
        match self {
            Self::Agent(thread) => thread.owner_id,
            Self::Person { user, .. } => *user,
        }
    }
}

/// Which threads of a topic an announcement is delivered to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// Every thread that is not archived.
    All,
    /// The threads with an active turn; the others get nothing, not even a notice.
    Active,
    /// None: only the followers of the sending thread hear of it.
    Nobody,
}

/// An announcement as it is delivered into one topic.
#[derive(Debug, Clone)]
pub struct Plan {
    /// A notice or a milestone.
    pub category: AnnouncementCategory,
    /// What happened.
    pub title: String,
    /// The rest.
    pub body: String,
    /// How much it matters.
    pub severity: AnnouncementSeverity,
    /// How it reaches a thread at work.
    pub delivery: AnnouncementDelivery,
    /// What it does where it may not steer.
    pub fallback: AnnouncementFallback,
    /// Whether it starts an idle thread.
    pub wake_idle: bool,
    /// Which threads.
    pub reach: Reach,
}

/// What an announcement did, thread by thread, counted.
pub type Delivered = AnnouncementDelivered;

fn no_delivery() -> Delivered {
    AnnouncementDelivered {
        interrupted: 0,
        steered: 0,
        queued: 0,
        deferred: 0,
    }
}

/// An announcement that was made, and what it did.
#[derive(Debug, Clone)]
pub struct Sent {
    /// The row.
    pub row: rows::AnnouncementRow,
    /// What it did to the threads.
    pub delivered: Delivered,
    /// Whether it was made by an earlier asking of the same call.
    pub already: bool,
}

/// The words of an announcement as the agent that receives it reads them. A place's own text for the key
/// `announcement` takes the place of the words, with the name of the sender, the title and the body put in.
#[must_use]
pub fn announcement_text(
    messages: &BTreeMap<String, String>,
    sender: &Sender,
    category: AnnouncementCategory,
    title: &str,
    body: &str,
    at: &str,
) -> String {
    let words = messages.get(MESSAGE_KEY).map_or_else(
        || format!("{title}\n{body}"),
        |own| {
            crate::instructions::substitute(
                own,
                &[
                    ("<source name>", sender.name()),
                    ("<title>", title),
                    ("<body>", body),
                    ("<time>", at),
                ],
            )
        },
    );
    crate::instructions::relayed(
        sender.thread().is_some(),
        sender.name(),
        match category {
            AnnouncementCategory::Notice => "announcement",
            AnnouncementCategory::Milestone => "milestone",
        },
        at,
        &words,
    )
}

/// What a thread does with an announcement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Treatment {
    Interrupt,
    Steer,
    Queue,
    Defer,
}

/// What the thread whose state and agent these are does with an announcement: the table of the delivery. A
/// thread at work is interrupted, steered or left until its next turn; an idle one is started or told at its
/// next turn; a blocked one is told at its next turn. `can_steer` is whether its agent takes a message into a
/// turn that runs and one does; `may_reach` is whether the sender may reach into that turn at all.
fn treatment(state: &str, plan: &Plan, can_steer: bool, may_reach: bool) -> Treatment {
    let active = matches!(
        state,
        thread_state::STARTING | thread_state::RUNNING | thread_state::WAITING_APPROVAL
    );
    if !active {
        return if state == thread_state::IDLE && plan.wake_idle {
            Treatment::Queue
        } else {
            Treatment::Defer
        };
    }
    // `steer` falls back where the agent cannot be steered now.
    let by = match plan.delivery {
        AnnouncementDelivery::Steer if !can_steer => match plan.fallback {
            AnnouncementFallback::Interrupt => AnnouncementDelivery::Interrupt,
            AnnouncementFallback::NextTurn => AnnouncementDelivery::NextTurn,
        },
        other => other,
    };
    match by {
        AnnouncementDelivery::Interrupt if may_reach => Treatment::Interrupt,
        AnnouncementDelivery::Steer if may_reach => Treatment::Steer,
        _ => Treatment::Queue,
    }
}

// ---------------------------------------------------------------- delivering into a topic

impl Engine {
    /// Makes an announcement and delivers it into the threads of one topic, in the transaction of `unit`.
    ///
    /// `id` makes it once: where it exists already nothing is made, and what it did is counted from what it
    /// left. `may_reach` is the threads whose running turn the sender may steer or interrupt; none for a sender
    /// who may not, `None` for one who may reach all of them.
    #[allow(
        clippy::too_many_arguments,
        reason = "an announcement is made of who sends it, what, to where and by what right; none of it can be left out"
    )]
    pub(crate) async fn deliver_announcement(
        &self,
        unit: &mut Unit<'_>,
        topic: Id,
        id: Id,
        group: Option<Id>,
        sender: &Sender,
        plan: &Plan,
        may_reach: Option<&HashSet<Id>>,
    ) -> Result<Sent, EngineError> {
        self.deliver_announcement_with_references(
            unit, topic, id, group, sender, plan, may_reach, &plan.body,
        )
        .await
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the stored source and resolved text share the same delivery checks"
    )]
    async fn deliver_announcement_with_references(
        &self,
        unit: &mut Unit<'_>,
        topic: Id,
        id: Id,
        group: Option<Id>,
        sender: &Sender,
        plan: &Plan,
        may_reach: Option<&HashSet<Id>>,
        authored: &str,
    ) -> Result<Sent, EngineError> {
        let messages = Broadcast::load(&mut *unit.conn, topic).await?.messages;
        let found = rows::topic_threads(&mut *unit.conn, topic).await?;
        let mut locked: Vec<Id> = found.iter().map(|thread| thread.node_id).collect();
        locked.extend(sender.thread());
        locked.sort_unstable();
        locked.dedup();
        rows::lock_threads(&mut *unit.conn, &locked).await?;
        unit.check_tool_source().await?;
        let targets = match plan.reach {
            Reach::All => "all",
            Reach::Active => "active",
            Reach::Nobody => "none",
        };
        let files = json!([]);
        let now = unit.now();
        let made = rows::insert_referenced_announcement(
            &mut *unit.conn,
            &rows::NewAnnouncement {
                id,
                topic_id: topic,
                from_thread_id: sender.thread(),
                from_user_id: match sender {
                    Sender::Person { user, .. } => Some(*user),
                    Sender::Agent(_) => None,
                },
                category: category_word(plan.category),
                title: &plan.title,
                body: &plan.body,
                files: &files,
                severity: severity_word(plan.severity),
                delivery: delivery_word(plan.delivery),
                fallback: fallback_word(plan.fallback),
                wake_idle: plan.wake_idle,
                targets,
                to_thread_id: None,
                group_id: group,
                at: now,
            },
            authored,
        )
        .await?;
        let Some(row) = made else {
            let row = rows::announcement(&mut *unit.conn, id)
                .await?
                .ok_or_else(|| EngineError::unreadable("an announcement is there and is not"))?;
            let delivered = delivered_counts(&mut *unit.conn, &[id])
                .await?
                .remove(&id)
                .unwrap_or_else(no_delivery);
            return Ok(Sent {
                row,
                delivered,
                already: true,
            });
        };
        // A milestone schedules summaries only after the announcement's transaction commits.
        if plan.category == AnnouncementCategory::Milestone
            && let Some(thread) = sender.thread()
        {
            crate::summaries::due(&mut *unit.conn, thread, "milestone", now).await?;
        }
        let from = from_json(&mut *unit.conn, sender).await?;
        let mut delivered = no_delivery();
        if plan.reach == Reach::Nobody {
            // The followers of the sending thread are told; no thread is touched but that one's attention.
            if let Some(source) = sender.thread() {
                let seq = unit.stamp(source).await?;
                threads::mark_activity(&mut *unit.conn, source, seq, now).await?;
            }
            self.tell_followers(unit, &row).await?;
            return Ok(Sent {
                row,
                delivered,
                already: false,
            });
        }
        for thread in found
            .iter()
            .filter(|thread| !thread.archived && thread.side_of.is_none())
        {
            let is_source = Some(thread.node_id) == sender.thread();
            // The thread's own place is asked only where a person announced.
            let names_people = is_source
                || !matches!(sender, Sender::Person { .. })
                || crate::resolved::attribution(&mut *unit.conn, thread.node_id)
                    .await?
                    .names_people();
            let references = orch_documents::references::typed_in(&row.body);
            let writings = self
                .writings_for(&mut *unit.conn, thread.node_id, &references)
                .await?;
            // Idle recipients retain the same rendered body that an immediate delivery reads.
            let mut recipient = row.clone();
            let mut numbered = 0;
            recipient.body = orch_documents::references::for_reader(
                &row.body,
                &|typed| writings.get(typed).cloned(),
                &mut numbered,
            );
            let anonymous = match sender {
                Sender::Person { user, .. } if !names_people => Some(Sender::Person {
                    user: *user,
                    name: crate::instructions::A_PERSON.to_owned(),
                }),
                _ => None,
            };
            // Render before the provenance frame, which is deliberately opaque to Markdown.
            let words = announcement_text(
                &messages,
                anonymous.as_ref().unwrap_or(sender),
                plan.category,
                &recipient.title,
                &recipient.body,
                &Time::new(row.created_at).to_string(),
            );
            let how = if is_source {
                None
            } else {
                match self
                    .deliver_into(
                        unit,
                        thread.node_id,
                        &recipient,
                        sender,
                        plan,
                        &words,
                        may_reach.is_none_or(|reached| reached.contains(&thread.node_id)),
                    )
                    .await?
                {
                    Some(how) => {
                        match how {
                            INTERRUPTED => delivered.interrupted += 1,
                            STEERED => delivered.steered += 1,
                            QUEUED => delivered.queued += 1,
                            _ => delivered.deferred += 1,
                        }
                        Some(how)
                    }
                    // A thread with no turn at work, under `active`, gets nothing at all.
                    None => continue,
                }
            };
            self.put_announcement_item(unit, thread.node_id, &row, &from, how, false)
                .await?;
        }
        self.tell_followers(unit, &row).await?;
        Ok(Sent {
            row,
            delivered,
            already: false,
        })
    }

    /// Does to one thread what the announcement is to do to it. None where it gets nothing at all.
    #[allow(
        clippy::too_many_arguments,
        reason = "what is delivered, to whom, how and by what right; none of it can be left out"
    )]
    pub(crate) async fn deliver_into(
        &self,
        unit: &mut Unit<'_>,
        thread_id: Id,
        row: &rows::AnnouncementRow,
        sender: &Sender,
        plan: &Plan,
        text: &str,
        may_reach: bool,
    ) -> Result<Option<&'static str>, EngineError> {
        let Some(thread) = threads::thread(&mut *unit.conn, thread_id).await? else {
            return Ok(None);
        };
        let turn = threads::active_turn(&mut *unit.conn, thread_id).await?;
        let steerable = match &turn {
            Some(turn) if turn.state == turn_state::RUNNING => {
                self.agent_takes_a_steer(unit, &thread).await?
            }
            _ => false,
        };
        let active = turn.is_some();
        if plan.reach == Reach::Active && !active {
            return Ok(None);
        }
        let now = unit.now();
        let doing = treatment(&thread.state, plan, steerable, may_reach);
        match doing {
            Treatment::Defer => {
                let notice = json!({
                    "id": row.id, "kind": "announcement", "at": Time::new(now), "from": sender.name(),
                    "from_thread": sender.thread().is_some(), "title": row.title, "body": row.body,
                    "category": row.category,
                    "to_thread": row.to_thread_id.is_some(),
                    // The files a message to this thread carries: written by the turn that tells of it.
                    "files": row.files,
                });
                if threads::notices::append(&mut *unit.conn, thread_id, &notice).await? {
                    let seq = unit.stamp(thread_id).await?;
                    threads::set_seq_state(&mut *unit.conn, thread_id, seq).await?;
                }
                Ok(Some(DEFERRED))
            }
            Treatment::Interrupt | Treatment::Steer | Treatment::Queue => {
                let arrival = match (doing, &turn) {
                    (Treatment::Interrupt, _) => Arrival::Interrupt {
                        responsible: sender.responsible(),
                    },
                    (Treatment::Steer, Some(turn)) => Arrival::Steer { turn: turn.id },
                    _ => Arrival::Front,
                };
                self.queue_announcement_input(
                    unit,
                    thread_id,
                    ids::derived(
                        "announcement.input",
                        &[
                            row.id.as_bytes().as_slice(),
                            thread_id.as_bytes().as_slice(),
                        ],
                    ),
                    text,
                    &crate::tools::thread_message::file_parts(&row.files),
                    arrival,
                )
                .await?;
                Ok(Some(match doing {
                    Treatment::Interrupt => INTERRUPTED,
                    Treatment::Steer => STEERED,
                    _ => QUEUED,
                }))
            }
        }
    }

    /// Whether the agent of a thread may send one more announcement or message in this hour: the refusal that says
    /// when, where it may not. What it sent is counted from the rows it made, so that a restart forgets nothing.
    /// The announcement `id` is the one this call makes: where it exists already, the call is its repeat and is
    /// let through.
    pub(crate) async fn within_hourly_rate(
        &self,
        thread: &ThreadRow,
        broadcast: &Broadcast,
        id: Id,
    ) -> Result<Option<Refusal>, EngineError> {
        if broadcast.rate_per_hour <= 0 {
            return Ok(Some(Refusal::new(
                Code::Limit,
                "Agents may not announce or send to other threads here: the setting broadcast.rate_per_hour is 0. A person who sets up the topic can change it.",
            )));
        }
        let now = self.inner.clock.now();
        let mut conn = self.inner.db.acquire().await?;
        if rows::announcement(&mut conn, id).await?.is_some() {
            return Ok(None);
        }
        let sent = rows::sent_since(&mut conn, thread.node_id, now - RATE_PERIOD).await?;
        if i64::try_from(sent.len()).unwrap_or(i64::MAX) < broadcast.rate_per_hour {
            return Ok(None);
        }
        // The next one is allowed when the oldest of those that count has been sent an hour.
        let wait = sent
            .first()
            .map_or(RATE_PERIOD, |first| RATE_PERIOD - (now - *first));
        Ok(Some(Refusal::new(
            Code::RateLimited,
            format!(
                "You have announced and sent as often as this topic allows ({} in an hour). The next is allowed in {} seconds.",
                broadcast.rate_per_hour,
                wait.num_seconds().max(1)
            ),
        )))
    }

    /// Writes the line an announcement leaves in a thread, once.
    pub(crate) async fn put_announcement_item(
        &self,
        unit: &mut Unit<'_>,
        thread_id: Id,
        row: &rows::AnnouncementRow,
        from: &Json,
        delivered_as: Option<&str>,
        to_thread: bool,
    ) -> Result<(), EngineError> {
        let Some(thread) = threads::thread(&mut *unit.conn, thread_id).await? else {
            return Ok(());
        };
        let Some(segment) = thread.current_segment_id else {
            return Ok(());
        };
        let mut payload = json!({
            "announcement_id": row.id, "category": row.category, "to_thread": to_thread,
            "title": row.title, "body": row.body, "severity": row.severity, "from": from,
        });
        if let Some(how) = delivered_as {
            payload["delivered_as"] = json!(how);
        }
        crate::ingest::add_item(
            unit,
            &crate::ingest::ItemSpec {
                thread: thread_id,
                segment,
                turn: None,
                stream: "main",
                parent: None,
                kind: "announcement",
                status: "completed",
                author: None,
                payload: &payload,
                native_ref: Some(&format!("announcement:{}", row.id)),
            },
        )
        .await?;
        Ok(())
    }

    /// Takes the notices with these identifiers out of what waits for a thread's next turn, once they were said.
    pub(crate) async fn forget_pending_notices(
        &self,
        thread: Id,
        said: &[String],
    ) -> Result<(), EngineError> {
        let mut tx = self.inner.db.begin().await?;
        orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
        let now = self.inner.clock.now();
        let finished = {
            let mut unit = Unit::new(&mut tx, now);
            if threads::lock(&mut *unit.conn, thread).await?.is_some()
                && threads::notices::forget(&mut *unit.conn, thread, said).await?
            {
                let seq = unit.stamp(thread).await?;
                threads::set_seq_state(&mut *unit.conn, thread, seq).await?;
            }
            unit.finish().await?
        };
        self.commit(tx, finished).await?;
        Ok(())
    }

    /// Acknowledges only the page pinned before this turn was sent to the harness.
    pub(crate) async fn forget_pending_notice_positions(
        &self,
        thread: Id,
        turn: Id,
        positions: &[i64],
    ) -> Result<(), EngineError> {
        let mut tx = self.inner.db.begin().await?;
        orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
        let finished = {
            let mut unit = Unit::new(&mut tx, self.inner.clock.now());
            if threads::lock(&mut *unit.conn, thread).await?.is_some()
                && let Some(pinned) =
                    threads::notices::delivery_positions(&mut *unit.conn, thread, turn).await?
            {
                let said: Vec<_> = positions
                    .iter()
                    .copied()
                    .filter(|position| pinned.contains(position))
                    .collect();
                if threads::notices::forget_positions(&mut *unit.conn, thread, &said).await? {
                    let seq = unit.stamp(thread).await?;
                    threads::set_seq_state(&mut *unit.conn, thread, seq).await?;
                }
            }
            unit.finish().await?
        };
        self.commit(tx, finished).await?;
        Ok(())
    }

    /// Whether the agent of a thread takes a message into a turn that runs.
    pub(crate) async fn agent_takes_a_steer(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
    ) -> Result<bool, EngineError> {
        let Some(agent) = thread.agent_id else {
            return Ok(false);
        };
        let Some(agent) =
            orch_db::queries::resources::thread_engine::agent(&mut *unit.conn, agent).await?
        else {
            return Ok(false);
        };
        Ok(crate::harnesses::descriptor_of(&agent)?
            .is_some_and(|descriptor| descriptor.capabilities.steer))
    }

    /// Tells eligible followers and destinations in the transaction that writes the announcement.
    pub(crate) async fn tell_followers(
        &self,
        unit: &mut Unit<'_>,
        row: &rows::AnnouncementRow,
    ) -> Result<(), EngineError> {
        crate::notifications::announcement(&mut *unit.conn, row).await?;
        Ok(())
    }
}

/// The sender as the line of an announcement names it.
async fn from_json(conn: &mut orch_db::Conn, sender: &Sender) -> Result<Json, EngineError> {
    Ok(match sender {
        Sender::Agent(thread) => json!({ "thread": { "id": thread.node_id, "name": thread.name } }),
        Sender::Person { user, .. } => {
            json!({ "user": crate::views::user_ref_json(conn, *user).await? })
        }
    })
}

/// How many threads each of the announcements reached in each way, counted from the lines they left.
pub async fn delivered_counts(
    conn: &mut orch_db::Conn,
    announcements: &[Id],
) -> Result<std::collections::HashMap<Id, Delivered>, EngineError> {
    let mut counted: std::collections::HashMap<Id, Delivered> = std::collections::HashMap::new();
    for (id, how, count) in rows::delivered_counts(conn, announcements).await? {
        let entry = counted.entry(id).or_insert_with(no_delivery);
        match how.as_str() {
            INTERRUPTED => entry.interrupted += count,
            STEERED => entry.steered += count,
            QUEUED => entry.queued += count,
            DEFERRED => entry.deferred += count,
            _ => {}
        }
    }
    Ok(counted)
}

// ---------------------------------------------------------------- the tool

/// Who hears it, as the tool's argument says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Audience {
    Followers,
    Topic,
    Project,
    Org,
}

/// The arguments of `announce`, once the schema has let them through.
#[derive(Debug, Deserialize)]
struct Arguments {
    title: String,
    body: Option<String>,
    category: Option<AnnouncementCategory>,
    audience: Option<Audience>,
    severity: Option<AnnouncementSeverity>,
    delivery: Option<AnnouncementDelivery>,
    wake_idle: Option<bool>,
}

impl Engine {
    /// `announce`: the call of an agent.
    pub(crate) async fn announce_call(
        &self,
        call: &CallContext,
        thread: &ThreadRow,
        _settings: &ToolSettings,
    ) -> Result<Answer, EngineError> {
        let arguments: Arguments = match serde_json::from_value(call.arguments.clone()) {
            Ok(arguments) => arguments,
            Err(error) => return Ok(Refusal::invalid(error.to_string()).into()),
        };
        if arguments
            .body
            .as_ref()
            .map_or(0, |body| body.chars().count())
            > BODY_MAX_CHARS
        {
            return Ok(Refusal::invalid("\"body\" is at most 1500 characters.").into());
        }
        let category = arguments.category.unwrap_or(AnnouncementCategory::Notice);
        let audience = arguments.audience.unwrap_or(match category {
            AnnouncementCategory::Notice => Audience::Topic,
            AnnouncementCategory::Milestone => Audience::Followers,
        });
        let broadcast = {
            let mut conn = self.inner.db.acquire().await?;
            Broadcast::load(&mut conn, thread.topic_id).await?
        };
        if (audience == Audience::Project
            && !matches!(broadcast.agent_scope.as_str(), "project" | "org"))
            || (audience == Audience::Org && broadcast.agent_scope != "org")
        {
            return Ok(Refusal::invalid(
                "That audience is outside the announcement reach set by broadcast.agent_scope here. Use the topic audience, or ask a person to change the announcement reach.",
            )
            .into());
        }
        let now = self.inner.clock.now();
        let id = announcement_id(thread.node_id, &call.call_ref);
        if audience != Audience::Followers
            && let Some(refusal) = self.within_hourly_rate(thread, &broadcast, id).await?
        {
            return Ok(refusal.into());
        }
        // What the agent may choose it chooses; the rest is the topic's.
        let may = |option: &str| {
            broadcast
                .agent_options
                .iter()
                .any(|listed| listed == option)
        };
        if arguments
            .delivery
            .is_some_and(|chosen| !may(delivery_word(chosen)))
        {
            return Ok(Refusal::invalid("The setting broadcast.agent_options does not allow that delivery here. Omit delivery to use the topic's setting.").into());
        }
        if arguments.wake_idle.is_some() && !may("wake_idle") {
            return Ok(Refusal::invalid("The setting broadcast.agent_options does not allow wake_idle here. Omit it to use the topic's setting.").into());
        }
        let delivery = arguments.delivery.unwrap_or(broadcast.delivery);
        let wake_idle = arguments.wake_idle.unwrap_or(broadcast.wake_idle);
        let (resolution, body) = {
            let mut conn = self.inner.db.acquire().await?;
            let body = arguments.body.as_deref().unwrap_or_default();
            let resolution = self
                .resolve_references(&mut conn, thread.node_id, body)
                .await?;
            let stored = resolution.store(body);
            (resolution, stored)
        };
        let plan = Plan {
            category,
            title: arguments.title.clone(),
            body,
            severity: arguments.severity.unwrap_or(AnnouncementSeverity::Info),
            delivery,
            fallback: broadcast.fallback,
            wake_idle,
            reach: if audience == Audience::Followers {
                Reach::Nobody
            } else {
                Reach::All
            },
        };
        let mut tx = self.inner.db.begin().await?;
        let (finished, sent, totals) = {
            let mut unit = Unit::new(&mut tx, now);
            let (sent, totals) = if matches!(audience, Audience::Project | Audience::Org) {
                self.announce_to_place_as_agent(&mut unit, thread, id, &plan, &arguments, audience)
                    .await?
            } else {
                let sender = Sender::Agent(Box::new(thread.clone()));
                let sent = self
                    .deliver_announcement_with_references(
                        &mut unit,
                        thread.topic_id,
                        id,
                        None,
                        &sender,
                        &plan,
                        None,
                        arguments.body.as_deref().unwrap_or_default(),
                    )
                    .await?;
                let totals = sent.delivered;
                (sent, totals)
            };
            if !sent.already {
                let mut entry = Entry::new(
                    Actor::Agent(thread.node_id),
                    orch_db::audit::action!(AuditAction::AnnouncementSent).as_str(),
                    "topic",
                    Some(thread.topic_id),
                    "An agent announced something.",
                );
                entry.node_id = Some(thread.topic_id);
                entry.org_id = Some(thread.org_id);
                entry.details = json!({ "announcement": sent.row.id, "category": sent.row.category, "targets": sent.row.targets });
                append(&mut *unit.conn, &entry, &Origin::default(), now).await?;
            }
            (unit.finish().await?, sent, totals)
        };
        self.commit(tx, finished).await?;
        let audience_word = match audience {
            Audience::Followers => "followers",
            Audience::Topic => "topic",
            Audience::Project => "project",
            Audience::Org => "org",
        };
        let followers_only = audience == Audience::Followers;
        let mut answer = json!({
            "announcement_id": sent.row.id,
            "category": sent.row.category,
            "audience": audience_word,
            "applied": if followers_only { Json::Null } else {
                json!({ "delivery": sent.row.delivery, "fallback": sent.row.fallback, "wake_idle": sent.row.wake_idle })
            },
            "delivered": if followers_only { Json::Null } else {
                json!({ "interrupted": totals.interrupted, "steered": totals.steered, "queued": totals.queued, "deferred": totals.deferred })
            },
        });
        if !resolution.is_empty() {
            answer["references"] = resolution.feedback();
        }
        Ok(Answer::Done(answer))
    }

    /// An agent's announcement below its chosen project or org: one in every eligible open topic,
    /// with its own topic's announcement named in the answer.
    async fn announce_to_place_as_agent(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        id: Id,
        plan: &Plan,
        arguments: &Arguments,
        audience: Audience,
    ) -> Result<(Sent, Delivered), EngineError> {
        let sender = Sender::Agent(Box::new(thread.clone()));
        let project = if audience == Audience::Org {
            let way = hierarchy::with_ancestors(&mut *unit.conn, &[thread.topic_id]).await?;
            way.iter()
                .filter(|node| node.kind == "org")
                .max_by_key(|node| node.path.len())
                .map(|node| node.id)
        } else {
            rows::node_parent(&mut *unit.conn, thread.topic_id).await?
        };
        let topics = match project {
            Some(project) => {
                let reachable = rows::open_topics_of(&mut *unit.conn, project).await?;
                // What was made private is passed over; whether it is does not depend on who asks.
                let now = Time::new(unit.now());
                let sender =
                    Principal::load(&mut *unit.conn, Caller::session(thread.owner_id), now).await?;
                Self::those_that_follow(unit, &sender, project, reachable).await?
            }
            None => vec![thread.topic_id],
        };
        let group = id;
        let mut own: Option<Sent> = None;
        let mut totals = no_delivery();
        for topic in topics {
            let broadcast = Broadcast::load(&mut *unit.conn, topic).await?;
            let mut here = plan.clone();
            // What the agent was not given the choice of, the topic decides.
            if topic != thread.topic_id {
                here.delivery = arguments
                    .delivery
                    .filter(|_| plan.delivery != broadcast.delivery)
                    .map_or(broadcast.delivery, |_| plan.delivery);
                here.fallback = broadcast.fallback;
                here.wake_idle = broadcast.wake_idle;
            }
            let topic_id = if topic == thread.topic_id {
                id
            } else {
                ids::derived(
                    "announcement.topic",
                    &[id.as_bytes().as_slice(), topic.as_bytes().as_slice()],
                )
            };
            let sent = self
                .deliver_announcement_with_references(
                    unit,
                    topic,
                    topic_id,
                    Some(group),
                    &sender,
                    &here,
                    None,
                    arguments.body.as_deref().unwrap_or_default(),
                )
                .await?;
            totals.interrupted += sent.delivered.interrupted;
            totals.steered += sent.delivered.steered;
            totals.queued += sent.delivered.queued;
            totals.deferred += sent.delivered.deferred;
            if topic == thread.topic_id {
                own = Some(sent);
            }
        }
        let own = match own {
            Some(own) => own,
            None => {
                self.deliver_announcement_with_references(
                    unit,
                    thread.topic_id,
                    id,
                    Some(group),
                    &sender,
                    plan,
                    None,
                    arguments.body.as_deref().unwrap_or_default(),
                )
                .await?
            }
        };
        Ok((own, totals))
    }
}

/// The identifier of the announcement a call makes.
fn announcement_id(thread: Id, call_ref: &str) -> Id {
    ids::derived(
        "announcement",
        &[thread.as_bytes().as_slice(), call_ref.as_bytes()],
    )
}

// ---------------------------------------------------------------- a person's announcement

/// Why an announcement of a person was not made.
#[derive(Debug, thiserror::Error)]
pub enum AnnounceError {
    /// There is no such org, project or topic.
    #[error("there is no such org, project or topic")]
    NotFound,
    /// The sender lacks what it takes, and this is all of it.
    #[error("the sender lacks what an announcement needs")]
    Denied(Box<orch_access_contract::decisions::Refusal>),
    /// The request is not one that can be made.
    #[error("{0}")]
    Invalid(String),
    /// Something the engine stands on failed.
    #[error(transparent)]
    Internal(#[from] EngineError),
}

impl From<orch_db::DbError> for AnnounceError {
    fn from(error: orch_db::DbError) -> Self {
        Self::Internal(error.into())
    }
}

impl From<orch_authz::Error> for AnnounceError {
    fn from(error: orch_authz::Error) -> Self {
        Self::Internal(error.into())
    }
}

/// What a person sends.
#[derive(Debug, Clone)]
pub struct PersonAnnouncement {
    /// What happened.
    pub title: String,
    /// The rest.
    pub body: String,
    /// A notice unless given.
    pub category: Option<AnnouncementCategory>,
    /// How much it matters.
    pub severity: Option<AnnouncementSeverity>,
    /// All the threads unless given.
    pub targets: Option<orch_api_contract::topics::AnnouncementSendAudience>,
    /// The topic's setting unless given.
    pub delivery: Option<AnnouncementDelivery>,
    /// The topic's setting unless given.
    pub fallback: Option<AnnouncementFallback>,
    /// The topic's setting unless given.
    pub wake_idle: Option<bool>,
}

/// A person's announcement to a place: one in each open topic, and how many were left out.
#[derive(Debug, Clone)]
pub struct PlaceSent {
    /// What the announcements have in common.
    pub group_id: Id,
    /// What was made, one for each topic.
    pub sent: Vec<Sent>,
    /// How many topics were left out: the archived, the done, and those made private.
    pub skipped: i64,
}

impl Engine {
    /// Of the topics a project-wide announcement could reach, those that follow the project: neither the
    /// topic nor a folder between it and the project was made private. What was made private there is passed
    /// over, and everything in it.
    async fn those_that_follow(
        unit: &mut Unit<'_>,
        principal: &Principal,
        project: Id,
        topics: Vec<Id>,
    ) -> Result<Vec<Id>, EngineError> {
        if topics.is_empty() {
            return Ok(topics);
        }
        let wanted: HashSet<Id> = topics.iter().copied().collect();
        // For each topic, the nodes on the way down from the project to it, itself last.
        let ways: Vec<(Id, Vec<Id>)> = hierarchy::with_ancestors(&mut *unit.conn, &topics)
            .await?
            .into_iter()
            .filter(|row| wanted.contains(&row.id))
            .map(|row| {
                let below = row
                    .path
                    .iter()
                    .copied()
                    .skip_while(|node| *node != project)
                    .skip(1)
                    .collect();
                (row.id, below)
            })
            .collect();
        let mut asked: Vec<Id> = ways
            .iter()
            .flat_map(|(_, way)| way.iter().copied())
            .collect();
        asked.sort_unstable();
        asked.dedup();
        let private = nodes::restricted_below(
            &mut *unit.conn,
            principal,
            &asked,
            project,
            orch_authz::Policy::default(),
        )
        .await?;
        Ok(topics
            .into_iter()
            .filter(|topic| {
                ways.iter()
                    .find(|(id, _)| id == topic)
                    .is_some_and(|(_, way)| way.iter().all(|node| !private.contains(node)))
            })
            .collect())
    }

    fn check_person_announcement(request: &PersonAnnouncement) -> Result<(), AnnounceError> {
        let title = request.title.chars().count();
        if title == 0 || title > crate::tools::definitions::TITLE_MAX_CHARS {
            return Err(AnnounceError::Invalid(
                "the title is from 1 to 120 characters".to_owned(),
            ));
        }
        if request.body.chars().count() > BODY_MAX_CHARS {
            return Err(AnnounceError::Invalid(
                "the body is at most 1500 characters".to_owned(),
            ));
        }
        Ok(())
    }

    /// The threads of a topic whose running turn the person may steer or interrupt: those on which they hold
    /// `thread.interact`.
    async fn threads_the_person_may_reach(
        &self,
        unit: &mut Unit<'_>,
        principal: &Principal,
        topic: Id,
    ) -> Result<HashSet<Id>, AnnounceError> {
        let mut reached = HashSet::new();
        for thread in rows::topic_threads(&mut *unit.conn, topic).await? {
            let allowed = nodes::check(
                &mut *unit.conn,
                principal,
                thread.node_id,
                Permission::ThreadInteract,
                Act::Use,
                orch_authz::Policy::default(),
            )
            .await?;
            if matches!(
                allowed,
                orch_access_contract::decisions::Decision::Allowed { .. }
            ) {
                reached.insert(thread.node_id);
            }
        }
        Ok(reached)
    }

    fn plan_of(request: &PersonAnnouncement, broadcast: &Broadcast) -> Plan {
        Plan {
            category: request.category.unwrap_or(AnnouncementCategory::Notice),
            title: request.title.clone(),
            body: request.body.clone(),
            severity: request.severity.unwrap_or(AnnouncementSeverity::Info),
            delivery: request.delivery.unwrap_or(broadcast.delivery),
            fallback: request.fallback.unwrap_or(broadcast.fallback),
            wake_idle: request.wake_idle.unwrap_or(broadcast.wake_idle),
            reach: match request.targets {
                Some(orch_api_contract::topics::AnnouncementSendAudience::Active) => Reach::Active,
                _ => Reach::All,
            },
        }
    }

    /// Uses the current guarded transaction to announce to any place under its Interact permission and each topic's settings.
    pub async fn announce_to_place_guarded(
        &self,
        mut tx: orch_db::Tx,
        principal: Principal,
        place: Id,
        request: &PersonAnnouncement,
    ) -> Result<PlaceSent, AnnounceError> {
        Self::check_person_announcement(request)?;
        let now = principal.now().get();
        let (finished, group, made, skipped) = {
            let mut unit = Unit::new(&mut tx, now);
            let node = rows::node_kind(&mut *unit.conn, place).await?;
            let permission = match node.as_deref() {
                Some("org") => Permission::OrgInteract,
                Some("project") => Permission::ProjectInteract,
                Some("topic") => Permission::TopicInteract,
                _ => return Err(AnnounceError::NotFound),
            };
            if let orch_access_contract::decisions::Decision::Refused(refusal) = nodes::check(
                &mut *unit.conn,
                &principal,
                place,
                permission,
                Act::Use,
                orch_authz::Policy::default(),
            )
            .await?
            {
                return Err(AnnounceError::Denied(Box::new(refusal)));
            }
            let reachable = rows::open_topics_of(&mut *unit.conn, place).await?;
            let topics = Self::those_that_follow(&mut unit, &principal, place, reachable).await?;
            let all = rows::count_topics_of(&mut *unit.conn, place).await?;
            let skipped = all - i64::try_from(topics.len()).unwrap_or(all);
            let group = orch_types::new_id();
            let name = display_name(&mut *unit.conn, principal.user()).await?;
            let sender = Sender::Person {
                user: principal.user(),
                name,
            };
            let mut made = Vec::new();
            for topic in topics {
                let broadcast = Broadcast::load(&mut *unit.conn, topic).await?;
                let plan = Self::plan_of(request, &broadcast);
                let reached = self
                    .threads_the_person_may_reach(&mut unit, &principal, topic)
                    .await?;
                let sent = self
                    .deliver_announcement(
                        &mut unit,
                        topic,
                        orch_types::new_id(),
                        Some(group),
                        &sender,
                        &plan,
                        Some(&reached),
                    )
                    .await?;
                audit_person(&mut unit, principal.user(), topic, &sent.row).await?;
                made.push(sent);
            }
            (unit.finish().await?, group, made, skipped)
        };
        self.commit(tx, finished).await?;
        Ok(PlaceSent {
            group_id: group,
            sent: made,
            skipped,
        })
    }

    /// The announcements as the wire has them, each with what it did.
    pub async fn announcement_views(
        &self,
        rows_of: &[rows::AnnouncementRow],
    ) -> Result<Vec<Announcement>, EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        self.announcement_views_on(&mut conn, rows_of).await
    }

    /// Projects announcements and their delivery counts in the caller's snapshot transaction.
    pub async fn announcement_views_on(
        &self,
        conn: &mut orch_db::Conn,
        rows_of: &[rows::AnnouncementRow],
    ) -> Result<Vec<Announcement>, EngineError> {
        let ids: Vec<Id> = rows_of.iter().map(|row| row.id).collect();
        let delivered = delivered_counts(conn, &ids).await?;
        let mut views = Vec::with_capacity(rows_of.len());
        for row in rows_of {
            views.push(announcement_view(conn, row, delivered.get(&row.id).copied()).await?);
        }
        Ok(views)
    }
}

async fn display_name(conn: &mut orch_db::Conn, user: Id) -> Result<String, EngineError> {
    Ok(threads::people(conn, &[user])
        .await?
        .into_iter()
        .next()
        .map(|person| person.display_name)
        .unwrap_or_default())
}

async fn audit_person(
    unit: &mut Unit<'_>,
    user: Id,
    topic: Id,
    row: &rows::AnnouncementRow,
) -> Result<(), EngineError> {
    let now = unit.now();
    let mut entry = Entry::new(
        Actor::User(user),
        AuditAction::AnnouncementSent.as_str(),
        "topic",
        Some(topic),
        "An announcement was sent to threads.",
    );
    entry.node_id = Some(topic);
    entry.details =
        json!({ "announcement": row.id, "category": row.category, "targets": row.targets });
    append(&mut *unit.conn, &entry, &Origin::default(), now).await?;
    Ok(())
}

/// One announcement as the wire has it.
pub async fn announcement_view(
    conn: &mut orch_db::Conn,
    row: &rows::AnnouncementRow,
    delivered: Option<Delivered>,
) -> Result<Announcement, EngineError> {
    let from = match (row.from_thread_id, row.from_user_id) {
        (Some(thread), _) => AnnouncementFrom::Thread(ThreadMention {
            id: thread,
            name: threads::node_name(conn, thread).await?.unwrap_or_default(),
        }),
        (None, Some(user)) => {
            let reference = crate::views::user_ref_json(conn, user).await?;
            AnnouncementFrom::User(serde_json::from_value::<UserRef>(reference)?)
        }
        (None, None) => {
            return Err(EngineError::unreadable("an announcement nobody sent"));
        }
    };
    let targets = match (row.targets.as_str(), row.to_thread_id) {
        ("thread", Some(thread)) => {
            AnnouncementTargets::Thread(AnnouncementThreadTarget { thread_id: thread })
        }
        ("active", _) => AnnouncementTargets::Audience(AnnouncementAudience::Active),
        ("none", _) => AnnouncementTargets::Audience(AnnouncementAudience::Nobody),
        _ => AnnouncementTargets::Audience(AnnouncementAudience::All),
    };
    Ok(Announcement {
        id: row.id,
        topic_id: row.topic_id,
        group_id: row.group_id,
        category: word_of(&row.category)?,
        title: row.title.clone(),
        body: row.body.clone(),
        severity: word_of(&row.severity)?,
        from,
        targets,
        files: serde_json::from_value::<Vec<AnnouncementFile>>(
            row.files
                .as_array()
                .map(|files| {
                    Json::Array(
                        files
                            .iter()
                            .map(|file| json!({ "name": file["name"], "size": file["size"] }))
                            .collect(),
                    )
                })
                .unwrap_or_else(|| json!([])),
        )?,
        delivery: word_of(&row.delivery)?,
        fallback: word_of(&row.fallback)?,
        wake_idle: row.wake_idle,
        delivered: delivered.unwrap_or_else(no_delivery),
        created_at: Time::new(row.created_at),
        topic_seq: row.topic_seq,
    })
}

/// A word of the stored row as the contract's type of that name.
fn word_of<T: serde::de::DeserializeOwned>(value: &str) -> Result<T, serde_json::Error> {
    serde_json::from_value(Json::String(value.to_owned()))
}

/// A moment as a time.
#[must_use]
pub fn when(at: DateTime<Utc>) -> Time {
    Time::new(at)
}
