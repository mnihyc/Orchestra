//! Exact native input-prefix work under the existing thread command key and policy.

use crate::commands::{SessionOpen, SessionOpenResult};
use orch_agent_contract::history::{NativeInputPrefix, NativePrefixOpened, NativePrefixSource};
use orch_types::Id;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Inspection creates no native/model input and attests the actual stored input identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionPrefixInspect {
    /// The catalogue's ordinary serial key; must equal source.thread_id.
    pub thread_id: Id,
    pub provider_id: Id,
    pub agent_id: Id,
    pub source: NativePrefixSource,
    /// Actual native item/client identity; never a global/display ordinal or matched text.
    pub before_input_ref: String,
}

/// Source and applied token are kept in the durable result before execution selection changes.
pub type SessionPrefixInspectResult = NativeInputPrefix;

/// Opens a distinct prefix session, with exactly the policy/configuration of an ordinary open.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionPrefixOpen {
    /// Must equal both session.thread_id and prefix.source.thread_id.
    pub thread_id: Id,
    pub session: Box<SessionOpen>,
    pub prefix: NativeInputPrefix,
}

pub type SessionPrefixOpenResult = NativePrefixOpened;

/// Observes/resumes one retained idle lane without autonomous goal continuation or model input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionContinuationResume {
    /// Must equal session.thread_id and source.thread_id.
    pub thread_id: Id,
    pub session: Box<SessionOpen>,
    pub source: NativePrefixSource,
    /// The unique provisional segment owned by this operation, when undo replaces it.
    /// Any other currently open generation refuses the resume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace_segment_id: Option<Id>,
    /// A retained source temporarily installed by this operation; both its native and segment identity must match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace_retained_source: Option<NativePrefixSource>,
}

pub type SessionContinuationResumeResult = SessionOpenResult;

/// Cleanup names the exact lane, so an old job cannot close a subsequently selected session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionContinuationClose {
    /// Must equal source.thread_id.
    pub thread_id: Id,
    pub provider_id: Id,
    pub agent_id: Id,
    pub source: NativePrefixSource,
}

/// Starts on the exact prepared lane; a late command cannot drive a replaced generation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TurnStartFenced {
    /// Must equal turn.thread_id.
    pub thread_id: Id,
    pub segment_id: Id,
    pub turn: Box<crate::commands::TurnStart>,
}

/// Ordinary lifecycle close still interrupts if needed, but cannot close a successor segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionCloseFenced {
    pub thread_id: Id,
    pub segment_id: Id,
}

/// Explicit replacement captures its predecessor; ordinary refreshes cannot replace a later lane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionOpenFenced {
    pub thread_id: Id,
    pub session: Box<SessionOpen>,
    /// None requires an empty target lane; Some permits only this exact idle predecessor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predecessor: Option<NativePrefixSource>,
}
