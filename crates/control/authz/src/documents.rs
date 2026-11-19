//! A document's page: the Documents rows of its node followed until changed (view, create, comment, edit, manage), private until shared, who makes and moves one, and what an agent may read and edit, where Edit on a place is what makes documents there.
//!
//! A document is described to the access engine as a node is: by a [`Site`].
//! Its page is its own, and a new one stores nothing but its Owners row: each
//! of its rows follows the row of the same name on the page of the node it is
//! written beside, which follows the defaults of the places above that node.
//! So a document is open to the people of the place it is in, by what that
//! place gives them there and from above, changes with that page, and beside a
//! restricted thread is as closed as the thread. Cutting its rows loose makes
//! it private: its owners' alone, and the admins of the place, who reach it by
//! naming themselves. A lock on a Documents row of a defaults page holds the
//! documents under it as it holds the nodes.
//!
//! Everything here is asked of the same [`Frame`] as a question about a node,
//! in the same few statements. Under an archived node a document is read-only:
//! every permission but those that read it is withheld, as for the node.
//!
//! Making a document takes `doc.create` on the node it is to stand beside,
//! which whoever holds `doc.edit` there holds as well; moving one takes
//! `doc.manage` on it and `doc.create` where it goes. Whether `docs.enabled`
//! resolves true there is a setting and is the caller's to ask.

use std::collections::BTreeMap;

use orch_access_contract::decisions::{Decision, Held, ObjectRef, Refusal, Through};
use orch_access_contract::pages::{PageState, StoredKind};
use orch_access_contract::permissions::{ObjectKind, Permission, Placement};
use orch_access_contract::roles::{self, PlaceRef};
use orch_db::Conn;
use orch_db::queries::documents as rows;
use orch_types::{Id, Time};

use crate::engine::{Frame, Graph, Site};
use crate::nodes::{archive_allows, every_permission};
use crate::{Act, Error, Policy, Principal, holders, missing};

/// A document, as a decision about it needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    /// Text or a distinct document-only container.
    pub kind: ObjectKind,
    /// Its identifier.
    pub id: Id,
    /// The node it is written beside.
    pub node: Id,
    /// That node's root org.
    pub root: Id,
    /// Its first owner.
    pub owner: Id,
    /// Its title.
    pub title: String,
    /// Whether it was deleted.
    pub deleted: bool,
    /// The thread of the agent that made it, where one did.
    pub made_by_thread: Option<Id>,
    /// That agent's name then.
    pub made_by_agent: Option<String>,
}

impl Document {
    /// A document as its row says it.
    #[must_use]
    pub fn of_row(row: &rows::DocumentRow) -> Self {
        Self {
            kind: ObjectKind::Document,
            id: row.id,
            node: row.node_id,
            root: row.org_id,
            owner: row.owner_id,
            title: row.title.clone(),
            deleted: row.deleted_at.is_some(),
            made_by_thread: row.created_by_thread,
            made_by_agent: row.created_by_agent.clone(),
        }
    }

    /// A folder uses the document permission family without an editable text document row.
    #[must_use]
    pub fn of_container(row: &rows::containers::ContainerRow) -> Self {
        Self {
            kind: ObjectKind::DocumentContainer,
            id: row.id,
            node: row.node_id,
            root: row.org_id,
            owner: row.owner_id,
            title: row.name.clone(),
            deleted: row.deleted_at.is_some(),
            made_by_thread: row.created_by_thread,
            made_by_agent: row.created_by_agent.clone(),
        }
    }

    /// The document as a thing that has a page.
    #[must_use]
    pub fn object(&self) -> ObjectRef {
        ObjectRef {
            kind: self.kind,
            id: self.id,
        }
    }
}

/// The site of a document: where its page stands. Nothing where the node it
/// is beside is not there, or it or a node above it was deleted: the document
/// goes with its node and comes back with it.
#[must_use]
pub fn site_of(graph: &Graph, document: &Document) -> Option<Site> {
    if graph.document_parent_deleted(document.id)
        && !orch_db::queries::trace::retained_read(document.id)
    {
        return None;
    }
    let node = graph.site_of(document.node)?;
    // Its page follows its node's row before the places above, and the admins
    // of the place its node is in (the node itself, where it is one) administer it.
    let node_is_a_place = node.role_place == Some(document.node);
    let admin_places = if node_is_a_place {
        std::iter::once(document.node)
            .chain(node.admin_places.iter().copied())
            .collect()
    } else {
        node.admin_places.clone()
    };
    Some(Site {
        kind: document.kind,
        id: document.id,
        name: document.title.clone(),
        first_owner: Some(document.owner),
        placement: Placement::default(),
        chain: node.chain,
        beside: Some(document.node),
        role_place: node.role_place,
        admin_places,
        orgs: node.orgs,
        root: node.root,
        path: node.path,
        archived_node: node.archived_node,
        of_system: false,
    })
}

/// Reads the documents `ids` into `graph`, with the nodes they are beside:
/// two statements however many there are. An identifier that is no document
/// is not among them; one that was deleted is, flagged.
pub async fn read_into(
    conn: &mut Conn,
    graph: &mut Graph,
    ids: &[Id],
) -> Result<Vec<Document>, Error> {
    let found = rows::get_many(conn, ids).await?;
    let containers = rows::containers::get_many(conn, ids).await?;
    let nodes: Vec<Id> = found
        .iter()
        .map(|row| row.node_id)
        .chain(containers.iter().map(|row| row.node_id))
        .collect();
    graph.extend(conn, &nodes).await?;
    graph.extend_documents(conn, ids).await?;
    Ok(found
        .iter()
        .map(Document::of_row)
        .chain(containers.iter().map(Document::of_container))
        .collect())
}

/// The document `id`, deleted or not, with the nodes it is beside and its
/// site. Not found where there is no such document or its place is deleted.
pub async fn site(conn: &mut Conn, id: Id) -> Result<(Graph, Document, Site), Error> {
    let mut graph = Graph::default();
    let found = read_into(conn, &mut graph, &[id]).await?;
    let document = found.into_iter().next().ok_or(Error::NotFound(id))?;
    let site = site_of(&graph, &document).ok_or(Error::NotFound(id))?;
    Ok((graph, document, site))
}

/// The site of a document for the machinery that reads and changes pages:
/// the same, but for a document that is deleted it is not found.
pub async fn site_for_page(conn: &mut Conn, id: Id) -> Result<(Graph, Site), Error> {
    let (graph, document, site) = site(conn, id).await?;
    if document.deleted && !orch_db::queries::trace::retained_read(id) {
        return Err(Error::NotFound(id));
    }
    Ok((graph, site))
}

/// Which questions are about a document that is deleted: none of them, but
/// bringing it back, and seeing it among those that can be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// A document that is there.
    Live,
    /// A document as it was before it was deleted: what is asked of whoever
    /// restores it.
    AsItWas,
}

/// Whether the caller holds every one of `permissions` on the document now,
/// for `act`; and if not, everything they lack and who can give it.
///
/// A caller who is suspended where the document is gets [`Error::Suspended`].
/// One who would hold what the act needs but finds the node archived gets
/// [`Error::Archived`], after the test of what they hold. A token that does not
/// carry a permission, or does not reach the document, is refused with nobody
/// to ask. A deleted document is not found, unless it is asked for as it was.
pub async fn check_all(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    permissions: &[Permission],
    act: Act,
    state: State,
    policy: Policy,
) -> Result<Decision, Error> {
    let (graph, document, site) = site(conn, id).await?;
    if document.deleted && state == State::Live && !orch_db::queries::trace::retained_read(id) {
        return Err(Error::NotFound(id));
    }
    if let Some(closing) = principal.suspension().closing(&site.orgs) {
        return Err(Error::Suspended { org: closing.org() });
    }
    let Some(first) = permissions.first().copied() else {
        return Ok(Decision::Allowed {
            through: Vec::new(),
        });
    };
    let object = document.object();
    let caller = principal.caller();
    if !caller.reaches(site.path.as_deref()) {
        return Ok(Decision::Refused(Refusal::of(first, object, Vec::new())));
    }
    let frame = Frame::load(
        conn,
        principal,
        policy,
        graph,
        std::slice::from_ref(&site),
        permissions,
    )
    .await?;
    let mut through: Vec<Through> = Vec::new();
    let mut lacking: Vec<Permission> = Vec::new();
    for permission in permissions {
        let ways = frame.ways(&site, *permission);
        if ways.is_empty() {
            lacking.push(*permission);
        }
        through.extend(ways);
    }
    if let Some(required) = lacking.first().copied() {
        if lacking
            .iter()
            .any(|permission| !caller.permits(*permission))
        {
            return Ok(Decision::Refused(Refusal::of(required, object, Vec::new())));
        }
        let graph = frame.graph().clone();
        let everything = every_permission([document.kind]);
        let frame = Frame::load(
            conn,
            principal,
            policy,
            graph,
            std::slice::from_ref(&site),
            &everything,
        )
        .await?;
        let refusal = missing::refusal(conn, &frame, &site, required, &lacking).await?;
        return Ok(Decision::Refused(refusal));
    }
    // A retained read or restore asks for the exact page as it was. Deleting its place
    // archived it as well; that archive must not hide the separately authorized parent-first result.
    if let Some(blocked) = site.archived_node
        && !(state == State::AsItWas && orch_db::queries::trace::retained_read(id))
        && permissions
            .iter()
            .any(|permission| !archive_allows(*permission, act))
    {
        return Err(Error::Archived { node: blocked });
    }
    Ok(Decision::Allowed { through })
}

/// [`check_all`] for one permission, of a document that is there.
pub async fn check(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    permission: Permission,
    policy: Policy,
) -> Result<Decision, Error> {
    check_all(
        conn,
        principal,
        id,
        &[permission],
        Act::Use,
        State::Live,
        policy,
    )
    .await
}

async fn notice_chat_readable(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
) -> Result<bool, Error> {
    let Some(chat) = orch_db::queries::chats::chat(conn, id).await? else {
        return Ok(false);
    };
    let listed = crate::chats::listed_access(conn, principal, std::slice::from_ref(&chat)).await?;
    if !listed.contains_key(&id) {
        return Ok(false);
    }
    let members = if chat.node_id.is_none() {
        orch_db::queries::chats::members(conn, id).await?
    } else {
        Vec::new()
    };
    // A notice is not an audited inspection of a direct or group chat.
    let readers =
        crate::chats::readers_among(conn, &chat, &members, &[principal.user()], principal.now())
            .await?;
    Ok(readers.contains(&principal.user()))
}

/// Whether a notice still reaches a current reader of the work it names.
pub async fn notice_readable(
    conn: &mut Conn,
    principal: &Principal,
    kind: &str,
    data: &orch_types::Json,
    chat: Option<Id>,
    thread: Option<Id>,
) -> Result<bool, Error> {
    if matches!(kind, "mention" | "chat.message") {
        if let Some(chat) = chat {
            return notice_chat_readable(conn, principal, chat).await;
        }
        if kind == "mention"
            && let Some(thread) = thread
        {
            // A notice is an ordinary delivery, even if this session can perform an override.
            let ordinary = Principal::load(
                conn,
                crate::Caller::session(principal.user()),
                principal.now(),
            )
            .await?;
            for reader in [principal, &ordinary] {
                match crate::nodes::check(
                    conn,
                    reader,
                    thread,
                    Permission::ThreadView,
                    Act::Use,
                    Policy::default(),
                )
                .await
                {
                    Ok(Decision::Allowed { .. }) => {}
                    Ok(Decision::Refused(_))
                    | Err(Error::NotFound(_) | Error::NoSuchUser(_) | Error::Suspended { .. }) => {
                        return Ok(false);
                    }
                    Err(error) => return Err(error),
                }
            }
            return Ok(true);
        }
        return Ok(false);
    }
    if kind == "person.notify" {
        if let (Some(resource), Some(kind)) = (
            data.get("resource_id")
                .and_then(orch_types::Json::as_str)
                .and_then(|id| id.parse().ok()),
            data.get("resource_kind")
                .and_then(orch_types::Json::as_str)
                .and_then(|word| {
                    word.parse::<orch_access_contract::permissions::ResourceKind>()
                        .ok()
                }),
        ) {
            let object = ObjectRef {
                kind: kind.into(),
                id: resource,
            };
            return match crate::things::effective(
                conn,
                principal,
                crate::things::Thing::Other(object),
                Policy::default(),
            )
            .await
            {
                Ok(held) => Ok(!held.held.is_empty()),
                Err(Error::NotFound(_) | Error::NoSuchUser(_) | Error::Suspended { .. }) => {
                    Ok(false)
                }
                Err(error) => Err(error),
            };
        }
        if let Some(chat) = data
            .get("chat_id")
            .and_then(orch_types::Json::as_str)
            .and_then(|id| id.parse().ok())
        {
            return notice_chat_readable(conn, principal, chat).await;
        }
        if let Some(node) = data
            .get("target_node_id")
            .and_then(orch_types::Json::as_str)
            .and_then(|id| id.parse().ok())
        {
            let visible = crate::nodes::holding(
                conn,
                principal,
                &[node],
                orch_access_contract::permissions::PermissionName::OfNode(
                    orch_access_contract::permissions::NodePermission::View,
                ),
                Act::Use,
                Policy::default(),
            )
            .await?;
            return Ok(!visible.is_empty());
        }
    }
    if !matches!(kind, "doc.comment" | "doc.mention" | "person.notify") {
        return Ok(true);
    }
    let Some(document) = data
        .get("document_id")
        .and_then(orch_types::Json::as_str)
        .and_then(|id| id.parse().ok())
    else {
        return Ok(false);
    };
    match check(
        conn,
        principal,
        document,
        Permission::DocView,
        Policy::default(),
    )
    .await
    {
        Ok(Decision::Allowed { .. }) => Ok(true),
        Ok(Decision::Refused(_))
        | Err(Error::NotFound(_) | Error::NoSuchUser(_) | Error::Suspended { .. }) => Ok(false),
        Err(error) => Err(error),
    }
}

/// What a person holds on a document, and through what.
#[derive(Debug, Clone, PartialEq)]
pub struct Effective {
    /// The document.
    pub object: ObjectRef,
    /// The role they have where a role of the document is held, and where it comes from.
    pub role: Option<roles::Held>,
    /// Whether they administer the place its node is in.
    pub administers: bool,
    /// Whether they are one of its owners, and count as one.
    pub owner: bool,
    /// What they hold, each with how, in the order of the catalogue, cut down
    /// to what archive leaves and to what a token carries.
    pub held: Vec<Held>,
    /// How its page stands against its node's.
    pub state: PageState,
}

fn state_of(frame: &Frame<'_>, site: &Site) -> PageState {
    let mut changed = false;
    let mut restricted = false;
    for row in frame.store().rows_of(site.page()) {
        // The Owners row follows nothing and is never "changed".
        if row.permission == Permission::AccessOwn {
            continue;
        }
        if !row.inherit || row.names(frame.now()).next().is_some() {
            changed = true;
        }
        if row.permission == Permission::DocView && !row.inherit {
            restricted = true;
        }
    }
    PageState::of(changed, restricted)
}

/// What the person `principal` holds on the document `id`, as it is or as it was.
pub async fn effective(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    policy: Policy,
) -> Result<Effective, Error> {
    let (graph, document, site) = site(conn, id).await?;
    let everything = every_permission([document.kind]);
    let frame = Frame::load(
        conn,
        principal,
        policy,
        graph,
        std::slice::from_ref(&site),
        &everything,
    )
    .await?;
    Ok(Effective {
        object: document.object(),
        role: frame.role_for(&site),
        administers: frame.administers(&site),
        owner: frame.owns(&site),
        held: frame.effective(&site),
        state: state_of(&frame, &site),
    })
}

/// A document in a list, with what the caller holds on it.
#[derive(Debug, Clone, PartialEq)]
pub struct Listed {
    /// The actual administration source, including when View is ordinary.
    pub admin_place: Option<PlaceRef>,
    /// Held permissions which the ordinary frame does not give.
    pub admin_only: Vec<Permission>,
    /// The administrative scope used to open a private document, if any.
    pub admin_access: Option<orch_access_contract::roles::AdminReach>,
    /// A personal/group assignment here, independent of View and native reach.
    pub participating: bool,
    /// The document.
    pub document: Document,
    /// What the caller holds on it.
    pub held: Vec<Held>,
    /// Whether they own it.
    pub owner: bool,
    /// Whether they administer the place it is in.
    pub administers: bool,
    /// How its page stands.
    pub state: PageState,
    /// The nearest archived node on the way up from it, itself included.
    pub archived_node: Option<Id>,
}

impl Listed {
    /// Whether the caller holds `permission` on it.
    #[must_use]
    pub fn holds(&self, permission: Permission) -> bool {
        self.held.iter().any(|held| held.permission == permission)
    }
}

/// What the caller holds on each of the documents `ids`, in the statements one
/// question takes however many they are. A document that is not there, or whose
/// place is deleted, is left out; so is one the caller is suspended in.
pub async fn listed(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
    policy: Policy,
) -> Result<Vec<Listed>, Error> {
    let mut graph = Graph::default();
    let found = read_into(conn, &mut graph, ids).await?;
    let sites: Vec<(Document, Site)> = found
        .into_iter()
        .filter_map(|document| site_of(&graph, &document).map(|site| (document, site)))
        .filter(|(_, site)| principal.suspension().closing(&site.orgs).is_none())
        .collect();
    if sites.is_empty() {
        return Ok(Vec::new());
    }
    let only: Vec<Site> = sites.iter().map(|(_, site)| site.clone()).collect();
    let everything = every_permission([ObjectKind::Document, ObjectKind::DocumentContainer]);
    let frame = Frame::load(conn, principal, policy, graph, &only, &everything).await?;
    Ok(sites
        .into_iter()
        .map(|(document, site)| Listed {
            admin_place: frame.admin_place(&site),
            admin_only: frame.admin_only(&site),
            admin_access: frame.admin_access(&site),
            participating: frame.participates(&site),
            held: if document.deleted && orch_db::queries::trace::retained_read(document.id) {
                frame.effective_as_it_was(&site)
            } else {
                frame.effective(&site)
            },
            owner: frame.owns(&site),
            administers: frame.administers(&site),
            state: state_of(&frame, &site),
            archived_node: site.archived_node,
            document,
        })
        .collect())
}

/// Writes the Owners row of a new document: its maker, the first of them.
/// Nothing else is stored for its page: every other row follows its node's.
pub async fn start(conn: &mut Conn, document: Id, maker: Id, at: Time) -> Result<(), Error> {
    crate::pages::start(conn, StoredKind::Document, document, maker, at).await
}

/// Who would hold something on a document that they do not hold now, and who
/// would lose something, if it were put beside another node, by permission.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MovePlan {
    /// People who would gain, with what.
    pub gaining: BTreeMap<Id, Vec<Permission>>,
    /// People who would lose, with what.
    pub losing: BTreeMap<Id, Vec<Permission>>,
}

/// The permissions of a document whose holders a move can change.
const MOVED_BY: [Permission; 4] = [
    Permission::DocView,
    Permission::DocComment,
    Permission::DocEdit,
    Permission::DocManage,
];

/// Works out who gains and who loses what when the rows of the document
/// follow those of the node `to`: everything on its own page stays as it is,
/// and only what it follows changes. The document is read as it is, whoever
/// asks, since the answer is about its people and not about the asker.
pub async fn plan_move(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    to: Id,
    policy: Policy,
) -> Result<MovePlan, Error> {
    let (mut graph, document, here) = site(conn, id).await?;
    graph.extend(conn, &[to]).await?;
    let moved = Document {
        node: to,
        ..document
    };
    let mut destination_graph = graph.clone();
    destination_graph.document_at_root(id);
    let there = site_of(&destination_graph, &moved).ok_or(Error::NotFound(to))?;
    let mut before: BTreeMap<Permission, Vec<Id>> = BTreeMap::new();
    let mut after: BTreeMap<Permission, Vec<Id>> = BTreeMap::new();
    for (site, graph, into) in [
        (&here, graph, &mut before),
        (&there, destination_graph, &mut after),
    ] {
        let frame = Frame::load(
            conn,
            principal,
            policy,
            graph,
            std::slice::from_ref(site),
            &MOVED_BY,
        )
        .await?;
        for permission in MOVED_BY {
            into.insert(
                permission,
                holders::who_holds(conn, &frame, site, permission).await?,
            );
        }
    }
    let mut plan = MovePlan::default();
    for permission in MOVED_BY {
        let was = before.get(&permission).map_or(&[][..], Vec::as_slice);
        let will = after.get(&permission).map_or(&[][..], Vec::as_slice);
        for person in will.iter().filter(|person| !was.contains(person)) {
            plan.gaining.entry(*person).or_default().push(permission);
        }
        for person in was.iter().filter(|person| !will.contains(person)) {
            plan.losing.entry(*person).or_default().push(permission);
        }
    }
    Ok(plan)
}
