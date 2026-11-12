//! A creation choice is a pure live Share made inside the thread's transaction.

use crate::engine::Engine;
use crate::error::EngineError;
use crate::sharing::ShareError;
use orch_api_contract::nodes::{AgentAccessChoice, AgentAccessWhere};
use orch_api_contract::share::{
    ShareParts, ShareTo, ShareToThread, ShareWhat, ShareWhatNode, ShareWithAgents,
};
use orch_api_contract::threads::ShareMode;
use orch_authz::Principal;
use orch_db::Conn;
use orch_db::audit::Origin;
use orch_db::queries::{access, hierarchy};
use orch_settings::Place;
use orch_settings_contract::registry::keys;
use orch_types::Id;
use serde::{Deserialize, Serialize};

/// Where an initial choice came from. Only a new holder consumes it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "choice", rename_all = "snake_case")]
pub enum BirthAgentAccess {
    /// An omitted request accepts the destination's creation preset.
    #[default]
    ResolvePreset,
    /// An omitted fork request keeps the preset seen when its plan was made.
    FrozenPreset(AgentAccessChoice),
    /// A person supplied this exact choice, even when it equals a preset.
    Exact(AgentAccessChoice),
}

impl From<Option<AgentAccessChoice>> for BirthAgentAccess {
    fn from(choice: Option<AgentAccessChoice>) -> Self {
        choice.map_or(Self::ResolvePreset, Self::Exact)
    }
}

impl BirthAgentAccess {
    pub(crate) fn nothing() -> Self {
        Self::Exact(AgentAccessChoice {
            where_: AgentAccessWhere::Nothing,
            parts: ShareParts::default(),
        })
    }

    pub(crate) async fn freeze(self, conn: &mut Conn, topic: Id) -> Result<Self, ShareError> {
        let frozen = if matches!(self, Self::ResolvePreset) {
            let choice = orch_settings::get::<AgentAccessChoice>(
                conn,
                Place::Node(topic),
                keys::THREADS_AGENT_ACCESS,
            )
            .await
            .map_err(EngineError::from)?
            .ok_or_else(|| EngineError::unreadable("the thread creation preset is unset"))?;
            Self::FrozenPreset(choice)
        } else {
            self
        };
        if let Self::FrozenPreset(choice) | Self::Exact(choice) = &frozen {
            validate(choice)?;
        }
        Ok(frozen)
    }
}

fn invalid(field: &str, message: &str) -> ShareError {
    ShareError::Invalid {
        field: format!("agent_access.{field}"),
        message: message.to_owned(),
    }
}

fn validate(choice: &AgentAccessChoice) -> Result<(), ShareError> {
    if choice.where_ == AgentAccessWhere::Nothing && choice.parts != ShareParts::default() {
        return Err(invalid("parts", "Nothing has no shared areas"));
    }
    if choice.where_ == AgentAccessWhere::Thread
        && (choice.parts.timeline.is_some()
            || choice.parts.thread_fork == Some(true)
            || choice.parts.thread_archive == Some(true))
    {
        return Err(invalid(
            "parts.timeline",
            "This thread covers its documents and separate chat; its agent already has its own conversation",
        ));
    }
    Ok(())
}

/// A preset must not prevent otherwise lawful creation, but only an ordinary access refusal
/// permits falling back. Invalid data, stale/internal failures and database errors still abort.
fn preset_unavailable(error: &ShareError) -> bool {
    use orch_authz::pages::ChangeError;
    match error {
        ShareError::NotFound | ShareError::Denied(_) => true,
        ShareError::Page(error) => matches!(
            error.as_ref(),
            ChangeError::Denied(_)
                | ChangeError::Locked { .. }
                | ChangeError::Outside { .. }
                | ChangeError::ExternalLifetime { .. }
        ),
        _ => false,
    }
}

async fn project_of(conn: &mut Conn, topic: Id) -> Result<Id, ShareError> {
    let nodes = hierarchy::with_ancestors(conn, &[topic]).await?;
    let path = &nodes
        .iter()
        .find(|node| node.id == topic)
        .ok_or(ShareError::NotFound)?
        .path;
    path.iter()
        .rev()
        .find_map(|id| {
            nodes
                .iter()
                .find(|node| node.id == *id && node.kind == "project")
                .map(|node| node.id)
        })
        .ok_or_else(|| invalid("where", "the topic has no containing project"))
}

impl Engine {
    /// Check an exact existing source before a fork can start copying files. The future
    /// recipient is not fabricated here; birth repeats source admission and checks it for real.
    pub(crate) async fn check_birth_source_in(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        topic: Id,
        asked: &BirthAgentAccess,
    ) -> Result<(), ShareError> {
        let BirthAgentAccess::Exact(choice) = asked else {
            return Ok(());
        };
        if choice.parts == ShareParts::default() {
            return Ok(());
        }
        let node = match choice.where_ {
            AgentAccessWhere::Topic => topic,
            AgentAccessWhere::Project => project_of(conn, topic).await?,
            AgentAccessWhere::Nothing | AgentAccessWhere::Thread => return Ok(()),
        };
        self.check_live_node_source_in(conn, principal, &Origin::default(), node, choice.parts)
            .await
    }

    pub(crate) async fn give_birth_access(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        thread: Id,
        topic: Id,
        asked: &BirthAgentAccess,
    ) -> Result<AgentAccessChoice, ShareError> {
        let frozen = asked.clone().freeze(conn, topic).await?;
        let (choice, preset) = match frozen {
            BirthAgentAccess::FrozenPreset(choice) => (choice, true),
            BirthAgentAccess::Exact(choice) => (choice, false),
            BirthAgentAccess::ResolvePreset => unreachable!("freeze resolves the preset"),
        };
        if choice.where_ == AgentAccessWhere::Nothing || choice.parts == ShareParts::default() {
            return Ok(choice);
        }
        let source = match choice.where_ {
            AgentAccessWhere::Thread => thread,
            AgentAccessWhere::Topic => topic,
            AgentAccessWhere::Project => project_of(conn, topic).await?,
            AgentAccessWhere::Nothing => unreachable!("Nothing has no share"),
        };
        let request = ShareWithAgents {
            link_origin: None,
            viewers_read: None,
            what: ShareWhat::Node(ShareWhatNode { node_id: source }),
            inside: Some(choice.where_ != AgentAccessWhere::Thread),
            parts: Some(choice.parts),
            // The creation choice names areas, not workspace export rights.
            download: Some(false),
            mode: ShareMode::Live,
            to: ShareTo::Threads(vec![ShareToThread { thread_id: thread }]),
            read_first: None,
            comment: None,
            expires_at: None,
            client_id: format!("agent-access:{thread}"),
        };
        if !preset {
            self.give_live_in(conn, principal, &Origin::default(), &request)
                .await?;
            return Ok(choice);
        }
        // Live Share may write several rows before a later row is refused. Its whole attempt
        // belongs to a savepoint, so falling back cannot leave a partial contribution behind.
        let mut trial = access::begin_trial(conn).await?;
        match self
            .give_live_in(&mut trial, principal, &Origin::default(), &request)
            .await
        {
            Ok(_) => {
                trial.commit().await.map_err(orch_db::DbError::from)?;
                Ok(choice)
            }
            Err(error) => {
                access::end_trial(trial).await?;
                if preset_unavailable(&error) {
                    Ok(AgentAccessChoice {
                        where_: AgentAccessWhere::Nothing,
                        parts: ShareParts::default(),
                    })
                } else {
                    Err(error)
                }
            }
        }
    }
}
