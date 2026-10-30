//! Cancelling a search read by its backend and transaction tag without cancelling a reused connection.

use crate::{Conn, Db, DbError};
use sqlx::{Connection, postgres::PgConnectOptions};
use std::sync::Arc;

/// Cancels an unfinished search read when its owning response is dropped.
pub struct CancelRead {
    options: Arc<PgConnectOptions>,
    active: Option<(i32, String)>,
}

impl CancelRead {
    /// The read completed before its transaction committed.
    pub fn disarm(&mut self) {
        self.active = None;
    }
}

impl Drop for CancelRead {
    fn drop(&mut self) {
        let Some((pid, tag)) = self.active.take() else {
            return;
        };
        let options = self.options.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                // The reading pool can be full of cancelled reads. The control connection holds no
                // transaction; the unique local tag prevents cancelling a subsequently reused backend.
                let result = async {
                    let mut conn = sqlx::PgConnection::connect_with(&options).await?;
                    let _ = sqlx::query_scalar::<_, bool>("SELECT pg_cancel_backend($1) WHERE EXISTS
                        (SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND application_name=$2 AND state='active')")
                        .bind(pid).bind(tag).fetch_optional(&mut conn).await?;
                    conn.close().await
                }.await;
                if let Err(error) = result { tracing::warn!(%error, "search cancellation could not reach its backend"); }
            });
        }
    }
}

/// Tags this read transaction so its cancellation can reach PostgreSQL after its future is dropped.
pub async fn cancel_on_drop(db: &Db, conn: &mut Conn) -> Result<CancelRead, DbError> {
    let tag = format!("orch-search:{}", orch_types::new_id());
    let (pid, _): (i32, String) =
        sqlx::query_as("SELECT pg_backend_pid(),set_config('application_name',$1,true)")
            .bind(&tag)
            .fetch_one(conn)
            .await?;
    Ok(CancelRead {
        options: db.pool().connect_options(),
        active: Some((pid, tag)),
    })
}
