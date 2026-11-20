//! A short source capture outlives its lock and stays owned through copied publication.
use super::*;
use orch_db::Conn;

/// Metadata only. The ordinary shared claim session protects this request's
/// internal replay row; no HTTP credential or reusable authority is retained.
pub(crate) struct HistoryCapture {
    pub id: Id,
    pub user: Id,
    pub key: String,
    db: orch_db::Db,
    lease: Option<claims::Lease>,
    completed: bool,
}

pub(crate) async fn begin(ctx: &Ctx) -> ApiResult<HistoryCapture> {
    let claims = ACTIVE_CLAIMS
        .try_with(Arc::clone)
        .map_err(|_| ApiError::internal("The history capture has no request owner"))?;
    let user = ctx.user()?.id();
    let id = new_id();
    let key = new_id().to_string();
    let lease = claims.lock(ctx.app.clone(), user, key.clone()).await?;
    let mut tx = ctx.app.db.begin().await?;
    rows::keep(
        &mut tx,
        user,
        &key,
        &Replay {
            method: "POST".into(),
            path: format!("/internal/history/{id}"),
            status: 200,
            response: serde_json::to_value(Record::pending(
                Hash::of(id.as_bytes()),
                "history_capture",
                id,
            ))
            .map_err(ApiError::internal)?,
        },
        ctx.app.clock.now(),
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(HistoryCapture {
        id,
        user,
        key,
        db: ctx.app.db.clone(),
        lease: Some(lease),
        completed: false,
    })
}

impl HistoryCapture {
    /// The publisher holds its own metadata row, never the source thread. Expiry
    /// skips locked owners and cannot retire a capture during publication.
    pub async fn lock(&self, conn: &mut Conn) -> ApiResult<()> {
        let value: Option<Json> = sqlx::query_scalar(
            "SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=$2 FOR UPDATE",
        )
        .bind(self.user)
        .bind(&self.key)
        .fetch_optional(conn)
        .await
        .map_err(orch_db::DbError::from)?;
        if value
            .map(Record::read)
            .transpose()?
            .is_none_or(|record| record.owner != self.id)
        {
            return Err(ApiError::internal(
                "The history capture's request owner is unavailable",
            ));
        }
        Ok(())
    }

    pub async fn cleanup(&mut self) -> ApiResult<()> {
        cleanup(&self.db, self.user, &self.key, self.id).await?;
        self.completed = true;
        Ok(())
    }
}

/// An already committed capture has a cleanup owner after request cancellation.
/// Acquiring a pool slot before any durable capture remains cancellable normally.
impl Drop for HistoryCapture {
    fn drop(&mut self) {
        let Some(lease) = self.lease.take() else {
            return;
        };
        if self.completed {
            drop(lease);
            return;
        }
        let db = self.db.clone();
        let user = self.user;
        let key = self.key.clone();
        let id = self.id;
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                if let Err(error)=cleanup(&db,user,&key,id).await {
                    tracing::warn!(%error,capture=%id,"An abandoned history capture awaits ordinary expiry cleanup.");
                }
                drop(lease);
            });
        }
    }
}

async fn cleanup(db: &orch_db::Db, user: Id, key: &str, id: Id) -> ApiResult<()> {
    let mut tx = db.begin().await?;
    let owner: Option<Json> = sqlx::query_scalar(
        "SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=$2 FOR UPDATE",
    )
    .bind(user)
    .bind(key)
    .fetch_optional(&mut *tx)
    .await
    .map_err(orch_db::DbError::from)?;
    if owner
        .map(Record::read)
        .transpose()?
        .is_none_or(|record| record.owner != id)
    {
        tx.commit().await.map_err(orch_db::DbError::from)?;
        return Ok(());
    }
    orch_db::queries::thread_history_cuts::retire_request(&mut tx, id).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    let mut tx = db.begin().await?;
    sqlx::query("SELECT user_id FROM idempotency_keys WHERE user_id=$1 AND key=$2 FOR UPDATE")
        .bind(user)
        .bind(key)
        .execute(&mut *tx)
        .await
        .map_err(orch_db::DbError::from)?;
    orch_db::queries::thread_history_cuts::release_request(&mut tx, id).await?;
    sqlx::query(
        "DELETE FROM idempotency_keys WHERE user_id=$1 AND key=$2 AND response->>'owner'=$3::uuid::text",
    )
    .bind(user)
    .bind(key)
    .bind(id)
    .execute(&mut *tx)
    .await
    .map_err(orch_db::DbError::from)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}
