//! What Orch supplies to a model, recorded before it is sent and settled once its sending ends: a session's opening
//! text and each input's text (a turn's start, a steer), block by block, as `model_inputs` keeps it.
//!
//! The record is written in a transaction of its own before the command that carries the text goes, and the command
//! is not sent when the record cannot be written: a model is never given text that nobody can read afterwards. Every
//! attempt at one delivery shares its reference (the turn, the steered input, the session's opening), so an attempt
//! made again writes what it sends over what the earlier one held. When the sending ends the record says how: taken,
//! refused, or not known.

use crate::engine::Engine;
use crate::error::EngineError;
use crate::unit::Unit;
use orch_db::queries::model_inputs::{self as rows, NewModelInput};
use orch_db::queries::threads;
use orch_types::{Id, Json, new_id};
use serde_json::json;

/// One block of the text a model is given: what it is (its key in `agent.messages`, `input`, `standing`), where it
/// comes from, and the text exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelBlock {
    /// What it is.
    pub key: String,
    /// Where it comes from and what it is for.
    pub source: &'static str,
    /// The text.
    pub text: String,
}

impl ModelBlock {
    /// A block.
    #[must_use]
    pub fn new(key: impl Into<String>, source: &'static str, text: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            source,
            text: text.into(),
        }
    }
}

/// The blocks as the record keeps them.
#[must_use]
pub fn blocks_json(blocks: &[ModelBlock]) -> Json {
    Json::Array(
        blocks
            .iter()
            .map(|block| json!({ "key": block.key, "source": block.source, "text": block.text }))
            .collect(),
    )
}

/// The text the blocks make together, as it is sent.
#[must_use]
pub fn joined(blocks: &[ModelBlock]) -> String {
    blocks
        .iter()
        .map(|block| block.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// What a record is written for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Delivery<'a> {
    /// The thread.
    pub thread: Id,
    /// The segment whose session it goes to.
    pub segment: Id,
    /// The turn it starts or goes into.
    pub turn: Option<Id>,
    /// The input whose words it carries.
    pub input: Option<Id>,
    /// `session` or `input`.
    pub purpose: &'a str,
    /// What every attempt at it shares.
    pub reference: &'a str,
}

/// How the sending of a record ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sent {
    /// The harness took it.
    Taken,
    /// The harness refused it, or it could not be sent.
    Refused,
    /// No answer came: whether it arrived is not known.
    Unknown,
}

impl Engine {
    /// Bounded inline tails with the exact full bytes in the ordinary content store.
    async fn stored_model_blocks(
        &self,
        conn: &mut orch_db::Conn,
        blocks: &[ModelBlock],
    ) -> Result<Json, EngineError> {
        let mut kept = Vec::new();
        for block in blocks {
            let mut value = json!({"key":block.key,"source":block.source,"text":block.text});
            if block.text.len()
                > usize::try_from(orch_api_contract::threads::OUTPUT_INLINE_MAX_BYTES)
                    .unwrap_or(usize::MAX)
            {
                let full = self
                    .put_manifest(conn, block.text.as_bytes().to_vec())
                    .await?;
                let output = orch_api_contract::threads::OutputRef::of(&block.text);
                value["text"] = json!(output.inline);
                value["blob"] = json!(full.hash);
                value["total_len"] = json!(full.size);
                value["truncated"] = json!(true);
            }
            kept.push(value);
        }
        Ok(Json::Array(kept))
    }
    /// Records an auxiliary prompt at a node, which need not have a thread or native segment.
    pub(crate) async fn record_node_model_input(
        &self,
        node: Id,
        purpose: &str,
        reference: &str,
        blocks: &[ModelBlock],
    ) -> Result<Id, EngineError> {
        self.record_model_input_at(node, None, purpose, reference, blocks)
            .await
    }

    /// Records a document's model prompt with its read protection before anything is sent.
    pub(crate) async fn record_document_model_input(
        &self,
        node: Id,
        document: Id,
        reference: &str,
        blocks: &[ModelBlock],
    ) -> Result<Id, EngineError> {
        self.record_model_input_at(node, Some(document), "summary", reference, blocks)
            .await
    }

    async fn record_model_input_at(
        &self,
        node: Id,
        document: Option<Id>,
        purpose: &str,
        reference: &str,
        blocks: &[ModelBlock],
    ) -> Result<Id, EngineError> {
        // Large bodies may write the file store. Prepare those bytes before the record's transaction.
        let blocks = {
            let mut conn = self.inner.db.acquire().await?;
            self.stored_model_blocks(&mut conn, blocks).await?
        };
        let mut tx = self.inner.db.begin().await?;
        let exists: Option<Id> = sqlx::query_scalar(
            "SELECT id FROM nodes WHERE id=$1 AND deleted_at IS NULL FOR UPDATE",
        )
        .bind(node)
        .fetch_optional(&mut *tx)
        .await
        .map_err(orch_db::DbError::from)?;
        if exists.is_none() {
            return Err(EngineError::unreadable(
                "the node is gone: its prompt was not sent",
            ));
        }
        let thread_seq = orch_db::sequences::next(&mut tx, node).await?;
        let seq=match thread_seq {Some(seq)=>seq,None=>sqlx::query_scalar::<_,i64>("SELECT GREATEST(COALESCE(max(seq_created),0),COALESCE(max(seq_updated),0))+1 FROM model_inputs WHERE node_id=$1").bind(node).fetch_one(&mut *tx).await.map_err(orch_db::DbError::from)?};
        let id = rows::prepare(
            &mut tx,
            &NewModelInput {
                id: new_id(),
                node,
                segment: None,
                turn: None,
                input: None,
                purpose,
                reference,
                blocks: &blocks,
                seq,
            },
        )
        .await?;
        if let Some(document) = document {
            sqlx::query("INSERT INTO document_model_inputs (model_input_id,document_id) VALUES ($1,$2) ON CONFLICT DO NOTHING")
                .bind(id).bind(document).execute(&mut *tx).await.map_err(orch_db::DbError::from)?;
            let bound: Id = sqlx::query_scalar(
                "SELECT document_id FROM document_model_inputs WHERE model_input_id=$1",
            )
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .map_err(orch_db::DbError::from)?;
            if bound != document {
                return Err(EngineError::unreadable(
                    "the model input already belongs to another document",
                ));
            }
        }
        if thread_seq.is_some() {
            orch_db::sequences::notify(&mut tx, node, seq).await?;
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(id)
    }

    /// A changed session opens before its records are attached. At commit they name the actual fork
    /// segment, or the newly inserted switch segment, and get the same marker as an ordinary turn.
    pub(crate) async fn attach_model_inputs(
        &self,
        unit: &mut Unit<'_>,
        thread: Id,
        segment: Id,
        records: &[Id],
    ) -> Result<(), EngineError> {
        for id in records {
            let purpose: Option<String> =
                sqlx::query_scalar("SELECT purpose FROM model_inputs WHERE id=$1 AND node_id=$2")
                    .bind(id)
                    .bind(thread)
                    .fetch_optional(&mut *unit.conn)
                    .await
                    .map_err(orch_db::DbError::from)?;
            let Some(purpose) = purpose else {
                continue;
            };
            let seq = unit.stamp(thread).await?;
            sqlx::query("UPDATE model_inputs SET segment_id=$2,seq_updated=$3 WHERE id=$1")
                .bind(id)
                .bind(segment)
                .bind(seq)
                .execute(&mut *unit.conn)
                .await
                .map_err(orch_db::DbError::from)?;
            if purpose == "session" {
                let key = format!("model-input:{id}");
                crate::ingest::add_marker(unit, thread, segment, None, &json!({"marker":"orch_context","data":{"model_input_id":id,"purpose":purpose}}), &key).await?;
                let seq = unit.stamp(thread).await?;
                sqlx::query("UPDATE items SET model_input_id=$3,seq_updated=$4 WHERE thread_id=$1 AND native_ref=$2")
                    .bind(thread).bind(&key).bind(id).bind(seq).execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
                sqlx::query("UPDATE model_inputs SET item_id=(SELECT id FROM items WHERE thread_id=$2 AND native_ref=$3),seq_updated=$4 WHERE id=$1")
                    .bind(id).bind(thread).bind(&key).bind(seq).execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
            }
        }
        Ok(())
    }

    /// Auxiliary prompt state, without altering a thread's native session or starting a turn.
    pub(crate) async fn settle_node_model_input(&self, node: Id, id: Id, sent: Sent) {
        let result = async {
            let mut tx = self.inner.db.begin().await?;
            sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR UPDATE")
                .bind(node)
                .fetch_optional(&mut *tx)
                .await
                .map_err(orch_db::DbError::from)?;
            let thread_seq = orch_db::sequences::next(&mut tx, node).await?;
            let seq = match thread_seq {
                Some(seq) => seq,
                None => sqlx::query_scalar::<_, i64>(
                    "SELECT COALESCE(max(seq_updated),0)+1 FROM model_inputs WHERE node_id=$1",
                )
                .bind(node)
                .fetch_one(&mut *tx)
                .await
                .map_err(orch_db::DbError::from)?,
            };
            let (state, at) = match sent {
                Sent::Taken => ("sent", Some(self.inner.clock.now())),
                Sent::Refused => ("failed", None),
                Sent::Unknown => ("uncertain", None),
            };
            rows::settle(&mut tx, id, state, at, seq).await?;
            if thread_seq.is_some() {
                orch_db::sequences::notify(&mut tx, node, seq).await?;
            }
            tx.commit().await.map_err(orch_db::DbError::from)?;
            Ok::<_, EngineError>(())
        }
        .await;
        if let Err(error) = result {
            tracing::warn!(%error,%node,%id,"an auxiliary prompt's sending state could not be kept");
        }
    }

    /// Writes the record of what goes to a model before it is sent. An error stops the sending: nothing is sent that
    /// nobody could read afterwards.
    pub(crate) async fn record_model_input(
        &self,
        delivery: &Delivery<'_>,
        blocks: &[ModelBlock],
    ) -> Result<Id, EngineError> {
        let mut tx = self.inner.db.begin().await?;
        let blocks = self.stored_model_blocks(&mut tx, blocks).await?;
        let (id, finished) = {
            let mut unit = Unit::new(&mut tx, self.inner.clock.now());
            if threads::lock(&mut *unit.conn, delivery.thread)
                .await?
                .is_none()
            {
                return Err(EngineError::unreadable(
                    "the thread is gone: what would have been sent to its model is not",
                ));
            }
            let seq = unit.stamp(delivery.thread).await?;
            let id = rows::prepare(
                &mut *unit.conn,
                &NewModelInput {
                    id: new_id(),
                    node: delivery.thread,
                    segment: Some(delivery.segment),
                    turn: delivery.turn,
                    input: delivery.input,
                    purpose: delivery.purpose,
                    reference: delivery.reference,
                    blocks: &blocks,
                    seq,
                },
            )
            .await?;
            if matches!(delivery.purpose, "session" | "input") {
                let key = format!("model-input:{id}");
                crate::ingest::add_marker(&mut unit,delivery.thread,delivery.segment,delivery.turn,&json!({"marker":"orch_context","data":{"model_input_id":id,"purpose":delivery.purpose}}),&key).await?;
                let seq = unit.stamp(delivery.thread).await?;
                sqlx::query("UPDATE items SET model_input_id=$3,seq_updated=$4 WHERE thread_id=$1 AND native_ref=$2")
                    .bind(delivery.thread).bind(&key).bind(id).bind(seq).execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
                sqlx::query("UPDATE model_inputs SET item_id=(SELECT id FROM items WHERE thread_id=$2 AND native_ref=$3) WHERE id=$1")
                    .bind(id).bind(delivery.thread).bind(&key).execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
                if let Some(input) = delivery.input {
                    sqlx::query("UPDATE items SET model_input_id=$3,seq_updated=$4 WHERE thread_id=$1 AND kind='user_message' AND payload->>'input_id'=$2::text")
                        .bind(delivery.thread).bind(input).bind(id).bind(seq).execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
                }
            }
            (id, unit.finish().await?)
        };
        self.commit(tx, finished).await?;
        Ok(id)
    }

    /// Says how the sending of a record ended. Where this is not reached the record stays `prepared`, which says the
    /// same as not knowing.
    pub(crate) async fn settle_model_input(&self, thread: Id, id: Id, sent: Sent) {
        let settled = async {
            let mut tx = self.inner.db.begin().await?;
            let finished = {
                let mut unit = Unit::new(&mut tx, self.inner.clock.now());
                if threads::lock(&mut *unit.conn, thread).await?.is_none() {
                    return Ok(());
                }
                let seq = unit.stamp(thread).await?;
                let now = unit.now();
                let (state, at) = match sent {
                    Sent::Taken => ("sent", Some(now)),
                    Sent::Refused => ("failed", None),
                    Sent::Unknown => ("uncertain", None),
                };
                rows::settle(&mut *unit.conn, id, state, at, seq).await?;
                unit.finish().await?
            };
            self.commit(tx, finished).await?;
            Ok::<_, EngineError>(())
        };
        if let Err(error) = settled.await {
            tracing::warn!(%error, %thread, record = %id, "how the sending of what a model was given ended could not be kept");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_blocks_are_kept_in_their_order_and_make_the_text_that_is_sent() {
        let blocks = [
            ModelBlock::new("where_you_are", "orch", "<orch>\nWhere you are\n</orch>"),
            ModelBlock::new("input", "input", "Fix the login"),
        ];
        assert_eq!(
            joined(&blocks),
            "<orch>\nWhere you are\n</orch>\n\nFix the login"
        );
        assert_eq!(
            blocks_json(&blocks),
            json!([
                { "key": "where_you_are", "source": "orch", "text": "<orch>\nWhere you are\n</orch>" },
                { "key": "input", "source": "input", "text": "Fix the login" },
            ])
        );
    }
}
