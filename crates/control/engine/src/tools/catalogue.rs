//! A native accepted offer is retained apart from current settings or the session refresh comparison.
use orch_agent_contract::{descriptor::SideReading, events::OrchTool};
use orch_api_contract::settings::{OrchToolFact, OrchToolUnavailable as Reason, OrchToolsSummary};
use orch_types::Time;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AcceptedOrchTools {
    #[serde(flatten)]
    pub summary: OrchToolsSummary,
    pub tools: Vec<OrchToolFact>,
}

impl AcceptedOrchTools {
    /// Check the captured offer against the tool names sent to the native session.
    pub fn matches_opening(&self, body: &orch_types::Json) -> bool {
        let Some(native) = body
            .get("session")
            .unwrap_or(body)
            .get("orch_tools")
            .and_then(orch_types::Json::as_array)
        else {
            return false;
        };
        let captured = self
            .tools
            .iter()
            .filter(|tool| tool.offered)
            .map(|tool| tool.key.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        let actual = native
            .iter()
            .filter_map(orch_types::Json::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        captured == actual
            && actual.len() == native.len()
            && self.summary.offered_count == native.len() as u64
    }
}

pub(crate) fn capture(
    thread: &orch_db::queries::threads::ThreadRow,
    actual: &[OrchTool],
    inputs: super::ToolOfferInputs,
    at: Time,
) -> AcceptedOrchTools {
    let selected = super::definitions::offered(&inputs.enabled, super::side_reading(thread));
    let tools = OrchTool::ALL
        .iter()
        .map(|tool| {
            let offered = actual.contains(tool);
            let service = matches!(
                tool,
                OrchTool::WebSearch | OrchTool::WebFetch | OrchTool::ImageGeneration
            );
            let special = service || matches!(tool, OrchTool::Goal | OrchTool::Schedule);
            let enabled =
                selected.contains(tool) || inputs.enabled.iter().any(|name| name == tool.name());
            let reason = if offered {
                None
            } else if let Some(blocker) = inputs.blocker {
                Some(blocker)
            } else if super::side_reading(thread) == Some(SideReading::NoTools) {
                Some(Reason::NoTools)
            } else if super::side_reading(thread).is_some()
                && !super::definitions::hints_of(*tool).read_only
            {
                Some(Reason::ReadOnly)
            } else if !special && !super::definitions::BUILT.contains(tool) {
                Some(Reason::NotBuilt)
            } else if *tool == OrchTool::Goal {
                Some(Reason::GoalRequired)
            } else if service || (enabled && !special) {
                Some(Reason::CapabilityUnavailable)
            } else {
                Some(Reason::OffHere)
            };
            OrchToolFact {
                key: tool.name().to_owned(),
                offered,
                reason,
                source: if special { None } else { inputs.source.clone() },
            }
        })
        .collect();
    let revision = orch_types::Hash::of(serde_json::json!({ "offered": actual, "enabled": inputs.enabled, "source": inputs.source, "at": at }).to_string().as_bytes()).to_string();
    AcceptedOrchTools {
        summary: OrchToolsSummary {
            session_revision: revision,
            offered_count: actual.len() as u64,
            accepted_at: at,
        },
        tools,
    }
}
