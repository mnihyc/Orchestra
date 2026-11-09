//! Exact current call-rule attempt facts, conjunctive with the original tool authority.
use super::*;
use orch_provider_contract::calls::{Candidate, Purpose};
use orch_provider_contract::capabilities::BackendRef;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Binding {
    root: Id,
    reference: Id,
    rule: Json,
    cursor: Json,
    configured: BackendRef,
    executed: BackendRef,
    candidate: Option<Candidate>,
}
impl Binding {
    pub(super) fn capture(
        root: Id,
        configured: &selection::Selection,
        executed: &selection::Selection,
        row: &crate::calls::Recovery,
        candidate: Option<Candidate>,
    ) -> Result<Self, EngineError> {
        if row.call_key != root
            || row.context["root_call"] != serde_json::json!(root)
            || row.state != "running"
            || row.context["configured_capability_route"] != serde_json::json!(configured.reference)
        {
            return Err(EngineError::StaleToolSource);
        }
        Ok(Self {
            root,
            reference: row.reference,
            rule: row.rule.clone(),
            cursor: row.cursor.clone(),
            configured: configured.reference.clone(),
            executed: executed.reference.clone(),
            candidate,
        })
    }
    pub(super) async fn lock(
        &self,
        conn: &mut Conn,
        node: Id,
        capability: &str,
    ) -> Result<(), EngineError> {
        let _=sqlx::query("SELECT reference FROM model_call_recovery WHERE node_id=$1 AND purpose=$2 AND call_key=$3 FOR SHARE")
            .bind(node).bind(capability).bind(self.root).fetch_optional(conn).await.map_err(orch_db::DbError::from)?;
        Ok(())
    }
    pub(super) async fn current(
        &self,
        conn: &mut Conn,
        command: &ServiceCommand,
    ) -> Result<bool, EngineError> {
        let Some((_, call)) = command.origin.tool() else {
            return Ok(false);
        };
        if source::resource_from_answer(
            "provider-service",
            crate::ids::tool_result(call.thread, &call.call_ref),
        ) != self.root
            || command
                .backend
                .as_ref()
                .is_none_or(|selected| selected.reference != self.executed)
        {
            return Ok(false);
        }
        let purpose: Purpose = serde_json::from_value(serde_json::json!(command.capability))?;
        let Some(row) = crate::calls::read_root(conn, call.thread, purpose, self.root).await?
        else {
            return Ok(false);
        };
        if row.state != "running"
            || row.reference != self.reference
            || row.rule != self.rule
            || row.cursor != self.cursor
            || row.context["next_call"] != serde_json::json!(command.command)
            || row.context["configured_capability_route"] != serde_json::json!(self.configured)
        {
            return Ok(false);
        }
        let candidate = row
            .context
            .get("candidate")
            .filter(|value| !value.is_null())
            .cloned()
            .map(serde_json::from_value::<Candidate>)
            .transpose()?;
        if candidate != self.candidate {
            return Ok(false);
        }
        Ok(crate::calls::Policy::load(conn, call.thread)
            .await?
            .rules
            .iter()
            .any(|rule| serde_json::json!(rule) == self.rule))
    }
}

#[derive(Clone)]
pub(super) struct Attempt {
    pub root: Id,
    pub command: Id,
    pub selected: selection::Selection,
    pub binding: Option<Binding>,
}
tokio::task_local! {static ATTEMPT:Attempt;}
impl Attempt {
    pub(super) async fn scope<T>(self, work: impl std::future::Future<Output = T>) -> T {
        ATTEMPT.scope(self, work).await
    }
}
pub(super) fn command(root: Id) -> Result<Id, EngineError> {
    match ATTEMPT.try_with(Clone::clone).ok() {
        Some(attempt) if attempt.root == root => Ok(attempt.command),
        Some(_) => Err(EngineError::StaleToolSource),
        None => Ok(root),
    }
}
pub(super) fn selected(root: selection::Selection) -> Result<selection::Selection, EngineError> {
    let Some(attempt) = ATTEMPT.try_with(Clone::clone).ok() else {
        return Ok(root);
    };
    if root.setting != attempt.selected.setting
        || root.value != attempt.selected.value
        || root.mode_setting != attempt.selected.mode_setting
    {
        return Err(EngineError::StaleToolSource);
    }
    Ok(attempt.selected)
}
pub(super) fn binding() -> Option<Binding> {
    ATTEMPT
        .try_with(|attempt| attempt.binding.clone())
        .ok()
        .flatten()
}
