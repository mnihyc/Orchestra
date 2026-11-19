//! Append-only audit writes, kept for good, with the rows that are lines of a thing's history and the mark of a override.
//!
//! A row is written in the transaction of the change it records, so that
//! neither exists without the other. Rows are never updated or deleted.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};
use std::cell::Cell;

pub mod emitters;
pub use crate::{audit_action as action, audit_name as name};

tokio::task_local! { static OPENING: Id; static OPENING_SESSION: Option<Id>; }

/// A browser's navigation nonce deduplicates reads of one view, never grants access.
pub async fn scope_opening<T>(opening: Id, work: impl std::future::Future<Output = T>) -> T {
    OPENING.scope(opening, work).await
}

/// The HTTP reader's browser session, for the same audit across chat/page readers.
pub async fn scope_browser_opening<T>(
    session: Option<Id>,
    opening: Id,
    work: impl std::future::Future<Output = T>,
) -> T {
    OPENING_SESSION
        .scope(session, scope_opening(opening, work))
        .await
}

#[must_use]
pub fn opening_session() -> Option<Id> {
    OPENING_SESSION.try_with(|session| *session).ok().flatten()
}

/// Readers without a shared navigation nonce are distinct openings.
#[must_use]
pub fn current_opening() -> Id {
    OPENING
        .try_with(|id| *id)
        .unwrap_or_else(|_| orch_types::new_id())
}

/// One logical opening of an object, shared by its HTTP and live readers.
#[derive(Debug, Clone, PartialEq)]
pub struct Opening<'a> {
    /// The person opening it.
    pub user: Id,
    /// Their browser session.
    pub session: Id,
    /// The navigation nonce, shared by all readers of this view.
    pub nonce: Id,
    /// The kind of object.
    pub kind: &'a str,
    /// The object opened.
    pub object: Id,
    /// The place whose authority made it readable.
    pub place: Json,
    /// The hierarchy node the audit concerns, when one exists.
    pub node: Option<Id>,
}

/// Records one logical private opening, in the caller's transaction. The narrow
/// advisory lock and partial index serialize only this actor/session/object/nonce;
/// a second request or feed of that same view does not create a second audit row.
pub async fn opened(
    conn: &mut Conn,
    record: Opening<'_>,
    origin: &Origin,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let Opening {
        user,
        session,
        nonce: opening,
        kind,
        object,
        place,
        node,
    } = record;
    crate::queries::chats::lock_named(
        conn,
        &format!("private.opening:{user}:{session}:{kind}:{object}:{opening}"),
    )
    .await?;
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM audit_log WHERE actor_kind='user' AND actor_id=$1
          AND object_id=$2 AND object_kind=$3 AND details->>'session_id'=$4
          AND details->>'opening_id'=$5
          AND action IN ('admin.opened','chat.inspected','profile.inspected'))",
    )
    .bind(user)
    .bind(object)
    .bind(kind)
    .bind(session.to_string())
    .bind(opening.to_string())
    .fetch_one(&mut *conn)
    .await?;
    if exists {
        return Ok(false);
    }
    let action = match kind {
        "chat" => action!(orch_access_contract::audit::AuditAction::ChatInspected).as_str(),
        "user" => action!(orch_access_contract::audit::AuditAction::ProfileInspected).as_str(),
        _ => action!(orch_access_contract::audit::AuditAction::AdminOpened).as_str(),
    };
    let mut entry = Entry::new(
        Actor::User(user),
        action,
        kind,
        Some(object),
        "opened a thing through administrative access",
    );
    entry.node_id = node;
    entry.details =
        serde_json::json!({ "place": place, "session_id": session, "opening_id": opening });
    append(conn, &entry, origin, at).await?;
    Ok(true)
}

/// Who did something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    /// A person, or a service identity.
    User(Id),
    /// The system itself: a schedule, a start.
    System,
    /// The agent of a thread.
    Agent(Id),
}

/// One thing that was done.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry<'a> {
    /// A turn's frozen responsibility for an agent act, when explicitly supplied.
    pub for_user_id: Option<Id>,
    /// Who did it.
    pub actor: Actor,
    /// The action, a name of the audit catalogue.
    pub action: &'a str,
    /// Whether it is a line of its object's history, shown to whoever may
    /// view the object.
    pub history: bool,
    /// Whether an admin of the place put themself on a page that did not
    /// give them what they were about to do.
    pub r#override: bool,
    /// The kind of thing it was done to.
    pub object_kind: &'a str,
    /// That thing.
    pub object_id: Option<Id>,
    /// The nearest node of the hierarchy.
    pub node_id: Option<Id>,
    /// The root org.
    pub org_id: Option<Id>,
    /// The environment it was done to or in.
    pub environment_id: Option<Id>,
    /// What happened, in a sentence.
    pub summary: &'a str,
    /// What the action adds.
    pub details: Json,
}

impl<'a> Entry<'a> {
    /// An entry by `actor` of `action` on the `object_kind` `object_id`,
    /// with nothing else said yet.
    #[must_use]
    pub fn new(
        actor: Actor,
        action: &'a str,
        object_kind: &'a str,
        object_id: Option<Id>,
        summary: &'a str,
    ) -> Self {
        Self {
            for_user_id: None,
            actor,
            action,
            history: false,
            r#override: false,
            object_kind,
            object_id,
            node_id: None,
            org_id: None,
            environment_id: None,
            summary,
            details: Json::Object(serde_json::Map::new()),
        }
    }
}

/// Where a request came from, for the rows it causes.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Origin {
    /// The person whose explicit browser-session administrator mode authorized this act.
    /// This is trusted request provenance, never inherited by an agent or another person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_admin: Option<Id>,
    /// The address.
    pub ip: Option<std::net::IpAddr>,
    /// The browser or the client.
    pub user_agent: Option<String>,
    /// The request's identifier.
    pub request_id: Option<String>,
}

tokio::task_local! { static REQUEST_ORIGIN: Origin; }
tokio::task_local! { static AGENT_TURN: (Id,Id); }
tokio::task_local! { static ADMIN_MODE: Cell<Option<Id>>; }

/// Retains the original tool-call turn while its asynchronous action is answered.
pub async fn scope_agent_turn<T>(
    thread: Id,
    turn: Id,
    work: impl std::future::Future<Output = T>,
) -> T {
    AGENT_TURN.scope((thread, turn), work).await
}

/// Keeps a request's safe origin through synchronous actions in the engine.
pub async fn scope_origin<T>(origin: Origin, work: impl std::future::Future<Output = T>) -> T {
    ADMIN_MODE
        .scope(
            Cell::new(origin.as_admin),
            REQUEST_ORIGIN.scope(origin, work),
        )
        .await
}

/// Replaces the initial mode with the session state read under the action guard.
pub fn set_admin_mode(user: Id, enabled: bool) {
    let _ = ADMIN_MODE.try_with(|mode| mode.set(enabled.then_some(user)));
}

/// The current request origin, or honestly unknown outside a request.
#[must_use]
pub fn current_origin() -> Origin {
    let mut origin = REQUEST_ORIGIN.try_with(Clone::clone).unwrap_or_default();
    if let Ok(mode) = ADMIN_MODE.try_with(Cell::get) {
        origin.as_admin = mode;
    }
    if AGENT_TURN.try_with(|_| ()).is_ok() {
        origin.as_admin = None;
    }
    origin
}

/// Appends `entry`, made at `at`, to the audit log.
pub async fn append(
    conn: &mut Conn,
    entry: &Entry<'_>,
    origin: &Origin,
    at: DateTime<Utc>,
) -> Result<Id, DbError> {
    let mut entry = entry.clone();
    crate::queries::trace::snapshot_names(conn, &mut entry).await?;
    crate::queries::trace::deletion_details(conn, &mut entry).await?;
    let inherited = current_origin();
    let origin = Origin {
        as_admin: ADMIN_MODE
            .try_with(Cell::get)
            .unwrap_or(origin.as_admin.or(inherited.as_admin)),
        ip: origin.ip.or(inherited.ip),
        user_agent: origin.user_agent.clone().or(inherited.user_agent),
        request_id: origin.request_id.clone().or(inherited.request_id),
    };
    let id = orch_types::new_id();
    let (kind, actor_id) = match entry.actor {
        Actor::User(id) => ("user", Some(id)),
        Actor::System => ("system", None),
        Actor::Agent(thread) => ("agent", Some(thread)),
    };
    let facts = crate::queries::trace::facts(
        conn,
        entry.object_kind,
        entry.object_id,
        entry.node_id,
        &entry.details,
    )
    .await?;
    let for_user = if let Actor::Agent(thread) = entry.actor {
        if entry.for_user_id.is_some() {
            entry.for_user_id
        } else {
            let turn = entry
                .details
                .get("turn_id")
                .or_else(|| entry.details.get("turn"))
                .and_then(Json::as_str)
                .and_then(|v| v.parse::<Id>().ok())
                .or_else(|| {
                    AGENT_TURN
                        .try_with(|(actor, turn)| (*actor == thread).then_some(*turn))
                        .ok()
                        .flatten()
                });
            if let Some(turn) = turn {
                sqlx::query_scalar::<_, Option<Id>>(
                    "SELECT audit_for_user_id FROM turns WHERE id=$1 AND thread_id=$2",
                )
                .bind(turn)
                .bind(thread)
                .fetch_optional(&mut *conn)
                .await?
                .flatten()
            } else {
                match sqlx::query_scalar::<_,Option<Id>>("SELECT audit_for_user_id FROM turns WHERE thread_id=$1 AND state IN ('starting','running') ORDER BY ordinal DESC LIMIT 1").bind(thread).fetch_optional(&mut *conn).await? {
                    Some(known)=>known,
                    None=>sqlx::query_scalar("SELECT owner_id FROM nodes WHERE id=$1").bind(thread).fetch_optional(&mut *conn).await?,
                }
            }
        }
    } else {
        None
    };
    sqlx::query(
        "INSERT INTO audit_log (id, at, actor_kind, actor_id, action, history, override, object_kind, object_id,
                                node_id, org_id, environment_id, summary, details, ip, user_agent, request_id,object_name,scope_node_ids,for_user_id,as_admin)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15::inet, $16, $17,$18,$19,$20,$21)
         ON CONFLICT (actor_id,object_kind,object_id,((at AT TIME ZONE 'UTC')::date)) WHERE action='deleted.inspected' DO NOTHING",
    )
    .bind(id)
    .bind(at)
    .bind(kind)
    .bind(actor_id)
    .bind(entry.action)
    .bind(entry.history)
    .bind(entry.r#override)
    .bind(entry.object_kind)
    .bind(entry.object_id)
    .bind(facts.node.or(entry.node_id))
    .bind(facts.org.or(entry.org_id))
    .bind(entry.environment_id)
    .bind(entry.summary)
    .bind(&entry.details)
    .bind(origin.ip.map(|ip| ip.to_string()))
    .bind(origin.user_agent.as_deref())
    .bind(origin.request_id.as_deref())
    .bind(facts.name.as_deref())
    .bind(&facts.scope)
    .bind(for_user)
    .bind(matches!(entry.actor, Actor::User(user) if origin.as_admin == Some(user)) && AGENT_TURN.try_with(|_| ()).is_err())
    .execute(&mut *conn)
    .await?;
    crate::queries::trace::changed(conn, &entry, &facts, at).await?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_testkit::{clock, database::TestDb};

    #[tokio::test]
    async fn a_row_is_kept_only_if_the_change_it_records_is() {
        let test = TestDb::fresh().await;
        let db = crate::Db::from_pool(test.pool.clone());
        let origin = Origin {
            as_admin: None,
            ip: Some("203.0.113.9".parse().unwrap()),
            user_agent: None,
            request_id: Some("r1".into()),
        };

        let mut tx = db.begin().await.unwrap();
        append(
            &mut tx,
            &Entry::new(
                Actor::System,
                "settings.updated",
                "setting",
                None,
                "a change that is undone",
            ),
            &origin,
            clock::start(),
        )
        .await
        .unwrap();
        drop(tx);

        let mut tx = db.begin().await.unwrap();
        let mut kept = Entry::new(
            Actor::System,
            "settings.updated",
            "setting",
            None,
            "a change that is kept",
        );
        kept.history = true;
        let id = append(&mut tx, &kept, &origin, clock::start())
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let rows: Vec<(Id, String, bool, String)> =
            sqlx::query_as("SELECT id, summary, history, host(ip) FROM audit_log")
                .fetch_all(db.pool())
                .await
                .unwrap();
        assert_eq!(
            rows,
            vec![(
                id,
                "a change that is kept".to_owned(),
                true,
                "203.0.113.9".to_owned()
            )]
        );
    }
}
