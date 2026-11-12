//! Moving a project, or an org that lies in another, under another org of its own tree, and the plan that says what the move would change before anything does.
//!
//! A move changes the parent of what is moved and nothing else of it: its
//! owners, its page and everything inside it stay, and so do its members.
//! What changes is what it inherits, which the plan lists: the settings that
//! resolve differently there, how many people start or stop to hold a role
//! in it from above, and whose address changes. Who may move is the engine's
//! to say, at both ends; what is checked here is the shape of the tree: the
//! tree stays one, an org or a project never goes into itself or below
//! itself, the depth the system allows of orgs and of projects one inside
//! another, and the tree not being flat. A project goes under an org or
//! under another project, a topic to a project or a folder, with the rows
//! of the ledger and of both rollups that name it following it, and not
//! while a fork, a switch or a move holds one of its threads.

pub use super::NodeKind;

pub use super::actor_name;

pub use super::shown;

pub use orch_api_contract::common::ErrorCode;

use orch_db::queries::hierarchy::NodeRow;

pub use orch_types::Id;
/// What stops a move for the shape of the tree.
#[derive(Debug, Clone)]
pub struct Blocker {
    pub code: ErrorCode,
    pub message: String,
    pub parent: Option<Id>,
    pub limit: Option<u32>,
    /// A thread that is held, and what it is doing.
    pub held: Option<(Id, String, String)>,
}

/// A move as it was found to be: both ends read, and what stops it.
pub struct Plan {
    pub moved: NodeRow,
    pub from: Option<NodeRow>,
    pub to: NodeRow,
    pub blockers: Vec<Blocker>,
}

orch_api_base::api_family_callback! {
    bind __api_bind_ecca30487d, __api_install_ecca30487d, __api_callback_ecca30487d;
    slot __ApiCallbackecca30487d, __API_CALLBACK_ECCA30487D, __api_set_ecca30487d;
    base orch_api_base::routes::nodes::moves;

    async fn plan_of<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: orch_api_contract::nodes::NodeKind, arg3: orch_types::Id, arg4: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::nodes::moves::Plan>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_ea3740bdc8, __api_install_ea3740bdc8, __api_callback_ea3740bdc8;
    slot __ApiCallbackea3740bdc8, __API_CALLBACK_EA3740BDC8, __api_set_ea3740bdc8;
    base orch_api_base::routes::nodes::moves;

    async fn plan_reply<'__api>(arg0: &'__api orch_api_base::registry::Ctx, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api orch_api_base::routes::nodes::moves::Plan) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::nodes::NodeMovePlan>;
}

#[path = "moves/thread.rs"]
pub mod thread;
