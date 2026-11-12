//! The native fork producer uses the same capture family as independent notice delivery.
use orch_db::{
    Conn,
    queries::{
        notifications::operations::ending::ThreadMove, notifications::people::NewNotification,
        transaction_guard,
    },
};
pub use orch_person_notices::*;
use orch_types::Id;
use std::collections::BTreeSet;

/// Factual output and reviewed choice of the native fork job that ends its source.
pub struct ThreadMoveNotice<'a> {
    /// The actual native fork job; also the stable notice identity.
    pub job: Id,
    /// The original thread, still live on this transaction.
    pub source: Id,
    /// The real new thread made by that job.
    pub created: Id,
    /// The accepted notification choice.
    pub notify: bool,
    /// The reviewed affected people; duplicates are sent once, the mover is excluded.
    pub people: &'a [Id],
}

impl crate::Engine {
    /// Capture and file an accepted move's personal receipts before its native source ending.
    /// The caller retains this same transaction for that ending and its atomic commit.
    pub async fn tell_thread_move(
        &self,
        conn: &mut Conn,
        notice: &ThreadMoveNotice<'_>,
    ) -> Result<(), crate::EngineError> {
        if !notice.notify || notice.people.is_empty() {
            return Ok(());
        }
        if !transaction_guard::in_transaction(conn) {
            return Err(crate::EngineError::unreadable(
                "a moved-thread notice needs its success transaction",
            ));
        }
        let row = orch_db::queries::jobs::job(conn, notice.job)
            .await?
            .ok_or_else(|| crate::EngineError::unreadable("the native move job is unavailable"))?;
        if row.kind != crate::jobs::fork::KIND
            || row.node_id != Some(notice.source)
            || row.plan.get("source_thread") != Some(&serde_json::json!(notice.source))
            || row.plan.get("new_thread") != Some(&serde_json::json!(notice.created))
            || notice.source == notice.created
        {
            return Err(crate::EngineError::unreadable(
                "the native moved-thread receipt changed its job association",
            ));
        }
        let receipt = ThreadMove {
            source: notice.source,
            created: notice.created,
            job: notice.job,
        };
        let by = row
            .created_by
            .ok_or_else(|| crate::EngineError::unreadable("the native move job has no actor"))?;
        if Capture::recorded_thread_move(conn, &receipt, by)
            .await
            .map_err(crate::EngineError::unreadable)?
        {
            return Ok(());
        }
        let preparation =
            crate::jobs::fork::operation::Preparation::from_job(&row)?.ok_or_else(|| {
                crate::EngineError::unreadable("the native move job has no original source")
            })?;
        let checkpoint = preparation
            .current(conn, self.inner.clock.as_ref(), self.inner.access.as_ref())
            .await?;
        let caller = checkpoint.caller();
        if caller.admitted_operation() != Some("ThreadMoveOut")
            || caller.admitted_required_roles().is_none()
        {
            return Err(crate::EngineError::unreadable(
                "the native move notice needs its complete actual operation",
            ));
        }
        let old = orch_db::queries::threads::thread(conn, notice.source)
            .await?
            .filter(|row| row.deleted_at.is_none())
            .ok_or_else(|| crate::EngineError::unreadable("the original thread already ended"))?;
        let people = notice
            .people
            .iter()
            .copied()
            .filter(|user| *user != caller.user())
            .collect::<BTreeSet<_>>();
        let eligible = people.iter().copied().collect::<Vec<_>>();
        let mut capture = Capture::prepare_thread_move(
            conn,
            self.inner.clock.as_ref(),
            caller,
            &receipt,
            &eligible,
        )
        .await
        .map_err(crate::EngineError::unreadable)?;
        let data = serde_json::json!({"operation_id":notice.job,"by":caller.user(),"subject":old.name,
            "path":format!("/thread/{}",notice.created),"completed_thread_move":receipt});
        let key = format!("person.notify:thread_move:{}", notice.job);
        let title = old.name.clone();
        let at = self.inner.clock.now();
        for user in people {
            let notification = NewNotification {
                user_id: user,
                kind: "person.notify",
                node_id: None,
                thread_id: None,
                chat_id: None,
                approval_id: None,
                title: &title,
                body: "",
                data: &data,
                dedupe_key: &key,
                at,
            };
            let actual = super::tell_person_notice_before(
                conn,
                Some(old.org_id),
                &notification,
                Some(|| checkpoint.check(self.inner.clock.as_ref())),
            )
            .await?;
            if let Some(actual) = actual {
                capture
                    .record(&notification, &actual)
                    .map_err(crate::EngineError::unreadable)?;
            }
        }
        capture
            .save(conn)
            .await
            .map_err(crate::EngineError::unreadable)?;
        checkpoint.check(self.inner.clock.as_ref())?;
        Ok(())
    }
}
