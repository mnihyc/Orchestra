//! Bounded, restartable discussion notification fan-out with current reading checks.

use super::runner::{Done, Job, Phase, Phased, Stop, Undone, initial_progress};
use crate::{Engine, EngineError};
use async_trait::async_trait;
use orch_access_contract::decisions::Decision;
use orch_access_contract::permissions::Permission;
use orch_authz::{Caller, Policy, Principal};
use orch_db::queries::notifications::people::NewNotification;
use orch_db::queries::{documents, identity, jobs};
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
    let id = new_id();
    jobs::insert(
        conn,
        &jobs::NewJob {
            id,
            kind: KIND,
            node_id: None,
            environment_id: None,
            provider_id: None,
            plan: &json!({"notice":plan,"processed":0}),
            progress: &serde_json::to_value(initial_progress(&PHASES))?,
            created_by: Some(plan.by),
        },
        plan.at,
    )
    .await?;
    Ok(id)
}

/// A recovered phase reads its durable cursor before taking another bounded batch.
pub struct Delivering {
    engine: Engine,
    plan: Plan,
}
impl Delivering {
    /// Decodes only event facts from the private job plan.
    pub fn of(engine: Engine, row: &jobs::JobRow) -> Result<Self, EngineError> {
        Ok(Self {
            engine,
            plan: serde_json::from_value(row.plan["notice"].clone())?,
        })
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
            let page = audience(&mut tx, &self.plan, after, job.now()).await?;
            deliver(&mut tx, &self.plan, &page.people, job.now()).await?;
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
            job.progress(0.0, Some(&processed.to_string())).await?;
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
