//! Closed permission catalogue: areas and levels on nodes, on documents and on resources, an environment's two levels among them, the advanced rows for one act each (owning a thing, naming somebody from outside, letting a turn past a limit among them), the options a permission takes, and the three everyday choices.
//!
//! A [`Permission`] is an [`Area`] at a [`Level`], or an advanced row for one
//! act, on one kind of [object](ObjectKind). The catalogue is closed: every
//! permission there is, is a variant, and everything a page, a seed or a
//! screen needs to know of one is asked of it here: where it exists, what
//! holding it also gives, which role a new installation names in it, which
//! kinds of holder its rows take and which options an entry in it may carry.
//!
//! Inside an area the levels nest, and whoever holds the level an advanced
//! row belongs with holds that row too ([`Permission::held_through`]).
//! Areas never imply each other. The three everyday [`Choice`]s name several
//! rows at once and add no meaning of their own: what is stored is the rows.

use std::collections::BTreeMap;

use orch_types::fields::FieldError;
use orch_types::{Id, Json};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::roles::{NodeKind, PlaceKind, Role, UnknownName, words};

words! {
    /// A kind of resource: something people bring, that work runs on or in.
    pub enum ResourceKind ("kind of resource") {
        /// A provider: a credential with the runtime that runs sessions on it.
        Provider = "provider",
        /// An agent profile.
        Agent = "agent",
        /// An environment, whatever it runs on.
        Environment = "environment",
        /// A host that environments and runtimes are made on.
        Host = "host",
        /// A physical machine whose directories and programs can be shared and managed.
        Machine = "machine",
        /// A template that an environment or a provider's runtime is made from.
        Template = "template",
        /// An MCP server: a preset, or one imported from a machine.
        McpServer = "mcp_server",
        /// A mount: data attached to environments.
        Mount = "mount",
        /// A skill: a directory of instructions, scripts and assets that agents are given.
        Skill = "skill",
    }
}

impl ResourceKind {
    /// The area of its permissions.
    #[must_use]
    pub const fn area(self) -> Area {
        match self {
            Self::Provider => Area::Provider,
            Self::Agent => Area::Agent,
            Self::Environment => Area::Environment,
            Self::Host => Area::Host,
            Self::Machine => Area::Machine,
            Self::Template => Area::Template,
            Self::McpServer => Area::Mcp,
            Self::Mount => Area::Mount,
            Self::Skill => Area::Skill,
        }
    }

    /// Whether putting work on it takes a level between looking and
    /// managing. Not for an environment: putting a thread in one takes its
    /// Manage, since its agent reaches all of it; its Use row is asked only of
    /// the people who give input to the agents of the threads that run there.
    #[must_use]
    pub const fn has_use(self) -> bool {
        !matches!(self, Self::Environment)
    }

    /// Whether one of this kind can be the system's own, provided by its
    /// admins for everybody and without a home. A provider always has a
    /// home, an environment a home node, and a skill the org or the project
    /// it belongs to.
    #[must_use]
    pub const fn can_be_the_systems(self) -> bool {
        !matches!(self, Self::Provider | Self::Environment | Self::Skill)
    }
}

words! {
    /// A kind of thing that has a permission page.
    #[derive(PartialOrd, Ord)]
    pub enum ObjectKind ("kind of object") {
        /// An org, at any level.
        Org = "org",
        /// A project.
        Project = "project",
        /// A topic.
        Topic = "topic",
        /// A thread.
        Thread = "thread",
        /// A document: written text beside a node, with a page of its own
        /// that follows the node's document rows.
        Document = "document",
        /// A document-only folder with its own document permission page.
        DocumentContainer = "document_container",
        /// A provider.
        Provider = "provider",
        /// An agent profile.
        Agent = "agent",
        /// An environment.
        Environment = "environment",
        /// A host.
        Host = "host",
        /// A physical machine, including one that also offers the node role.
        Machine = "machine",
        /// A template.
        Template = "template",
        /// An MCP server.
        McpServer = "mcp_server",
        /// A mount.
        Mount = "mount",
        /// A skill.
        Skill = "skill",
    }
}

impl ObjectKind {
    /// Both text and folders use the document permission family.
    #[must_use]
    pub const fn is_document(self) -> bool {
        matches!(self, Self::Document | Self::DocumentContainer)
    }
    /// The kind of node it is, where it is one.
    #[must_use]
    pub const fn node(self) -> Option<NodeKind> {
        match self {
            Self::Org => Some(NodeKind::Org),
            Self::Project => Some(NodeKind::Project),
            Self::Topic => Some(NodeKind::Topic),
            Self::Thread => Some(NodeKind::Thread),
            _ => None,
        }
    }

    /// The kind of resource it is, where it is one.
    #[must_use]
    pub const fn resource(self) -> Option<ResourceKind> {
        match self {
            Self::Provider => Some(ResourceKind::Provider),
            Self::Agent => Some(ResourceKind::Agent),
            Self::Environment => Some(ResourceKind::Environment),
            Self::Host => Some(ResourceKind::Host),
            Self::Machine => Some(ResourceKind::Machine),
            Self::Template => Some(ResourceKind::Template),
            Self::McpServer => Some(ResourceKind::McpServer),
            Self::Mount => Some(ResourceKind::Mount),
            Self::Skill => Some(ResourceKind::Skill),
            _ => None,
        }
    }

    /// The permissions that exist on a thing of this kind: the rows of its
    /// page, in the order of the catalogue, the Owners row among them.
    pub fn permissions(self) -> impl Iterator<Item = Permission> {
        Permission::ALL
            .iter()
            .copied()
            .filter(move |permission| permission.exists_on(self))
    }

    /// Whether a thing of this kind can be made inside a place of kind
    /// `place`, at any depth, so that its page follows that place's
    /// defaults. A document follows the rows of its node, which is why it is
    /// in a place wherever a node is; a resource's home is an org, a skill's
    /// an org or a project, and an environment's home node an org, a project
    /// or a topic.
    #[must_use]
    pub const fn made_in(self, place: PlaceKind) -> bool {
        match (place, self) {
            (PlaceKind::System | PlaceKind::Org, _) => true,
            (PlaceKind::Project, Self::Topic | Self::Skill)
            | (
                PlaceKind::Project | PlaceKind::Topic,
                Self::Thread | Self::Document | Self::DocumentContainer | Self::Environment,
            ) => true,
            (PlaceKind::Project | PlaceKind::Topic, _) => false,
        }
    }
}

impl From<NodeKind> for ObjectKind {
    fn from(kind: NodeKind) -> Self {
        match kind {
            NodeKind::Org => Self::Org,
            NodeKind::Project => Self::Project,
            NodeKind::Topic => Self::Topic,
            NodeKind::Thread => Self::Thread,
        }
    }
}

impl From<ResourceKind> for ObjectKind {
    fn from(kind: ResourceKind) -> Self {
        match kind {
            ResourceKind::Provider => Self::Provider,
            ResourceKind::Agent => Self::Agent,
            ResourceKind::Environment => Self::Environment,
            ResourceKind::Host => Self::Host,
            ResourceKind::Machine => Self::Machine,
            ResourceKind::Template => Self::Template,
            ResourceKind::McpServer => Self::McpServer,
            ResourceKind::Mount => Self::Mount,
            ResourceKind::Skill => Self::Skill,
        }
    }
}

words! {
    /// What a permission is about. Holding one area says nothing about
    /// another.
    pub enum Area ("area") {
        /// The chat of a node.
        Chat = "chat",
        /// The documents beside a node, and one document.
        Doc = "doc",
        /// A thread's session: its conversation, driving it, setting it up.
        Thread = "thread",
        /// The files of an environment's workspace: one of its four doors.
        Files = "files",
        /// The processes and shells of an environment: one of its four doors.
        Terminal = "terminal",
        /// The browsers of an environment, and the profiles it keeps: one of
        /// its four doors.
        Browser = "browser",
        /// The displays served in an environment: one of its four doors.
        Vnc = "vnc",
        /// The owned ports of an environment and their wider audiences.
        Ports = "ports",
        /// A topic.
        Topic = "topic",
        /// A project.
        Project = "project",
        /// An org.
        Org = "org",
        /// Policies, locks, secrets and destinations of a node.
        Settings = "settings",
        /// What was used in a node, and its caps.
        Usage = "usage",
        /// The audit trail of a node.
        Audit = "audit",
        /// The people of an org.
        Members = "members",
        /// The direct and group chats among an org's members.
        Directs = "directs",
        /// Who holds what on a thing, and who owns it.
        Access = "access",
        /// A provider.
        Provider = "provider",
        /// An agent profile.
        Agent = "agent",
        /// An environment.
        Environment = "environment",
        /// A host.
        Host = "host",
        /// A physical machine and its policy.
        Machine = "machine",
        /// A template.
        Template = "template",
        /// An MCP server.
        Mcp = "mcp",
        /// A mount.
        Mount = "mount",
        /// A skill.
        Skill = "skill",
    }
}

impl Area {
    /// Whether it is one of the four doors of an environment: Files, Terminal,
    /// Browsers and VNC. Each lets somebody in who does not manage the
    /// environment, and whoever manages it holds the Manage of all four.
    #[must_use]
    pub const fn is_door(self) -> bool {
        matches!(
            self,
            Self::Files | Self::Terminal | Self::Browser | Self::Vnc | Self::Ports
        )
    }

    /// Its name, as a page shows it.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Chat => "Chat",
            Self::Doc => "Documents",
            Self::Thread => "Thread",
            Self::Files => "Files",
            Self::Terminal => "Terminal",
            Self::Browser => "Browsers",
            Self::Vnc => "VNC",
            Self::Ports => "Ports",
            Self::Topic => "Topic",
            Self::Project => "Project",
            Self::Org => "Org",
            Self::Settings => "Settings",
            Self::Usage => "Usage",
            Self::Audit => "Audit",
            Self::Members => "Members",
            Self::Directs => "Direct chats",
            Self::Access => "Access",
            Self::Provider => "Provider",
            Self::Agent => "Agent profile",
            Self::Environment => "Environment",
            Self::Host => "Host",
            Self::Machine => "Machine",
            Self::Template => "Template",
            Self::Mcp => "MCP server",
            Self::Mount => "Mount",
            Self::Skill => "Skill",
        }
    }
}

words! {
    /// How far a permission goes in its area. The levels nest: whoever holds
    /// one holds those below it. An advanced row stands apart from them, for
    /// one act.
    pub enum Level ("level") {
        /// Looking.
        View = "view",
        /// Taking part: the middle level of a chat and of a node.
        Interact = "interact",
        /// Writing: the middle level of documents.
        Edit = "edit",
        /// Using: the middle level of a resource.
        Use = "use",
        /// Managing.
        Manage = "manage",
        /// Owning: above managing, in the area of access alone.
        Own = "own",
        /// A row for one act, apart from the levels.
        Advanced = "advanced",
    }
}

impl Level {
    /// Its name, as a page shows it.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::View => "View",
            Self::Interact => "Interact",
            Self::Edit => "Edit",
            Self::Use => "Use",
            Self::Manage => "Manage",
            Self::Own => "Own",
            Self::Advanced => "Advanced",
        }
    }

    /// Its height among the levels; nothing for an advanced row. The three
    /// middle levels are one height: an area has at most one of them.
    const fn height(self) -> Option<u8> {
        match self {
            Self::View => Some(1),
            Self::Interact | Self::Edit | Self::Use => Some(2),
            Self::Manage => Some(3),
            Self::Own => Some(4),
            Self::Advanced => None,
        }
    }
}

words! {
    /// Where work runs: the two kinds of environment whose defaults differ.
    #[derive(Default)]
    pub enum Placement ("placement") {
        /// An environment made from a template on a host. A thread that has
        /// no environment yet counts as this.
        #[default]
        Managed = "managed",
        /// A directory on a machine somebody connected.
        Connected = "connected",
    }
}

words! {
    /// Which row of a defaults page a row is: the one row of its permission,
    /// or one of the two that a permission has when its default depends on
    /// where work runs. A row of a thing's own page is always `any`.
    pub enum Profile ("profile") {
        /// The permission has one row.
        Any = "any",
        /// The row for a managed environment.
        Managed = "managed",
        /// The row for a person's machine.
        Connected = "connected",
    }
}

impl From<Placement> for Profile {
    fn from(placement: Placement) -> Self {
        match placement {
            Placement::Managed => Self::Managed,
            Placement::Connected => Self::Connected,
        }
    }
}

words! {
    /// The kinds of holder a row can name.
    pub enum HolderKind ("kind of holder") {
        /// Everyone whose role in a place is that one or a higher one.
        Role = "role",
        /// The members of a group.
        Group = "group",
        /// Members whose domain is this org or an org inside it.
        Members = "members",
        /// One person.
        User = "user",
        /// The work in a place: threads there run on the resource. It names
        /// no person.
        Node = "node",
        /// The agents of the threads at or under a place. It names no person.
        Agents = "agents",
        /// Every user of the system.
        Everyone = "everyone",
    }
}

impl HolderKind {
    /// Whether the row of `permission` on the page of a thing of kind
    /// `object` takes a holder of this kind. `of_system` says that the thing
    /// is a resource of the system, the one place where everyone can be
    /// named.
    ///
    /// The Owners row takes people and groups. Every other row takes a role,
    /// a group and a person; the work in a place stands in the Use row of a
    /// resource that has one; the agents of a place stand in the rows by
    /// which conversations, chats and documents are read and written, and in
    /// the one by which documents are managed.
    #[must_use]
    pub fn fits(self, permission: Permission, object: ObjectKind, of_system: bool) -> bool {
        match self {
            Self::Everyone => {
                let systems = object
                    .resource()
                    .is_some_and(ResourceKind::can_be_the_systems);
                of_system && systems && permission != Permission::AccessOwn
            }
            _ => self.fits_defaults(permission),
        }
    }

    /// Whether the row of `permission` on a defaults page, or on a page of a
    /// thing that is not the system's, takes a holder of this kind.
    #[must_use]
    pub fn fits_defaults(self, permission: Permission) -> bool {
        if permission == Permission::AccessOwn {
            return matches!(self, Self::User | Self::Group);
        }
        match self {
            Self::Role | Self::Group | Self::User | Self::Members => true,
            Self::Node => matches!(permission.level(), Level::Use),
            Self::Agents => permission.shared_with_agents() || permission.is_export(),
            Self::Everyone => false,
        }
    }
}

/// The kinds of object the rows of every node stand on.
const NODES: &[ObjectKind] = &[
    ObjectKind::Org,
    ObjectKind::Project,
    ObjectKind::Topic,
    ObjectKind::Thread,
];
/// Every node, and every document.
const NODES_AND_DOCUMENTS: &[ObjectKind] = &[
    ObjectKind::Org,
    ObjectKind::Project,
    ObjectKind::Topic,
    ObjectKind::Thread,
    ObjectKind::Document,
    ObjectKind::DocumentContainer,
];
const DOCUMENT_PARENTS: &[ObjectKind] = &[
    ObjectKind::Org,
    ObjectKind::Project,
    ObjectKind::Topic,
    ObjectKind::Thread,
    ObjectKind::DocumentContainer,
];
/// Everything that has a page.
const EVERYTHING: &[ObjectKind] = ObjectKind::ALL;
const ORG: &[ObjectKind] = &[ObjectKind::Org];
const PROJECT: &[ObjectKind] = &[ObjectKind::Project];
const TOPIC: &[ObjectKind] = &[ObjectKind::Topic];
const THREAD: &[ObjectKind] = &[ObjectKind::Thread];
const PROVIDER: &[ObjectKind] = &[ObjectKind::Provider];
const AGENT: &[ObjectKind] = &[ObjectKind::Agent];
const ENVIRONMENT: &[ObjectKind] = &[ObjectKind::Environment];
const HOST: &[ObjectKind] = &[ObjectKind::Host];
const MACHINE: &[ObjectKind] = &[ObjectKind::Machine];
const TEMPLATE: &[ObjectKind] = &[ObjectKind::Template];
const MCP_SERVER: &[ObjectKind] = &[ObjectKind::McpServer];
const MOUNT: &[ObjectKind] = &[ObjectKind::Mount];
const SKILL: &[ObjectKind] = &[ObjectKind::Skill];

/// Whom a new installation names in a row: for a managed environment, and
/// for a person's machine. `None` is nobody, which leaves the row to the
/// owners.
type Ships = [Option<Role>; 2];
const VIEWERS: Ships = [Some(Role::Viewer); 2];
const INTERACTORS: Ships = [Some(Role::Interactor); 2];
const DEVELOPERS: Ships = [Some(Role::Developer); 2];
const ADMINS: Ships = [Some(Role::Admin); 2];
const NOBODY: Ships = [None; 2];
/// Interactors where the environment is managed; on a person's machine nobody.
const INTERACTORS_WHERE_MANAGED: Ships = [Some(Role::Interactor), None];
/// Developers where the environment is managed; on a person's machine nobody.
const DEVELOPERS_WHERE_MANAGED: Ships = [Some(Role::Developer), None];

macro_rules! permissions {
    ( $( $variant:ident = $key:literal, $area:ident, $level:ident, $on:ident, $ships:ident, $label:literal, $allows:literal; )* ) => {
        /// A permission of the catalogue: an area at a level, or an advanced
        /// row for one act. It is written as its key, `thread.interact`.
        ///
        /// Permissions compare as their keys do, so that a sorted list of them
        /// reads in the order of their names.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
        pub enum Permission {
            $( #[doc = $allows] #[serde(rename = $key)] $variant, )*
        }

        impl Permission {
            /// Every permission, in the order of the catalogue: those on
            /// nodes and documents, then those of each kind of resource.
            pub const ALL: &'static [Permission] = &[ $( Permission::$variant, )* ];

            /// Its key, as it is written and stored.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self { $( Permission::$variant => $key, )* }
            }

            /// The area it belongs to.
            #[must_use]
            pub const fn area(self) -> Area {
                match self { $( Permission::$variant => Area::$area, )* }
            }

            /// Its level in that area, or that it is an advanced row.
            #[must_use]
            pub const fn level(self) -> Level {
                match self { $( Permission::$variant => Level::$level, )* }
            }

            /// The kinds of object it exists on.
            #[must_use]
            pub const fn on(self) -> &'static [ObjectKind] {
                match self { $( Permission::$variant => $on, )* }
            }

            /// What a page calls its row.
            #[must_use]
            pub const fn label(self) -> &'static str {
                match self { $( Permission::$variant => $label, )* }
            }

            /// What it lets a person do, in a sentence.
            #[must_use]
            pub const fn allows(self) -> &'static str {
                match self { $( Permission::$variant => $allows, )* }
            }

            const fn ships(self) -> Ships {
                match self { $( Permission::$variant => $ships, )* }
            }
        }

        impl std::str::FromStr for Permission {
            type Err = UnknownName;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                match text {
                    $( $key => Ok(Permission::$variant), )*
                    _ => Err(UnknownName::new("permission", text)),
                }
            }
        }
    };
}

permissions! {
    ChatView = "chat.view", Chat, View, NODES, VIEWERS,
        "View Chat", "Read the chat of the node and follow it.";
    ChatInteract = "chat.interact", Chat, Interact, NODES, VIEWERS,
        "Interact Chat", "Write in the chat of the node: post, react, pin, forward into it, and change or delete what one wrote oneself.";
    ChatManage = "chat.manage", Chat, Manage, NODES, DEVELOPERS,
        "Manage Chat", "Delete what anybody wrote in the chat of the node.";
    DocView = "doc.view", Doc, View, NODES_AND_DOCUMENTS, VIEWERS,
        "View Documents", "Read the documents beside the node, or this document: its text, earlier versions, discussions and files.";
    DocEdit = "doc.edit", Doc, Edit, NODES_AND_DOCUMENTS, INTERACTORS,
        "Edit Documents", "Write the documents beside the node, or this document: its title, its blocks and the files it holds.";
    DocCreate = "doc.create", Doc, Advanced, DOCUMENT_PARENTS, VIEWERS,
        "New documents", "Make a document beside the node; whoever makes one owns it.";
    DocComment = "doc.comment", Doc, Advanced, NODES_AND_DOCUMENTS, VIEWERS,
        "Comment", "Discuss a document: start a discussion, answer in one, react and resolve.";
    DocManage = "doc.manage", Doc, Manage, NODES_AND_DOCUMENTS, DEVELOPERS,
        "Manage Documents", "Manage the documents beside the node, or this document: put an earlier version back, move it, delete and restore it, and delete anybody's comment.";
    ThreadView = "thread.view", Thread, View, THREAD, VIEWERS,
        "View Thread", "See the thread and read its conversation, its state and its usage; follow it.";
    ThreadInteract = "thread.interact", Thread, Interact, THREAD, INTERACTORS_WHERE_MANAGED,
        "Interact Thread", "Drive the agent: give it input, steer or interrupt a running turn, stop or retry one, put in order what waits, and set the thread's tags.";
    ThreadManage = "thread.manage", Thread, Manage, THREAD, DEVELOPERS_WHERE_MANAGED,
        "Manage Thread", "Set the session up: its name, model and effort, the agent's parameters and instructions, its approval mode, its provider and profile; answer what the agent asks; delete the thread.";
    ThreadFork = "thread.fork", Thread, Advanced, THREAD, VIEWERS,
        "Fork", "Take a fork of the thread: a new thread of one's own, with as much of its history as one chooses.";
    ThreadArchive = "thread.archive", Thread, Advanced, THREAD, NOBODY,
        "Archive", "Archive the thread, and bring it back.";
    TopicView = "topic.view", Topic, View, TOPIC, VIEWERS,
        "View Topic", "See the topic: its name, description, status, tags and the settings that are no secret, and who has access.";
    TopicInteract = "topic.interact", Topic, Interact, TOPIC, DEVELOPERS,
        "Contribute to Topic", "Start threads in the topic, announce to its threads, set its status and tags, and create an environment for work there.";
    TopicManage = "topic.manage", Topic, Manage, TOPIC, DEVELOPERS,
        "Manage Topic", "Rename the topic, change its description, its parameters and its default environment, delete it, and restore what was deleted in it.";
    TopicArchive = "topic.archive", Topic, Advanced, TOPIC, NOBODY,
        "Archive", "Archive the topic, and bring it back.";
    TopicStartThreads = "topic.start_threads", Topic, Advanced, TOPIC, NOBODY,
        "Start threads", "Start threads in the topic; whoever starts one owns it.";
    ProjectView = "project.view", Project, View, PROJECT, VIEWERS,
        "View Project", "See the project: its name, description and the settings that are no secret, and who has access.";
    ProjectInteract = "project.interact", Project, Interact, PROJECT, DEVELOPERS,
        "Contribute to Project", "Create topics, announce to the open topics of the project, set its tags, and create an environment for work there.";
    ProjectManage = "project.manage", Project, Manage, PROJECT, ADMINS,
        "Manage Project", "Rename the project, change its description and parameters, move it to another org of its tree, delete it, and restore what was deleted in it.";
    ProjectArchive = "project.archive", Project, Advanced, PROJECT, NOBODY,
        "Archive", "Archive the project, and bring it back.";
    ProjectCreateTopics = "project.create_topics", Project, Advanced, PROJECT, NOBODY,
        "Create topics", "Create topics in the project; whoever creates one owns it.";
    OrgView = "org.view", Org, View, ORG, VIEWERS,
        "View Org", "See the org: its name, its members and groups, the orgs inside it and the settings that are no secret.";
    OrgInteract = "org.interact", Org, Interact, ORG, ADMINS,
        "Contribute to Org", "Create projects, announce to the open topics of the org, set its tags, and create an environment for work there.";
    OrgManage = "org.manage", Org, Manage, ORG, ADMINS,
        "Manage Org", "Rename the org, change its parameters and tags, create orgs inside it, move it within its tree, delete it, and restore what was deleted in it.";
    OrgArchive = "org.archive", Org, Advanced, ORG, NOBODY,
        "Archive", "Archive the org with everything inside it, and bring it back.";
    SettingsManage = "settings.manage", Settings, Manage, NODES, ADMINS,
        "Manage Settings", "Write the policies and locks of the node, its secrets, and where what happens there is sent.";
    UsageView = "usage.view", Usage, View, NODES, ADMINS,
        "View Usage", "Read every record of use in the node and below it, and the caps defined there.";
    UsageSummary = "usage.summary", Usage, Advanced, NODES, VIEWERS,
        "Usage summary", "See how much was used in the node as figures by person, model, provider, agent and kind of environment, without anybody else's records.";
    UsageManage = "usage.manage", Usage, Manage, NODES, ADMINS,
        "Manage Usage", "Add, change and remove the usage caps of the node.";
    UsageBypass = "usage.bypass", Usage, Advanced, NODES, ADMINS,
        "Go past limits", "Let one turn run past a cap that is defined on the node and has no room left.";
    AuditView = "audit.view", Audit, View, NODES, ADMINS,
        "View Audit", "Read the audit trail of the node and of what is inside it.";
    MembersManage = "members.manage", Members, Manage, ORG, ADMINS,
        "Manage Members", "Invite people into the org, remove, suspend and move them, and manage its groups and service accounts.";
    DirectsView = "directs.view", Directs, View, ORG, ADMINS,
        "View Direct chats", "Read the direct and group chats among the org's members without being in them.";
    AccessManage = "access.manage", Access, Manage, EVERYTHING, NOBODY,
        "Manage Access", "Change who holds what on the thing, and on a place also its defaults and its roles; decide requests for access; share it with agents.";
    AccessOwn = "access.own", Access, Own, EVERYTHING, NOBODY,
        "Own", "Be one of its owners: hold everything on its page, and say who else owns it and who else may change the page.";
    AccessExternal = "access.external", Access, Advanced, EVERYTHING, DEVELOPERS,
        "Share outside", "Name on its page somebody who is no member of its root org.";
    // The Export rows (the export of a place): taking a thing away as a file. As shipped whoever reads a thing
    // holds its row, and one of the three choices names its holder here too.
    ThreadExport = "thread.export", Thread, Advanced, THREAD, VIEWERS,
        "Export", "Take the conversation of the thread away as a file: its own export, its place in an archive, a copy given to an agent, and an agent's download of it.";
    ChatExport = "chat.export", Chat, Advanced, NODES, VIEWERS,
        "Export", "Take the chat of the node away as a file: its own export, its place in an archive, a copy given to an agent, and an agent's download of it.";
    DocExport = "doc.export", Doc, Advanced, NODES_AND_DOCUMENTS, VIEWERS,
        "Export", "Take the documents beside the node, or this document, away as files: its own export, its place in an archive, a copy given to an agent, and an agent's download of it.";
    ProviderView = "provider.view", Provider, View, PROVIDER, NOBODY,
        "View Provider", "See the provider in lists, with its status, plan and limits, and what each thread used of it.";
    ProviderUse = "provider.use", Provider, Use, PROVIDER, NOBODY,
        "Use Provider", "Run turns on the provider: put a thread on it, and give input to threads that run on it, with the models and efforts the entry allows.";
    ProviderManage = "provider.manage", Provider, Manage, PROVIDER, NOBODY,
        "Manage Provider", "Sign the provider in and out, rotate its key, say which models and arrangements it allows, run and update its runtime, and enable, disable and delete it.";
    ProviderBypass = "provider.bypass", Provider, Advanced, PROVIDER, NOBODY,
        "Go past limits", "Let one turn run past a cap on the provider that has no room left, or past the limit of its plan.";
    AgentView = "agent.view", Agent, View, AGENT, NOBODY,
        "View Agent profile", "See the agent profile in lists, with what its adapter declares.";
    AgentUse = "agent.use", Agent, Use, AGENT, NOBODY,
        "Use Agent profile", "Put a thread on the profile, and sign a subscription in through it.";
    AgentManage = "agent.manage", Agent, Manage, AGENT, NOBODY,
        "Manage Agent profile", "Change the profile and its overrides, and enable, disable and delete it.";
    EnvironmentView = "environment.view", Environment, View, ENVIRONMENT, VIEWERS,
        "View Environment", "See the environment: its name, owner, place and state, and what updates wait for it.";
    EnvironmentUse = "environment.use", Environment, Use, ENVIRONMENT, INTERACTORS_WHERE_MANAGED,
        "Use Environment", "Start threads in the environment, and have the agents of its threads act for oneself there: give them input, steer or interrupt their turns, stop or retry one.";
    EnvironmentManage = "environment.manage", Environment, Manage, ENVIRONMENT, DEVELOPERS_WHERE_MANAGED,
        "Manage Environment", "Have all of the environment: where its threads run, how far agents are confined and what it offers; start, stop, recreate, update, configure, move and remove it.";
    FilesView = "files.view", Files, View, ENVIRONMENT, VIEWERS,
        "View Files", "List, read and download the files of the environment's whole workspace, with their differences and snapshots.";
    FilesManage = "files.manage", Files, Manage, ENVIRONMENT, DEVELOPERS_WHERE_MANAGED,
        "Manage Files", "Change the files of that workspace through Orch, and make, label and restore snapshots.";
    TerminalView = "terminal.view", Terminal, View, ENVIRONMENT, VIEWERS,
        "View Terminal", "See the processes of the environment and what they use, and watch an agent's own terminal there.";
    TerminalManage = "terminal.manage", Terminal, Manage, ENVIRONMENT, DEVELOPERS_WHERE_MANAGED,
        "Manage Terminal", "Open a shell of one's own in the environment and come back to it, signal processes there, and type into an agent's own terminal.";
    BrowserView = "browser.view", Browser, View, ENVIRONMENT, VIEWERS,
        "View Browsers", "See the browsers open in the environment and the profiles it keeps, and watch a tab of one, and who else watches or drives it.";
    BrowserManage = "browser.manage", Browser, Manage, ENVIRONMENT, NOBODY,
        "Manage Browsers", "Drive a browser of the environment with pointer and keys, open and close its tabs, open or quit one, make, rename and delete profiles, and read the address a browser is driven at.";
    VncView = "vnc.view", Vnc, View, ENVIRONMENT, VIEWERS,
        "View VNC", "See the displays served in the environment, and watch one.";
    VncManage = "vnc.manage", Vnc, Manage, ENVIRONMENT, NOBODY,
        "Manage VNC", "Work in a display with pointer, keys and the clipboard, open one from a preset, and close one.";
    PortsView = "ports.view", Ports, View, ENVIRONMENT, VIEWERS,
        "View Ports", "See owned TCP listeners, bound UDP sockets, targets and counts, and use a port oneself.";
    PortsManage = "ports.manage", Ports, Manage, ENVIRONMENT, NOBODY,
        "Manage Ports", "Read callers and both ends, admit a named place, link or host within current policy, and close openings or decide port requests.";
    HostView = "host.view", Host, View, HOST, NOBODY,
        "View Host", "See the host in lists, with its capacity and what runs on it for the reader.";
    HostUse = "host.use", Host, Use, HOST, NOBODY,
        "Use Host", "Have environments and a provider's runtime made on the host, or moved to it, within what the entry allows.";
    HostManage = "host.manage", Host, Manage, HOST, NOBODY,
        "Manage Host", "Rename and label the host, take it out of scheduling, say what it does when it is full, stop or remove what runs on it, decide the capability requests of its scope, and remove it.";
    MachineView = "machine.view", Machine, View, MACHINE, NOBODY,
        "View Machine", "See the machine, its declared services, connection and policy.";
    MachineUse = "machine.use", Machine, Use, MACHINE, NOBODY,
        "Use Machine", "Register environments and provider runtimes on this machine within its policy and the entry's limits.";
    MachineManage = "machine.manage", Machine, Manage, MACHINE, NOBODY,
        "Manage Machine", "Rename or revoke the machine, manage its page and roles, and change policy keys handed to Orch.";
    TemplateView = "template.view", Template, View, TEMPLATE, NOBODY,
        "View Template", "See the template in lists, with what it specifies.";
    TemplateUse = "template.use", Template, Use, TEMPLATE, NOBODY,
        "Use Template", "Have an environment, or a provider's runtime, made from the template within what the entry allows.";
    TemplateManage = "template.manage", Template, Manage, TEMPLATE, NOBODY,
        "Manage Template", "Change, enable, disable and delete the template.";
    McpView = "mcp.view", Mcp, View, MCP_SERVER, NOBODY,
        "View MCP server", "See the server in lists, without its secrets.";
    McpUse = "mcp.use", Mcp, Use, MCP_SERVER, NOBODY,
        "Use MCP server", "Switch the server on for a scope.";
    McpManage = "mcp.manage", Mcp, Manage, MCP_SERVER, NOBODY,
        "Manage MCP server", "Change, enable, disable and delete the server.";
    MountView = "mount.view", Mount, View, MOUNT, NOBODY,
        "View Mount", "See the mount in lists, with the machines that have it.";
    MountUse = "mount.use", Mount, Use, MOUNT, NOBODY,
        "Use Mount", "Attach the mount to an environment, or to the environments made for a scope.";
    MountManage = "mount.manage", Mount, Manage, MOUNT, NOBODY,
        "Manage Mount", "Change, enable, disable and delete the mount, and say where its data may be.";
    SkillView = "skill.view", Skill, View, SKILL, NOBODY,
        "View Skill", "See the skill in lists, with its versions, their files and where each came from.";
    SkillUse = "skill.use", Skill, Use, SKILL, NOBODY,
        "Use Skill", "Switch the skill on for a place, so that the agents of its threads are given it.";
    SkillManage = "skill.manage", Skill, Manage, SKILL, NOBODY,
        "Manage Skill", "Import, update and upload versions, edit and publish its draft, enable, disable and delete the skill.";
}

impl std::fmt::Display for Permission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl PartialOrd for Permission {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Permission {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_str().cmp(other.as_str())
    }
}

/// Who may name a holder in a row, beyond being able to change the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NamedBy {
    /// Whoever may change the page.
    WhoeverChangesThePage,
    /// One of the thing's owners, or an admin of its place: control of a
    /// page does not pass itself on.
    OwnerOrAdmin,
    /// An admin of the place and nobody else: the owners of a thing do not
    /// hold this row, and so do not give it.
    Admin,
}

/// What archive leaves of a permission on a node that is archived, itself or
/// through a node above it. Under archive everything is read-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WhenArchived {
    /// It holds as it is: looking, and saying who can.
    Holds,
    /// It holds for deleting the node, and for bringing back the one that
    /// was archived, and for nothing else.
    HoldsToDeleteOrUnarchive,
    /// It holds for bringing the node back, and for nothing else.
    HoldsToUnarchive,
    /// It is not held while the archive lasts.
    Withheld,
}

/// How what a permission is no use without is offered with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Offer {
    /// An entry whose holder is the work in a place: the thing itself or a
    /// place above it. The resource is then open to that work once, instead
    /// of to its people one by one.
    ToTheWork,
    /// An entry for the same holders.
    ToTheSameHolders,
    /// Nothing is offered: it is the caller's own to have.
    Nothing,
}

/// Something on another object that a permission is no use without.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GoesWith {
    /// The permission on the other object.
    pub needs: Permission,
    /// How it is offered in the same step.
    pub offer: Offer,
}

const fn with(needs: Permission, offer: Offer) -> GoesWith {
    GoesWith { needs, offer }
}

impl Permission {
    /// Whether it exists on a thing of kind `object`.
    #[must_use]
    pub fn exists_on(self, object: ObjectKind) -> bool {
        self.on().contains(&object)
    }

    /// For an advanced row: the level it belongs with, whose holders hold
    /// the row without being named in it. Nothing for a level, and nothing
    /// for the one advanced row that belongs with none: naming somebody from
    /// outside is held by whom its row names and by nobody because of
    /// anything else they hold.
    #[must_use]
    pub const fn with(self) -> Option<Permission> {
        match self {
            Self::ThreadFork | Self::ThreadArchive => Some(Self::ThreadManage),
            Self::TopicArchive => Some(Self::TopicManage),
            // Making things inside is a part of contributing to the place, and can be given alone.
            Self::TopicStartThreads => Some(Self::TopicInteract),
            Self::ProjectCreateTopics => Some(Self::ProjectInteract),
            Self::ProjectArchive => Some(Self::ProjectManage),
            Self::OrgArchive => Some(Self::OrgManage),
            Self::DocCreate | Self::DocComment => Some(Self::DocEdit),
            // The Export rows belong with the Manage of their area, whose holders hold the advanced rows.
            Self::ThreadExport => Some(Self::ThreadManage),
            Self::ChatExport => Some(Self::ChatManage),
            Self::DocExport => Some(Self::DocManage),
            Self::UsageSummary => Some(Self::UsageView),
            Self::UsageBypass => Some(Self::UsageManage),
            Self::ProviderBypass => Some(Self::ProviderManage),
            _ => None,
        }
    }

    /// The other permissions whose holders hold this one too: the higher
    /// levels of its area, and for an advanced row the level it belongs with
    /// and the levels above that. Each of them exists wherever this one
    /// does. Nothing of another area is among them, but one thing: whoever
    /// manages an environment holds each of its four doors, since an agent
    /// they put there can do all of it, so `environment.manage` comes last
    /// among those that hold a door's row.
    pub fn held_through(self) -> impl Iterator<Item = Permission> {
        let (area, above) = match (self.level().height(), self.with()) {
            (Some(height), _) => (Some(self.area()), height + 1),
            (None, Some(with)) => (Some(with.area()), with.level().height().unwrap_or(u8::MAX)),
            (None, None) => (None, u8::MAX),
        };
        let manager = self
            .area()
            .is_door()
            .then_some(Permission::EnvironmentManage);
        Self::ALL
            .iter()
            .copied()
            .filter(move |other| {
                Some(other.area()) == area
                    && other.level().height().is_some_and(|height| height >= above)
            })
            .chain(manager)
    }

    /// The other permissions that whoever holds this one holds with it: the
    /// lower levels of its area, and the advanced rows that belong with it
    /// or with a level below it.
    pub fn gives(self) -> impl Iterator<Item = Permission> {
        Self::ALL
            .iter()
            .copied()
            .filter(move |other| other.held_through().any(|through| through == self))
    }

    /// Whether its default has two rows, one for a managed environment and
    /// one for a person's machine: the rows of a thread, by the environment
    /// it runs in, and those of an environment and of its four doors, by
    /// itself.
    #[must_use]
    pub const fn profiles(self) -> bool {
        matches!(self.area(), Area::Thread | Area::Environment) || self.area().is_door()
    }

    /// The row of a defaults page that a thing follows for this permission,
    /// when its work runs in an environment of kind `placement`.
    #[must_use]
    pub fn profile(self, placement: Placement) -> Profile {
        if self.profiles() {
            placement.into()
        } else {
            Profile::Any
        }
    }

    /// Whether defaults pages have a row for it. The Owners row follows no
    /// default and has none.
    #[must_use]
    pub const fn has_default(self) -> bool {
        !matches!(self, Self::AccessOwn)
    }

    /// The role a new installation names in its row of the system's
    /// defaults, for an environment of kind `placement`: that role and the
    /// higher ones then hold it wherever nothing was changed. `None` is
    /// nobody: the row is left to the owners until a page or a default names
    /// somebody. These are the values an installation starts with, and no
    /// rule: what holds in a place is what its pages say.
    #[must_use]
    pub const fn shipped(self, placement: Placement) -> Option<Role> {
        let [managed, connected] = self.ships();
        match placement {
            Placement::Managed => managed,
            Placement::Connected => connected,
        }
    }

    /// The kinds of holder its rows take, on some page or other. Whether a
    /// row of one page takes a kind is [`HolderKind::fits`].
    #[must_use]
    pub fn holders(self) -> Vec<HolderKind> {
        let systems = self.on().iter().any(|kind| {
            kind.resource()
                .is_some_and(ResourceKind::can_be_the_systems)
        });
        HolderKind::ALL
            .iter()
            .copied()
            .filter(|kind| match kind {
                HolderKind::Everyone => systems && self != Self::AccessOwn,
                _ => kind.fits_defaults(self),
            })
            .collect()
    }

    /// Whether the agents of a place can be named in its rows: the rows by
    /// which conversations, chats and documents are read and written, and the
    /// one by which documents are deleted, moved and brought back (nothing of
    /// that kind is open to agents for conversations or chats).
    #[must_use]
    pub const fn shared_with_agents(self) -> bool {
        matches!(
            self,
            Self::ThreadView
                | Self::ThreadInteract
                | Self::ChatView
                | Self::ChatInteract
                | Self::DocView
                | Self::DocEdit
                | Self::DocManage
        )
    }

    /// Whether an entry in its rows may carry options. Three do: the use of
    /// a provider, of a host and of a template.
    #[must_use]
    pub const fn takes_options(self) -> bool {
        matches!(
            self,
            Self::ProviderUse | Self::HostUse | Self::MachineUse | Self::TemplateUse
        )
    }

    /// A JSON Schema of the options an entry in its rows may carry; nothing
    /// for a permission that takes none.
    #[must_use]
    pub fn options_schema(self) -> Option<Json> {
        let schema = match self {
            Self::ProviderUse => schemars::schema_for!(ProviderUse),
            Self::HostUse | Self::MachineUse => schemars::schema_for!(HostUse),
            Self::TemplateUse => schemars::schema_for!(TemplateUse),
            _ => return None,
        };
        Some(schema.to_value())
    }

    /// Who may name a holder in its rows.
    #[must_use]
    pub const fn named_by(self) -> NamedBy {
        match self {
            Self::AccessManage | Self::AccessOwn => NamedBy::OwnerOrAdmin,
            Self::AccessExternal => NamedBy::Admin,
            _ => NamedBy::WhoeverChangesThePage,
        }
    }

    /// What archive leaves of it on a node. Looking stays, and so does
    /// saying who can; the node's own Manage is left for deleting and for
    /// bringing it back, and its Archive row for bringing it back.
    #[must_use]
    pub const fn when_archived(self) -> WhenArchived {
        match self {
            Self::ThreadManage | Self::TopicManage | Self::ProjectManage | Self::OrgManage => {
                WhenArchived::HoldsToDeleteOrUnarchive
            }
            Self::ThreadArchive | Self::TopicArchive | Self::ProjectArchive | Self::OrgArchive => {
                WhenArchived::HoldsToUnarchive
            }
            Self::AccessManage | Self::AccessOwn | Self::UsageSummary => WhenArchived::Holds,
            // Taking a thing away as a file only reads it, and an archive is read-only, not closed.
            Self::ThreadExport | Self::ChatExport | Self::DocExport => WhenArchived::Holds,
            _ if matches!(self.level(), Level::View) => WhenArchived::Holds,
            _ => WhenArchived::Withheld,
        }
    }

    /// What it is no use without on another object, and how that is offered
    /// in the step that gives it. The two stay separate permissions on
    /// separate pages; the page that gives the first shows the second beside
    /// it.
    #[must_use]
    pub const fn goes_with(self) -> &'static [GoesWith] {
        // Talking to an agent has it act in its environment and on its provider for the one who talks.
        const TALKING: &[GoesWith] = &[
            with(Permission::ProviderUse, Offer::ToTheWork),
            with(Permission::EnvironmentUse, Offer::ToTheWork),
        ];
        const STARTING_WORK: &[GoesWith] = &[
            with(Permission::ProviderUse, Offer::ToTheWork),
            with(Permission::AgentUse, Offer::ToTheWork),
            with(Permission::HostUse, Offer::ToTheWork),
            with(Permission::TemplateUse, Offer::ToTheWork),
            with(Permission::EnvironmentUse, Offer::ToTheWork),
        ];
        const A_SUBSCRIPTION: &[GoesWith] = &[with(Permission::AgentUse, Offer::ToTheSameHolders)];
        // Reading a thread shows its people none of the files it works on until
        // the environment lets them look in: its View rows, and for whoever
        // sets the thread up the Manage of Files and Terminal as well. Never
        // `environment.manage`, and never the Manage of Browsers or VNC.
        const LOOKING_IN: &[GoesWith] = &[
            with(Permission::EnvironmentView, Offer::ToTheSameHolders),
            with(Permission::FilesView, Offer::ToTheSameHolders),
            with(Permission::TerminalView, Offer::ToTheSameHolders),
            with(Permission::BrowserView, Offer::ToTheSameHolders),
            with(Permission::VncView, Offer::ToTheSameHolders),
            with(Permission::PortsView, Offer::ToTheSameHolders),
        ];
        const ANOTHER_PROVIDER_AND_WORKING_IN: &[GoesWith] = &[
            with(Permission::ProviderUse, Offer::Nothing),
            with(Permission::AgentUse, Offer::Nothing),
            with(Permission::FilesManage, Offer::ToTheSameHolders),
            with(Permission::TerminalManage, Offer::ToTheSameHolders),
        ];
        const A_FORK: &[GoesWith] = &[
            with(Permission::ProviderUse, Offer::Nothing),
            with(Permission::AgentUse, Offer::Nothing),
            with(Permission::EnvironmentManage, Offer::Nothing),
        ];
        match self {
            Self::ThreadView => LOOKING_IN,
            Self::ThreadInteract => TALKING,
            Self::TopicInteract
            | Self::TopicStartThreads
            | Self::ProjectInteract
            | Self::ProjectCreateTopics
            | Self::OrgInteract => STARTING_WORK,
            Self::ProviderUse => A_SUBSCRIPTION,
            Self::ThreadManage => ANOTHER_PROVIDER_AND_WORKING_IN,
            Self::ThreadFork => A_FORK,
            _ => &[],
        }
    }

    /// Everything a page, a seed or a screen shows of it, as data.
    #[must_use]
    pub fn def(self) -> PermissionDef {
        PermissionDef {
            name: self,
            area: self.area(),
            level: self.level(),
            with: self.with(),
            on: self.on().to_vec(),
            label: self.label().to_owned(),
            allows: self.allows().to_owned(),
            profiles: self.profiles(),
            holders: self.holders(),
            options_schema: self.options_schema(),
        }
    }
}

words! {
    /// A name that stands for a permission of a node's own area, for a
    /// request that acts on a node of any kind: on a thread the thread's, on
    /// a topic the topic's, on a project the project's, on an org the org's.
    /// It is shorthand in what a request asks for; no row is stored under it.
    pub enum NodePermission ("permission of a node") {
        /// View of the node's own area.
        View = "node.view",
        /// Interact of the node's own area.
        Interact = "node.interact",
        /// Manage of the node's own area.
        Manage = "node.manage",
        /// The advanced row by which a node is archived and brought back.
        Archive = "node.archive",
    }
}

impl NodePermission {
    /// The permission it stands for on a node of kind `node`.
    #[must_use]
    pub const fn on(self, node: NodeKind) -> Option<Permission> {
        Some(match (self, node) {
            (Self::View, NodeKind::Org) => Permission::OrgView,
            (Self::View, NodeKind::Project) => Permission::ProjectView,
            (Self::View, NodeKind::Topic) => Permission::TopicView,
            (Self::View, NodeKind::Thread) => Permission::ThreadView,
            (Self::Interact, NodeKind::Org) => Permission::OrgInteract,
            (Self::Interact, NodeKind::Project) => Permission::ProjectInteract,
            (Self::Interact, NodeKind::Topic) => Permission::TopicInteract,
            (Self::Interact, NodeKind::Thread) => Permission::ThreadInteract,
            (Self::Manage, NodeKind::Org) => Permission::OrgManage,
            (Self::Manage, NodeKind::Project) => Permission::ProjectManage,
            (Self::Manage, NodeKind::Topic) => Permission::TopicManage,
            (Self::Manage, NodeKind::Thread) => Permission::ThreadManage,
            (Self::Archive, NodeKind::Org) => Permission::OrgArchive,
            (Self::Archive, NodeKind::Project) => Permission::ProjectArchive,
            (Self::Archive, NodeKind::Topic) => Permission::TopicArchive,
            (Self::Archive, NodeKind::Thread) => Permission::ThreadArchive,
        })
    }
}

/// A permission as a request names what it needs: one of the catalogue, or a
/// name that stands for one by the kind of node it is asked of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PermissionName {
    /// A permission of the catalogue.
    Exact(Permission),
    /// A permission of the node's own area.
    OfNode(NodePermission),
}

impl PermissionName {
    /// The name as it is written.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Exact(permission) => permission.as_str(),
            Self::OfNode(permission) => permission.as_str(),
        }
    }

    /// The permission it means on a thing of kind `object`; nothing where
    /// that kind has none of the name.
    #[must_use]
    pub fn on(self, object: ObjectKind) -> Option<Permission> {
        match self {
            Self::Exact(permission) => permission.exists_on(object).then_some(permission),
            Self::OfNode(permission) => object.node().and_then(|node| permission.on(node)),
        }
    }
}

impl std::fmt::Display for PermissionName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for PermissionName {
    type Err = UnknownName;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if let Ok(permission) = text.parse() {
            return Ok(Self::Exact(permission));
        }
        text.parse()
            .map(Self::OfNode)
            .map_err(|_| UnknownName::new("permission", text))
    }
}

words! {
    /// The three choices for everyday use. Each names several rows at once,
    /// so that nobody has to say access row by row. They are a convenience
    /// of a page and of a request, and add no meaning: what is stored is the
    /// rows. The advanced rows are no part of any choice, and neither is
    /// administration: settings, usage, the audit trail, and who may change
    /// the page.
    #[derive(PartialOrd, Ord)]
    pub enum Choice ("choice") {
        /// Can view.
        FullView = "full_view",
        /// Can interact; on a resource, can use; on a document, can edit.
        FullInteract = "full_interact",
        /// Can manage.
        FullManage = "full_manage",
    }
}

impl Choice {
    /// What it is called.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::FullView => "Can view",
            Self::FullInteract => "Can interact",
            Self::FullManage => "Can manage",
        }
    }

    /// What it is called on a thing of kind `object`: the middle one is
    /// using for a resource and editing for a document.
    #[must_use]
    pub const fn label_on(self, object: ObjectKind) -> &'static str {
        match (self, object) {
            (Self::FullInteract, ObjectKind::Document | ObjectKind::DocumentContainer) => {
                "Can edit"
            }
            (Self::FullInteract, kind) if kind.resource().is_some() => "Can use",
            // A place's middle level makes things inside it; talking to an agent or people is a thread's or a chat's.
            (Self::FullInteract, ObjectKind::Org | ObjectKind::Project | ObjectKind::Topic) => {
                "Can contribute"
            }
            _ => self.label(),
        }
    }

    /// The rows it names on a thing of kind `object`. Empty where that kind
    /// has no such choice.
    #[must_use]
    pub const fn permissions(self, object: ObjectKind) -> &'static [Permission] {
        use Permission as P;
        match (self, object) {
            (Self::FullView, ObjectKind::Thread) => &[P::ChatView, P::DocView, P::ThreadView],
            (Self::FullInteract, ObjectKind::Thread) => {
                &[P::ChatInteract, P::DocEdit, P::ThreadInteract]
            }
            (Self::FullManage, ObjectKind::Thread) => {
                &[P::ChatManage, P::DocManage, P::ThreadManage]
            }
            (Self::FullView, ObjectKind::Topic) => &[P::ChatView, P::DocView, P::TopicView],
            (Self::FullInteract, ObjectKind::Topic) => {
                &[P::ChatInteract, P::DocEdit, P::TopicInteract]
            }
            (Self::FullManage, ObjectKind::Topic) => &[P::ChatManage, P::DocManage, P::TopicManage],
            (Self::FullView, ObjectKind::Project) => &[P::ChatView, P::DocView, P::ProjectView],
            (Self::FullInteract, ObjectKind::Project) => {
                &[P::ChatInteract, P::DocEdit, P::ProjectInteract]
            }
            (Self::FullManage, ObjectKind::Project) => {
                &[P::ChatManage, P::DocManage, P::ProjectManage]
            }
            (Self::FullView, ObjectKind::Org) => &[P::ChatView, P::DocView, P::OrgView],
            (Self::FullInteract, ObjectKind::Org) => &[P::ChatInteract, P::DocEdit, P::OrgInteract],
            (Self::FullManage, ObjectKind::Org) => &[P::ChatManage, P::DocManage, P::OrgManage],
            (Self::FullView, ObjectKind::Document | ObjectKind::DocumentContainer) => &[P::DocView],
            (Self::FullInteract, ObjectKind::Document | ObjectKind::DocumentContainer) => {
                &[P::DocEdit]
            }
            (Self::FullManage, ObjectKind::Document | ObjectKind::DocumentContainer) => {
                &[P::DocManage]
            }
            (Self::FullView, ObjectKind::Provider) => &[P::ProviderView],
            (Self::FullInteract, ObjectKind::Provider) => &[P::ProviderUse],
            (Self::FullManage, ObjectKind::Provider) => &[P::ProviderManage],
            (Self::FullView, ObjectKind::Agent) => &[P::AgentView],
            (Self::FullInteract, ObjectKind::Agent) => &[P::AgentUse],
            (Self::FullManage, ObjectKind::Agent) => &[P::AgentManage],
            (Self::FullView, ObjectKind::Environment) => &[
                P::EnvironmentView,
                P::FilesView,
                P::TerminalView,
                P::BrowserView,
                P::VncView,
                P::PortsView,
            ],
            (Self::FullInteract, ObjectKind::Environment) => &[P::EnvironmentUse],
            (Self::FullManage, ObjectKind::Environment) => &[P::EnvironmentManage],
            (Self::FullView, ObjectKind::Host) => &[P::HostView],
            (Self::FullInteract, ObjectKind::Host) => &[P::HostUse],
            (Self::FullManage, ObjectKind::Host) => &[P::HostManage],
            (Self::FullView, ObjectKind::Machine) => &[P::MachineView],
            (Self::FullInteract, ObjectKind::Machine) => &[P::MachineUse],
            (Self::FullManage, ObjectKind::Machine) => &[P::MachineManage],
            (Self::FullView, ObjectKind::Template) => &[P::TemplateView],
            (Self::FullInteract, ObjectKind::Template) => &[P::TemplateUse],
            (Self::FullManage, ObjectKind::Template) => &[P::TemplateManage],
            (Self::FullView, ObjectKind::McpServer) => &[P::McpView],
            (Self::FullInteract, ObjectKind::McpServer) => &[P::McpUse],
            (Self::FullManage, ObjectKind::McpServer) => &[P::McpManage],
            (Self::FullView, ObjectKind::Mount) => &[P::MountView],
            (Self::FullInteract, ObjectKind::Mount) => &[P::MountUse],
            (Self::FullManage, ObjectKind::Mount) => &[P::MountManage],
            (Self::FullView, ObjectKind::Skill) => &[P::SkillView],
            (Self::FullInteract, ObjectKind::Skill) => &[P::SkillUse],
            (Self::FullManage, ObjectKind::Skill) => &[P::SkillManage],
        }
    }

    /// The line of a page that somebody stands in who is named in `rows` of
    /// a thing of kind `object`: the highest of the three choices that they
    /// hold in every area it covers. Being named at a level is holding the
    /// levels below it. `None` is somebody who holds no choice whole: they
    /// hold some areas only, and a page lists them apart with what they
    /// hold.
    #[must_use]
    pub fn highest(object: ObjectKind, rows: &[Permission]) -> Option<Choice> {
        let holds = |needed: Permission| {
            rows.contains(&needed) || needed.held_through().any(|row| rows.contains(&row))
        };
        [Self::FullManage, Self::FullInteract, Self::FullView]
            .into_iter()
            .find(|choice| {
                let covered = choice.permissions(object);
                !covered.is_empty() && covered.iter().all(|needed| holds(*needed))
            })
    }
}

// ---------------------------------------------------------------- the Export rows (the export of a place)

impl Permission {
    /// The three rows by which a thing is taken away as a file: a conversation, a chat and a document.
    pub const EXPORTS: [Permission; 3] = [
        Permission::ThreadExport,
        Permission::ChatExport,
        Permission::DocExport,
    ];

    /// Whether it is one of the Export rows.
    #[must_use]
    pub const fn is_export(self) -> bool {
        matches!(
            self,
            Self::ThreadExport | Self::ChatExport | Self::DocExport
        )
    }
}

impl Choice {
    /// The Export rows of the areas the choice covers on a thing of kind `object`: the rows in which whoever is
    /// given the choice is named as well, unless the giver leaves that out or does not hold them. The three
    /// choices travel with them alike, since reading a thing is what lets one take it away. Empty where the
    /// kind has no such choice, and for a resource.
    #[must_use]
    pub fn exports(self, object: ObjectKind) -> Vec<Permission> {
        let covered = self.permissions(object);
        Permission::EXPORTS
            .into_iter()
            .filter(|export| {
                export.exists_on(object) && covered.iter().any(|row| row.area() == export.area())
            })
            .collect()
    }
}

/// One permission of the catalogue, as data for the pages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PermissionDef {
    /// The permission.
    pub name: Permission,
    /// Its area.
    pub area: Area,
    /// Its level, or that it is an advanced row.
    pub level: Level,
    /// For an advanced row: the permission whose holders hold it too; `null`
    /// for a level, and for the advanced row that belongs with none.
    pub with: Option<Permission>,
    /// The kinds of object it exists on.
    pub on: Vec<ObjectKind>,
    /// What a page calls its row.
    pub label: String,
    /// What it lets a person do, in a sentence.
    pub allows: String,
    /// Whether its default has one row for a managed environment and one for
    /// a person's machine.
    pub profiles: bool,
    /// The kinds of holder its rows take.
    pub holders: Vec<HolderKind>,
    /// A JSON Schema of the options an entry in it may carry.
    pub options_schema: Option<Json>,
}

/// One of the three choices, with the rows it names on each kind of object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChoiceDef {
    /// The choice.
    pub key: Choice,
    /// What it is called.
    pub label: String,
    /// The rows it names, by kind of object. A kind that has no such choice
    /// is left out.
    pub permissions: BTreeMap<ObjectKind, Vec<Permission>>,
}

/// A role and its name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RoleDef {
    /// The role.
    pub key: Role,
    /// Its name.
    pub name: String,
}

/// The catalogue as data: every page is drawn from it, and names no
/// permission of its own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Catalogue {
    /// Every permission.
    pub permissions: Vec<PermissionDef>,
    /// The three everyday choices.
    pub choices: Vec<ChoiceDef>,
    /// The four roles, lowest first.
    pub roles: Vec<RoleDef>,
}

/// The whole catalogue.
#[must_use]
pub fn catalogue() -> Catalogue {
    let choice = |key: Choice| ChoiceDef {
        key,
        label: key.label().to_owned(),
        permissions: ObjectKind::ALL
            .iter()
            .map(|kind| (*kind, key.permissions(*kind).to_vec()))
            .filter(|(_, rows)| !rows.is_empty())
            .collect(),
    };
    Catalogue {
        permissions: Permission::ALL
            .iter()
            .map(|permission| permission.def())
            .collect(),
        choices: Choice::ALL.iter().copied().map(choice).collect(),
        roles: Role::ALL
            .iter()
            .map(|role| RoleDef {
                key: *role,
                name: role.name().to_owned(),
            })
            .collect(),
    }
}

words! {
    /// A way to place a harness, its tools and a credential that an entry
    /// may allow for work done through it. The fifth arrangement, everything
    /// on one machine, is never chosen and so is no option.
    #[derive(PartialOrd, Ord)]
    pub enum Arrangement ("arrangement type") {
        /// The harness stays with its provider, and its tools run elsewhere.
        Separated = "separated",
        /// The key stays with the provider and is forwarded.
        KeySeparated = "key_separated",
        /// A copy of the credential is shared.
        CredentialShared = "credential_shared",
        /// The credential itself is handed over.
        HandedOver = "handed_over",
    }
}

/// What an entry in the Use row of a provider lets its holder run. A member
/// that is left out does not narrow.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderUse {
    /// The models the holder may run, by name; left out, every enabled model
    /// of the provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<String>>,
    /// The reasoning efforts the holder may run, by name, on any model that
    /// lists them; left out, every effort a model lists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub efforts: Option<Vec<String>>,
    /// The arrangement types in which work done through this entry may use
    /// the provider; left out, all that the provider's owners allow. It can
    /// only narrow what they allow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrangements: Option<Vec<Arrangement>>,
}

/// The most that an environment may be given. A member that is left out is
/// no cap.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResourceLimits {
    /// Thousandths of a processor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_millis: Option<i64>,
    /// Mebibytes of memory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_mb: Option<i64>,
    /// Processes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pids: Option<i64>,
    /// Gibibytes of disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_gb: Option<i64>,
}

/// What an entry in the Use row of a host lets its holder have made there.
/// A member that is left out does not narrow.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HostUse {
    /// How many environments created through this entry may exist on the
    /// host at one time; left out, as many as the host has room for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_environments: Option<u32>,
    /// The host's storages their volumes may be created on; left out, all of
    /// them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storages: Option<Vec<String>>,
    /// The most one environment created through this entry may be given on
    /// this host, whatever its template allows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources_max: Option<ResourceLimits>,
    /// Which of the host's sources such an environment's template may name;
    /// left out, all the host offers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sources: Option<Vec<String>>,
    /// The templates that may be used on this host through this entry; left
    /// out, any template the holder may use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub templates: Option<Vec<Id>>,
}

/// What an entry in the Use row of a template lets its holder have made from
/// it. A member that is left out does not narrow.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TemplateUse {
    /// The most an environment created through this entry may be given. It
    /// can only narrow what the template itself allows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources_max: Option<ResourceLimits>,
    /// Which of the capabilities the template allows such an environment may
    /// ever be given; left out, all of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
}

/// The options of one entry, read for the permission whose row it stands in.
/// They are the owners' usage policies: what an entry lets the other person
/// do, written on the entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Options {
    /// The permission takes no options.
    #[default]
    None,
    /// Of the use of a provider.
    ProviderUse(ProviderUse),
    /// Of the use of a host.
    HostUse(HostUse),
    /// Of the use of a template.
    TemplateUse(TemplateUse),
}

impl Options {
    /// Reads the options of an entry in the row of `permission`, as they
    /// were written or stored. Nothing, `null` and an empty object are no
    /// options. A member the permission does not define is refused, and so
    /// is any option on a permission that takes none; every fault is
    /// returned, each with the member it is about.
    pub fn parse(permission: Permission, options: &Json) -> Result<Self, Vec<FieldError>> {
        let empty = serde_json::Map::new();
        let members = match options {
            Json::Null => &empty,
            Json::Object(members) => members,
            _ => return Err(vec![fault("", "must be an object")]),
        };
        let mut reader = Reader {
            members,
            faults: Vec::new(),
            read: Vec::new(),
        };
        let parsed = match permission {
            Permission::ProviderUse => Self::ProviderUse(ProviderUse {
                models: reader.texts("models"),
                efforts: reader.texts("efforts"),
                arrangements: reader.words::<Arrangement>("arrangements"),
            }),
            Permission::HostUse | Permission::MachineUse => Self::HostUse(HostUse {
                max_environments: reader.count("max_environments"),
                storages: reader.texts("storages"),
                resources_max: reader.limits("resources_max"),
                sources: reader.texts("sources"),
                templates: reader.ids("templates"),
            }),
            Permission::TemplateUse => Self::TemplateUse(TemplateUse {
                resources_max: reader.limits("resources_max"),
                capabilities: reader.texts("capabilities"),
            }),
            _ => Self::None,
        };
        let faults = reader.finish(permission);
        if faults.is_empty() {
            Ok(parsed)
        } else {
            Err(faults)
        }
    }

    /// The options as they are stored: an object with the members that were
    /// given.
    #[must_use]
    pub fn to_json(&self) -> Json {
        let written = match self {
            Self::None => return Json::Object(serde_json::Map::new()),
            Self::ProviderUse(options) => serde_json::to_value(options),
            Self::HostUse(options) => serde_json::to_value(options),
            Self::TemplateUse(options) => serde_json::to_value(options),
        };
        written.unwrap_or_else(|_| Json::Object(serde_json::Map::new()))
    }
}

fn fault(path: &str, message: &str) -> FieldError {
    FieldError {
        path: path.to_owned(),
        message: message.to_owned(),
    }
}

/// Reads the members of an options object one by one, and remembers which
/// were asked for, so that what nobody asked for can be refused by name.
struct Reader<'a> {
    members: &'a serde_json::Map<String, Json>,
    faults: Vec<FieldError>,
    read: Vec<&'static str>,
}

impl<'a> Reader<'a> {
    fn member(&mut self, name: &'static str) -> Option<&'a Json> {
        self.read.push(name);
        self.members.get(name).filter(|value| !value.is_null())
    }

    fn list<T>(
        &mut self,
        name: &'static str,
        what: &str,
        read: impl Fn(&Json) -> Option<T>,
    ) -> Option<Vec<T>> {
        let value = self.member(name)?;
        let Json::Array(items) = value else {
            self.faults.push(fault(name, "must be a list"));
            return None;
        };
        let mut list = Vec::with_capacity(items.len());
        for (index, item) in items.iter().enumerate() {
            match read(item) {
                Some(item) => list.push(item),
                None => self.faults.push(fault(&format!("{name}[{index}]"), what)),
            }
        }
        Some(list)
    }

    fn texts(&mut self, name: &'static str) -> Option<Vec<String>> {
        self.list(name, "must be text", |item| {
            item.as_str().map(str::to_owned)
        })
    }

    fn ids(&mut self, name: &'static str) -> Option<Vec<Id>> {
        self.list(name, "must be an identifier", |item| {
            item.as_str().and_then(|text| text.parse().ok())
        })
    }

    fn words<T: std::str::FromStr>(&mut self, name: &'static str) -> Option<Vec<T>> {
        self.list(name, "is not one of the values it takes", |item| {
            item.as_str().and_then(|text| text.parse().ok())
        })
    }

    fn count(&mut self, name: &'static str) -> Option<u32> {
        let value = self.member(name)?;
        let count = value.as_u64().and_then(|count| u32::try_from(count).ok());
        if count.is_none() {
            self.faults
                .push(fault(name, "must be a whole number, zero or more"));
        }
        count
    }

    fn limits(&mut self, name: &'static str) -> Option<ResourceLimits> {
        let value = self.member(name)?;
        let Json::Object(given) = value else {
            self.faults.push(fault(name, "must be an object"));
            return None;
        };
        let mut limits = ResourceLimits::default();
        for (member, value) in given {
            let slot = match member.as_str() {
                "cpu_millis" => &mut limits.cpu_millis,
                "memory_mb" => &mut limits.memory_mb,
                "pids" => &mut limits.pids,
                "disk_gb" => &mut limits.disk_gb,
                _ => {
                    self.faults
                        .push(fault(&format!("{name}.{member}"), "is no limit"));
                    continue;
                }
            };
            match value.as_i64().filter(|limit| *limit >= LEAST_LIMIT) {
                Some(limit) => *slot = Some(limit),
                None if value.is_null() => {}
                None => self.faults.push(fault(
                    &format!("{name}.{member}"),
                    "must be a whole number, one or more",
                )),
            }
        }
        Some(limits)
    }

    fn finish(mut self, permission: Permission) -> Vec<FieldError> {
        for name in self.members.keys() {
            if !self.read.contains(&name.as_str()) {
                let message = if permission.takes_options() {
                    "is no option of this permission"
                } else {
                    "is refused: this permission takes no options"
                };
                self.faults.push(fault(name, message));
            }
        }
        self.faults
    }
}

/// The least a limit on what an environment may be given can be. A limit of
/// nothing would give nothing, and to some drivers zero means no limit at
/// all, which is the opposite of what a cap is for.
const LEAST_LIMIT: i64 = 1;

/// Of two lists that each say what is allowed, the one that allows what
/// either does. A list that is left out allows everything, and so does the
/// result.
fn either<T: Clone + PartialEq>(one: Option<&Vec<T>>, other: Option<&Vec<T>>) -> Option<Vec<T>> {
    let (one, other) = (one?, other?);
    let mut all = one.clone();
    for item in other {
        if !all.contains(item) {
            all.push(item.clone());
        }
    }
    Some(all)
}

/// Of two caps the larger; a cap that is left out is none, and so is the
/// result.
fn larger<T: Ord + Copy>(one: Option<T>, other: Option<T>) -> Option<T> {
    Some(one?.max(other?))
}

/// Of two caps the smaller; a cap that is left out is none.
fn smaller<T: Ord + Copy>(one: Option<T>, other: Option<T>) -> Option<T> {
    match (one, other) {
        (Some(one), Some(other)) => Some(one.min(other)),
        (one, other) => one.or(other),
    }
}

fn allowed<T: PartialEq + ?Sized, U: std::borrow::Borrow<T>>(
    list: Option<&Vec<U>>,
    item: &T,
) -> bool {
    list.is_none_or(|list| list.iter().any(|allowed| allowed.borrow() == item))
}

impl ProviderUse {
    /// What somebody may run whom this entry and `other` both let use the
    /// provider. Entries combine permissively: a model, an effort or an
    /// arrangement type that either allows is allowed.
    #[must_use]
    pub fn or(&self, other: &Self) -> Self {
        Self {
            models: either(self.models.as_ref(), other.models.as_ref()),
            efforts: either(self.efforts.as_ref(), other.efforts.as_ref()),
            arrangements: either(self.arrangements.as_ref(), other.arrangements.as_ref()),
        }
    }

    /// Whether the entry lets its holder run `model`.
    #[must_use]
    pub fn allows_model(&self, model: &str) -> bool {
        allowed(self.models.as_ref(), model)
    }

    /// Whether the entry lets its holder run at `effort`.
    #[must_use]
    pub fn allows_effort(&self, effort: &str) -> bool {
        allowed(self.efforts.as_ref(), effort)
    }

    /// Whether work done through the entry may use the provider in
    /// `arrangement`, as far as the entry says.
    #[must_use]
    pub fn allows_arrangement(&self, arrangement: Arrangement) -> bool {
        allowed(self.arrangements.as_ref(), &arrangement)
    }
}

impl ResourceLimits {
    /// The limits of somebody whom two entries of one row both let in: of
    /// two numbers the larger counts.
    #[must_use]
    pub fn or(&self, other: &Self) -> Self {
        Self {
            cpu_millis: larger(self.cpu_millis, other.cpu_millis),
            memory_mb: larger(self.memory_mb, other.memory_mb),
            pids: larger(self.pids, other.pids),
            disk_gb: larger(self.disk_gb, other.disk_gb),
        }
    }

    /// The limits that hold where two things each set some: the entry used
    /// on a host and the entry used on a template, of which the narrower
    /// counts.
    #[must_use]
    pub fn and(&self, other: &Self) -> Self {
        Self {
            cpu_millis: smaller(self.cpu_millis, other.cpu_millis),
            memory_mb: smaller(self.memory_mb, other.memory_mb),
            pids: smaller(self.pids, other.pids),
            disk_gb: smaller(self.disk_gb, other.disk_gb),
        }
    }
}

/// The limits of two entries of one row together, where either may set none.
fn limits_or(
    one: Option<&ResourceLimits>,
    other: Option<&ResourceLimits>,
) -> Option<ResourceLimits> {
    Some(one?.or(other?))
}

impl HostUse {
    /// What somebody may have made on the host whom this entry and `other`
    /// both let use it: what either allows, and of two numbers the larger.
    #[must_use]
    pub fn or(&self, other: &Self) -> Self {
        Self {
            max_environments: larger(self.max_environments, other.max_environments),
            storages: either(self.storages.as_ref(), other.storages.as_ref()),
            resources_max: limits_or(self.resources_max.as_ref(), other.resources_max.as_ref()),
            sources: either(self.sources.as_ref(), other.sources.as_ref()),
            templates: either(self.templates.as_ref(), other.templates.as_ref()),
        }
    }

    /// Whether a volume may be created on the host's storage `storage`
    /// through the entry.
    #[must_use]
    pub fn allows_storage(&self, storage: &str) -> bool {
        allowed(self.storages.as_ref(), storage)
    }

    /// Whether a template that names `source` may be used on the host
    /// through the entry.
    #[must_use]
    pub fn allows_source(&self, source: &str) -> bool {
        allowed(self.sources.as_ref(), source)
    }

    /// Whether `template` may be used on the host through the entry.
    #[must_use]
    pub fn allows_template(&self, template: Id) -> bool {
        allowed(self.templates.as_ref(), &template)
    }
}

impl TemplateUse {
    /// What somebody may have made from the template whom this entry and
    /// `other` both let use it.
    #[must_use]
    pub fn or(&self, other: &Self) -> Self {
        Self {
            resources_max: limits_or(self.resources_max.as_ref(), other.resources_max.as_ref()),
            capabilities: either(self.capabilities.as_ref(), other.capabilities.as_ref()),
        }
    }

    /// Whether an environment made through the entry may ever be given
    /// `capability`, as far as the entry says.
    #[must_use]
    pub fn allows_capability(&self, capability: &str) -> bool {
        allowed(self.capabilities.as_ref(), capability)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::new_id;
    use serde_json::json;

    use Permission as P;

    fn through(permission: Permission) -> Vec<Permission> {
        permission.held_through().collect()
    }

    #[test]
    fn manage_of_an_area_gives_its_lower_levels() {
        assert_eq!(through(P::ThreadView), [P::ThreadInteract, P::ThreadManage]);
        assert_eq!(through(P::ThreadInteract), [P::ThreadManage]);
        assert_eq!(through(P::ThreadManage), []);
        assert_eq!(through(P::DocView), [P::DocEdit, P::DocManage]);
        assert_eq!(
            through(P::ProviderView),
            [P::ProviderUse, P::ProviderManage]
        );
        assert_eq!(
            through(P::FilesView),
            [P::FilesManage, P::EnvironmentManage],
            "files have no middle level, and whoever manages the environment holds both"
        );
        assert_eq!(
            through(P::EnvironmentView),
            [P::EnvironmentUse, P::EnvironmentManage]
        );
        assert_eq!(through(P::EnvironmentUse), [P::EnvironmentManage]);
    }

    #[test]
    fn an_advanced_row_is_held_by_the_level_it_belongs_with_and_those_above() {
        assert_eq!(through(P::ThreadFork), [P::ThreadManage]);
        assert_eq!(through(P::ProjectArchive), [P::ProjectManage]);
        assert_eq!(through(P::DocCreate), [P::DocEdit, P::DocManage]);
        assert_eq!(through(P::DocComment), [P::DocEdit, P::DocManage]);
        assert_eq!(through(P::UsageSummary), [P::UsageView, P::UsageManage]);
        assert_eq!(through(P::UsageBypass), [P::UsageManage]);
        assert_eq!(through(P::ProviderBypass), [P::ProviderManage]);
    }

    #[test]
    fn naming_somebody_from_outside_comes_with_nothing_else_held() {
        assert_eq!(through(P::AccessExternal), []);
        assert_eq!(P::AccessExternal.with(), None);
        assert!(!P::AccessOwn.gives().any(|given| given == P::AccessExternal));
    }

    #[test]
    fn owning_is_above_managing_who_can() {
        assert_eq!(through(P::AccessManage), [P::AccessOwn]);
        assert_eq!(P::AccessOwn.gives().collect::<Vec<_>>(), [P::AccessManage]);
    }

    #[test]
    fn nothing_crosses_areas_but_the_environment_holding_its_four_doors() {
        for permission in P::ALL {
            let floor = permission.with().unwrap_or(*permission);
            for higher in permission.held_through() {
                if higher == P::EnvironmentManage && permission.area().is_door() {
                    continue;
                }
                assert_eq!(higher.area(), floor.area(), "{permission} through {higher}");
            }
        }
        let manage: Vec<_> = P::ThreadManage.gives().collect();
        assert_eq!(
            manage,
            [
                P::ThreadView,
                P::ThreadInteract,
                P::ThreadFork,
                P::ThreadArchive,
                P::ThreadExport
            ],
            "managing a thread gives nothing of its environment"
        );
        assert!(
            !P::FilesManage
                .gives()
                .any(|given| given == P::ThreadInteract),
            "a door gives no word in any thread"
        );
        assert!(
            !P::FilesManage
                .gives()
                .chain(P::BrowserManage.gives())
                .any(|given| given == P::EnvironmentView || given == P::EnvironmentManage),
            "and no door gives the environment itself"
        );
    }

    #[test]
    fn whoever_manages_an_environment_holds_the_manage_of_its_four_doors_and_nothing_else_does() {
        let doors = [
            (P::FilesView, P::FilesManage),
            (P::TerminalView, P::TerminalManage),
            (P::BrowserView, P::BrowserManage),
            (P::VncView, P::VncManage),
            (P::PortsView, P::PortsManage),
        ];
        let given: Vec<_> = P::EnvironmentManage.gives().collect();
        for (view, manage) in doors {
            assert_eq!(through(manage), [P::EnvironmentManage], "{manage}");
            assert_eq!(
                through(view),
                [manage, P::EnvironmentManage],
                "Manage holds View, and the environment's Manage holds both"
            );
            assert!(given.contains(&view) && given.contains(&manage));
            assert!(
                !manage.gives().any(|row| row.area() != manage.area()),
                "a door gives nothing beyond itself"
            );
        }
        assert_eq!(
            given.len(),
            2 + 2 * doors.len(),
            "its own View and Use and the two rows of each door, and no more"
        );
        for kind in ObjectKind::ALL {
            if *kind != ObjectKind::Environment {
                assert!(
                    P::EnvironmentManage
                        .gives()
                        .all(|row| !row.exists_on(*kind)),
                    "nothing the environment's Manage gives is on a {kind}"
                );
            }
        }
    }

    #[test]
    fn the_four_doors_are_rows_of_the_environment_and_of_no_thread() {
        let doors = [
            P::FilesView,
            P::FilesManage,
            P::TerminalView,
            P::TerminalManage,
            P::BrowserView,
            P::BrowserManage,
            P::VncView,
            P::VncManage,
            P::PortsView,
            P::PortsManage,
        ];
        for door in doors {
            assert_eq!(door.on(), [ObjectKind::Environment], "{door}");
            assert!(door.area().is_door(), "{door}");
            assert!(
                ObjectKind::Environment.permissions().any(|row| row == door),
                "{door}"
            );
            assert!(
                !ObjectKind::Thread.permissions().any(|row| row == door),
                "{door} is not on a thread's page"
            );
            assert!(
                door.profiles(),
                "{door} has a row for each kind of environment"
            );
        }
        assert_eq!(
            ObjectKind::Environment
                .permissions()
                .filter(|row| !doors.contains(row))
                .collect::<Vec<_>>(),
            [
                P::AccessManage,
                P::AccessOwn,
                P::AccessExternal,
                P::EnvironmentView,
                P::EnvironmentUse,
                P::EnvironmentManage
            ],
            "the rest of an environment's page is its own three levels and the three about access"
        );
        assert_eq!(
            ObjectKind::Thread
                .permissions()
                .filter(|row| row.area().is_door())
                .count(),
            0
        );
    }

    #[test]
    fn what_holds_a_permission_exists_wherever_the_permission_does() {
        for permission in P::ALL {
            for higher in permission.held_through() {
                for kind in permission.on() {
                    assert!(
                        higher.exists_on(*kind),
                        "{higher} is not on a {kind}, where {permission} is"
                    );
                }
            }
        }
    }

    #[test]
    fn the_rows_of_a_thread_and_of_an_environment_depend_on_where_work_runs() {
        assert_eq!(
            P::ThreadInteract.profile(Placement::Connected),
            Profile::Connected
        );
        assert_eq!(P::FilesView.profile(Placement::Managed), Profile::Managed);
        assert_eq!(
            P::EnvironmentManage.profile(Placement::Connected),
            Profile::Connected
        );
        assert_eq!(P::ChatView.profile(Placement::Connected), Profile::Any);
        assert_eq!(P::TopicManage.profile(Placement::Managed), Profile::Any);
        assert_eq!(
            P::ThreadFork.profile(Placement::default()),
            Profile::Managed
        );
    }

    #[test]
    fn on_a_persons_machine_everybody_but_the_owners_only_watches_as_shipped() {
        for permission in ObjectKind::Thread
            .permissions()
            .chain(ObjectKind::Environment.permissions())
        {
            if !permission.profiles() {
                continue;
            }
            let shipped = permission.shipped(Placement::Connected);
            let watching = matches!(permission.level(), Level::View)
                || permission == P::ThreadFork
                || permission == P::ThreadExport;
            assert_eq!(shipped.is_some(), watching, "{permission}");
            assert!(
                shipped.is_none_or(|role| role == Role::Viewer),
                "{permission}"
            );
        }
        assert_eq!(
            P::ThreadInteract.shipped(Placement::Managed),
            Some(Role::Interactor)
        );
        assert_eq!(
            P::TerminalManage.shipped(Placement::Managed),
            Some(Role::Developer)
        );
        assert_eq!(
            P::EnvironmentManage.shipped(Placement::Managed),
            Some(Role::Developer)
        );
    }

    #[test]
    fn the_four_doors_ship_as_the_specification_says_in_both_placements() {
        use Placement::{Connected, Managed};
        for view in [P::FilesView, P::TerminalView, P::BrowserView, P::VncView] {
            assert_eq!(view.shipped(Managed), Some(Role::Viewer), "{view}");
            assert_eq!(view.shipped(Connected), Some(Role::Viewer), "{view}");
        }
        for manage in [P::FilesManage, P::TerminalManage] {
            assert_eq!(manage.shipped(Managed), Some(Role::Developer), "{manage}");
            assert_eq!(manage.shipped(Connected), None, "{manage}");
        }
        for manage in [P::BrowserManage, P::VncManage] {
            assert_eq!(
                manage.shipped(Managed),
                None,
                "{manage}: whoever manages the environment drives it, and nobody else"
            );
            assert_eq!(manage.shipped(Connected), None, "{manage}");
        }
    }

    #[test]
    fn a_new_resource_is_its_owners_alone_as_shipped() {
        for kind in ResourceKind::ALL {
            for permission in ObjectKind::from(*kind).permissions() {
                if permission.area() != kind.area() || *kind == ResourceKind::Environment {
                    continue;
                }
                assert_eq!(permission.shipped(Placement::Managed), None, "{permission}");
            }
        }
    }

    #[test]
    fn a_thing_is_archived_by_whoever_manages_it_and_by_nobody_else_as_shipped() {
        for archive in [
            P::ThreadArchive,
            P::TopicArchive,
            P::ProjectArchive,
            P::OrgArchive,
        ] {
            assert_eq!(archive.shipped(Placement::Managed), None);
            assert_eq!(archive.with().map(Permission::level), Some(Level::Manage));
        }
    }

    #[test]
    fn the_owners_row_follows_no_default_and_takes_people_and_groups() {
        assert!(!P::AccessOwn.has_default());
        assert_eq!(
            P::AccessOwn.holders(),
            [HolderKind::Group, HolderKind::User]
        );
        assert!(!HolderKind::Role.fits(P::AccessOwn, ObjectKind::Thread, false));
        assert!(!HolderKind::Everyone.fits(P::AccessOwn, ObjectKind::Host, true));
    }

    #[test]
    fn the_work_in_a_place_is_named_in_the_use_row_of_a_resource_and_nowhere_else() {
        assert!(HolderKind::Node.fits(P::ProviderUse, ObjectKind::Provider, false));
        assert!(HolderKind::Node.fits(P::MountUse, ObjectKind::Mount, false));
        assert!(!HolderKind::Node.fits(P::ProviderView, ObjectKind::Provider, false));
        assert!(!HolderKind::Node.fits(P::ProviderManage, ObjectKind::Provider, false));
        assert!(!HolderKind::Node.fits(P::EnvironmentManage, ObjectKind::Environment, false));
        assert!(!HolderKind::Node.fits(P::ThreadInteract, ObjectKind::Thread, false));
    }

    #[test]
    fn agents_are_named_where_conversations_chats_and_documents_are_read_written_and_managed() {
        for row in [
            P::ThreadView,
            P::ThreadInteract,
            P::ChatView,
            P::ChatInteract,
            P::DocView,
            P::DocEdit,
            P::DocManage,
        ] {
            assert!(HolderKind::Agents.fits(row, row.on()[0], false), "{row}");
        }
        assert!(HolderKind::Agents.fits(P::DocEdit, ObjectKind::Document, false));
        assert!(HolderKind::Agents.fits(P::DocManage, ObjectKind::Document, false));
        // A conversation and a chat are never managed by an agent, whatever was shared.
        for row in [
            P::DocCreate,
            P::DocComment,
            P::FilesView,
            P::TerminalManage,
            P::ThreadManage,
            P::ChatManage,
        ] {
            assert!(!HolderKind::Agents.fits(row, row.on()[0], false), "{row}");
        }
    }

    #[test]
    fn everyone_is_named_on_a_resource_of_the_system_only() {
        assert!(HolderKind::Everyone.fits(P::HostUse, ObjectKind::Host, true));
        assert!(HolderKind::Everyone.fits(P::TemplateView, ObjectKind::Template, true));
        assert!(
            !HolderKind::Everyone.fits(P::HostUse, ObjectKind::Host, false),
            "a member's host"
        );
        assert!(
            !HolderKind::Everyone.fits(P::ProviderUse, ObjectKind::Provider, true),
            "a provider has a home"
        );
        assert!(!HolderKind::Everyone.fits(P::ChatView, ObjectKind::Org, true));
        assert!(!HolderKind::Everyone.fits_defaults(P::HostUse));
    }

    #[test]
    fn a_shorthand_means_the_permission_of_the_nodes_own_area() {
        assert_eq!(
            NodePermission::View.on(NodeKind::Thread),
            Some(P::ThreadView)
        );
        assert_eq!(
            NodePermission::Interact.on(NodeKind::Org),
            Some(P::OrgInteract)
        );
        assert_eq!(
            NodePermission::Manage.on(NodeKind::Topic),
            Some(P::TopicManage)
        );
        assert_eq!(
            NodePermission::Archive.on(NodeKind::Project),
            Some(P::ProjectArchive)
        );
        assert_eq!(
            NodePermission::Archive.on(NodeKind::Org),
            Some(P::OrgArchive)
        );
        let name: PermissionName = "node.manage".parse().unwrap();
        assert_eq!(name.on(ObjectKind::Project), Some(P::ProjectManage));
        assert_eq!(name.on(ObjectKind::Provider), None);
        let exact: PermissionName = "files.view".parse().unwrap();
        assert_eq!(exact.on(ObjectKind::Environment), Some(P::FilesView));
        assert_eq!(
            exact.on(ObjectKind::Thread),
            None,
            "a thread's page has no Files row"
        );
        assert_eq!(exact.on(ObjectKind::Topic), None);
        assert!("node.delete".parse::<PermissionName>().is_err());
    }

    #[test]
    fn a_permission_is_written_as_its_key() {
        assert_eq!(
            serde_json::to_value(P::ThreadInteract).unwrap(),
            "thread.interact"
        );
        assert_eq!(
            serde_json::from_value::<Permission>(json!("mcp.use")).unwrap(),
            P::McpUse
        );
        assert!(serde_json::from_value::<Permission>(json!("thread.read")).is_err());
        assert_eq!("access.own".parse::<Permission>().unwrap(), P::AccessOwn);
        assert!(
            "node.view".parse::<Permission>().is_err(),
            "a shorthand is no row"
        );
        for permission in P::ALL {
            assert_eq!(
                permission.as_str().parse::<Permission>().unwrap(),
                *permission
            );
            assert_eq!(
                permission.as_str().split('.').next(),
                Some(permission.area().as_str())
            );
        }
    }

    #[test]
    fn permissions_sort_as_their_keys_do() {
        let mut held = vec![P::ThreadView, P::ChatView, P::AccessManage, P::FilesView];
        held.sort();
        assert_eq!(
            held,
            [P::AccessManage, P::ChatView, P::FilesView, P::ThreadView]
        );
    }

    #[test]
    fn the_three_choices_of_a_thread_name_its_session_and_nothing_of_its_environment() {
        assert_eq!(
            Choice::FullView.permissions(ObjectKind::Thread),
            [P::ChatView, P::DocView, P::ThreadView]
        );
        assert_eq!(
            Choice::FullInteract.permissions(ObjectKind::Thread),
            [P::ChatInteract, P::DocEdit, P::ThreadInteract]
        );
        assert_eq!(
            Choice::FullManage.permissions(ObjectKind::Thread),
            [P::ChatManage, P::DocManage, P::ThreadManage]
        );
        for choice in Choice::ALL {
            assert!(
                choice
                    .permissions(ObjectKind::Thread)
                    .iter()
                    .all(|row| !row.area().is_door()),
                "{choice}"
            );
        }
    }

    #[test]
    fn can_view_an_environment_is_looking_in_through_all_four_doors_and_can_manage_is_the_environment()
     {
        assert_eq!(
            Choice::FullView.permissions(ObjectKind::Environment),
            [
                P::EnvironmentView,
                P::FilesView,
                P::TerminalView,
                P::BrowserView,
                P::VncView,
                P::PortsView
            ]
        );
        assert_eq!(
            Choice::FullManage.permissions(ObjectKind::Environment),
            [P::EnvironmentManage],
            "which holds the Manage of the four doors"
        );
        assert_eq!(
            Choice::FullInteract.permissions(ObjectKind::Environment),
            [P::EnvironmentUse],
            "using an environment is talking to the agents that work in it"
        );
        assert_eq!(
            Choice::FullInteract.label_on(ObjectKind::Environment),
            "Can use"
        );
        assert_eq!(Choice::FullInteract.label_on(ObjectKind::Host), "Can use");
        assert_eq!(
            Choice::FullInteract.label_on(ObjectKind::Document),
            "Can edit"
        );
        assert_eq!(
            Choice::FullInteract.label_on(ObjectKind::Topic),
            "Can contribute"
        );
        assert_eq!(
            Choice::FullInteract.label_on(ObjectKind::Thread),
            "Can interact"
        );
        assert_eq!(P::TopicStartThreads.with(), Some(P::TopicInteract));
        assert_eq!(P::ProjectCreateTopics.with(), Some(P::ProjectInteract));
        assert!(
            P::TopicStartThreads
                .held_through()
                .any(|held| held == P::TopicInteract)
        );
    }

    #[test]
    fn no_choice_names_an_advanced_row_or_administration() {
        for choice in Choice::ALL {
            for kind in ObjectKind::ALL {
                for row in choice.permissions(*kind) {
                    assert!(row.exists_on(*kind), "{row} on a {kind}");
                    assert_ne!(row.level(), Level::Advanced, "{row}");
                    let administration = matches!(
                        row.area(),
                        Area::Settings | Area::Usage | Area::Audit | Area::Access | Area::Members
                    );
                    assert!(!administration, "{row}");
                }
            }
        }
    }

    #[test]
    fn somebody_stands_in_the_line_of_the_highest_choice_they_hold_whole() {
        let thread = ObjectKind::Thread;
        let shipped_viewer = [
            P::ChatView,
            P::ChatInteract,
            P::DocView,
            P::ThreadView,
            P::ThreadFork,
        ];
        assert_eq!(
            Choice::highest(thread, &shipped_viewer),
            Some(Choice::FullView)
        );
        let managing = [P::ChatManage, P::DocManage, P::ThreadManage];
        assert_eq!(Choice::highest(thread, &managing), Some(Choice::FullManage));
        let driving = [P::ChatManage, P::DocEdit, P::ThreadInteract];
        assert_eq!(
            Choice::highest(thread, &driving),
            Some(Choice::FullInteract)
        );
        let some_areas_only = [P::ThreadInteract, P::ChatView];
        assert_eq!(Choice::highest(thread, &some_areas_only), None);
        let environment = ObjectKind::Environment;
        assert_eq!(
            Choice::highest(environment, &[P::EnvironmentManage]),
            Some(Choice::FullManage)
        );
        let looking_in = [
            P::EnvironmentView,
            P::FilesView,
            P::TerminalView,
            P::BrowserView,
            P::VncView,
            P::PortsView,
        ];
        assert_eq!(
            Choice::highest(environment, &looking_in),
            Some(Choice::FullView)
        );
        assert_eq!(
            Choice::highest(
                environment,
                &[P::EnvironmentView, P::FilesView, P::TerminalView]
            ),
            None,
            "two doors are no choice"
        );
        assert_eq!(
            Choice::highest(environment, &[P::EnvironmentView, P::FilesManage]),
            None,
            "nor is one door at its Manage"
        );
        assert_eq!(
            Choice::highest(ObjectKind::Provider, &[P::ProviderUse]),
            Some(Choice::FullInteract)
        );
        assert_eq!(
            Choice::highest(thread, &[P::AuditView, P::SettingsManage]),
            None
        );
    }

    #[test]
    fn whoever_reads_a_thing_may_take_it_away_as_shipped_and_whoever_manages_it_holds_the_row() {
        for (export, manage, view) in [
            (P::ThreadExport, P::ThreadManage, P::ThreadView),
            (P::ChatExport, P::ChatManage, P::ChatView),
            (P::DocExport, P::DocManage, P::DocView),
        ] {
            assert!(export.is_export() && !view.is_export());
            assert_eq!(export.level(), Level::Advanced);
            assert_eq!(export.shipped(Placement::Managed), Some(Role::Viewer));
            assert_eq!(
                export.shipped(Placement::Connected),
                Some(Role::Viewer),
                "also on a person's machine: whoever reads a thing there may take it away"
            );
            assert_eq!(export.with(), Some(manage));
            assert_eq!(through(export), [manage]);
            assert_eq!(export.area(), view.area());
            assert_eq!(export.when_archived(), WhenArchived::Holds);
        }
        assert_eq!(Permission::EXPORTS.len(), 3);
        assert!(P::ThreadExport.exists_on(ObjectKind::Thread));
        assert!(!P::ThreadExport.exists_on(ObjectKind::Topic));
        assert!(P::ChatExport.exists_on(ObjectKind::Org));
        assert!(P::ChatExport.exists_on(ObjectKind::Thread));
        assert!(!P::ChatExport.exists_on(ObjectKind::Document));
        assert!(P::DocExport.exists_on(ObjectKind::Document));
        assert!(P::DocExport.exists_on(ObjectKind::Project));
        assert!("thread.export".parse::<Permission>().is_ok());
    }

    #[test]
    fn agents_are_named_in_the_export_rows_so_that_they_may_download() {
        for row in Permission::EXPORTS {
            assert!(
                HolderKind::Agents.fits(row, row.on()[0], false),
                "{row}: a live share names the agents there when they may download"
            );
            assert!(HolderKind::Role.fits(row, row.on()[0], false), "{row}");
        }
    }

    #[test]
    fn a_choice_travels_with_the_export_rows_of_the_areas_it_covers() {
        let thread = Choice::FullView.exports(ObjectKind::Thread);
        assert_eq!(thread, [P::ThreadExport, P::ChatExport, P::DocExport]);
        for choice in Choice::ALL {
            assert_eq!(
                choice.exports(ObjectKind::Thread),
                thread,
                "the three choices alike"
            );
            assert_eq!(
                choice.exports(ObjectKind::Topic),
                [P::ChatExport, P::DocExport]
            );
            assert_eq!(choice.exports(ObjectKind::Document), [P::DocExport]);
            assert_eq!(
                choice.exports(ObjectKind::Org),
                [P::ChatExport, P::DocExport]
            );
            assert!(choice.exports(ObjectKind::Provider).is_empty());
            assert!(
                choice.exports(ObjectKind::Mount).is_empty(),
                "a resource is not taken away as a file"
            );
            for row in choice.exports(ObjectKind::Thread) {
                assert!(row.exists_on(ObjectKind::Thread));
            }
        }
        assert!(
            Choice::FullInteract
                .exports(ObjectKind::Environment)
                .is_empty()
        );
    }

    #[test]
    fn archive_leaves_looking_and_saying_who_can() {
        assert_eq!(P::ThreadView.when_archived(), WhenArchived::Holds);
        assert_eq!(P::ChatView.when_archived(), WhenArchived::Holds);
        assert_eq!(P::UsageView.when_archived(), WhenArchived::Holds);
        assert_eq!(P::AuditView.when_archived(), WhenArchived::Holds);
        assert_eq!(P::AccessManage.when_archived(), WhenArchived::Holds);
        assert_eq!(
            P::ThreadManage.when_archived(),
            WhenArchived::HoldsToDeleteOrUnarchive
        );
        assert_eq!(
            P::TopicArchive.when_archived(),
            WhenArchived::HoldsToUnarchive
        );
        for gone in [
            P::ThreadInteract,
            P::ChatInteract,
            P::DocEdit,
            P::DocComment,
            P::FilesManage,
            P::ThreadFork,
        ] {
            assert_eq!(gone.when_archived(), WhenArchived::Withheld, "{gone}");
        }
        assert_eq!(P::SettingsManage.when_archived(), WhenArchived::Withheld);
    }

    #[test]
    fn talking_to_an_agent_goes_with_the_use_of_its_provider_and_its_environment() {
        assert_eq!(
            P::ThreadInteract.goes_with(),
            [
                GoesWith {
                    needs: P::ProviderUse,
                    offer: Offer::ToTheWork
                },
                GoesWith {
                    needs: P::EnvironmentUse,
                    offer: Offer::ToTheWork
                }
            ]
        );
        let starting: Vec<_> = P::TopicInteract
            .goes_with()
            .iter()
            .map(|with| with.needs)
            .collect();
        assert_eq!(
            starting,
            [
                P::ProviderUse,
                P::AgentUse,
                P::HostUse,
                P::TemplateUse,
                P::EnvironmentUse
            ]
        );
        assert_eq!(
            P::TopicStartThreads.goes_with(),
            P::TopicInteract.goes_with()
        );
        assert_eq!(
            P::ProjectCreateTopics.goes_with(),
            P::ProjectInteract.goes_with()
        );
        assert!(
            P::ThreadFork
                .goes_with()
                .iter()
                .all(|with| with.offer == Offer::Nothing)
        );
        assert!(P::ThreadArchive.goes_with().is_empty());
    }

    #[test]
    fn reading_a_thread_goes_with_looking_into_its_environment_and_setting_it_up_with_working_in_it()
     {
        let looking: Vec<_> = P::ThreadView
            .goes_with()
            .iter()
            .map(|with| with.needs)
            .collect();
        assert_eq!(
            looking,
            [
                P::EnvironmentView,
                P::FilesView,
                P::TerminalView,
                P::BrowserView,
                P::VncView,
                P::PortsView
            ]
        );
        let setting_up: Vec<_> = P::ThreadManage
            .goes_with()
            .iter()
            .filter(|with| with.offer == Offer::ToTheSameHolders)
            .map(|with| with.needs)
            .collect();
        assert_eq!(setting_up, [P::FilesManage, P::TerminalManage]);
        for row in [P::ThreadView, P::ThreadManage] {
            assert!(
                row.goes_with().iter().all(|with| {
                    ![P::EnvironmentManage, P::BrowserManage, P::VncManage].contains(&with.needs)
                }),
                "never the environment's Manage, and never the Manage of Browsers or VNC"
            );
        }
    }

    #[test]
    fn only_owners_and_admins_pass_control_of_a_page_on() {
        assert_eq!(P::AccessManage.named_by(), NamedBy::OwnerOrAdmin);
        assert_eq!(P::AccessOwn.named_by(), NamedBy::OwnerOrAdmin);
        assert_eq!(P::AccessExternal.named_by(), NamedBy::Admin);
        assert_eq!(P::ThreadManage.named_by(), NamedBy::WhoeverChangesThePage);
    }

    #[test]
    fn a_defaults_page_has_rows_for_what_can_be_made_inside_its_place() {
        assert!(ObjectKind::Provider.made_in(PlaceKind::Org));
        assert!(
            !ObjectKind::Provider.made_in(PlaceKind::Project),
            "a resource's home is an org"
        );
        assert!(ObjectKind::Environment.made_in(PlaceKind::Topic));
        assert!(ObjectKind::Thread.made_in(PlaceKind::Project));
        assert!(!ObjectKind::Project.made_in(PlaceKind::Project));
        assert!(!ObjectKind::Org.made_in(PlaceKind::Topic));
        assert!(ObjectKind::Org.made_in(PlaceKind::System));
    }

    #[test]
    fn the_options_of_a_provider_entry_are_read_and_written_back() {
        let written = json!({ "models": ["gpt-x", "gpt-y"], "efforts": ["high"], "arrangements": ["separated"] });
        let Options::ProviderUse(options) = Options::parse(P::ProviderUse, &written).unwrap()
        else {
            panic!("the use of a provider takes options");
        };
        assert!(options.allows_model("gpt-x"));
        assert!(!options.allows_model("gpt-z"));
        assert!(options.allows_effort("high"));
        assert!(!options.allows_effort("low"));
        assert!(options.allows_arrangement(Arrangement::Separated));
        assert!(!options.allows_arrangement(Arrangement::HandedOver));
        assert_eq!(Options::ProviderUse(options).to_json(), written);
    }

    #[test]
    fn an_option_that_is_left_out_does_not_narrow() {
        let Options::ProviderUse(options) = Options::parse(P::ProviderUse, &json!({})).unwrap()
        else {
            panic!("the use of a provider takes options");
        };
        assert!(options.allows_model("anything"));
        assert!(options.allows_effort("anything"));
        assert!(options.allows_arrangement(Arrangement::CredentialShared));
        assert_eq!(
            Options::parse(P::ProviderUse, &Json::Null)
                .unwrap()
                .to_json(),
            json!({})
        );
    }

    #[test]
    fn an_unknown_option_is_refused_by_name() {
        let faults =
            Options::parse(P::ProviderUse, &json!({ "models": ["m"], "budget": 5 })).unwrap_err();
        assert_eq!(faults.len(), 1);
        assert_eq!(faults[0].path, "budget");
        let faults =
            Options::parse(P::HostUse, &json!({ "resources_max": { "gpus": 2 } })).unwrap_err();
        assert_eq!(faults[0].path, "resources_max.gpus");
    }

    #[test]
    fn a_permission_that_takes_no_options_refuses_any() {
        assert_eq!(
            Options::parse(P::ThreadInteract, &json!({})).unwrap(),
            Options::None
        );
        assert_eq!(
            Options::parse(P::AgentUse, &Json::Null).unwrap(),
            Options::None
        );
        let faults = Options::parse(P::ThreadInteract, &json!({ "models": ["m"] })).unwrap_err();
        assert_eq!(faults[0].path, "models");
        assert!(
            P::ALL
                .iter()
                .all(|permission| permission.takes_options()
                    == permission.options_schema().is_some())
        );
    }

    #[test]
    fn every_faulty_option_is_reported_with_its_place() {
        let template = new_id();
        let written = json!({
            "max_environments": -1,
            "storages": ["fast", 7],
            "resources_max": { "memory_mb": 0, "pids": 64 },
            "templates": [template, "not-an-id"],
            "arrangements": ["separated"],
        });
        let faults = Options::parse(P::HostUse, &written).unwrap_err();
        let paths: Vec<_> = faults.iter().map(|fault| fault.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "max_environments",
                "storages[1]",
                "resources_max.memory_mb",
                "templates[1]",
                "arrangements"
            ]
        );
        let faults =
            Options::parse(P::ProviderUse, &json!({ "arrangements": ["together"] })).unwrap_err();
        assert_eq!(
            faults[0].path, "arrangements[0]",
            "everything on one machine is never chosen"
        );
    }

    #[test]
    fn several_entries_combine_permissively() {
        let one = ProviderUse {
            models: Some(vec!["a".into()]),
            efforts: Some(vec!["low".into()]),
            arrangements: None,
        };
        let other = ProviderUse {
            models: Some(vec!["b".into(), "a".into()]),
            efforts: None,
            arrangements: Some(vec![Arrangement::Separated]),
        };
        let both = one.or(&other);
        assert_eq!(both.models, Some(vec!["a".to_owned(), "b".to_owned()]));
        assert_eq!(
            both.efforts, None,
            "an entry that names no efforts allows them all"
        );
        assert_eq!(both.arrangements, None);
    }

    #[test]
    fn of_two_numbers_the_larger_counts_between_entries_and_the_narrower_between_host_and_template()
    {
        let small = ResourceLimits {
            cpu_millis: Some(2000),
            memory_mb: Some(4096),
            pids: None,
            disk_gb: Some(20),
        };
        let large = ResourceLimits {
            cpu_millis: Some(8000),
            memory_mb: Some(2048),
            pids: Some(512),
            disk_gb: None,
        };
        let either = small.or(&large);
        assert_eq!(
            (either.cpu_millis, either.memory_mb),
            (Some(8000), Some(4096))
        );
        assert_eq!(
            (either.pids, either.disk_gb),
            (None, None),
            "no cap in one is no cap"
        );
        let both = small.and(&large);
        assert_eq!((both.cpu_millis, both.memory_mb), (Some(2000), Some(2048)));
        assert_eq!((both.pids, both.disk_gb), (Some(512), Some(20)));

        let one = HostUse {
            max_environments: Some(2),
            ..HostUse::default()
        };
        let other = HostUse {
            max_environments: Some(5),
            storages: Some(vec!["fast".into()]),
            ..HostUse::default()
        };
        let host = one.or(&other);
        assert_eq!(host.max_environments, Some(5));
        assert!(
            host.allows_storage("slow"),
            "the first entry names no storages"
        );
        assert!(other.allows_storage("fast") && !other.allows_storage("slow"));
    }

    #[test]
    fn the_catalogue_is_written_as_data_for_the_pages() {
        let catalogue = catalogue();
        let written = serde_json::to_value(&catalogue).unwrap();
        let fork = written["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|def| def["name"] == "thread.fork")
            .unwrap();
        assert_eq!(
            *fork,
            json!({
                "name": "thread.fork", "area": "thread", "level": "advanced", "with": "thread.manage",
                "on": ["thread"], "label": "Fork", "allows": P::ThreadFork.allows(), "profiles": true,
                "holders": ["role", "group", "members", "user"], "options_schema": null
            })
        );
        assert_eq!(written["choices"][1]["key"], "full_interact");
        assert_eq!(
            written["choices"][1]["permissions"]["document"],
            json!(["doc.edit"])
        );
        assert_eq!(
            written["choices"][1]["permissions"]["environment"],
            json!(["environment.use"])
        );
        assert_eq!(
            written["roles"][3],
            json!({ "key": "admin", "name": "Admin" })
        );
        assert_eq!(
            serde_json::from_value::<Catalogue>(written).unwrap(),
            catalogue
        );
    }
}
