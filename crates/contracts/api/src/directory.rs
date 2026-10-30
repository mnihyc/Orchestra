//! Org groups, members with their domain, their roles, where each is held and suspensions, service identities and labels.
//!
//! An org keeps its own people: its [`Member`]s, whose domain is that org or
//! one inside it, its [`Group`]s, and its service identities for automation.
//! A member can be suspended, which closes the org and everything inside it to
//! them without removing anything of theirs. Tags are the labels that topics
//! carry, defined by an org or a project.

use orch_types::{Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::access::Role;
use crate::access::{Holder, HolderInput};
use crate::auth::{ApiTokenCreateBody, ApiTokenInfo, TokenCreated};
use crate::common::Page;
use crate::nodes::{NodeRef, Tag};
use crate::routes::Access::{Permission, Rule};
use crate::users::UserRef;

/// A group of people of one root org, kept by an org at any level.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Group {
    /// The group's identifier.
    pub id: Id,
    /// The org that keeps it.
    pub org_id: Id,
    /// The visible path of the org that keeps it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place: Option<String>,
    /// Its name.
    pub name: String,
    /// What it is for.
    pub description: String,
    /// How many people are in it.
    pub member_count: i64,
}

/// A member of a tree of orgs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Member {
    /// The person.
    pub user: UserRef,
    /// The single org of the tree that they belong to.
    pub domain: NodeRef,
    /// The short names down to their domain, followed by their username.
    pub address: String,
    /// The role they hold at their domain.
    pub role: Role,
    /// Whether they are one of the owners of the org that was asked about.
    pub is_owner: bool,
    /// Since when they have been a member.
    pub since: Option<Time>,
    /// How many roles they hold by name in the rest of the tree, apart from the one at their
    /// domain.
    pub also: i64,
    /// Whether the admins of this org hold them out of it; shown only to those
    /// who manage members.
    pub suspended: bool,
    /// How their sign-in stands with a second step; `null` for whoever does not manage this
    /// org's members, and for a service identity, which has no human second step.
    pub second_step: Option<MemberSecondStep>,
    /// Whether an org of theirs requires a second step of them; `null` as `second_step` is.
    pub second_step_required: Option<bool>,
    /// When they last used something inside this org; `null` where that is not known, and for
    /// whoever does not manage this org's members.
    pub last_seen_at: Option<Time>,
}

/// How a member's sign-in stands with a second step, as the org's managers read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemberSecondStep {
    /// They set up a second proof here: an authenticator, or a mailed code.
    Enabled,
    /// They set none up.
    Missing,
    /// They sign in through an identity provider and not by a password here; that says how their
    /// first proof is made, and nothing of a second.
    Sso,
}

/// A member, or somebody let into a place, who is held out of an org.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Suspension {
    /// The person.
    pub user: UserRef,
    /// True when they are no member, but were let into a place here.
    pub outside: bool,
    /// Why, in the words of whoever suspended them.
    pub reason: String,
    /// Who suspended them.
    pub by: UserRef,
    /// When.
    pub at: Time,
    /// How many things of theirs stay theirs meanwhile.
    pub owns: i64,
}

/// An identity for automation that an org keeps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ServiceAccount {
    /// Who it is to others.
    pub user: UserRef,
    /// Who made it.
    pub created_by: Option<UserRef>,
    /// When it was made.
    pub created_at: Time,
    /// How many tokens it has.
    pub token_count: i64,
}

/// The members of an org, at whatever level it lies.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OrgMemberListQuery {
    /// Only those whose domain is that org itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direct: Option<bool>,
    /// Only those whose name or username holds this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    /// How many entries to return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The `next_cursor` of the page before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Suspends a member in an org.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OrgMemberSuspendBody {
    /// Why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// The groups an org keeps.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OrgGroupListQuery {
    /// Also the groups that the orgs inside it keep.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub under: Option<bool>,
}

/// Makes a group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OrgGroupCreateBody {
    /// Its name.
    pub name: String,
    /// What it is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Changes a group.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GroupUpdateBody {
    /// A new name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A new description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Makes a service identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OrgServiceAccountCreateBody {
    /// The username it works under.
    pub username: String,
    /// The name to show.
    pub display_name: String,
}

/// Makes a tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeTagCreateBody {
    /// Its name.
    pub name: String,
    /// Its colour, an index below [`crate::users::COLOR_COUNT`]; left out, the first of them that no
    /// tag of the place has yet, and after all of them the next in turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<u32>,
}

/// Changes a tag.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TagUpdateBody {
    /// A new name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A new colour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<u32>,
}

/// The thing a person from outside reaches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OutsideSourceObject {
    /// What kind of thing it is: `org`, `project`, `topic`, `thread`, `document`, or a kind of
    /// resource (`provider`, `environment`, …), as its permission page names it.
    pub kind: String,
    /// Its identifier.
    pub id: Id,
    /// Its name.
    pub name: String,
}

/// One way somebody from outside the root tree reaches a thing of an org: an entry of the thing's
/// page, or a role given at a place, naming them or a holder they belong to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OutsideSource {
    /// The thing.
    pub object: OutsideSourceObject,
    /// The row of its page (a permission), or `role:<role>` for a role given at a place.
    pub row: String,
    /// The holder as the row names it: `{ kind: "user" }` for the person themself; a group, an
    /// org's members or a role of another place, with its name (never the others it holds).
    pub holder: orch_types::Json,
    /// Who gave it; `null` for what was seeded.
    pub by: Option<UserRef>,
    /// When it was given.
    pub at: Time,
    /// When it ends; `null` for never.
    pub until: Option<Time>,
    /// It names the person themself, so that taking them out removes it.
    pub direct: bool,
    /// The reader may change it: a direct source on a page or at a place they manage.
    pub can_change: bool,
}

/// Somebody from outside the root tree who reaches something of an org now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OutsidePerson {
    /// The person.
    pub user: UserRef,
    /// When they last used something inside this org; `null` where that is not known.
    pub last_seen_at: Option<Time>,
    /// The first page of the ways they reach it.
    pub sources: Vec<OutsideSource>,
    /// Where the rest of the ways continue; `null` when that page was all of them.
    pub sources_cursor: Option<String>,
    /// The org's admins hold them out of it now.
    pub suspended: bool,
}

/// What taking somebody from outside out of an org would do, kept on the server for its caller to
/// read page by page and confirm once: the complete set of ways they reach it now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OutsideRemovalPlan {
    /// The plan, which the confirmation names.
    pub id: Id,
    /// When it may no longer be confirmed.
    pub expires_at: Time,
    /// The org.
    pub org: crate::nodes::NodeRef,
    /// The person.
    pub user: UserRef,
    /// A page of the ways they reach it: those that name them are removed; the others stay, and
    /// hold them out by a suspension instead.
    pub removes: Vec<OutsideSource>,
    /// Where the next page begins; `null` on the last.
    pub next_cursor: Option<String>,
    /// On the last page only: what the confirmation carries to say it read the whole plan.
    pub reviewed_fingerprint: Option<String>,
    /// A group, an org's members or a role of another place lets them in as well, so that they
    /// are held out of this org by the existing suspension.
    pub suspension_needed: bool,
    /// What the caller lacks to change the rows it removes, with who can give it; empty when they
    /// may.
    pub missing: Vec<crate::access::MissingAccess>,
}

/// What taking somebody from outside out of an org did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OutsideRemovalResult {
    /// The person.
    pub user: UserRef,
    /// How many entries naming them were removed.
    pub removed_entries: i64,
    /// How many roles given to them were taken away.
    pub removed_roles: i64,
    /// The suspension that holds them out, where one was needed.
    pub suspension: Option<Suspension>,
}

/// Pages the people from outside who reach something of an org.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OrgOutsiderListQuery {
    /// Only those whose name or username holds this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    /// How many people at most.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The `next_cursor` of the page before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Pages the ways one person from outside reaches an org.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OrgOutsiderSourceListQuery {
    /// How many at most.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The cursor where the page before ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Pages a stored removal plan.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OrgOutsiderRemovePlanGetQuery {
    /// The `next_cursor` of the page before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// How many at most.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Confirms a removal plan that was read whole.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OrgOutsiderRemoveBody {
    /// The plan.
    pub plan_id: Id,
    /// What its last page gave.
    pub reviewed_fingerprint: String,
    /// Why, for the suspension and the record.
    pub reason: String,
}

crate::endpoints! {
    /// Lists the people who belong to an org at any level, that is everybody who has that org, or
    /// one inside it, as their domain.
    OrgMemberList: GET "/orgs/{id}/members", Permission("node.view"), query OrgMemberListQuery, none, json Page<Member>, 200;
    /// Lists the people from outside the root tree who reach something of an org now, each with
    /// the first ways they do.
    OrgOutsiderList: GET "/orgs/{id}/outsiders", Permission("members.manage"), query OrgOutsiderListQuery, none, json Page<OutsidePerson>, 200;
    /// Pages the ways one person from outside reaches an org now.
    OrgOutsiderSourceList: GET "/orgs/{id}/outsiders/{user_id}/sources", Permission("members.manage"), query OrgOutsiderSourceListQuery, none, json Page<OutsideSource>, 200;
    /// Plans taking a person from outside out of an org, keeps the plan, and gives its first page.
    OrgOutsiderRemovePreview: POST "/orgs/{id}/outsiders/{user_id}/remove/preview", Permission("members.manage"), none, none, json OutsideRemovalPlan, 200;
    /// Gives the next page of a stored removal plan, to the caller who asked for it.
    OrgOutsiderRemovePlanGet: GET "/orgs/{id}/outsiders/{user_id}/remove/plans/{plan_id}", Rule("the caller who asked for the plan, with members.manage on the org now"), query OrgOutsiderRemovePlanGetQuery, none, json OutsideRemovalPlan, 200;
    /// Takes a person from outside out of an org as a plan read whole says: the entries and roles
    /// naming them go, and a suspension holds them out where something else lets them in.
    OrgOutsiderRemove: POST "/orgs/{id}/outsiders/{user_id}/remove", Permission("members.manage"), none, json OrgOutsiderRemoveBody, json OutsideRemovalResult, 200;
    /// Suspends a member in an org, which closes it and everything inside it to them at once.
    OrgMemberSuspend: POST "/orgs/{id}/members/{user_id}/suspend", Rule("members.manage; not for an owner of that org, for oneself or for a system admin"), none, json OrgMemberSuspendBody, json Suspension, 200;
    /// Lifts a suspension, so that everything is as it was.
    OrgMemberResume: POST "/orgs/{id}/members/{user_id}/resume", Permission("members.manage"), none, none, none, 204;
    /// Lists everybody who is suspended in an org now.
    OrgSuspensionList: GET "/orgs/{id}/suspensions", Permission("members.manage"), none, none, json Vec<Suspension>, 200;
    /// Lists the groups an org keeps.
    OrgGroupList: GET "/orgs/{id}/groups", Permission("node.view"), query OrgGroupListQuery, none, json Vec<Group>, 200;
    /// The groups kept by the nearest org of a visible place, where that org is visible too.
    NodeGroupList: GET "/nodes/{id}/groups", Permission("node.view"), none, none, json Vec<Group>, 200;
    /// Makes a group in an org.
    OrgGroupCreate: POST "/orgs/{id}/groups", Permission("members.manage"), none, json OrgGroupCreateBody, json Group, 200;
    /// Renames a group or changes what it is for.
    GroupUpdate: PATCH "/groups/{id}", Rule("members.manage on the org that keeps the group"), none, json GroupUpdateBody, json Group, 200;
    /// Deletes a group, together with the roles it holds and the entries naming it.
    GroupDelete: DELETE "/groups/{id}", Rule("members.manage on the org that keeps the group"), none, none, none, 204;
    /// Lists the people in a group.
    GroupMemberList: GET "/groups/{id}/members", Rule("node.view on the org that keeps the group"), none, none, json Vec<UserRef>, 200;
    /// Direct members as holders, retaining nested groups and source places.
    GroupHolderList: GET "/groups/{id}/holders", Rule("node.view on the org that keeps the group"), none, none, json Vec<Holder>, 200;
    /// Adds a holder; nested group cycles and paths deeper than five groups are refused.
    GroupHolderAdd: PUT "/groups/{id}/holders", Rule("members.manage on the org that keeps the group"), none, json HolderInput, none, 204;
    /// Removes only the named direct member, preserving every other path.
    GroupHolderRemove: DELETE "/groups/{id}/holders", Rule("members.manage on the org that keeps the group"), none, json HolderInput, none, 204;
    /// Puts a member of the group's root org into the group.
    GroupMemberAdd: PUT "/groups/{id}/members/{user_id}", Rule("members.manage on the org that keeps the group"), none, none, none, 204;
    /// Takes a person out of a group.
    GroupMemberRemove: DELETE "/groups/{id}/members/{user_id}", Rule("members.manage on the org that keeps the group"), none, none, none, 204;
    /// Lists the service identities an org keeps.
    OrgServiceAccountList: GET "/orgs/{id}/service-accounts", Permission("members.manage"), none, none, json Vec<ServiceAccount>, 200;
    /// Makes a service identity in an org.
    OrgServiceAccountCreate: POST "/orgs/{id}/service-accounts", Permission("members.manage"), none, json OrgServiceAccountCreateBody, json ServiceAccount, 200;
    /// Disables a service identity, revokes its tokens, and removes its roles and every entry
    /// naming it.
    ServiceAccountDelete: DELETE "/service-accounts/{id}", Rule("members.manage on the org that keeps the service identity"), none, none, none, 204;
    /// Lists the tokens of a service identity.
    ServiceAccountTokenList: GET "/service-accounts/{id}/tokens", Rule("members.manage on the org that keeps the service identity"), none, none, json Vec<ApiTokenInfo>, 200;
    /// Makes a token for a service identity; its secret is shown once.
    ServiceAccountTokenCreate: POST "/service-accounts/{id}/tokens", Rule("members.manage on the org that keeps the service identity"), none, json ApiTokenCreateBody, json TokenCreated, 200;
    /// Revokes a token of a service identity.
    ServiceAccountTokenDelete: DELETE "/service-accounts/{id}/tokens/{token_id}", Rule("members.manage on the org that keeps the service identity"), none, none, none, 204;
    /// Lists the tags of an org or a project and of the orgs above it.
    NodeTagList: GET "/nodes/{id}/tags", Permission("node.view"), none, none, json Vec<Tag>, 200;
    /// Defines a tag on an org or a project.
    NodeTagCreate: POST "/nodes/{id}/tags", Permission("node.manage"), none, json NodeTagCreateBody, json Tag, 200;
    /// Renames a tag or changes its colour.
    TagUpdate: PATCH "/tags/{id}", Rule("org.manage or project.manage on the tag's node"), none, json TagUpdateBody, json Tag, 200;
    /// Deletes a tag, and takes it off every topic.
    TagDelete: DELETE "/tags/{id}", Rule("org.manage or project.manage on the tag's node"), none, none, none, 204;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_member_is_read_with_the_org_that_is_their_domain() {
        let written = json!({
            "user": {
                "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b", "kind": "human", "username": "bo",
                "display_name": "Bo", "avatar": { "text": "B", "color": 2 }
            },
            "domain": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6c", "kind": "org", "name": "Research", "redacted": false },
            "address": "acme.research.bo",
            "role": "developer",
            "is_owner": false,
            "since": null,
            "also": 1,
            "suspended": false,
            "second_step": "sso",
            "second_step_required": true,
            "last_seen_at": "2026-10-14T09:30:00.000Z"
        });
        let member: Member = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(member.role, Role::Developer);
        assert_eq!(member.second_step, Some(MemberSecondStep::Sso));
        assert_eq!(serde_json::to_value(&member).unwrap(), written);
        let unmanaged =
            json!({ "second_step": null, "second_step_required": null, "last_seen_at": null });
        let mut plain = written;
        for (key, value) in unmanaged.as_object().unwrap() {
            plain[key] = value.clone();
        }
        let member: Member = serde_json::from_value(plain.clone()).unwrap();
        assert_eq!(
            serde_json::to_value(&member).unwrap(),
            plain,
            "a reader who does not manage members reads nulls"
        );
    }

    #[test]
    fn a_group_update_names_only_what_it_changes() {
        assert!(
            serde_json::from_value::<GroupUpdateBody>(
                json!({ "name": "Reviewers", "members": [] })
            )
            .is_err()
        );
        let rename: GroupUpdateBody =
            serde_json::from_value(json!({ "name": "Reviewers" })).unwrap();
        assert_eq!(rename.description, None);
    }
}
