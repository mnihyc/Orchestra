//! What Orch supplied to a model, as it was sent (`model_inputs`): written before the sending, settled after it.
//!
//! One record for each delivery, under the reference that every attempt at that delivery shares (a turn, a steered
//! input, a session's opening): an attempt made again writes what it sends over what the earlier one held. The blocks
//! are the exact texts, in the order they were given.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json, Seq};

/// A record to write before what it holds is sent.
#[derive(Debug, Clone)]
pub struct NewModelInput<'a> {
    /// Its identifier, where the delivery has none yet.
    pub id: Id,
    /// The node it is given for.
    pub node: Id,
    /// The segment whose session it goes to.
    pub segment: Option<Id>,
    /// The turn it starts or goes into.
    pub turn: Option<Id>,
    /// The input whose words it carries.
    pub input: Option<Id>,
    /// `session`, `input`, `tool` or `summary`.
    pub purpose: &'a str,
    /// The delivery it belongs to.
    pub reference: &'a str,
    /// Its blocks: `[{ key, source, text }]`.
    pub blocks: &'a Json,
    /// The sequence of the thread now.
    pub seq: Seq,
}

/// A record as it is kept.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct ModelInputRow {
    /// Its identifier.
    pub id: Id,
    /// The node it was given for.
    pub node_id: Id,
    /// The segment whose session it went to.
    pub segment_id: Option<Id>,
    /// The turn it started or went into.
    pub turn_id: Option<Id>,
    /// The input whose words it carried.
    pub input_id: Option<Id>,
    /// The item that stands for it.
    pub item_id: Option<Id>,
    /// `session`, `input`, `tool` or `summary`.
    pub purpose: String,
    /// The delivery it belongs to.
    pub reference: String,
    /// Its blocks.
    pub blocks: Json,
    /// `prepared`, `sent`, `failed` or `uncertain`.
    pub state: String,
    /// When it was written.
    pub created_at: DateTime<Utc>,
    /// When the harness took it.
    pub sent_at: Option<DateTime<Utc>>,
    /// The sequence of the thread when it was written.
    pub seq_created: Seq,
    /// The sequence of the thread when it last changed.
    pub seq_updated: Seq,
}

/// Writes a record before what it holds is sent: a new one, or the one an earlier attempt at the same delivery wrote,
/// which now holds what this attempt sends. The record's identifier is returned.
pub async fn prepare(conn: &mut Conn, row: &NewModelInput<'_>) -> Result<Id, DbError> {
    Ok(sqlx::query_scalar(
        "INSERT INTO model_inputs (id, node_id, segment_id, turn_id, input_id, purpose, reference, blocks, state,
                                   seq_created, seq_updated)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'prepared', $9, $9)
         ON CONFLICT (node_id, purpose, reference) DO UPDATE
            SET blocks = EXCLUDED.blocks, state = 'prepared', sent_at = NULL, segment_id = EXCLUDED.segment_id,
                seq_updated = EXCLUDED.seq_updated
         RETURNING id",
    )
    .bind(row.id)
    .bind(row.node)
    .bind(row.segment)
    .bind(row.turn)
    .bind(row.input)
    .bind(row.purpose)
    .bind(row.reference)
    .bind(row.blocks)
    .bind(row.seq)
    .fetch_one(conn)
    .await?)
}

/// Settles how the sending of a record ended: `sent` (taken at `at`), `failed` or `uncertain`.
pub async fn settle(
    conn: &mut Conn,
    id: Id,
    state: &str,
    at: Option<DateTime<Utc>>,
    seq: Seq,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE model_inputs SET state = $2, sent_at = COALESCE($3, sent_at), seq_updated = $4 WHERE id = $1",
    )
    .bind(id)
    .bind(state)
    .bind(at)
    .bind(seq)
    .execute(conn)
    .await?;
    Ok(())
}

/// Which records of a node to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelInputFilter {
    /// One record.
    pub id: Option<Id>,
    /// The records of one turn.
    pub turn: Option<Id>,
    /// Those written before this sequence.
    pub before: Option<Seq>,
    /// Those written after this sequence.
    pub after: Option<Seq>,
    /// How many at most.
    pub limit: i64,
}

/// The records of a node, the newest first.
pub async fn list(
    conn: &mut Conn,
    node: Id,
    filter: &ModelInputFilter,
) -> Result<Vec<ModelInputRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, node_id, segment_id, turn_id, input_id, item_id, purpose, reference, blocks, state, created_at,
                sent_at, seq_created, seq_updated
           FROM model_inputs
          WHERE node_id = $1
            AND ($2::uuid IS NULL OR id = $2)
            AND ($3::uuid IS NULL OR turn_id = $3)
            AND ($4::bigint IS NULL OR seq_created < $4)
            AND ($5::bigint IS NULL OR seq_created > $5)
          ORDER BY seq_created DESC, id DESC
          LIMIT $6",
    )
    .bind(node)
    .bind(filter.id)
    .bind(filter.turn)
    .bind(filter.before)
    .bind(filter.after)
    .bind(filter.limit)
    .fetch_all(conn)
    .await?)
}

/// Document identities protecting model inputs at this node, without names or content.
pub async fn input_documents(conn: &mut Conn, node: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar("SELECT DISTINCT d.document_id FROM document_model_inputs d JOIN model_inputs m ON m.id=d.model_input_id WHERE m.node_id=$1")
        .bind(node).fetch_all(conn).await?)
}

/// The captured document protecting a model-input record, where one exists.
pub async fn input_document(conn: &mut Conn, record: Id) -> Result<Option<Id>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT document_id FROM document_model_inputs WHERE model_input_id=$1")
            .bind(record)
            .fetch_optional(conn)
            .await?,
    )
}

/// Records whose document is readable, plus ordinary node records, with filtering before pagination.
pub async fn list_for_documents(
    conn: &mut Conn,
    node: Id,
    filter: &ModelInputFilter,
    documents: &[Id],
) -> Result<Vec<ModelInputRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT m.id,m.node_id,m.segment_id,m.turn_id,m.input_id,m.item_id,m.purpose,m.reference,m.blocks,m.state,m.created_at,m.sent_at,m.seq_created,m.seq_updated
         FROM model_inputs m LEFT JOIN document_model_inputs d ON d.model_input_id=m.id
         WHERE m.node_id=$1 AND ($2::uuid IS NULL OR m.id=$2) AND ($3::uuid IS NULL OR m.turn_id=$3)
           AND ($4::bigint IS NULL OR m.seq_created<$4) AND ($5::bigint IS NULL OR m.seq_created>$5)
           AND (d.model_input_id IS NULL OR d.document_id=ANY($7))
         ORDER BY m.seq_created DESC,m.id DESC LIMIT $6")
        .bind(node).bind(filter.id).bind(filter.turn).bind(filter.before).bind(filter.after).bind(filter.limit).bind(documents)
        .fetch_all(conn).await?)
}
