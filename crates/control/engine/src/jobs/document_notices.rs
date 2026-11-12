//! Bounded, restartable discussion notification fan-out with current reading checks.

use super::runner::{Done, Job, Phase, Phased, Stop, Undone, initial_progress};
use crate::{Engine, EngineError};
use async_trait::async_trait;
use orch_access_contract::decisions::Decision;
use orch_access_contract::permissions::Permission;
use orch_authz::{Caller, Policy, Principal};
use orch_db::queries::notifications::people::NewNotification;
use orch_db::queries::{admin_messages, documents, identity, jobs};
use orch_db::{Conn, DbError};
use orch_types::{Id, Json, Time, new_id};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeSet;

/// Fan-out changes no authority and uses the existing durable job runner.
pub const KIND: &str = "document_notices";
/// One page bounds both memory and the interval between cancellation checks.
pub const BATCH: usize = 200;
/// Progress belongs to the ordinary job view; cursor and delivered count are checkpointed atomically.
pub const PHASES: [Phase; 1] = [Phase {
    name: "notify_people",
    weight: 1,
}];

/// Immutable event facts and holder definitions, not an expanded roster or comment history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    /// The document.
    pub document: Id,
    /// The discussion.
    pub discussion: Id,
    /// Its block, where one remains.
    pub block: Option<Id>,
    /// The accepted comment.
    pub comment: Option<Id>,
    /// The author, omitted from the audience.
    pub by: Id,
    /// Their event-time displayed name.
    pub by_name: String,
    /// A bounded quote of the accepted words.
    pub quote: String,
    /// A new discussion additionally names its owners and last block writer.
    pub started: bool,
    /// The document sequence at acceptance.
    pub sequence: i64,
    /// The subscriber generation at acceptance.
    pub subscriptions: i64,
    /// The audience's user-creation cutoff and event time.
    pub at: chrono::DateTime<chrono::Utc>,
    /// The unexpanded owner and block-writer definitions.
    pub owners: identity::MentionSources,
}

/// One page and whether another page can remain.
pub struct Audience {
    /// At most BATCH people, in immutable identifier order.
    pub people: Vec<Id>,
    /// Whether a further candidate was observed.
    pub more: bool,
}

/// The union of two bounded pages cannot hold an unbounded participant or owner roster.
pub async fn audience(
    conn: &mut Conn,
    plan: &Plan,
    after: Option<Id>,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Audience, EngineError> {
    let take =
        i64::try_from(BATCH + 2).map_err(|error| EngineError::unreadable(error.to_string()))?;
    let mut people: BTreeSet<_> = documents::people::discussion_audience_page(
        conn,
        plan.document,
        plan.discussion,
        after,
        plan.sequence,
        plan.subscriptions,
        plan.started,
        take,
    )
    .await?
    .into_iter()
    .collect();
    if plan.started {
        people.extend(
            identity::mention_readers_page(conn, &plan.owners, after, "", take, now, plan.at)
                .await?
                .into_iter()
                .map(|person| person.id),
        );
    }
    people.remove(&plan.by);
    let more = people.len() > BATCH;
    Ok(Audience {
        people: people.into_iter().take(BATCH).collect(),
        more,
    })
}

/// Current View and event sequence are checked again before renewing an Inbox item.
pub async fn deliver(
    conn: &mut Conn,
    plan: &Plan,
    people: &[Id],
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), EngineError> {
    let Some(document) = documents::get(conn, plan.document)
        .await?
        .filter(|row| row.deleted_at.is_none())
    else {
        return Ok(());
    };
    orch_db::queries::transaction_guard::watch_nodes(conn, &[document.node_id]).await?;
    orch_db::queries::transaction_guard::watch_standing(conn).await?;
    // Different replies may be queued concurrently. A newer reply always wins each recipient's renewable notice.
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended('doc.comment:' || $1::uuid::text,0))",
    )
    .bind(plan.discussion)
    .execute(&mut *conn)
    .await
    .map_err(DbError::from)?;
    let verb = if plan.started {
        "started"
    } else {
        "answered in"
    };
    let title = format!("{} {verb} a discussion on {}", plan.by_name, document.title);
    let key = format!("doc.comment:{}", plan.discussion);
    let data = json!({"document_id":plan.document,"block_id":plan.block,"discussion_id":plan.discussion,"comment_id":plan.comment,"by":plan.by,"discussion_seq":plan.sequence,"path":format!("/documents/{}",plan.document)});
    for person in people {
        let principal = match Principal::load(conn, Caller::session(*person), Time::new(now)).await
        {
            Ok(principal) => principal,
            Err(orch_authz::Error::NoSuchUser(_) | orch_authz::Error::Suspended { .. }) => continue,
            Err(error) => return Err(error.into()),
        };
        match orch_authz::documents::check(
            conn,
            &principal,
            plan.document,
            Permission::DocView,
            Policy::default(),
        )
        .await
        {
            Ok(Decision::Allowed { .. }) => {}
            Ok(Decision::Refused(_))
            | Err(
                orch_authz::Error::NotFound(_)
                | orch_authz::Error::NoSuchUser(_)
                | orch_authz::Error::Suspended { .. },
            ) => continue,
            Err(error) => return Err(error.into()),
        }
        let newer:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM notifications WHERE user_id=$1 AND dedupe_key=$2 AND COALESCE((data->>'discussion_seq')::bigint,0)>$3)").bind(person).bind(&key).bind(plan.sequence).fetch_one(&mut *conn).await.map_err(DbError::from)?;
        if newer {
            continue;
        }
        crate::notifications::tell_document_notice(
            conn,
            document.org_id,
            &NewNotification {
                user_id: *person,
                kind: "doc.comment",
                node_id: Some(document.node_id),
                thread_id: None,
                approval_id: None,
                chat_id: None,
                title: &title,
                body: &plan.quote,
                data: &data,
                dedupe_key: &key,
                at: plan.at,
            },
        )
        .await?;
    }
    Ok(())
}

/// A large audience becomes a durable cancellable job in the same transaction as its comment.
pub async fn enqueue(conn: &mut Conn, plan: &Plan) -> Result<Id, EngineError> {
    enqueue_plan(
        conn,
        plan.by,
        None,
        &json!({"notice":plan,"processed":0}),
        plan.at,
    )
    .await
}

async fn enqueue_plan(
    conn: &mut Conn,
    by: Id,
    node: Option<Id>,
    plan: &Json,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<Id, EngineError> {
    let id = new_id();
    jobs::insert(
        conn,
        &jobs::NewJob {
            id,
            kind: KIND,
            node_id: node,
            environment_id: None,
            provider_id: None,
            plan,
            progress: &serde_json::to_value(initial_progress(&PHASES))?,
            created_by: Some(by),
        },
        at,
    )
    .await?;
    Ok(id)
}

/// The same notice runner materializes the already accepted, frozen message audience.
pub async fn enqueue_message(conn: &mut Conn, message: Id) -> Result<Id, EngineError> {
    let row = admin_messages::lock(conn, message)
        .await?
        .ok_or_else(|| EngineError::unreadable("the accepted message disappeared"))?;
    if let Some(job) = row.delivery_job_id {
        return Ok(job);
    }
    let job = enqueue_plan(
        conn,
        row.sender_id,
        row.org_id,
        &json!({"admin_message":row.id,"total":row.recipient_count,"processed":0}),
        row.created_at,
    )
    .await?;
    admin_messages::attach_job(conn, row.id, job).await?;
    orch_db::idempotency::creation::observe_with(
        conn,
        Some(row.sender_id),
        "message",
        row.id,
        json!({"count":row.recipient_count,"job":job}),
    )
    .await?;
    Ok(job)
}

enum Notice {
    Discussion(Box<Plan>),
    Message(Id),
}

/// Writes one mandatory Inbox record through the existing ordinary notice/outbox producer.
async fn deliver_message(
    conn: &mut Conn,
    message: &admin_messages::MessageRow,
    people: &[Id],
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), EngineError> {
    let root: Option<Id> = match message.org_id {
        Some(org) => sqlx::query_scalar("SELECT org_id FROM nodes WHERE id=$1")
            .bind(org)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?,
        None => None,
    };
    let data = json!({"message_id":message.id,"by":message.sender_id,"org_id":message.org_id,
        "org_name":message.org_name,"path":format!("/messages/{}",message.id)});
    let key = format!("admin_message:{}", message.id);
    for person in people {
        crate::notifications::tell_person_notice(
            conn,
            root,
            &NewNotification {
                user_id: *person,
                kind: "admin_message",
                node_id: message.org_id,
                thread_id: None,
                approval_id: None,
                chat_id: None,
                title: &message.title,
                body: &message.body,
                data: &data,
                dedupe_key: &key,
                at: message.created_at,
            },
        )
        .await?;
    }
    admin_messages::delivered(conn, message.id, people, now).await?;
    Ok(())
}

/// A recovered phase reads its durable cursor before taking another bounded batch.
pub struct Delivering {
    engine: Engine,
    notice: Notice,
}
impl Delivering {
    /// Decodes only event facts from the private job plan.
    pub fn of(engine: Engine, row: &jobs::JobRow) -> Result<Self, EngineError> {
        let notice = match (row.plan.get("notice"), row.plan.get("admin_message")) {
            (Some(plan), None) => {
                Notice::Discussion(Box::new(serde_json::from_value(plan.clone())?))
            }
            (None, Some(message)) => Notice::Message(serde_json::from_value(message.clone())?),
            _ => {
                return Err(EngineError::unreadable(
                    "the notice job has no unique native subject",
                ));
            }
        };
        Ok(Self { engine, notice })
    }
}
#[async_trait]
impl Phased for Delivering {
    async fn run(&self, job: &Job, phase: &str) -> Result<Done, Stop> {
        if phase != "notify_people" {
            return Err(super::runner::Failure::new(
                "internal",
                "an unknown discussion notice phase was requested",
            )
            .into());
        }
        loop {
            job.check()?;
            let mut tx = self.engine.db().begin().await?;
            orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
            let state = jobs::job(&mut tx, job.id())
                .await?
                .ok_or_else(|| EngineError::unreadable("the discussion notice job disappeared"))?;
            let after = state
                .plan
                .get("after")
                .and_then(Json::as_str)
                .and_then(|value| value.parse().ok());
            if state.cancel_requested() {
                return Err(Stop::Cancelled);
            }
            orch_db::queries::transaction_guard::watch_standing(&mut tx).await?;
            let (page, message) = match &self.notice {
                Notice::Discussion(plan) => {
                    (audience(&mut tx, plan, after, job.now()).await?, None)
                }
                Notice::Message(id) => {
                    let message = admin_messages::get(&mut tx, *id).await?;
                    if let Some(message) = &message
                        && (message.delivery_job_id != Some(job.id())
                            || state.created_by != Some(message.sender_id)
                            || state.node_id != message.org_id)
                    {
                        return Err(EngineError::unreadable(
                            "the notice job no longer matches its accepted message",
                        )
                        .into());
                    }
                    let mut people = if message.is_some() {
                        admin_messages::recipient_page(
                            &mut tx,
                            *id,
                            after,
                            i64::try_from(BATCH + 1)
                                .map_err(|error| EngineError::unreadable(error.to_string()))?,
                        )
                        .await?
                    } else {
                        Vec::new()
                    };
                    let more = people.len() > BATCH;
                    people.truncate(BATCH);
                    (Audience { people, more }, message)
                }
            };
            match &self.notice {
                Notice::Discussion(plan) => deliver(&mut tx, plan, &page.people, job.now()).await?,
                Notice::Message(_) => {
                    if let Some(message) = &message {
                        deliver_message(&mut tx, message, &page.people, job.now()).await?;
                    }
                }
            }
            let processed = state
                .plan
                .get("processed")
                .and_then(Json::as_u64)
                .unwrap_or(0)
                .saturating_add(u64::try_from(page.people.len()).unwrap_or(u64::MAX));
            jobs::merge_plan(
                &mut tx,
                job.id(),
                &json!({"after":page.people.last().copied().or(after),"processed":processed}),
                job.now(),
            )
            .await?;
            tx.commit().await.map_err(DbError::from)?;
            let fraction = state
                .plan
                .get("total")
                .and_then(Json::as_u64)
                .map_or(0.0, |total| {
                    if total == 0 {
                        1.0
                    } else {
                        (processed as f64 / total as f64).min(1.0)
                    }
                });
            job.progress(fraction, Some(&processed.to_string())).await?;
            if !page.more {
                return Ok(Done::Finished);
            }
        }
    }
    async fn undo(&self, job: &Job, _why: &Stop) -> Undone {
        if job
            .plan()
            .await
            .ok()
            .and_then(|plan| plan.get("processed").and_then(Json::as_u64))
            .unwrap_or(0)
            == 0
        {
            Undone::Everything
        } else {
            Undone::Partly(
                "Already delivered notices remain; no further recipients will be notified"
                    .to_owned(),
            )
        }
    }
    async fn result(&self, job: &Job) -> Option<Json> {
        job.plan()
            .await
            .ok()
            .map(|plan| json!({"processed":plan["processed"]}))
    }
}
