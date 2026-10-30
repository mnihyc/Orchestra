//! The reader's complete hierarchy, read a level at a time, or searched by name with its authorized paths.

use orch_types::{Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    common::Page,
    nodes::{NodeRef, Tag},
    routes::Access::User,
    users::UserRef,
};

/// The usual page is small enough to open a branch without delaying the rest of the tree.
pub const EVERYTHING_LIMIT_DEFAULT: u32 = 50;
/// The largest branch page, including a search across all levels.
pub const EVERYTHING_LIMIT_MAX: u32 = 200;

/// Whose visible inventory is being read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EverythingScope {
    /// What the signed-in reader may see.
    #[default]
    Mine,
    /// Every org and resource; available only to a system administrator in a session.
    System,
}

/// The kinds of thing the complete hierarchy contains.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EverythingKind {
    /// An org, including a department or a person's own place.
    Org,
    /// A project or a folder inside another project.
    Project,
    /// A topic.
    Topic,
    /// A thread.
    Thread,
    /// A document.
    Document,
    /// A place's chat, a direct chat or a group chat.
    Chat,
    /// An environment.
    Environment,
    /// A provider.
    Provider,
    /// An agent profile.
    Agent,
    /// An MCP server or preset.
    McpServer,
    /// A machine.
    Machine,
    /// A host.
    Host,
    /// A mount.
    Mount,
    /// An environment or provider template.
    Template,
    /// A skill.
    Skill,
    /// A navigational group, identified by its `group` member.
    Group,
}

/// A group whose stable identifier can be supplied as `parent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EverythingGroup {
    /// Things shared from outside the reader's ordinary navigation.
    Shared,
    /// Resources kept together, regardless of their home.
    Resources,
}

/// The column used to order every level; identifier and kind break ties.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EverythingSort {
    /// The name, without regard to case.
    #[default]
    Name,
    /// The kind.
    Kind,
    /// When it was made.
    Created,
    /// Who made it.
    CreatedBy,
    /// Its first owner.
    Owner,
    /// When it was last worked in.
    LastActivity,
    /// When this reader last opened it.
    Opened,
    /// Its authorized location in the hierarchy.
    Where,
    /// The last recorded human document editor; unknown stands last.
    ActivityBy,
    /// Its current state.
    State,
    /// Its known stored bytes.
    Size,
    /// What the reader holds.
    MyAccess,
    /// Its tag names.
    Tags,
    /// A host's declared memory as a fraction of capacity; unknown stands last.
    Load,
    /// A host's running or starting environments and providers, together.
    Running,
    /// Recorded real uses, in the resource kind's own unit; unrecorded history stands last.
    Use,
}

/// The source of a flat recent listing across every level of its beginning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EverythingRecent {
    /// Things this reader opened, ordered by the last visit unless another column is chosen.
    Opened,
    /// Threads and documents worked in by anyone, including agents.
    Active,
}

/// The direction of an order; unknown values always stand last.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EverythingDirection {
    /// Smallest or earliest first.
    #[default]
    Asc,
    /// Largest or latest first.
    Desc,
}

/// The reader's own level, rather than somebody else's role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EverythingAccess {
    /// The reader may view it.
    View,
    /// The reader may contribute, edit or use it, according to its kind.
    TakePart,
    /// The reader may manage it.
    Manage,
    /// Only this step on the way to something readable; no content or metadata is supplied.
    PathOnly,
}

/// Recorded resource requirements; an unknown measurement stays null.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EverythingTakes {
    /// Declared CPU cores.
    pub cpus: Option<f64>,
    /// Declared memory in bytes.
    pub memory_bytes: Option<i64>,
    /// Declared storage in bytes.
    pub storage_bytes: Option<i64>,
}

/// One visible thing, or a deliberately minimal path step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EverythingRow {
    /// Batched current giving and reported use, supplied with readable resource list rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library: Option<crate::resource_facts::ResourceFacts>,
    /// Distinct historical threads on a provider, from its existing usage ledger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library_threads: Option<i64>,
    /// A readable thread's icon and safe particulars; absent for other kinds or without thread View.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_display: Option<crate::threads::ThreadDisplay>,
    /// The existing readable resource view, requested explicitly for a resource list.
    /// Never supplied for a name-only or path-only row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<EverythingResource>,
    /// Native reach alone opens this object; the same marker as its page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admin_access: Option<crate::access::AdminAccess>,
    /// A personal/group assignment here or below, or a current follow. This is
    /// independent of both ordinary View and native administrator reach.
    #[serde(default, skip_serializing_if = "crate::common::is_false")]
    pub participating: bool,
    /// Its UUID, or `shared` / `resources` for a navigational group.
    pub id: String,
    /// What it is.
    pub kind: EverythingKind,
    /// Which navigational group this is; null for an actual thing.
    pub group: Option<EverythingGroup>,
    /// Its name; null when the finder would redact this path step too.
    pub name: Option<String>,
    /// Its authorized place label, to distinguish names repeated in different orgs.
    pub place_label: Option<String>,
    /// Its actual parent, or the group's stable identifier.
    pub parent_id: Option<String>,
    /// Whether opening the branch can reveal any readable descendants.
    pub has_children: bool,
    /// The path the finder would show; a redacted step has no name.
    pub path: Vec<NodeRef>,
    /// When it was made; null for a path step or a group.
    pub created_at: Option<Time>,
    /// Its recorded maker; null when unknown or withheld.
    pub created_by: Option<UserRef>,
    /// Its first owner; null when unknown or withheld.
    pub owner: Option<UserRef>,
    /// Its last recorded activity; null when unknown or withheld.
    pub last_activity: Option<Time>,
    /// This reader's last visit; absent when never opened or when content is withheld.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_opened: Option<Time>,
    /// The last recorded human document editor; absent for agent or unknown activity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_by: Option<UserRef>,
    /// Whether a readable document changed since this reader last saw it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_unread: Option<bool>,
    /// A thread's or resource's state; null when unknown or withheld for a path step.
    pub state: Option<String>,
    /// Known stored bytes; a place totals readable documents only, never private storage.
    /// Null when unknown or withheld.
    pub size_bytes: Option<i64>,
    /// Recorded requirements; null when unknown or withheld.
    pub takes: Option<EverythingTakes>,
    /// The reader's own level.
    pub my_access: EverythingAccess,
    /// Tags whose definitions the reader may see.
    pub tags: Vec<Tag>,
    /// Where Open leads; null for a group or a path step.
    pub href: Option<String>,
    /// Whether this reader may offer Share.
    pub can_share: bool,
    /// Where this place's settings lead; null unless the reader holds the required permission.
    pub settings_href: Option<String>,
}

/// One branch, or matches at every level when `q` is supplied.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EverythingListQuery {
    /// A flat listing at every level, without requiring a name query.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recent: Option<EverythingRecent>,
    /// Apply Navigation's kinds and hide descendants of threads when Threads is not shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub navigation: Option<bool>,
    /// Include archived things and their descendants; omission preserves the ordinary inventory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_archived: Option<bool>,
    /// Hide done topics and their contents unless unread work or a decision keeps them visible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hide_done: Option<bool>,
    /// Keep the current thing's path visible while done topics are hidden; grants no access.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep: Option<Id>,
    /// Include the existing resource view for each readable row, bounded by this page.
    /// Requires exactly one resource kind and parent=resources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_details: Option<bool>,
    /// A resource's own state, applied before the page limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_state: Option<String>,
    /// An agent's harness, applied before the page limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    /// A template's unit kind, or an environment's origin (machine/template).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_kind: Option<String>,
    /// Nested ownership, management, use and view of the selected resource kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_view: Option<crate::resources::ProviderListScope>,
    /// Only resources at home in this place or below it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<Id>,
    /// Select only native administrator reach, or only ordinary rows when false; before paging.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admin_access: Option<bool>,
    /// Only personal/group assignments here or below and followed conversations; before paging.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub participating: Option<bool>,
    /// A UUID, `shared` or `resources`; absent means the top level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// The reader's visible inventory by default; `system` requires a system administrator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<EverythingScope>,
    /// A comma list of kinds; absent means every kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<String>,
    /// Only things whose first owner is this person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<Id>,
    /// Only things made on or after this moment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_from: Option<Time>,
    /// Only things made on or before this moment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_to: Option<Time>,
    /// Only things active on or after this moment, including work below a place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_from: Option<Time>,
    /// Literal words or quoted phrases in a name, at every level, with its authorized path.
    /// Resource lists also match the visible owner and home labels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    /// The column to sort by; name by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<EverythingSort>,
    /// Ascending by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<EverythingDirection>,
    /// The next cursor of this same branch, filters and order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Between one and 200; 50 by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// The same resource views returned by their own readers, with a discriminator for shared lists.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum EverythingResource {
    /// An environment with its visible bindings and acts.
    Environment(Box<crate::environment::Environment>),
    /// A provider with its current status and plan.
    Provider(Box<crate::resources::Provider>),
    /// A harness profile.
    Agent(Box<crate::resources::Agent>),
    /// A physical machine, using its host's policy where applicable.
    Machine(Box<crate::resources::Machine>),
    /// A host with its declared load and running units.
    Host(Box<crate::resources::Host>),
    /// An environment or provider template.
    Template(Box<crate::resources::Template>),
    /// A preset or imported MCP server.
    McpServer(Box<crate::resources::McpServer>),
    /// A mount with the locations this caller may see.
    Mount(Box<crate::mounts::Mount>),
    /// A skill with its latest version.
    Skill(Box<crate::skills::Skill>),
}

/// Counts of the resource lists this caller may discover, including named permission pages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResourceCounts {
    /// Providers.
    pub providers: i64,
    /// Agent profiles.
    pub agents: i64,
    /// Physical machines, excluding runtime instances.
    pub machines: i64,
    /// Environments.
    pub environments: i64,
    /// Templates.
    pub templates: i64,
    /// Hosts.
    pub hosts: i64,
    /// Independently registered MCP servers.
    pub mcp: i64,
    /// Mounts.
    pub mounts: i64,
    /// Skills.
    pub skills: i64,
}

crate::endpoints! {
    /// Counts the caller's discoverable resources without inspecting their runtimes or reading their contents.
    ResourcesSummaryGet: GET "/resources/summary", User, none, none, json ResourceCounts, 200;
    /// Lists a level of everything the reader may see, or searches names at every level with their authorized paths.
    EverythingList: GET "/everything", User, query EverythingListQuery, none, json Page<EverythingRow>, 200;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_in_clients_can_discover_the_inventory_in_the_public_api() {
        let routes = crate::routes::all();
        let route = routes
            .iter()
            .find(|route| route.method.as_str() == "GET" && route.path == "/everything")
            .expect("the public route collection must expose the inventory to generated clients");
        assert_eq!(route.access, crate::routes::Access::User);
    }

    #[test]
    fn a_path_step_carries_no_details_and_keeps_a_redacted_name_redacted() {
        let value = serde_json::json!({"id":"0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a01","kind":"topic","group":null,"name":null,"place_label":null,"parent_id":"shared","has_children":true,"path":[],"created_at":null,"created_by":null,"owner":null,"last_activity":null,"state":null,"size_bytes":null,"takes":null,"my_access":"path_only","tags":[],"href":null,"can_share":false,"settings_href":null});
        let row: EverythingRow = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(row.my_access, EverythingAccess::PathOnly);
        assert_eq!(serde_json::to_value(row).unwrap(), value);
    }

    #[test]
    fn unknown_filters_are_refused_instead_of_silently_widening_the_list() {
        assert!(
            serde_json::from_value::<EverythingListQuery>(serde_json::json!({"orgs":"all"}))
                .is_err()
        );
        assert!(
            serde_json::from_value::<EverythingListQuery>(serde_json::json!({"scope":"everyone"}))
                .is_err()
        );
        assert!(
            serde_json::from_value::<EverythingListQuery>(serde_json::json!({"recent":"mixed"}))
                .is_err()
        );
    }

    #[test]
    fn recent_uses_the_same_paged_listing_with_explicit_navigation_choices() {
        let value = serde_json::json!({"recent":"opened","navigation":true,"show_archived":false,"hide_done":true,"sort":"opened","direction":"desc","kinds":"thread,document"});
        let query: EverythingListQuery = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(query.recent, Some(EverythingRecent::Opened));
        assert_eq!(query.sort, Some(EverythingSort::Opened));
        assert_eq!(serde_json::to_value(query).unwrap(), value);
    }
}
