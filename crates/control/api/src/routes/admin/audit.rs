//! The audit trail of the whole system, read a page at a time with its filters and written out as a streamed file for a span of at most a year.
//!
//! The trail is kept for good and is append-only; this is how an operator reads it and takes a copy.
//! The list and the file are made of the same entries, so that a file holds exactly what the list
//! holds for the same span and filters. Taking a copy is itself audited, and the line is written
//! before the first row is read, so that a copy cannot be taken without leaving it.

use super::{admin_of, invalid, page_size, read_ahead};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Ctx, Registry};
use axum::body::Body;
use axum::response::Response;
use bytes::Bytes;
use chrono::{DateTime, Duration, Utc};
use http::{HeaderValue, StatusCode, header};
use orch_access_contract::audit::AuditAction;
use orch_api_contract::admin::{
    AUDIT_EXPORT_MAX_DAYS, AdminAuditExport, AdminAuditExportQuery, AdminAuditList,
    AdminAuditListQuery, AuditExportFormat,
};
use orch_api_contract::common::Page;
use orch_api_contract::nodes::{AuditActor, AuditEntry};
use orch_db::queries::hierarchy::{AuditCursor, AuditRow};
use orch_db::{Conn, DbError};
use orch_types::Time;
use serde_json::json;

/// How many rows a file takes in with one read. Large enough that a year of a busy system is a few
/// thousand reads, small enough that a chunk is a few hundred kilobytes in memory.
const EXPORT_CHUNK_ROWS: i64 = 500;

/// The columns of a file of comma-separated values, in order.
const CSV_COLUMNS: [&str; 32] = [
    "id",
    "at",
    "actor_kind",
    "actor_id",
    "actor",
    "action",
    "object_kind",
    "object_id",
    "history",
    "override",
    "as_admin",
    "node_id",
    "node",
    "summary",
    "details",
    "ip",
    "kind",
    "for_user_id",
    "for_user",
    "object_name",
    "object_state",
    "deletion_id",
    "event_deletion_id",
    "deleted_at",
    "deleted_by",
    "purge_at",
    "purged_at",
    "can_open",
    "can_inspect",
    "can_restore",
    "request_id",
    "object_lifecycle",
];

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(AdminAuditList, list)
        .on_raw(AdminAuditExport, export);
}

fn cursor_text(row: &AuditRow) -> String {
    format!("{}|{}", row.at.timestamp_micros(), row.id)
}

fn parse_cursor(text: &str) -> ApiResult<AuditCursor> {
    let bad = || invalid("cursor", "is not a cursor of this list");
    let (micros, id) = text.split_once('|').ok_or_else(bad)?;
    Ok(AuditCursor {
        at: micros
            .parse::<i64>()
            .ok()
            .and_then(DateTime::<Utc>::from_timestamp_micros)
            .ok_or_else(bad)?,
        id: id.parse().map_err(|_| bad())?,
    })
}

/// The rows as entries of the trail, with the people and the nodes they name. A system admin is
/// let know every name, so none is held back.
async fn entries(
    ctx: &Ctx,
    conn: &mut Conn,
    rows: &[orch_db::queries::trace::Row],
) -> ApiResult<Vec<AuditEntry>> {
    crate::routes::trace::entries(ctx, conn, rows).await
}

/// The audit trail of the whole system, newest first.
async fn list(ctx: Ctx, query: AdminAuditListQuery, (): ()) -> ApiResult<Page<AuditEntry>> {
    admin_of(&ctx)?;
    let size = page_size(query.limit);
    let after = query.cursor.as_deref().map(parse_cursor).transpose()?;
    let mut conn = ctx.app.db.acquire().await?;
    let filter = orch_db::queries::trace::Filter {
        actor: query.actor,
        include_agents: query.include_agents.unwrap_or(false),
        action: query.action,
        actions: crate::routes::trace::actions(query.kind),
        node: query.node_id,
        from: query.from.map(Time::get),
        to: query.to.map(Time::get),
    };
    let mut rows =
        orch_db::queries::trace::page(&mut conn, &filter, after, read_ahead(size), true).await?;
    let more = rows.len() > size;
    rows.truncate(size);
    Ok(Page {
        next_cursor: more
            .then(|| rows.last().map(|r| cursor_text(&r.base)))
            .flatten(),
        items: entries(&ctx, &mut conn, &rows).await?,
    })
}

/// A cell of a file of comma-separated values. One that begins with a character by which a
/// spreadsheet reads a formula is written with a quote before it, so that what a person typed
/// somewhere in Orch is never run by whoever opens the file; one with a comma, a quote or a line
/// break in it is put in quotes.
fn csv_cell(text: &str) -> String {
    const FORMULA_STARTS: [char; 6] = ['=', '+', '-', '@', '\t', '\r'];
    let guarded = if text.starts_with(FORMULA_STARTS) {
        format!("'{text}")
    } else {
        text.to_owned()
    };
    if guarded.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", guarded.replace('"', "\"\""))
    } else {
        guarded
    }
}

fn csv_line(cells: &[String]) -> String {
    let mut line = cells
        .iter()
        .map(|cell| csv_cell(cell))
        .collect::<Vec<_>>()
        .join(",");
    line.push_str("\r\n");
    line
}

/// One entry as the cells of [`CSV_COLUMNS`].
fn csv_cells(entry: &AuditEntry) -> Vec<String> {
    let (kind, actor_id, actor) = match &entry.actor {
        AuditActor::User { user } => ("user", Some(user.id), user.username.clone()),
        AuditActor::System => ("system", None, String::new()),
        AuditActor::Agent { thread, .. } => (
            "agent",
            Some(thread.id),
            thread.name.clone().unwrap_or_default(),
        ),
    };
    let responsible = match &entry.actor {
        AuditActor::Agent { for_user, .. } => for_user.as_ref(),
        _ => None,
    };
    let mut cells = vec![
        entry.id.to_string(),
        entry.at.to_string(),
        kind.to_owned(),
        actor_id.map(|id| id.to_string()).unwrap_or_default(),
        actor,
        entry.action.clone(),
        entry.object.kind.clone(),
        entry.object.id.map(|id| id.to_string()).unwrap_or_default(),
        entry.history.to_string(),
        entry.r#override.to_string(),
        entry.as_admin.to_string(),
        entry
            .node
            .as_ref()
            .map(|node| node.id.to_string())
            .unwrap_or_default(),
        entry
            .node
            .as_ref()
            .and_then(|node| node.name.clone())
            .unwrap_or_default(),
        entry.summary.clone(),
        entry.details.to_string(),
        entry.ip.clone().unwrap_or_default(),
    ];
    cells.extend([
        serde_json::to_value(entry.kind)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default(),
        responsible.map(|u| u.id.to_string()).unwrap_or_default(),
        responsible.map(|u| u.username.clone()).unwrap_or_default(),
        entry.object.name.clone().unwrap_or_default(),
        serde_json::to_value(entry.object.state)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default(),
        entry
            .object
            .deletion_id
            .map(|v| v.to_string())
            .unwrap_or_default(),
        entry
            .event_deletion_id
            .map(|v| v.to_string())
            .unwrap_or_default(),
        entry
            .object
            .deleted_at
            .map(|v| v.to_string())
            .unwrap_or_default(),
        entry
            .object
            .deleted_by
            .as_ref()
            .map(|u| u.username.clone())
            .unwrap_or_default(),
        entry
            .object
            .purge_at
            .map(|v| v.to_string())
            .unwrap_or_default(),
        entry
            .object
            .purged_at
            .map(|v| v.to_string())
            .unwrap_or_default(),
        entry.object.can_open.to_string(),
        entry.object.can_inspect.to_string(),
        entry.object.can_restore.to_string(),
        entry.request_id.clone().unwrap_or_default(),
        serde_json::to_string(&entry.object).unwrap_or_default(),
    ]);
    cells
}

/// Where a file stands as it is written.
struct Writing {
    place: bool,
    ctx: Ctx,
    filter: orch_db::queries::trace::Filter,
    snapshot: Option<orch_db::Tx>,
    format: AuditExportFormat,
    after: Option<AuditCursor>,
    started: bool,
    over: bool,
}

impl Writing {
    /// The next chunk of the file, none once everything is written. A failure ends the file: the
    /// status was sent long ago, so the connection is what tells the reader that it is short.
    async fn next(&mut self) -> ApiResult<Option<Bytes>> {
        let mut out = String::new();
        if !self.started {
            self.started = true;
            if self.format == AuditExportFormat::Csv {
                let header: Vec<String> =
                    CSV_COLUMNS.iter().map(|name| (*name).to_owned()).collect();
                out.push_str(&csv_line(&header));
            }
        }
        let (mut authority, guard) = transaction_guard::begin(&self.ctx).await?;
        if !self.place && !guard.user().is_system_admin() {
            return Err(ApiError::new(
                orch_api_contract::common::ErrorCode::AuthzDenied,
                "system administration was withdrawn",
            ));
        }
        if self.place
            && let Some(node) = self.filter.node
        {
            crate::middleware::authorization::require(
                &mut authority,
                guard.principal(),
                node,
                crate::middleware::authorization::exact(
                    orch_access_contract::permissions::Permission::AuditView,
                ),
                orch_authz::Act::Use,
                orch_authz::Policy::default(),
            )
            .await?;
        }
        let mut conn = self
            .snapshot
            .take()
            .ok_or_else(|| ApiError::internal("the audit snapshot ended"))?;
        let rows = orch_db::queries::trace::page(
            &mut conn,
            &self.filter,
            self.after,
            EXPORT_CHUNK_ROWS,
            false,
        )
        .await?;
        if let Some(last) = rows.last() {
            self.after = Some(AuditCursor {
                at: last.base.at,
                id: last.base.id,
            });
        }
        self.over = i64::try_from(rows.len()).is_ok_and(|count| count < EXPORT_CHUNK_ROWS);
        let mut page_ctx = self.ctx.clone();
        page_ctx.caller = crate::middleware::authentication::Caller::User(std::sync::Arc::new(
            guard.user().clone(),
        ));
        page_ctx.now = guard.now().get();
        for entry in entries(&page_ctx, &mut authority, &rows).await? {
            match self.format {
                AuditExportFormat::Csv => out.push_str(&csv_line(&csv_cells(&entry))),
                AuditExportFormat::Jsonl => {
                    out.push_str(&serde_json::to_string(&entry).map_err(ApiError::internal)?);
                    out.push('\n');
                }
            }
        }
        self.snapshot = Some(conn);
        authority.commit().await.map_err(DbError::from)?;
        Ok((!out.is_empty()).then(|| Bytes::from(out)))
    }
}

/// How the file of a span is named.
fn file_name(from: DateTime<Utc>, to: DateTime<Utc>, format: AuditExportFormat) -> String {
    let extension = match format {
        AuditExportFormat::Csv => "csv",
        AuditExportFormat::Jsonl => "jsonl",
    };
    format!(
        "orch-audit-{}-{}.{extension}",
        from.format("%Y%m%d"),
        to.format("%Y%m%d")
    )
}

/// Writes a span of the audit trail out as a file, a chunk at a time, oldest first.
async fn export(ctx: Ctx, query: AdminAuditExportQuery, _body: Body) -> ApiResult<Response> {
    admin_of(&ctx)?;
    export_scope(ctx, query, false).await
}

pub(crate) async fn export_scope(
    mut ctx: Ctx,
    query: AdminAuditExportQuery,
    place: bool,
) -> ApiResult<Response> {
    if !place {
        admin_of(&ctx)?;
    }
    let (from, to) = (query.from.get(), query.to.get());
    if to < from {
        return Err(invalid("to", "must not be before from"));
    }
    if to - from > Duration::days(i64::from(AUDIT_EXPORT_MAX_DAYS)) {
        return Err(invalid(
            "to",
            &format!("must be at most {AUDIT_EXPORT_MAX_DAYS} days after from"),
        ));
    }
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    if !place && !guard.user().is_system_admin() {
        return Err(ApiError::new(
            orch_api_contract::common::ErrorCode::AuthzDenied,
            "system administration was withdrawn",
        ));
    }
    let admin = guard.user().id();
    let who = guard.user().user.display_name.clone();
    let summary = format!("{who} began exporting the audit trail");
    let mut entry = orch_db::audit::Entry::new(
        orch_db::audit::Actor::User(admin),
        orch_db::audit::action!(AuditAction::AuditExported).as_str(),
        if place { "node" } else { "audit" },
        if place { query.node_id } else { None },
        &summary,
    );
    if place {
        entry.node_id = query.node_id;
    }
    entry.details = json!({ "format": query.format, "from": query.from, "to": query.to,"actor":query.actor,"include_agents":query.include_agents.unwrap_or(false),"action":query.action,"kind":query.kind,"node_id":query.node_id,"scope":if place{"place"}else{"system"} });
    orch_db::audit::append(&mut tx, &entry, &crate::sign_in::origin(&ctx), ctx.now).await?;
    tx.commit().await.map_err(DbError::from)?;

    let mut snapshot = ctx.app.db.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *snapshot)
        .await
        .map_err(DbError::from)?;
    sqlx::query("SELECT txid_current_snapshot()")
        .execute(&mut *snapshot)
        .await
        .map_err(DbError::from)?;
    let writing = Writing {
        place,
        ctx: ctx.clone(),
        filter: orch_db::queries::trace::Filter {
            actor: query.actor,
            include_agents: query.include_agents.unwrap_or(false),
            action: query.action.clone(),
            actions: crate::routes::trace::actions(query.kind),
            node: query.node_id,
            from: Some(from),
            to: Some(to),
        },
        snapshot: Some(snapshot),
        format: query.format,
        after: None,
        started: false,
        over: false,
    };
    let stream = futures::stream::unfold(writing, |mut writing| async move {
        if writing.over && writing.started {
            return None;
        }
        match writing.next().await {
            Ok(Some(chunk)) => Some((Ok::<Bytes, std::io::Error>(chunk), writing)),
            Ok(None) => None,
            Err(error) => {
                // Nothing more is read after a failure: the connection ends short.
                writing.over = true;
                writing.started = true;
                Some((Err(std::io::Error::other(error.to_string())), writing))
            }
        }
    });
    let (content_type, disposition) = (
        match query.format {
            AuditExportFormat::Csv => "text/csv; charset=utf-8",
            AuditExportFormat::Jsonl => "application/x-ndjson",
        },
        format!(
            "attachment; filename=\"{}\"",
            file_name(from, to, query.format)
        ),
    );
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    if let Ok(value) = HeaderValue::from_str(&disposition) {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cell_that_a_spreadsheet_would_run_is_written_as_text_and_a_cell_with_a_comma_is_quoted() {
        assert_eq!(csv_cell("plain"), "plain");
        assert_eq!(csv_cell("=HYPERLINK(\"x\")"), "\"'=HYPERLINK(\"\"x\"\")\"");
        assert_eq!(csv_cell("+1"), "'+1");
        assert_eq!(csv_cell("a, b"), "\"a, b\"");
        assert_eq!(csv_cell("two\nlines"), "\"two\nlines\"");
    }

    #[test]
    fn a_file_is_named_by_its_span() {
        let from = "2026-01-02T03:04:05Z".parse::<DateTime<Utc>>().unwrap();
        let to = "2026-02-03T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        assert_eq!(
            file_name(from, to, AuditExportFormat::Csv),
            "orch-audit-20260102-20260203.csv"
        );
    }
}
