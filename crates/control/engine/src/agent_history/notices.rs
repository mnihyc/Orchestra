//! Recovery handoffs stay with their native recipient and become history after delivery.

use super::*;
use crate::sessions::StartSnapshot;
use crate::unit::Unit;
use orch_db::{Conn, queries::threads::ThreadRow};
use orch_types::Json;
use serde_json::json;

/// The writer's recipient segment is also its durable notice identity.
const PREFIX: &str = "agent-history:";

fn recipient(notice: &Json) -> Option<Id> {
    notice["id"].as_str()?.strip_prefix(PREFIX)?.parse().ok()
}

fn ours(notice: &Json) -> bool {
    notice["id"]
        .as_str()
        .is_some_and(|id| id.starts_with(PREFIX))
}

impl Engine {
    /// Only the same native lane or its physical aliases receive a recovery handoff.
    pub(crate) async fn scope_agent_history_notices(
        &self,
        snapshot: &mut StartSnapshot,
    ) -> Result<(), EngineError> {
        let mut conn = self.db().acquire().await?;
        let mut notices = match threads::notices::delivery_positions(
            &mut conn,
            snapshot.thread.node_id,
            snapshot.turn.id,
        )
        .await?
        {
            Some(positions) => {
                threads::notices::delivery_page(&mut conn, snapshot.thread.node_id, &positions)
                    .await?
            }
            None => {
                threads::notices::for_segment(
                    &mut conn,
                    snapshot.thread.node_id,
                    snapshot.segment.id,
                )
                .await?
            }
        };
        if notices.iter().any(|row| ours(&row.notice)) {
            let recipients: Vec<_> = notices
                .iter()
                .filter_map(|row| recipient(&row.notice))
                .collect();
            let allowed = rows::notice_receivers(
                &mut conn,
                snapshot.thread.node_id,
                snapshot.segment.id,
                &recipients,
            )
            .await?;
            notices.retain(|row| {
                !ours(&row.notice) || recipient(&row.notice).is_some_and(|id| allowed.contains(&id))
            });
        }
        snapshot.pending_notice_positions = notices.iter().map(|row| row.position).collect();
        snapshot.thread.pending_notices = json!(
            notices
                .into_iter()
                .map(|row| row.notice)
                .collect::<Vec<_>>()
        );
        Ok(())
    }

    /// An unsaid file handoff means native reuse cannot yet carry the indexed conversation.
    pub(crate) async fn agent_history_handoff_pending(
        conn: &mut Conn,
        thread: &ThreadRow,
        segment: Id,
    ) -> Result<bool, EngineError> {
        let notices = threads::notices::for_segment(conn, thread.node_id, segment).await?;
        let recipients: Vec<_> = notices
            .iter()
            .filter_map(|notice| recipient(&notice.notice))
            .collect();
        Ok(
            !rows::notice_receivers(conn, thread.node_id, segment, &recipients)
                .await?
                .is_empty(),
        )
    }

    /// A file handoff's accepted input records its real outcome and releases temporary pins.
    pub(crate) async fn complete_agent_history_delivery(
        &self,
        snapshot: &StartSnapshot,
    ) -> Result<(), EngineError> {
        let recipients: BTreeSet<_> = snapshot
            .thread
            .pending_notices
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(recipient)
            .collect();
        if recipients.is_empty() {
            return Ok(());
        }
        let mut tx = self.db().begin().await?;
        let finished = {
            let mut unit = Unit::new(&mut tx, self.inner.clock.now());
            threads::lock(unit.conn, snapshot.thread.node_id).await?;
            for recipient in recipients {
                self.complete_agent_history_recovery(&mut unit, snapshot, recipient, true)
                    .await?;
            }
            unit.finish().await?
        };
        self.commit(tx, finished).await?;
        Ok(())
    }
}
