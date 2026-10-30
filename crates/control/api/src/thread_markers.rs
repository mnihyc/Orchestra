//! The lines a person's change of a thread leaves in its history: a new model or effort, another setting of its own, its page, its first owner.
//!
//! Whoever changes a thread through an act that needs a permission on it leaves a line in the
//! thread's history, in the same transaction as the change and naming themself, so that the
//! conversation says who changed what it runs with and when, between the turns it changed. The
//! requests that change a thread's settings and its page write those lines here, each the same way:
//! on the segment under way, announced to the thread's readers with the change, and once: a line is
//! keyed by what it says and the moment of the change, and a request that is retried and changes
//! nothing writes nothing.
//!
//! A line states a few facts and carries no content: the instructions of a thread are never put in
//! one (both sides are null and the line says only that they changed).

use chrono::{DateTime, Utc};
use orch_access_contract::pages::{PageChange, RowChange, Whole};
use orch_db::Conn;
use orch_engine::unit::Unit;
use orch_types::{Id, Json};
use serde_json::json;

use crate::middleware::requests::{ApiError, ApiResult};

/// A line of a thread's history that a person's change writes.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Line {
    /// A new model, a new effort, or both: what the thread runs with from its next turn.
    Model {
        /// The model before.
        from: Option<String>,
        /// The model now.
        to: String,
        /// The effort before.
        effort_from: Option<String>,
        /// The effort now.
        effort_to: Option<String>,
    },
    /// Another of the thread's own settings, or one member of its agent's parameters
    /// (`agent.params.<member>`): what it was and what it is now.
    Setting {
        /// The key, or the key of the parameters and the member.
        key: String,
        /// What it is called.
        label: String,
        /// The value before; null where it was unset, and for the instructions.
        from: Option<Json>,
        /// The value now; null where it is unset now, and for the instructions.
        to: Option<Json>,
    },
    /// Its page was changed: who was added to or removed from what, or that it was made private
    /// or follows its topic again.
    Access {
        /// What changed, in a few words.
        summary: String,
    },
    /// Its first owner changed.
    Owner {
        /// The first owner before.
        from: Id,
        /// The first owner now.
        to: Id,
    },
}

impl Line {
    /// The marker the line is written as.
    const fn marker(&self) -> &'static str {
        match self {
            Self::Model { .. } => "model_changed",
            Self::Setting { .. } => "setting_changed",
            Self::Access { .. } => "access_changed",
            Self::Owner { .. } => "owner_changed",
        }
    }

    /// What the line is about, for the key that keeps it to one: the setting for a change of one.
    fn about(&self) -> &str {
        match self {
            Self::Setting { key, .. } => key.as_str(),
            Self::Model { .. } | Self::Access { .. } | Self::Owner { .. } => "",
        }
    }
}

/// What a change of a page did, in a few words: made private, made to follow its topic again, or
/// how many holders were added, changed and taken out.
fn page_summary(change: &PageChange) -> String {
    match change.whole {
        Some(Whole::Private) => {
            return "made it private: only its owners and whom its page names hold it".to_owned();
        }
        Some(Whole::Reset) => return "made it follow its topic's page again".to_owned(),
        None => {}
    }
    let rows: &[RowChange] = change.rows.as_deref().unwrap_or_default();
    let count = |each: fn(&RowChange) -> usize| rows.iter().map(each).sum::<usize>();
    let owners = change.owners.as_ref();
    let added = change.name.as_ref().map_or(0, |names| {
        names.iter().map(|name| name.holders.len()).sum::<usize>()
    }) + count(|row| row.add.as_ref().map_or(0, Vec::len))
        + owners
            .and_then(|owners| owners.add.as_ref())
            .map_or(0, Vec::len);
    let changed = count(|row| row.update.as_ref().map_or(0, Vec::len))
        + count(|row| usize::from(row.inherit.is_some() || row.locked.is_some()));
    let removed = count(|row| row.remove.as_ref().map_or(0, Vec::len))
        + owners
            .and_then(|owners| owners.remove.as_ref())
            .map_or(0, Vec::len);
    let said: Vec<String> = [
        (added, "added"),
        (changed, "changed"),
        (removed, "taken out"),
    ]
    .into_iter()
    .filter(|(how_many, _)| *how_many > 0)
    .map(|(how_many, what)| format!("{how_many} {what}"))
    .collect();
    if said.is_empty() {
        "changed who holds it".to_owned()
    } else {
        format!("changed who holds it: {}", said.join(", "))
    }
}

/// Whether a change of a page does nothing but name another first owner.
fn only_names_the_first(change: &PageChange) -> bool {
    change.name.is_none()
        && change.rows.is_none()
        && change.whole.is_none()
        && change.owners.as_ref().is_some_and(|owners| {
            owners.first.is_some() && owners.add.is_none() && owners.remove.is_none()
        })
}

/// Writes the lines a change of the page of the thread `thread` leaves: what changed on it, in a
/// few words, and a line of its own where the change made somebody else its first owner
/// (`first_before` is who was first before it).
pub(crate) async fn page_changed(
    conn: &mut Conn,
    now: DateTime<Utc>,
    thread: Id,
    by: Id,
    first_before: Id,
    change: &PageChange,
) -> ApiResult<()> {
    if !only_names_the_first(change) {
        let summary = page_summary(change);
        write(conn, now, thread, by, &Line::Access { summary }).await?;
    }
    let first_after = orch_db::queries::threads::thread(conn, thread)
        .await?
        .map(|row| row.owner_id);
    if let Some(first_after) = first_after.filter(|after| *after != first_before) {
        let line = Line::Owner {
            from: first_before,
            to: first_after,
        };
        write(conn, now, thread, by, &line).await?;
    }
    Ok(())
}

/// The reference of a person as a line of history carries it.
async fn person(conn: &mut Conn, user: Id) -> ApiResult<Json> {
    orch_engine::views::user_ref_json(conn, user)
        .await
        .map_err(ApiError::internal)
}

/// Writes `line` into the history of the thread `thread`, as done by `by` at `now`, in the
/// transaction `conn` belongs to. A thread that has no segment yet has no history to write in, and
/// a node that is not a thread none at all: nothing is written for either.
pub(crate) async fn write(
    conn: &mut Conn,
    now: DateTime<Utc>,
    thread: Id,
    by: Id,
    line: &Line,
) -> ApiResult<()> {
    let mut unit = Unit::new(conn, now);
    let Some(row) = orch_db::queries::threads::thread(unit.conn, thread).await? else {
        return Ok(());
    };
    let Some(segment) = row.current_segment_id else {
        return Ok(());
    };
    let by_ref = person(unit.conn, by).await?;
    let data = match line {
        Line::Model {
            from,
            to,
            effort_from,
            effort_to,
        } => json!({
            "from": from,
            "to": to,
            "effort": { "from": effort_from, "to": effort_to },
            "by": by_ref,
        }),
        Line::Setting {
            key,
            label,
            from,
            to,
        } => json!({ "key": key, "label": label, "from": from, "to": to, "by": by_ref }),
        Line::Access { summary } => json!({ "summary": summary, "by": by_ref }),
        Line::Owner { from, to } => {
            let from = person(unit.conn, *from).await?;
            let to = person(unit.conn, *to).await?;
            json!({ "from": from, "to": to, "by": by_ref })
        }
    };
    let payload = json!({ "marker": line.marker(), "data": data });
    let key = format!(
        "{}:{}:{}",
        line.marker(),
        line.about(),
        now.timestamp_millis()
    );
    orch_engine::ingest::add_marker(&mut unit, thread, segment, None, &payload, &key)
        .await
        .map_err(ApiError::internal)?;
    unit.finish().await.map_err(ApiError::internal)?;
    Ok(())
}
