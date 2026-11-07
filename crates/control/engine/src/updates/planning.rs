//! Once-only external observations consumed by the ordinary native update inspector.

use super::{Engine, Target, ToolBuild, UpdateError, blocker, operation};
use crate::hosting::HarnessRequest;
use orch_db::Conn;
use orch_driver_contract::kept::KeepEstimate;
use orch_types::Id;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ToolRequests {
    Original(Vec<String>),
    Current(Vec<HarnessRequest>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ToolInput {
    pub target: Target,
    pub runtime: Id,
    pub template: Id,
    pub os: String,
    pub arch: String,
    pub requests: ToolRequests,
}

/// Immutable observations of the actual native requests, without reusable authority.
/// Replanning checks their complete native request keys before using either result.
#[derive(Debug, Clone, Default)]
pub struct PlanningObservation {
    tools: Option<(ToolInput, Result<Vec<ToolBuild>, String>)>,
    keep: Option<((Id, Id), Option<KeepEstimate>)>,
}

pub(super) enum Planning<'a> {
    Observe {
        observation: &'a mut PlanningObservation,
        source: Option<&'a mut operation::Input>,
    },
    Replay(&'a PlanningObservation),
}
impl Planning<'_> {
    pub(super) async fn tools(
        &mut self,
        engine: &Engine,
        conn: &mut Conn,
        input: ToolInput,
    ) -> Result<Vec<ToolBuild>, UpdateError> {
        let result = match self {
            Self::Observe {
                observation,
                source,
            } => {
                if let Some(source) = source.as_deref_mut() {
                    source.current(engine).await?;
                }
                let services = engine.host_services().ok_or_else(|| {
                    crate::EngineError::unreadable("no host services are attached")
                })?;
                let result = match &input.requests {
                    ToolRequests::Original(wanted) => {
                        services.tools(conn, &input.os, &input.arch, wanted).await
                    }
                    ToolRequests::Current(requested) => {
                        services
                            .requested_tools(conn, &input.os, &input.arch, requested)
                            .await
                    }
                }
                .map_err(|error| error.to_string());
                observation.tools = Some((input, result.clone()));
                if let Some(source) = source.as_deref_mut() {
                    source.current(engine).await?;
                }
                result
            }
            Self::Replay(observation) => observation
                .tools
                .as_ref()
                .filter(|(observed, _)| observed == &input)
                .map(|(_, result)| result.clone())
                .ok_or(UpdateError::Expired)?,
        };
        result.map_err(|message| UpdateError::Blocked(vec![blocker("build_unavailable", message)]))
    }

    pub(super) async fn keep_estimate(
        &mut self,
        engine: &Engine,
        machine: Id,
        runtime: Id,
    ) -> Result<Option<KeepEstimate>, UpdateError> {
        match self {
            Self::Observe {
                observation,
                source,
            } => {
                let estimate = match source {
                    Some(source) => source.estimate_files(engine, machine, runtime).await?,
                    None => engine.estimate_runtime_files(machine, runtime).await?,
                };
                observation.keep = Some(((machine, runtime), estimate));
                Ok(estimate)
            }
            Self::Replay(observation) => observation
                .keep
                .as_ref()
                .filter(|(observed, _)| *observed == (machine, runtime))
                .map(|(_, estimate)| *estimate)
                .ok_or(UpdateError::Expired),
        }
    }
}
