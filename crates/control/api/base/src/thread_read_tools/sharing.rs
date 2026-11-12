//! Informed disclosure on the existing native access/share transaction and preview.

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::state::App;

use orch_api_contract::access::OwnerReadDisclosure;

use orch_api_contract::common::ErrorCode;

use orch_db::Conn;

use orch_db::queries::threads;

use orch_engine::tools::read_tools::Audience;

use orch_types::Hash;

use orch_types::Id;

use serde::Serialize;

use std::collections::BTreeMap;
pub const HEADER: &str = "orch-owner-reads-confirmed";

pub struct Before {
    intent: Hash,
    audiences: BTreeMap<Id, Audience>,
    force: bool,
}

impl Before {
    /// Lock the actual affected threads in identity order, including a thread with no prior
    /// result. A first personal result cannot race its first sharing confirmation.
    pub async fn take<T: Serialize>(
        app: &App,
        conn: &mut Conn,
        anchor: Option<Id>,
        intent: &T,
        force: bool,
        inside: bool,
    ) -> ApiResult<Self> {
        let ids = if let Some(anchor) = anchor {
            sqlx::query_scalar("SELECT t.node_id FROM threads t JOIN nodes n ON n.id=t.node_id
                WHERE n.deleted_at IS NULL AND (t.node_id=$1 OR ($2 AND $1=ANY(n.path))) ORDER BY t.node_id FOR UPDATE OF t")
                .bind(anchor).bind(inside).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?
        } else {
            Vec::new()
        };
        Self::capture(app, conn, intent, force, ids).await
    }

    /// A lock on the system's defaults can reach otherwise private threads in every tree.
    pub async fn system<T: Serialize>(app: &App, conn: &mut Conn, intent: &T) -> ApiResult<Self> {
        let ids=sqlx::query_scalar("SELECT t.node_id FROM threads t JOIN nodes n ON n.id=t.node_id WHERE n.deleted_at IS NULL ORDER BY t.node_id FOR UPDATE OF t")
            .fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?;
        Self::capture(app, conn, intent, false, ids).await
    }

    async fn capture<T: Serialize>(
        app: &App,
        conn: &mut Conn,
        intent: &T,
        force: bool,
        ids: Vec<Id>,
    ) -> ApiResult<Self> {
        let intent = Hash::of(&serde_json::to_vec(intent).map_err(ApiError::internal)?);
        let mut audiences = BTreeMap::new();
        for id in ids {
            let thread = threads::thread(conn, id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let audience = orch_engine::tools::read_tools::thread_read_audience(
                conn,
                &thread,
                app.clock.time(),
            )
            .await
            .map_err(ApiError::engine)?;
            audiences.insert(id, audience);
        }
        Ok(Self {
            intent,
            audiences,
            force,
        })
    }

    /// Native producers already made their actual proposed change in this transaction. A
    /// preview rolls it back; a writer requires the same disclosure before committing it.
    pub async fn disclosure(
        &self,
        app: &App,
        conn: &mut Conn,
    ) -> ApiResult<Option<OwnerReadDisclosure>> {
        let mut selected = Vec::new();
        let mut total = 0_i64;
        for (id, before) in &self.audiences {
            let count: i64 = sqlx::query_scalar(
                "SELECT COALESCE(sum(owner_read_results),0)::bigint FROM turns WHERE thread_id=$1",
            )
            .bind(id)
            .fetch_one(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?;
            if count == 0 {
                continue;
            }
            let thread = threads::thread(conn, *id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let after = orch_engine::tools::read_tools::thread_read_audience(
                conn,
                &thread,
                app.clock.time(),
            )
            .await
            .map_err(ApiError::engine)?;
            if self.force || after.gains_from(before) {
                selected.push((*id, count));
                total = total
                    .checked_add(count)
                    .ok_or_else(|| ApiError::internal("the disclosure count is too large"))?;
            }
        }
        if selected.is_empty() {
            return Ok(None);
        }
        let confirmation =
            Hash::of(&serde_json::to_vec(&(self.intent, &selected)).map_err(ApiError::internal)?)
                .to_string();
        Ok(Some(OwnerReadDisclosure {
            count: total,
            threads: i64::try_from(selected.len()).map_err(ApiError::internal)?,
            confirmation,
        }))
    }

    pub async fn require(&self, ctx: &Ctx, conn: &mut Conn) -> ApiResult<()> {
        let Some(disclosure) = self.disclosure(&ctx.app, conn).await? else {
            return Ok(());
        };
        if ctx
            .headers
            .get(HEADER)
            .and_then(|value| value.to_str().ok())
            == Some(disclosure.confirmation.as_str())
        {
            return Ok(());
        }
        let error = ApiError::new(
            ErrorCode::ConflictConfirmationRequired,
            "Confirm that sharing this history reveals the results read with the owner's access.",
        );
        if ctx.key_receipt_required() {
            return Err(error);
        }
        Err(error.with("owner_reads", disclosure))
    }
}
