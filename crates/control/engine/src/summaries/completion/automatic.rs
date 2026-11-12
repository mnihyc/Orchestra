//! Existing automatic title/tag/handoff/goal work retains its actual native call receipt.
use super::*;
use orch_authz::{Caller, Principal};
use orch_db::queries::{hierarchy, link::NewCommand};
use orch_types::{Clock, Time};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Source {
    call: Call,
    root: Id,
    configured: (Id, String),
    workflow: Id,
    task: String,
    history: Option<(Id, i64)>,
    placement: Json,
}

impl Source {
    pub(super) async fn capture(
        conn: &mut Conn,
        host: &Host,
        call: &Call,
        root: Id,
        configured: &(Id, String),
    ) -> Result<Self, EngineError> {
        // This is standing automatic work created by the native factory, not an HTTP
        // person/key reconstructed from ids. Person-issued work must retain its Work carrier.
        if host.resource.is_some() || call.issued_by.is_some() || host.writer != call.person_id {
            return Err(unavailable());
        }
        let mut nodes = hierarchy::with_ancestors(conn, &[call.node_id]).await?;
        let node = nodes
            .iter()
            .find(|node| node.id == call.node_id)
            .ok_or_else(unavailable)?;
        if !matches!(node.kind.as_str(), "thread" | "topic") {
            return Err(unavailable());
        }
        let is_thread = node.kind == "thread";
        nodes.sort_unstable_by_key(|node| node.id);
        let history = threads::thread(conn, call.node_id)
            .await?
            .map(|thread| (thread.active_continuation_id, thread.execution_revision));
        if is_thread != history.is_some() {
            return Err(unavailable());
        }
        Ok(Self {
            call: call.clone(),
            root,
            configured: configured.clone(),
            workflow: host.workflow,
            task: host.task.into(),
            history,
            placement: serde_json::to_value(
                nodes
                    .iter()
                    .map(|node| (node.id, node.parent_id, node.org_id))
                    .collect::<Vec<_>>(),
            )?,
        })
    }

    pub(crate) fn provider(&self) -> Id {
        self.call.provider_id
    }
    pub(crate) fn belongs_to(&self, node: Id, call: Id) -> bool {
        self.call.node_id == node && self.call.id == call
    }

    pub(crate) async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        if self.history.is_some() {
            threads::lock(conn, self.call.node_id)
                .await?
                .ok_or_else(unavailable)?;
        }
        let mut nodes = hierarchy::with_ancestors(conn, &[self.call.node_id]).await?;
        nodes.sort_unstable_by_key(|node| node.id);
        for node in nodes {
            hierarchy::lock(conn, node.id).await?;
        }
        sqlx::query("SELECT id FROM providers WHERE id=$1 FOR SHARE")
            .bind(self.call.provider_id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?
            .ok_or_else(unavailable)?;
        sqlx::query("SELECT id FROM summary_calls WHERE id=$1 FOR UPDATE")
            .bind(self.call.id)
            .fetch_optional(conn)
            .await
            .map_err(DbError::from)?
            .ok_or_else(unavailable)?;
        Ok(())
    }

    pub(crate) async fn current(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        let Some(writer) = self.call.person_id else {
            return Ok(None);
        };
        let kept: Option<Call> = sqlx::query_as("SELECT id,node_id,topic_id,thread_id,provider_id,person_id,issued_by,entry_id,model_name,created_at FROM summary_calls WHERE id=$1 AND purpose='summary' AND cancel_reason IS NULL")
            .bind(command.id).fetch_optional(&mut *conn).await.map_err(DbError::from)?;
        if kept.as_ref() != Some(&self.call) {
            return Ok(None);
        }
        let mut nodes = hierarchy::with_ancestors(conn, &[self.call.node_id]).await?;
        nodes.sort_unstable_by_key(|node| node.id);
        if nodes
            .iter()
            .any(|node| node.deleted_at.is_some() || node.archived_at.is_some())
            || serde_json::to_value(
                nodes
                    .iter()
                    .map(|node| (node.id, node.parent_id, node.org_id))
                    .collect::<Vec<_>>(),
            )? != self.placement
        {
            return Ok(None);
        }
        let history = threads::thread(conn, self.call.node_id)
            .await?
            .map(|thread| (thread.active_continuation_id, thread.execution_revision));
        if history != self.history {
            return Ok(None);
        }
        if matches!(self.task.as_str(), "title" | "tags") {
            let active: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM node_summary_state WHERE node_id=$1 AND run_id=$2 AND claimed_until>$3)")
                .bind(self.call.node_id).bind(self.workflow).bind(clock.now()).fetch_one(&mut *conn).await.map_err(DbError::from)?;
            if !active {
                return Ok(None);
            }
        }
        let caller = Caller::session(writer).with_admin_mode(false);
        let principal = Principal::load(conn, caller.clone(), clock.time()).await?;
        let target = if self.history.is_some() {
            orch_api_contract::summaries::SummaryTarget::Thread {
                id: self.call.node_id,
            }
        } else {
            orch_api_contract::summaries::SummaryTarget::Topic {
                id: self.call.node_id,
            }
        };
        if !crate::summaries::sources::readable(conn, &principal, target)
            .await
            .map_err(|_| unavailable())?
            || !activity::accepts_command(
                conn,
                command.id,
                self.call.provider_id,
                &self.call.model_name,
            )
            .await?
        {
            return Ok(None);
        }
        let request: ProviderComplete = serde_json::from_value(command.body.clone())?;
        if request.provider_id != self.call.provider_id
            || request.model_name != self.call.model_name
            || request.purpose != orch_link_contract::commands::CompletionPurpose::Summary
        {
            return Ok(None);
        }
        if self.call.id != self.root
            || (request.provider_id, &request.model_name) != (self.configured.0, &self.configured.1)
        {
            let Some(recovery) = crate::calls::read_root(
                conn,
                self.call.node_id,
                orch_provider_contract::calls::Purpose::Summary,
                self.root,
            )
            .await?
            else {
                return Ok(None);
            };
            let policy = crate::calls::Policy::load(conn, self.call.node_id).await?;
            if recovery.state != "running"
                || recovery.context["next_call"] != json!(self.call.id)
                || !policy.rules.iter().any(|rule| json!(rule) == recovery.rule)
            {
                return Ok(None);
            }
        }
        let thread = threads::thread(conn, self.call.node_id).await?;
        let original = json!({"system":request.system,"input":request.input});
        if crate::raw_events::mask_summary(
            conn,
            self.call.node_id,
            self.call.topic_id,
            thread.and_then(|thread| thread.environment_id),
            original.clone(),
        )
        .await?
        .as_ref()
            != Some(&original)
        {
            return Ok(None);
        }
        Ok(Some(vec![(Some(caller), None)]))
    }
}

fn unavailable() -> EngineError {
    EngineError::BeforeEffect(Box::new(crate::operation_scope::continuing::Error::Refused))
}
