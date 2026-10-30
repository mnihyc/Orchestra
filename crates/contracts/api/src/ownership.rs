//! What ending a membership touches and where each thing goes, a member's domain and the move that changes it with what they lose, gain and keep, the deletion of an org that lies in another, and what was deleted and can be restored.
//!
//! A person belongs to a tree of orgs by one org, their domain. Ending the
//! membership, or moving the domain, changes who reaches what, so both are
//! planned first: [`Holdings`] lists everything of the person's that ending
//! the membership touches, and [`MemberMovePlan`] says what a move would
//! lose, gain and leave alone. What is deleted stays for a while and can be
//! brought back; so can ownership be handed to another person.

use std::collections::BTreeMap;

use orch_types::{Id, Revision, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::access::{MissingAccess, ObjectRef, Role, RoleHeld};
use crate::common::{Page, PageQuery};
use crate::directory::Member;
use crate::nodes::{Node, NodeKind, NodeOrSystem, NodeRef};
use crate::resources::Resource;
use crate::routes::Access::{Permission, Rule, User};
use crate::users::UserRef;

pub use crate::resources::{MachineOs, MachineRef};

/// How many nodes of each kind a person owns alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HoldingsNodes {
    /// Orgs.
    pub orgs: i64,
    /// Projects.
    pub projects: i64,
    /// Topics.
    pub topics: i64,
    /// Threads.
    pub threads: i64,
    /// Documents.
    pub documents: i64,
}

/// A resource that a person manages and owns alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HoldingsResource {
    /// What it is.
    pub object: ObjectRef,
    /// How many others it is shared with.
    pub shared_with: i64,
    /// How many threads use it.
    pub threads: i64,
    /// Whether it is a subscription that this person signed in.
    pub personal_sign_in: bool,
}

/// A resource registered from a machine of the person's own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HoldingsMachineRegistration {
    /// What was registered.
    pub object: ObjectRef,
    /// The machine it was registered from.
    pub machine: MachineRef,
    /// How many threads use it.
    pub threads: i64,
}

/// What a person owns with others, by kind: left to the others when their membership ends.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HoldingsShared {
    /// Orgs.
    pub orgs: i64,
    /// Projects.
    pub projects: i64,
    /// Topics.
    pub topics: i64,
    /// Threads.
    pub threads: i64,
    /// Documents.
    pub documents: i64,
    /// Resources.
    pub resources: i64,
}

/// Everything of a person's in a tree that ending their membership would touch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Holdings {
    /// What they own alone. Each of these goes to the first owner of the org it lies in.
    pub nodes: HoldingsNodes,
    /// What they own with others: left to the others.
    pub shared: i64,
    /// What they own with others, by kind; together it is `shared`.
    pub shared_by_kind: HoldingsShared,
    /// Resources they manage and own alone: kept, disabled or removed.
    pub resources: Vec<HoldingsResource>,
    /// What was registered from their own machines: unregistered.
    pub machine_registrations: Vec<HoldingsMachineRegistration>,
    /// How many roles they hold and entries name them; all of these go.
    pub entries: i64,
    /// Their pending requests, which go too.
    pub pending: i64,
}

/// What a person reaches, counted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemberMoveReach {
    /// Nodes.
    pub nodes: i64,
    /// Resources.
    pub resources: i64,
    /// Documents.
    pub documents: i64,
}

/// What a move of a person's domain leaves alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemberMoveStays {
    /// What they own in the place they are leaving. It stays there, still theirs.
    pub nodes: HoldingsNodes,
    /// Resources of theirs that have that place as their home.
    pub resources: i64,
    /// Roles given to them by name elsewhere, which a move does not touch.
    pub roles: Vec<RoleHeld>,
    /// Entries that name them personally, which a move does not touch either.
    pub named: i64,
}

/// What changing a member's domain would do, before anything does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemberMovePlan {
    /// The member.
    pub user: UserRef,
    /// Their domain now.
    pub from: NodeRef,
    /// The org that would be their domain.
    pub to: NodeRef,
    /// The role they would hold there.
    pub role: Role,
    /// What they reach today through the role at `from` and through nothing else.
    pub loses: MemberMoveReach,
    /// What the role at `to` would give them access to; it is all inherited, nothing is named.
    pub gains: MemberMoveReach,
    /// What no move touches.
    pub stays: MemberMoveStays,
    /// Threads of theirs that depend on something only the old role allowed; an input with no
    /// author would have to wait there.
    pub waiting: Vec<NodeRef>,
    /// What the caller lacks for the move and who holds it; an empty list means the caller may.
    pub missing: Vec<MissingAccess>,
}

/// What becomes of a resource that a leaving member owned alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResourceFate {
    /// It stays, and goes to the first owner of the org it lies in.
    Keep,
    /// It stays, switched off.
    Disable,
    /// It is removed.
    Remove,
}

/// Ends a membership. The body may be left out.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OrgMemberRemoveBody {
    /// What happens to each managed resource that the person owns alone there, keyed by the
    /// resource's identifier. Any left out is kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<BTreeMap<String, ResourceFate>>,
}

/// Moves a member's domain, or shows what the move would change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemberMoveBody {
    /// The org to move to. It belongs to the same root org, being that root or an org inside it.
    pub to: Id,
    /// The role to hold there; the one they hold at their domain when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
}

/// What members are given when the org they were in is deleted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeDeleteMembers {
    /// The role given on the org above when a member's domain is moved there;
    /// a viewer when left out. It needs `members.manage` on both.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
}

/// What becomes of the resources whose home is a deleted org.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeletedResources {
    /// They are switched off with it.
    Disable,
    /// They are given to the org above.
    ToParent,
}

/// Deletes a node, with everything in it. The members and resources of an
/// org that lies in another are handled as this says. The body may be left out.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeDeleteBody {
    /// For an org that lies in another: what members are given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub members: Option<NodeDeleteMembers>,
    /// For such an org: what becomes of its resources; they are disabled when
    /// left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<DeletedResources>,
    /// The plan the caller confirmed (`POST /nodes/{id}/delete/preview`): the deletion removes
    /// exactly what it counted, or is refused as stale with the plan as it now is. Left out, the
    /// deletion keeps its ordinary authority and subtree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preflight_id: Option<Id>,
    /// The node's revision as that plan showed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<Revision>,
}

/// What a deletion did: the group of everything it deleted, which its Undo names
/// (`POST /nodes/{id}/restore` with `undo_delete`) to put back how each of them stood.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeDeleted {
    /// The deletion group; `null` for a side thread that was closed.
    pub deletion_id: Option<Id>,
}

/// How many of each kind deleting a node removes, itself among them, and the documents beside them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeDeleteCounts {
    /// Orgs.
    pub orgs: i64,
    /// Projects, folders among them.
    pub projects: i64,
    /// Topics.
    pub topics: i64,
    /// Threads.
    pub threads: i64,
    /// Documents.
    pub documents: i64,
}

/// Who may bring a deleted node back: the permission, and the place it is held on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RestoreBy {
    /// The permission: the manage permission of the parent's kind, or `access.own` for a root org.
    pub permission: String,
    /// Where it is held: the deleted node's parent, the root org itself, or the system.
    pub place: NodeOrSystem,
}

/// What deleting a node would remove, until when it can be brought back and by whom, kept for a few
/// minutes as a plan that the deletion names: a changed subtree, or a later last day, asks again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeDeletePlan {
    /// The plan, for the deletion to name.
    pub preflight_id: Id,
    /// When it goes out of date.
    pub expires_at: Time,
    /// The node.
    pub node: NodeRef,
    /// Its revision.
    pub revision: Revision,
    /// What goes with it, counted.
    pub counts: NodeDeleteCounts,
    /// When what is deleted now is gone for good.
    pub purge_at: Time,
    /// Who may bring it back until then.
    pub restore_by: RestoreBy,
}

/// A node that was deleted directly under another and is not purged yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeletedNode {
    /// What was deleted.
    pub node: NodeRef,
    /// Its kind.
    pub kind: NodeKind,
    /// When it was deleted.
    pub deleted_at: Time,
    /// Who deleted it.
    pub deleted_by: Option<UserRef>,
    /// When it will be gone for good.
    pub purge_at: Time,
    /// How many nodes lay directly in it.
    pub children: i64,
}

/// A team root org that was deleted and is not purged yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeletedOrg {
    /// The org.
    pub org: NodeRef,
    /// Its short name.
    pub slug: String,
    /// When it was deleted.
    pub deleted_at: Time,
    /// Who deleted it.
    pub deleted_by: Option<UserRef>,
    /// When it will be gone for good.
    pub purge_at: Time,
}

/// Hands the first place among the owners of a thing to another person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TransferBody {
    /// An active member of the root org. They are named in the Owners row and
    /// are first in it from now on.
    pub user_id: Id,
    /// Whether the person who was first stays an owner; they do when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_previous: Option<bool>,
}

/// Restores the current group, or refuses an older audit selection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeRestoreBody {
    /// Undo the matching newly deleted group's captured archive states.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo_delete: Option<Id>,
    /// The deletion group selected from the audit event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_deletion_id: Option<Id>,
}

crate::endpoints! {
    /// Ends a membership of a root org, with everything that follows from it.
    OrgMemberRemove: DELETE "/orgs/{id}/members/{user_id}", Rule("members.manage on an org that is the person's domain or lies above it, or the member themself"), none, json OrgMemberRemoveBody, none, 204;
    /// Lists everything of a person's in a tree that ending their membership would touch.
    OrgMemberHoldings: GET "/orgs/{id}/members/{user_id}/holdings", Rule("the same as ending the membership"), none, none, json Holdings, 200;
    /// Shows what changing a member's domain would do, before anything does.
    OrgMemberMovePreview: POST "/orgs/{id}/members/{user_id}/move/preview", Rule("the same as the move itself"), none, json MemberMoveBody, json MemberMovePlan, 200;
    /// Changes a member's domain, carrying the role they held along.
    OrgMemberMove: POST "/orgs/{id}/members/{user_id}/move", Rule("members.manage on the org that is the person's domain and on the org they move to"), none, json MemberMoveBody, json Member, 200;
    /// Says what deleting a node would remove, until when and by whom it can be brought back, and keeps that as a plan for the deletion to name; nothing is deleted.
    NodeDeletePreview: POST "/nodes/{id}/delete/preview", Rule("the same as deleting the node"), none, none, json NodeDeletePlan, 200;
    /// Deletes a node with everything in it, archiving it first when it is not.
    NodeDelete: DELETE "/nodes/{id}", Rule("node manager; side thread maker, parent manager or environment manager"), none, json NodeDeleteBody, json NodeDeleted, 200;
    /// Lists what was deleted directly under a node and can still be brought back.
    NodeDeletedList: GET "/nodes/{id}/deleted", Permission("node.manage"), query PageQuery, none, json Page<DeletedNode>, 200;
    /// Brings a deleted node back, archived, with everything under it.
    NodeRestore: POST "/nodes/{id}/restore", Rule("node.manage on the deleted node's parent; for a root org, one of its owners or a system admin"), none, json NodeRestoreBody, json Node, 200;
    /// Lists the deleted team root orgs of the caller that can still be restored.
    OrgDeletedList: GET "/orgs/deleted", User, none, none, json Vec<DeletedOrg>, 200;
    /// Makes another person the first owner of a node.
    NodeTransfer: POST "/nodes/{id}/transfer", Rule("one of the node's owners, or an admin of the place it is in"), none, json TransferBody, json Node, 200;
    /// Makes another person the first owner of a resource or a document.
    ObjectTransfer: POST "/{object}/{id}/transfer", Rule("one of the resource's owners, or an admin of its home"), none, json TransferBody, json Resource, 200;
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_restore_selection_carries_the_event_identity_and_refuses_a_timestamp_guard() {
        let id = orch_types::new_id();
        let body: super::NodeRestoreBody =
            serde_json::from_value(serde_json::json!({"expected_deletion_id":id})).unwrap();
        assert_eq!(body.expected_deletion_id, Some(id));
        assert!(
            serde_json::from_value::<super::NodeRestoreBody>(
                serde_json::json!({"expected_deleted_at":"2026-10-13T00:00:00Z"})
            )
            .is_err()
        );
    }
    use super::*;
    use serde_json::json;

    #[test]
    fn a_leaving_member_says_what_becomes_of_each_resource_and_may_say_nothing() {
        let body: OrgMemberRemoveBody = serde_json::from_value(json!({
            "resources": {
                "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b": "disable",
                "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6c": "remove"
            }
        }))
        .unwrap();
        let fates = body.resources.as_ref().unwrap();
        assert_eq!(
            fates["0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b"],
            ResourceFate::Disable
        );
        assert_eq!(
            serde_json::from_value::<OrgMemberRemoveBody>(json!({})).unwrap(),
            OrgMemberRemoveBody::default()
        );
        assert!(
            serde_json::from_value::<OrgMemberRemoveBody>(json!({ "resources": { "x": "burn" } }))
                .is_err()
        );
    }

    #[test]
    fn deleting_a_department_chooses_what_members_and_resources_get() {
        let body: NodeDeleteBody = serde_json::from_value(
            json!({ "members": { "role": "developer" }, "resources": "to_parent" }),
        )
        .unwrap();
        assert_eq!(
            body.members.and_then(|members| members.role),
            Some(Role::Developer)
        );
        assert_eq!(body.resources, Some(DeletedResources::ToParent));
    }

    #[test]
    fn keeping_the_previous_owner_is_the_default_of_a_transfer() {
        let body: TransferBody =
            serde_json::from_value(json!({ "user_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b" }))
                .unwrap();
        assert_eq!(body.keep_previous, None);
        assert!(
            serde_json::from_value::<TransferBody>(
                json!({ "user_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b", "keep": false })
            )
            .is_err()
        );
    }
}
