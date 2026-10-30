//! Permission pages: the Owners row, rows that follow or are cut loose, entries with their holders, options, expiry and source; defaults pages with their two profiles and locks.
//!
//! Every thing has a page: one [`Row`] for each permission that exists on its
//! kind, and in each row the [`Entry`]s that name its holders. A row follows
//! the default of the place the thing is in until it is cut loose; a place
//! keeps those defaults on a page of its own, where a row may also be locked.
//! Rows that nobody changed are not stored, so a new thing's page is its
//! Owners row and nothing else: inherited throughout, and its owners.
//!
//! What a row comes to is read from a [`Chain`]: the thing's own row, then
//! the rows it follows, nearest first. A change of a page arrives as a
//! [`PageChange`]; [`check`] says whether it can be a change of that kind of
//! page at all, and spells it out row by row. Whether the person who asks may
//! make it, and what it does to the people on the page, are decided where the
//! stored pages are.

use orch_types::fields::FieldError;
use orch_types::{Id, Json, Revision, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

use crate::permissions::{
    Choice, HolderKind, NamedBy, ObjectKind, Options, Permission, Placement, Profile,
};
use crate::roles::{NOTE_MAX_CHARS, Person, PlaceKind, PlaceRef, Role, ended, words};

words! {
    /// Which of the two pages of a thing a row is on.
    pub enum PageKind ("page") {
        /// The thing's own page.
        Live = "live",
        /// For a place: what it gives to the pages of what is made inside it.
        Defaults = "defaults",
    }
}

words! {
    /// What a stored row or entry belongs to, as it is kept. Every node is a
    /// `node`; the defaults of the system belong to `system`; and a direct or
    /// group chat, which has no page, keeps as `chat` the entries by which it
    /// was shared with agents.
    pub enum StoredKind ("kind of stored object") {
        /// The system, for its defaults page.
        System = "system",
        /// An org, a project, a topic or a thread.
        Node = "node",
        /// A provider.
        Provider = "provider",
        /// An agent profile.
        Agent = "agent",
        /// An environment.
        Environment = "environment",
        /// A host.
        Host = "host",
        /// A physical machine and its shared management page.
        Machine = "machine",
        /// A template.
        Template = "template",
        /// An MCP server.
        McpServer = "mcp_server",
        /// A mount.
        Mount = "mount",
        /// A skill.
        Skill = "skill",
        /// A document.
        Document = "document",
        /// A document-only container's live permission page.
        DocumentContainer = "document_container",
        /// A direct or group chat that was shared with agents.
        Chat = "chat",
    }
}

impl From<ObjectKind> for StoredKind {
    fn from(kind: ObjectKind) -> Self {
        match kind {
            ObjectKind::Org | ObjectKind::Project | ObjectKind::Topic | ObjectKind::Thread => {
                Self::Node
            }
            ObjectKind::Document => Self::Document,
            ObjectKind::DocumentContainer => Self::DocumentContainer,
            ObjectKind::Provider => Self::Provider,
            ObjectKind::Agent => Self::Agent,
            ObjectKind::Environment => Self::Environment,
            ObjectKind::Host => Self::Host,
            ObjectKind::Machine => Self::Machine,
            ObjectKind::Template => Self::Template,
            ObjectKind::McpServer => Self::McpServer,
            ObjectKind::Mount => Self::Mount,
            ObjectKind::Skill => Self::Skill,
        }
    }
}

words! {
    /// How an entry came about.
    pub enum EntrySource ("source of an entry") {
        /// Written on the page by whoever made it, an admin who named
        /// themself included.
        Page = "page",
        /// Chosen when the thing was made.
        Creation = "creation",
        /// An approved request for access.
        Request = "request",
        /// Shipped with the system.
        Seed = "seed",
    }
}

/// Whom an entry names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Holder {
    /// Members whose domain is this org or an org inside it, distinct from its viewers.
    Members {
        /// The org.
        id: Id,
    },
    /// Everyone whose role is that one or a higher one.
    Role {
        /// The role.
        role: Role,
        /// The place where the role is held, which may be of another tree of
        /// orgs. Left out, it is where the thing is: the node itself, for a
        /// resource its home, for a document its node.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<Id>,
    },
    /// The members of a group.
    Group {
        /// The group.
        id: Id,
    },
    /// One person.
    User {
        /// The person.
        id: Id,
    },
    /// The work in a place: a thread there may run on the resource, and
    /// whoever gives such a thread input does so on it. It names no person.
    Node {
        /// The place, or one thread.
        id: Id,
    },
    /// The agents of the threads at or under a place, those started later
    /// among them. It names no person.
    Agents {
        /// The place, or one thread.
        id: Id,
    },
    /// Every user of the system.
    Everyone,
}

impl Holder {
    /// What kind of holder it is.
    #[must_use]
    pub const fn kind(&self) -> HolderKind {
        match self {
            Self::Role { .. } => HolderKind::Role,
            Self::Group { .. } => HolderKind::Group,
            Self::Members { .. } => HolderKind::Members,
            Self::User { .. } => HolderKind::User,
            Self::Node { .. } => HolderKind::Node,
            Self::Agents { .. } => HolderKind::Agents,
            Self::Everyone => HolderKind::Everyone,
        }
    }

    /// Whether the holder is, or contains, `person`.
    ///
    /// `role_at` says which role the person has at the place a role holder
    /// names, or, asked with `None`, where the thing is. The work in a place
    /// and the agents of a place are no person, and match nobody.
    #[must_use]
    pub fn matches(
        &self,
        person: &Person<'_>,
        role_at: impl FnOnce(Option<Id>) -> Option<Role>,
    ) -> bool {
        match self {
            Self::User { id } => *id == person.id,
            Self::Group { id } => person.groups.contains(id),
            Self::Members { id } => person.member_of.contains(id),
            Self::Role { role, at } => role_at(*at).is_some_and(|held| held.includes(*role)),
            Self::Everyone => true,
            Self::Node { .. } | Self::Agents { .. } => false,
        }
    }

    /// Whether the holder is the work in a place on `path`: the nodes from
    /// the root org down to a thread, or to the node whose settings name the
    /// resource. A resource that names a place is open to the threads at and
    /// under that place, of whatever org.
    #[must_use]
    pub fn is_work_on(&self, path: &[Id]) -> bool {
        matches!(self, Self::Node { id } if path.contains(id))
    }

    /// Whether the holder is the agents of a place on `path`: the nodes from
    /// the root org down to the thread whose agent asks. What a place was
    /// given, the agent of every thread under it has.
    #[must_use]
    pub fn is_agents_on(&self, path: &[Id]) -> bool {
        matches!(self, Self::Agents { id } if path.contains(id))
    }

    /// The holder as an entry keeps it: its kind, the identifier it names,
    /// and for a role that role. A role names an identifier only when it is
    /// held somewhere else than where the thing is; everyone names none.
    #[must_use]
    pub const fn stored(&self) -> (HolderKind, Option<Id>, Option<Role>) {
        match *self {
            Self::Role { role, at } => (HolderKind::Role, at, Some(role)),
            Self::Group { id } => (HolderKind::Group, Some(id), None),
            Self::Members { id } => (HolderKind::Members, Some(id), None),
            Self::User { id } => (HolderKind::User, Some(id), None),
            Self::Node { id } => (HolderKind::Node, Some(id), None),
            Self::Agents { id } => (HolderKind::Agents, Some(id), None),
            Self::Everyone => (HolderKind::Everyone, None, None),
        }
    }

    /// The holder that an entry keeps as `kind`, `id` and `role`; nothing
    /// when the three do not go together: a role without its role, another
    /// kind with one, or a group, a person or a place without an identifier.
    #[must_use]
    pub const fn from_stored(kind: HolderKind, id: Option<Id>, role: Option<Role>) -> Option<Self> {
        Some(match (kind, id, role) {
            (HolderKind::Role, at, Some(role)) => Self::Role { role, at },
            (HolderKind::Group, Some(id), None) => Self::Group { id },
            (HolderKind::Members, Some(id), None) => Self::Members { id },
            (HolderKind::User, Some(id), None) => Self::User { id },
            (HolderKind::Node, Some(id), None) => Self::Node { id },
            (HolderKind::Agents, Some(id), None) => Self::Agents { id },
            (HolderKind::Everyone, None, None) => Self::Everyone,
            _ => return None,
        })
    }
}

/// One holder named in one row, with what it may do there and until when.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Entry {
    /// The entry.
    pub id: Id,
    /// Whom it names.
    pub holder: Holder,
    /// What the holder may do with the permission, where it takes options;
    /// an empty object otherwise. Read them with [`Options::parse`].
    pub options: Json,
    /// When it ends by itself; this is how access for a week is given.
    pub expires_at: Option<Time>,
    /// Why, in at most [`NOTE_MAX_CHARS`] characters.
    pub note: String,
    /// How it came about.
    pub source: EntrySource,
    /// Who wrote it; nobody for what was shipped.
    pub created_by: Option<Id>,
    /// When it was written.
    pub created_at: Time,
    /// When it was removed. It is kept for the record and counts for nothing
    /// from then on.
    pub removed_at: Option<Time>,
    /// How many times it was changed.
    pub revision: Revision,
}

impl Entry {
    /// Whether it counts at `now`: it was not removed and has not ended.
    #[must_use]
    pub fn counts(&self, now: Time) -> bool {
        self.removed_at.is_none() && !ended(self.expires_at, now)
    }
}

/// One permission on one page: whether it follows the default of the place
/// above, and whom it names itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Row {
    /// The permission.
    pub permission: Permission,
    /// On a defaults page, which of the rows of that permission it is; `any`
    /// on a thing's own page.
    pub profile: Profile,
    /// Whether it follows the default of the place above. Switched off, the
    /// row is cut loose: it is held by the owners and by whom it names.
    pub inherit: bool,
    /// On a defaults page: whether the pages inside follow this row exactly,
    /// whatever they store for it.
    pub locked: bool,
    /// Whom it names.
    pub entries: Vec<Entry>,
}

impl Row {
    /// The row as it is while nothing is stored for it: it follows, and
    /// names nobody.
    #[must_use]
    pub const fn following(permission: Permission, profile: Profile) -> Self {
        Self {
            permission,
            profile,
            inherit: true,
            locked: false,
            entries: Vec::new(),
        }
    }

    /// The entries that count at `now`.
    pub fn names(&self, now: Time) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(move |entry| entry.counts(now))
    }
}

/// Where a row that a thing follows, or an entry that names somebody, stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "page", content = "of", rename_all = "snake_case")]
pub enum Origin {
    /// The thing's own page.
    Own,
    /// For a document: the page of the node it belongs to, whose row of the
    /// same name it follows.
    Node(Id),
    /// The live document row of an enclosing document folder.
    DocumentContainer(Id),
    /// The defaults page of a place.
    Defaults(PlaceRef),
}

/// An entry that gives a row its holder, with the page it stands on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Named<'a> {
    /// The entry.
    pub entry: &'a Entry,
    /// Where it stands.
    pub from: Origin,
}

#[derive(Debug, Clone, Copy)]
struct Link<'a> {
    origin: Origin,
    row: Option<&'a Row>,
}

impl Link<'_> {
    /// A row that is not stored follows.
    fn follows(&self) -> bool {
        self.origin != Origin::Defaults(PlaceRef::System) && self.row.is_none_or(|row| row.inherit)
    }

    fn locked(&self) -> bool {
        matches!(self.origin, Origin::Defaults(_)) && self.row.is_some_and(|row| row.locked)
    }
}

/// One row of a page together with the rows it follows, nearest first: what
/// the row comes to is read from it.
///
/// A thing's row follows the defaults row of the place the thing is in, that
/// one the row of the place above, and so on up to the system, whose
/// defaults follow nothing. A document's row follows the row of the same
/// name on the page of its node first. A row that is not stored is given as
/// `None`: it follows, and names nobody.
#[derive(Debug, Clone)]
pub struct Chain<'a> {
    links: Vec<Link<'a>>,
}

impl<'a> Chain<'a> {
    /// The chain of a row on a thing's own page.
    #[must_use]
    pub fn of(own: Option<&'a Row>) -> Self {
        Self {
            links: vec![Link {
                origin: Origin::Own,
                row: own,
            }],
        }
    }

    /// The chain of a row on the defaults page of `place`, for reading that
    /// page itself.
    #[must_use]
    pub fn of_defaults(place: PlaceRef, row: Option<&'a Row>) -> Self {
        Self {
            links: vec![Link {
                origin: Origin::Defaults(place),
                row,
            }],
        }
    }

    /// Adds, for a document, the row of the same name on the page of the
    /// node it belongs to.
    #[must_use]
    pub fn beside(mut self, node: Id, row: Option<&'a Row>) -> Self {
        self.links.push(Link {
            origin: Origin::Node(node),
            row,
        });
        self
    }

    /// Adds a containing folder's live document row, without inventing a hierarchy place.
    #[must_use]
    pub fn in_document_container(mut self, id: Id, row: Option<&'a Row>) -> Self {
        self.links.push(Link {
            origin: Origin::DocumentContainer(id),
            row,
        });
        self
    }

    /// Adds the defaults row of the next place above. The system's comes
    /// last.
    #[must_use]
    pub fn under(mut self, place: PlaceRef, row: Option<&'a Row>) -> Self {
        self.links.push(Link {
            origin: Origin::Defaults(place),
            row,
        });
        self
    }

    fn lock(&self) -> Option<(usize, PlaceRef)> {
        self.links
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, link)| match link.origin {
                Origin::Defaults(place) if link.locked() => Some((index, place)),
                _ => None,
            })
    }

    /// The place whose lock decides the row: the topmost place on the way
    /// up whose defaults row is locked. The pages inside it follow that row
    /// exactly: they cannot add to it or cut it loose, and what they stored
    /// for it counts for nothing while the lock holds.
    #[must_use]
    pub fn locked_by(&self) -> Option<PlaceRef> {
        self.lock().map(|(_, place)| place)
    }

    /// The place whose lock decides the row from above the page that is
    /// read. Such a row is not changed on this page: it is changed where the
    /// lock is. Nothing for a lock that was set on this defaults page itself.
    #[must_use]
    pub fn locked_above(&self) -> Option<PlaceRef> {
        self.lock()
            .filter(|(index, _)| *index > 0)
            .map(|(_, place)| place)
    }

    fn gathered(&self, from: usize, now: Time) -> Vec<Named<'a>> {
        let mut named = Vec::new();
        for link in self.links.iter().skip(from) {
            if let Some(row) = link.row {
                named.extend(row.names(now).map(|entry| Named {
                    entry,
                    from: link.origin,
                }));
            }
            if !link.follows() {
                break;
            }
        }
        named
    }

    /// Whom the row names at `now`, by itself and through what it follows,
    /// each with the page the entry stands on.
    ///
    /// A lock decides whatever is stored below it. Otherwise the row gives
    /// to whom it names, and for as long as it follows, to whom the rows
    /// above it name. An entry that ended or was removed counts for nothing.
    /// The owners of the thing hold the row besides, whatever it names.
    #[must_use]
    pub fn holders(&self, now: Time) -> Vec<Named<'a>> {
        self.gathered(self.lock().map_or(0, |(index, _)| index), now)
    }

    /// What following amounts to at `now`: whom the rows above give the row.
    /// Nothing for a row that was cut loose and that no lock decides.
    #[must_use]
    pub fn inherited(&self, now: Time) -> Option<Vec<Named<'a>>> {
        match self.lock() {
            Some((index, _)) if index > 0 => Some(self.gathered(index, now)),
            _ if self.links.first().is_some_and(Link::follows) => Some(self.gathered(1, now)),
            _ => None,
        }
    }

    /// Whether the row is cut loose somewhere below the defaults of `place`:
    /// on the thing's own page, or on a defaults page on the way up to it.
    ///
    /// A node is restricted, which other products call private, when this
    /// holds for its View row and its root org.
    #[must_use]
    pub fn cut_below(&self, place: PlaceRef) -> bool {
        let from = self.lock().map_or(0, |(index, _)| index);
        for link in self.links.iter().skip(from) {
            if link.origin == Origin::Defaults(place) {
                return false;
            }
            if !link.follows() {
                return link.origin != Origin::Defaults(PlaceRef::System);
            }
        }
        false
    }
}

/// Why a change of an Owners row cannot be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
pub enum OwnersFault {
    /// The last person in the row is not taken out: somebody else is named
    /// first.
    #[error("the last person in an Owners row is not taken out")]
    LastPerson,
}

/// What an Owners row is to become.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnersAfter {
    /// The owners to add: those asked for who are not in the row yet, and
    /// the person named first when they are not in it.
    pub add: Vec<Holder>,
    /// The entries to take out.
    pub remove: Vec<Id>,
    /// The first owner from then on.
    pub first: Id,
}

/// The Owners row of a thing: the people, and the groups, who own it.
///
/// Whoever makes a thing is its first entry. The row follows no default,
/// has none on any defaults page, and is never without a person. An owner
/// holds every permission of the page but the naming of somebody from
/// outside.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Owners {
    /// The entries of the row, each a person or a group.
    pub entries: Vec<Entry>,
    /// The first owner: the one of them who stands for the thing wherever
    /// exactly one person is needed, as in its address. It is the maker
    /// until an owner or an admin names another.
    pub first: Id,
}

impl Owners {
    /// The row of a thing that `maker` has just made, at `now`.
    #[must_use]
    pub fn of_maker(entry: Id, maker: Id, now: Time) -> Self {
        let entry = Entry {
            id: entry,
            holder: Holder::User { id: maker },
            options: Json::Object(serde_json::Map::new()),
            expires_at: None,
            note: String::new(),
            source: EntrySource::Creation,
            created_by: Some(maker),
            created_at: now,
            removed_at: None,
            revision: 1,
        };
        Self {
            entries: vec![entry],
            first: maker,
        }
    }

    /// Whether `person` is one of the owners at `now`: an entry that counts
    /// names them, or a group of theirs. Owning counts only while they are
    /// an active member of the thing's root org, which is asked elsewhere.
    #[must_use]
    pub fn include(&self, person: &Person<'_>, now: Time) -> bool {
        self.entries
            .iter()
            .filter(|entry| entry.counts(now))
            .any(|entry| entry.holder.matches(person, |_| None))
    }

    /// The people the row names in person at `now`, the one who has been in
    /// it longest first.
    #[must_use]
    pub fn people(&self, now: Time) -> Vec<Id> {
        let mut named: Vec<&Entry> = self
            .entries
            .iter()
            .filter(|entry| entry.counts(now))
            .collect();
        named.sort_by_key(|entry| entry.created_at);
        named
            .into_iter()
            .filter_map(|entry| match entry.holder {
                Holder::User { id } => Some(id),
                _ => None,
            })
            .collect()
    }

    /// What the row becomes by `change`, made at `now`.
    ///
    /// The person named first is first from then on, and is added when they
    /// are not in the row. When the first owner is taken out and nobody is
    /// named, the person who has been in the row longest is first. The row
    /// keeps at least one person, whatever groups it names: a change that
    /// would take the last one out is refused.
    pub fn after(&self, change: &CheckedOwners, now: Time) -> Result<OwnersAfter, OwnersFault> {
        let staying: Vec<&Entry> = self
            .entries
            .iter()
            .filter(|entry| entry.counts(now) && !change.remove.contains(&entry.id))
            .collect();
        let first_named = change.first.map(|id| Holder::User { id });
        let mut add: Vec<Holder> = Vec::new();
        for holder in change.add.iter().chain(first_named.iter()) {
            if !staying.iter().any(|entry| entry.holder == *holder) && !add.contains(holder) {
                add.push(*holder);
            }
        }
        let remaining = Self {
            entries: staying.into_iter().cloned().collect(),
            first: self.first,
        };
        let mut people = remaining.people(now);
        people.extend(add.iter().filter_map(|holder| match holder {
            Holder::User { id } => Some(*id),
            _ => None,
        }));
        let longest = people.first().copied().ok_or(OwnersFault::LastPerson)?;
        let first = change.first.unwrap_or(if people.contains(&self.first) {
            self.first
        } else {
            longest
        });
        Ok(OwnersAfter {
            add,
            remove: change.remove.clone(),
            first,
        })
    }
}

words! {
    /// How a thing's page stands to the defaults it could follow.
    pub enum PageState ("state of a page") {
        /// Every row follows the defaults, and names nobody itself.
        Follows = "follows",
        /// Somebody changed a row.
        Custom = "custom",
        /// Its View row no longer follows the defaults up to its root org.
        Restricted = "restricted",
    }
}

impl PageState {
    /// The state of a page of which `changed` says that something is stored
    /// for it besides its Owners row, and `restricted` that its View row is
    /// cut loose below its root org.
    #[must_use]
    pub const fn of(changed: bool, restricted: bool) -> Self {
        match (restricted, changed) {
            (true, _) => Self::Restricted,
            (false, true) => Self::Custom,
            (false, false) => Self::Follows,
        }
    }
}

/// Which page a change is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PageOf {
    /// The page of one thing.
    Thing {
        /// What kind of thing it is.
        kind: ObjectKind,
        /// Whether it is a resource of the system, which has no home.
        of_system: bool,
    },
    /// The defaults page of a place of this kind.
    Defaults(PlaceKind),
    /// A direct or group chat. It has no page: all it keeps is the entries
    /// by which it was shared with agents.
    Chat,
}

impl PageOf {
    /// The rows of such a page, in the order of the catalogue, without the
    /// Owners row. A defaults page has one row for every permission that
    /// exists on anything that can be made inside its place, and two where
    /// the default depends on where work runs.
    #[must_use]
    pub fn rows(self) -> Vec<(Permission, Profile)> {
        let mut rows = Vec::new();
        for permission in Permission::ALL
            .iter()
            .copied()
            .filter(|permission| self.has_row(*permission))
        {
            if matches!(self, Self::Defaults(_)) && permission.profiles() {
                rows.push((permission, Profile::Managed));
                rows.push((permission, Profile::Connected));
            } else {
                rows.push((permission, Profile::Any));
            }
        }
        rows
    }

    /// Whether such a page has a row for `permission`, the Owners row aside.
    #[must_use]
    pub fn has_row(self, permission: Permission) -> bool {
        if !permission.has_default() {
            return false;
        }
        match self {
            Self::Thing { kind, .. } => permission.exists_on(kind),
            Self::Defaults(place) => permission.on().iter().any(|kind| kind.made_in(place)),
            Self::Chat => matches!(permission, Permission::ChatView | Permission::ChatInteract),
        }
    }

    fn takes(self, holder: HolderKind, permission: Permission) -> bool {
        match self {
            Self::Thing { kind, of_system } => holder.fits(permission, kind, of_system),
            Self::Defaults(_) => holder.fits_defaults(permission),
            Self::Chat => matches!(holder, HolderKind::Agents),
        }
    }
}

/// A row of the defaults page that a new installation is seeded with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Shipped {
    /// The permission.
    pub permission: Permission,
    /// Which of its rows.
    pub profile: Profile,
    /// The role that row names: it and the higher ones hold the permission
    /// wherever nothing was changed.
    pub role: Role,
}

/// What the system ships: the entries of its defaults page on a new
/// installation, each a role named in one row. A row that is not listed
/// names nobody, which leaves it to the owners of each thing. Every one of
/// these is a row that a place changes for itself, and none is a rule.
#[must_use]
pub fn shipped() -> Vec<Shipped> {
    PageOf::Defaults(PlaceKind::System)
        .rows()
        .into_iter()
        .filter_map(|(permission, profile)| {
            let placement = match profile {
                Profile::Connected => Placement::Connected,
                Profile::Any | Profile::Managed => Placement::Managed,
            };
            permission.shipped(placement).map(|role| Shipped {
                permission,
                profile,
                role,
            })
        })
        .collect()
}

/// Reads a member that may be absent, `null` or a value, keeping the three
/// apart: absent is `None`, `null` is `Some(None)`.
fn absent_or_null<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// Names holders in every row that one of the three choices covers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NameChange {
    /// Whom to name.
    pub holders: Vec<Holder>,
    /// The choice that says in which rows.
    pub choice: Choice,
    /// Whether the holders are named in the Export rows of the same areas too, so that whoever is let
    /// read may take it away; `true` unless given. It is done as far as the giver holds those rows:
    /// the others are left out, not refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export: Option<bool>,
    /// The options of the entry in the row that takes any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Json>,
    /// When they stop holding it; one end for all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Time>,
    /// A note; one for all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// An entry to add to a row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EntryAdd {
    /// Whom to name.
    pub holder: Holder,
    /// Its options, where the permission takes any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Json>,
    /// When it ends by itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Time>,
    /// A note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A change of an entry that exists. A member that is left out stays as it
/// is; an end of `null` takes the end away.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EntryUpdate {
    /// The entry.
    pub entry_id: Id,
    /// New options.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Json>,
    /// A new end, or `null` for none.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "absent_or_null"
    )]
    #[schemars(with = "Option<Time>")]
    pub expires_at: Option<Option<Time>>,
    /// A new note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A change of one row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RowChange {
    /// The permission whose row it is, by its key.
    pub permission: String,
    /// On a defaults page, which of the two rows of a permission that has
    /// two.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<Placement>,
    /// Whether the row follows the default of the place above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inherit: Option<bool>,
    /// On a defaults page: whether the row is locked for everything inside.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locked: Option<bool>,
    /// Entries to add.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add: Option<Vec<EntryAdd>>,
    /// Entries to change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<Vec<EntryUpdate>>,
    /// Entries to remove.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove: Option<Vec<Id>>,
}

/// A change of the Owners row.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OwnersChange {
    /// Further owners: people and groups of the thing's root org.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add: Option<Vec<Holder>>,
    /// Entries of the row to take out; never the last person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove: Option<Vec<Id>>,
    /// The person who is first from now on. They are added when they are
    /// not in the row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first: Option<Id>,
}

words! {
    /// A change of a whole page at once.
    pub enum Whole ("change of a whole page") {
        /// Cut every row loose, so that the page is held by its owners and
        /// by whom it names.
        Private = "private",
        /// Remove every stored row but the Owners row, so that the page
        /// follows the defaults again throughout.
        Reset = "reset",
    }
}

/// One change of a page or of a defaults page, as a request writes it. It is
/// applied as a whole or not at all.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageChange {
    /// The short way to add people: holders named in every row a choice
    /// covers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<Vec<NameChange>>,
    /// The same said row by row. Both may be in one change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<Vec<RowChange>>,
    /// A change of the Owners row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owners: Option<OwnersChange>,
    /// A change of the whole page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub whole: Option<Whole>,
    /// The revision of the page that whoever asks has seen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<Revision>,
}

/// An entry to add, read and found fit for its row.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckedEntry {
    /// Whom to name.
    pub holder: Holder,
    /// Its options.
    pub options: Options,
    /// When it ends by itself.
    pub expires_at: Option<Time>,
    /// Its note; empty when none was given.
    pub note: String,
}

/// A change of an entry that exists, read for its row.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckedUpdate {
    /// The entry.
    pub entry: Id,
    /// New options, when they change.
    pub options: Option<Options>,
    /// A new end when it changes: `Some(None)` takes the end away.
    pub expires_at: Option<Option<Time>>,
    /// A new note, when it changes.
    pub note: Option<String>,
}

/// A change of one row, read and found fit for its page.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckedRow {
    /// The permission.
    pub permission: Permission,
    /// Which of its rows.
    pub profile: Profile,
    /// Whether it is to follow the default, when that changes.
    pub inherit: Option<bool>,
    /// Whether it is to be locked, when that changes.
    pub locked: Option<bool>,
    /// Entries to add.
    pub add: Vec<CheckedEntry>,
    /// Entries to change.
    pub update: Vec<CheckedUpdate>,
    /// Entries to remove.
    pub remove: Vec<Id>,
    /// A row that travels with a choice and is named as far as the giver may give it: the Export rows
    /// of the areas a choice covers. What the giver may not give there is left out, not refused.
    pub optional: bool,
}

/// A change of the Owners row, read and found fit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckedOwners {
    /// Further owners: each a person or a group.
    pub add: Vec<Holder>,
    /// Entries to take out.
    pub remove: Vec<Id>,
    /// The person who is first from now on.
    pub first: Option<Id>,
}

/// What a change asks of whoever makes it, beyond being able to change the
/// page at all.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Asks {
    /// It names a holder of the right to change the page, or changes the
    /// Owners row: that is for one of the thing's owners or an admin of its
    /// place. Somebody who may change the page through an entry changes the
    /// rest of it, and cannot pass that on.
    pub owner_or_admin: bool,
    /// It names a holder of the right to name somebody from outside: that
    /// is for an admin of the place and nobody else.
    pub admin: bool,
}

/// A change that can be a change of its kind of page, spelled out row by
/// row: every choice is the rows it covers, every permission is one of the
/// catalogue, and every entry fits its row.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CheckedChange {
    /// The rows to change, in the order they were asked for: first those of
    /// the choices, then those said row by row.
    pub rows: Vec<CheckedRow>,
    /// The change of the Owners row.
    pub owners: Option<CheckedOwners>,
    /// The change of the whole page.
    pub whole: Option<Whole>,
    /// The revision of the page that whoever asks has seen.
    pub expected_revision: Option<Revision>,
}

impl CheckedChange {
    /// What the change asks of whoever makes it.
    #[must_use]
    pub fn asks(&self) -> Asks {
        let mut asks = Asks {
            owner_or_admin: self.owners.is_some(),
            admin: false,
        };
        for row in self
            .rows
            .iter()
            .filter(|row| !row.add.is_empty() || !row.update.is_empty())
        {
            match row.permission.named_by() {
                NamedBy::OwnerOrAdmin => asks.owner_or_admin = true,
                NamedBy::Admin => asks.admin = true,
                NamedBy::WhoeverChangesThePage => {}
            }
        }
        asks
    }
}

struct Faults(Vec<FieldError>);

impl Faults {
    fn add(&mut self, path: impl Into<String>, message: impl Into<String>) {
        self.0.push(FieldError {
            path: path.into(),
            message: message.into(),
        });
    }

    /// Reads the options of an entry in the row of `permission`, reporting
    /// what is wrong with them under `path`.
    fn options(&mut self, path: &str, permission: Permission, options: Option<&Json>) -> Options {
        match Options::parse(permission, options.unwrap_or(&Json::Null)) {
            Ok(options) => options,
            Err(faults) => {
                for fault in faults {
                    let at = if fault.path.is_empty() {
                        path.to_owned()
                    } else {
                        format!("{path}.{}", fault.path)
                    };
                    self.add(at, fault.message);
                }
                Options::None
            }
        }
    }

    fn note(&mut self, path: &str, note: Option<&String>) {
        if note.is_some_and(|note| note.chars().count() > NOTE_MAX_CHARS) {
            self.add(path, format!("must be at most {NOTE_MAX_CHARS} characters"));
        }
    }
}

/// Says whether `change` can be a change of `page`, and spells it out.
///
/// It is refused, with every fault and the member each is about, when it
/// names a permission that page has no row for, a holder of a kind a row
/// does not take, options a permission does not allow, a choice the kind of
/// thing does not have, a lock or a profile outside a defaults page, or an
/// owner who is neither a person nor a group. Nothing stored is looked at:
/// who may make the change, a lock above, who is from outside, and the last
/// owner are decided where the pages are.
pub fn check(change: &PageChange, page: PageOf) -> Result<CheckedChange, Vec<FieldError>> {
    let mut faults = Faults(Vec::new());
    let mut rows = Vec::new();
    for (index, name) in change.name.iter().flatten().enumerate() {
        check_name(
            &format!("name[{index}]"),
            name,
            page,
            &mut rows,
            &mut faults,
        );
    }
    for (index, row) in change.rows.iter().flatten().enumerate() {
        if let Some(row) = check_row(&format!("rows[{index}]"), row, page, &mut faults) {
            rows.push(row);
        }
    }
    let owners = change
        .owners
        .as_ref()
        .map(|owners| check_owners(owners, page, &mut faults));
    if change.whole.is_some() && page == PageOf::Chat {
        faults.add(
            "whole",
            "is not for a chat: it has no page to cut loose or reset",
        );
    }
    if faults.0.is_empty() {
        Ok(CheckedChange {
            rows,
            owners,
            whole: change.whole,
            expected_revision: change.expected_revision,
        })
    } else {
        Err(faults.0)
    }
}

fn check_name(
    path: &str,
    name: &NameChange,
    page: PageOf,
    rows: &mut Vec<CheckedRow>,
    faults: &mut Faults,
) {
    let PageOf::Thing { kind, .. } = page else {
        faults.add(
            path,
            "is for the page of one thing: a choice names the rows of one kind of thing",
        );
        return;
    };
    let covered = name.choice.permissions(kind);
    if covered.is_empty() {
        faults.add(
            format!("{path}.choice"),
            format!("is no choice on a thing of kind {kind}"),
        );
        return;
    }
    for (index, holder) in name.holders.iter().enumerate() {
        if let Some(row) = covered.iter().find(|row| !page.takes(holder.kind(), **row)) {
            faults.add(
                format!("{path}.holders[{index}]"),
                format!("is a holder that the row {row} does not take"),
            );
        }
    }
    faults.note(&format!("{path}.note"), name.note.as_ref());
    let options_path = format!("{path}.options");
    let given = name.options.as_ref().filter(|options| !is_empty(options));
    if given.is_some() && !covered.iter().any(|row| row.takes_options()) {
        faults.add(
            options_path.as_str(),
            "is refused: no row of this choice takes options",
        );
    }
    for permission in covered.iter().copied() {
        let options = if permission.takes_options() {
            faults.options(&options_path, permission, given)
        } else {
            Options::None
        };
        let entry = |holder: &Holder| CheckedEntry {
            holder: *holder,
            options: options.clone(),
            expires_at: name.expires_at,
            note: name.note.clone().unwrap_or_default(),
        };
        rows.push(CheckedRow {
            permission,
            profile: Profile::Any,
            inherit: None,
            locked: None,
            add: name.holders.iter().map(entry).collect(),
            update: Vec::new(),
            remove: Vec::new(),
            optional: false,
        });
    }
    // Export is the one that travels with a choice: whoever is let read is named in the Export rows of
    // the same areas too, unless the giver unticks that. A holder that a row does not take is left out
    // of it, and what the giver may not give is dropped when the change is made.
    if name.export.unwrap_or(true) {
        for permission in name.choice.exports(kind) {
            let add: Vec<CheckedEntry> = name
                .holders
                .iter()
                .filter(|holder| page.takes(holder.kind(), permission))
                .map(|holder| CheckedEntry {
                    holder: *holder,
                    options: Options::None,
                    expires_at: name.expires_at,
                    note: name.note.clone().unwrap_or_default(),
                })
                .collect();
            rows.push(CheckedRow {
                permission,
                profile: Profile::Any,
                inherit: None,
                locked: None,
                add,
                update: Vec::new(),
                remove: Vec::new(),
                optional: true,
            });
        }
    }
}

fn is_empty(options: &Json) -> bool {
    match options {
        Json::Null => true,
        Json::Object(members) => members.is_empty(),
        _ => false,
    }
}

fn check_row(path: &str, row: &RowChange, page: PageOf, faults: &mut Faults) -> Option<CheckedRow> {
    let Ok(permission) = row.permission.parse::<Permission>() else {
        faults.add(format!("{path}.permission"), "is no permission");
        return None;
    };
    if permission == Permission::AccessOwn {
        faults.add(
            format!("{path}.permission"),
            "is the Owners row, which is changed with owners",
        );
        return None;
    }
    if !page.has_row(permission) {
        faults.add(format!("{path}.permission"), "is no row of this page");
        return None;
    }
    let defaults = matches!(page, PageOf::Defaults(_));
    let profile = match (defaults && permission.profiles(), row.profile) {
        (true, Some(placement)) => placement.into(),
        (true, None) => {
            let message = "is needed: this row is one for a managed environment and one for a person's machine";
            faults.add(format!("{path}.profile"), message);
            Profile::Any
        }
        (false, Some(_)) if defaults => {
            faults.add(
                format!("{path}.profile"),
                "is refused: this permission has one row",
            );
            Profile::Any
        }
        (false, Some(_)) => {
            faults.add(format!("{path}.profile"), "is said on a defaults page only");
            Profile::Any
        }
        (false, None) => Profile::Any,
    };
    if row.locked.is_some() && !defaults {
        faults.add(
            format!("{path}.locked"),
            "is refused: a row is locked on a defaults page",
        );
    }
    if row.inherit.is_some() && page == PageOf::Chat {
        faults.add(
            format!("{path}.inherit"),
            "is refused: a chat follows no defaults",
        );
    }
    let mut add = Vec::new();
    for (index, entry) in row.add.iter().flatten().enumerate() {
        let at = format!("{path}.add[{index}]");
        if !page.takes(entry.holder.kind(), permission) {
            faults.add(
                format!("{at}.holder"),
                "is a holder that this row does not take",
            );
        }
        faults.note(&format!("{at}.note"), entry.note.as_ref());
        add.push(CheckedEntry {
            holder: entry.holder,
            options: faults.options(&format!("{at}.options"), permission, entry.options.as_ref()),
            expires_at: entry.expires_at,
            note: entry.note.clone().unwrap_or_default(),
        });
    }
    let mut update = Vec::new();
    for (index, entry) in row.update.iter().flatten().enumerate() {
        let at = format!("{path}.update[{index}]");
        faults.note(&format!("{at}.note"), entry.note.as_ref());
        let options = entry
            .options
            .as_ref()
            .map(|options| faults.options(&format!("{at}.options"), permission, Some(options)));
        update.push(CheckedUpdate {
            entry: entry.entry_id,
            options,
            expires_at: entry.expires_at,
            note: entry.note.clone(),
        });
    }
    Some(CheckedRow {
        permission,
        profile,
        inherit: row.inherit,
        locked: row.locked,
        add,
        update,
        remove: row.remove.clone().unwrap_or_default(),
        optional: false,
    })
}

fn check_owners(owners: &OwnersChange, page: PageOf, faults: &mut Faults) -> CheckedOwners {
    if !matches!(page, PageOf::Thing { .. }) {
        faults.add(
            "owners",
            "is for the page of a thing: only a thing has owners",
        );
    }
    for (index, holder) in owners.add.iter().flatten().enumerate() {
        if !HolderKind::fits_defaults(holder.kind(), Permission::AccessOwn) {
            faults.add(
                format!("owners.add[{index}]"),
                "must be a person or a group",
            );
        }
    }
    CheckedOwners {
        add: owners.add.clone().unwrap_or_default(),
        remove: owners.remove.clone().unwrap_or_default(),
        first: owners.first,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::new_id;
    use serde_json::json;

    use Permission as P;

    fn at(text: &str) -> Time {
        text.parse().unwrap()
    }

    fn now() -> Time {
        at("2026-10-08T12:00:00Z")
    }

    fn entry(holder: Holder) -> Entry {
        Entry {
            id: new_id(),
            holder,
            options: json!({}),
            expires_at: None,
            note: String::new(),
            source: EntrySource::Page,
            created_by: Some(new_id()),
            created_at: at("2026-10-01T00:00:00Z"),
            removed_at: None,
            revision: 1,
        }
    }

    fn role(role: Role) -> Holder {
        Holder::Role { role, at: None }
    }

    fn row(permission: Permission, holders: &[Holder]) -> Row {
        Row {
            entries: holders.iter().copied().map(entry).collect(),
            ..Row::following(permission, Profile::Any)
        }
    }

    fn cut(permission: Permission, holders: &[Holder]) -> Row {
        Row {
            inherit: false,
            ..row(permission, holders)
        }
    }

    fn locked(permission: Permission, holders: &[Holder]) -> Row {
        Row {
            locked: true,
            ..row(permission, holders)
        }
    }

    fn named(chain: &Chain<'_>) -> Vec<Holder> {
        chain
            .holders(now())
            .iter()
            .map(|named| named.entry.holder)
            .collect()
    }

    fn thread() -> PageOf {
        PageOf::Thing {
            kind: ObjectKind::Thread,
            of_system: false,
        }
    }

    fn paths(faults: &[FieldError]) -> Vec<&str> {
        faults.iter().map(|fault| fault.path.as_str()).collect()
    }

    #[test]
    fn a_new_thing_is_held_by_whom_the_defaults_above_it_name() {
        let (topic, project) = (new_id(), new_id());
        let project_defaults = row(P::ThreadView, &[role(Role::Interactor)]);
        let system_defaults = row(P::ThreadView, &[role(Role::Viewer)]);
        let chain = Chain::of(None)
            .under(topic.into(), None)
            .under(project.into(), Some(&project_defaults))
            .under(PlaceRef::System, Some(&system_defaults));
        assert_eq!(named(&chain), [role(Role::Interactor), role(Role::Viewer)]);
        let from: Vec<_> = chain
            .holders(now())
            .iter()
            .map(|named| named.from)
            .collect();
        assert_eq!(
            from,
            [
                Origin::Defaults(project.into()),
                Origin::Defaults(PlaceRef::System)
            ]
        );
        assert!(!chain.cut_below(PlaceRef::System));
    }

    #[test]
    fn adding_somebody_to_a_row_leaves_it_following() {
        let chen = Holder::User { id: new_id() };
        let own = row(P::ThreadInteract, &[chen]);
        let defaults = row(P::ThreadInteract, &[role(Role::Interactor)]);
        let chain = Chain::of(Some(&own)).under(PlaceRef::System, Some(&defaults));
        assert_eq!(named(&chain), [chen, role(Role::Interactor)]);
        assert_eq!(chain.inherited(now()).map(|named| named.len()), Some(1));
    }

    #[test]
    fn a_row_that_was_cut_loose_is_held_by_whom_it_names_and_nobody_else() {
        let (dana, topic) = (Holder::User { id: new_id() }, new_id());
        let own = cut(P::ThreadView, &[dana]);
        let defaults = row(P::ThreadView, &[role(Role::Viewer)]);
        let chain = Chain::of(Some(&own))
            .under(topic.into(), None)
            .under(PlaceRef::System, Some(&defaults));
        assert_eq!(named(&chain), [dana]);
        assert_eq!(chain.inherited(now()), None);
        let empty = cut(P::ThreadView, &[]);
        let chain = Chain::of(Some(&empty)).under(PlaceRef::System, Some(&defaults));
        assert_eq!(
            named(&chain),
            [],
            "a row that names nobody and follows nothing is its owners' alone"
        );
    }

    #[test]
    fn a_default_that_was_cut_loose_stops_what_is_above_it() {
        let (topic, org) = (new_id(), new_id());
        let topic_defaults = cut(P::ThreadView, &[role(Role::Developer)]);
        let org_defaults = row(P::ThreadView, &[role(Role::Viewer)]);
        let chain = Chain::of(None)
            .under(topic.into(), Some(&topic_defaults))
            .under(org.into(), Some(&org_defaults));
        assert_eq!(named(&chain), [role(Role::Developer)]);
    }

    #[test]
    fn a_lock_above_decides_a_row_whatever_the_page_says() {
        let (org, project, eve) = (new_id(), new_id(), Holder::User { id: new_id() });
        let org_defaults = locked(P::TerminalManage, &[role(Role::Admin)]);
        let project_defaults = row(P::TerminalManage, &[role(Role::Developer)]);
        let above = |own| {
            Chain::of(own)
                .under(project.into(), Some(&project_defaults))
                .under(org.into(), Some(&org_defaults))
                .under(PlaceRef::System, None)
        };
        let added = row(P::TerminalManage, &[eve]);
        let loose = cut(P::TerminalManage, &[eve]);
        for own in [None, Some(&added), Some(&loose)] {
            let chain = above(own);
            assert_eq!(
                named(&chain),
                [role(Role::Admin)],
                "every page below gives the same holders"
            );
            assert_eq!(chain.locked_by(), Some(org.into()));
            assert_eq!(chain.locked_above(), Some(org.into()));
            assert!(chain.inherited(now()).is_some());
        }
    }

    #[test]
    fn of_two_locks_the_one_further_up_decides() {
        let (org, project) = (new_id(), new_id());
        let org_defaults = locked(P::FilesManage, &[role(Role::Admin)]);
        let project_defaults = locked(P::FilesManage, &[role(Role::Interactor)]);
        let chain = Chain::of(None)
            .under(project.into(), Some(&project_defaults))
            .under(org.into(), Some(&org_defaults));
        assert_eq!(chain.locked_by(), Some(org.into()));
        assert_eq!(named(&chain), [role(Role::Admin)]);
    }

    #[test]
    fn a_locked_row_still_follows_what_is_above_it() {
        let org = new_id();
        let org_defaults = locked(P::ThreadView, &[role(Role::Developer)]);
        let system_defaults = row(P::ThreadView, &[role(Role::Viewer)]);
        let own = cut(P::ThreadView, &[]);
        let chain = Chain::of(Some(&own))
            .under(org.into(), Some(&org_defaults))
            .under(PlaceRef::System, Some(&system_defaults));
        assert_eq!(named(&chain), [role(Role::Developer), role(Role::Viewer)]);
    }

    #[test]
    fn a_row_is_changed_where_its_lock_is() {
        let (org, project) = (new_id(), new_id());
        let org_defaults = locked(P::ChatView, &[role(Role::Viewer)]);
        let here =
            Chain::of_defaults(org.into(), Some(&org_defaults)).under(PlaceRef::System, None);
        assert_eq!(here.locked_by(), Some(org.into()));
        assert_eq!(
            here.locked_above(),
            None,
            "the page that set the lock changes the row"
        );
        let below = Chain::of_defaults(project.into(), None).under(org.into(), Some(&org_defaults));
        assert_eq!(below.locked_above(), Some(org.into()));
    }

    #[test]
    fn a_lock_is_set_on_a_defaults_page_and_nowhere_else() {
        let own = locked(P::ChatView, &[role(Role::Admin)]);
        let defaults = row(P::ChatView, &[role(Role::Viewer)]);
        let chain = Chain::of(Some(&own)).under(PlaceRef::System, Some(&defaults));
        assert_eq!(chain.locked_by(), None);
        assert_eq!(named(&chain), [role(Role::Admin), role(Role::Viewer)]);
    }

    #[test]
    fn an_entry_that_ended_or_was_removed_counts_for_nothing() {
        let mut ended = entry(role(Role::Developer));
        ended.expires_at = Some(at("2026-10-08T11:00:00Z"));
        let mut removed = entry(role(Role::Interactor));
        removed.removed_at = Some(at("2026-10-07T00:00:00Z"));
        let mut lasting = entry(role(Role::Viewer));
        lasting.expires_at = Some(at("2026-10-15T00:00:00Z"));
        let own = Row {
            entries: vec![ended, removed, lasting],
            ..Row::following(P::ChatView, Profile::Any)
        };
        assert_eq!(named(&Chain::of(Some(&own))), [role(Role::Viewer)]);
    }

    #[test]
    fn a_document_follows_the_row_of_its_node() {
        let (thread, topic, bo) = (new_id(), new_id(), Holder::User { id: new_id() });
        let node_row = row(P::DocView, &[bo]);
        let defaults = row(P::DocView, &[role(Role::Viewer)]);
        let chain = Chain::of(None)
            .beside(thread, Some(&node_row))
            .under(topic.into(), Some(&defaults));
        assert_eq!(named(&chain), [bo, role(Role::Viewer)]);
        assert_eq!(chain.holders(now())[0].from, Origin::Node(thread));

        let private_node = cut(P::DocView, &[bo]);
        let beside_private = Chain::of(None)
            .beside(thread, Some(&private_node))
            .under(topic.into(), Some(&defaults));
        assert_eq!(
            named(&beside_private),
            [bo],
            "beside a restricted thread it is as closed as the thread"
        );

        let private_document = cut(P::DocView, &[]);
        let chain = Chain::of(Some(&private_document))
            .beside(thread, Some(&node_row))
            .under(topic.into(), Some(&defaults));
        assert_eq!(named(&chain), [], "private until shared");
    }

    #[test]
    fn a_lock_on_a_nodes_document_row_holds_its_documents_too() {
        let (thread, org) = (new_id(), new_id());
        let private_document = cut(P::DocEdit, &[Holder::User { id: new_id() }]);
        let org_defaults = locked(P::DocEdit, &[role(Role::Interactor)]);
        let chain = Chain::of(Some(&private_document))
            .beside(thread, None)
            .under(org.into(), Some(&org_defaults));
        assert_eq!(named(&chain), [role(Role::Interactor)]);
        assert_eq!(chain.locked_above(), Some(org.into()));
    }

    #[test]
    fn the_defaults_of_the_system_follow_nothing() {
        let system_defaults = row(P::ChatView, &[role(Role::Viewer)]);
        let chain = Chain::of_defaults(PlaceRef::System, Some(&system_defaults));
        assert_eq!(named(&chain), [role(Role::Viewer)]);
        assert_eq!(chain.inherited(now()), None);
    }

    #[test]
    fn a_node_is_restricted_when_its_view_is_cut_loose_below_its_root_org() {
        let (topic, project, root) = (new_id(), new_id(), new_id());
        let chain = |own, topic_defaults, root_defaults| {
            Chain::of(own)
                .under(topic.into(), topic_defaults)
                .under(project.into(), None)
                .under(root.into(), root_defaults)
                .under(PlaceRef::System, None)
        };
        let loose = cut(P::ThreadView, &[]);
        assert!(!chain(None, None, None).cut_below(root.into()));
        assert!(
            chain(Some(&loose), None, None).cut_below(root.into()),
            "its own row"
        );
        assert!(
            chain(None, Some(&loose), None).cut_below(root.into()),
            "a defaults row on the way"
        );
        assert!(
            !chain(None, None, Some(&loose)).cut_below(root.into()),
            "what the root org follows is its own"
        );
        let locked_at_root = locked(P::ThreadView, &[role(Role::Viewer)]);
        assert!(
            !chain(Some(&loose), None, Some(&locked_at_root)).cut_below(root.into()),
            "a lock decides"
        );
        let root_itself = Chain::of(Some(&loose)).under(PlaceRef::System, None);
        assert!(root_itself.cut_below(root.into()));
        assert_eq!(PageState::of(true, true), PageState::Restricted);
        assert_eq!(PageState::of(true, false), PageState::Custom);
        assert_eq!(PageState::of(false, false), PageState::Follows);
    }

    #[test]
    fn a_resource_opened_to_the_work_in_a_place_serves_the_threads_under_it() {
        let (root, project, topic, thread, elsewhere) =
            (new_id(), new_id(), new_id(), new_id(), new_id());
        let path = [root, project, topic, thread];
        assert!(Holder::Node { id: project }.is_work_on(&path));
        assert!(Holder::Node { id: thread }.is_work_on(&path));
        assert!(!Holder::Node { id: elsewhere }.is_work_on(&path));
        assert!(
            !Holder::Agents { id: project }.is_work_on(&path),
            "reading what was shared runs nothing"
        );
        assert!(Holder::Agents { id: topic }.is_agents_on(&path));
        assert!(!Holder::Agents { id: elsewhere }.is_agents_on(&path));
        assert!(!Holder::User { id: thread }.is_work_on(&path));
    }

    #[test]
    fn a_holder_is_kept_as_its_kind_what_it_names_and_its_role() {
        let place = new_id();
        let holders = [
            role(Role::Developer),
            Holder::Role {
                role: Role::Viewer,
                at: Some(place),
            },
            Holder::Group { id: place },
            Holder::User { id: place },
            Holder::Node { id: place },
            Holder::Agents { id: place },
            Holder::Everyone,
        ];
        for holder in holders {
            let (kind, id, held) = holder.stored();
            assert_eq!(Holder::from_stored(kind, id, held), Some(holder));
        }
        assert_eq!(
            role(Role::Developer).stored(),
            (HolderKind::Role, None, Some(Role::Developer))
        );
        assert_eq!(Holder::from_stored(HolderKind::Role, None, None), None);
        assert_eq!(Holder::from_stored(HolderKind::User, None, None), None);
        assert_eq!(
            Holder::from_stored(HolderKind::User, Some(place), Some(Role::Admin)),
            None
        );
        assert_eq!(
            Holder::from_stored(HolderKind::Everyone, Some(place), None),
            None
        );
    }

    #[test]
    fn a_holder_is_a_person_a_group_of_theirs_or_a_role_they_hold_at_least() {
        let (me, group, project) = (new_id(), new_id(), new_id());
        let groups = [group];
        let person = Person {
            id: me,
            groups: &groups,
            member_of: &[],
        };
        let developer_here = |place: Option<Id>| place.is_none().then_some(Role::Developer);
        assert!(Holder::User { id: me }.matches(&person, developer_here));
        assert!(!Holder::User { id: new_id() }.matches(&person, developer_here));
        assert!(Holder::Group { id: group }.matches(&person, developer_here));
        assert!(role(Role::Interactor).matches(&person, developer_here));
        assert!(role(Role::Developer).matches(&person, developer_here));
        assert!(!role(Role::Admin).matches(&person, developer_here));
        let elsewhere = Holder::Role {
            role: Role::Viewer,
            at: Some(project),
        };
        assert!(
            !elsewhere.matches(&person, developer_here),
            "they hold no role in that other place"
        );
        assert!(Holder::Everyone.matches(&person, developer_here));
        assert!(!Holder::Node { id: project }.matches(&person, developer_here));
        assert!(!Holder::Agents { id: project }.matches(&person, developer_here));
    }

    #[test]
    fn the_system_ships_roles_and_never_a_person() {
        let shipped = shipped();
        let of = |permission, profile| {
            shipped
                .iter()
                .find(|row| row.permission == permission && row.profile == profile)
                .map(|row| row.role)
        };
        assert_eq!(of(P::ChatInteract, Profile::Any), Some(Role::Viewer));
        assert_eq!(
            of(P::ThreadInteract, Profile::Managed),
            Some(Role::Interactor)
        );
        assert_eq!(of(P::ThreadInteract, Profile::Connected), None);
        assert_eq!(of(P::ThreadView, Profile::Connected), Some(Role::Viewer));
        assert_eq!(of(P::ProjectManage, Profile::Any), Some(Role::Admin));
        assert_eq!(of(P::ProviderUse, Profile::Any), None);
        // The four doors of an environment, in both placements.
        for view in [P::FilesView, P::TerminalView, P::BrowserView, P::VncView] {
            assert_eq!(of(view, Profile::Managed), Some(Role::Viewer), "{view}");
            assert_eq!(of(view, Profile::Connected), Some(Role::Viewer), "{view}");
        }
        for manage in [P::FilesManage, P::TerminalManage] {
            assert_eq!(of(manage, Profile::Managed), Some(Role::Developer));
            assert_eq!(of(manage, Profile::Connected), None);
        }
        for manage in [P::BrowserManage, P::VncManage] {
            assert_eq!(of(manage, Profile::Managed), None, "{manage}");
            assert_eq!(of(manage, Profile::Connected), None, "{manage}");
        }
        assert!(
            shipped
                .iter()
                .all(|row| row.permission != P::AccessOwn && row.permission != P::AccessManage)
        );
        assert!(
            shipped
                .iter()
                .all(|row| row.permission.profiles() != (row.profile == Profile::Any))
        );
    }

    #[test]
    fn a_defaults_page_has_two_rows_where_the_default_depends_on_where_work_runs() {
        let topic = PageOf::Defaults(PlaceKind::Topic).rows();
        assert!(topic.contains(&(P::ThreadManage, Profile::Managed)));
        assert!(topic.contains(&(P::ThreadManage, Profile::Connected)));
        assert!(topic.contains(&(P::EnvironmentView, Profile::Connected)));
        assert!(topic.contains(&(P::ChatView, Profile::Any)));
        assert!(
            !topic
                .iter()
                .any(|(permission, _)| *permission == P::TopicView),
            "a topic holds no topic"
        );
        assert!(
            !topic
                .iter()
                .any(|(permission, _)| *permission == P::ProviderUse)
        );
        assert!(
            !topic
                .iter()
                .any(|(permission, _)| *permission == P::AccessOwn)
        );
        let org = PageOf::Defaults(PlaceKind::Org).rows();
        assert!(org.contains(&(P::ProviderUse, Profile::Any)));
        assert!(
            org.contains(&(P::OrgView, Profile::Any)),
            "orgs lie inside orgs"
        );
        let thread_page = thread().rows();
        assert!(
            thread_page
                .iter()
                .all(|(_, profile)| *profile == Profile::Any)
        );
        assert!(thread_page.contains(&(P::ThreadView, Profile::Any)));
        assert!(
            !thread_page
                .iter()
                .any(|(permission, _)| permission.area().is_door()),
            "a thread's page holds none of what is reached in its environment"
        );
    }

    #[test]
    fn the_four_doors_are_rows_of_an_environments_page_and_have_two_rows_on_every_defaults_page() {
        let environment = PageOf::Thing {
            kind: ObjectKind::Environment,
            of_system: false,
        }
        .rows();
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
            assert!(environment.contains(&(door, Profile::Any)), "{door}");
            for place in [PlaceKind::Topic, PlaceKind::Project, PlaceKind::Org] {
                let defaults = PageOf::Defaults(place).rows();
                assert!(defaults.contains(&(door, Profile::Managed)), "{door}");
                assert!(defaults.contains(&(door, Profile::Connected)), "{door}");
                assert!(!defaults.contains(&(door, Profile::Any)), "{door}");
            }
        }
    }

    #[test]
    fn a_change_is_read_as_a_request_writes_it() {
        let (chen, entry_id) = (new_id(), new_id());
        let written = json!({
            "name": [{ "holders": [{ "kind": "user", "id": chen }], "choice": "full_interact",
                       "expires_at": "2026-11-01T00:00:00.000Z" }],
            "rows": [{ "permission": "thread.fork", "inherit": false,
                       "add": [{ "holder": { "kind": "role", "role": "developer" }, "note": "the team" }],
                       "update": [{ "entry_id": entry_id, "expires_at": null }],
                       "remove": [entry_id] }],
            "owners": { "first": chen },
            "expected_revision": 7
        });
        let change: PageChange = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(
            change.rows.as_ref().unwrap()[0].update.as_ref().unwrap()[0].expires_at,
            Some(None)
        );
        assert_eq!(serde_json::to_value(&change).unwrap(), written);
        assert!(
            serde_json::from_value::<PageChange>(json!({ "rows": [], "everything": true }))
                .is_err()
        );
        let foreign = json!({ "kind": "role", "role": "viewer", "at": chen });
        assert_eq!(
            serde_json::from_value::<Holder>(foreign).unwrap(),
            Holder::Role {
                role: Role::Viewer,
                at: Some(chen)
            }
        );
        assert_eq!(
            serde_json::to_value(Holder::Everyone).unwrap(),
            json!({ "kind": "everyone" })
        );
    }

    #[test]
    fn a_choice_names_its_holders_in_every_row_it_covers() {
        let chen = Holder::User { id: new_id() };
        let change = PageChange {
            name: Some(vec![NameChange {
                holders: vec![chen],
                choice: Choice::FullInteract,
                export: None,
                options: None,
                expires_at: Some(at("2026-11-01T00:00:00Z")),
                note: Some("for the release".into()),
            }]),
            ..PageChange::default()
        };
        let checked = check(&change, thread()).unwrap();
        let rows: Vec<_> = checked
            .rows
            .iter()
            .filter(|row| !row.optional)
            .map(|row| row.permission)
            .collect();
        assert_eq!(
            rows,
            [P::ChatInteract, P::DocEdit, P::ThreadInteract],
            "the session of the thread, and nothing of its environment"
        );
        for row in &checked.rows {
            assert_eq!(row.add.len(), 1);
            assert_eq!(row.add[0].holder, chen);
            assert_eq!(row.add[0].expires_at, Some(at("2026-11-01T00:00:00Z")));
            assert_eq!(row.add[0].note, "for the release");
            assert_eq!(row.inherit, None, "adding somebody leaves a row following");
        }
        assert_eq!(checked.asks(), Asks::default());
    }

    #[test]
    fn can_view_an_environment_names_the_holder_in_it_and_in_the_view_row_of_each_door() {
        let chen = Holder::User { id: new_id() };
        let change = |choice| PageChange {
            name: Some(vec![NameChange {
                holders: vec![chen],
                choice,
                export: None,
                options: None,
                expires_at: None,
                note: None,
            }]),
            ..PageChange::default()
        };
        let environment = PageOf::Thing {
            kind: ObjectKind::Environment,
            of_system: false,
        };
        let looking = check(&change(Choice::FullView), environment).unwrap();
        let rows: Vec<_> = looking.rows.iter().map(|row| row.permission).collect();
        assert_eq!(
            rows,
            [
                P::EnvironmentView,
                P::FilesView,
                P::TerminalView,
                P::BrowserView,
                P::VncView,
                P::PortsView
            ]
        );
        let managing = check(&change(Choice::FullManage), environment).unwrap();
        let rows: Vec<_> = managing.rows.iter().map(|row| row.permission).collect();
        assert_eq!(
            rows,
            [P::EnvironmentManage],
            "the Manage of the four doors is held through it, and nobody is named in them"
        );
    }

    #[test]
    fn a_choice_names_its_holders_in_the_export_rows_of_the_same_areas_unless_the_giver_says_not() {
        let chen = Holder::User { id: new_id() };
        let change = |export| PageChange {
            name: Some(vec![NameChange {
                holders: vec![chen],
                choice: Choice::FullView,
                export,
                options: None,
                expires_at: None,
                note: None,
            }]),
            ..PageChange::default()
        };
        for asked in [None, Some(true)] {
            let checked = check(&change(asked), thread()).unwrap();
            let travelling: Vec<_> = checked
                .rows
                .iter()
                .filter(|row| row.optional)
                .map(|row| row.permission)
                .collect();
            assert_eq!(
                travelling,
                [P::ThreadExport, P::ChatExport, P::DocExport],
                "named with Can view, a holder may take it away as shipped"
            );
            assert!(
                checked.rows.iter().filter(|row| row.optional).all(|row| {
                    row.add.len() == 1 && row.add[0].holder == chen && row.profile == Profile::Any
                }),
                "the same holder, in the row of the same page"
            );
        }
        let ticked_off = check(&change(Some(false)), thread()).unwrap();
        assert!(
            ticked_off.rows.iter().all(|row| !row.optional),
            "unticked: only the rows of the choice"
        );
        assert_eq!(ticked_off.rows.len(), 3);
        let provider = PageOf::Thing {
            kind: ObjectKind::Provider,
            of_system: false,
        };
        assert!(
            check(&change(None), provider)
                .unwrap()
                .rows
                .iter()
                .all(|row| !row.optional),
            "a resource is not taken away as a file"
        );
        let on_a_document = PageOf::Thing {
            kind: ObjectKind::Document,
            of_system: false,
        };
        let rows = check(&change(None), on_a_document).unwrap().rows;
        assert_eq!(
            rows.iter().map(|row| row.permission).collect::<Vec<_>>(),
            [P::DocView, P::DocExport]
        );
    }

    #[test]
    fn an_environment_offers_use_between_looking_and_managing() {
        let change = PageChange {
            name: Some(vec![NameChange {
                holders: vec![Holder::User { id: new_id() }],
                choice: Choice::FullInteract,
                export: None,
                options: None,
                expires_at: None,
                note: None,
            }]),
            ..PageChange::default()
        };
        let environment = PageOf::Thing {
            kind: ObjectKind::Environment,
            of_system: false,
        };
        assert_eq!(
            check(&change, environment).unwrap().rows[0].permission,
            P::EnvironmentUse
        );
        let provider = PageOf::Thing {
            kind: ObjectKind::Provider,
            of_system: false,
        };
        assert_eq!(
            check(&change, provider).unwrap().rows[0].permission,
            P::ProviderUse
        );
        assert_eq!(
            paths(&check(&change, PageOf::Defaults(PlaceKind::Org)).unwrap_err()),
            ["name[0]"]
        );
    }

    #[test]
    fn the_options_of_a_choice_go_to_the_row_that_takes_them() {
        let name = |choice, options| PageChange {
            name: Some(vec![NameChange {
                holders: vec![Holder::Node { id: new_id() }],
                choice,
                export: None,
                options: Some(options),
                expires_at: None,
                note: None,
            }]),
            ..PageChange::default()
        };
        let provider = PageOf::Thing {
            kind: ObjectKind::Provider,
            of_system: false,
        };
        let checked = check(
            &name(Choice::FullInteract, json!({ "models": ["gpt-x"] })),
            provider,
        )
        .unwrap();
        let Options::ProviderUse(options) = &checked.rows[0].add[0].options else {
            panic!("the use of a provider takes options");
        };
        assert!(options.allows_model("gpt-x") && !options.allows_model("gpt-y"));
        let faults = check(
            &name(Choice::FullInteract, json!({ "budget": 1 })),
            provider,
        )
        .unwrap_err();
        assert_eq!(paths(&faults), ["name[0].options.budget"]);
        let faults = check(
            &name(Choice::FullManage, json!({ "models": ["gpt-x"] })),
            provider,
        )
        .unwrap_err();
        assert_eq!(
            paths(&faults),
            ["name[0].holders[0]", "name[0].options"],
            "the work in a place is no manager"
        );
    }

    fn one_row(row: RowChange) -> PageChange {
        PageChange {
            rows: Some(vec![row]),
            ..PageChange::default()
        }
    }

    fn row_of(permission: &str) -> RowChange {
        RowChange {
            permission: permission.to_owned(),
            profile: None,
            inherit: None,
            locked: None,
            add: None,
            update: None,
            remove: None,
        }
    }

    fn adding(permission: &str, holder: Holder) -> PageChange {
        let add = EntryAdd {
            holder,
            options: None,
            expires_at: None,
            note: None,
        };
        one_row(RowChange {
            add: Some(vec![add]),
            ..row_of(permission)
        })
    }

    #[test]
    fn a_row_names_a_permission_that_exists_on_that_kind_of_thing() {
        assert!(check(&one_row(row_of("thread.fork")), thread()).is_ok());
        for wrong in [
            "topic.manage",
            "provider.use",
            "thread.read",
            "node.view",
            "access.own",
            "files.view",
            "terminal.manage",
            "browser.view",
            "vnc.manage",
        ] {
            let faults = check(&one_row(row_of(wrong)), thread()).unwrap_err();
            assert_eq!(paths(&faults), ["rows[0].permission"], "{wrong}");
        }
        let environment = PageOf::Thing {
            kind: ObjectKind::Environment,
            of_system: false,
        };
        for door in [
            "files.view",
            "files.manage",
            "terminal.view",
            "terminal.manage",
        ] {
            assert!(check(&one_row(row_of(door)), environment).is_ok(), "{door}");
        }
        for door in ["browser.view", "browser.manage", "vnc.view", "vnc.manage"] {
            assert!(check(&one_row(row_of(door)), environment).is_ok(), "{door}");
        }
        assert!(check(&one_row(row_of("thread.view")), environment).is_err());
        let document = PageOf::Thing {
            kind: ObjectKind::Document,
            of_system: false,
        };
        assert!(check(&one_row(row_of("doc.comment")), document).is_ok());
        assert!(
            check(&one_row(row_of("doc.create")), document).is_err(),
            "a document holds no document"
        );
        assert!(
            check(
                &one_row(row_of("project.view")),
                PageOf::Defaults(PlaceKind::Project)
            )
            .is_err()
        );
        assert!(
            check(
                &one_row(row_of("topic.view")),
                PageOf::Defaults(PlaceKind::Project)
            )
            .is_ok()
        );
    }

    #[test]
    fn a_row_takes_the_holders_of_its_kind_and_no_others() {
        let place = new_id();
        let agents = Holder::Agents { id: place };
        assert!(check(&adding("doc.edit", agents), thread()).is_ok());
        let faults = check(&adding("doc.create", agents), thread()).unwrap_err();
        assert_eq!(
            paths(&faults),
            ["rows[0].add[0].holder"],
            "agents make documents by editing a place's"
        );
        let environment = PageOf::Thing {
            kind: ObjectKind::Environment,
            of_system: false,
        };
        let faults = check(&adding("files.view", agents), environment).unwrap_err();
        assert_eq!(
            paths(&faults),
            ["rows[0].add[0].holder"],
            "an agent reaches an environment by the thread it runs in, and is named in no door"
        );
        let faults = check(&adding("files.view", agents), thread()).unwrap_err();
        assert_eq!(
            paths(&faults),
            ["rows[0].permission"],
            "and a thread's page has no such row"
        );

        let provider = PageOf::Thing {
            kind: ObjectKind::Provider,
            of_system: false,
        };
        let work = Holder::Node { id: place };
        assert!(check(&adding("provider.use", work), provider).is_ok());
        assert!(check(&adding("provider.manage", work), provider).is_err());

        let member_host = PageOf::Thing {
            kind: ObjectKind::Host,
            of_system: false,
        };
        let system_host = PageOf::Thing {
            kind: ObjectKind::Host,
            of_system: true,
        };
        assert!(check(&adding("host.use", Holder::Everyone), system_host).is_ok());
        assert!(check(&adding("host.use", Holder::Everyone), member_host).is_err());
        assert!(
            check(
                &adding("host.use", Holder::Everyone),
                PageOf::Defaults(PlaceKind::Org)
            )
            .is_err()
        );
        let other_orgs_developers = Holder::Role {
            role: Role::Developer,
            at: Some(place),
        };
        assert!(check(&adding("thread.interact", other_orgs_developers), thread()).is_ok());
    }

    #[test]
    fn options_are_taken_where_the_permission_allows_them() {
        let provider = PageOf::Thing {
            kind: ObjectKind::Provider,
            of_system: false,
        };
        let with = |permission: &str, options| {
            let add = EntryAdd {
                holder: role(Role::Viewer),
                options: Some(options),
                expires_at: None,
                note: None,
            };
            one_row(RowChange {
                add: Some(vec![add]),
                ..row_of(permission)
            })
        };
        let checked = check(
            &with("provider.use", json!({ "efforts": ["high"] })),
            provider,
        )
        .unwrap();
        assert!(matches!(
            checked.rows[0].add[0].options,
            Options::ProviderUse(_)
        ));
        let faults =
            check(&with("provider.use", json!({ "speed": "fast" })), provider).unwrap_err();
        assert_eq!(paths(&faults), ["rows[0].add[0].options.speed"]);
        let faults =
            check(&with("provider.view", json!({ "models": ["m"] })), provider).unwrap_err();
        assert_eq!(paths(&faults), ["rows[0].add[0].options.models"]);
        let faults = check(&with("provider.use", json!(["m"])), provider).unwrap_err();
        assert_eq!(paths(&faults), ["rows[0].add[0].options"]);
        assert!(check(&with("provider.view", json!({})), provider).is_ok());
    }

    #[test]
    fn a_lock_and_a_profile_are_said_on_a_defaults_page() {
        let lock = RowChange {
            locked: Some(true),
            ..row_of("chat.view")
        };
        assert_eq!(
            paths(&check(&one_row(lock.clone()), thread()).unwrap_err()),
            ["rows[0].locked"]
        );
        let checked = check(&one_row(lock), PageOf::Defaults(PlaceKind::Org)).unwrap();
        assert_eq!(
            (checked.rows[0].locked, checked.rows[0].profile),
            (Some(true), Profile::Any)
        );

        let connected = RowChange {
            profile: Some(Placement::Connected),
            ..row_of("thread.interact")
        };
        assert_eq!(
            paths(&check(&one_row(connected.clone()), thread()).unwrap_err()),
            ["rows[0].profile"]
        );
        let checked = check(&one_row(connected), PageOf::Defaults(PlaceKind::Topic)).unwrap();
        assert_eq!(checked.rows[0].profile, Profile::Connected);

        let unsaid = check(
            &one_row(row_of("thread.interact")),
            PageOf::Defaults(PlaceKind::Topic),
        )
        .unwrap_err();
        assert_eq!(
            paths(&unsaid),
            ["rows[0].profile"],
            "which of its two rows has to be said"
        );
        let one = RowChange {
            profile: Some(Placement::Managed),
            ..row_of("chat.view")
        };
        assert_eq!(
            paths(&check(&one_row(one), PageOf::Defaults(PlaceKind::Topic)).unwrap_err()),
            ["rows[0].profile"]
        );
    }

    #[test]
    fn an_owner_is_a_person_or_a_group_and_only_a_thing_has_owners() {
        let owners = |add| PageChange {
            owners: Some(OwnersChange {
                add: Some(add),
                remove: None,
                first: None,
            }),
            ..PageChange::default()
        };
        let people = vec![
            Holder::User { id: new_id() },
            Holder::Group { id: new_id() },
        ];
        let checked = check(&owners(people.clone()), thread()).unwrap();
        assert_eq!(
            checked.owners.as_ref().map(|owners| owners.add.len()),
            Some(2)
        );
        let faults =
            check(&owners(vec![role(Role::Admin), Holder::Everyone]), thread()).unwrap_err();
        assert_eq!(paths(&faults), ["owners.add[0]", "owners.add[1]"]);
        assert_eq!(
            paths(&check(&owners(people), PageOf::Defaults(PlaceKind::Org)).unwrap_err()),
            ["owners"]
        );
    }

    #[test]
    fn passing_on_control_of_a_page_asks_an_owner_or_an_admin() {
        let dana = Holder::User { id: new_id() };
        let asks = |change: &PageChange| check(change, thread()).unwrap().asks();
        assert_eq!(asks(&adding("thread.manage", dana)), Asks::default());
        assert_eq!(
            asks(&adding("access.manage", dana)),
            Asks {
                owner_or_admin: true,
                admin: false
            }
        );
        assert_eq!(
            asks(&adding("access.external", dana)),
            Asks {
                owner_or_admin: false,
                admin: true
            }
        );
        let owners = PageChange {
            owners: Some(OwnersChange::default()),
            ..PageChange::default()
        };
        assert_eq!(
            asks(&owners),
            Asks {
                owner_or_admin: true,
                admin: false
            }
        );
        let cut_loose = one_row(RowChange {
            inherit: Some(false),
            ..row_of("access.external")
        });
        assert_eq!(
            asks(&cut_loose),
            Asks::default(),
            "cutting a row loose names nobody"
        );
        let private = PageChange {
            whole: Some(Whole::Private),
            ..PageChange::default()
        };
        assert_eq!(asks(&private), Asks::default());
    }

    fn owners_of(people: &[(Id, &str)], first: Id) -> Owners {
        let entries = people
            .iter()
            .map(|(id, since)| Entry {
                created_at: at(since),
                ..entry(Holder::User { id: *id })
            })
            .collect();
        Owners { entries, first }
    }

    fn owners_change(add: &[Holder], remove: &[Id], first: Option<Id>) -> CheckedOwners {
        CheckedOwners {
            add: add.to_vec(),
            remove: remove.to_vec(),
            first,
        }
    }

    #[test]
    fn whoever_makes_a_thing_is_its_first_owner() {
        let (maker, other) = (new_id(), new_id());
        let owners = Owners::of_maker(new_id(), maker, now());
        assert_eq!(owners.first, maker);
        assert_eq!(owners.people(now()), [maker]);
        assert_eq!(owners.entries[0].source, EntrySource::Creation);
        let is = |id| Person {
            id,
            groups: &[],
            member_of: &[],
        };
        assert!(owners.include(&is(maker), now()));
        assert!(!owners.include(&is(other), now()));
    }

    #[test]
    fn a_group_among_the_owners_makes_its_people_owners() {
        let (alice, bo, team) = (new_id(), new_id(), new_id());
        let mut owners = owners_of(&[(alice, "2026-01-01T00:00:00Z")], alice);
        owners.entries.push(entry(Holder::Group { id: team }));
        let groups = [team];
        assert!(owners.include(
            &Person {
                id: bo,
                groups: &groups,
                member_of: &[]
            },
            now()
        ));
        assert!(!owners.include(
            &Person {
                id: bo,
                groups: &[],
                member_of: &[]
            },
            now()
        ));
        assert_eq!(owners.people(now()), [alice], "a group is no person");
    }

    #[test]
    fn the_last_person_in_an_owners_row_is_not_taken_out() {
        let (alice, team) = (new_id(), new_id());
        let mut owners = owners_of(&[(alice, "2026-01-01T00:00:00Z")], alice);
        owners.entries.push(entry(Holder::Group { id: team }));
        let alices = owners.entries[0].id;
        assert_eq!(
            owners.after(&owners_change(&[], &[alices], None), now()),
            Err(OwnersFault::LastPerson)
        );
        let bo = new_id();
        let handed_on = owners
            .after(
                &owners_change(&[Holder::User { id: bo }], &[alices], None),
                now(),
            )
            .unwrap();
        assert_eq!(
            handed_on.first, bo,
            "somebody else is named, and the thing passes to them"
        );
        assert_eq!(handed_on.add, [Holder::User { id: bo }]);
    }

    #[test]
    fn when_the_first_owner_leaves_whoever_has_been_there_longest_is_first() {
        let (alice, bo, chen) = (new_id(), new_id(), new_id());
        let owners = owners_of(
            &[
                (alice, "2026-01-01T00:00:00Z"),
                (chen, "2026-03-01T00:00:00Z"),
                (bo, "2026-02-01T00:00:00Z"),
            ],
            alice,
        );
        let alices = owners.entries[0].id;
        let after = owners
            .after(&owners_change(&[], &[alices], None), now())
            .unwrap();
        assert_eq!(after.first, bo);
        let chens = owners.entries[1].id;
        let after = owners
            .after(&owners_change(&[], &[chens], None), now())
            .unwrap();
        assert_eq!(
            after.first, alice,
            "taking another owner out changes nothing about who is first"
        );
    }

    #[test]
    fn naming_somebody_first_adds_them_when_they_are_not_in_the_row() {
        let (alice, bo, dana) = (new_id(), new_id(), new_id());
        let owners = owners_of(
            &[
                (alice, "2026-01-01T00:00:00Z"),
                (bo, "2026-02-01T00:00:00Z"),
            ],
            alice,
        );
        let after = owners
            .after(&owners_change(&[], &[], Some(dana)), now())
            .unwrap();
        assert_eq!(
            (after.first, after.add.as_slice()),
            (dana, &[Holder::User { id: dana }][..])
        );
        assert!(
            after.remove.is_empty(),
            "the person who was first stays an owner"
        );
        let after = owners
            .after(
                &owners_change(&[Holder::User { id: bo }], &[], Some(bo)),
                now(),
            )
            .unwrap();
        assert_eq!(after.first, bo);
        assert!(
            after.add.is_empty(),
            "somebody who is in the row is not added again"
        );
    }

    #[test]
    fn a_chat_keeps_the_agents_it_was_shared_with_and_nothing_else() {
        let agents = Holder::Agents { id: new_id() };
        assert!(check(&adding("chat.view", agents), PageOf::Chat).is_ok());
        assert!(check(&adding("chat.interact", agents), PageOf::Chat).is_ok());
        assert!(check(&adding("chat.view", role(Role::Viewer)), PageOf::Chat).is_err());
        assert!(check(&adding("chat.manage", agents), PageOf::Chat).is_err());
        let private = PageChange {
            whole: Some(Whole::Private),
            ..PageChange::default()
        };
        assert_eq!(
            paths(&check(&private, PageOf::Chat).unwrap_err()),
            ["whole"]
        );
    }

    #[test]
    fn every_fault_of_a_change_is_reported_at_once() {
        let long_note = "x".repeat(NOTE_MAX_CHARS + 1);
        let change = PageChange {
            rows: Some(vec![
                row_of("thread.read"),
                RowChange {
                    locked: Some(true),
                    add: Some(vec![EntryAdd {
                        holder: Holder::Everyone,
                        options: Some(json!({ "models": [] })),
                        expires_at: None,
                        note: Some(long_note),
                    }]),
                    ..row_of("thread.view")
                },
            ]),
            ..PageChange::default()
        };
        let faults = check(&change, thread()).unwrap_err();
        assert_eq!(
            paths(&faults),
            [
                "rows[0].permission",
                "rows[1].locked",
                "rows[1].add[0].holder",
                "rows[1].add[0].note",
                "rows[1].add[0].options.models"
            ]
        );
    }

    #[test]
    fn stored_kinds_name_every_node_alike() {
        assert_eq!(StoredKind::from(ObjectKind::Topic), StoredKind::Node);
        assert_eq!(StoredKind::from(ObjectKind::Org).as_str(), "node");
        assert_eq!(
            StoredKind::from(ObjectKind::McpServer).as_str(),
            "mcp_server"
        );
        assert_eq!(
            serde_json::to_value(Origin::Defaults(PlaceRef::System)).unwrap(),
            json!({ "page": "defaults", "of": "system" })
        );
        assert_eq!(
            serde_json::to_value(Origin::Own).unwrap(),
            json!({ "page": "own" })
        );
    }
}
