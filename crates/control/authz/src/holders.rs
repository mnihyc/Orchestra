//! Holders: a role held in a place, a group, a user, the work in a place, the agents of a place, and everyone.
//!
//! A row of a page names holders, and a holder is matched to a person: a
//! user is that person; a group is the people in it; a role held in a place
//! is everyone whose role there is that one or a higher one; everyone is
//! every user. The work in a place and the agents of a place name no
//! person. [`Principal`] is the person asked about, with everything that
//! matching needs of them, read once; the people behind a holder, which a
//! page shows only as a name, are found by [`people_of`].

mod kept;

use crate::Error;
use crate::engine::{Frame, Graph, Site};
use crate::orgs::Memberships;
use crate::suspension::Suspension;
use crate::tokens::Caller;
use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::Permission;
use orch_access_contract::roles::{Credential, Person, Role, RoleHeld, Subject, SubjectKind};
use orch_db::Conn;
use orch_db::queries::access;
use orch_types::{Id, Time};

/// A caller together with what is known of them: the groups they are in,
/// the trees of orgs they are members of, where they are suspended, and
/// whether they are a system admin. It is read once for a request and
/// holds nothing else; what they hold on anything is asked of [`Frame`].
///
/// [`Frame`]: crate::engine::Frame
#[derive(Debug, Clone)]
pub struct Principal {
    caller: Caller,
    now: Time,
    suspension: Suspension,
    flag: bool,
    groups: Vec<Id>,
    viewers: Vec<Id>,
    memberships: Memberships,
    member_of: Vec<Id>,
    second_step: Option<crate::suspension::SecondStepHold>,
}

impl Principal {
    /// This person's ordinary pages, preserving second-step holds but never a session's
    /// native administrator reach. Used to filter focus before a list is paged.
    pub async fn ordinary(&self, conn: &mut Conn) -> Result<Self, Error> {
        if !self.caller.as_admin() {
            return Ok(self.clone());
        }
        let ordinary =
            Self::load(conn, self.caller.clone().with_admin_mode(false), self.now).await?;
        Ok(match &self.second_step {
            Some(hold) => ordinary.held_for_second_step(hold.clone()),
            None => ordinary,
        })
    }
    /// The same caller, held out of the root orgs of `hold` until their sign-in has the second step
    /// those orgs require: what lies there is closed to them as under a suspension, and nothing on
    /// any page changes. Whoever reads the principal's refusals can tell the hold from a suspension.
    #[must_use]
    pub fn held_for_second_step(mut self, hold: crate::suspension::SecondStepHold) -> Self {
        self.suspension = self.suspension.with_held(hold.orgs.clone());
        self.second_step = Some(hold);
        self
    }

    /// Where, and why, the caller is held out until a second step, if they are.
    #[must_use]
    pub fn second_step_hold(&self) -> Option<&crate::suspension::SecondStepHold> {
        self.second_step.as_ref()
    }

    /// Reads what is known of the caller, as of `now`: two statements,
    /// whatever the caller is.
    pub async fn load(conn: &mut Conn, caller: Caller, now: Time) -> Result<Self, Error> {
        let user = caller.user();
        // A stored token is kept out, besides its subtree, of the orgs that refused it and of the places
        // whose longest admitted life it exceeds: read now, so that a refusal holds from the next request.
        let stored_token = caller.grant().and_then(crate::TokenGrant::token);
        let caller = match stored_token {
            Some(token) => {
                let places = orch_db::queries::tokens::blocked(conn, token)
                    .await?
                    .into_iter()
                    .filter_map(|at| match (at.refused, at.org, at.max_days) {
                        (true, Some(org), _) => Some(crate::KeptOut::Refused { org }),
                        (false, place, Some(max_days)) => {
                            Some(crate::KeptOut::Lifetime { place, max_days })
                        }
                        _ => None,
                    })
                    .collect();
                caller.with_kept_out(places)
            }
            None => caller,
        };
        let facts = access::principal(conn, user)
            .await?
            .ok_or(Error::NoSuchUser(user))?;
        let memberships = Memberships::load(conn, user).await?;
        // Kept between requests while nothing they are worked out from has changed (`kept`).
        let sets = kept::holder_sets(
            conn,
            user,
            now,
            facts.is_system_admin && caller.carries_system_admin(),
        )
        .await?;
        let (groups, viewers) = (sets.groups.clone(), sets.viewers.clone());
        let member_of = memberships.member_of();
        Ok(Self {
            caller,
            now,
            suspension: Suspension::new(facts.status == "suspended", facts.suspended_in),
            flag: facts.is_system_admin,
            groups,
            viewers,
            memberships,
            member_of,
            second_step: None,
        })
    }

    /// The caller.
    #[must_use]
    pub fn caller(&self) -> &Caller {
        &self.caller
    }

    /// The user.
    #[must_use]
    pub fn user(&self) -> Id {
        self.caller.user()
    }

    /// What the request is made with.
    #[must_use]
    pub fn credential(&self) -> Credential {
        self.caller.credential()
    }

    /// The moment the questions are asked at.
    #[must_use]
    pub fn now(&self) -> Time {
        self.now
    }

    /// Where they are suspended.
    #[must_use]
    pub fn suspension(&self) -> &Suspension {
        &self.suspension
    }

    /// The groups they are in, sorted.
    #[must_use]
    pub fn groups(&self) -> &[Id] {
        &self.groups
    }

    /// Page-only viewing counts for Everyone here without giving an inherited role.
    #[must_use]
    pub fn is_viewer_of(&self, place: Id) -> bool {
        self.viewers.contains(&place)
    }

    /// Their membership of trees of orgs.
    #[must_use]
    pub fn memberships(&self) -> &Memberships {
        &self.memberships
    }

    /// Every org they are a member of, sorted.
    #[must_use]
    pub fn member_of(&self) -> &[Id] {
        &self.member_of
    }

    /// Whether they are a member of the tree of the root org `root`.
    #[must_use]
    pub fn is_in_tree(&self, root: Id) -> bool {
        self.memberships.is_in_tree(root)
    }

    /// Whether they hold Admin at the system, and it counts: they are a
    /// system admin and the request is made in a browser session.
    #[must_use]
    pub fn is_system_admin(&self) -> bool {
        self.flag && self.caller.carries_system_admin()
    }

    /// The person, for matching the holders of a row.
    #[must_use]
    pub fn person(&self) -> Person<'_> {
        Person {
            id: self.user(),
            groups: &self.groups,
            member_of: &self.member_of,
        }
    }

    /// Whether `holder` is, or contains, this person; `role_at` says which
    /// role they have at the place a role holder names (or, given `None`,
    /// where the thing is).
    pub fn is_held_by(
        &self,
        holder: &Holder,
        role_at: impl FnOnce(Option<Id>) -> Option<Role>,
    ) -> bool {
        if let Holder::Role {
            role: Role::Viewer,
            at: Some(place),
        } = holder
        {
            return self.is_viewer_of(*place);
        }
        holder.matches(&self.person(), role_at)
    }
}

/// Every identity the person `user` holds at `now`, their flag of system admin not counted, as `access_holders`
/// finds them: kept between requests as a principal's groups are (`kept`), for a statement that needs them all.
pub async fn held(
    conn: &mut Conn,
    user: Id,
    now: Time,
) -> Result<std::sync::Arc<access::kept::HolderSets>, Error> {
    kept::holder_sets(conn, user, now, false).await
}

/// The most people looked at behind any one source: more than a refusal
/// could usefully count. Beyond it a count is a lower bound.
pub const SCAN_MOST: i64 = 500;

/// The people the Owners row of `object` names, groups opened, who are
/// members of the tree of `root`: the owners whose owning counts.
pub async fn owner_people(
    conn: &mut Conn,
    frame: &Frame<'_>,
    object: Id,
    root: Option<Id>,
) -> Result<Vec<Id>, Error> {
    let Some(root) = root else {
        return Ok(Vec::new());
    };
    let now = frame.now();
    let mut users: Vec<Id> = Vec::new();
    let mut groups: Vec<Id> = Vec::new();
    for entry in frame
        .owner_entries(object)
        .iter()
        .filter(|entry| entry.counts(now))
    {
        match entry.holder {
            Holder::User { id } => users.push(id),
            Holder::Group { id } => groups.push(id),
            _ => {}
        }
    }
    if !groups.is_empty() {
        // Ownership is a complete set, even when a group's visible roster pages.
        // A page-size limit must not remove an owner's rights or notices.
        for (_, user) in access::group_people(conn, &groups, i64::MAX, now.get()).await? {
            users.push(user);
        }
    }
    users.sort_unstable();
    users.dedup();
    let members = access::members_among(conn, root, &users).await?;
    users.retain(|user| members.contains(user));
    Ok(users)
}

/// The people who hold Admin at each of the places `places` by being given
/// it or as owners of it, not those who hold it from above, who are further
/// away: each with the index in `places` of the place they hold it at. The
/// places lie in one tree. Three statements, however many places.
pub async fn admins_at(
    conn: &mut Conn,
    frame: &Frame<'_>,
    places: &[Id],
) -> Result<Vec<(usize, Id)>, Error> {
    if places.is_empty() {
        return Ok(Vec::new());
    }
    let mut found: Vec<(usize, Id)> = Vec::new();
    for (place, user) in access::people_given_role(
        conn,
        places,
        &[Role::Admin.as_str().to_owned()],
        frame.now().get(),
        SCAN_MOST,
    )
    .await?
    {
        if let Some(at) = places.iter().position(|candidate| *candidate == place) {
            found.push((at, user));
        }
    }
    let now = frame.now();
    let mut owners: Vec<(usize, Id)> = Vec::new();
    let mut groups: Vec<(usize, Id)> = Vec::new();
    for (at, place) in places.iter().enumerate() {
        for entry in frame
            .owner_entries(*place)
            .iter()
            .filter(|entry| entry.counts(now))
        {
            match entry.holder {
                Holder::User { id } => owners.push((at, id)),
                Holder::Group { id } => groups.push((at, id)),
                _ => {}
            }
        }
    }
    if !groups.is_empty() {
        let ids: Vec<Id> = groups.iter().map(|(_, group)| *group).collect();
        for (group, user) in access::group_people(conn, &ids, SCAN_MOST, now.get()).await? {
            for (at, _) in groups
                .iter()
                .filter(|(_, owner_group)| *owner_group == group)
            {
                owners.push((*at, user));
            }
        }
    }
    if let Some(root) = places
        .first()
        .and_then(|place| frame.graph().get(*place))
        .map(|node| node.root)
    {
        let users: Vec<Id> = owners.iter().map(|(_, user)| *user).collect();
        let members = access::members_among(conn, root, &users).await?;
        found.extend(
            owners
                .into_iter()
                .filter(|(_, user)| members.contains(user)),
        );
    }
    found.sort_unstable();
    found.dedup();
    Ok(found)
}

/// The people behind one holder of a row of the site: a person is that
/// person; a group is its members; a role held in a place is everyone given
/// that role or a higher one there or above, the owners of those places and
/// the system's admins, who hold Admin everywhere. Everyone, the work in a
/// place and the agents of a place are no people that can be listed.
pub async fn people_of(
    conn: &mut Conn,
    frame: &Frame<'_>,
    site: &Site,
    holder: &Holder,
) -> Result<Vec<Id>, Error> {
    match *holder {
        Holder::User { id } => Ok(vec![id]),
        Holder::Group { id } => Ok(
            access::group_people(conn, &[id], SCAN_MOST, frame.now().get())
                .await?
                .into_iter()
                .map(|(_, user)| user)
                .collect(),
        ),
        Holder::Members { id } => Ok(access::org_member_people(conn, id, SCAN_MOST).await?),
        Holder::Role { role, at } => {
            let Some(place) = at.or(site.role_place) else {
                return Ok(Vec::new());
            };
            let Some(node) = frame.graph().get(place) else {
                return Ok(Vec::new());
            };
            if node.deleted
                || node
                    .path
                    .iter()
                    .any(|id| frame.graph().get(*id).is_some_and(|node| node.deleted))
            {
                return Ok(Vec::new());
            }
            let on_the_way: Vec<(Id, Id)> = frame
                .graph()
                .places_on(node)
                .into_iter()
                .map(|place| (place.id, place.root))
                .collect();
            let places = vec![place];
            let roles: Vec<String> = Role::ALL
                .iter()
                .filter(|held| held.includes(role))
                .map(|held| held.as_str().to_owned())
                .collect();
            let mut people: Vec<Id> = access::people_in_roles(
                conn,
                &places,
                &roles,
                frame.now().get(),
                SCAN_MOST,
                at.is_some(),
            )
            .await?
            .into_iter()
            .map(|(_, user)| user)
            .collect();
            for (place, root) in on_the_way {
                if role != Role::Viewer || at.is_none() {
                    people.extend(owner_people(conn, frame, place, Some(root)).await?);
                }
            }
            if role != Role::Viewer || at.is_none() {
                people.extend(access::system_admin_people(conn, SCAN_MOST).await?);
            }
            Ok(people)
        }
        Holder::Everyone | Holder::Node { .. } | Holder::Agents { .. } => Ok(Vec::new()),
    }
}

/// A concrete current path from a holder to this person, for the page's Reaches explanation.
/// Group paths are at most five deep; repeated identities are skipped, so role links cannot cycle.
pub async fn chain(
    conn: &mut Conn,
    principal: &Principal,
    holder: Holder,
    default_place: Option<Id>,
) -> Result<Vec<Holder>, Error> {
    let implicit_viewer = matches!(
        holder,
        Holder::Role {
            role: Role::Viewer,
            at: None
        }
    );
    let first = match holder {
        Holder::Role { role, at: None } => Holder::Role {
            role,
            at: default_place,
        },
        other => other,
    };
    let mut pending = std::collections::VecDeque::from([vec![first]]);
    while let Some(path) = pending.pop_front() {
        let Some(last) = path.last().copied() else {
            continue;
        };
        let mut next = Vec::new();
        match last {
            Holder::User { id } if id == principal.user() => return Ok(path),
            Holder::Members { id } if principal.member_of().contains(&id) => {
                next.push(Holder::User {
                    id: principal.user(),
                })
            }
            Holder::Group { id } if principal.groups().contains(&id) => {
                for member in access::group_holders(conn, id).await? {
                    if let Some(id) = member.user_id {
                        next.push(Holder::User { id });
                    } else if let Some(id) = member.member_group_id {
                        next.push(Holder::Group { id });
                    } else if let Some(id) = member.member_org_id {
                        next.push(Holder::Members { id });
                    } else if let (Some(id), Some(role)) =
                        (member.member_node_id, member.member_role)
                    {
                        next.push(Holder::Role {
                            role: role.parse()?,
                            at: Some(id),
                        });
                    }
                }
            }
            Holder::Role {
                role,
                at: Some(place),
            } => {
                if role == Role::Viewer && !implicit_viewer && !principal.is_viewer_of(place) {
                    continue;
                }
                let graph = Graph::load(conn, &[place]).await?;
                let Some(site) = graph.site_of(place) else {
                    continue;
                };
                let wanted: Vec<Permission> = site
                    .kind
                    .node()
                    .and_then(|kind| {
                        orch_access_contract::permissions::NodePermission::View.on(kind)
                    })
                    .map(|view| std::iter::once(view).chain(view.held_through()).collect())
                    .unwrap_or_default();
                let frame = Frame::load(
                    conn,
                    principal,
                    crate::Policy::default(),
                    graph,
                    std::slice::from_ref(&site),
                    &wanted,
                )
                .await?;
                if role == Role::Viewer && principal.is_viewer_of(place) {
                    for permission in &wanted {
                        for named in frame.matching(&site, *permission, None) {
                            next.push(match named.entry.holder {
                                Holder::Role { role, at: None } => Holder::Role {
                                    role,
                                    at: Some(place),
                                },
                                other => other,
                            });
                        }
                    }
                }
                if let Some(held) = frame.role_at(place).filter(|held| held.role.includes(role)) {
                    if let orch_access_contract::roles::PlaceRef::Node(from) = held.at {
                        if from != place {
                            next.push(Holder::Role {
                                role: held.role,
                                at: Some(from),
                            });
                        } else if frame.owns(&site) {
                            next.extend(
                                frame
                                    .owner_entries(site.id)
                                    .iter()
                                    .filter(|entry| {
                                        entry.counts(principal.now())
                                            && principal.is_held_by(&entry.holder, |_| None)
                                    })
                                    .map(|entry| entry.holder),
                            );
                        } else {
                            let rows = access::roles_reaching(
                                conn,
                                &[from],
                                principal.user(),
                                principal.groups(),
                                principal.member_of(),
                                principal.now().get(),
                            )
                            .await?;
                            for row in rows.into_iter().filter(|row| {
                                row.role
                                    .parse::<Role>()
                                    .is_ok_and(|given| given.includes(role))
                            }) {
                                next.push(match row.subject_kind.as_str() {
                                    "user" => Holder::User { id: row.subject_id },
                                    "group" => Holder::Group { id: row.subject_id },
                                    "members" => Holder::Members { id: row.subject_id },
                                    "role" => match row.subject_role.as_deref() {
                                        Some(role) => Holder::Role {
                                            at: Some(row.subject_id),
                                            role: role.parse()?,
                                        },
                                        None => continue,
                                    },
                                    _ => continue,
                                });
                            }
                        }
                    } else {
                        next.push(Holder::User {
                            id: principal.user(),
                        });
                    }
                }
            }
            Holder::Everyone => next.push(Holder::User {
                id: principal.user(),
            }),
            _ => {}
        }
        for holder in next {
            if !path.contains(&holder) {
                let mut expanded = path.clone();
                expanded.push(holder);
                pending.push_back(expanded);
            }
        }
    }
    Ok(Vec::new())
}

/// The roles given in a place and in the places above it, as the place's
/// page lists them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RolesHere {
    /// The roles given in the place itself.
    pub own: Vec<RoleHeld>,
    /// The roles given in the places above it, the nearest place first.
    pub above: Vec<RoleHeld>,
}

fn role_held(row: access::RoleRow) -> Result<RoleHeld, Error> {
    Ok(RoleHeld {
        id: row.id,
        node: row.node_id,
        subject: Subject {
            kind: row.subject_kind.parse::<SubjectKind>()?,
            id: row.subject_id,
            role: row.subject_role.as_deref().map(str::parse).transpose()?,
        },
        role: row.role.parse()?,
        expires_at: row.expires_at.map(Time::new),
        note: row.note,
        created_by: row.created_by,
        created_at: Time::new(row.created_at),
        removed_at: row.removed_at.map(Time::new),
        revision: row.revision,
    })
}

/// Who holds which role in the node `node` and in the places above it, as of
/// `now`: what ended or was taken away is left out. A node that is no place
/// has no roles of its own, and its people are named on its page.
pub async fn roles_here(conn: &mut Conn, node: Id, now: Time) -> Result<RolesHere, Error> {
    let graph = Graph::load(conn, &[node]).await?;
    let facts = graph.get(node).ok_or(Error::NotFound(node))?.clone();
    let places: Vec<Id> = graph
        .places_on(&facts)
        .into_iter()
        .map(|place| place.id)
        .collect();
    let mut own = Vec::new();
    let mut above = Vec::new();
    for row in access::roles_in_places(conn, &places, now.get()).await? {
        if row.node_id == node {
            own.push(role_held(row)?);
        } else {
            above.push((row.node_id, role_held(row)?));
        }
    }
    // The nearest place first: the order of the way up, reversed.
    above.sort_by_key(|(place, _)| {
        std::cmp::Reverse(places.iter().position(|candidate| candidate == place))
    });
    Ok(RolesHere {
        own,
        above: above.into_iter().map(|(_, held)| held).collect(),
    })
}

/// Everyone who holds `permission` on the site: its owners, the admins of
/// its place where the permission is `access.manage`, and whom its rows name
/// for it or for a level above it, the people behind each role and group
/// opened. Only people who are not suspended where the site is are listed;
/// nobody is listed twice; the order is fixed. The frame has read the rows of
/// the permission and of the levels above it.
pub async fn who_holds(
    conn: &mut Conn,
    frame: &Frame<'_>,
    site: &Site,
    permission: Permission,
) -> Result<Vec<Id>, Error> {
    let mut people: Vec<Id> = Vec::new();
    if permission != Permission::AccessExternal || frame.policy().owners_name_outsiders {
        people.extend(owner_people(conn, frame, site.id, site.root).await?);
    }
    if permission == Permission::AccessManage {
        people.extend(
            admins_at(conn, frame, &site.admin_places)
                .await?
                .into_iter()
                .map(|(_, user)| user),
        );
        people.extend(access::system_admin_people(conn, SCAN_MOST).await?);
    }
    if permission != Permission::AccessOwn {
        for row in std::iter::once(permission)
            .chain(permission.held_through())
            .filter(|row| *row != Permission::AccessOwn)
        {
            let named: Vec<Holder> = frame
                .chain(site, row)
                .holders(frame.now())
                .into_iter()
                .map(|named| named.entry.holder)
                .collect();
            for holder in named {
                people.extend(people_of(conn, frame, site, &holder).await?);
            }
        }
    }
    people.sort_unstable();
    people.dedup();
    let active = access::active_among(conn, &people, &site.orgs).await?;
    people.retain(|user| active.contains(user));
    Ok(people)
}

/// All human recipients behind a source, in stable bounded pages. The refusal-preview bound
/// is a page size here, so a large group loses no recipients.
async fn notification_people(
    conn: &mut Conn,
    source: access::NotificationPeopleSource<'_>,
    now: Time,
) -> Result<Vec<Id>, Error> {
    let mut people = Vec::new();
    let mut after = None;
    loop {
        let page =
            access::notification_people_page(conn, &source, after, SCAN_MOST, now.get()).await?;
        let more = i64::try_from(page.len()).is_ok_and(|count| count == SCAN_MOST);
        after = page.last().copied();
        people.extend(page);
        if !more {
            return Ok(people);
        }
    }
}

/// All active human system admins for an event, without truncating at a refusal-preview page.
pub async fn notification_admins(conn: &mut Conn, now: Time) -> Result<Vec<Id>, Error> {
    notification_people(
        conn,
        access::NotificationPeopleSource {
            system_admins: true,
            ..Default::default()
        },
        now,
    )
    .await
}

async fn notification_owners(
    conn: &mut Conn,
    frame: &Frame<'_>,
    object: Id,
    root: Option<Id>,
) -> Result<Vec<Id>, Error> {
    let Some(root) = root else {
        return Ok(Vec::new());
    };
    let mut people = std::collections::BTreeSet::new();
    for entry in frame
        .owner_entries(object)
        .iter()
        .filter(|entry| entry.counts(frame.now()))
    {
        let source = match entry.holder {
            Holder::User { id } => access::NotificationPeopleSource {
                user: Some(id),
                ..Default::default()
            },
            Holder::Group { id } => access::NotificationPeopleSource {
                group: Some(id),
                ..Default::default()
            },
            _ => continue,
        };
        people.extend(notification_people(conn, source, frame.now()).await?);
    }
    Ok(access::members_among(conn, root, &people.into_iter().collect::<Vec<_>>()).await?)
}

/// Every current human behind a holder for a deliberate notification, without a roster page cap.
pub async fn notification_holder(
    conn: &mut Conn,
    frame: &Frame<'_>,
    site: &Site,
    holder: &Holder,
) -> Result<Vec<Id>, Error> {
    notification_holder_with(conn, frame, site, holder, &mut OwnersFound::default()).await
}

/// The owners of each place, found once for all the holders asked of one frame: the roles
/// of one place reach the Owners rows of the same places above it, and each is the same people.
#[derive(Default)]
pub struct OwnersFound(std::collections::HashMap<Id, Vec<Id>>);

impl OwnersFound {
    async fn of(
        &mut self,
        conn: &mut Conn,
        frame: &Frame<'_>,
        place: Id,
        root: Id,
    ) -> Result<&[Id], Error> {
        let found = match self.0.entry(place) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(notification_owners(conn, frame, place, Some(root)).await?)
            }
        };
        Ok(found.as_slice())
    }
}

/// [`notification_holder`] for one of several holders asked of the same `frame`, sharing `owners`.
pub async fn notification_holder_with(
    conn: &mut Conn,
    frame: &Frame<'_>,
    site: &Site,
    holder: &Holder,
    owners: &mut OwnersFound,
) -> Result<Vec<Id>, Error> {
    let source = match *holder {
        Holder::User { id } => access::NotificationPeopleSource {
            user: Some(id),
            ..Default::default()
        },
        Holder::Group { id } => access::NotificationPeopleSource {
            group: Some(id),
            ..Default::default()
        },
        Holder::Members { id } => access::NotificationPeopleSource {
            members_org: Some(id),
            ..Default::default()
        },
        Holder::Everyone => access::NotificationPeopleSource {
            everyone_root: site.root,
            ..Default::default()
        },
        Holder::Role { role, at } => {
            let Some(node) = at
                .or(site.role_place)
                .and_then(|place| frame.graph().get(place))
            else {
                return Ok(Vec::new());
            };
            let places = frame.graph().places_on(node);
            if node.deleted
                || node
                    .path
                    .iter()
                    .any(|id| frame.graph().get(*id).is_some_and(|node| node.deleted))
            {
                return Ok(Vec::new());
            }
            let ids = vec![node.id];
            let roles: Vec<String> = Role::ALL
                .iter()
                .filter(|held| held.includes(role))
                .map(|held| held.as_str().to_owned())
                .collect();
            let mut people = notification_people(
                conn,
                access::NotificationPeopleSource {
                    role_places: &ids,
                    roles: &roles,
                    include_viewers: at.is_some(),
                    system_admins: role != Role::Viewer || at.is_none(),
                    ..Default::default()
                },
                frame.now(),
            )
            .await?;
            for place in places {
                if role != Role::Viewer || at.is_none() {
                    people.extend(owners.of(conn, frame, place.id, place.root).await?);
                }
            }
            return Ok(people);
        }
        Holder::Node { .. } | Holder::Agents { .. } => return Ok(Vec::new()),
    };
    notification_people(conn, source, frame.now()).await
}

/// Every active human holder for notification delivery, without a preview's source cap. Everyone
/// expands to active members of the site's tree; role and group sources are paged to their end.
pub async fn who_holds_for_notifications(
    conn: &mut Conn,
    frame: &Frame<'_>,
    site: &Site,
    permission: Permission,
) -> Result<Vec<Id>, Error> {
    let mut people = std::collections::BTreeSet::new();
    // The people are a union: a holder named by several rows, and the Owners rows that several roles
    // reach, are expanded once.
    let mut owners = OwnersFound::default();
    let mut asked: Vec<Holder> = Vec::new();
    if permission != Permission::AccessExternal || frame.policy().owners_name_outsiders {
        people.extend(notification_owners(conn, frame, site.id, site.root).await?);
    }
    if permission == Permission::AccessManage {
        for place in &site.admin_places {
            let holder = Holder::Role {
                role: Role::Admin,
                at: Some(*place),
            };
            if asked.contains(&holder) {
                continue;
            }
            asked.push(holder);
            people.extend(notification_holder_with(conn, frame, site, &holder, &mut owners).await?);
        }
        people.extend(
            notification_people(
                conn,
                access::NotificationPeopleSource {
                    system_admins: true,
                    ..Default::default()
                },
                frame.now(),
            )
            .await?,
        );
    }
    if permission != Permission::AccessOwn {
        for row in std::iter::once(permission)
            .chain(permission.held_through())
            .filter(|row| *row != Permission::AccessOwn)
        {
            let holders: Vec<Holder> = frame
                .chain(site, row)
                .holders(frame.now())
                .into_iter()
                .map(|named| named.entry.holder)
                .collect();
            for holder in holders {
                if asked.contains(&holder) {
                    continue;
                }
                asked.push(holder);
                people.extend(
                    notification_holder_with(conn, frame, site, &holder, &mut owners).await?,
                );
            }
        }
    }
    let mut active =
        access::active_among(conn, &people.into_iter().collect::<Vec<_>>(), &site.orgs).await?;
    active.sort_unstable();
    Ok(active)
}

/// The sources behind a document-reader page, retaining holder definitions rather than every person behind them.
/// Role source unions are candidates only: the caller rechecks ordinary document View before exposing a name.
#[must_use]
pub fn notification_sources(
    frame: &Frame<'_>,
    site: &Site,
    permissions: &[Permission],
) -> orch_db::queries::identity::MentionSources {
    use std::collections::BTreeSet;
    let mut sources = orch_db::queries::identity::MentionSources::default();
    let mut named = BTreeSet::new();
    let mut implicit_places = BTreeSet::new();
    let mut implicit_roles = BTreeSet::new();
    let mut explicit_places = BTreeSet::new();
    let mut explicit_roles = BTreeSet::new();
    let mut members = BTreeSet::new();
    let mut owners = |object, root| {
        if let Some(root) = root {
            for entry in frame
                .owner_entries(object)
                .iter()
                .filter(|entry| entry.counts(frame.now()))
            {
                match entry.holder {
                    Holder::User { id } => {
                        named.insert(("user".to_owned(), Some(id), String::new(), Some(root)));
                    }
                    Holder::Group { id } => {
                        named.insert(("group".to_owned(), Some(id), String::new(), Some(root)));
                    }
                    _ => {}
                }
            }
        }
    };
    owners(site.id, site.root);
    let mut holders = Vec::new();
    for permission in permissions {
        for row in std::iter::once(*permission)
            .chain(permission.held_through())
            .filter(|row| *row != Permission::AccessOwn)
        {
            holders.extend(
                frame
                    .chain(site, row)
                    .holders(frame.now())
                    .into_iter()
                    .map(|named| named.entry.holder),
            );
        }
    }
    for holder in &holders {
        if let Holder::Role { role, at } = holder
            && let Some(node) = at
                .or(site.role_place)
                .and_then(|place| frame.graph().get(place))
            && (*role != Role::Viewer || at.is_none())
        {
            for place in frame.graph().places_on(node) {
                owners(place.id, Some(place.root));
            }
        }
    }
    for holder in holders {
        match holder {
            Holder::User { id } => {
                named.insert(("user".to_owned(), Some(id), String::new(), None));
            }
            Holder::Group { id } => {
                named.insert(("group".to_owned(), Some(id), String::new(), None));
            }
            Holder::Members { id } => {
                named.insert(("members".to_owned(), Some(id), String::new(), None));
            }
            Holder::Everyone => {
                if let Some(root) = site.root {
                    members.insert(root);
                }
            }
            Holder::Role { role, at } => {
                let Some(node) = at
                    .or(site.role_place)
                    .and_then(|place| frame.graph().get(place))
                else {
                    continue;
                };
                if node.deleted
                    || node
                        .path
                        .iter()
                        .any(|id| frame.graph().get(*id).is_some_and(|node| node.deleted))
                {
                    continue;
                }
                let (places, roles) = if at.is_some() {
                    (&mut explicit_places, &mut explicit_roles)
                } else {
                    (&mut implicit_places, &mut implicit_roles)
                };
                places.insert(node.id);
                for held in Role::ALL.iter().filter(|held| held.includes(role)) {
                    roles.insert(held.as_str().to_owned());
                }
                sources.system_admins |= role != Role::Viewer || at.is_none();
            }
            Holder::Node { .. } | Holder::Agents { .. } => {}
        }
    }
    for (kind, id, role, root) in named {
        sources.kinds.push(kind);
        sources.ids.push(id);
        sources.minimum_roles.push(role);
        sources.owner_roots.push(root);
    }
    sources.implicit_places = implicit_places.into_iter().collect();
    sources.implicit_roles = implicit_roles.into_iter().collect();
    sources.explicit_places = explicit_places.into_iter().collect();
    sources.explicit_roles = explicit_roles.into_iter().collect();
    sources.member_roots = members.into_iter().collect();
    sources
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_access_contract::pages::Holder;
    use orch_db::queries::access::testing::*;
    use orch_db::queries::hierarchy::testing::insert_node;
    use orch_db::queries::identity::testing::{insert_user, suspend};
    use orch_testkit::{clock, database::TestDb};
    use orch_types::new_id;

    #[tokio::test]
    async fn a_person_is_read_with_their_groups_trees_and_suspensions() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (ada, bob) = (new_id(), new_id());
        insert_user(&mut conn, ada, "ada").await.unwrap();
        insert_user(&mut conn, bob, "bob").await.unwrap();
        let (root, research) = (new_id(), new_id());
        insert_node(&mut conn, root, "org", None, "Acme", ada)
            .await
            .unwrap();
        insert_node(&mut conn, research, "org", Some(root), "Research", ada)
            .await
            .unwrap();
        insert_member(&mut conn, root, bob, research).await.unwrap();
        let group = insert_group(&mut conn, root, "Ops", ada).await.unwrap();
        add_to_group(&mut conn, group, bob).await.unwrap();
        suspend_in(&mut conn, research, bob, ada).await.unwrap();
        make_system_admin(&mut conn, bob).await.unwrap();
        let now = Time::new(clock::start());

        // A system admin's session carries it with As admin on.
        let session = Principal::load(&mut conn, Caller::session(bob).with_admin_mode(true), now)
            .await
            .unwrap();
        assert_eq!(session.groups(), [group]);
        assert_eq!(session.member_of().len(), 2);
        assert!(session.is_in_tree(root));
        assert_eq!(session.suspension().in_orgs(), [research]);
        assert!(session.is_system_admin());
        let token = Principal::load(
            &mut conn,
            Caller::token(bob, crate::TokenGrant::everything()),
            now,
        )
        .await
        .unwrap();
        assert!(!token.is_system_admin(), "a token never carries it");

        let person = session.person();
        assert_eq!(person.id, bob);
        assert!(session.is_held_by(&Holder::Group { id: group }, |_| None));
        assert!(session.is_held_by(&Holder::User { id: bob }, |_| None));
        assert!(!session.is_held_by(&Holder::User { id: ada }, |_| None));
        assert!(session.is_held_by(
            &Holder::Role {
                role: Role::Viewer,
                at: None
            },
            |_| Some(Role::Developer)
        ));
        assert!(!session.is_held_by(
            &Holder::Role {
                role: Role::Admin,
                at: None
            },
            |_| Some(Role::Developer)
        ));
        assert!(session.is_held_by(&Holder::Everyone, |_| None));

        suspend(&mut conn, bob).await.unwrap();
        let suspended = Principal::load(&mut conn, Caller::session(bob), now)
            .await
            .unwrap();
        assert!(suspended.suspension().altogether());
        assert!(matches!(
            Principal::load(&mut conn, Caller::session(new_id()), now).await,
            Err(Error::NoSuchUser(_))
        ));
    }
}
