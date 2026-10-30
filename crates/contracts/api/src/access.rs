//! Roles held in a place, permission pages with their owners and defaults pages with their previews, what a page's reader may give and what a place's page covers, a holder as a name and a count with its people on request, the change of a page that names owners and holders at a choice, who may name somebody from outside, what a caller lacks for an action and whether they may force it, requests for all of it at once, effective access views, and the lines of a thing's history.
//!
//! Every thing that can be shared has a permission page: rows of permissions,
//! each naming the holders who have it. [`PermissionPage`] reads one with what
//! each row inherits, whom it names and what the caller may change;
//! [`PageChange`] is the one way it is changed, and [`PagePreview`] says what
//! a change would do before it is made. A page never carries the people behind
//! a role, a group or a place: a holder is a name, and the people are read
//! page by page when somebody asks. What a caller lacks for an act is a list of
//! [`MissingAccess`], which can be sent back as an access request.

use std::collections::BTreeMap;

use orch_types::{Id, Json, Revision, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::approvals::Approval;
use crate::common::{Page, absent_or_null};
use crate::directory::Group;
use crate::nodes::{EnvironmentRef, NodeOrSystem, NodeRef, PageState};
use crate::routes::Access::{Permission, Rule, User};
use crate::users::UserRef;

/// The most things one batch of access requests may ask for; a batch holds at
/// least one.
pub const ACCESS_REQUEST_MAX_ITEMS: u32 = 8;
/// The most people a preview names in each of its two lists; the rest are counted.
pub const PREVIEW_MAX_PEOPLE: u32 = 200;
/// The most people a refusal names as able to give what is missing; the rest
/// are counted and never listed.
pub const ASK_MAX_PEOPLE: u32 = 5;

/// What a person can be in a place, from least to most; the order of the
/// variants is the order of the roles, so the higher of two compares greater.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Reads.
    Viewer,
    /// Reads and takes part.
    Interactor,
    /// Reads, takes part and changes.
    Developer,
    /// Manages the place and what is in it.
    Admin,
}

/// A thing that has a permission page, named by kind.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ObjectRef {
    /// What kind of thing it is, as a word such as `thread` or `provider`.
    pub kind: String,
    /// Its identifier.
    pub id: Id,
    /// Its name.
    pub name: String,
}

/// How far up a ladder of levels a permission sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PermissionLevel {
    /// To see.
    View,
    /// To take part.
    Interact,
    /// To change.
    Edit,
    /// To use.
    Use,
    /// To manage.
    Manage,
    /// To own.
    Own,
    /// A row for one single act, kept apart from the ladder of levels.
    Advanced,
}

/// One permission of the catalogue, as data for the pages that show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PermissionDef {
    /// Its key, such as `thread.interact`.
    pub name: String,
    /// The area of things it belongs to.
    pub area: String,
    /// Its level in that area.
    pub level: PermissionLevel,
    /// For an advanced row, the permission whose holders also hold it; `null` for the one that
    /// stands with no other.
    pub with: Option<String>,
    /// The kinds of thing it exists on.
    pub on: Vec<String>,
    /// What it is called, such as `Interact Thread`.
    pub label: String,
    /// A sentence on what it allows a person to do.
    pub allows: String,
    /// Whether its default is split into one row for managed environments and one for a person's
    /// machine.
    pub profiles: bool,
    /// The kinds of holder its rows take.
    pub holders: Vec<String>,
    /// A JSON Schema of the options an entry in it may carry.
    pub options_schema: Option<Json>,
}

/// The three everyday ways to name somebody on a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AccessChoice {
    /// To see everything in the area.
    FullView,
    /// To see it and take part.
    FullInteract,
    /// To see it, take part and manage.
    FullManage,
}

/// One everyday choice, with the permissions it covers for each kind of thing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PermissionChoice {
    /// Which choice it is.
    pub key: AccessChoice,
    /// What it is called.
    pub label: String,
    /// The permissions it covers, by kind of thing.
    pub permissions: BTreeMap<String, Vec<String>>,
}

/// A role and what it is called.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RoleName {
    /// The role.
    pub key: Role,
    /// Its name.
    pub name: String,
}

/// The catalogue of permissions as data: every page is drawn from it and names
/// no permission of its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PermissionCatalogueGetReply {
    /// Every permission.
    pub permissions: Vec<PermissionDef>,
    /// The three everyday choices.
    pub choices: Vec<PermissionChoice>,
    /// The roles and their names.
    pub roles: Vec<RoleName>,
}

/// Whom a row names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Holder {
    /// The members whose domain is this org or inside it.
    Members {
        /// The org.
        node: NodeRef,
    },
    /// A role, or any role above it, held at the object's own place or at another named place,
    /// possibly in a different org.
    Role {
        /// The role.
        role: Role,
        /// The place the role is held in; the object's own place when `null`.
        at: Option<NodeRef>,
        /// The whole visible place for an implicit local role; display only, never stored by a change.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        place: Option<String>,
        /// How many people hold it now, for the chip ("Developers · 14"). Who they are is asked for
        /// by the people request, and is never sent with a page.
        count: i64,
    },
    /// A group.
    Group {
        /// The group.
        group: Group,
    },
    /// A person.
    User {
        /// The person.
        user: UserRef,
    },
    /// For the use of a resource: the work in that place.
    Node {
        /// The place.
        node: NodeRef,
    },
    /// The agents of the threads at or below that place.
    Agents {
        /// The place.
        node: NodeRef,
    },
    /// Everybody, for the resources of the system.
    Everyone,
}

/// Whom a change names, by identifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HolderInput {
    /// The members whose domain is this org or inside it.
    Members {
        /// The org.
        id: Id,
    },
    /// That role or a higher one.
    Role {
        /// The role.
        role: Role,
        /// The place it is held in; the object's own place when left out.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<Id>,
    },
    /// A group.
    Group {
        /// The group.
        id: Id,
    },
    /// A person.
    User {
        /// The person.
        id: Id,
    },
    /// The work in a place.
    Node {
        /// The place.
        id: Id,
    },
    /// The agents of the threads at or under a place.
    Agents {
        /// The place.
        id: Id,
    },
    /// Everybody.
    Everyone,
}

/// One accurate result of a holder search, ready to select by its identifiers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HolderCandidate {
    /// What a change stores.
    pub input: HolderInput,
    /// Names and marks for the selected holder.
    pub holder: Holder,
    /// Its full current locator, for completing a search.
    pub locator: String,
    /// Its place, or a person's current address, in human form.
    pub place: String,
    /// Whether it belongs to another tree than the page.
    pub outside: bool,
}

/// Searches people, custom groups and predefined holders together.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HolderSearchQuery {
    /// A name or full/partial address, with a role or group after it.
    #[serde(default)]
    pub q: String,
    /// Where the picker starts; also chooses the tree listed first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place_id: Option<Id>,
    /// The tree the page belongs to, for outsider classification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<Id>,
    /// Only a resource Use field asks for the work in a place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_work: Option<bool>,
    /// The place control lists visible places of its tree, or matching addresses in any tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list_places: Option<bool>,
}

/// Accurate search results; an empty list says that nothing visible is called that.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HolderSearchReply {
    /// The page's tree first, every result with its whole place.
    pub items: Vec<HolderCandidate>,
}

/// How an entry came to be on a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EntrySource {
    /// Written on the page by the person who made it.
    Page,
    /// Chosen when the thing was made.
    Creation,
    /// An approved request.
    Request,
    /// Shipped with the system.
    Seed,
}

/// One holder named in one row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Entry {
    /// First-reading material hint on an agents/doc.view entry, never another permission.
    #[serde(default)]
    pub read_first: bool,
    /// The entry's identifier.
    pub id: Id,
    /// Whom it names.
    pub holder: Holder,
    /// The options it carries, as the permission allows.
    pub options: Json,
    /// Whether the holder is, or reaches, somebody outside the object's root org.
    pub outside: bool,
    /// When it stops holding.
    pub expires_at: Option<Time>,
    /// A note its maker left.
    pub note: String,
    /// How it came to be there.
    pub source: EntrySource,
    /// Who wrote it.
    pub created_by: Option<UserRef>,
    /// When it was written.
    pub created_at: Time,
    /// How many times it was changed.
    pub revision: Revision,
}

/// The kind of environment a row of a defaults page is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RowProfile {
    /// The permission has one row.
    Any,
    /// The row for an environment made from a template on a host.
    Managed,
    /// The row for a directory on a person's machine.
    Connected,
}

/// The two kinds of environment whose defaults differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PageProfile {
    /// An environment made from a template on a host.
    Managed,
    /// A directory on a person's machine.
    Connected,
}

/// What a row that follows the defaults amounts to now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InheritedHolders {
    /// Whom the inherited row names.
    pub holders: Vec<Holder>,
    /// The effective entries behind those holders, so a local cut keeps their limits and ends.
    /// Older readers need only the holder names; empty rows omit this member.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<Entry>,
    /// The nearest place whose defaults say so.
    pub from: Option<NodeOrSystem>,
}

/// A lock above that decides a row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RowLock {
    /// Where the lock was set.
    pub by: NodeOrSystem,
    /// Whether that is this defaults page.
    pub here: bool,
}

/// One permission of a page, with whom it names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PermissionRow {
    /// The permission, such as `thread.interact`.
    pub permission: String,
    /// On a defaults page, which kind of environment the row applies to.
    pub profile: RowProfile,
    /// Whether the row follows the default of the place the object lies in.
    pub inherit: bool,
    /// What following amounts to now; `null` when the row does not follow.
    pub inherited: Option<InheritedHolders>,
    /// Whom the row names itself.
    pub entries: Vec<Entry>,
    /// The lock that decides this row, if there is one.
    pub locked: Option<RowLock>,
    /// Whether the caller manages this row (`access.manage`): may take holders out of it, cut it
    /// loose and tie it back.
    pub writable: bool,
    /// Whether the caller may name somebody in it: they hold this permission here themself, and may
    /// share here, as an owner, as a manager of the page, or because the place lets holders share.
    /// True also for somebody who does not manage the row.
    pub can_give: bool,
}

/// How far the holders of a permission here also hold what it depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum DependencyOpen {
    /// All of them, through an entry for a place or each by themself.
    #[serde(rename = "all")]
    All,
    /// Some of them.
    #[serde(rename = "some")]
    Part,
    /// Nobody but its owner.
    #[serde(rename = "none")]
    Nobody,
}

/// The other object a dependency is on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DependencyObject {
    /// What kind of thing it is.
    pub kind: String,
    /// Its identifier.
    pub id: Id,
    /// Its name.
    pub name: String,
    /// Its first owner.
    pub owner: Option<UserRef>,
}

/// Something on another object that a permission here is of no use without.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Dependency {
    /// The permission here, such as `thread.interact`, `topic.interact`; for a thread's environment
    /// `thread.view` and `thread.manage`.
    #[serde(rename = "for")]
    pub for_permission: String,
    /// The object it depends on.
    pub object: DependencyObject,
    /// The permission there, such as `provider.use` or `environment.view`.
    pub permission: String,
    /// An everyday choice covering several permissions there, such as the environment's Can view;
    /// null when this dependency names only the permission above.
    pub choice: Option<AccessChoice>,
    /// The options in use there: for a provider, the model and effort.
    pub options: Option<Json>,
    /// Whether the holders of the permission here have what it needs there.
    pub open: DependencyOpen,
    /// The place whose entry opens it, when one does.
    pub through: Option<NodeRef>,
    /// Whether the caller can give it there: they hold it, and may share there.
    pub can_give: bool,
}

/// Whether a page belongs to a thing or is what a place gives to what is
/// made inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PageKind {
    /// The page of one thing.
    Live,
    /// A defaults page.
    Defaults,
}

/// Who holds `access.manage` on a page without being named on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PageAdmins {
    /// The places whose admins they are, nearest first.
    pub of: Vec<NodeOrSystem>,
    /// How many people that comes to; who they are is asked for separately.
    pub count: i64,
}

/// What a thread's page does not keep: the files of the environment it runs in, which whoever looks
/// into that environment reads, whatever this page says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PageReaches {
    /// The environment the thread works in.
    pub environment: EnvironmentRef,
    /// How many people look into it: those who hold its `files.view`.
    pub people: i64,
    /// How many other threads work there.
    pub threads: i64,
}

/// What may be given to the agents of a place or of a thread: the words of the sharing form for
/// them, as for a person.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentLevel {
    /// Can view: they read the conversations, the chats and the documents.
    View,
    /// Can edit: they also send to those threads, post in those chats and change those documents.
    Edit,
    /// Can manage: they also delete, move and bring back the documents.
    Manage,
}

/// What the reader of a page may give here, which is at most what they hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PageGives {
    /// The everyday choices whose every row they may give.
    pub choices: Vec<AccessChoice>,
    /// On a place, the roles up to their own; empty elsewhere.
    pub roles: Vec<Role>,
    /// What they may share with agents.
    pub agents: Vec<AgentLevel>,
    /// The policy `sharing.allow_holders` as it resolves here. The lists above are empty for a
    /// holder who is neither an owner nor a manager of the page where it is false, and this lets a
    /// form say why.
    pub holders_share: bool,
}

/// How many places, threads and documents lie inside a place: the three things a role held there or
/// a share of the place with what is in it reaches.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PageCounts {
    /// Orgs, projects, folders and topics.
    pub places: i64,
    /// Threads.
    pub threads: i64,
    /// Documents written beside a place or a thread.
    pub documents: i64,
}

/// What lies inside a place, as far as the caller may see it: what still follows the place, which a
/// role held there or a share of the place with what is in it reaches, and what was made private or
/// cut loose throughout, which neither reaches.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PageCovers {
    /// Places inside that follow it.
    pub places: i64,
    /// Threads inside that follow it.
    pub threads: i64,
    /// Documents inside that follow it.
    pub documents: i64,
    /// What was made private or cut loose throughout.
    pub apart: PageCounts,
}

/// A permission page: every row, what each inherits, whom it names, and what
/// the caller may change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PermissionPage {
    /// What the page is of.
    pub object: ObjectRef,
    /// Whether it is a thing's page or a defaults page.
    pub page: PageKind,
    /// The first owner; `null` for the system's defaults.
    pub owner: Option<UserRef>,
    /// The Owners row, in the order the owners were named. A defaults page has none.
    pub owners: Vec<Entry>,
    /// Whether the caller may change the Owners row.
    pub owners_writable: bool,
    /// Whether the caller may name somebody from outside the root org here.
    pub external: bool,
    /// Who holds `access.manage` here without being named.
    pub admins: PageAdmins,
    /// The rows.
    pub rows: Vec<PermissionRow>,
    /// How the page stands against the defaults it inherits.
    pub state: PageState,
    /// For a defaults page, how many pages below it follow at least one of its rows.
    pub followers: Option<i64>,
    /// What the caller may give here: the choices, the roles and what to share with agents.
    pub gives: PageGives,
    /// The live page of a place: what lies inside it and still follows it, and under `apart` what
    /// was made private or cut loose throughout; `null` for anything that is not a place, and for
    /// a defaults page.
    pub covers: Option<PageCovers>,
    /// On a live page: what its permissions are of no use without.
    pub depends: Vec<Dependency>,
    /// Set only on the page of a bound thread: the environment it runs in, whose files are read by
    /// whoever looks into it whatever this page says, with how many people that is and how many other
    /// threads work there.
    pub reaches: Option<PageReaches>,
    /// How many times the page was changed.
    pub revision: Revision,
}

/// Names holders in every row that one of the three choices covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageChangeName {
    /// Whom to name.
    pub holders: Vec<HolderInput>,
    /// The choice that says in which rows.
    pub choice: AccessChoice,
    /// Whether the holders are also named in the Export rows of the same areas, so that whoever is let
    /// read may take it away; `true` unless given, as far as the caller holds those rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export: Option<bool>,
    /// The options for every entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Json>,
    /// When they stop holding it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Time>,
    /// A note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// An entry to add to a row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageChangeAdd {
    /// Whom to name.
    pub holder: HolderInput,
    /// Its options.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Json>,
    /// When it stops holding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Time>,
    /// A note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A change to an entry that exists. A member left out stays; `expires_at` of
/// `null` takes the expiry away.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageChangeUpdate {
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

/// A change to one row of a page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageChangeRow {
    /// The permission whose row it is.
    pub permission: String,
    /// On a defaults page, which of the two rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<PageProfile>,
    /// Whether the row follows the default of the place above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inherit: Option<bool>,
    /// On a defaults page: whether the row is locked for everything below.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locked: Option<bool>,
    /// Entries to add.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add: Option<Vec<PageChangeAdd>>,
    /// Entries to change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<Vec<PageChangeUpdate>>,
    /// Entries to remove, by identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove: Option<Vec<Id>>,
}

/// A change to the Owners row.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageChangeOwners {
    /// Owners to add: users or groups of the object's root org.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add: Option<Vec<HolderInput>>,
    /// Entries of the Owners row to take out, never the last person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove: Option<Vec<Id>>,
    /// The user who is to be first among the owners; they are added to the row if they are not in
    /// it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first: Option<Id>,
}

/// A change to the whole of a page at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PageChangeWhole {
    /// Cut every row loose from the defaults.
    Private,
    /// Drop every stored row except the owners', so that the page follows the defaults again.
    Reset,
}

/// One change to a page or a defaults page, applied entirely or not at all.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageChange {
    /// The short way to add people: it names holders in every row its choice covers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<Vec<PageChangeName>>,
    /// The same change, row by row. A change may use both forms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<Vec<PageChangeRow>>,
    /// Changes of the Owners row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owners: Option<PageChangeOwners>,
    /// A change of the whole page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub whole: Option<PageChangeWhole>,
    /// The revision of the page the caller saw.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<Revision>,
    /// Tell the people explicitly named by this change after they gain access.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify: Option<bool>,
    /// The part to open from the notice when the change also tells people.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify_anchor: Option<crate::notifications::PersonNoticeAnchor>,
}

/// A person who would gain or lose something by a change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PreviewPerson {
    /// The person.
    pub user: UserRef,
    /// What they would gain or lose.
    pub permissions: Vec<String>,
    /// On how many pages.
    pub pages: i64,
}

/// The effect a change would have, worked out before it is made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PagePreview {
    /// How many pages it reaches; one for a live page.
    pub pages: i64,
    /// For a preview of the system's defaults: in how many root orgs those pages lie; `null` for
    /// every other preview.
    pub orgs: Option<i64>,
    /// People who would gain.
    pub gaining: Vec<PreviewPerson>,
    /// People who would lose.
    pub losing: Vec<PreviewPerson>,
    /// People beyond the first [`PREVIEW_MAX_PEOPLE`] in either list.
    pub more: i64,
}

/// One ordinary sharing act, previewed without applying it or accepting an input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SharePreviewBody {
    /// Add holders on the thing's live permission page, using the ordinary change body.
    Page {
        /// The additive change; management edits are not a sharing preview.
        change: PageChange,
    },
    /// Give a role on this place, including what follows it inside.
    Role {
        /// The ordinary role creation body.
        give: RoleCreateBody,
    },
    /// Share with agents, with the ordinary source, reach, areas and options.
    Agents {
        /// The ordinary agent sharing body; its source must agree with the path.
        share: crate::share::ShareWithAgents,
    },
}

/// A rule resolved where this share is made; a port-use policy never grants or forbids a holder entry by itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SharePreviewRule {
    /// The registry key, or a capability's key under capability.policy.
    pub key: String,
    /// Its current resolved value, with no secrets.
    pub value: Json,
    /// Where it came from; null for a registry or capability definition's default.
    pub source: Option<NodeOrSystem>,
    /// True for a rule governing the sharing act; false for later use, such as opening a port.
    pub sharing: bool,
}

/// The current and proposed live areas for one agent destination, or the bounded size of a proposed copy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SharePreviewAgent {
    /// The thread or place whose agents receive it, named only as far as the reader may know it.
    pub to: NodeRef,
    /// Whether this live share includes what follows the source place inside it.
    pub inside: bool,
    /// The areas the ordinary producer would give.
    pub parts: crate::share::ShareParts,
    /// Current direct live areas for this same destination and reach; implicit topic/project access remains separate.
    pub already_shared: crate::share::ShareParts,
    /// Whether these agents may also take the material away as files.
    pub download: bool,
    /// Whether this destination includes threads made later under the named place.
    pub future_threads: bool,
    /// A copy's files and bytes; null for a live share. No bytes are kept or inputs accepted by a preview.
    pub copy: Option<SharePreviewCopy>,
}

/// The size of a proposed copy, computed by the ordinary copy reader.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SharePreviewCopy {
    /// Files it contains.
    pub files: i64,
    /// Total bytes before an archive is made.
    pub bytes: i64,
    /// The same client id already sent this copy; applying again returns the existing receipt.
    pub already_sent: bool,
}

/// Current and proposed entries for a selected holder's row, including its options, expiry and first-reading hint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SharePreviewRow {
    /// The source place's defaults, rather than its live page.
    pub defaults: bool,
    /// The permission row.
    pub permission: String,
    /// Its environment form, using the common row profile.
    pub profile: RowProfile,
    /// Selected holders' current direct entries.
    pub before: Vec<Entry>,
    /// Proposed trial entries. Their identifiers are not accepted receipts and must not be used for Undo.
    pub after: Vec<Entry>,
}

/// One row of a selected-row review. It is advice only: the ordinary write rechecks current authority and its own revision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SharePreview {
    /// Whether the exact request may be made now.
    pub allowed: bool,
    /// Current live page revision; null for a chat, which has no permission page.
    pub revision: Option<Revision>,
    /// What the requested holders would be given, in catalogue names.
    pub permissions: Vec<String>,
    /// The selected current holders, using the common holder shape and counts.
    pub holders: Vec<Holder>,
    /// Human gains and losses through the normal access engine, with its normal name cut.
    pub effects: PagePreview,
    /// Selected people who already hold some of the requested rows, through any current holder.
    pub already: Vec<PreviewPerson>,
    /// People already holding rows who are not named, including those beyond the named-list bound.
    pub already_more: i64,
    /// Agent destinations and their current/proposed direct reaches.
    pub agents: Vec<SharePreviewAgent>,
    /// Direct entry changes; the ordinary entries retain exact option bounds and expiries.
    pub rows: Vec<SharePreviewRow>,
    /// The sharing rules and relevant later-use capability rules, with their current provenance.
    pub rules: Vec<SharePreviewRule>,
    /// The ordinary refusal for the exact request; null when allowed. A caller who cannot share gets no preview.
    pub refusal: Option<crate::common::Problem>,
}

/// How a new thread's page starts, short of a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AccessPreset {
    /// Every row follows the defaults of its topic.
    Defaults,
    /// Every row is cut loose: its owner alone until somebody is named.
    Private,
}

/// A page taken over from another thread, with changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadAccessAs {
    /// The thread whose page is carried over, its owners never.
    #[serde(rename = "as")]
    pub source: Id,
    /// The rows in which the new page differs from it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<PageChange>,
}

/// The page that a new thread starts with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ThreadAccess {
    /// Follow the defaults, or start private.
    Preset(AccessPreset),
    /// A fork's page: another thread's, carried over.
    As(ThreadAccessAs),
    /// The rows in which the page differs from the defaults.
    Change(PageChange),
}

/// What gives a person a permission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[expect(
    clippy::large_enum_variant,
    reason = "a line of an explanation is built once to be written; boxing the entry would put a layer in every use"
)]
pub enum AccessThrough {
    /// They own the thing.
    Owner,
    /// They own the physical resource that inherently grants management here.
    SourceOwner,
    /// They administer its place.
    Admin {
        /// Where their Admin role is held, named only as far as the reader may know it.
        from: Option<NodeOrSystem>,
    },
    /// They are a system admin reading a thing that belongs to the system.
    SystemAdmin,
    /// A higher level of the same area.
    Level {
        /// The permission of that higher level.
        of: String,
    },
    /// An entry, on the page itself or on the defaults of a place.
    Entry {
        /// The entry.
        entry: Entry,
        /// `null` for the page itself; otherwise the place whose defaults hold it.
        from: Option<NodeOrSystem>,
        /// The current chain through nested groups, role inheritance or domain membership.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        chain: Vec<Holder>,
    },
}

/// One permission a person holds, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EffectivePermission {
    /// The permission.
    pub permission: String,
    /// Everything that gives it.
    pub through: Vec<AccessThrough>,
}

/// What one person holds on an object, and what gives it to them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EffectiveAccess {
    /// Held permissions absent from this person's ordinary frame, in catalogue order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub admin_only: Vec<String>,
    /// The person.
    pub user: UserRef,
    /// Their role there.
    pub role: Option<Role>,
    /// Where that role is held, named only as far as the reader may know it; `null` without a role.
    pub role_from: Option<NodeOrSystem>,
    /// Whether they administer the place the object is in.
    pub administers: bool,
    /// What they hold.
    pub permissions: Vec<EffectivePermission>,
}

/// What kind of holder a role is given to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubjectKind {
    /// A person.
    User,
    /// A group.
    Group,
    /// All the members of an org.
    Members,
    /// Everyone holding a role at a named place.
    Role,
}

/// Whom a role is given to, as a request names them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubjectInput {
    /// What kind of holder.
    pub kind: SubjectKind,
    /// Its identifier.
    pub id: Id,
    /// For `role`, the role held at the place `id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
}

#[cfg(test)]
mod member_subject_tests {
    #[test]
    fn roles_and_page_entries_use_the_same_members_subject_word() {
        let id = orch_types::new_id();
        let written = serde_json::json!({ "kind": "members", "id": id });
        let role: super::SubjectInput = serde_json::from_value(written.clone()).unwrap();
        let page: super::HolderInput = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(role.kind, super::SubjectKind::Members);
        assert_eq!(serde_json::to_value(role).unwrap(), written);
        assert_eq!(serde_json::to_value(page).unwrap(), written);
        assert!(
            serde_json::from_value::<super::SubjectInput>(
                serde_json::json!({ "kind": "org_members", "id": id })
            )
            .is_err()
        );
    }
}

/// Whom a role is given to, with the names to show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Subject {
    /// What kind of holder.
    pub kind: SubjectKind,
    /// Its identifier.
    pub id: Id,
    /// For a person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<UserRef>,
    /// For a group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<Group>,
    /// For the members of an org.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org: Option<NodeRef>,
    /// For a role held at another place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    /// Where that role is held.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<NodeRef>,
}

/// A role held in a place by a person, a group or the members of an org.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RoleHeld {
    /// The row's identifier.
    pub id: Id,
    /// The place the role is held in.
    pub node: NodeRef,
    /// Whom it is given to.
    pub subject: Subject,
    /// The role.
    pub role: Role,
    /// Whether it is a member's role at their domain, which moves with them and
    /// is not removed by itself.
    pub domain: bool,
    /// Whether the subject is not of this tree of orgs.
    pub outside: bool,
    /// When it ends.
    pub expires_at: Option<Time>,
    /// A note.
    pub note: String,
    /// Who gave it.
    pub created_by: UserRef,
    /// When it was given.
    pub created_at: Time,
    /// How many times it was changed.
    pub revision: Revision,
}

/// A page boiled down to three lines, for places with no room for the page itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AccessSummary {
    /// Who holds the choice to view in every area.
    pub view: Vec<Holder>,
    /// Who holds the choice to take part.
    pub interact: Vec<Holder>,
    /// Who holds the choice to manage.
    pub manage: Vec<Holder>,
    /// Whether the rows say more than these lines: somebody holds only some of the areas.
    pub custom: bool,
    /// The nearest place whose defaults settle this; `null` when nothing is inherited.
    pub from: Option<NodeOrSystem>,
}

/// Who gets to use a resource while it is being created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NewResourceAccess {
    /// As the defaults of its home say.
    Defaults,
    /// Its maker alone, whatever the defaults become.
    OnlyMe,
    /// Every member of its home, the org it is made in.
    Home,
}

/// What gives the caller their highest level on a resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[expect(
    clippy::large_enum_variant,
    reason = "a line of an explanation is built once to be written; boxing the holder would put a layer in every use"
)]
pub enum ResourceThrough {
    /// They own it.
    Owner,
    /// They own the physical resource that inherently grants management here.
    SourceOwner,
    /// Through an entry that names the caller, a group or role of theirs, or the work in a place.
    Entry {
        /// Whom the entry names.
        holder: Holder,
        /// Who wrote it.
        by: Option<UserRef>,
    },
}

/// Why an object is readable only through this session's administrative reach.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminAccess {
    /// The nearest actual administered place, or the system when only system administration gives View.
    pub place: crate::nodes::NodeOrSystem,
    /// The content area's page is cut loose below its administrative defaults.
    #[serde(default)]
    pub private: bool,
}

/// What the caller holds on a resource, and how.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResourceAccess {
    /// The actual source of native-only permissions, even when ordinary View leaves no marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admin_place: Option<crate::nodes::NodeOrSystem>,
    /// Native reach alone gives View; absent when the person's ordinary pages already do.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admin_access: Option<AdminAccess>,
    /// Held permissions absent from the ordinary frame, in catalogue order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub admin_only: Vec<String>,
    /// A personal/group assignment here, independent of View and As admin.
    #[serde(default, skip_serializing_if = "crate::common::is_false")]
    pub participating: bool,
    /// What they hold.
    pub permissions: Vec<String>,
    /// Whether they administer the place it belongs to.
    pub administers: bool,
    /// What gives them their highest level.
    pub through: Vec<ResourceThrough>,
}

/// The least of a provider's models and efforts that would do.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AccessOptions {
    /// The models.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<String>>,
    /// The efforts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub efforts: Option<Vec<String>>,
}

/// Why nobody can give what is missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClosedReason {
    /// A subscription of this kind stays with its owner.
    SubscriptionNotShared,
    /// A lock above decides the row.
    Locked,
}

/// The object an access request or a refusal is about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MissingAccessObject {
    /// What kind of thing it is.
    pub kind: String,
    /// Its identifier.
    pub id: Id,
    /// Its name; `null` where the reader may not know it.
    pub name: Option<String>,
    /// Its first owner.
    pub owner: Option<UserRef>,
}

/// What a person lacks on one object for an act, and who can give it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MissingAccess {
    /// The object.
    pub object: MissingAccessObject,
    /// The permissions of its page that the act needs and the person lacks.
    pub permissions: Vec<String>,
    /// When the use of a provider is missing: the smallest set of models and efforts that would
    /// suffice.
    pub options: Option<AccessOptions>,
    /// Up to [`ASK_MAX_PEOPLE`] of the closest people able to approve a request for it at once.
    pub ask: Vec<UserRef>,
    /// How many others could; never listed.
    pub ask_more: i64,
    /// Whether the caller administers the object and may force the act.
    pub r#override: bool,
    /// Set when nobody can give what is missing, with the reason. `ask` is then empty, and an
    /// access request leaves the entry out.
    pub closed: Option<ClosedReason>,
}

/// Who did a thing that is in a history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HistoryAgent {
    /// The thread whose agent it was.
    pub agent: NodeRef,
}

/// Who did a thing: a person, or the agent of a thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum HistoryActor {
    /// A person.
    User(UserRef),
    /// The agent of a thread.
    Agent(HistoryAgent),
}

/// What a history line points at, where there is something to open. A line carries no content, only
/// a pointer to what opens it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HistoryAbout {
    /// A thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<NodeRef>,
    /// The turn that changed files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<Id>,
    /// A shell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_id: Option<Id>,
    /// Whether the last output of that shell was kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept: Option<bool>,
    /// A file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Several files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paths: Option<Vec<String>>,
    /// How many files there are beyond those listed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub more: Option<i64>,
}

/// One line of the history of a thing: what was done to it and who did it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HistoryLine {
    /// The line's identifier.
    pub id: Id,
    /// When it happened.
    pub at: Time,
    /// Who did it, as everybody who may view the thing sees it.
    pub actor: Option<HistoryActor>,
    /// The audit action.
    pub action: String,
    /// What happened, in a sentence.
    pub text: String,
    /// Versioned wording facts for the reader's catalogue; unknown versions keep the safe text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wording: Option<HistoryWording>,
    /// What it points at.
    pub about: Option<HistoryAbout>,
    /// Structured files and outcome of one upload, when this line groups a batch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upload: Option<UploadHistory>,
}

/// A file's measured upload outcome; an absent path was never written into the workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UploadHistoryFile {
    /// Its recorded filename.
    pub name: String,
    /// Where it was written, when its workspace write completed.
    pub path: Option<String>,
    /// Its selected or verified byte size.
    pub size: i64,
    /// How many bytes the server received, absent when no receipt remains.
    pub received: Option<i64>,
    /// How it ended.
    pub outcome: crate::workspace::WorkspaceUploadOutcome,
    /// Whether the person chose to replace an existing name.
    pub replaced: bool,
}

/// One upload's files, destination, measured bytes and final outcome, rendered in the reader's language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UploadHistory {
    /// The workspace that received it.
    pub environment_id: Id,
    /// Its chosen folder.
    pub folder: String,
    /// Every selected file and its measured outcome.
    pub files: Vec<UploadHistoryFile>,
    /// Total bytes received, absent when any receipt is unknown.
    pub bytes: Option<i64>,
    /// The files deliberately replaced.
    pub replaced: i64,
    /// How the batch ended.
    pub outcome: crate::workspace::WorkspaceUploadOutcome,
}

/// A stable semantic verb, independent of the server's fallback language and raw audit identifiers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HistoryWording {
    /// The vocabulary version; currently 1.
    pub version: u32,
    /// A verb of the history vocabulary, selected from the action catalogue, never parsed from text.
    pub verb: String,
    /// The thing as the reader may know it; removed or hidden names remain absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<NodeRef>,
}

/// The kinds of line in the history of an environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HistoryKind {
    /// Made, started, stopped, recreated, moved.
    Lifecycle,
    /// Threads put into it or taken out.
    Threads,
    /// What its agents work with there.
    Tools,
    /// Shells opened in it.
    Shells,
    /// Files changed in it.
    Files,
    /// Its page.
    Access,
}

/// The roles held in a place and above it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeRoleListReply {
    /// The Owners row of this place, in its stored order. Owners hold every role here;
    /// they are not fabricated role assignments and have no role-expiry controls.
    pub owners: Vec<Holder>,
    /// Roles held in this place.
    pub own: Vec<RoleHeld>,
    /// Roles held in a place above this one. System admins hold Admin over every org through a flag
    /// rather than a row, so a page counts them and the people request lists them.
    pub above: Vec<RoleHeld>,
    /// The roles the caller may give here: those up to their own where they may share the place,
    /// and none where the place keeps sharing to its owners and to whoever manages its page and the
    /// caller is neither.
    pub can_give: Vec<Role>,
}

/// Gives a role in a place to a person, a group or the members of an org.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoleCreateBody {
    /// Whom to give it to.
    pub subject: SubjectInput,
    /// The role.
    pub role: Role,
    /// When it ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Time>,
    /// A note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Changes a role that was given. A member left out stays; `expires_at` of
/// `null` takes the end away.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoleUpdateBody {
    /// The new role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
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
    /// The revision the caller saw.
    pub expected_revision: Revision,
}

/// Whose access to read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EffectiveAccessGetQuery {
    /// The person.
    pub user_id: Id,
}

/// A page of people behind a holder, with their actual ownership of this object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PermissionPeopleListReply {
    /// The people in this page, named by the same visibility rules as its permission page.
    pub items: Vec<UserRef>,
    /// The next page, or none when this is the last.
    pub next_cursor: Option<String>,
    /// Identifiers among `items` whose ownership counts, directly or through an owner group.
    /// Absence means none of this page's people are owners, never that ownership was inferred
    /// from their role or the level of the holder being expanded.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub owners: Vec<Id>,
}

/// The people behind one holder of a page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PermissionPeopleListQuery {
    /// Which holder to list. The forms are `role:<place>:<role>` with the place a node identifier
    /// or `system`; `group:<id>`; `node:<id>`, for those with a role in a
    /// place named in a Use row; and `admins`, for the admins of the page's place. Only holders
    /// that the page shows are answered.
    pub holder: String,
    /// How many entries to return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The `next_cursor` of the page before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// The history of a thing, newest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoryListQuery {
    /// Summary contains meaningful acts; Every includes all recorded changes. Thread History starts in Summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<crate::nodes::ActivityDepth>,
    /// A comma list of the kinds of line wanted; for an environment, values of
    /// [`HistoryKind`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<String>,
    /// For an environment, only lines about a file or about anything under a directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Only lines about this thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<Id>,
    /// Only what this person did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<Id>,
    /// How many entries to return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The `next_cursor` of the page before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// The object an access request asks for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AccessRequestObject {
    /// What kind of thing it is.
    pub kind: String,
    /// Its identifier.
    pub id: Id,
}

/// One thing asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AccessRequestItem {
    /// The object.
    pub object: AccessRequestObject,
    /// The permissions of its page.
    pub permissions: Vec<String>,
    /// For the use of a provider: the models and efforts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<AccessOptions>,
    /// The selected person or work in a place the entries would name; the requester when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holder: Option<HolderInput>,
    /// The name written with a reference, when the request came from its card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Asks the owners of up to [`ACCESS_REQUEST_MAX_ITEMS`] things for access,
/// normally the `missing` list of a refusal sent back as it came.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AccessRequestCreateBody {
    /// What is asked for; from one to eight.
    pub items: Vec<AccessRequestItem>,
    /// Why.
    pub reason: String,
    /// Shown to each approver to explain why they were asked: the thread that the person was
    /// attempting to act on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub for_thread: Option<Id>,
}

/// The requests that were made, one approval for each item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AccessRequestCreateReply {
    /// The batch they were sent together as.
    pub batch_id: Id,
    /// An approval of kind `access` per item.
    pub approvals: Vec<Approval>,
}

/// Where a batch of access requests stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AccessRequestGetReply {
    /// The batch.
    pub batch_id: Id,
    /// Its approvals.
    pub approvals: Vec<Approval>,
    /// True once every item is held, by whatever means.
    pub complete: bool,
}

crate::endpoints! {
    /// Reads the catalogue of permissions, the everyday choices and the roles, as data.
    PermissionCatalogueGet: GET "/permissions", User, none, none, json PermissionCatalogueGetReply, 200;
    /// Resolves a holder locator only among places the caller may view.
    HolderSearch: GET "/holders/search", User, query HolderSearchQuery, none, json HolderSearchReply, 200;
    /// Lists who holds which role in a place and in the places above it.
    NodeRoleList: GET "/nodes/{id}/roles", Permission("node.view"), none, none, json NodeRoleListReply, 200;
    /// Gives a role in a place, up to the role the caller holds there.
    NodeRoleCreate: POST "/nodes/{id}/roles", Rule("holding that role there or a higher one, and being an owner of the place or a manager of its page (access.manage), or the place letting holders share"), none, json RoleCreateBody, json RoleHeld, 200;
    /// Changes a role that was given, by whoever manages the place's page or by whoever gave it.
    RoleUpdate: PATCH "/roles/{id}", Rule("access.manage on the place the role is held in; or having given it, for a change that gives no more than the caller may give now"), none, json RoleUpdateBody, json RoleHeld, 200;
    /// Takes a role away, by whoever manages the place's page or by whoever gave it.
    RoleDelete: DELETE "/roles/{id}", Rule("access.manage on the place the role is held in; or having given it"), none, none, none, 204;
    /// Reads a permission page: every row, what it inherits, whom it names and what the caller may change.
    PermissionPageGet: GET "/{object}/{id}/permissions", Rule("View on a node or document, or access.manage to inspect its permission page without content; any permission on a resource"), none, none, json PermissionPage, 200;
    /// Changes a permission page, as a whole or not at all.
    PermissionPageUpdate: PATCH "/{object}/{id}/permissions", Rule("for a change that only names holders and takes back entries the caller gave: holding what it gives oneself, and being an owner or a manager of the page, or the place letting holders share; for anything else in it: access.manage, and for what it gives, holding it oneself"), none, json PageChange, json PermissionPage, 200;
    /// Shows what a change to a page would do, with nothing changed.
    PermissionPagePreview: POST "/{object}/{id}/permissions/preview", Permission("access.manage"), none, json PageChange, json PagePreview, 200;
    /// Previews an ordinary holder, role or agent share for a current sharer, without keeping any change.
    SharePreviewCreate: POST "/{object}/{id}/share/preview", Rule("being able to read and share this thing; the exact write is checked by its ordinary producer"), none, json SharePreviewBody, json SharePreview, 200;
    /// Reads what one person holds on an object and what gives it to them.
    EffectiveAccessGet: GET "/{object}/{id}/permissions/effective", Rule("access.manage on the object, or the person themself"), query EffectiveAccessGetQuery, none, json EffectiveAccess, 200;
    /// Lists the people behind one holder of a page.
    PermissionPeopleList: GET "/{object}/{id}/permissions/people", Rule("the same as reading the page"), query PermissionPeopleListQuery, none, json PermissionPeopleListReply, 200;
    /// Lists what was done to a thing and by whom, newest first.
    HistoryList: GET "/{object}/{id}/history", Rule("node.view on a node, or any permission on the resource"), query HistoryListQuery, none, json Page<HistoryLine>, 200;
    /// Reads what a place gives to what is made inside it.
    NodeDefaultsGet: GET "/nodes/{id}/defaults", Permission("node.view"), none, none, json PermissionPage, 200;
    /// Changes what a place gives to what is made inside it.
    NodeDefaultsUpdate: PATCH "/nodes/{id}/defaults", Permission("access.manage"), none, json PageChange, json PermissionPage, 200;
    /// Shows how many pages follow the rows a change touches and who gains or loses what on them.
    NodeDefaultsPreview: POST "/nodes/{id}/defaults/preview", Permission("access.manage"), none, json PageChange, json PagePreview, 200;
    /// Asks the owners of up to eight things for access in one batch.
    AccessRequestCreate: POST "/access-requests", User, none, json AccessRequestCreateBody, json AccessRequestCreateReply, 200;
    /// Reads where a batch of access requests stands.
    AccessRequestGet: GET "/access-requests/{batch_id}", Rule("the person who asked"), none, none, json AccessRequestGetReply, 200;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_share_review_is_advice_with_the_common_holders_and_ordinary_apply_body() {
        let wire = serde_json::json!({"allowed":true,"revision":7,"permissions":[],"holders":[],
            "effects":{"pages":1,"orgs":null,"gaining":[],"losing":[],"more":0},"already":[],"already_more":0,"agents":[],"rows":[],
            "rules":[{"key":"sharing.allow_holders","value":true,"source":"system","sharing":true}],"refusal":null});
        let read: SharePreview = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(read).unwrap(), wire);
        assert!(
            serde_json::from_value::<SharePreviewBody>(
                serde_json::json!({"kind":"page","change":{},"fence":"invented"})
            )
            .is_err()
        );
    }
    use serde_json::json;

    #[test]
    fn administration_keeps_its_place_and_system_ownership_keeps_its_distinct_cause() {
        for (cause, written) in [
            (
                AccessThrough::Admin {
                    from: Some(NodeOrSystem::SYSTEM),
                },
                json!({"kind": "admin", "from": "system"}),
            ),
            (AccessThrough::SystemAdmin, json!({"kind": "system_admin"})),
        ] {
            assert_eq!(serde_json::to_value(&cause).unwrap(), written);
            assert_eq!(
                serde_json::from_value::<AccessThrough>(written).unwrap(),
                cause
            );
        }
    }

    #[test]
    fn a_change_that_clears_an_expiry_is_told_from_one_that_leaves_it() {
        let clears: PageChange = serde_json::from_value(json!({
            "rows": [{
                "permission": "thread.interact",
                "update": [{ "entry_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b", "expires_at": null }]
            }]
        }))
        .unwrap();
        let leaves: PageChange = serde_json::from_value(json!({
            "rows": [{
                "permission": "thread.interact",
                "update": [{ "entry_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b", "note": "for the audit" }]
            }]
        }))
        .unwrap();
        let expiry = |change: &PageChange| {
            change.rows.as_ref().unwrap()[0].update.as_ref().unwrap()[0].expires_at
        };
        assert_eq!(expiry(&clears), Some(None));
        assert_eq!(expiry(&leaves), None);
        assert_eq!(
            serde_json::to_value(&clears).unwrap()["rows"][0]["update"][0]["expires_at"],
            json!(null)
        );
        assert!(
            serde_json::to_value(&leaves).unwrap()["rows"][0]["update"][0]
                .get("expires_at")
                .is_none()
        );
    }

    #[test]
    fn a_holder_is_told_by_its_kind_and_names_no_people() {
        let written = json!({
            "kind": "role",
            "role": "developer",
            "at": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b", "kind": "project", "name": "Fix login", "redacted": false },
            "count": 14
        });
        let holder: Holder = serde_json::from_value(written.clone()).unwrap();
        assert!(matches!(
            holder,
            Holder::Role {
                place: None,
                role: Role::Developer,
                at: Some(_),
                count: 14
            }
        ));
        assert_eq!(serde_json::to_value(&holder).unwrap(), written);
        assert_eq!(
            serde_json::to_value(Holder::Everyone).unwrap(),
            json!({ "kind": "everyone" })
        );
    }

    #[test]
    fn a_page_says_what_its_reader_may_give_and_what_lies_inside_a_place() {
        let gives: PageGives = serde_json::from_value(json!({
            "choices": ["full_view", "full_interact"],
            "roles": ["viewer", "interactor"],
            "agents": ["view", "edit"],
            "holders_share": true
        }))
        .unwrap();
        assert_eq!(
            gives.choices,
            vec![AccessChoice::FullView, AccessChoice::FullInteract]
        );
        assert_eq!(gives.roles, vec![Role::Viewer, Role::Interactor]);
        assert_eq!(gives.agents, vec![AgentLevel::View, AgentLevel::Edit]);
        let covers: PageCovers = serde_json::from_value(json!({
            "places": 3, "threads": 40, "documents": 12,
            "apart": { "places": 1, "threads": 2, "documents": 0 }
        }))
        .unwrap();
        assert_eq!(covers.threads, 40);
        assert_eq!(covers.apart.places, 1);
        let row: PermissionRow = serde_json::from_value(json!({
            "permission": "thread.interact", "profile": "any", "inherit": true, "inherited": null,
            "entries": [], "locked": null, "writable": false, "can_give": true
        }))
        .unwrap();
        assert!(
            row.can_give && !row.writable,
            "giving is not managing the row"
        );
    }

    #[test]
    fn a_preview_of_the_systems_defaults_counts_the_orgs_and_any_other_says_null() {
        let system: PagePreview = serde_json::from_value(
            json!({ "pages": 40, "orgs": 3, "gaining": [], "losing": [], "more": 0 }),
        )
        .unwrap();
        assert_eq!(system.orgs, Some(3));
        let other = PagePreview {
            orgs: None,
            ..system
        };
        assert_eq!(serde_json::to_value(&other).unwrap()["orgs"], json!(null));
    }

    #[test]
    fn the_roles_a_caller_may_give_are_told_with_the_roles_held() {
        let reply: NodeRoleListReply = serde_json::from_value(json!({
            "owners": [], "own": [], "above": [], "can_give": ["viewer", "interactor", "developer"]
        }))
        .unwrap();
        assert_eq!(reply.can_give.last(), Some(&Role::Developer));
    }

    #[test]
    fn a_page_for_a_new_thread_is_a_preset_a_copy_or_a_change() {
        let preset: ThreadAccess = serde_json::from_value(json!("private")).unwrap();
        assert_eq!(preset, ThreadAccess::Preset(AccessPreset::Private));
        let copy: ThreadAccess =
            serde_json::from_value(json!({ "as": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b" }))
                .unwrap();
        assert!(matches!(copy, ThreadAccess::As(_)));
        let change: ThreadAccess = serde_json::from_value(
            json!({ "owners": { "first": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b" } }),
        )
        .unwrap();
        assert!(matches!(change, ThreadAccess::Change(_)));
        assert!(
            serde_json::from_value::<ThreadAccess>(
                json!({ "as": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b", "bogus": 1 })
            )
            .is_err()
        );
    }

    #[test]
    fn a_holder_named_by_identifier_may_be_a_role_in_another_place() {
        let input: HolderInput = serde_json::from_value(json!({ "kind": "role", "role": "viewer", "at": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b" })).unwrap();
        assert!(matches!(
            input,
            HolderInput::Role {
                role: Role::Viewer,
                at: Some(_)
            }
        ));
        assert!(serde_json::from_value::<HolderInput>(json!({ "kind": "group", "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b", "name": "x" })).is_err());
    }

    #[test]
    fn what_a_system_flag_grants_is_written_as_the_word_system() {
        let from: NodeOrSystem = serde_json::from_value(json!("system")).unwrap();
        assert_eq!(from, NodeOrSystem::SYSTEM);
        assert_eq!(
            serde_json::to_value(NodeOrSystem::SYSTEM).unwrap(),
            json!("system")
        );
    }
}
