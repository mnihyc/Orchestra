//! The API's shared documentation and person-scoped readers on the ordinary Orch tool path.

use super::definitions::{Code, Refusal};
use super::{Answer, CallContext, side_reading};
use crate::{Engine, EngineError};
use async_trait::async_trait;
use orch_access_contract::pages::Holder;
use orch_agent_contract::{descriptor::SideReading, events::OrchTool};
use orch_db::{Conn, queries::threads::ThreadRow};
use orch_settings::{Place, Snapshot, Wanted};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json, Time};
use std::collections::BTreeMap;
use std::sync::{Arc, PoisonError};

/// Actual ordinary readers, for the native sharing transition and its informed confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Audience {
    /// The first owner before the proposed sharing act.
    pub owner: Id,
    /// Native grant identities and their actual holders, including empty groups and future agents.
    pub grants: BTreeMap<Id, Holder>,
}

impl Audience {
    #[must_use]
    pub fn gains_from(&self, previous: &Self) -> bool {
        self.grants.iter().any(|(id, holder)| {
            *holder != Holder::User { id: previous.owner }
                && previous.grants.get(id) != Some(holder)
        })
    }
}

/// The public alternative when a thread has readers besides its owner.
pub const SHARED_POINTER: &str = "This thread is shared. Person-scoped reads are unavailable here. Use list_shared, read_shared or search_shared instead.";

/// The API supplies its existing Ask readers, not another HTTP client or permission evaluator.
#[async_trait]
pub trait ReadTools: Send + Sync {
    /// Public help, without a person-scoped read grant.
    async fn documentation(&self, arguments: &Json) -> Result<Json, Refusal>;
    /// One genuine admitted native call, checked against the current owner and viewer set.
    async fn owner_read(&self, call: &CallContext, thread: &ThreadRow) -> Result<Json, Refusal>;
    /// Recheck a successful personal result on the command insertion transaction. A false
    /// result is ordinary data or has been narrowed to a refusal, never a new read.
    async fn prepare_result(
        &self,
        conn: &mut Conn,
        answer: Id,
        body: &mut Json,
    ) -> Result<bool, EngineError>;
}

pub(crate) async fn enabled(conn: &mut Conn, thread: Id) -> Result<bool, EngineError> {
    let definition = super::definition_of(keys::ORCH_TOOLS_OWNER_READS)?;
    let snapshot = Snapshot::load(conn, Place::Node(thread), Wanted::Keys(&[definition])).await?;
    Ok(super::setting::<bool>(&snapshot, keys::ORCH_TOOLS_OWNER_READS)?.unwrap_or(false))
}

/// Documentation has no per-thread opt-in. Personal reads are controlled by their one option,
/// rather than by inserting their names into the independently configured shared tool list.
pub(crate) async fn append_offered(
    conn: &mut Conn,
    thread: &ThreadRow,
    offered: &mut Vec<OrchTool>,
) -> Result<(), EngineError> {
    offered.retain(|tool| !tool.is_owner_read() && *tool != OrchTool::OrchDocs);
    if side_reading(thread) == Some(SideReading::NoTools) {
        return Ok(());
    }
    offered.push(OrchTool::OrchDocs);
    if enabled(conn, thread.node_id).await? {
        offered.extend([
            OrchTool::Open,
            OrchTool::Search,
            OrchTool::FindOperation,
            OrchTool::Read,
        ]);
    }
    Ok(())
}

impl Engine {
    /// Attach the same API-owned readers used by Ask, without retaining its application.
    pub fn attach_read_tools(&self, tools: Arc<dyn ReadTools>) {
        *self
            .inner
            .read_tools
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(tools);
    }

    pub(crate) async fn read_tool_call(
        &self,
        call: &CallContext,
        thread: &ThreadRow,
    ) -> Result<Answer, EngineError> {
        let tools = self
            .inner
            .read_tools
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let Some(tools) = tools else {
            return Ok(Refusal::new(
                Code::Unsupported,
                "The server's read tools are unavailable.",
            )
            .into());
        };
        let answer = if call.tool == OrchTool::OrchDocs {
            tools.documentation(&call.arguments).await
        } else {
            tools.owner_read(call, thread).await
        };
        Ok(match answer {
            Ok(value) => Answer::Done(value),
            Err(refusal) => refusal.into(),
        })
    }
}

/// Eligibility follows the native grants, never their current expanded membership.
/// A group, role, agent, link or other person can gain readers later without another act here.
pub async fn owner_is_only_viewer(
    conn: &mut Conn,
    thread: &ThreadRow,
    now: Time,
) -> Result<bool, EngineError> {
    let grants = orch_authz::engine::conversation_reader_grants(conn, thread.node_id, now).await?;
    Ok(!grants.is_empty()
        && grants.iter().all(|entry| {
            entry.holder
                == Holder::User {
                    id: thread.owner_id,
                }
        }))
}

/// The same native grant facts are compared before and after an informed sharing act.
pub async fn thread_read_audience(
    conn: &mut Conn,
    thread: &ThreadRow,
    now: Time,
) -> Result<Audience, EngineError> {
    let grants = orch_authz::engine::conversation_reader_grants(conn, thread.node_id, now)
        .await?
        .into_iter()
        .map(|entry| (entry.id, entry.holder))
        .collect();
    Ok(Audience {
        owner: thread.owner_id,
        grants,
    })
}
