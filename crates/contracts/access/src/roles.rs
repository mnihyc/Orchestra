//! The four roles, as names held in a place, with Admin inherited downward from the system through orgs and the orgs inside them.
//!
//! There is one hierarchy: the system, its orgs, which lie inside one another
//! to any depth, their projects, topics and threads. A [`Role`] is a name for
//! the people who hold it in a [place](PlaceKind): it is given to a
//! [`Subject`] there, reaches everything inside, and carries no permission by
//! itself. What a role may do is whatever the pages name it for.
//!
//! [`role_at`] is the one rule by which a person's role at a node is found
//! from every way they hold one on its path. The system is the first node of
//! every path, so whoever is Admin there is Admin everywhere; the owners of a
//! place count as its Admins. Nothing else sets one admin apart from another.

use orch_types::{Id, Revision, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The most characters of the note on a role or on an entry of a page.
///
/// A note says why somebody was named, in a sentence or two; five hundred
/// characters hold that and keep a list of entries readable.
pub const NOTE_MAX_CHARS: usize = 500;

/// A name that the catalogue does not have.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{name:?} is no {what}")]
pub struct UnknownName {
    /// What kind of name was expected, as `role` or `permission`.
    pub what: &'static str,
    /// The name as it was given.
    pub name: String,
}

impl UnknownName {
    pub(crate) fn new(what: &'static str, name: &str) -> Self {
        Self {
            what,
            name: name.to_owned(),
        }
    }
}

/// Declares an enum that is written as a fixed word, with its list, its word
/// and its reading.
macro_rules! words {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident ($what:literal) {
            $( $(#[$vmeta:meta])* $variant:ident = $word:literal, )*
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
        $vis enum $name {
            $( $(#[$vmeta])* #[serde(rename = $word)] $variant, )*
        }

        impl $name {
            /// Every one, in the order of the catalogue.
            pub const ALL: &'static [$name] = &[ $( $name::$variant, )* ];

            /// The word it is written with.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self { $( $name::$variant => $word, )* }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl std::str::FromStr for $name {
            type Err = $crate::roles::UnknownName;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                match text {
                    $( $word => Ok($name::$variant), )*
                    _ => Err($crate::roles::UnknownName::new($what, text)),
                }
            }
        }
    };
}
pub(crate) use words;

words! {
    /// A kind of node of the hierarchy. An org lies in another org or in none;
    /// one that lies in another is what people call a department, and it is an
    /// org in everything.
    pub enum NodeKind ("kind of node") {
        /// An org, at any level.
        Org = "org",
        /// A project, which lies in an org.
        Project = "project",
        /// A topic, which lies in a project.
        Topic = "topic",
        /// A thread, which lies in a topic.
        Thread = "thread",
    }
}

impl NodeKind {
    /// The kind of node this one lies in. An org lies in an org, or as a
    /// root org in none.
    #[must_use]
    pub const fn lies_in(self) -> NodeKind {
        match self {
            Self::Org | Self::Project => Self::Org,
            Self::Topic => Self::Project,
            Self::Thread => Self::Topic,
        }
    }

    /// The kind of place it is, where roles are held and defaults are kept.
    /// A thread is none: its people are named on its page.
    #[must_use]
    pub const fn place(self) -> Option<PlaceKind> {
        match self {
            Self::Org => Some(PlaceKind::Org),
            Self::Project => Some(PlaceKind::Project),
            Self::Topic => Some(PlaceKind::Topic),
            Self::Thread => None,
        }
    }
}

words! {
    /// A kind of place: where roles are held and where a defaults page says
    /// what is given to the things made inside.
    pub enum PlaceKind ("kind of place") {
        /// The system: the top of the hierarchy, one for an installation.
        System = "system",
        /// An org, at any level.
        Org = "org",
        /// A project.
        Project = "project",
        /// A topic.
        Topic = "topic",
    }
}

impl PlaceKind {
    /// Whether `role` can be held there. The one role held at the system is
    /// Admin; an org, a project and a topic take all four.
    #[must_use]
    pub const fn takes(self, role: Role) -> bool {
        match self {
            Self::System => matches!(role, Role::Admin),
            Self::Org | Self::Project | Self::Topic => true,
        }
    }

    /// The kinds of place that can lie inside it, at any depth.
    #[must_use]
    pub const fn holds(self) -> &'static [PlaceKind] {
        match self {
            Self::System | Self::Org => &[Self::Org, Self::Project, Self::Topic],
            Self::Project => &[Self::Topic],
            Self::Topic => &[],
        }
    }
}

/// One place: the system, or a node that is an org, a project or a topic.
/// It is written as `system` or as the node's identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlaceRef {
    /// The system.
    System,
    /// The node with this identifier.
    Node(Id),
}

impl PlaceRef {
    /// The node, where it is one.
    #[must_use]
    pub const fn node(self) -> Option<Id> {
        match self {
            Self::System => None,
            Self::Node(id) => Some(id),
        }
    }
}

impl From<Id> for PlaceRef {
    fn from(id: Id) -> Self {
        Self::Node(id)
    }
}

/// Native-only View provenance, before a reader qualifies the place's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminReach {
    /// The nearest place whose administration gives the otherwise absent View.
    pub place: PlaceRef,
    /// The content area's page is cut loose below its administrative defaults.
    pub private: bool,
}

/// The word the system is written with where a place is named.
const SYSTEM: &str = "system";

impl std::fmt::Display for PlaceRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::System => f.write_str(SYSTEM),
            Self::Node(id) => id.fmt(f),
        }
    }
}

impl std::str::FromStr for PlaceRef {
    type Err = UnknownName;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text == SYSTEM {
            return Ok(Self::System);
        }
        text.parse()
            .map(Self::Node)
            .map_err(|_| UnknownName::new("place", text))
    }
}

impl Serialize for PlaceRef {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for PlaceRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for PlaceRef {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "PlaceRef".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "anyOf": [
                { "type": "string", "const": SYSTEM },
                { "type": "string", "format": "uuid" }
            ]
        })
    }
}

words! {
    /// What somebody is in a place. Each role contains the ones before it: a
    /// row that names a role is held by everyone whose role there is that one
    /// or a higher one.
    #[derive(PartialOrd, Ord)]
    pub enum Role ("role") {
        /// Viewer: the lowest.
        Viewer = "viewer",
        /// Interactor.
        Interactor = "interactor",
        /// Developer.
        Developer = "developer",
        /// Admin: the highest, and the one that is inherited from the system
        /// down through every org.
        Admin = "admin",
    }
}

impl Role {
    /// Its name, as people read it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Viewer => "Viewer",
            Self::Interactor => "Interactor",
            Self::Developer => "Developer",
            Self::Admin => "Admin",
        }
    }

    /// Whether somebody with this role is among those a row means when it
    /// names `named`: this role is that one or a higher one.
    #[must_use]
    pub fn includes(self, named: Role) -> bool {
        self >= named
    }

    /// Whether giving, changing or taking away a role takes an admin or an
    /// owner of the place: it does when Admin is what is given or what is
    /// taken. Any other change is made by whoever may change the roles there.
    #[must_use]
    pub fn takes_an_admin(before: Option<Role>, after: Option<Role>) -> bool {
        before == Some(Role::Admin) || after == Some(Role::Admin)
    }
}

words! {
    /// What a role is given to.
    pub enum SubjectKind ("kind of subject") {
        /// One person.
        User = "user",
        /// The people of a group.
        Group = "group",
        /// Everyone in an org: the members whose domain is that org or an org
        /// inside it.
        Members = "members",
        /// The people holding a role at the named place.
        Role = "role",
    }
}

/// Whom a role is given to: a person, a group, or everyone in an org. Any of
/// them may be of another tree of orgs than the place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Subject {
    /// What kind of subject it is.
    pub kind: SubjectKind,
    /// The user, the group, or the org whose members are meant.
    pub id: Id,
    /// The role held at `id`, for a role subject.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
}

impl Subject {
    /// The person `id`.
    #[must_use]
    pub const fn user(id: Id) -> Self {
        Self {
            kind: SubjectKind::User,
            id,
            role: None,
        }
    }

    /// The group `id`.
    #[must_use]
    pub const fn group(id: Id) -> Self {
        Self {
            kind: SubjectKind::Group,
            id,
            role: None,
        }
    }

    /// Everyone in the org `id`.
    #[must_use]
    pub const fn members(id: Id) -> Self {
        Self {
            kind: SubjectKind::Members,
            id,
            role: None,
        }
    }

    /// Whether the subject is, or contains, `person`.
    #[must_use]
    pub fn matches(&self, person: &Person<'_>) -> bool {
        match self.kind {
            SubjectKind::User => self.id == person.id,
            SubjectKind::Group => person.groups.contains(&self.id),
            SubjectKind::Members => person.member_of.contains(&self.id),
            // A role subject is expanded with its place by the authorization reader.
            SubjectKind::Role => false,
        }
    }
}

/// What has to be known of a person to say whether a subject, or a holder on
/// a page, is them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Person<'a> {
    /// The person.
    pub id: Id,
    /// The groups they are in.
    pub groups: &'a [Id],
    /// Every org they are a member of: in each tree they belong to, their
    /// domain and the orgs above it up to the root org. Holding a role in an
    /// org from above makes nobody a member of it, and a system admin is a
    /// member of none by being one.
    pub member_of: &'a [Id],
}

/// Whether a member whose domain lies on `domain_path` is a member of `org`.
///
/// `domain_path` is the orgs from the root org down to the member's domain,
/// both included. The members of an org are those whose domain is that org
/// or an org inside it, so everybody of a tree is a member of its root org,
/// and nobody is a member of an org beside or below their domain.
#[must_use]
pub fn is_member_of(domain_path: &[Id], org: Id) -> bool {
    domain_path.contains(&org)
}

/// Whether a person holds nothing on a thing because they are suspended.
///
/// `altogether` says that the user is suspended on the whole system.
/// `suspended_in` is the orgs in which they are suspended, and `orgs` the
/// orgs that the thing lies in, from its root org inward: a suspension in
/// one org closes that org and what is inside it, and nothing beside or
/// above it. What a suspended person owns stays theirs; lifting the
/// suspension puts everything back.
#[must_use]
pub fn is_suspended(altogether: bool, suspended_in: &[Id], orgs: &[Id]) -> bool {
    altogether || orgs.iter().any(|org| suspended_in.contains(org))
}

/// A person's membership of a tree of orgs, with their domain: the one org
/// of that tree they belong to, where they hold a role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Member {
    /// The root org of the tree.
    pub org: Id,
    /// The person.
    pub user: Id,
    /// Their domain: the root org itself, or an org inside it at any depth.
    pub domain: Id,
    /// When they joined the tree.
    pub joined_at: Time,
    /// Who invited them, where somebody did.
    pub invited_by: Option<Id>,
    /// When their domain last changed.
    pub moved_at: Option<Time>,
    /// Who changed it.
    pub moved_by: Option<Id>,
}

/// A named set of people that one org keeps, at any level. It may be given a
/// role in a place, and named in the rows of any page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Group {
    /// The group.
    pub id: Id,
    /// The org that keeps it.
    pub org: Id,
    /// Its name, unique in that org whatever the case of its letters.
    pub name: String,
    /// What it is for.
    pub description: String,
}

/// A role given to a subject in a place: on an org at any level, a project
/// or a topic, never on a thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RoleHeld {
    /// The identifier of this giving.
    pub id: Id,
    /// The place it is held in.
    pub node: Id,
    /// Whom it is given to.
    pub subject: Subject,
    /// The role.
    pub role: Role,
    /// When it ends by itself.
    pub expires_at: Option<Time>,
    /// Why it was given, in at most [`NOTE_MAX_CHARS`] characters.
    pub note: String,
    /// Who gave it.
    pub created_by: Id,
    /// When it was given.
    pub created_at: Time,
    /// When it was taken away. It is kept for the record and counts for
    /// nothing from then on.
    pub removed_at: Option<Time>,
    /// How many times it was changed.
    pub revision: Revision,
}

impl RoleHeld {
    /// Whether it counts at `now`: it was not taken away and has not ended.
    #[must_use]
    pub fn counts(&self, now: Time) -> bool {
        self.removed_at.is_none() && !ended(self.expires_at, now)
    }
}

/// Whether something that ends at `expires_at` has ended at `now`. It has
/// from the moment of its end on.
pub(crate) fn ended(expires_at: Option<Time>, now: Time) -> bool {
    expires_at.is_some_and(|end| end <= now)
}

/// One way in which a person holds a role on the path of a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Held {
    /// The role.
    pub role: Role,
    /// Where it is held.
    pub at: PlaceRef,
}

impl Held {
    /// Admin of the system, which is Admin of every org and of everything in
    /// it. Pass it only for a caller for whom that counts: see
    /// [`Credential::carries_system_admin`].
    #[must_use]
    pub const fn system_admin() -> Self {
        Self {
            role: Role::Admin,
            at: PlaceRef::System,
        }
    }

    /// One of the owners of the org, project or topic `place`: the owners of
    /// a place count as its Admins.
    #[must_use]
    pub const fn owner_of(place: Id) -> Self {
        Self {
            role: Role::Admin,
            at: PlaceRef::Node(place),
        }
    }

    /// `role`, given on the org, project or topic `place` to a subject that
    /// is the person, and counting now.
    #[must_use]
    pub const fn given(role: Role, place: Id) -> Self {
        Self {
            role,
            at: PlaceRef::Node(place),
        }
    }
}

/// A person's role at a node: the highest among every way they hold one on
/// its path, or none.
///
/// `ways` holds, for the nodes from the system down to the node itself, each
/// role that reaches the person there: as a system admin, as an owner of a
/// place, or given to them, to a group of theirs or to an org they are a
/// member of. Of several ways to the same role the first given is returned,
/// so that passing them from the top down names the place furthest up.
#[must_use]
pub fn role_at(ways: impl IntoIterator<Item = Held>) -> Option<Held> {
    let mut best: Option<Held> = None;
    for way in ways {
        if best.is_none_or(|held| way.role > held.role) {
            best = Some(way);
        }
    }
    best
}

words! {
    /// What a request is made with.
    pub enum Credential ("credential") {
        /// A browser session: a person at a screen.
        Session = "session",
        /// An API token, which acts as its user within its scopes.
        Token = "token",
    }
}

impl Credential {
    /// Whether what a user holds as a system admin counts for a request made
    /// with this. It counts in a browser session only: with a token a system
    /// admin holds what their own roles and entries give them, so that a
    /// token that leaks is never the whole system.
    #[must_use]
    pub const fn carries_system_admin(self) -> bool {
        matches!(self, Self::Session)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::new_id;

    #[test]
    fn each_role_contains_the_ones_before_it() {
        assert!(Role::Admin.includes(Role::Viewer));
        assert!(Role::Developer.includes(Role::Interactor));
        assert!(Role::Interactor.includes(Role::Interactor));
        assert!(!Role::Viewer.includes(Role::Interactor));
        assert!(!Role::Developer.includes(Role::Admin));
    }

    #[test]
    fn a_role_at_a_node_is_the_highest_held_on_its_path() {
        let (org, project, topic) = (new_id(), new_id(), new_id());
        let held = role_at([
            Held::given(Role::Viewer, org),
            Held::given(Role::Developer, project),
            Held::given(Role::Interactor, topic),
        ]);
        assert_eq!(held, Some(Held::given(Role::Developer, project)));
        assert_eq!(role_at([]), None);
    }

    #[test]
    fn a_system_admin_is_an_admin_of_every_node() {
        let topic = new_id();
        let held = role_at([Held::system_admin(), Held::given(Role::Viewer, topic)]);
        assert_eq!(
            held,
            Some(Held {
                role: Role::Admin,
                at: PlaceRef::System
            })
        );
    }

    #[test]
    fn the_owners_of_a_place_count_as_its_admins() {
        let (org, project) = (new_id(), new_id());
        let held = role_at([Held::given(Role::Interactor, org), Held::owner_of(project)]);
        assert_eq!(held.map(|held| held.role), Some(Role::Admin));
        assert_eq!(held.map(|held| held.at), Some(PlaceRef::Node(project)));
    }

    #[test]
    fn of_two_ways_to_one_role_the_one_further_up_is_named() {
        let (org, project) = (new_id(), new_id());
        let held = role_at([Held::given(Role::Admin, org), Held::owner_of(project)]);
        assert_eq!(held.map(|held| held.at), Some(PlaceRef::Node(org)));
    }

    #[test]
    fn what_a_system_admin_holds_counts_in_a_browser_session_only() {
        assert!(Credential::Session.carries_system_admin());
        assert!(!Credential::Token.carries_system_admin());
    }

    #[test]
    fn only_admin_is_held_at_the_system_and_no_role_on_a_thread() {
        assert!(PlaceKind::System.takes(Role::Admin));
        assert!(!PlaceKind::System.takes(Role::Developer));
        assert!(PlaceKind::Topic.takes(Role::Viewer));
        assert_eq!(NodeKind::Thread.place(), None);
        assert_eq!(NodeKind::Org.place(), Some(PlaceKind::Org));
    }

    #[test]
    fn giving_or_taking_admin_takes_an_admin() {
        assert!(Role::takes_an_admin(None, Some(Role::Admin)));
        assert!(Role::takes_an_admin(
            Some(Role::Admin),
            Some(Role::Developer)
        ));
        assert!(Role::takes_an_admin(Some(Role::Admin), None));
        assert!(!Role::takes_an_admin(
            Some(Role::Viewer),
            Some(Role::Developer)
        ));
    }

    #[test]
    fn a_subject_matches_a_person_a_group_of_theirs_or_an_org_they_are_in() {
        let (me, group, root, department, sibling) =
            (new_id(), new_id(), new_id(), new_id(), new_id());
        let groups = [group];
        let member_of = [root, department];
        let person = Person {
            id: me,
            groups: &groups,
            member_of: &member_of,
        };
        assert!(Subject::user(me).matches(&person));
        assert!(!Subject::user(new_id()).matches(&person));
        assert!(Subject::group(group).matches(&person));
        assert!(!Subject::group(new_id()).matches(&person));
        assert!(Subject::members(root).matches(&person));
        assert!(Subject::members(department).matches(&person));
        assert!(!Subject::members(sibling).matches(&person));
    }

    #[test]
    fn a_member_is_in_their_domain_and_every_org_above_it_and_in_none_beside() {
        let (root, research, ml, sales) = (new_id(), new_id(), new_id(), new_id());
        let domain_path = [root, research];
        assert!(is_member_of(&domain_path, root));
        assert!(is_member_of(&domain_path, research));
        assert!(
            !is_member_of(&domain_path, ml),
            "an org inside their domain is not theirs"
        );
        assert!(!is_member_of(&domain_path, sales));
    }

    #[test]
    fn a_suspension_closes_the_org_it_was_made_in_and_what_is_inside_it() {
        let (root, research, ml, sales, elsewhere) =
            (new_id(), new_id(), new_id(), new_id(), new_id());
        let in_research = [research];
        assert!(
            is_suspended(false, &in_research, &[root, research]),
            "the org itself"
        );
        assert!(
            is_suspended(false, &in_research, &[root, research, ml]),
            "an org inside it"
        );
        assert!(
            !is_suspended(false, &in_research, &[root]),
            "the org above it"
        );
        assert!(
            !is_suspended(false, &in_research, &[root, sales]),
            "a sibling org"
        );
        assert!(
            !is_suspended(false, &in_research, &[elsewhere]),
            "another root org"
        );
        assert!(is_suspended(true, &[], &[elsewhere]), "altogether");
        assert!(!is_suspended(false, &[], &[root, research]));
    }

    #[test]
    fn a_role_that_ended_or_was_taken_away_counts_for_nothing() {
        let now: Time = "2026-10-08T12:00:00Z".parse().unwrap();
        let earlier: Time = "2026-10-08T11:59:59Z".parse().unwrap();
        let later: Time = "2026-10-09T00:00:00Z".parse().unwrap();
        let mut held = RoleHeld {
            id: new_id(),
            node: new_id(),
            subject: Subject::user(new_id()),
            role: Role::Developer,
            expires_at: Some(later),
            note: String::new(),
            created_by: new_id(),
            created_at: earlier,
            removed_at: None,
            revision: 1,
        };
        assert!(held.counts(now));
        held.expires_at = Some(now);
        assert!(
            !held.counts(now),
            "it has ended from the moment of its end on"
        );
        held.expires_at = None;
        held.removed_at = Some(earlier);
        assert!(!held.counts(now));
    }

    #[test]
    fn a_place_is_written_as_the_word_system_or_as_its_identifier() {
        let node = new_id();
        assert_eq!(serde_json::to_value(PlaceRef::System).unwrap(), "system");
        assert_eq!(
            serde_json::to_value(PlaceRef::Node(node)).unwrap(),
            node.to_string()
        );
        assert_eq!(
            serde_json::from_value::<PlaceRef>("system".into()).unwrap(),
            PlaceRef::System
        );
        assert_eq!(
            node.to_string().parse::<PlaceRef>().unwrap(),
            PlaceRef::Node(node)
        );
        assert!("everywhere".parse::<PlaceRef>().is_err());
    }

    #[test]
    fn a_subject_is_written_with_its_kind_and_identifier() {
        let org = new_id();
        let written = serde_json::to_value(Subject::members(org)).unwrap();
        assert_eq!(written, serde_json::json!({ "kind": "members", "id": org }));
        assert_eq!(
            serde_json::from_value::<Subject>(written).unwrap(),
            Subject::members(org)
        );
        let holder = crate::pages::Holder::Members { id: org };
        assert_eq!(
            serde_json::to_value(holder).unwrap(),
            serde_json::to_value(Subject::members(org)).unwrap()
        );
        assert!(
            serde_json::from_value::<Subject>(
                serde_json::json!({ "kind": "org_members", "id": org })
            )
            .is_err()
        );
        assert_eq!("developer".parse::<Role>().unwrap(), Role::Developer);
        assert!("owner".parse::<Role>().is_err());
    }
}
