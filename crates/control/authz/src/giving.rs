//! Who may give what, and how much: the one rule behind an entry in a row, one of the three choices, a role in a place, a row of a defaults page, a share with agents and the decision of a request for access, which a page also tells beforehand.
//!
//! Changing who holds what is two acts. *Sharing* is naming somebody further in
//! a row one holds oneself, giving a role up to one's own, and sharing with
//! agents up to what one may do: it takes holding what is given, which is the
//! whole of it for a thing's owners and for whoever manages its page, and for
//! anybody else who holds it also that the place lets holders share (the
//! policy `sharing.allow_holders`, in [`Policy::holders_share`]). *Managing
//! the page* is everything else: taking out what others gave, cutting a row
//! loose, Private, a lock, and every change of a defaults page, and it takes
//! `access.manage`. A request that gives more than its giver holds is refused
//! whole, with what the giver lacks.
//!
//! [`Giving`] is the giver on one page. [`Giving::judge`] is the rule for a set
//! of rows, and is what a change of a page, the decision of a request and a
//! share with agents are each asked; [`Giving::can_give`] and [`Giving::gives`]
//! are the same rule told beforehand, so that a form offers nothing that would
//! be refused. There is no second copy of it: a role is given by the same
//! question about the role held in a place ([`Giving::role`]).
//!
//! **A defaults page.** An entry in a row of the defaults page of a place takes
//! `access.manage` on the place and holding the permission on a thing that
//! follows the row. That is asked of a thing made for the purpose: one that
//! lies directly in the place, has nothing stored for its page, and so follows
//! every default. The admins of the place (those who hold Admin there, its
//! owners among them) give any row of its defaults: they set what the pages
//! of their place start from, which includes the rows that as shipped nobody
//! holds. The share of a place with what is in it writes its defaults page as
//! part of the share and takes what the share takes: for a row that has one
//! form for a managed environment and one for a person's machine it is given
//! in the forms the sharer holds, and refused when they hold neither.

use orch_access_contract::decisions::{MissingAccess, MissingObject, ObjectRef, Refusal};
use orch_access_contract::pages::{CheckedUpdate, Entry, Holder, PageKind, PageOf};
use orch_access_contract::permissions::{
    Choice, NamedBy, ObjectKind, Permission, Placement, Profile,
};
use orch_access_contract::roles::{PlaceRef, Role};
use orch_db::Conn;
use orch_types::{Id, Json};
use std::collections::HashSet;

use crate::engine::{Frame, Graph, PageRef, Site};
use crate::{Error, Policy, Principal, missing};

/// Whether the checked options allow no more than the stored options. The option catalogue uses
/// exact-name allowlists and numeric maximums; an absent member is unbounded.
fn options_only_narrow(before: &Json, after: &Json) -> bool {
    match (before, after) {
        (Json::Null, _) => true,
        (Json::Object(before), Json::Object(after)) => before.iter().all(|(name, before)| {
            options_only_narrow(before, after.get(name).unwrap_or(&Json::Null))
        }),
        (Json::Object(before), Json::Null) => before
            .values()
            .all(|before| options_only_narrow(before, &Json::Null)),
        (Json::Array(before), Json::Array(after)) => {
            let Some(allowed): Option<HashSet<&str>> = before.iter().map(Json::as_str).collect()
            else {
                return false;
            };
            after
                .iter()
                .all(|value| value.as_str().is_some_and(|value| allowed.contains(value)))
        }
        (Json::Number(before), Json::Number(after)) => before
            .as_i64()
            .zip(after.as_i64())
            .is_some_and(|(before, after)| after <= before),
        _ => before == after,
    }
}

/// Whether changing an entry's options or end grants anything beyond what it currently grants.
pub(crate) fn entry_widens(before: &Entry, update: &CheckedUpdate) -> bool {
    update
        .options
        .as_ref()
        .is_some_and(|after| !options_only_narrow(&before.options, &after.to_json()))
        || update
            .expires_at
            .is_some_and(|after| match (before.expires_at, after) {
                (Some(before), Some(after)) => after > before,
                (Some(_), None) => true,
                _ => false,
            })
}

/// How a change of a page comes about, which decides who may make it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Acting {
    /// A person changes a page: naming holders is sharing, and everything else is managing the page.
    Ordinary,
    /// An approver carries out the decision of a request for access: they manage the page, and
    /// give what they hold.
    Decision,
    /// Part of a share with agents: entries in the rows of the areas shared, on the page of the
    /// thing and, for a place shared with what is in it, on its defaults page.
    Share,
}

/// What may be given to the agents of a place: the words of the form for them, as for a person.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentLevel {
    /// Can view: the agents read.
    View,
    /// Can edit: the agents also send to the threads, post in the chats and change the documents.
    Edit,
    /// Can manage: the agents also delete, move and bring back the documents. It adds nothing for
    /// conversations and chats: no agent removes a thread, a chat or a place.
    Manage,
}

/// What the reader of a page may give there, which is at most what they hold.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Gives {
    /// The everyday choices whose every row they may give.
    pub choices: Vec<Choice>,
    /// On a place, the roles up to their own; nothing elsewhere.
    pub roles: Vec<Role>,
    /// What they may share with agents.
    pub agents: Vec<AgentLevel>,
    /// Whether the place lets holders share, as the policy resolves there, so that a form can say
    /// why it offers nothing.
    pub holders_share: bool,
}

/// What the rule says of rows that are to be given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Judgement {
    /// They may be given. The rows to write: all of them, except that a share of a place with what
    /// is in it leaves out the form of a row, for a managed environment or for a person's machine,
    /// that its sharer holds nothing in while they hold the other form.
    Allowed {
        /// The rows to write.
        rows: Vec<(Permission, Profile)>,
    },
    /// The giver neither owns the thing nor manages its page, and the place keeps sharing to those
    /// who do (or the act is one that only they make).
    KeptToOwners,
    /// The giver holds less than the rows give: these permissions, in the order they were asked.
    Lacks(Vec<Permission>),
}

/// The giver on one page: who they are there, and what the place lets them give.
#[derive(Debug)]
pub struct Giving<'a, 'p> {
    frame: &'a Frame<'p>,
    site: Option<&'a Site>,
    page: PageKind,
    acting: Acting,
    manages: bool,
    owns: bool,
    administers: bool,
    admin_of_the_place: bool,
    /// For a place: the places whose defaults a thing in it follows, the place itself first.
    places: Vec<PlaceRef>,
    /// For a place: a thing made for the question, in each form of environment.
    followers: Option<[Site; 2]>,
}

/// A thing that lies directly in `place` and has nothing stored for its page, so that every one of
/// its rows follows the defaults of the place and of the places above it; it works in a managed
/// environment or on a person's machine, as `placement` says.
fn follower_in(place: &Site, placement: Placement) -> Site {
    let mut chain = vec![PlaceRef::Node(place.id)];
    chain.extend(place.chain.iter().copied());
    let mut admin_places = vec![place.id];
    admin_places.extend(place.admin_places.iter().copied());
    Site {
        kind: ObjectKind::Thread,
        id: orch_types::Id::nil(),
        name: place.name.clone(),
        first_owner: None,
        placement,
        chain,
        beside: None,
        role_place: Some(place.id),
        admin_places,
        orgs: place.orgs.clone(),
        root: place.root,
        path: place.path.clone(),
        archived_node: None,
        of_system: false,
    }
}

fn is_a_place(site: &Site) -> bool {
    site.kind.node().is_some_and(|node| node.place().is_some())
}

impl<'a, 'p> Giving<'a, 'p> {
    /// The giver on the page `page` of the thing `site` (for a defaults page, of the place `site`;
    /// none for the defaults of the system), under `acting`. The frame has read every permission
    /// of the page's kind, and the defaults of a place the page is of.
    #[must_use]
    pub fn of(
        frame: &'a Frame<'p>,
        site: Option<&'a Site>,
        page: PageKind,
        acting: Acting,
    ) -> Self {
        let principal = frame.principal();
        let manages = match site {
            Some(site) => !frame.ways(site, Permission::AccessManage).is_empty(),
            None => principal.is_system_admin(),
        };
        let (owns, administers) = site.map_or((false, principal.is_system_admin()), |site| {
            (frame.owns(site), frame.administers(site))
        });
        let admin_of_the_place = match site {
            Some(site) if is_a_place(site) => frame
                .role_at(site.id)
                .is_some_and(|held| held.role == Role::Admin),
            Some(_) => false,
            None => principal.is_system_admin(),
        };
        let place = site.filter(|site| is_a_place(site));
        let places = place.map_or_else(Vec::new, |site| {
            let mut places = vec![PlaceRef::Node(site.id)];
            places.extend(site.chain.iter().copied());
            places
        });
        let followers = place.map(|site| {
            [
                follower_in(site, Placement::Managed),
                follower_in(site, Placement::Connected),
            ]
        });
        Self {
            frame,
            site,
            page,
            acting,
            manages,
            owns,
            administers,
            admin_of_the_place,
            places,
            followers,
        }
    }

    /// The policy the question is asked under.
    #[must_use]
    pub fn policy(&self) -> Policy {
        self.frame.policy()
    }

    /// Whether the giver manages the page: they hold `access.manage` on it (an owner, an admin of
    /// its place, or whoever its page names), or on the defaults of the system are a system admin.
    #[must_use]
    pub fn manages(&self) -> bool {
        self.manages
    }

    /// Whether the giver is one of the owners of the thing.
    #[must_use]
    pub fn owns(&self) -> bool {
        self.owns
    }

    /// Whether the giver administers the thing: they are an admin of the place it lies in.
    #[must_use]
    pub fn administers(&self) -> bool {
        self.administers
    }

    /// Whether the place lets holders share, as the policy resolves there.
    #[must_use]
    pub fn holders_share(&self) -> bool {
        self.frame.policy().holders_share
    }

    /// Whether the giver may share here at all: managing the page is enough, and so is holding
    /// something where the place lets holders share. A decision on a request, and a change of a
    /// defaults page that is no part of a share, are managing the page and nothing less.
    #[must_use]
    pub fn may_share(&self) -> bool {
        match (self.acting, self.page) {
            (Acting::Decision, _) | (Acting::Ordinary, PageKind::Defaults) => self.manages,
            _ => self.manages || self.owns || self.holders_share(),
        }
    }

    /// Whether the giver holds `permission` in what is inside the place, in the form of
    /// environment `profile` says: on a thing that follows the row. An admin of the place holds
    /// any row of what its defaults give.
    fn holds_inside(&self, permission: Permission, profile: Profile) -> bool {
        if self.admin_of_the_place {
            return true;
        }
        let Some([managed, connected]) = &self.followers else {
            return false;
        };
        let follower = if profile == Profile::Connected {
            connected
        } else {
            managed
        };
        !self.frame.ways(follower, permission).is_empty()
    }

    /// Whether the giver holds `permission` where it would be given: on the page of the thing; for
    /// a defaults page, on a thing that follows the row.
    #[must_use]
    pub fn holds(&self, permission: Permission, profile: Profile) -> bool {
        match (self.page, self.site) {
            (PageKind::Live, Some(site)) => !self.frame.ways(site, permission).is_empty(),
            (PageKind::Live, None) => false,
            (PageKind::Defaults, None) => self.admin_of_the_place,
            (PageKind::Defaults, Some(_)) => self.holds_inside(permission, profile),
        }
    }

    /// Whether the row is decided by a lock set at a place above the page it is on, and so is
    /// changed where the lock is. `inside`: the row of the defaults of the place, which is where
    /// a share of the place with what is in it writes.
    fn locked_above(&self, permission: Permission, profile: Profile, inside: bool) -> bool {
        let defaults = inside || self.page == PageKind::Defaults;
        match (self.site, defaults) {
            (Some(_), true) => self
                .frame
                .defaults_chain(&self.places, profile, permission)
                .locked_above()
                .is_some(),
            (Some(site), false) => self.frame.chain(site, permission).locked_above().is_some(),
            (None, _) => false,
        }
    }

    /// Whether the permission may be given, apart from who the holder is: what the rows named by
    /// owners and admins take (`access.manage`, `access.external`) is theirs alone, and every other
    /// row takes holding it.
    fn within_bound(&self, permission: Permission, profile: Profile) -> bool {
        if !self.frame.principal().caller().permits(permission) {
            return false;
        }
        match permission.named_by() {
            NamedBy::Admin => self.administers,
            NamedBy::OwnerOrAdmin => self.owns || self.administers,
            NamedBy::WhoeverChangesThePage => self.holds(permission, profile),
        }
    }

    /// Whether the giver may name somebody in the row, as the page tells it beforehand: they may
    /// share here, hold the permission, and no lock above decides the row.
    #[must_use]
    pub fn can_give(&self, permission: Permission, profile: Profile) -> bool {
        permission != Permission::AccessOwn
            && self.may_share()
            && !self.locked_above(permission, profile, false)
            && self.within_bound(permission, profile)
    }

    /// The rule for rows to be given: whether the giver may share here, and holds every one of
    /// them. `rows` are the permissions with the form of the row on a defaults page. The rows that
    /// only owners and admins name (`access.manage`, `access.external`) are asked whether the
    /// giver is one.
    #[must_use]
    pub fn judge(&self, rows: &[(Permission, Profile)]) -> Judgement {
        if !self.may_share() {
            return Judgement::KeptToOwners;
        }
        let in_forms = self.acting == Acting::Share && self.page == PageKind::Defaults;
        let mut keep: Vec<(Permission, Profile)> = Vec::new();
        let mut lacks: Vec<Permission> = Vec::new();
        for (permission, profile) in rows {
            if self.within_bound(*permission, *profile) {
                keep.push((*permission, *profile));
                continue;
            }
            let other_form_held = in_forms
                && permission.profiles()
                && rows.iter().any(|(other, form)| {
                    other == permission && form != profile && self.within_bound(*other, *form)
                });
            if !other_form_held && !lacks.contains(permission) {
                lacks.push(*permission);
            }
        }
        if lacks.is_empty() {
            Judgement::Allowed { rows: keep }
        } else {
            Judgement::Lacks(lacks)
        }
    }

    /// The role the giver holds in the place, where the thing is one.
    fn role_held(&self) -> Option<Role> {
        self.site
            .filter(|site| is_a_place(site))
            .and_then(|site| self.frame.role_at(site.id))
            .map(|held| held.role)
    }

    fn role_names(&self, holder: &Holder, role: Role) -> bool {
        let Some(site) = self.site.filter(|site| is_a_place(site)) else {
            return false;
        };
        match holder {
            Holder::Role { role: named, at } if role >= *named => at.is_none_or(|place| {
                self.frame
                    .graph()
                    .get(place)
                    .is_some_and(|node| node.path.contains(&site.id))
            }),
            _ => false,
        }
    }

    // A role reaches the place's own page and the pages inside that follow
    // its defaults. Naming a higher level also gives its lower and advanced
    // rows, in either environment profile, so their names are scopes too.
    /// The actual rows this role reaches on the place and its followed defaults, also used by the share review.
    pub fn role_permissions(&self, role: Role) -> Vec<Permission> {
        let Some(site) = self.site.filter(|site| is_a_place(site)) else {
            return Vec::new();
        };
        let Some(kind) = site.kind.node().and_then(|node| node.place()) else {
            return Vec::new();
        };
        let mut given = Vec::new();
        for (permission, profile) in PageOf::Defaults(kind).rows() {
            let named = std::iter::once(permission)
                .chain(permission.held_through())
                .any(|level| {
                    let profiles: &[Profile] = match (level.profiles(), profile) {
                        (false, _) => &[Profile::Any],
                        (true, Profile::Any) => &[Profile::Managed, Profile::Connected],
                        (true, Profile::Managed) => &[Profile::Managed],
                        (true, Profile::Connected) => &[Profile::Connected],
                    };
                    profiles.iter().any(|form| {
                        self.frame
                            .defaults_chain(&self.places, *form, level)
                            .holders(self.frame.now())
                            .iter()
                            .any(|named| self.role_names(&named.entry.holder, role))
                    })
                });
            if named && !given.contains(&permission) {
                given.push(permission);
            }
        }
        for permission in site.kind.permissions() {
            let named = std::iter::once(permission)
                .chain(permission.held_through())
                .any(|level| {
                    self.frame
                        .chain(site, level)
                        .holders(self.frame.now())
                        .iter()
                        .any(|named| self.role_names(&named.entry.holder, role))
                });
            if named && !given.contains(&permission) {
                given.push(permission);
            }
        }
        // Administration grants page management even on private pages.
        if role == Role::Admin && !given.contains(&Permission::AccessManage) {
            given.push(Permission::AccessManage);
        }
        given
    }

    fn scope_ceiling(&self) -> Option<Role> {
        let caller = self.frame.principal().caller();
        if caller.grant().is_none() {
            return Some(Role::Admin);
        }
        Role::ALL
            .iter()
            .copied()
            .filter(|role| {
                self.role_permissions(*role)
                    .iter()
                    .all(|permission| caller.permits(*permission))
            })
            .max()
    }

    /// How the giver stands to giving roles in the place.
    #[must_use]
    pub fn role_standing(&self) -> RoleStanding {
        let reaches = self.site.is_some_and(|site| {
            self.frame
                .principal()
                .caller()
                .reaches(site.path.as_deref())
        });
        RoleStanding {
            manages: self.manages && reaches,
            may_share: self.may_share() && reaches,
            held: self.role_held().filter(|_| reaches),
            scope_ceiling: self.scope_ceiling(),
        }
    }

    /// Whether the giver may give the role `role` in the place: they hold that role there or a
    /// higher one, and may share here. Only an admin of the place gives Admin: whoever holds Admin
    /// there is one, the owners of the place among them.
    #[must_use]
    pub fn role(&self, role: Role) -> bool {
        self.role_standing().may_give(role)
    }

    /// The roles the giver may give in the place, lowest first.
    #[must_use]
    pub fn roles(&self) -> Vec<Role> {
        self.role_standing().roles()
    }

    /// The rows that sharing a thing of kind `kind` with agents at `level` names, in every area
    /// the thing has, with whether each is read through what the place gives to what is inside it:
    /// the conversations of a place are, the chat and the documents are on its own page. Writing
    /// includes reading, and managing includes writing; managing adds the documents' row alone.
    fn rows_of_agents(kind: ObjectKind, level: AgentLevel) -> Vec<(Permission, bool)> {
        use Permission as P;
        let place = kind.node().is_some_and(|node| node.place().is_some());
        let mut rows: Vec<(Permission, bool)> = match kind {
            ObjectKind::Document | ObjectKind::DocumentContainer => vec![(P::DocView, false)],
            ObjectKind::Thread => vec![
                (P::ThreadView, false),
                (P::ChatView, false),
                (P::DocView, false),
            ],
            _ if place => vec![
                (P::ThreadView, true),
                (P::ChatView, false),
                (P::DocView, false),
            ],
            _ => Vec::new(),
        };
        if matches!(level, AgentLevel::Edit | AgentLevel::Manage) {
            match kind {
                ObjectKind::Document | ObjectKind::DocumentContainer => {
                    rows.push((P::DocEdit, false))
                }
                ObjectKind::Thread => rows.extend([
                    (P::ThreadInteract, false),
                    (P::ChatInteract, false),
                    (P::DocEdit, false),
                ]),
                _ if place => rows.extend([
                    (P::ThreadInteract, true),
                    (P::ChatInteract, false),
                    (P::DocEdit, false),
                ]),
                _ => {}
            }
        }
        if level == AgentLevel::Manage && !rows.is_empty() {
            rows.push((P::DocManage, false));
        }
        rows
    }

    /// Whether the giver may share the thing with agents at `level`: in every area the thing has,
    /// reading for a share to read and writing for one to write, and where no lock above decides
    /// the row.
    #[must_use]
    pub fn agents(&self, level: AgentLevel) -> bool {
        let Some(site) = self.site else {
            return false;
        };
        if self.page != PageKind::Live || !self.may_share() {
            return false;
        }
        let rows = Self::rows_of_agents(site.kind, level);
        !rows.is_empty()
            && rows.iter().all(|(permission, inside)| {
                if *inside {
                    // The managed form is the one a thing follows unless it works on a person's
                    // machine, where nobody but its owner holds the right to drive it.
                    let profile = Profile::Managed;
                    self.holds_inside(*permission, profile)
                        && !self.locked_above(*permission, profile, true)
                } else {
                    self.within_bound(*permission, Profile::Any)
                        && !self.locked_above(*permission, Profile::Any, false)
                }
            })
    }

    /// Everything the page tells of what its reader may give.
    #[must_use]
    pub fn gives(&self) -> Gives {
        let holders_share = self.holders_share();
        let (Some(site), PageKind::Live) = (self.site, self.page) else {
            return Gives {
                holders_share,
                ..Gives::default()
            };
        };
        let choices = [Choice::FullView, Choice::FullInteract, Choice::FullManage]
            .into_iter()
            .filter(|choice| {
                let rows = choice.permissions(site.kind);
                !rows.is_empty()
                    && rows
                        .iter()
                        .all(|permission| self.can_give(*permission, Profile::Any))
            })
            .collect();
        let agents = [AgentLevel::View, AgentLevel::Edit, AgentLevel::Manage]
            .into_iter()
            .filter(|level| self.agents(*level))
            .collect();
        Gives {
            choices,
            roles: self.roles(),
            agents,
            holders_share,
        }
    }
}

/// How a person stands to giving roles in a place: what the page of the place lets them do, and the
/// role they hold there. It is the same rule as for a row of a page, asked about the role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoleStanding {
    /// They hold `access.manage` on the place: they are its owners, admins of the place above it, or
    /// named in the row.
    pub manages: bool,
    /// They may share here at all: they manage the page, or the place lets holders share.
    pub may_share: bool,
    /// The role they hold in the place, the owners of a place and the system's admins holding Admin.
    pub held: Option<Role>,
    // Role permissions nest as their holders do; the highest role whose
    // effective permission names all fit the token also bounds lower roles.
    scope_ceiling: Option<Role>,
}

impl RoleStanding {
    /// Whether they may give `role`: they may share here and hold that role or a higher one.
    #[must_use]
    pub fn may_give(&self, role: Role) -> bool {
        self.may_share
            && self.held.is_some_and(|held| held >= role)
            && self.scope_ceiling.is_some_and(|ceiling| ceiling >= role)
    }

    /// The roles they may give, lowest first.
    #[must_use]
    pub fn roles(&self) -> Vec<Role> {
        Role::ALL
            .iter()
            .copied()
            .filter(|role| self.may_give(*role))
            .collect()
    }

    /// Whether they hold Admin in the place: giving, changing or taking away Admin takes it.
    #[must_use]
    pub fn is_admin(&self) -> bool {
        self.held == Some(Role::Admin)
    }
}

async fn standing_in(
    conn: &mut Conn,
    principal: &Principal,
    place: Id,
) -> Result<(Graph, Site), Error> {
    let graph = Graph::load(conn, &[place]).await?;
    let site = graph.site_of(place).ok_or(Error::NotFound(place))?;
    if let Some(closing) = principal.suspension().closing(&site.orgs) {
        return Err(Error::Suspended { org: closing.org() });
    }
    Ok((graph, site))
}

/// How `principal` stands to giving roles in the place `place`. Somebody suspended where the place
/// is gets [`Error::Suspended`], as for any act there.
pub async fn role_standing(
    conn: &mut Conn,
    principal: &Principal,
    place: Id,
    policy: Policy,
) -> Result<RoleStanding, Error> {
    let (graph, site) = standing_in(conn, principal, place).await?;
    let frame = Frame::load_with(
        conn,
        principal,
        policy,
        graph,
        std::slice::from_ref(&site),
        &[PageRef::defaults_of(PlaceRef::Node(place))],
        Permission::ALL,
    )
    .await?;
    Ok(Giving::of(&frame, Some(&site), PageKind::Live, Acting::Ordinary).role_standing())
}

/// The refusal of giving, changing or taking away `role` in the place when the caller may not:
/// Admin takes being an admin of the place or one of its owners, and any other role the role
/// itself and being allowed to share here. It names what is lacking and who can give it.
pub async fn role_refusal(
    conn: &mut Conn,
    principal: &Principal,
    place: Id,
    role: Role,
    policy: Policy,
) -> Result<Refusal, Error> {
    let (graph, site) = standing_in(conn, principal, place).await?;
    let frame = Frame::load_with(
        conn,
        principal,
        policy,
        graph,
        std::slice::from_ref(&site),
        &[PageRef::defaults_of(PlaceRef::Node(place))],
        Permission::ALL,
    )
    .await?;
    let giving = Giving::of(&frame, Some(&site), PageKind::Live, Acting::Ordinary);
    let scopes_missing: Vec<Permission> = giving
        .role_permissions(role)
        .into_iter()
        .filter(|permission| !principal.caller().permits(*permission))
        .collect();
    if let Some(required) = scopes_missing.first().copied() {
        let mut refusal = Refusal::of(
            required,
            ObjectRef {
                kind: site.kind,
                id: site.id,
            },
            vec![MissingAccess {
                object: MissingObject {
                    kind: site.kind,
                    id: site.id,
                    name: frame.holds_any(&site).then(|| site.name.clone()),
                    owner: frame.holds_any(&site).then_some(site.first_owner).flatten(),
                },
                permissions: scopes_missing,
                options: None,
                ask: Vec::new(),
                ask_more: 0,
                r#override: false,
                closed: None,
                locked_by: None,
            }],
        );
        // No person can widen the credential's scopes by giving an entry.
        refusal.requestable = false;
        return Ok(refusal);
    }
    let required = if role == Role::Admin {
        Permission::AccessOwn
    } else {
        Permission::AccessManage
    };
    let lacking = [required];
    let missing = missing::missing_access(conn, &frame, &site, &lacking, true).await?;
    Ok(Refusal::of(
        required,
        ObjectRef {
            kind: site.kind,
            id: site.id,
        },
        missing,
    ))
}
