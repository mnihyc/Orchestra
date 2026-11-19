//! What Orch supplied to a model, as it was sent: the text a session was opened with and each input's text, block by
//! block, recorded before it was sent and marked once its sending ended.
//!
//! A person keeps the original words of the conversation, and can read beside them what the agent was given: the
//! preamble, the guide to the tools and the standing instructions a session was opened with; where the agent is,
//! what a side conversation is, why a turn before was cut, what waited and who the earlier senders were, and the
//! words themselves, as each input carried them. A record is written before what it holds is sent, and the sending
//! is not made when the record cannot be: a model is never given text nobody can read afterwards. Reading needs what
//! reading the node needs (a thread's view, which for a side thread is its parent's), and never managing it; the
//! record holds no credential, header or file of a person's home. Reading changes nothing: no read mark, no audit
//! row, no notification.

use orch_types::{Id, Seq, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::Page;
use crate::routes::Access::Permission;
use crate::threads::OutputRef;

/// One block of the text a model was given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelTextBlock {
    /// What the block is: the key of its text in `agent.messages` (`preamble`, `where_you_are`, `turn_stopped`, …),
    /// or `input` for the words of an input, `standing` for the standing instructions.
    pub key: String,
    /// Where it comes from and what it is for, in a few words; never a transport that carries a secret.
    pub source: String,
    /// Exactly the text supplied: a bounded inline tail and, for long text, the full blob under this node's view permission.
    pub text: OutputRef,
}

/// What a record was given for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModelInputPurpose {
    /// What a session was opened with.
    Session,
    /// An input: what a turn was started with, or a steer.
    Input,
    /// What a tool was described with.
    Tool,
    /// What a summary was asked with.
    Summary,
}

/// How the sending of a record stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModelInputState {
    /// Recorded, and being sent.
    Prepared,
    /// The harness took it.
    Sent,
    /// The harness refused it, or it could not be sent.
    Failed,
    /// Whether it arrived is not known: the answer never came.
    Uncertain,
}

/// One record of what a model was given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelInput {
    /// Its identifier.
    pub id: Id,
    /// The node it was given for: the thread, or the place of a summary.
    pub node_id: Id,
    /// The segment of the thread whose session it went to.
    pub segment_id: Option<Id>,
    /// The turn it started or went into.
    pub turn_id: Option<Id>,
    /// The input whose words it carried.
    pub input_id: Option<Id>,
    /// The item of the timeline that stands for it.
    pub item_id: Option<Id>,
    /// What it was given for.
    pub purpose: ModelInputPurpose,
    /// The delivery it belongs to: the same for every attempt at one.
    pub reference: String,
    /// Its blocks, in the order they were given.
    pub blocks: Vec<ModelTextBlock>,
    /// How its sending stands.
    pub state: ModelInputState,
    /// When it was recorded.
    pub created_at: Time,
    /// When the harness took it.
    pub sent_at: Option<Time>,
    /// The sequence of the thread when it was recorded.
    pub seq_created: Seq,
    /// The sequence of the thread when it last changed.
    pub seq_updated: Seq,
}

/// Which records to read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelInputListQuery {
    /// One record, by its identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    /// The records of one turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<Id>,
    /// Those recorded before this sequence: the page before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Seq>,
    /// Those recorded after this sequence: the page after.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Seq>,
    /// How many: 50 when left out, at most 200.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
}

crate::endpoints! {
    /// Reads what Orch supplied to a model for a node, as it was sent: the newest first, a page at a time (`next_cursor` is the `before` of the next page).
    NodeModelInputList: GET "/nodes/{id}/model-inputs", Permission("node.view"), query ModelInputListQuery, none, json Page<ModelInput>, 200;
    /// Reads a long block of one model-input record under the same node's current view permission.
    NodeModelInputBlobGet: GET "/nodes/{id}/model-inputs/{model_input_id}/blobs/{hash}", Permission("node.view"), none, none, raw, 200;
}
