//! Owners as a row: who may make each kind of resource and who then owns it, the first owner, naming another and leaving, and the last who cannot; what becomes of a person's things when a membership ends; a member's move to another domain, with what they lose, gain and keep; restore; who may move work, which changes neither owners nor page; a fork as its maker's, with the page of its source.
//!
//! Everything is somebody's. A member makes the five kinds of resource the policy lists in their own
//! domain or an org inside it, and whoever makes the rules of an org makes any kind there; the
//! maker is the first owner, alone in the Owners row until somebody is named. Ownership counts
//! only while the owner is an active member of the thing's root org, so when a membership ends each
//! thing of the person goes one of a few ways, which [`plan_leaving`] lists before anything
//! happens and [`end_membership`] carries out in the caller's transaction: what they own with
//! others is the others', what they own alone passes to the first owner of the org it lies in, what
//! was registered from their machines is unregistered, and the entries that name them are taken
//! away. Where a rule needs a part of the system that does not exist yet (stopping a runtime,
//! unregistering a machine), what has to be done comes back as data, a [`FollowUp`], for the
//! caller to carry out.
//!
//! Passing a thing on is the ordinary change of its Owners row ([`transfer`]); a switch and a move
//! of work change neither owners nor page, and a fork is its maker's, starting with the page of its
//! source or with the defaults of where it is made ([`start_fork`]).

use std::collections::{HashMap, HashSet};

use orch_access_contract::audit::AuditAction;
use orch_access_contract::decisions::{Decision, ObjectRef, Refusal, Through};
use orch_access_contract::pages::{EntrySource, Holder, OwnersChange, PageChange, StoredKind};
use orch_access_contract::permissions::{
    NodePermission, ObjectKind, Permission, Profile, ResourceKind,
};
use orch_access_contract::roles::{NodeKind, Role};
use orch_db::Conn;
use orch_db::audit::{self, Actor};
use orch_db::queries::access::{self, NewEntry, PageKey, RowWrite};
use orch_db::queries::hierarchy;
use orch_db::queries::resources::{self as resource_rows, OwnedRow};
use orch_types::{Id, Json, Time};

use crate::engine::{Frame, Graph};
use crate::nodes;
use crate::pages::{self, ChangeError, Outcome, Target};
use crate::resources::{self, use_permission};
use crate::usable::{self, Policies, ResourcePolicy};
use crate::{Act, Caller, Error, Policy, Principal};

// ---- Who may make one ---------------------------------------------------------------------------

/// Why a person may make a resource where they ask to.
#[derive(Debug, Clone, PartialEq)]
pub enum Basis {
    /// They are a member and the kind is one the policy `resources.created_by_members` lists at
    /// that org, and it lies in their domain or inside it.
    Member,
    /// They hold `settings.manage` on the org: they make its rules, and any kind there.
    Rules(Vec<Through>),
    /// They are a system admin, and it is a resource of the system.
    SystemAdmin,
}

/// Why a person may not make a resource where they ask to.
#[derive(Debug, Clone, PartialEq)]
pub enum Why {
    /// A provider always has a home, and an environment is made for a node: the first is made in an
    /// org, the second as [`crate::environments::check_creation`] says.
    NotMadeThisWay,
    /// The home is no org.
    HomeIsNoOrg,
    /// Only a system admin makes a resource of the system.
    SystemAdminsOnly,
    /// They are no member of the root org of the home, so what they made would not be theirs.
    NotAMember,
    /// The kind is listed for members, and the home lies outside their domain: a member makes a
    /// resource in their domain or in an org inside it.
    OutsideYourDomain {
        /// Their domain in that tree.
        domain: Id,
    },
    /// The kind is left out for members at that org, or is an agent profile, which only whoever
    /// makes the org's rules makes: `settings.manage` is what is lacking, with who can give it.
    KeptByAdmins(Box<Refusal>),
}

/// Whether a person may make a resource of a kind at a home.
#[derive(Debug, Clone, PartialEq)]
pub enum Making {
    /// They may.
    Allowed(Basis),
    /// They may not.
    Refused(Why),
}

/// Whether the caller may make a resource of the kind `kind` whose home is the org `home` (none: a
/// resource of the system). `members_make` is the setting `resources.created_by_members` resolved
/// at that org: the kinds any member may make there as their own; as shipped all five.
///
/// A member makes a resource in their domain, or in an org inside it, and that org is its home.
/// Where the kind is left out at that org, or for an agent profile, the caller needs
/// `settings.manage` on the org. For the system, a system admin; a provider has no such thing.
pub async fn may_make(
    conn: &mut Conn,
    principal: &Principal,
    kind: ResourceKind,
    home: Option<Id>,
    members_make: &[ResourceKind],
    policy: Policy,
) -> Result<Making, Error> {
    if kind == ResourceKind::Environment {
        return Ok(Making::Refused(Why::NotMadeThisWay));
    }
    let caller = principal.caller();
    let manage = match kind {
        ResourceKind::Provider => Permission::ProviderManage,
        ResourceKind::Agent => Permission::AgentManage,
        ResourceKind::Host => Permission::HostManage,
        ResourceKind::Machine => Permission::MachineManage,
        ResourceKind::Template => Permission::TemplateManage,
        ResourceKind::McpServer => Permission::McpManage,
        ResourceKind::Mount => Permission::MountManage,
        ResourceKind::Skill => Permission::SkillManage,
        ResourceKind::Environment => return Ok(Making::Refused(Why::NotMadeThisWay)),
    };
    let Some(home) = home else {
        if !kind.can_be_the_systems() {
            return Ok(Making::Refused(Why::NotMadeThisWay));
        }
        return Ok(if principal.is_system_admin() && caller.permits(manage) {
            Making::Allowed(Basis::SystemAdmin)
        } else {
            Making::Refused(Why::SystemAdminsOnly)
        });
    };
    let graph = Graph::load(conn, &[home]).await?;
    let node = graph.get(home).ok_or(Error::NotFound(home))?.clone();
    let site = graph.site_of(home).ok_or(Error::NotFound(home))?;
    // A skill may also belong to a project, whose work it is made for.
    let project_home = kind == ResourceKind::Skill && node.kind == NodeKind::Project;
    if node.kind != NodeKind::Org && !project_home {
        return Ok(Making::Refused(Why::HomeIsNoOrg));
    }
    if let Some(closing) = principal.suspension().closing(&site.orgs) {
        return Err(Error::Suspended { org: closing.org() });
    }
    let domain = principal.memberships().domain_in(node.root);
    let listed = kind != ResourceKind::Agent && members_make.contains(&kind);
    let inside = domain.is_some_and(|domain| node.path.contains(&domain));
    // A member makes one in a project only where they may work in it, as they would make a topic.
    let works_there = !project_home
        || matches!(
            nodes::check(
                conn,
                principal,
                home,
                Permission::ProjectInteract,
                Act::Use,
                policy,
            )
            .await?,
            Decision::Allowed { .. }
        );
    if listed && inside && works_there && caller.permits(manage) && caller.reaches(Some(&node.path))
    {
        return Ok(Making::Allowed(Basis::Member));
    }
    match nodes::check(
        conn,
        principal,
        home,
        Permission::SettingsManage,
        Act::Use,
        policy,
    )
    .await?
    {
        Decision::Allowed { through } => {
            owner_of_new(conn, principal.user(), Some(home)).await?;
            Ok(Making::Allowed(Basis::Rules(through)))
        }
        Decision::Refused(refusal) => Ok(Making::Refused(match domain {
            None => Why::NotAMember,
            Some(domain) if listed && !inside => Why::OutsideYourDomain { domain },
            Some(_) => Why::KeptByAdmins(Box::new(refusal)),
        })),
    }
}

// ---- Whose it is --------------------------------------------------------------------------------

/// Selects a new thing's first owner without making its creator a member: an existing member owns
/// what they make, and an outsider's thing belongs to the nearest org's first owner who is a member.
/// No home denotes a system resource or a new root org, whose creator is its first owner.
pub async fn owner_of_new(conn: &mut Conn, maker: Id, place: Option<Id>) -> Result<Id, Error> {
    let Some(place) = place else {
        return Ok(maker);
    };
    let graph = Graph::load(conn, &[place]).await?;
    let node = graph.get(place).ok_or(Error::NotFound(place))?;
    let orgs: Vec<_> = node
        .path
        .iter()
        .rev()
        .filter_map(|id| graph.get(*id))
        .filter(|node| node.kind == NodeKind::Org)
        .collect();
    let candidates: Vec<_> = std::iter::once(maker)
        .chain(orgs.iter().map(|org| org.first_owner))
        .collect();
    let members: HashSet<_> = access::members_among(conn, node.root, &candidates)
        .await?
        .into_iter()
        .collect();
    if members.contains(&maker) {
        return Ok(maker);
    }
    orgs.iter()
        .find_map(|org| {
            members
                .contains(&org.first_owner)
                .then_some(org.first_owner)
        })
        .ok_or(Error::NoActiveOwner { place })
}

/// Keeps every creation path's stored first owner and Owners row consistent, including documents
/// and threads made by an outside holder. Returns the creator's Manage permission when they differ.
pub(crate) async fn first_for_new_page(
    conn: &mut Conn,
    kind: StoredKind,
    thing: Id,
    maker: Id,
    at: Time,
) -> Result<(Id, Option<Permission>), Error> {
    let (place, manage) = match kind {
        StoredKind::Node => {
            let graph = Graph::load(conn, &[thing]).await?;
            let node = graph.get(thing).ok_or(Error::NotFound(thing))?;
            (
                node.parent,
                NodePermission::Manage
                    .on(node.kind)
                    .ok_or(Error::NotFound(thing))?,
            )
        }
        StoredKind::Document | StoredKind::DocumentContainer => {
            let (_, document, _) = crate::documents::site(conn, thing).await?;
            (Some(document.node), Permission::DocManage)
        }
        StoredKind::System | StoredKind::Chat => return Ok((maker, None)),
        _ => {
            let row = resource_rows::resource_facts(conn, &[thing])
                .await?
                .into_iter()
                .find(|row| row.kind == kind.as_str())
                .ok_or(Error::NotFound(thing))?;
            let resource: ResourceKind = kind.as_str().parse()?;
            let manage = match resource {
                ResourceKind::Provider => Permission::ProviderManage,
                ResourceKind::Agent => Permission::AgentManage,
                ResourceKind::Environment => Permission::EnvironmentManage,
                ResourceKind::Host => Permission::HostManage,
                ResourceKind::Machine => Permission::MachineManage,
                ResourceKind::Template => Permission::TemplateManage,
                ResourceKind::McpServer => Permission::McpManage,
                ResourceKind::Mount => Permission::MountManage,
                ResourceKind::Skill => Permission::SkillManage,
            };
            (row.home_id, manage)
        }
    };
    let first = owner_of_new(conn, maker, place).await?;
    if first == maker {
        return Ok((first, None));
    }
    match kind {
        StoredKind::Node => hierarchy::set_first_owner(conn, thing, first, at.get()).await?,
        StoredKind::Document => {
            resource_rows::set_document_owner(conn, thing, first, at.get()).await?
        }
        StoredKind::DocumentContainer => {
            orch_db::queries::documents::containers::set_owner(conn, thing, first, at.get())
                .await?;
        }
        _ => resource_rows::set_resource_owner(conn, kind.as_str(), thing, first, at.get()).await?,
    }
    Ok((first, Some(manage)))
}

/// Who a new resource's page names, as its maker chose in the dialog that made it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NewAccess {
    /// As its home decides: every row follows, with what that gives beside it.
    #[default]
    Defaults,
    /// Its maker alone: the Use row (for an environment the Manage row) is cut loose, so that a
    /// later change of its home's defaults does not open it.
    OnlyMe,
    /// Everyone in its home: the Viewers of its home are named in its Use row.
    Home,
    /// Every user of the system: its Use row names everyone. Only for a resource of the system, which has
    /// no home whose people could be named instead, and whose pages are the only ones that may hold the
    /// entry.
    Everyone,
}

/// Makes `maker` the first owner of the new resource `object`, alone in its Owners row, and applies
/// the choice they made of who else uses it. Moves the authorization epoch. The row of the
/// resource itself is the caller's.
pub async fn start_resource(
    conn: &mut Conn,
    object: ObjectRef,
    maker: Id,
    access_choice: NewAccess,
    at: Time,
) -> Result<(), Error> {
    pages::start(conn, StoredKind::from(object.kind), object.id, maker, at).await?;
    let page = PageKey::new(StoredKind::from(object.kind).as_str(), object.id, "live");
    let kind = object.kind.resource();
    match access_choice {
        NewAccess::Defaults => {}
        NewAccess::OnlyMe => {
            let row = match kind {
                Some(ResourceKind::Environment) => Some(Permission::EnvironmentManage),
                Some(kind) => use_permission(kind),
                None => None,
            };
            if let Some(permission) = row {
                access::write_row(
                    conn,
                    &RowWrite {
                        page: &page,
                        profile: Profile::Any.as_str(),
                        permission: permission.as_str(),
                        inherit: false,
                        locked: false,
                    },
                    Some(maker),
                    at.get(),
                )
                .await?;
            }
        }
        NewAccess::Home | NewAccess::Everyone => {
            if let Some(permission) = kind.and_then(use_permission) {
                let holder = if access_choice == NewAccess::Everyone {
                    Holder::Everyone
                } else {
                    Holder::Role {
                        role: Role::Viewer,
                        at: None,
                    }
                };
                let (holder_kind, holder_id, holder_role) = holder.stored();
                access::insert_entry(
                    conn,
                    &NewEntry {
                        id: orch_types::new_id(),
                        page: &page,
                        profile: Profile::Any.as_str(),
                        permission: permission.as_str(),
                        holder_kind: holder_kind.as_str(),
                        holder_id,
                        holder_role: holder_role.map(Role::as_str),
                        options: &Json::Object(serde_json::Map::new()),
                        expires_at: None,
                        note: "",
                        source: EntrySource::Creation.as_str(),
                        created_by: Some(maker),
                        at: at.get(),
                    },
                )
                .await?;
            }
        }
    }
    Ok(())
}

/// Passes a thing on: `to` is named in its Owners row and is first in it from now on; the person
/// who was first stays an owner unless `keep_previous` is false. It is the ordinary change of the
/// Owners row, by one of its owners or an admin of the place it is in, audited as any, and `to` is
/// an active member of its root org, or the change is refused as naming an outsider.
/// Other source-bound registrations keep their machine's owner first; an environment's
/// Owners row is independent of physical machine management.
pub async fn transfer(
    conn: &mut Conn,
    principal: &Principal,
    object: ObjectRef,
    to: Id,
    keep_previous: bool,
    policy: &ResourcePolicy,
    origin: &audit::Origin,
) -> Result<Outcome, ChangeError> {
    let key = PageKey::new(StoredKind::from(object.kind).as_str(), object.id, "live");
    let previous = if object.kind.node().is_some() {
        let graph = Graph::load(conn, &[object.id]).await?;
        graph.get(object.id).map(|node| node.first_owner)
    } else {
        let (_, resource, _) = resources::site(conn, object).await?;
        resource.first_owner
    };
    let mut remove = Vec::new();
    if !keep_previous && previous != Some(to) {
        for entry in access::owner_entries(conn, std::slice::from_ref(&key)).await? {
            if entry.holder_kind == "user" && entry.holder_id == previous {
                remove.push(entry.id);
            }
        }
    }
    let change = PageChange {
        owners: Some(OwnersChange {
            add: Some(vec![Holder::User { id: to }]),
            remove: (!remove.is_empty()).then_some(remove),
            first: Some(to),
        }),
        ..PageChange::default()
    };
    if object.kind.node().is_some() {
        pages::change(
            conn,
            principal,
            Target::Live(object.id),
            &change,
            policy.node_policy(),
            origin,
        )
        .await
    } else {
        pages::change_resource(conn, principal, object, &change, policy, origin).await
    }
}

// ---- A membership ends --------------------------------------------------------------------------

/// What a person who ends their membership may choose for a managed resource of theirs, instead of
/// leaving it with the first owner of its home.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Disposal {
    /// It goes on working for everyone its page names, with a new first owner.
    #[default]
    Keep,
    /// It is switched off.
    Disable,
    /// It is removed.
    Remove,
}

/// What becomes of one thing of a person whose membership ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fate {
    /// It is owned with others: the person is taken out of its Owners row, the one who has been in
    /// it longest is first if they were, and nothing else changes.
    LeftToOthers {
        /// How many others own it.
        others: i64,
    },
    /// It is owned alone: it goes to the first owner of the org it lies in, nothing else about it
    /// changing.
    PassedOn {
        /// The org it lies in, or the nearest org above that has an owner who stays.
        org: Id,
        /// The person it goes to.
        owner: Id,
    },
    /// It was registered from one of their machines: it is unregistered from this tree. The machine
    /// is theirs.
    Unregistered,
    /// Nobody can have it: the last owner of a root org names another before leaving it.
    Stuck,
}

/// One thing of a person, and what becomes of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Passing {
    /// The thing.
    pub object: ObjectRef,
    /// Its name.
    pub name: String,
    /// What becomes of it.
    pub fate: Fate,
    /// For a node that is passed on: its page was restricted, so that it is now reachable by the
    /// person it goes to, which the dialog counts apart.
    pub restricted: bool,
    /// For a provider: it is a subscription this person signed in, which other people will go on
    /// working on, as the dialog says.
    pub their_sign_in: bool,
    /// Whether it is a managed resource that may be disabled or removed instead of kept.
    pub may_be_disposed_of: bool,
}

/// What a person's membership of a tree costs them, thing by thing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// Everything they own in the tree, in the order of its identifiers.
    pub things: Vec<Passing>,
}

impl Plan {
    /// Whether anything is stuck: the membership cannot end until it is resolved.
    #[must_use]
    pub fn stuck(&self) -> Vec<&Passing> {
        self.things
            .iter()
            .filter(|passing| passing.fate == Fate::Stuck)
            .collect()
    }
}

fn object_of(row: &OwnedRow) -> Result<ObjectRef, Error> {
    let kind: ObjectKind = if row.object_kind == "node" {
        row.thing_kind.parse()?
    } else {
        row.object_kind.parse()?
    };
    Ok(ObjectRef {
        kind,
        id: row.object_id,
    })
}

/// What becomes of everything that `user` owns in the tree of the root org `root` when their
/// membership ends, for the dialog that ends it to list before anything happens. `actor` is who
/// ends it: the member, or an admin.
pub async fn plan_leaving(
    conn: &mut Conn,
    actor: &Principal,
    root: Id,
    user: Id,
    policy: Policy,
) -> Result<Plan, Error> {
    let owned = resource_rows::owned_by(conn, root, user, actor.now().get()).await?;
    if owned.is_empty() {
        return Ok(Plan::default());
    }
    let mut ids: Vec<Id> = owned.iter().filter_map(|row| row.anchor).collect();
    ids.extend(
        owned
            .iter()
            .filter(|row| row.object_kind == "node")
            .map(|row| row.object_id),
    );
    let graph = Graph::load(conn, &ids).await?;
    // The org a thing lies in: the nearest org at or above where it lies. That is the org above a
    // node, the org of the node a document is written beside, a resource's home, and for an
    // environment the nearest org at or above its home node.
    let org_above = |row: &OwnedRow| -> Option<Id> {
        let facts = graph.get(row.anchor?)?;
        facts
            .path
            .iter()
            .rev()
            .filter_map(|id| graph.get(*id))
            .find(|node| node.kind == NodeKind::Org)
            .map(|node| node.id)
    };
    // The first owners of the orgs a thing may pass to, in the order they are asked.
    let mut candidates: Vec<Id> = Vec::new();
    for row in &owned {
        if let Some(org) = org_above(row)
            && let Some(facts) = graph.get(org)
        {
            for id in facts.path.iter().rev() {
                if let Some(node) = graph.get(*id)
                    && node.kind == NodeKind::Org
                {
                    candidates.push(node.first_owner);
                }
            }
        }
    }
    candidates.sort_unstable();
    candidates.dedup();
    let members: HashSet<Id> = access::members_among(conn, root, &candidates)
        .await?
        .into_iter()
        .collect();
    let target = |row: &OwnedRow| -> Option<(Id, Id)> {
        let org = org_above(row)?;
        let facts = graph.get(org)?;
        facts.path.iter().rev().find_map(|id| {
            let node = graph.get(*id)?;
            (node.kind == NodeKind::Org
                && node.first_owner != user
                && members.contains(&node.first_owner))
            .then_some((node.id, node.first_owner))
        })
    };
    let mut things: Vec<Passing> = Vec::new();
    for row in &owned {
        let object = object_of(row)?;
        let managed_resource = matches!(
            row.object_kind.as_str(),
            "provider" | "environment" | "host" | "template" | "mcp_server" | "mount" | "skill"
        );
        let fate = if row.registered {
            Fate::Unregistered
        } else if row.others > 0 {
            Fate::LeftToOthers { others: row.others }
        } else {
            match target(row) {
                Some((org, owner)) => Fate::PassedOn { org, owner },
                None => Fate::Stuck,
            }
        };
        things.push(Passing {
            object,
            name: row.name.clone(),
            fate,
            restricted: false,
            their_sign_in: row.object_kind == "provider" && row.first_owner == Some(user),
            may_be_disposed_of: managed_resource && !row.registered,
        });
    }
    // Which of the nodes that are passed on were restricted.
    let passed_nodes: Vec<Id> = things
        .iter()
        .filter(|passing| {
            passing.object.kind.node().is_some() && matches!(passing.fate, Fate::PassedOn { .. })
        })
        .map(|passing| passing.object.id)
        .collect();
    if !passed_nodes.is_empty() {
        let graph = Graph::load(conn, &passed_nodes).await?;
        let sites: Vec<_> = passed_nodes
            .iter()
            .filter_map(|id| graph.site_of(*id))
            .collect();
        let views: Vec<Permission> = sites
            .iter()
            .filter_map(|site| site.kind.node())
            .filter_map(|kind| NodePermission::View.on(kind))
            .collect();
        let frame = Frame::load(conn, actor, policy, graph, &sites, &views).await?;
        let restricted: HashSet<Id> = sites
            .iter()
            .filter(|site| frame.restricted(site))
            .map(|site| site.id)
            .collect();
        for passing in &mut things {
            passing.restricted = restricted.contains(&passing.object.id);
        }
    }
    Ok(Plan { things })
}

/// What has to be done by a part of the system that is not here.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FollowUp {
    /// Switch the resource off: its leases close, its turns are interrupted, its threads are
    /// blocked, and a credential it had handed over is returned.
    Disable(ObjectRef),
    /// Remove the resource, as its owner removing it would.
    Remove(ObjectRef),
    /// Forget the registration of a thing from a person's machine in this tree; nothing is deleted
    /// on the machine.
    Unregister(ObjectRef),
}

/// What carrying out a plan did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ended {
    /// What became of each thing, as planned.
    pub passed: Vec<Passing>,
    /// What the caller still has to do.
    pub follow_ups: Vec<FollowUp>,
    /// How many entries that named the person were taken away.
    pub entries_taken: usize,
}

/// Why a membership's things cannot be settled.
#[derive(Debug, thiserror::Error)]
pub enum EndError {
    /// Something is stuck: the last owner of a root org names another before leaving it.
    #[error("the person is the last owner of something that nobody else can have")]
    Stuck(Vec<ObjectRef>),
    /// The access engine failed.
    #[error(transparent)]
    Engine(#[from] Error),
}

impl From<orch_db::DbError> for EndError {
    fn from(error: orch_db::DbError) -> Self {
        Self::Engine(error.into())
    }
}

struct Tools<'a> {
    actor: Id,
    root: Id,
    at: Time,
    origin: &'a audit::Origin,
}

impl Tools<'_> {
    async fn line(
        &self,
        conn: &mut Conn,
        action: AuditAction,
        thing: ObjectRef,
        summary: String,
        details: Json,
    ) -> Result<(), Error> {
        let kind = if thing.kind == ObjectKind::DocumentContainer {
            thing.kind.as_str()
        } else {
            action.object_kind().unwrap_or(thing.kind.as_str())
        };
        let mut entry = audit::Entry::new(
            Actor::User(self.actor),
            action.as_str(),
            kind,
            Some(thing.id),
            &summary,
        );
        entry.history = action.is_history_line();
        entry.org_id = Some(self.root);
        entry.node_id = thing.kind.node().map(|_| thing.id);
        entry.environment_id = (thing.kind == ObjectKind::Environment).then_some(thing.id);
        let mut details = details;
        if let Json::Object(members) = &mut details {
            members.insert(
                "object".to_owned(),
                serde_json::json!({ "kind": thing.kind.as_str(), "id": thing.id }),
            );
        }
        entry.details = details;
        audit::append(conn, &entry, self.origin, self.at.get()).await?;
        Ok(())
    }
}

async fn set_first(
    conn: &mut Conn,
    row: &OwnedRow,
    owner: Id,
    at: Time,
) -> Result<AuditAction, Error> {
    match row.object_kind.as_str() {
        "node" => {
            hierarchy::set_first_owner(conn, row.object_id, owner, at.get()).await?;
            Ok(AuditAction::NodeTransferred)
        }
        "document" => {
            resource_rows::set_document_owner(conn, row.object_id, owner, at.get()).await?;
            Ok(AuditAction::DocumentTransferred)
        }
        "document_container" => {
            orch_db::queries::documents::containers::lock(conn, row.object_id)
                .await?
                .ok_or(Error::NotFound(row.object_id))?;
            orch_db::queries::documents::containers::set_owner(
                conn,
                row.object_id,
                owner,
                at.get(),
            )
            .await?;
            Ok(AuditAction::DocumentTransferred)
        }
        kind => {
            resource_rows::set_resource_owner(conn, kind, row.object_id, owner, at.get()).await?;
            Ok(AuditAction::ResourceTransferred)
        }
    }
}

/// Carries out what [`plan_leaving`] lists, in the caller's transaction, for the membership of
/// `user` in the tree of the root org `root` that `actor` is ending (the member themself, or an
/// admin): the Owners rows are changed with their lines of history, the entries that name the
/// person in the pages of the tree are taken away, and the authorization epoch moves once. The
/// person's domain, roles and groups, and their pending requests, are the membership's own and
/// are not touched here. `choices` says, for a managed resource owned alone, whether to disable or
/// remove it instead of keeping it.
pub async fn end_membership(
    conn: &mut Conn,
    actor: &Principal,
    root: Id,
    user: Id,
    choices: &HashMap<Id, Disposal>,
    policy: Policy,
    origin: &audit::Origin,
) -> Result<Ended, EndError> {
    let plan = plan_leaving(conn, actor, root, user, policy).await?;
    let stuck: Vec<ObjectRef> = plan.stuck().iter().map(|passing| passing.object).collect();
    if !stuck.is_empty() {
        return Err(EndError::Stuck(stuck));
    }
    let at = actor.now();
    let owned = resource_rows::owned_by(conn, root, user, at.get()).await?;
    let tools = Tools {
        actor: actor.user(),
        root,
        at,
        origin,
    };
    let mut follow_ups: Vec<FollowUp> = Vec::new();
    for passing in &plan.things {
        let Some(row) = owned.iter().find(|row| row.object_id == passing.object.id) else {
            continue;
        };
        let key = PageKey::new(
            StoredKind::from(passing.object.kind).as_str(),
            row.object_id,
            "live",
        );
        match &passing.fate {
            Fate::Unregistered => {
                follow_ups.push(FollowUp::Unregister(passing.object));
            }
            Fate::Stuck => {}
            Fate::LeftToOthers { .. } | Fate::PassedOn { .. } => {
                let who = Holder::User { id: user };
                if let Fate::PassedOn { owner, .. } = &passing.fate {
                    let (kind, holder_id, role) = Holder::User { id: *owner }.stored();
                    if access::find_entry(
                        conn,
                        &key,
                        "any",
                        "access.own",
                        kind.as_str(),
                        holder_id,
                        role.map(Role::as_str),
                    )
                    .await?
                    .is_none()
                    {
                        let id = orch_types::new_id();
                        access::insert_entry(
                            conn,
                            &NewEntry {
                                id,
                                page: &key,
                                profile: "any",
                                permission: "access.own",
                                holder_kind: kind.as_str(),
                                holder_id,
                                holder_role: None,
                                options: &Json::Object(serde_json::Map::new()),
                                expires_at: None,
                                note: "",
                                source: EntrySource::Page.as_str(),
                                created_by: Some(actor.user()),
                                at: at.get(),
                            },
                        )
                        .await?;
                        tools
                            .line(
                                conn,
                                AuditAction::PermissionAdded,
                                passing.object,
                                format!("{} became an owner of {} as {} left", owner, passing.name, user),
                                serde_json::json!({ "permission": "access.own", "page": "page", "holder": Holder::User { id: *owner } }),
                            )
                            .await?;
                    }
                }
                access::remove_entry(conn, row.entry_id, actor.user(), at.get()).await?;
                tools
                    .line(
                        conn,
                        AuditAction::PermissionRemoved,
                        passing.object,
                        format!("{user} stopped owning {} as their membership ended", passing.name),
                        serde_json::json!({ "permission": "access.own", "page": "page", "holder": who }),
                    )
                    .await?;
                if row.first_owner == Some(user) {
                    let next = match &passing.fate {
                        Fate::PassedOn { owner, .. } => Some(*owner),
                        _ => access::owner_entries(conn, std::slice::from_ref(&key))
                            .await?
                            .into_iter()
                            .find(|entry| {
                                entry.holder_kind == "user" && entry.holder_id != Some(user)
                            })
                            .and_then(|entry| entry.holder_id),
                    };
                    if let Some(next) = next {
                        let action = set_first(conn, row, next, at).await?;
                        tools
                            .line(
                                conn,
                                action,
                                passing.object,
                                format!("{next} became the first owner of {}", passing.name),
                                serde_json::json!({ "from": user, "to": next }),
                            )
                            .await?;
                    }
                }
                if matches!(passing.fate, Fate::PassedOn { .. }) && passing.may_be_disposed_of {
                    match choices.get(&passing.object.id).copied().unwrap_or_default() {
                        Disposal::Keep => {}
                        Disposal::Disable => follow_ups.push(FollowUp::Disable(passing.object)),
                        Disposal::Remove => follow_ups.push(FollowUp::Remove(passing.object)),
                    }
                }
            }
        }
    }
    let taken =
        resource_rows::take_entries_naming(conn, root, user, actor.user(), at.get()).await?;
    access::bump_epoch(conn).await?;
    Ok(Ended {
        passed: plan.things,
        follow_ups,
        entries_taken: taken.len(),
    })
}

/// Whether the caller may end the membership of `user` in the tree of `root`: whoever holds
/// `members.manage` on the org that is the person's domain, or on one above it, and the member
/// themself. Never the last owner of the root org, which is the plan's to say.
pub async fn may_end_membership(
    conn: &mut Conn,
    principal: &Principal,
    root: Id,
    user: Id,
    policy: Policy,
) -> Result<Decision, Error> {
    if principal.user() == user && principal.is_in_tree(root) {
        return Ok(Decision::Allowed {
            through: Vec::new(),
        });
    }
    let memberships = crate::orgs::Memberships::load(conn, user).await?;
    let domain = memberships.domain_in(root).ok_or(Error::NotFound(root))?;
    nodes::check(
        conn,
        principal,
        domain,
        Permission::MembersManage,
        Act::Use,
        policy,
    )
    .await
}

// ---- A member moves -----------------------------------------------------------------------------

/// A member's move to another org of the tree they belong to, as the request puts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemberMove {
    /// The member.
    pub user: Id,
    /// The org that becomes their domain: an org of a tree they are a member of.
    pub to: Id,
    /// The role they are to hold there, where the request names one; otherwise the one they hold by
    /// name at their domain now.
    pub role: Option<Role>,
}

/// Why a member cannot be moved, whoever asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unmovable {
    /// The org they are to go to is not there, is deleted, or is no org.
    NoSuchOrg,
    /// The person is no member of the tree that org lies in.
    NotAMember,
    /// It is their domain already.
    AlreadyThere,
}

/// What stops the caller from moving a member: each end that lacks `members.manage` with what is
/// missing and who holds it there, so that somebody who holds it at one end only is told who holds
/// the other.
#[derive(Debug, Clone, PartialEq)]
pub struct MoveRefused {
    /// `members.manage` is lacking at the org the person leaves.
    pub leaving: Option<Box<Refusal>>,
    /// `members.manage` is lacking at the org they join.
    pub joining: Option<Box<Refusal>>,
    /// Admin would be given at the org they join, and the caller is neither an owner nor an admin
    /// there: the people who are.
    pub admins_there: Option<Vec<Id>>,
}

/// Whether the caller may move a member.
#[derive(Debug, Clone, PartialEq)]
pub enum MoveCheck {
    /// There is nothing to move.
    Impossible(Unmovable),
    /// They hold `members.manage` at both ends, and where the move gives Admin they are an owner
    /// or an admin there.
    Allowed,
    /// They do not.
    Refused(MoveRefused),
}

/// What is read of a member and the two orgs, before a move is checked, planned or made.
#[derive(Debug, Clone)]
struct Facts {
    root: Id,
    from: Id,
    /// Their role by name at the domain they leave: the one that moves.
    held: Option<access::RoleRow>,
    /// Their role by name at the org they join already.
    there: Option<access::RoleRow>,
    before: Option<Role>,
    on_arrival: Option<Role>,
    /// What they hold by name at the new domain: the role asked for, or the one they had, or the
    /// role they already hold there, whichever is highest.
    after: Option<Role>,
}

async fn facts_of(
    conn: &mut Conn,
    ask: &MemberMove,
    now: Time,
    lock: bool,
) -> Result<Result<Facts, Unmovable>, Error> {
    let graph = Graph::load(conn, &[ask.to]).await?;
    let Some(target) = graph
        .get(ask.to)
        .filter(|node| node.kind == NodeKind::Org && !node.deleted)
    else {
        return Ok(Err(Unmovable::NoSuchOrg));
    };
    let root = target.root;
    if lock {
        access::lock_membership(conn, root, ask.user).await?;
    }
    let Some(from) = crate::orgs::Memberships::load(conn, ask.user)
        .await?
        .domain_in(root)
    else {
        return Ok(Err(Unmovable::NotAMember));
    };
    if from == ask.to {
        return Ok(Err(Unmovable::AlreadyThere));
    }
    let rows = access::roles_in_places(conn, &[from, ask.to], now.get()).await?;
    let own = |place: Id| {
        rows.iter()
            .find(|row| {
                row.node_id == place && row.subject_kind == "user" && row.subject_id == ask.user
            })
            .cloned()
    };
    let (held, there) = (own(from), own(ask.to));
    let before = held
        .as_ref()
        .map(|row| row.role.parse::<Role>())
        .transpose()?;
    let on_arrival = there
        .as_ref()
        .map(|row| row.role.parse::<Role>())
        .transpose()?;
    Ok(Ok(Facts {
        root,
        from,
        held,
        there,
        before,
        on_arrival,
        after: ask.role.or(before).max(on_arrival),
    }))
}

async fn check_facts(
    conn: &mut Conn,
    principal: &Principal,
    ask: &MemberMove,
    facts: &Facts,
    policies: &Policies,
) -> Result<MoveCheck, Error> {
    let policy = policies.at(Some(ask.to)).node_policy();
    let mut refused = MoveRefused {
        leaving: None,
        joining: None,
        admins_there: None,
    };
    let manage = Permission::MembersManage;
    if let Decision::Refused(refusal) =
        nodes::check(conn, principal, facts.from, manage, Act::Use, policy).await?
    {
        refused.leaving = Some(Box::new(refusal));
    }
    if let Decision::Refused(refusal) =
        nodes::check(conn, principal, ask.to, manage, Act::Use, policy).await?
    {
        refused.joining = Some(Box::new(refusal));
    }
    // Admin is given by an owner or an admin of the org it is given on, whoever else may move people.
    let gives_admin = facts.after == Some(Role::Admin) && facts.on_arrival != Some(Role::Admin);
    if gives_admin && refused.joining.is_none() && !principal.is_system_admin() {
        let graph = Graph::load(conn, &[ask.to]).await?;
        let site = graph.site_of(ask.to).ok_or(Error::NotFound(ask.to))?;
        let frame = Frame::load(
            conn,
            principal,
            policy,
            graph,
            std::slice::from_ref(&site),
            &[manage],
        )
        .await?;
        if !crate::admins::is_admin_at(&frame, ask.to) {
            refused.admins_there = Some(
                crate::holders::admins_at(conn, &frame, &[ask.to])
                    .await?
                    .into_iter()
                    .map(|(_, user)| user)
                    .collect(),
            );
        }
    }
    // A move gives the resulting role at the destination, just as a direct role grant does.
    if let Some(role) = facts.after
        && facts.on_arrival.is_none_or(|held| role > held)
        && refused.joining.is_none()
        && refused.admins_there.is_none()
    {
        let sharing = policies.sharing_at(Some(ask.to));
        let standing = crate::giving::role_standing(conn, principal, ask.to, sharing).await?;
        if !standing.may_give(role) {
            refused.joining = Some(Box::new(
                crate::giving::role_refusal(conn, principal, ask.to, role, sharing).await?,
            ));
        }
    }
    Ok(
        if refused.leaving.is_none() && refused.joining.is_none() && refused.admins_there.is_none()
        {
            MoveCheck::Allowed
        } else {
            MoveCheck::Refused(refused)
        },
    )
}

/// Whether the caller may move the member: `members.manage` at both ends and authority to give
/// any role newly granted at the destination.
pub async fn may_move_member(
    conn: &mut Conn,
    principal: &Principal,
    ask: &MemberMove,
    policies: &Policies,
) -> Result<MoveCheck, Error> {
    match facts_of(conn, ask, principal.now(), false).await? {
        Ok(facts) => check_facts(conn, principal, ask, &facts, policies).await,
        Err(why) => Ok(MoveCheck::Impossible(why)),
    }
}

/// How many nodes of one kind the member stops reaching by the role they hold at their domain, and
/// how many they start to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reached {
    /// The kind.
    pub kind: NodeKind,
    /// How many lie in the org they leave and not in the one they join.
    pub lost: i64,
    /// How many lie in the org they join and not in the one they leave.
    pub gained: i64,
}

/// A thing of the member that stays where it is: theirs still, in the place they leave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Staying {
    /// The thing.
    pub object: ObjectRef,
    /// Its name.
    pub name: String,
}

/// A thread of the member that would wait: they are its first owner, and the provider it runs on
/// is one only the old place let them use, so that an input without an author waits for them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Waiting {
    /// The thread.
    pub thread: Id,
    /// Its name.
    pub name: String,
    /// The provider it runs on.
    pub provider: Id,
}

/// What a member's move comes to, in numbers, before anything happens. Everything that names them
/// in person stays: the roles they were given elsewhere, every entry, every Owners row they are in,
/// their groups and their membership of the root org.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveNumbers {
    /// The org they leave.
    pub from: Id,
    /// The org they join.
    pub to: Id,
    /// The role they hold by name at the org they leave.
    pub role_before: Option<Role>,
    /// The role they hold by name at the org they join: the one asked for or the one they had,
    /// or the one they already hold there if that is higher.
    pub role_after: Option<Role>,
    /// What lies where only one of the two orgs reaches, by kind.
    pub places: Vec<Reached>,
    /// How many roles given to the members of an org that stops to be one of theirs they stop to hold.
    pub member_roles_lost: i64,
    /// How many roles given to the members of an org they now belong to they start to hold.
    pub member_roles_gained: i64,
    /// What they own in the org they leave and not in the one they join.
    pub stays_behind: Vec<Staying>,
    /// Their threads that would wait.
    pub would_wait: Vec<Waiting>,
}

/// What asking for a member's move says before it is made.
#[derive(Debug, Clone, PartialEq)]
pub struct MovePlan {
    /// Whether the caller may make it.
    pub check: MoveCheck,
    /// What it comes to, for a caller who holds `members.manage` at one end at least; none for one
    /// who holds it at neither, and when there is nothing to move.
    pub numbers: Option<MoveNumbers>,
}

/// Plans a member's move: whether the caller may make it, and what it comes to in numbers: what the
/// person loses and gains, what of theirs stays behind in the place they leave, and which of their
/// threads would wait. Nothing is changed: the move is made inside a trial that is undone, so that
/// the same rules as after the real move say what they would hold.
pub async fn plan_move(
    conn: &mut Conn,
    principal: &Principal,
    ask: &MemberMove,
    policies: &Policies,
) -> Result<MovePlan, Error> {
    let now = principal.now();
    let facts = match facts_of(conn, ask, now, false).await? {
        Ok(facts) => facts,
        Err(why) => {
            return Ok(MovePlan {
                check: MoveCheck::Impossible(why),
                numbers: None,
            });
        }
    };
    let check = check_facts(conn, principal, ask, &facts, policies).await?;
    if let MoveCheck::Refused(refused) = &check
        && refused.leaving.is_some()
        && refused.joining.is_some()
    {
        return Ok(MovePlan {
            check,
            numbers: None,
        });
    }
    let graph = Graph::load(conn, &[facts.from, ask.to]).await?;
    let path_of = |org: Id| {
        graph
            .get(org)
            .map(|node| node.path.clone())
            .unwrap_or_default()
    };
    let (old_path, new_path) = (path_of(facts.from), path_of(ask.to));
    let only_old: Vec<Id> = old_path
        .iter()
        .filter(|org| !new_path.contains(org))
        .copied()
        .collect();
    let only_new: Vec<Id> = new_path
        .iter()
        .filter(|org| !old_path.contains(org))
        .copied()
        .collect();
    let mut places = Vec::new();
    for count in access::places_between(conn, facts.from, ask.to).await? {
        places.push(Reached {
            kind: count.kind.parse::<NodeKind>()?,
            lost: count.lost,
            gained: count.gained,
        });
    }
    let both: Vec<Id> = only_old.iter().chain(only_new.iter()).copied().collect();
    let given = access::roles_given_to_members(conn, &both, now.get()).await?;
    let total = |orgs: &[Id]| -> i64 {
        given
            .iter()
            .filter(|(org, _)| orgs.contains(org))
            .map(|(_, count)| *count)
            .sum()
    };
    let (member_roles_lost, member_roles_gained) = (total(&only_old), total(&only_new));
    let stays_behind = stays_behind(conn, ask, &facts, now).await?;
    let would_wait = would_wait(conn, principal, ask, &facts, policies).await?;
    Ok(MovePlan {
        check,
        numbers: Some(MoveNumbers {
            from: facts.from,
            to: ask.to,
            role_before: facts.before,
            role_after: facts.after,
            places,
            member_roles_lost,
            member_roles_gained,
            stays_behind,
            would_wait,
        }),
    })
}

/// What the member owns in the tree that lies in the org they leave and not in the one they join.
async fn stays_behind(
    conn: &mut Conn,
    ask: &MemberMove,
    facts: &Facts,
    now: Time,
) -> Result<Vec<Staying>, Error> {
    let owned = resource_rows::owned_by(conn, facts.root, ask.user, now.get()).await?;
    let place = |row: &OwnedRow| {
        if row.object_kind == "node" {
            Some(row.object_id)
        } else {
            row.anchor
        }
    };
    let places: Vec<Id> = owned.iter().filter_map(place).collect();
    let graph = Graph::load(conn, &places).await?;
    let mut staying = Vec::new();
    for row in &owned {
        let Some(path) = place(row)
            .and_then(|id| graph.get(id))
            .map(|node| &node.path)
        else {
            continue;
        };
        if path.contains(&facts.from) && !path.contains(&ask.to) {
            staying.push(Staying {
                object: object_of(row)?,
                name: row.name.clone(),
            });
        }
    }
    Ok(staying)
}

/// Their threads that run on a provider the person uses now and would not after the move.
async fn would_wait(
    conn: &mut Conn,
    principal: &Principal,
    ask: &MemberMove,
    facts: &Facts,
    policies: &Policies,
) -> Result<Vec<Waiting>, Error> {
    let bound = resource_rows::threads_first_owned_by(conn, facts.root, ask.user).await?;
    if bound.is_empty() {
        return Ok(Vec::new());
    }
    let now = principal.now();
    let pairs: Vec<(Id, Id)> = bound
        .iter()
        .map(|thread| (thread.thread, thread.provider))
        .collect();
    let person = Principal::load(conn, Caller::session(ask.user), now).await?;
    let before = usable::usable_on_threads(conn, &person, &pairs, policies).await?;
    if before.is_empty() {
        return Ok(Vec::new());
    }
    let mut trial = access::begin_trial(conn).await?;
    let after = async {
        carry_out(&mut trial, ask, facts, principal.user(), now).await?;
        let moved = Principal::load(&mut trial, Caller::session(ask.user), now).await?;
        usable::usable_on_threads(&mut trial, &moved, &before, policies).await
    }
    .await;
    access::end_trial(trial).await?;
    let after = after?;
    Ok(bound
        .into_iter()
        .filter(|thread| {
            let pair = (thread.thread, thread.provider);
            before.contains(&pair) && !after.contains(&pair)
        })
        .map(|thread| Waiting {
            thread: thread.thread,
            name: thread.name,
            provider: thread.provider,
        })
        .collect())
}

/// What a member's move did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Moved {
    /// The org they left.
    pub from: Id,
    /// The org they joined.
    pub to: Id,
    /// The role they held by name at the org they left.
    pub role_before: Option<Role>,
    /// The role they hold by name at the org they joined.
    pub role_after: Option<Role>,
}

/// Why a member could not be moved.
#[derive(Debug, thiserror::Error)]
pub enum MoveError {
    /// There is nothing to move.
    #[error("the member cannot be moved there")]
    Impossible(Unmovable),
    /// The caller may not move them.
    #[error("the caller may not move the member")]
    Refused(MoveRefused),
    /// The access engine failed.
    #[error(transparent)]
    Engine(#[from] Error),
}

impl From<orch_db::DbError> for MoveError {
    fn from(error: orch_db::DbError) -> Self {
        Self::Engine(error.into())
    }
}

/// The writes of a move, without a line of history and without the epoch: the role is taken from
/// the old org and given on the new one, the higher of two held by name there, and the domain is
/// changed.
async fn carry_out(
    conn: &mut Conn,
    ask: &MemberMove,
    facts: &Facts,
    by: Id,
    at: Time,
) -> Result<(), Error> {
    if let Some(held) = &facts.held {
        access::end_role(conn, held.id, by, at.get()).await?;
    }
    match (&facts.there, facts.after) {
        (Some(there), Some(after)) if Some(after) != facts.on_arrival => {
            access::set_role(conn, there.id, after.as_str()).await?;
        }
        (None, Some(after)) => {
            access::give_role_to_person(
                conn,
                &access::PersonRole {
                    place: ask.to,
                    user: ask.user,
                    role: after.as_str(),
                    expires_at: facts.held.as_ref().and_then(|held| held.expires_at),
                    note: facts.held.as_ref().map_or("", |held| held.note.as_str()),
                    by,
                    at: at.get(),
                },
            )
            .await?;
        }
        _ => {}
    }
    access::set_domain(conn, facts.root, ask.user, ask.to, by, at.get()).await?;
    Ok(())
}

/// Moves the member, in the caller's transaction: their domain is changed, and with it where their
/// member's role is held, which is taken from the old org and given on the new one (the higher of the
/// two where they held a role there already). Nothing that names them in person is touched. Checked
/// as [`may_move_member`] says, with a line of history and one move of the authorization epoch.
pub async fn move_member(
    conn: &mut Conn,
    actor: &Principal,
    ask: &MemberMove,
    policies: &Policies,
    origin: &audit::Origin,
) -> Result<Moved, MoveError> {
    let at = actor.now();
    let facts = match facts_of(conn, ask, at, true).await? {
        Ok(facts) => facts,
        Err(why) => return Err(MoveError::Impossible(why)),
    };
    match check_facts(conn, actor, ask, &facts, policies).await? {
        MoveCheck::Allowed => {}
        MoveCheck::Refused(refused) => return Err(MoveError::Refused(refused)),
        MoveCheck::Impossible(why) => return Err(MoveError::Impossible(why)),
    }
    carry_out(conn, ask, &facts, actor.user(), at).await?;
    let action = AuditAction::MemberMoved;
    let summary = format!("{} was moved from {} to {}", ask.user, facts.from, ask.to);
    let mut entry = audit::Entry::new(
        Actor::User(actor.user()),
        action.as_str(),
        action.object_kind().unwrap_or("member"),
        Some(ask.user),
        &summary,
    );
    entry.history = action.is_history_line();
    entry.org_id = Some(facts.root);
    entry.node_id = Some(ask.to);
    entry.details = serde_json::json!({
        "from": facts.from,
        "to": ask.to,
        "role_before": facts.before,
        "role_after": facts.after,
    });
    audit::append(conn, &entry, origin, at.get()).await?;
    access::bump_epoch(conn).await?;
    Ok(Moved {
        from: facts.from,
        to: ask.to,
        role_before: facts.before,
        role_after: facts.after,
    })
}

// ---- Restoring ----------------------------------------------------------------------------------

/// Whether the caller may restore a node that was deleted.
#[derive(Debug, Clone, PartialEq)]
pub enum Restoring {
    /// They may.
    Allowed,
    /// They lack `node.manage` on the parent of what was deleted.
    Refused(Box<Refusal>),
    /// A deleted root org is restored by one of its owners or by a system admin.
    OwnersAndSystemAdminsOnly,
    /// The node is not deleted.
    NotDeleted,
}

/// Whether the caller may restore the deleted node `node`: `node.manage` on its parent, and for a
/// root org one of its owners or a system admin. A node that was purged is not there to be restored.
pub async fn may_restore(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    policy: Policy,
) -> Result<Restoring, Error> {
    let rows = hierarchy::with_ancestors(conn, &[node]).await?;
    let row = rows
        .iter()
        .find(|row| row.id == node)
        .ok_or(Error::NotFound(node))?;
    if row.deleted_at.is_none() {
        return Ok(Restoring::NotDeleted);
    }
    match row.parent_id {
        Some(parent) => {
            let parent_kind = rows
                .iter()
                .find(|other| other.id == parent)
                .map(|other| other.kind.parse::<NodeKind>())
                .transpose()?
                .unwrap_or(NodeKind::Org);
            let manage = NodePermission::Manage
                .on(parent_kind)
                .ok_or(Error::NotFound(parent))?;
            Ok(
                match nodes::check(conn, principal, parent, manage, Act::Use, policy).await? {
                    Decision::Allowed { .. } => Restoring::Allowed,
                    Decision::Refused(refusal) => Restoring::Refused(Box::new(refusal)),
                },
            )
        }
        None => {
            if principal.is_system_admin() {
                return Ok(Restoring::Allowed);
            }
            let key = PageKey::new("node", node, "live");
            let person = principal.person();
            let owns = access::owner_entries(conn, std::slice::from_ref(&key))
                .await?
                .iter()
                .filter_map(|entry| crate::engine::entry_of(entry).ok())
                .any(|entry| {
                    entry.counts(principal.now()) && entry.holder.matches(&person, |_| None)
                });
            Ok(if owns {
                Restoring::Allowed
            } else {
                Restoring::OwnersAndSystemAdminsOnly
            })
        }
    }
}

// ---- A fork -------------------------------------------------------------------------------------

/// How a fork's page starts, as its maker chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForkPage {
    /// As the thread it was taken from: every row that was changed there is copied, and every row
    /// that followed follows the defaults of where the fork is made.
    AsTheOriginal,
    /// As the place decides: nothing is copied and every row follows those defaults, like any new
    /// thread.
    AsThePlaceDecides,
}

/// Makes `maker` the owner of the fork `fork` of the thread `source`, alone in its Owners row, and
/// gives it the page it starts with. The owners of the source are not carried over, and the source
/// is not touched. Further rows that its maker wants changed are the ordinary change of the page,
/// made after.
pub async fn start_fork(
    conn: &mut Conn,
    source: Id,
    fork: Id,
    maker: Id,
    how: ForkPage,
    at: Time,
) -> Result<(), Error> {
    pages::start(conn, StoredKind::Node, fork, maker, at).await?;
    if how == ForkPage::AsTheOriginal {
        access::copy_page(
            conn,
            &PageKey::new("node", source, "live"),
            &PageKey::new("node", fork, "live"),
            maker,
            at.get(),
        )
        .await?;
    }
    Ok(())
}
