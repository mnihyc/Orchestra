//! What a person holds and through what, and refusals with everything that is missing: the nearest people who can give it and how many more could, what an admin of the place may force, and what nobody can give.
//!
//! [`holds`] is the one rule by which a person holds a permission on a
//! thing: as one of its owners, as an admin of its place for the right to
//! view and manage it while their session's As admin mode is on, through an explicitly
//! named supplying resource's ownership, or because a row names them at that level or higher.
//! Nothing else gives anything, and nothing stands in for a person:
//! every act is asked of whoever does it, when they do it.
//!
//! An act that is refused is refused with everything the caller lacks for
//! it, object by object, as a [`Refusal`]: one request can then ask for all
//! of it, of the [nearest] people who can give each part. An admin
//! of the place does not ask. They may force the act by being put on the
//! page in the same step, which is an override and is recorded as one
//! ([`is_override`]).

use std::collections::BTreeSet;

use orch_types::Id;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::pages::Origin;
use crate::permissions::{Level, ObjectKind, Permission, WhenArchived};
use crate::roles::{PlaceRef, UnknownName, words};

/// One thing that has a permission page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "AccessObjectRef")]
pub struct ObjectRef {
    /// What kind of thing it is.
    pub kind: ObjectKind,
    /// Which one.
    pub id: Id,
}

/// What is known of one person on one thing, apart from what its rows name
/// them for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Standing {
    /// They are suspended: altogether, or in an org that the thing lies in.
    /// Somebody suspended holds nothing there.
    pub suspended: bool,
    /// An entry of the thing's Owners row is them, and they are an active
    /// member of its root org, which is the while for which owning counts.
    pub owner: bool,
    /// Ownership of a supplying resource intrinsically grants this level here.
    /// It never changes this object's Owners row or administrative standing.
    pub source_owner: Option<(ObjectRef, Permission)>,
    /// They are an admin of the place the thing is in: they hold Admin at it
    /// or at a place above, the system among those for a caller for whom
    /// that counts.
    pub administers: bool,
    /// The thing is a resource of the system, and they are a system admin
    /// for whom that counts.
    pub system_resource_admin: bool,
    /// The place of the thing lets the owners of a thing there name somebody
    /// from outside its root org themselves.
    pub owners_name_outsiders: bool,
}

/// An entry that names a person in a row, with the page it stands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Via {
    /// The entry.
    pub entry: Id,
    /// Where it stands: on the thing's own page, or on what that follows.
    pub from: Origin,
}

/// One way in which a person holds a permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Through {
    /// They are one of the thing's owners.
    Owner,
    /// They own the supplying resource that intrinsically grants a level here.
    SourceOwner {
        /// The supplying resource, not the target's Owners row.
        object: ObjectRef,
    },
    /// Their session's As admin mode gives View and Manage in its administrative scope.
    Admin,
    /// They are a system admin, and the thing is the system's own.
    SystemAdmin,
    /// They hold a higher level of the area, or the level this advanced row
    /// belongs with.
    Level {
        /// That permission.
        of: Permission,
    },
    /// An entry names them, a group of theirs or a role they hold.
    Entry(Via),
}

/// The ways in which a person holds `permission` on a thing; none when they
/// do not hold it.
///
/// `named` gives, for a row of the thing's page, the entries that count now
/// and whose holder is the person: on the row itself and on what it follows,
/// or under a lock, on what the lock decides.
///
/// Somebody suspended holds nothing. An owner holds every permission of the
/// page but the naming of somebody from outside, which they hold where the
/// place lets owners. An admin of the place holds the right to change the
/// page. On a resource of the system its admins hold everything. Beyond
/// that a permission is held by whom its row names, and by whom a higher
/// level of its area names, or for an advanced row the level it belongs
/// with. Of several ways the wider counts; no entry takes anything away.
pub fn holds(
    permission: Permission,
    standing: &Standing,
    mut named: impl FnMut(Permission) -> Vec<Via>,
) -> Vec<Through> {
    let mut ways = Vec::new();
    if standing.suspended {
        return ways;
    }
    if standing.owner
        && (permission != Permission::AccessExternal || standing.owners_name_outsiders)
    {
        ways.push(Through::Owner);
    }
    if let Some((object, level)) = standing.source_owner
        && (permission == level || permission.held_through().any(|higher| higher == level))
    {
        ways.push(Through::SourceOwner { object });
    }
    if standing.administers && native_admin_holds(permission) {
        ways.push(Through::Admin);
    }
    if standing.system_resource_admin
        && (native_admin_holds(permission)
            || matches!(
                permission,
                Permission::MachineUse | Permission::MachineManage
            ))
    {
        ways.push(Through::SystemAdmin);
    }
    // Who owns a thing is `standing.owner`: the Owners row is asked nothing.
    if permission != Permission::AccessOwn {
        ways.extend(named(permission).into_iter().map(Through::Entry));
        for higher in permission
            .held_through()
            .filter(|higher| *higher != Permission::AccessOwn)
        {
            if !named(higher).is_empty() {
                ways.push(Through::Level { of: higher });
            }
        }
    }
    ways
}

/// The permissions a viewer and manager hold. Administration is not ownership, and
/// does not give advanced acts which no View or Manage level includes. A person's
/// machine requires an explicit named grant before its programs or identity are managed.
#[must_use]
pub fn native_admin_holds(permission: Permission) -> bool {
    !matches!(
        permission,
        Permission::AccessOwn | Permission::MachineUse | Permission::MachineManage
    ) && (matches!(permission.level(), Level::View | Level::Manage)
        || permission
            .held_through()
            .any(|level| matches!(level.level(), Level::View | Level::Manage)))
}

/// A permission that a person holds, and every way in which they hold it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Held {
    /// The permission.
    pub permission: Permission,
    /// The ways; never empty.
    pub through: Vec<Through>,
}

/// Everything a person holds on a thing of kind `object`, in the order of
/// the catalogue, each with the ways in which they hold it.
///
/// `archived` says that the thing is archived, itself or through a node
/// above it: what is held is then cut down to what archive leaves, which is
/// looking, saying who can, and what deletes the node or brings it back.
/// `named` is as for [`holds`], and is asked once for each row.
pub fn effective(
    object: ObjectKind,
    standing: &Standing,
    archived: bool,
    mut named: impl FnMut(Permission) -> Vec<Via>,
) -> Vec<Held> {
    let rows: Vec<(Permission, Vec<Via>)> = object
        .permissions()
        .filter(|permission| *permission != Permission::AccessOwn)
        .map(|permission| (permission, named(permission)))
        .collect();
    let row = |permission: Permission| {
        rows.iter()
            .find(|(of, _)| *of == permission)
            .map(|(_, entries)| entries.clone())
            .unwrap_or_default()
    };
    object
        .permissions()
        .filter(|permission| !archived || permission.when_archived() != WhenArchived::Withheld)
        .map(|permission| Held {
            permission,
            through: holds(permission, standing, row),
        })
        .filter(|held| !held.through.is_empty())
        .collect()
}

/// The scope that stands for everything its user holds.
pub const ALL_SCOPES: &str = "*";

/// The scopes of an API token: what of its user's permissions it carries.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Scopes {
    /// No narrowing: everything the user holds.
    All,
    /// These permissions and no others.
    Only(BTreeSet<Permission>),
}

impl Scopes {
    /// Reads a list of scopes: [`ALL_SCOPES`], or keys of permissions of the
    /// catalogue. A name that is neither is refused.
    pub fn parse<S: AsRef<str>>(scopes: &[S]) -> Result<Self, UnknownName> {
        let mut only = BTreeSet::new();
        let mut all = false;
        for scope in scopes {
            match scope.as_ref() {
                ALL_SCOPES => all = true,
                key => {
                    only.insert(
                        key.parse::<Permission>()
                            .map_err(|_| UnknownName::new("scope", key))?,
                    );
                }
            }
        }
        Ok(if all { Self::All } else { Self::Only(only) })
    }

    /// Whether a token of these scopes carries `permission`, where its user
    /// holds it. A token never has a permission its user lacks.
    #[must_use]
    pub fn allow(&self, permission: Permission) -> bool {
        match self {
            Self::All => true,
            Self::Only(only) => only.contains(&permission),
        }
    }

    /// The scopes as they are written.
    #[must_use]
    pub fn to_strings(&self) -> Vec<String> {
        match self {
            Self::All => vec![ALL_SCOPES.to_owned()],
            Self::Only(only) => only
                .iter()
                .map(|permission| permission.as_str().to_owned())
                .collect(),
        }
    }
}

/// Whether a token that is kept to the subtree of `subtree` reaches a
/// request.
///
/// `path` is the nodes from the root org down to the node the request acts
/// on, or through which it reaches a resource; `None` for a request about a
/// resource that goes through no node. A token that is kept to no subtree
/// reaches everything. One that is, is refused on any node outside it, and
/// on every request about a resource but those reached through a node in it.
#[must_use]
pub fn token_reaches(subtree: Option<Id>, path: Option<&[Id]>) -> bool {
    subtree.is_none_or(|root| path.is_some_and(|path| path.contains(&root)))
}

words! {
    /// Why nobody can give what is missing.
    pub enum Closed ("reason") {
        /// The thread runs on a subscription that its org keeps to its
        /// owner: nobody else uses it, whatever its page says. The way on is
        /// a switch to a provider the person may use.
        SubscriptionNotShared = "subscription_not_shared",
        /// The row is under a lock set at a place above. The way on is
        /// whoever may change that place's defaults.
        Locked = "locked",
    }
}

/// The least of a provider's models and efforts that would do, for a
/// missing use of it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MissingOptions {
    /// The models.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<String>>,
    /// The efforts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub efforts: Option<Vec<String>>,
}

/// The thing that something is missing on, with the little of it that a
/// refusal tells.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MissingObject {
    /// What kind of thing it is.
    pub kind: ObjectKind,
    /// Which one.
    pub id: Id,
    /// Its name, where the caller may know it. A resource is named only to
    /// somebody who can already see the thread that runs on it.
    pub name: Option<String>,
    /// Its first owner, where the caller may know them.
    pub owner: Option<Id>,
}

/// The most people a refusal names as able to give what is missing. A
/// refusal is read by one person who wants to know whom to ask: five names
/// answer that, and no refusal carries the people behind a role or a group.
pub const ASK_MOST: usize = 5;

/// What a person lacks on one thing for an act, and who can give it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MissingAccess {
    /// The thing.
    pub object: MissingObject,
    /// The permissions of its page that the act needs and the person does
    /// not hold.
    pub permissions: Vec<Permission>,
    /// For the use of a provider: the least that would do.
    pub options: Option<MissingOptions>,
    /// The nearest people who could approve a request for it now, at most
    /// [`ASK_MOST`]; empty when nobody can give it.
    pub ask: Vec<Id>,
    /// How many others could; they are never listed.
    pub ask_more: i64,
    /// Whether the caller administers the thing and may force this.
    pub r#override: bool,
    /// Why nobody can give it, when nobody can. A request then leaves the
    /// entry out, and nobody forces it either.
    pub closed: Option<Closed>,
    /// For a row under a lock: the place that locked it.
    pub locked_by: Option<PlaceRef>,
}

impl MissingAccess {
    /// Something that can be given: by `givers`, and by force where the
    /// caller `administers` the thing.
    #[must_use]
    pub fn open(
        object: MissingObject,
        permissions: Vec<Permission>,
        options: Option<MissingOptions>,
        givers: &[Giver],
        administers: bool,
    ) -> Self {
        let (ask, ask_more) = nearest(givers).ask();
        Self {
            object,
            permissions,
            options,
            ask,
            ask_more,
            r#override: administers,
            closed: None,
            locked_by: None,
        }
    }

    /// Something that nobody can give, an admin of the place included: the
    /// use of a subscription that is kept to its owner.
    #[must_use]
    pub fn kept_to_its_owner(object: MissingObject, options: Option<MissingOptions>) -> Self {
        Self {
            object,
            permissions: vec![Permission::ProviderUse],
            options,
            ask: Vec::new(),
            ask_more: 0,
            r#override: false,
            closed: Some(Closed::SubscriptionNotShared),
            locked_by: None,
        }
    }

    /// Something that nobody can give on this page, an admin of the place
    /// included: rows under a lock that was set at `place`.
    #[must_use]
    pub fn locked(object: MissingObject, permissions: Vec<Permission>, place: PlaceRef) -> Self {
        Self {
            object,
            permissions,
            options: None,
            ask: Vec::new(),
            ask_more: 0,
            r#override: false,
            closed: Some(Closed::Locked),
            locked_by: Some(place),
        }
    }
}

/// An act that was refused: what it needed, and everything the caller lacks
/// for it, not only the first thing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Refusal {
    /// The permission the act needs on the thing it was asked of.
    pub required: Permission,
    /// That thing.
    pub object: ObjectRef,
    /// Whether a request for access can be made: somebody can give some of
    /// what is missing.
    pub requestable: bool,
    /// Whether the caller may force the act: they administer every thing in
    /// `missing`, and none of it is closed to everybody.
    pub r#override: bool,
    /// Everything the caller lacks, thing by thing, each with who can give
    /// it.
    pub missing: Vec<MissingAccess>,
}

impl Refusal {
    /// The refusal of an act that needs `required` on `object` and for
    /// which the caller lacks `missing`.
    #[must_use]
    pub fn of(required: Permission, object: ObjectRef, missing: Vec<MissingAccess>) -> Self {
        let requestable = missing.iter().any(|entry| entry.closed.is_none());
        let r#override = !missing.is_empty()
            && missing
                .iter()
                .all(|entry| entry.r#override && entry.closed.is_none());
        Self {
            required,
            object,
            requestable,
            r#override,
            missing,
        }
    }
}

/// Whether a person may do an act, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum Decision {
    /// They may.
    Allowed {
        /// The ways in which they hold what the act needs.
        through: Vec<Through>,
    },
    /// They may not.
    Refused(Refusal),
}

impl Decision {
    /// The decision that follows from the `ways` in which a person holds
    /// what an act needs; `refusal` is made only when there is none.
    pub fn from_ways(ways: Vec<Through>, refusal: impl FnOnce() -> Refusal) -> Self {
        if ways.is_empty() {
            Self::Refused(refusal())
        } else {
            Self::Allowed { through: ways }
        }
    }

    /// Whether the act is allowed.
    #[must_use]
    pub const fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed { .. })
    }
}

/// How near somebody who can give access to a thing stands to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Nearness {
    /// One of its owners.
    Owner,
    /// Named on its page as able to change it.
    Named,
    /// An admin of a place above it: by a role held there, or as an owner
    /// of that place.
    Admin {
        /// How many places lie between the thing and the place where they
        /// hold that: 0 for the place the thing is in, and the system last.
        above: u32,
    },
}

/// Somebody who can give access to a thing now: they would pass the check
/// for changing its page at this moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Giver {
    /// The person.
    pub user: Id,
    /// How near they stand.
    pub nearness: Nearness,
}

/// The people who can give access to a thing, split by whom a request goes
/// to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Givers {
    /// The nearest: a request is sent to them.
    pub nearest: Vec<Id>,
    /// Admins further up. They pass the check too and may decide, and are
    /// sent nothing while somebody nearer exists.
    pub further: Vec<Id>,
}

impl Givers {
    /// What a refusal says of them: the first of the nearest, at most
    /// [`ASK_MOST`], and how many others could give it.
    #[must_use]
    pub fn ask(&self) -> (Vec<Id>, i64) {
        let ask: Vec<Id> = self.nearest.iter().copied().take(ASK_MOST).collect();
        let others = self.nearest.len() + self.further.len() - ask.len();
        (ask, i64::try_from(others).unwrap_or(i64::MAX))
    }
}

/// Splits the people who can give access to a thing into those a request is
/// sent to and those further up.
///
/// The nearest are the thing's owners, whoever its page names as able to
/// change it, and the admins of the first place above it that has any. An
/// org's admins are so not asked about every thread of a project that has
/// admins of its own, nor the system's about every org's. Somebody who is
/// given twice counts once, as near as they stand at their nearest, and
/// everybody keeps the order in which they were given.
#[must_use]
pub fn nearest(givers: &[Giver]) -> Givers {
    let rank = |nearness: Nearness| match nearness {
        Nearness::Owner | Nearness::Named => None,
        Nearness::Admin { above } => Some(above),
    };
    let first_admins = givers.iter().filter_map(|giver| rank(giver.nearness)).min();
    let is_near =
        |giver: &Giver| rank(giver.nearness).is_none_or(|above| Some(above) == first_admins);
    let mut split = Givers::default();
    for giver in givers.iter().filter(|giver| is_near(giver)) {
        if !split.nearest.contains(&giver.user) {
            split.nearest.push(giver.user);
        }
    }
    for giver in givers.iter().filter(|giver| !is_near(giver)) {
        if !split.nearest.contains(&giver.user) && !split.further.contains(&giver.user) {
            split.further.push(giver.user);
        }
    }
    split
}

/// Whether an entry by which somebody names themself on a page is an override,
/// to be marked so in the audit trail.
///
/// `manages` is every way in which they hold the right to change that page,
/// and `held` every way in which they held the row's permission before. It
/// is an override when they change the page by administering it and by nothing
/// else, and put themself in a row they did not hold: an admin forcing what
/// the page does not give them. An act that their page already gives them
/// is none, and nobody but an admin of the place has one.
#[must_use]
pub fn is_override(manages: &[Through], held: &[Through]) -> bool {
    !manages.is_empty()
        && manages.iter().all(|way| matches!(way, Through::Admin))
        && held.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::Area;
    use orch_types::new_id;
    use serde_json::json;

    use Permission as P;

    fn nobody(_: Permission) -> Vec<Via> {
        Vec::new()
    }

    fn via() -> Via {
        Via {
            entry: new_id(),
            from: Origin::Own,
        }
    }

    /// Somebody whom the rows `rows` name, and no others.
    fn named_in(rows: &[Permission]) -> impl FnMut(Permission) -> Vec<Via> + '_ {
        move |row| {
            if rows.contains(&row) {
                vec![via()]
            } else {
                Vec::new()
            }
        }
    }

    fn object(kind: ObjectKind) -> MissingObject {
        MissingObject {
            kind,
            id: new_id(),
            name: Some("Fix login".into()),
            owner: Some(new_id()),
        }
    }

    #[test]
    fn an_owner_holds_everything_on_the_page_but_naming_somebody_from_outside() {
        let owner = Standing {
            owner: true,
            ..Standing::default()
        };
        for permission in ObjectKind::Thread.permissions() {
            let ways = holds(permission, &owner, nobody);
            if permission == P::AccessExternal {
                assert_eq!(
                    ways,
                    [],
                    "if owners held it, the policy would decide nothing"
                );
            } else {
                assert_eq!(ways, [Through::Owner], "{permission}");
            }
        }
    }

    #[test]
    fn an_owner_names_somebody_from_outside_where_the_place_lets_owners() {
        let owner = Standing {
            owner: true,
            owners_name_outsiders: true,
            ..Standing::default()
        };
        assert_eq!(holds(P::AccessExternal, &owner, nobody), [Through::Owner]);
        let stranger = Standing {
            owners_name_outsiders: true,
            ..Standing::default()
        };
        assert_eq!(holds(P::AccessExternal, &stranger, nobody), []);
    }

    #[test]
    fn a_source_owner_manages_only_the_derived_level_without_becoming_target_owner_or_admin() {
        let object = ObjectRef {
            kind: ObjectKind::Machine,
            id: new_id(),
        };
        let source = Standing {
            source_owner: Some((object, P::EnvironmentManage)),
            ..Standing::default()
        };
        for permission in [
            P::EnvironmentManage,
            P::EnvironmentView,
            P::FilesView,
            P::FilesManage,
            P::TerminalView,
            P::TerminalManage,
            P::BrowserView,
            P::BrowserManage,
            P::VncView,
            P::VncManage,
        ] {
            assert_eq!(
                holds(permission, &source, nobody),
                [Through::SourceOwner { object }],
                "{permission}"
            );
        }
        assert!(holds(P::AccessManage, &source, nobody).is_empty());
        assert!(holds(P::AccessOwn, &source, nobody).is_empty());
        assert!(holds(P::AccessExternal, &source, nobody).is_empty());
        let suspended = Standing {
            suspended: true,
            ..source
        };
        assert!(holds(P::EnvironmentManage, &suspended, nobody).is_empty());
        let way = Through::SourceOwner { object };
        let encoded = serde_json::to_string(&way).unwrap();
        assert_eq!(serde_json::from_str::<Through>(&encoded).unwrap(), way);
    }

    #[test]
    fn an_enabled_admin_views_and_manages_without_becoming_an_owner() {
        let admin = Standing {
            administers: true,
            ..Standing::default()
        };
        assert_eq!(holds(P::AccessManage, &admin, nobody), [Through::Admin]);
        for permission in [
            P::ThreadView,
            P::ThreadInteract,
            P::EnvironmentManage,
            P::FilesView,
        ] {
            assert_eq!(
                holds(permission, &admin, nobody),
                [Through::Admin],
                "{permission}"
            );
        }
        for permission in [P::AccessExternal, P::AccessOwn] {
            assert_eq!(holds(permission, &admin, nobody), [], "{permission}");
        }
    }

    #[test]
    fn administering_a_persons_machine_requires_a_named_grant_before_using_or_managing_it() {
        let admin = Standing {
            administers: true,
            ..Standing::default()
        };
        assert_eq!(holds(P::MachineView, &admin, nobody), [Through::Admin]);
        assert_eq!(holds(P::AccessManage, &admin, nobody), [Through::Admin]);
        for permission in [P::MachineUse, P::MachineManage] {
            assert!(holds(permission, &admin, nobody).is_empty(), "{permission}");
            let granted = holds(permission, &admin, |row| {
                if row == P::MachineManage {
                    vec![via()]
                } else {
                    Vec::new()
                }
            });
            assert!(
                !granted.is_empty(),
                "an explicitly named Manage entry gives {permission}"
            );
            assert!(!granted.contains(&Through::Admin));
        }
        assert_eq!(holds(P::HostManage, &admin, nobody), [Through::Admin]);
        let system = Standing {
            system_resource_admin: true,
            ..Standing::default()
        };
        for permission in [P::MachineUse, P::MachineManage] {
            assert_eq!(
                holds(permission, &system, nobody),
                [Through::SystemAdmin],
                "the system's own resources retain their existing administration"
            );
        }
    }

    #[test]
    fn an_enabled_system_admin_is_a_viewer_and_manager_of_a_system_resource() {
        let admin = Standing {
            system_resource_admin: true,
            ..Standing::default()
        };
        for permission in ObjectKind::Host.permissions() {
            assert_eq!(
                holds(permission, &admin, nobody),
                if native_admin_holds(permission) {
                    vec![Through::SystemAdmin]
                } else {
                    vec![]
                },
                "{permission}"
            );
        }
    }

    #[test]
    fn manage_of_an_area_gives_its_lower_levels_and_its_advanced_rows() {
        let nothing = Standing::default();
        let manages = [P::ThreadManage];
        for permission in [
            P::ThreadView,
            P::ThreadInteract,
            P::ThreadFork,
            P::ThreadArchive,
        ] {
            let ways = holds(permission, &nothing, named_in(&manages));
            assert_eq!(
                ways,
                [Through::Level {
                    of: P::ThreadManage
                }],
                "{permission}"
            );
        }
        let direct = holds(P::ThreadManage, &nothing, named_in(&manages));
        assert!(matches!(direct[..], [Through::Entry(_)]));
        let writes = [P::DocEdit];
        assert_eq!(
            holds(P::DocCreate, &nothing, named_in(&writes)),
            [Through::Level { of: P::DocEdit }]
        );
        assert_eq!(holds(P::DocManage, &nothing, named_in(&writes)), []);
    }

    #[test]
    fn nothing_on_one_area_gives_anything_on_another_but_an_environments_manage_holding_its_doors()
    {
        let nothing = Standing::default();
        let thread_manager = [P::ThreadManage];
        assert_eq!(holds(P::FilesView, &nothing, named_in(&thread_manager)), []);
        assert_eq!(
            holds(P::TerminalManage, &nothing, named_in(&thread_manager)),
            [],
            "managing a thread opens nothing of its environment"
        );
        let files_manager = [P::FilesManage];
        assert_eq!(
            holds(P::ThreadInteract, &nothing, named_in(&files_manager)),
            []
        );
        assert_eq!(
            holds(P::EnvironmentView, &nothing, named_in(&files_manager)),
            [],
            "a door is not the environment"
        );
        let environment_manager = [P::EnvironmentManage];
        assert_eq!(
            holds(P::ThreadInteract, &nothing, named_in(&environment_manager)),
            [],
            "managing an environment gives no word in any thread there"
        );
        for door in [
            P::FilesView,
            P::FilesManage,
            P::TerminalView,
            P::TerminalManage,
            P::BrowserView,
            P::BrowserManage,
            P::VncView,
            P::VncManage,
        ] {
            assert_eq!(
                holds(door, &nothing, named_in(&environment_manager)),
                [Through::Level {
                    of: P::EnvironmentManage
                }],
                "{door} is held by whoever manages the environment, with nobody named in it"
            );
        }
        let page_manager = [P::AccessManage];
        assert_eq!(
            holds(P::AccessExternal, &nothing, named_in(&page_manager)),
            []
        );
        assert_eq!(holds(P::AccessOwn, &nothing, named_in(&page_manager)), []);
    }

    #[test]
    fn somebody_suspended_holds_nothing_whatever_they_own() {
        let suspended = Standing {
            suspended: true,
            owner: true,
            administers: true,
            system_resource_admin: true,
            ..Standing::default()
        };
        let every_row = |_: Permission| vec![via()];
        for permission in P::ALL {
            assert_eq!(
                holds(*permission, &suspended, every_row),
                [],
                "{permission}"
            );
        }
    }

    #[test]
    fn what_somebody_holds_is_listed_with_how() {
        let nothing = Standing::default();
        let rows = [P::ThreadInteract, P::ChatView];
        let held = effective(ObjectKind::Thread, &nothing, false, named_in(&rows));
        let permissions: Vec<_> = held.iter().map(|held| held.permission).collect();
        assert_eq!(permissions, [P::ChatView, P::ThreadView, P::ThreadInteract]);
        assert_eq!(
            held[1].through,
            [Through::Level {
                of: P::ThreadInteract
            }]
        );
    }

    #[test]
    fn the_doors_are_listed_among_what_somebody_holds_on_an_environment() {
        let nothing = Standing::default();
        let doors = [P::FilesView, P::TerminalManage];
        let held = effective(ObjectKind::Environment, &nothing, false, named_in(&doors));
        let permissions: Vec<_> = held.iter().map(|held| held.permission).collect();
        assert_eq!(
            permissions,
            [P::FilesView, P::TerminalView, P::TerminalManage],
            "one door to look, one to reach in, and the environment itself not at all"
        );
        let manager = [P::EnvironmentManage];
        let held = effective(ObjectKind::Environment, &nothing, false, named_in(&manager));
        assert_eq!(
            held.len(),
            ObjectKind::Environment
                .permissions()
                .filter(|row| !matches!(row.area(), Area::Access))
                .count(),
            "all of it, each door too, with nobody named in a door"
        );
        let browser = held
            .iter()
            .find(|held| held.permission == P::BrowserManage)
            .unwrap();
        assert_eq!(
            browser.through,
            [Through::Level {
                of: P::EnvironmentManage
            }]
        );
    }

    #[test]
    fn under_archive_what_is_held_is_cut_down_to_looking_and_bringing_back() {
        let owner = Standing {
            owner: true,
            ..Standing::default()
        };
        let held: Vec<_> = effective(ObjectKind::Thread, &owner, true, nobody)
            .iter()
            .map(|held| held.permission)
            .collect();
        assert!(
            held.contains(&P::ThreadView)
                && held.contains(&P::ChatView)
                && held.contains(&P::DocView)
        );
        assert!(
            held.contains(&P::AccessManage)
                && held.contains(&P::AuditView)
                && held.contains(&P::UsageView)
        );
        assert!(
            held.contains(&P::ThreadManage) && held.contains(&P::ThreadArchive),
            "to delete it and to bring it back"
        );
        for gone in [
            P::ThreadInteract,
            P::ChatInteract,
            P::DocEdit,
            P::DocManage,
            P::ThreadFork,
        ] {
            assert!(!held.contains(&gone), "{gone}");
        }
        let all = effective(ObjectKind::Thread, &owner, false, nobody).len();
        assert!(held.len() < all);
    }

    #[test]
    fn nobody_holds_more_when_an_entry_is_removed() {
        let nothing = Standing::default();
        let pages: [(ObjectKind, [Permission; 3]); 2] = [
            (
                ObjectKind::Thread,
                [P::ThreadManage, P::DocEdit, P::ChatInteract],
            ),
            (
                ObjectKind::Environment,
                [P::EnvironmentManage, P::FilesView, P::BrowserView],
            ),
        ];
        for (kind, before) in pages {
            for removed in before {
                let after: Vec<_> = before
                    .iter()
                    .copied()
                    .filter(|row| *row != removed)
                    .collect();
                let then: Vec<_> = effective(kind, &nothing, false, named_in(&before))
                    .iter()
                    .map(|held| held.permission)
                    .collect();
                for held in effective(kind, &nothing, false, named_in(&after)) {
                    assert!(
                        then.contains(&held.permission),
                        "{} appeared when {removed} went",
                        held.permission
                    );
                }
            }
        }
    }

    #[test]
    fn a_token_carries_what_its_scopes_name_and_nothing_else() {
        let scoped = Scopes::parse(&["thread.view", "provider.manage"]).unwrap();
        assert!(scoped.allow(P::ThreadView) && scoped.allow(P::ProviderManage));
        assert!(!scoped.allow(P::ThreadInteract));
        assert!(
            !scoped.allow(P::ProviderView),
            "a scope is the permission it names, not what that would give"
        );
        assert_eq!(scoped.to_strings(), ["provider.manage", "thread.view"]);
        let all = Scopes::parse(&["*"]).unwrap();
        assert!(P::ALL.iter().all(|permission| all.allow(*permission)));
        assert_eq!(all.to_strings(), ["*"]);
        assert!(Scopes::parse(&["thread.read"]).is_err());
        assert!(
            Scopes::parse(&["node.view"]).is_err(),
            "a shorthand is no scope"
        );
        let none = Scopes::parse::<&str>(&[]).unwrap();
        assert!(!P::ALL.iter().any(|permission| none.allow(*permission)));
    }

    #[test]
    fn a_token_kept_to_a_subtree_is_refused_outside_it() {
        let (root, project, topic, elsewhere) = (new_id(), new_id(), new_id(), new_id());
        let inside = [root, project, topic];
        let outside = [root, elsewhere];
        assert!(token_reaches(Some(project), Some(&inside)));
        assert!(!token_reaches(Some(project), Some(&outside)));
        assert!(
            !token_reaches(Some(project), None),
            "a resource that is reached through no node"
        );
        assert!(token_reaches(None, Some(&outside)));
        assert!(token_reaches(None, None));
    }

    #[test]
    fn a_request_goes_to_the_nearest_who_can_give_and_those_above_may_decide() {
        let (owner, named, project_admin, org_admin, system_admin) =
            (new_id(), new_id(), new_id(), new_id(), new_id());
        let givers = [
            Giver {
                user: system_admin,
                nearness: Nearness::Admin { above: 3 },
            },
            Giver {
                user: owner,
                nearness: Nearness::Owner,
            },
            Giver {
                user: org_admin,
                nearness: Nearness::Admin { above: 2 },
            },
            Giver {
                user: named,
                nearness: Nearness::Named,
            },
            Giver {
                user: project_admin,
                nearness: Nearness::Admin { above: 1 },
            },
        ];
        let split = nearest(&givers);
        assert_eq!(
            split.nearest,
            [owner, named, project_admin],
            "the topic itself has no admin of its own"
        );
        assert_eq!(split.further, [system_admin, org_admin]);
        assert_eq!(split.ask(), (vec![owner, named, project_admin], 2));
    }

    #[test]
    fn somebody_who_can_give_in_two_ways_is_asked_once() {
        let (alice, bo) = (new_id(), new_id());
        let givers = [
            Giver {
                user: alice,
                nearness: Nearness::Admin { above: 2 },
            },
            Giver {
                user: alice,
                nearness: Nearness::Owner,
            },
            Giver {
                user: bo,
                nearness: Nearness::Admin { above: 0 },
            },
            Giver {
                user: bo,
                nearness: Nearness::Admin { above: 2 },
            },
        ];
        let split = nearest(&givers);
        assert_eq!(split.nearest, [alice, bo]);
        assert!(split.further.is_empty());
    }

    #[test]
    fn a_refusal_names_five_people_at_most_and_counts_the_rest() {
        let givers: Vec<_> = (0..8)
            .map(|_| Giver {
                user: new_id(),
                nearness: Nearness::Owner,
            })
            .collect();
        let missing = MissingAccess::open(
            object(ObjectKind::Thread),
            vec![P::ThreadInteract],
            None,
            &givers,
            false,
        );
        assert_eq!(missing.ask.len(), ASK_MOST);
        assert_eq!(missing.ask_more, 3);
        assert_eq!(
            missing.ask,
            givers
                .iter()
                .take(5)
                .map(|giver| giver.user)
                .collect::<Vec<_>>()
        );
        assert_eq!(nearest(&[]).ask(), (Vec::new(), 0));
    }

    #[test]
    fn an_admin_of_the_place_may_force_what_they_administer_all_of() {
        let thread = ObjectRef {
            kind: ObjectKind::Thread,
            id: new_id(),
        };
        let givers = [Giver {
            user: new_id(),
            nearness: Nearness::Owner,
        }];
        let on_thread = |administers| {
            MissingAccess::open(
                object(ObjectKind::Thread),
                vec![P::ThreadInteract],
                None,
                &givers,
                administers,
            )
        };
        let on_provider = |administers| {
            let options = MissingOptions {
                models: Some(vec!["gpt-x".into()]),
                efforts: Some(vec!["high".into()]),
            };
            MissingAccess::open(
                object(ObjectKind::Provider),
                vec![P::ProviderUse],
                Some(options),
                &givers,
                administers,
            )
        };
        let all = Refusal::of(
            P::ThreadInteract,
            thread,
            vec![on_thread(true), on_provider(true)],
        );
        assert!(all.r#override && all.requestable);
        let other_orgs_provider = Refusal::of(
            P::ThreadInteract,
            thread,
            vec![on_thread(true), on_provider(false)],
        );
        assert!(!other_orgs_provider.r#override && other_orgs_provider.requestable);
        let nobody_administers = Refusal::of(P::ThreadInteract, thread, vec![on_thread(false)]);
        assert!(!nobody_administers.r#override);
    }

    #[test]
    fn what_nobody_can_give_is_neither_asked_for_nor_forced() {
        let thread = ObjectRef {
            kind: ObjectKind::Thread,
            id: new_id(),
        };
        let kept = MissingAccess::kept_to_its_owner(object(ObjectKind::Provider), None);
        assert_eq!(
            (kept.ask.len(), kept.ask_more, kept.r#override),
            (0, 0, false)
        );
        assert_eq!(kept.closed, Some(Closed::SubscriptionNotShared));
        let org = PlaceRef::Node(new_id());
        let locked = MissingAccess::locked(
            object(ObjectKind::Environment),
            vec![P::TerminalManage],
            org,
        );
        assert_eq!(
            (locked.closed, locked.locked_by),
            (Some(Closed::Locked), Some(org))
        );

        let closed = Refusal::of(P::ThreadInteract, thread, vec![kept.clone()]);
        assert!(!closed.requestable && !closed.r#override);
        let givers = [Giver {
            user: new_id(),
            nearness: Nearness::Owner,
        }];
        let open = MissingAccess::open(
            object(ObjectKind::Thread),
            vec![P::ThreadInteract],
            None,
            &givers,
            true,
        );
        let mixed = Refusal::of(P::ThreadInteract, thread, vec![open, kept]);
        assert!(
            mixed.requestable,
            "the part that somebody can give is asked for"
        );
        assert!(
            !mixed.r#override,
            "an admin does not force a subscription that is kept to its owner"
        );
    }

    #[test]
    fn an_admin_naming_themself_where_the_page_gave_nothing_is_an_override() {
        let entry = Through::Entry(via());
        assert!(is_override(&[Through::Admin], &[]));
        assert!(
            !is_override(&[Through::Admin], &[entry]),
            "an act their page already gives them"
        );
        assert!(
            !is_override(&[Through::Admin, Through::Owner], &[]),
            "an owner changes their own page"
        );
        assert!(
            !is_override(&[entry], &[]),
            "somebody the page names as able to change it"
        );
        assert!(
            !is_override(&[], &[]),
            "nobody but an admin of the place has one"
        );
        assert!(!is_override(&[Through::SystemAdmin], &[]));
    }

    #[test]
    fn a_decision_is_allowed_through_something_or_refused_with_what_is_missing() {
        let thread = ObjectRef {
            kind: ObjectKind::Thread,
            id: new_id(),
        };
        let refusal = || Refusal::of(P::ThreadView, thread, Vec::new());
        assert!(Decision::from_ways(vec![Through::Owner], refusal).is_allowed());
        let refused = Decision::from_ways(Vec::new(), refusal);
        assert!(!refused.is_allowed());
        let Decision::Refused(refusal) = refused else {
            panic!("refused")
        };
        assert!(
            !refusal.requestable && !refusal.r#override,
            "nothing is listed, so nothing is asked for or forced"
        );
    }

    #[test]
    fn a_refusal_is_written_with_everything_that_is_missing() {
        let (thread, provider, asked) = (new_id(), new_id(), new_id());
        let refusal = Refusal::of(
            P::ThreadInteract,
            ObjectRef {
                kind: ObjectKind::Thread,
                id: thread,
            },
            vec![
                MissingAccess {
                    object: MissingObject {
                        kind: ObjectKind::Thread,
                        id: thread,
                        name: Some("Fix login".into()),
                        owner: None,
                    },
                    permissions: vec![P::ThreadInteract],
                    options: None,
                    ask: vec![asked],
                    ask_more: 0,
                    r#override: false,
                    closed: None,
                    locked_by: None,
                },
                MissingAccess::kept_to_its_owner(
                    MissingObject {
                        kind: ObjectKind::Provider,
                        id: provider,
                        name: None,
                        owner: None,
                    },
                    Some(MissingOptions {
                        models: Some(vec!["gpt-x".into()]),
                        efforts: None,
                    }),
                ),
            ],
        );
        let written = serde_json::to_value(&refusal).unwrap();
        assert_eq!(
            written,
            json!({
                "required": "thread.interact", "object": { "kind": "thread", "id": thread },
                "requestable": true, "override": false,
                "missing": [
                    { "object": { "kind": "thread", "id": thread, "name": "Fix login", "owner": null },
                      "permissions": ["thread.interact"], "options": null,
                      "ask": [asked], "ask_more": 0, "override": false, "closed": null, "locked_by": null },
                    { "object": { "kind": "provider", "id": provider, "name": null, "owner": null },
                      "permissions": ["provider.use"], "options": { "models": ["gpt-x"] },
                      "ask": [], "ask_more": 0, "override": false, "closed": "subscription_not_shared",
                      "locked_by": null }
                ]
            })
        );
        assert_eq!(serde_json::from_value::<Refusal>(written).unwrap(), refusal);
    }
}
