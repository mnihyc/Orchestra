//! Bounded upload history over the existing indexed audit facts, without rewriting records.

/// Project a stored upload's preview/window before its JSON reaches the application.
/// Arguments are trusted SQL literals/placeholders, never a submitted fragment.
macro_rules! details {
    ($offset:literal, $limit:literal) => {
        concat!(
            "CASE WHEN jsonb_typeof(a.details #> '{upload,files}')='array' THEN ",
            "jsonb_set(a.details,'{upload}',(a.details->'upload') || jsonb_build_object(",
            "'file_count',COALESCE((a.details #>> '{upload,file_count}')::bigint,jsonb_array_length(a.details #> '{upload,files}')),",
            "'files_paged',true,'files',COALESCE((SELECT jsonb_agg((a.details #> '{upload,files}')->i ORDER BY i) ",
            "FROM generate_series(", $offset, ",LEAST(jsonb_array_length(a.details #> '{upload,files}')-1,(", $offset, ")+(", $limit, ")-1)) AS i), '[]'::jsonb))) ",
            "ELSE a.details END"
        )
    };
}
pub(crate) use details;

use crate::{
    Conn, DbError,
    queries::hierarchy::{AuditCursor, AuditRow},
};
use orch_types::Id;

/// Native upload page failures retain their ordinary transport classification.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No matching actual upload summary belongs to this environment.
    #[error("the upload history is unavailable")]
    Missing,
    /// The cursor belongs to another native page shape.
    #[error("this cursor is not for this upload page")]
    Cursor,
    /// The existing stored selector failed.
    #[error(transparent)]
    Db(#[from] DbError),
}
/// The actual stored records and the native legacy nested-window continuation.
pub struct Page {
    /// Actual audit identities, never synthesized file UUIDs.
    pub rows: Vec<AuditRow>,
    /// The next legacy array ordinal; new per-file rows keep normal audit cursors.
    pub legacy_next: Option<i64>,
}

/// Page actual indexed file records, or a bounded window of an immutable legacy grouped record.
pub async fn page(
    conn: &mut Conn,
    environment: Id,
    line: Id,
    after: Option<AuditCursor>,
    legacy_offset: Option<i64>,
    take: i64,
) -> Result<Page, Error> {
    let (batch, actor_kind, actor_id, paged, total): (String,String,Option<Id>,bool,i64) = sqlx::query_as(
        "SELECT details #>> '{upload,id}',actor_kind,actor_id,
            COALESCE((details #>> '{upload,files_paged}')::boolean,false),
            COALESCE((details #>> '{upload,file_count}')::bigint,jsonb_array_length(details #> '{upload,files}')::bigint)
           FROM audit_log WHERE id=$1 AND environment_id=$2 AND action='workspace.file_written'
             AND jsonb_typeof(details #> '{upload,id}')='string'
             AND jsonb_typeof(details #> '{upload,files}')='array'"
    ).bind(line).bind(environment).fetch_optional(&mut *conn).await.map_err(DbError::from)?.ok_or(Error::Missing)?;
    if paged {
        if legacy_offset.is_some() {
            return Err(Error::Cursor);
        }
        let rows = sqlx::query_as(
            "SELECT a.id,a.at,a.actor_kind,a.actor_id,a.action,a.history,a.override,a.object_kind,a.object_id,a.node_id,a.summary,
              a.details || jsonb_build_object('upload',jsonb_build_object(
                'environment_id',a.environment_id,
                'folder',COALESCE(a.details->>'upload_folder',rtrim(regexp_replace(a.details->>'path','[^/]+$',''),'/'),''),
                'files',jsonb_build_array(f.file),'file_count',1,'files_paged',false,
                'bytes',f.file->'received','replaced',CASE WHEN f.file->>'replaced'='true' THEN 1 ELSE 0 END,
                'outcome',f.file->'outcome')) AS details,host(a.ip) AS ip
              FROM audit_log a CROSS JOIN LATERAL (SELECT COALESCE(a.details->'upload_file',jsonb_build_object(
                'name',regexp_replace(a.details->>'path','^.*/',''),'path',a.details->'path',
                'size',a.details->'size','received',a.details->'size','outcome','complete',
                'replaced',COALESCE(a.details->'replaced','false'::jsonb))) AS file) f
             WHERE a.environment_id=$1 AND a.actor_kind=$3 AND a.actor_id IS NOT DISTINCT FROM $4::uuid
               AND a.action='workspace.file_written' AND a.details->>'upload_batch'=$2
               AND (a.details ? 'path' OR a.details ? 'upload_file') AND a.id<$7
               AND ($5::uuid IS NULL OR a.id<$5)
             ORDER BY a.id DESC LIMIT $6"
        ).bind(environment).bind(batch).bind(actor_kind).bind(actor_id)
            .bind(after.map(|cursor| cursor.id)).bind(take)
            .bind(line)
            .fetch_all(conn).await.map_err(DbError::from)?;
        Ok(Page {
            rows,
            legacy_next: None,
        })
    } else {
        if after.is_some() {
            return Err(Error::Cursor);
        }
        let offset = legacy_offset.unwrap_or(0);
        let count = take.saturating_sub(1).max(1);
        if offset < 0 {
            return Err(Error::Cursor);
        }
        let mut row: AuditRow = sqlx::query_as(concat!(
            "SELECT a.id,a.at,a.actor_kind,a.actor_id,a.action,a.history,a.override,a.object_kind,a.object_id,a.node_id,a.summary,",
            details!("$3::integer", "$4::integer"),
            " AS details,host(a.ip) AS ip FROM audit_log a WHERE a.id=$1 AND a.environment_id=$2"
        )).bind(line).bind(environment).bind(i32::try_from(offset).map_err(|_| Error::Cursor)?)
            .bind(i32::try_from(count).map_err(|_| Error::Cursor)?)
            .fetch_one(conn).await.map_err(DbError::from)?;
        let next = offset.saturating_add(count);
        let legacy_next = (next < total).then_some(next);
        if let Some(next) = legacy_next {
            row.details["upload_file_cursor"] = serde_json::json!(format!("upload:{next}"));
        }
        Ok(Page {
            rows: vec![row],
            legacy_next,
        })
    }
}
