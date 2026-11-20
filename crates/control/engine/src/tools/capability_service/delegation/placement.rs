//! Placement facts do not grant provider, environment, creator or source authority.
use crate::EngineError;
use orch_db::{
    Conn,
    queries::{environments::thread_engine as environments, hierarchy, threads},
};
use orch_provider_contract::capabilities::ProgramPlacement;
use orch_types::Id;
use serde::{Deserialize, Serialize};

/// The exact source placement, retained through startup and result publication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    pub root: Id,
    pub topic: Id,
    pub environment: Id,
    pub environment_generation: i32,
    pub directory: String,
    pub source_thread: Option<(Id, i64)>,
}

impl Placement {
    /// A real thread supplies its actual placement, independently of backend config.
    pub async fn for_thread(
        conn: &mut Conn,
        thread: &threads::ThreadRow,
    ) -> Result<Self, EngineError> {
        let environment = thread.environment_id.ok_or(EngineError::StaleToolSource)?;
        let row = environments::environment(conn, environment)
            .await?
            .filter(|row| row.deleted_at.is_none() && row.remove_at.is_none())
            .ok_or(EngineError::StaleToolSource)?;
        Ok(Self {
            root: thread.org_id,
            topic: thread.topic_id,
            environment,
            environment_generation: row.generation,
            directory: thread.workspace_dir.clone(),
            source_thread: Some((thread.node_id, thread.execution_revision)),
        })
    }

    /// A threadless producer supplies its real root/topic; explicit config selects no authority.
    pub async fn configured(
        conn: &mut Conn,
        root: Id,
        source_topic: Option<Id>,
        config: &ProgramPlacement,
    ) -> Result<Self, EngineError> {
        let environment = config.environment_id.ok_or_else(|| {
            EngineError::unreadable("Choose an environment for this delegated program.")
        })?;
        let topic = source_topic.or(config.helper_topic_id).ok_or_else(|| {
            EngineError::unreadable("Choose a helper topic in this work's root organization.")
        })?;
        let topic_row = hierarchy::node(conn, topic)
            .await?
            .filter(|node| {
                node.kind == "topic"
                    && node.org_id == root
                    && node.deleted_at.is_none()
                    && node.archived_at.is_none()
            })
            .ok_or_else(|| {
                EngineError::unreadable(
                    "The helper topic is unavailable in this work's root organization.",
                )
            })?;
        let row = environments::environment(conn, environment)
            .await?
            .filter(|row| {
                row.deleted_at.is_none()
                    && row.remove_at.is_none()
                    && !matches!(
                        row.status.as_str(),
                        "lost" | "removed" | "failed" | "disabled"
                    )
            })
            .ok_or_else(|| {
                EngineError::unreadable("The selected program environment is unavailable.")
            })?;
        Ok(Self {
            root,
            topic: topic_row.id,
            environment,
            environment_generation: row.generation,
            directory: String::new(),
            source_thread: None,
        })
    }

    /// Recheck the actual binding; starting a runtime cannot move work to a different place.
    pub async fn current(&self, conn: &mut Conn) -> Result<bool, EngineError> {
        let Some(environment) = environments::environment(conn, self.environment).await? else {
            return Ok(false);
        };
        if environment.generation != self.environment_generation
            || environment.deleted_at.is_some()
            || environment.remove_at.is_some()
        {
            return Ok(false);
        }
        let Some(topic) = hierarchy::node(conn, self.topic).await? else {
            return Ok(false);
        };
        if topic.kind != "topic"
            || topic.org_id != self.root
            || topic.deleted_at.is_some()
            || topic.archived_at.is_some()
        {
            return Ok(false);
        }
        if let Some((id, revision)) = self.source_thread {
            let Some(thread) = threads::thread(conn, id).await? else {
                return Ok(false);
            };
            return Ok(thread.deleted_at.is_none()
                && thread.archived_at.is_none()
                && thread.org_id == self.root
                && thread.execution_revision == revision
                && thread.topic_id == self.topic
                && thread.environment_id == Some(self.environment)
                && thread.workspace_dir == self.directory);
        }
        Ok(true)
    }
}
