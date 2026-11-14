//! Words are read without row locks; their native versions are checked under the short write fence.
use super::*;

/// An in-memory selection, never a stored permission or a substitute for the continuing frame.
pub struct Read {
    pub(super) work: Work,
    pub(super) selected: Arc<Selection>,
    versions: Json,
}

#[derive(Debug, thiserror::Error)]
#[error("the summary source changed while it was read")]
struct Moved;

impl Read {
    pub(super) async fn check(&self, conn: &mut Conn, now: Time) -> Result<(), Error> {
        if self.work.source_versions(conn).await? != self.versions {
            return Err(EngineError::BeforeEffect(Box::new(Moved)).into());
        }
        require_request(conn, &self.work, now).await?;
        if self.selected.policy.fingerprint != self.work.policy {
            return Err(SourceChange::Settings.error().into());
        }
        if self.selected.material.snapshot != self.work.source
            || self.selected.material.source_refs != self.work.refs
        {
            return Err(SourceChange::Content.error().into());
        }
        Ok(())
    }
}

impl Work {
    pub(crate) async fn read_frame(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<operation_scope::continuing::Frame<Source>, EngineError> {
        // Watch permission/settings changes before reading any words. This takes no row lock.
        orch_db::queries::transaction_guard::lock_action(conn).await?;
        let place: Id = serde_json::from_value(self.source["place"].clone())?;
        let allowed = super::super::change::read_repeats(conn, place).await?;
        let mut repeated = 0_i64;
        loop {
            let versions = self.source_versions(conn).await?;
            // The preliminary read grants no effect or disclosure. The frame below still restores
            // the exact raw credential and binds/requires every factual input as its current caller.
            let caller = self
                .origin
                .preview_current(conn, clock)
                .await?
                .ok_or_else(refused)?;
            let selected = read_selection(conn, &caller, self, clock.time())
                .await
                .map_err(frame_error)?;
            observation::source_read(self.request).await;
            let read = Read {
                work: self.clone(),
                selected: Arc::new(selected),
                versions,
            };

            // A failed optimistic read releases even the frame's identity locks before another
            // full read. The savepoint encloses no business mutation, model IO or command issue.
            sqlx::query("SAVEPOINT orch_summary_source")
                .execute(&mut *conn)
                .await
                .map_err(DbError::from)?;
            let checked = if let Some(http) = &self.http {
                if http.origin() != &self.origin {
                    return Err(refused());
                }
                http.prepare::<Source>(conn, clock, read).await
            } else {
                operation_scope::continuing::prepare::<Source>(conn, clock, &self.origin, read)
                    .await
            }
            .map_err(frame_error);
            match checked {
                Ok(frame) => {
                    sqlx::query("RELEASE SAVEPOINT orch_summary_source")
                        .execute(conn)
                        .await
                        .map_err(DbError::from)?;
                    return Ok(frame);
                }
                Err(error) => {
                    sqlx::query("ROLLBACK TO SAVEPOINT orch_summary_source")
                        .execute(&mut *conn)
                        .await
                        .map_err(DbError::from)?;
                    sqlx::query("RELEASE SAVEPOINT orch_summary_source")
                        .execute(&mut *conn)
                        .await
                        .map_err(DbError::from)?;
                    if !matches!(&error, EngineError::BeforeEffect(cause) if cause.is::<Moved>()) {
                        return Err(error);
                    }
                    if repeated >= allowed {
                        return Err(SourceChange::Unstable.error());
                    }
                    repeated += 1;
                    // The request lease and original caller's deadlines are checked on every read.
                    // Changed words never become the authority of the old, already prepared prompt.
                }
            }
        }
    }

    async fn source_versions(&self, conn: &mut Conn) -> Result<Json, EngineError> {
        let mut targets = self.refs.clone();
        if !targets.contains(&self.target) {
            targets.push(self.target);
        }
        let mut candidates = Vec::new();
        if let SummaryTarget::Topic { id } = self.target {
            let mut after: Option<(chrono::DateTime<chrono::Utc>, Id, String)> = None;
            loop {
                let page = rows::topic_sources(
                    conn,
                    id,
                    after
                        .as_ref()
                        .map(|(at, id, kind)| (*at, *id, kind.as_str())),
                    sources::SOURCE_PAGE,
                )
                .await?;
                let done = page.len() < usize::try_from(sources::SOURCE_PAGE).unwrap_or(usize::MAX);
                for row in &page {
                    let target = match row.kind.as_str() {
                        "thread" => SummaryTarget::Thread { id: row.id },
                        "chat" => SummaryTarget::Chat { id: row.id },
                        _ => return Err(refused()),
                    };
                    targets.push(target);
                    candidates.push(serde_json::json!([target, row.at]));
                }
                after = page.last().map(|row| (row.at, row.id, row.kind.clone()));
                if done {
                    break;
                }
            }
        }
        // Source tuples and their ancestry are metadata: no activity, block or message text is read.
        // xmin also detects a write whose values changed back before the fence was acquired.
        let rows: Json = sqlx::query_scalar(include_str!("source_versions.sql"))
            .bind(serde_json::to_value(targets)?)
            .bind(self.request)
            .bind(self.model.provider_id)
            .fetch_one(conn)
            .await
            .map_err(DbError::from)?;
        Ok(serde_json::json!({"rows": rows, "candidates": candidates}))
    }
}
