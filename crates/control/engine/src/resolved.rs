//! The settings a thread runs with, read once.
//!
//! A turn is set up from settings of the thread's place: its model and
//! effort, the parameters of its agent, its instructions, its approval mode
//! and how far its sandbox may reach, the models it may use, how messages
//! are attributed, the arrangement types a scope prefers or allows. They are
//! read together ([`load`]) in the few statements the settings package needs
//! for any number of keys, and kept as typed values.

use crate::error::EngineError;
use orch_agent_contract::descriptor::{ApprovalMode, ArrangementType, SandboxMode};
use orch_agent_contract::session::MemorySetting;
use orch_db::Conn;
use orch_settings::{Place, Snapshot, Wanted};
use orch_settings_contract::provenance::SourceScope;
use orch_settings_contract::registry::{self, SettingDef, keys};
use orch_types::{Id, Json};
use serde::de::DeserializeOwned;
use std::collections::BTreeMap;

/// How many people must have spoken in a turn's inputs before the setting
/// `auto` names who said what: one person alone needs no name, and two
/// already need telling apart.
pub const AUTHORS_TO_ATTRIBUTE: i64 = 2;

/// How the author of a message is shown to the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attribution {
    /// Only when two people or more have taken part.
    Auto,
    /// Always.
    Always,
    /// Never.
    Never,
}

impl Attribution {
    /// Whether messages are attributed when `authors` people took part.
    #[must_use]
    pub const fn applies(self, authors: i64) -> bool {
        match self {
            Self::Always => true,
            Self::Never => false,
            Self::Auto => authors >= AUTHORS_TO_ATTRIBUTE,
        }
    }

    /// Whether the texts Orch composes for the agent may name a person at all. Under `never` they name nobody: who
    /// stopped a turn, who denied a request, who wrote or forwarded words is "a person" (or "an approver"); under
    /// `auto` and `always` they name the person as the place shows them.
    #[must_use]
    pub const fn names_people(self) -> bool {
        !matches!(self, Self::Never)
    }
}

/// How the author of a message is shown, as a resolution says it.
fn attribution_of(snapshot: &Snapshot) -> Result<Attribution, EngineError> {
    Ok(
        match value::<String>(snapshot, keys::COLLAB_ATTRIBUTION)?.as_deref() {
            Some("always") => Attribution::Always,
            Some("never") => Attribution::Never,
            _ => Attribution::Auto,
        },
    )
}

/// How the author of a message is shown at a node, read alone: for a text composed outside a turn's setting up.
pub async fn attribution(conn: &mut Conn, node: Id) -> Result<Attribution, EngineError> {
    let wanted = [def(keys::COLLAB_ATTRIBUTION)?];
    let snapshot = Snapshot::load(conn, Place::Node(node), Wanted::Keys(&wanted)).await?;
    attribution_of(&snapshot)
}

/// The settings of a thread's place that setting up a turn needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    /// The model's routing name; none while no scope sets one.
    pub model: Option<String>,
    /// The effort the thread asks for.
    pub effort: Option<String>,
    /// The parameters of the agent, by key.
    pub params: Json,
    /// The smallest inherited native limits over the harness's own sub-agents.
    pub subagent_limits: orch_agent_contract::session::SubagentLimits,
    /// The features that are on, by name.
    pub features: BTreeMap<String, bool>,
    /// The standing instructions scope by scope, outermost first, those that say nothing left out.
    pub standing: Vec<StandingPart>,
    /// What `agent.memory` is set to.
    pub memory: MemorySetting,
    /// The texts Orch says to an agent that a scope replaced, by key.
    pub messages: BTreeMap<String, String>,
    /// When the harness asks before it acts.
    pub approval_mode: ApprovalMode,
    /// The modes a scope allows.
    pub allowed_modes: Vec<ApprovalMode>,
    /// What the harness's own sandbox is asked to allow.
    pub sandbox_mode: SandboxMode,
    /// The models a scope allows; none while no scope limits them.
    pub models_allowed: Option<Vec<String>>,
    /// How the author of a message is shown.
    pub attribution: Attribution,
    /// Where the agent starts, relative to the thread's directory.
    pub cwd: String,
    /// The arrangement types a scope puts first.
    pub binding_prefer: Vec<ArrangementType>,
    /// The arrangement types a scope allows; none while no scope limits them.
    pub binding_allowed: Option<Vec<ArrangementType>>,
    /// How many calls of Orch's tools the agent may make in a minute, as the guide to them says.
    pub tool_calls_per_minute: Option<i64>,
    /// How many times in an hour the agent may send to other threads, as the guide to the tools says.
    pub sends_per_hour: Option<i64>,
    /// What announcement choices the current place permits advertising in its schema.
    pub broadcast_agent_scope: String,
    pub broadcast_agent_options: Vec<String>,
}

/// One scope's standing instructions, as the agent is told them under a heading of their place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandingPart {
    /// The kind of place that set them.
    pub scope: SourceScope,
    /// The place, where it is a node.
    pub node: Option<Id>,
    /// What it says.
    pub text: String,
}

/// The standing instructions of a place scope by scope, outermost first, those that say nothing left out.
fn standing_from(snapshot: &Snapshot) -> Result<Vec<StandingPart>, EngineError> {
    Ok(snapshot
        .resolve(def(keys::AGENT_INSTRUCTIONS)?)
        .parts
        .into_iter()
        .filter_map(|part| {
            let text = part.value.as_str()?.trim();
            (!text.is_empty()).then(|| StandingPart {
                scope: part.source.scope,
                node: part.source.node,
                text: text.to_owned(),
            })
        })
        .collect())
}

/// The standing instructions of a place, read alone: what a thread there is told under the headings of its places.
pub async fn standing_parts(conn: &mut Conn, place: Id) -> Result<Vec<StandingPart>, EngineError> {
    let wanted = [def(keys::AGENT_INSTRUCTIONS)?];
    let snapshot = Snapshot::load(conn, Place::Node(place), Wanted::Keys(&wanted)).await?;
    standing_from(&snapshot)
}

fn def(key: &str) -> Result<&'static SettingDef, EngineError> {
    registry::get(key)
        .ok_or_else(|| EngineError::unreadable(format!("the setting {key} is not in the registry")))
}

fn value<T: DeserializeOwned>(snapshot: &Snapshot, key: &str) -> Result<Option<T>, EngineError> {
    match snapshot.resolve(def(key)?).value {
        None | Some(Json::Null) => Ok(None),
        Some(found) => serde_json::from_value(found).map(Some).map_err(|error| {
            EngineError::unreadable(format!(
                "the value of {key} is not what it should be: {error}"
            ))
        }),
    }
}

fn word<T: DeserializeOwned>(snapshot: &Snapshot, key: &str) -> Result<Option<T>, EngineError> {
    value::<String>(snapshot, key)?
        .map(|text| {
            serde_json::from_value(Json::String(text)).map_err(|error| {
                EngineError::unreadable(format!(
                    "the value of {key} is not one of its words: {error}"
                ))
            })
        })
        .transpose()
}

/// The settings a thread runs with.
pub async fn load(conn: &mut Conn, thread: Id) -> Result<Resolved, EngineError> {
    let wanted: Vec<&'static SettingDef> = [
        keys::AGENT_MODEL,
        keys::AGENT_EFFORT,
        keys::AGENT_PARAMS,
        keys::AGENT_FEATURES,
        keys::AGENT_INSTRUCTIONS,
        keys::AGENT_MEMORY,
        keys::AGENTS_SUBAGENTS_AT_ONCE,
        keys::AGENTS_SUBAGENT_DEPTH,
        keys::AGENT_MESSAGES,
        keys::APPROVAL_MODE,
        keys::APPROVAL_ALLOWED_MODES,
        keys::SANDBOX_MODE,
        keys::MODELS_ALLOWED,
        keys::COLLAB_ATTRIBUTION,
        keys::THREAD_CWD,
        keys::BINDING_PREFER,
        keys::BINDING_ALLOWED,
        keys::ORCH_TOOLS_CALLS_PER_MINUTE,
        keys::BROADCAST_RATE_PER_HOUR,
        keys::BROADCAST_AGENT_SCOPE,
        keys::BROADCAST_AGENT_OPTIONS,
    ]
    .into_iter()
    .map(def)
    .collect::<Result<_, _>>()?;
    let snapshot = Snapshot::load(conn, Place::Node(thread), Wanted::Keys(&wanted)).await?;
    let attribution = attribution_of(&snapshot)?;
    let mut resolved = Resolved {
        model: value(&snapshot, keys::AGENT_MODEL)?,
        effort: value(&snapshot, keys::AGENT_EFFORT)?,
        params: value(&snapshot, keys::AGENT_PARAMS)?
            .unwrap_or_else(|| Json::Object(serde_json::Map::new())),
        subagent_limits: orch_agent_contract::session::SubagentLimits {
            at_once: value(&snapshot, keys::AGENTS_SUBAGENTS_AT_ONCE)?.unwrap_or(8),
            depth: value(&snapshot, keys::AGENTS_SUBAGENT_DEPTH)?.unwrap_or(1),
        },
        features: value(&snapshot, keys::AGENT_FEATURES)?.unwrap_or_default(),
        standing: standing_from(&snapshot)?,
        memory: word(&snapshot, keys::AGENT_MEMORY)?.unwrap_or_default(),
        messages: value(&snapshot, keys::AGENT_MESSAGES)?.unwrap_or_default(),
        approval_mode: word(&snapshot, keys::APPROVAL_MODE)?.unwrap_or(ApprovalMode::OnRequest),
        allowed_modes: value::<Vec<String>>(&snapshot, keys::APPROVAL_ALLOWED_MODES)?
            .map(|modes| {
                modes
                    .into_iter()
                    .filter_map(|mode| serde_json::from_value(Json::String(mode)).ok())
                    .collect()
            })
            .unwrap_or_else(|| {
                vec![
                    ApprovalMode::Ask,
                    ApprovalMode::OnRequest,
                    ApprovalMode::Auto,
                    ApprovalMode::Bypass,
                ]
            }),
        sandbox_mode: word(&snapshot, keys::SANDBOX_MODE)?.unwrap_or(SandboxMode::Workspace),
        models_allowed: value(&snapshot, keys::MODELS_ALLOWED)?,
        attribution,
        cwd: value(&snapshot, keys::THREAD_CWD)?.unwrap_or_default(),
        binding_prefer: types(&snapshot, keys::BINDING_PREFER)?.unwrap_or_default(),
        binding_allowed: types(&snapshot, keys::BINDING_ALLOWED)?,
        tool_calls_per_minute: value(&snapshot, keys::ORCH_TOOLS_CALLS_PER_MINUTE)?,
        sends_per_hour: value(&snapshot, keys::BROADCAST_RATE_PER_HOUR)?,
        broadcast_agent_scope: value(&snapshot, keys::BROADCAST_AGENT_SCOPE)?
            .unwrap_or_else(|| "topic".to_owned()),
        broadcast_agent_options: value(&snapshot, keys::BROADCAST_AGENT_OPTIONS)?
            .unwrap_or_default(),
    };
    crate::calls::apply_override(conn, thread, &mut resolved).await?;
    Ok(resolved)
}

fn types(snapshot: &Snapshot, key: &str) -> Result<Option<Vec<ArrangementType>>, EngineError> {
    Ok(value::<Vec<String>>(snapshot, key)?.map(|names| {
        names
            .into_iter()
            .filter_map(|name| serde_json::from_value(Json::String(name)).ok())
            .collect()
    }))
}

/// Whether the routing name `model` is named by one of `patterns`, which is the one rule of every pattern
/// that names models ([`crate::models::allowed_by`]); here for the readers that ask it of a resolved
/// allowlist.
#[must_use]
pub fn model_matches(patterns: &[String], model: &str, groups: &[String]) -> bool {
    crate::models::allowed_by(patterns, model, groups)
}
