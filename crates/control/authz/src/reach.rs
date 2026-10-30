//! What an agent may read, write and send, kind by kind: the reach policy, and entries by area whose holder is the agents of a thread or of a place, inherited on both sides; a chat and a thread each written to by a right of its own.
//!
//! An agent is not a person, holds no role and is named by no group: what it reaches is what was given to the
//! **agents of a place on its way**. A place's agents are every agent thread at or under it, those started later
//! among them, so an entry that names the agents of a project is held by the agent of every thread in it. The
//! entries are in the rows of three areas, shared apart:
//!
//! | Area | Read | Written |
//! |------|------|---------|
//! | the conversation of a thread | `thread.view` | `thread.interact`: a message is sent to its agent |
//! | a chat | `chat.view` | `chat.interact`: a message is posted |
//! | a document | `doc.view` | `doc.edit`, on the document to change it, on its node to make a new one there |
//!
//! A document has a third level that the other two lack: `doc.manage` lets the agent delete the document, move it
//! to another place, bring it back and put an earlier version back (the thing itself, and not what is in it). It is
//! given to the agents like the other two, on the document or on the node it lies beside, and it holds the two below
//! it. No agent manages a conversation or a chat, whatever was shared.
//!
//! Which row of which page is read is the same rule as for a person: a row follows the row of the same name on
//! the page above it (a document follows its node's, a node follows the defaults of the place it lies in) until
//! it is cut loose, and a lock above decides what lies below. So a share of a place reaches what lies inside it
//! and follows its defaults, what is made there later included, and a restricted node or a private document is
//! left out unless it was shared itself. A higher level holds the lower one: whoever may write an area may read it.
//!
//! Two things reach an agent without any entry, and neither is a share: the policy `sharing.agent_reach` (the
//! conversations of the other threads of its topic, or of its project, never a chat nor a document, and never a
//! restricted thread), and its own topic and thread: the other threads of the topic are always listed, and may
//! always be sent to, as they may be announced to; its own conversation is its own to read.
//!
//! Nothing here is cached: an agent's reach is worked out from the pages at every call, so that an entry which
//! was removed ends at the next one. Settings are not read here; the policy is given in.

use std::collections::{BTreeSet, HashSet};

use orch_access_contract::pages::{Chain, Entry, Holder, PageKind, StoredKind};
use orch_access_contract::permissions::{HolderKind, Permission, Profile};
use orch_access_contract::roles::{NodeKind, PlaceRef};
use orch_db::Conn;
use orch_db::queries::shares as rows;
use orch_types::{Id, Time};

use crate::Error;
use crate::documents::{self, Document};
use crate::engine::{Graph, NodeFacts, PageRef, Site, Store};

/// The permissions by which an agent reads, writes and manages: those that the agents of a place can be named in.
pub const AREAS: [Permission; 7] = [
    Permission::ThreadView,
    Permission::ThreadInteract,
    Permission::ChatView,
    Permission::ChatInteract,
    Permission::DocView,
    Permission::DocEdit,
    Permission::DocManage,
];

/// How far the agents of a topic reach by policy (`sharing.agent_reach`), apart from any share.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum AgentReach {
    /// Nothing.
    #[default]
    None,
    /// The conversations of the other threads of its topic.
    Topic,
    /// The conversations of every thread of its project.
    Project,
}

impl AgentReach {
    /// The policy a setting names: `none`, `topic` or `project`.
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "none" => Some(Self::None),
            "topic" => Some(Self::Topic),
            "project" => Some(Self::Project),
            _ => None,
        }
    }
}

/// The independent implicit reach of public documents at an agent's topic and its main threads.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum DocumentPolicy {
    /// Only explicit shares and the agent's creation entry apply.
    #[default]
    None,
    /// Read public material.
    Read,
    /// Also edit and create public material, without Manage or Export.
    Write,
}

impl DocumentPolicy {
    /// The value of sharing.agent_documents resolved at the topic.
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "none" => Some(Self::None),
            "read" => Some(Self::Read),
            "write" => Some(Self::Write),
            _ => None,
        }
    }
}

/// Why an agent has something.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Via {
    /// The policy of its topic or project lets it.
    Policy,
    /// An entry names the agents of a place on its way.
    Share,
    /// It lies in its own topic or is its own thread.
    Topic,
}

/// What an agent may do with one thing: read it, write it, manage it, and why each.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Standing {
    /// Why it may read it; nothing when it may not.
    pub read: Option<Via>,
    /// Why it may write it (send to it, post in it, edit it); nothing when it may not.
    pub write: Option<Via>,
    /// Why it may manage it (delete it, move it, bring it back, put an earlier version back); nothing when it
    /// may not. Only a document is managed by an agent, and only by a share.
    pub manage: Option<Via>,
    /// For what a share gives: who wrote the entry that does, the one that manages, the one that writes or the
    /// one that reads, in that order.
    pub shared_by: Option<Id>,
}

impl Standing {
    /// Whether it may read.
    #[must_use]
    pub const fn reads(&self) -> bool {
        self.read.is_some()
    }

    /// Whether it may write.
    #[must_use]
    pub const fn writes(&self) -> bool {
        self.write.is_some()
    }

    /// Whether it may manage.
    #[must_use]
    pub const fn manages(&self) -> bool {
        self.manage.is_some()
    }

    /// Whether it holds anything on the thing.
    #[must_use]
    pub const fn any(&self) -> bool {
        self.read.is_some() || self.write.is_some() || self.manage.is_some()
    }
}

/// The agent whose reach is asked about: a thread, with the nodes it lies in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    /// The thread.
    pub thread: Id,
    /// The nodes from its root org down to it, itself included: where the agents named by an entry are looked for.
    pub path: Vec<Id>,
    /// The topic it works in.
    pub topic: Id,
    /// The project of that topic.
    pub project: Option<Id>,
    /// Its root org.
    pub root: Id,
    /// Its name, which is the thread's.
    pub name: String,
    /// Its first owner: the person who answers for what it does.
    pub owner: Id,
}

fn nearest(facts: &NodeFacts, graph: &Graph, kind: NodeKind) -> Option<Id> {
    facts
        .path
        .iter()
        .rev()
        .filter_map(|id| graph.get(*id))
        .find(|node| node.kind == kind)
        .map(|node| node.id)
}

impl Agent {
    /// The agent of the thread `thread`. Nothing where it is not a thread that is there: deleted, or under a
    /// node that was.
    pub async fn of(conn: &mut Conn, thread: Id) -> Result<Option<Self>, Error> {
        let graph = Graph::load(conn, &[thread]).await?;
        let Some(node) = graph.get(thread) else {
            return Ok(None);
        };
        if node.kind != NodeKind::Thread || graph.site_of(thread).is_none() {
            return Ok(None);
        }
        let Some(topic) = nearest(node, &graph, NodeKind::Topic) else {
            return Ok(None);
        };
        Ok(Some(Self {
            thread,
            path: node.path.clone(),
            topic,
            project: nearest(node, &graph, NodeKind::Project),
            root: node.root,
            name: node.name.clone(),
            owner: node.first_owner,
        }))
    }
}

// ---------------------------------------------------------------- the entries that give an agent something

/// One entry that names the agents of a place on an agent's way, with the row it stands in.
#[derive(Debug, Clone, PartialEq)]
pub struct Grant {
    /// The entry.
    pub entry: Id,
    /// The page it stands on: what it is of, which, and whether that is the thing itself or the defaults it gives.
    pub page: PageRef,
    /// The row's permission.
    pub permission: Permission,
    /// The row's profile.
    pub profile: Profile,
    /// The place whose agents it names.
    pub holder: Id,
    /// Who wrote it.
    pub by: Option<Id>,
    /// When.
    pub at: Time,
}

fn grant_of(row: &rows::GrantRow) -> Result<Grant, Error> {
    Ok(Grant {
        entry: row.entry_id,
        page: PageRef {
            kind: row.object_kind.parse()?,
            id: row.object_id,
            page: row.page.parse()?,
        },
        permission: row
            .permission
            .parse::<Permission>()
            .map_err(|error| Error::Stored(error.to_string()))?,
        profile: row.profile.parse()?,
        holder: row.holder_id,
        by: row.created_by,
        at: Time::new(row.created_at),
    })
}

fn area_words() -> Vec<String> {
    AREAS
        .iter()
        .map(|permission| permission.as_str().to_owned())
        .collect()
}

/// Every entry that names the agents of a place on the way of `agent` and counts at `now`: the whole of what was
/// given to it, before the rows are followed. Where each entry reaches is for [`Reach`] to say.
pub async fn grants(conn: &mut Conn, agent: &Agent, now: Time) -> Result<Vec<Grant>, Error> {
    rows::grants_to(conn, &agent.path, &area_words(), now.get())
        .await?
        .iter()
        .map(grant_of)
        .collect()
}

/// Every entry that names agents of any place, on the pages of one thing: how a thing says what it was shared
/// with. `kind` is what the page is of.
pub async fn grants_on(
    conn: &mut Conn,
    kind: StoredKind,
    object: Id,
    now: Time,
) -> Result<Vec<Grant>, Error> {
    rows::grants_on(conn, kind.as_str(), object, &area_words(), now.get())
        .await?
        .iter()
        .map(grant_of)
        .collect()
}

/// Every entry that names agents of any place that counts at `now`.
pub async fn all_grants(conn: &mut Conn, now: Time) -> Result<Vec<Grant>, Error> {
    rows::all_grants(conn, &area_words(), now.get())
        .await?
        .iter()
        .map(grant_of)
        .collect()
}

// ---------------------------------------------------------------- the reach

/// What a reach is asked about, so that the rows of all of it are read together.
#[derive(Debug, Clone, Default)]
pub struct Things {
    /// Nodes: threads, for their conversations, and any node for its chat and the documents beside it.
    pub nodes: Vec<Id>,
    /// Documents.
    pub documents: Vec<Document>,
    /// Direct and group chats.
    pub people_chats: Vec<Id>,
}

/// The reach of one agent, read for some things at one moment: a few statements for any number of them.
#[derive(Debug)]
pub struct Reach {
    agent: Agent,
    policy: AgentReach,
    now: Time,
    graph: Graph,
    store: Store,
    stopped_chats: HashSet<Id>,
    stopped_nodes: HashSet<Id>,
    documents_policy: DocumentPolicy,
    side_threads: HashSet<Id>,
}

impl Reach {
    /// Reads what is needed to say what `agent` may do with `things`. `policy` is `sharing.agent_reach` as it
    /// resolves at the agent's topic.
    pub async fn load(
        conn: &mut Conn,
        agent: Agent,
        policy: AgentReach,
        now: Time,
        things: &Things,
    ) -> Result<Self, Error> {
        let mut ids: Vec<Id> = agent.path.clone();
        ids.extend(things.nodes.iter().copied());
        ids.extend(things.documents.iter().map(|document| document.node));
        let mut graph = Graph::load(conn, &ids).await?;
        graph
            .extend_documents(
                conn,
                &things
                    .documents
                    .iter()
                    .map(|document| document.id)
                    .collect::<Vec<_>>(),
            )
            .await?;
        let mut pages: HashSet<PageRef> = HashSet::new();
        let mut sites: Vec<Site> = things
            .nodes
            .iter()
            .filter_map(|node| graph.site_of(*node))
            .collect();
        sites.extend(
            things
                .documents
                .iter()
                .filter_map(|document| documents::site_of(&graph, document)),
        );
        graph.load_profiles(conn, &sites).await?;
        for site in &sites {
            pages.insert(site.page());
            pages.extend(
                graph
                    .document_ancestors(site.id)
                    .into_iter()
                    .map(|id| PageRef::live(StoredKind::DocumentContainer, id)),
            );
            if let Some(node) = site.beside {
                pages.insert(PageRef::live(StoredKind::Node, node));
            }
            pages.extend(site.chain.iter().copied().map(PageRef::defaults_of));
        }
        for chat in &things.people_chats {
            pages.insert(PageRef::live(StoredKind::Chat, *chat));
        }
        let pages: Vec<PageRef> = pages.into_iter().collect();
        // The Export rows are read with the six: they say whether the agents may take a thing away as a file.
        let rows: Vec<Permission> = AREAS
            .iter()
            .chain(Permission::EXPORTS.iter())
            .copied()
            .collect();
        let store = Store::load(conn, &pages, &rows).await?;
        let stops = orch_db::queries::chats::posting::stopped(conn, agent.thread).await?;
        let stopped_chats = stops.iter().map(|(chat, _)| *chat).collect();
        let stopped_nodes = stops.iter().filter_map(|(_, node)| *node).collect();
        let side_threads = orch_db::queries::threads::side_threads_among(conn, &ids)
            .await?
            .into_iter()
            .collect();
        Ok(Self {
            agent,
            policy,
            now,
            graph,
            store,
            stopped_chats,
            stopped_nodes,
            documents_policy: DocumentPolicy::None,
            side_threads,
        })
    }

    /// Applies the independently resolved topic-document setting to this same current page resolver.
    /// Explicit-share-only callers retain `None` unless they supply the setting.
    #[must_use]
    pub fn with_document_policy(mut self, policy: DocumentPolicy) -> Self {
        self.documents_policy = policy;
        self
    }

    fn policy_documents(&self, site: &Site, node: Id) -> Standing {
        let public_place = self.graph.get(node).is_some_and(|facts| {
            node == self.agent.topic
                || (facts.kind == NodeKind::Thread
                    && self.is_in_topic(facts)
                    && !self.side_threads.contains(&node)
                    && self
                        .graph
                        .site_of(node)
                        .is_some_and(|site| !self.restricted(&site)))
        });
        if !public_place || self.documents_policy == DocumentPolicy::None {
            return Standing::default();
        }
        let public_row = |permission| {
            !self
                .chain(site, permission)
                .cut_below(PlaceRef::Node(self.agent.root))
        };
        let read = public_row(Permission::DocView);
        Standing {
            read: read.then_some(Via::Policy),
            write: (read
                && self.documents_policy == DocumentPolicy::Write
                && public_row(Permission::DocEdit)
                && !site.is_archived())
            .then_some(Via::Policy),
            manage: None,
            shared_by: None,
        }
    }

    fn including_document_policy(&self, shared: Standing, site: &Site, node: Id) -> Standing {
        let policy = self.policy_documents(site, node);
        Standing {
            read: shared.read.or(policy.read),
            write: shared.write.or(policy.write),
            ..shared
        }
    }

    /// The agent.
    #[must_use]
    pub fn agent(&self) -> &Agent {
        &self.agent
    }

    /// The nodes that were read.
    #[must_use]
    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    /// The chain of a row of a site's page, with the rows it follows.
    fn chain(&self, site: &Site, permission: Permission) -> Chain<'_> {
        let profile = self.graph.profile(site, permission);
        let mut chain = Chain::of(self.store.row(site.page(), Profile::Any, permission));
        for id in self.graph.document_ancestors(site.id) {
            chain = chain.in_document_container(
                id,
                self.store.row(
                    PageRef::live(StoredKind::DocumentContainer, id),
                    Profile::Any,
                    permission,
                ),
            );
        }
        if let Some(node) = site.beside {
            chain = chain.beside(
                node,
                self.store.row(
                    PageRef::live(StoredKind::Node, node),
                    Profile::Any,
                    permission,
                ),
            );
        }
        for place in &site.chain {
            chain = chain.under(
                *place,
                self.store
                    .row(PageRef::defaults_of(*place), profile, permission),
            );
        }
        chain
    }

    /// The first entry of the rows `permissions` of a site that names the agents of a place on this agent's way.
    fn named(&self, site: &Site, permissions: &[Permission]) -> Option<(Id, Option<Id>)> {
        permissions.iter().find_map(|permission| {
            agents_in(&self.chain(site, *permission), &self.agent.path, self.now)
        })
    }

    /// The first entry of the rows `permissions` of a direct or group chat that names this agent's places.
    fn named_for_chat(&self, chat: Id, permissions: &[Permission]) -> Option<(Id, Option<Id>)> {
        permissions.iter().find_map(|permission| {
            let chain = Chain::of(self.store.row(
                PageRef::live(StoredKind::Chat, chat),
                Profile::Any,
                *permission,
            ));
            agents_in(&chain, &self.agent.path, self.now)
        })
    }

    fn share_standing(
        read: Option<(Id, Option<Id>)>,
        write: Option<(Id, Option<Id>)>,
        manage: Option<(Id, Option<Id>)>,
    ) -> Standing {
        Standing {
            read: read.or(write).or(manage).map(|_| Via::Share),
            write: write.or(manage).map(|_| Via::Share),
            manage: manage.map(|_| Via::Share),
            shared_by: manage.or(write).or(read).and_then(|(_, by)| by),
        }
    }

    /// The thread's own topic, and its project, as the agent's topic and project are known.
    fn is_in_topic(&self, facts: &NodeFacts) -> bool {
        nearest(facts, &self.graph, NodeKind::Topic) == Some(self.agent.topic)
    }

    /// Whether a thread lies in the agent's project: the one its topic lies in directly, and the projects inside
    /// that one. The agent's project is the nearest one, so the folders above it are not reached, and the
    /// folders inside it are, as the orgs inside an org are.
    fn is_in_project(&self, facts: &NodeFacts) -> bool {
        self.agent
            .project
            .is_some_and(|project| facts.path.contains(&project))
    }

    /// What the agent may do with the conversation of a thread: read it, and send to its agent. `side` says it is
    /// a side thread, which is an aside of another thread and is none of these.
    #[must_use]
    pub fn conversation(&self, thread: Id, side: bool) -> Standing {
        if thread == self.agent.thread {
            return Standing {
                read: Some(Via::Topic),
                write: None,
                manage: None,
                shared_by: None,
            };
        }
        let (Some(facts), Some(site)) = (self.graph.get(thread), self.graph.site_of(thread)) else {
            return Standing::default();
        };
        if side {
            return Standing::default();
        }
        let shared = Self::share_standing(
            self.named(&site, &[Permission::ThreadView, Permission::ThreadInteract]),
            self.named(&site, &[Permission::ThreadInteract]),
            None,
        );
        let in_topic = self.is_in_topic(facts);
        let by_policy = !self.restricted(&site)
            && match self.policy {
                AgentReach::None => false,
                AgentReach::Topic => in_topic,
                AgentReach::Project => self.is_in_project(facts),
            };
        Standing {
            read: shared.read.or(by_policy.then_some(Via::Policy)),
            write: if in_topic {
                Some(Via::Topic)
            } else {
                shared.write
            },
            manage: None,
            shared_by: shared.shared_by,
        }
    }

    /// Whether a thread's View row is cut loose below its root org: it is restricted, and what the policy reaches
    /// leaves it out.
    fn restricted(&self, site: &Site) -> bool {
        let Some(root) = site.root else {
            return false;
        };
        self.chain(site, Permission::ThreadView)
            .cut_below(PlaceRef::Node(root))
    }

    /// What the agent may do with the chat of a node: read it and post in it.
    #[must_use]
    pub fn node_chat(&self, node: Id) -> Standing {
        let Some(site) = self.graph.site_of(node) else {
            return Standing::default();
        };
        let mut standing = Self::share_standing(
            self.named(&site, &[Permission::ChatView, Permission::ChatInteract]),
            self.named(&site, &[Permission::ChatInteract]),
            None,
        );
        if self.stopped_nodes.contains(&node) {
            standing.write = None;
        }
        standing
    }

    /// What the agent may do with a direct or group chat: read it and post in it.
    #[must_use]
    pub fn people_chat(&self, chat: Id) -> Standing {
        let mut standing = Self::share_standing(
            self.named_for_chat(chat, &[Permission::ChatView, Permission::ChatInteract]),
            self.named_for_chat(chat, &[Permission::ChatInteract]),
            None,
        );
        if self.stopped_chats.contains(&chat) {
            standing.write = None;
        }
        standing
    }

    /// What the agent may do with a document: read it, with its files and versions, edit it, and manage it. A
    /// level holds the ones below it, so an entry in `doc.manage` alone reads and writes as well. The document may
    /// be one that was deleted: what the agent may do with it is then what it could do before, which is how it
    /// is asked whether it may bring it back.
    #[must_use]
    pub fn document(&self, document: &Document) -> Standing {
        let Some(site) = documents::site_of(&self.graph, document) else {
            return Standing::default();
        };
        let shared = Self::share_standing(
            self.named(&site, &DOCUMENT_READ),
            self.named(&site, &DOCUMENT_WRITE),
            self.named(&site, &DOCUMENT_MANAGE),
        );
        self.including_document_policy(shared, &site, document.node)
    }

    /// What the agent may do with the documents beside a node: read them (some, at least), make new ones there,
    /// and manage the ones that follow the node's row. Its `write` is the right to make a document, which is Edit
    /// on the node itself.
    #[must_use]
    pub fn documents_of(&self, node: Id) -> Standing {
        let Some(site) = self.graph.site_of(node) else {
            return Standing::default();
        };
        let shared = Self::share_standing(
            self.named(&site, &DOCUMENT_READ),
            self.named(&site, &DOCUMENT_WRITE),
            self.named(&site, &DOCUMENT_MANAGE),
        );
        self.including_document_policy(shared, &site, node)
    }

    /// The places whose agents are named in a row that gives a document to agents, on the document's own page or
    /// on any page it follows: every share that reaches it, told by the place it was made for. The agent's own
    /// places are among them where a share of theirs reaches it, and are the caller's to leave out.
    #[must_use]
    pub fn shared_with(&self, document: &Document) -> BTreeSet<Id> {
        let Some(site) = documents::site_of(&self.graph, document) else {
            return BTreeSet::new();
        };
        DOCUMENT_READ
            .iter()
            .flat_map(|permission| self.chain(&site, *permission).holders(self.now))
            .filter_map(|named| match &named.entry.holder {
                Holder::Agents { id } => Some(*id),
                _ => None,
            })
            .collect()
    }

    /// A hint counts only while that agents/doc.view entry is in this document's current followed
    /// chain. Another independent share cannot revive a hint behind a private row.
    #[must_use]
    pub fn first_reading(&self, document: &Document, hinted: &HashSet<Id>) -> bool {
        documents::site_of(&self.graph, document).is_some_and(|site| {
            self.document(document).reads()
                && self
                    .chain(&site, Permission::DocView)
                    .holders(self.now)
                    .iter()
                    .any(|named| {
                        hinted.contains(&named.entry.id)
                            && named.entry.holder.is_agents_on(&self.agent.path)
                    })
        })
    }

    /// Whether the node, or a node above it, is archived: what is under it is read-only.
    #[must_use]
    pub fn archived(&self, node: Id) -> bool {
        self.graph
            .site_of(node)
            .is_some_and(|site| site.is_archived())
    }
}

/// The rows that give an agent a document to read: each level holds the ones below it.
const DOCUMENT_READ: [Permission; 3] = [
    Permission::DocView,
    Permission::DocEdit,
    Permission::DocManage,
];
/// The rows that give an agent a document to change, or the right to make one beside a node.
const DOCUMENT_WRITE: [Permission; 2] = [Permission::DocEdit, Permission::DocManage];
/// The rows that give an agent a document to manage.
const DOCUMENT_MANAGE: [Permission; 1] = [Permission::DocManage];

/// The first entry of a row, or of what it follows, that names the agents of one of the places on `path`.
fn agents_in(chain: &Chain<'_>, path: &[Id], now: Time) -> Option<(Id, Option<Id>)> {
    chain
        .holders(now)
        .into_iter()
        .map(|named| named.entry)
        .find(|entry: &&Entry| {
            entry.holder.kind() == HolderKind::Agents && entry.holder.is_agents_on(path)
        })
        .map(|entry| (entry.id, entry.created_by))
}

/// The page kinds a grant stands on that are a thing and not a place's defaults: used to tell the own entries of
/// a place from the ones it gives to what is made in it.
#[must_use]
pub const fn is_defaults(grant: &Grant) -> bool {
    matches!(grant.page.page, PageKind::Defaults)
}

// ---------------------------------------------------------------- taking a thing away as a file (the export of a place)

impl Reach {
    /// Whether the agent may copy the conversation of a thread into its workspace: its own, which is its own
    /// to keep, or one whose Export row names the agents of a place on its way. A side thread is none of these.
    #[must_use]
    pub fn may_download_conversation(&self, thread: Id, side: bool) -> bool {
        if thread == self.agent.thread {
            return true;
        }
        if side {
            return false;
        }
        self.graph
            .site_of(thread)
            .is_some_and(|site| self.named(&site, &[Permission::ThreadExport]).is_some())
    }

    /// Whether the agent may copy the chat of a node into its workspace: its Export row names the agents.
    #[must_use]
    pub fn may_download_node_chat(&self, node: Id) -> bool {
        self.graph
            .site_of(node)
            .is_some_and(|site| self.named(&site, &[Permission::ChatExport]).is_some())
    }

    /// Whether the agent may copy a direct or group chat into its workspace.
    #[must_use]
    pub fn may_download_people_chat(&self, chat: Id) -> bool {
        self.named_for_chat(chat, &[Permission::ChatExport])
            .is_some()
    }

    /// Whether the agent may copy a document, with its versions, into its workspace: its Export row names the
    /// agents, or the agent of this very thread made it.
    #[must_use]
    pub fn may_download_document(&self, document: &Document) -> bool {
        if document.made_by_thread == Some(self.agent.thread) {
            return true;
        }
        documents::site_of(&self.graph, document)
            .is_some_and(|site| self.named(&site, &[Permission::DocExport]).is_some())
    }
}
