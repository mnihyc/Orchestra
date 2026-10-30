//! Reviewed reparenting of a main conversation while its owner, native history and workspace stay.

use crate::environment::EnvironmentRef;
use crate::nodes::NodeMovePlan;
use crate::routes::Access::Rule;
use crate::threads::Thread;
use crate::users::UserRef;
use orch_types::{Id, Revision};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What stays with the same conversation when its topic changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadMoveUnchanged {
    /// Its first owner stays.
    pub owner: UserRef,
    /// Its workspace environment stays, including its home and files.
    pub environment: Option<EnvironmentRef>,
    /// Its workspace directory stays.
    pub directory: String,
}

/// The existing node-move review with exact thread revision and a stored expiry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadMovePlan {
    /// Access, inherited settings, roles and blockers use the ordinary move plan.
    #[serde(flatten)]
    pub movement: NodeMovePlan,
    /// The exact source revision the person reviewed.
    pub expected_revision: Revision,
    /// Owner and workspace identities this act preserves.
    pub unchanged: ThreadMoveUnchanged,
}

/// The destination another topic of this same root tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadMovePreviewBody {
    /// The destination topic.
    pub parent_id: Id,
}

/// Confirmation of one stored, unchanged review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadMoveBody {
    /// The exact destination the person reviewed.
    pub parent_id: Id,
    /// The displayed source revision.
    pub expected_revision: Revision,
    /// The same actor's stored access and settings review.
    pub preflight_id: Id,
}

crate::endpoints! {
    /// Reviews moving this main thread to another topic without changing its workspace.
    ThreadMovePreview: POST "/threads/{id}/move/preview", Rule("thread.manage at the source and topic.interact at the destination, within the same root tree"), none, json ThreadMovePreviewBody, json ThreadMovePlan, 200;
    /// Moves the reviewed idle conversation with its side lanes and held queue.
    ThreadMove: POST "/threads/{id}/move", Rule("the current source and destination rights and the actor's unchanged stored review"), none, json ThreadMoveBody, json Thread, 200;
}
