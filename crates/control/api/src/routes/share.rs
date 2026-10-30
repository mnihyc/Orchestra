//! Sharing a chat, a document, a thread or a whole place with agents, by area, as a copy or a live share to read or to write; listing, taking back, and an agent's reach as a tree.
//!
//! What sharing means is the engine's (`orch_engine::sharing`): which entries a live share is, what a copy holds,
//! who may do either. The handlers here read a request, ask the engine, and turn the answer into the types of the
//! API with the names of everything it mentions, and a refusal into the problem a client acts on.
//!
//! **An agent's reach as a tree** is the tree its own tools list, read for a person: every entry the agent reaches
//! is there, and one the person may not see themself comes without its name, and without the names of the places
//! above it in its path (it is then written by its identifier), so that nothing is learnt from the tree that the
//! person could not learn from the place.

use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Answer, Ctx, Registry};
use crate::routes::documents::build as document_views;
use crate::routes::nodes::view;
use crate::sign_in::origin;
use chrono::{DateTime, Utc};
use http::StatusCode;
use orch_access_contract::permissions::{NodePermission, Permission, PermissionName};
use orch_api_contract::access as access_api;
use orch_api_contract::chats::{ChatKind, ChatRef};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::{NodeKind, NodeRef};
use orch_api_contract::share::{
    AgentShare, AgentShareList, AgentShareListQuery, AgentShareSide, AgentShareSnapshot,
    AgentShareSnapshotState, AgentShareWhat, SHARED_TREE_DEPTH_MAX, SHARED_TREE_DEPTH_MIN,
    SHARED_TREE_ROOT, ShareAgentCreate, ShareAgentCreateReply, ShareAgentDelete, ShareAgentUpdate,
    ShareAgentUpdateBody, ShareWithAgents, SharedEntry, ThreadSharedGet, ThreadSharedGetReply,
    ThreadSharedPolicy, ThreadSharedTreeGet, ThreadSharedTreeGetQuery, ThreadSharedTreeGetReply,
};
use orch_authz::Error as AuthError;
use orch_authz::reach::AgentReach;
use orch_authz::{Act, Policy, Principal, documents as document_access, nodes};
use orch_db::Conn;
use orch_db::queries::{chats as chat_rows, documents as document_rows, hierarchy, threads};
use orch_engine::document_tree::Directory;
use orch_engine::sharing::{Record, ShareError, Subject};
use orch_engine::tools::tree::{Entry as TreeEntry, Tree, Where};
use orch_engine::views::{People, input_view};
use orch_types::{Id, Time};
use serde_json::json;
use std::collections::{HashMap, HashSet};

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(ShareAgentCreate, create)
        .on(AgentShareList, list)
        .on(ThreadSharedGet, thread_shared)
        .on(ThreadSharedTreeGet, tree)
        .on(ShareAgentUpdate, update)
        .on(ShareAgentDelete, delete);
}

// ---------------------------------------------------------------- refusals

/// What went wrong in sharing, as the problem a client acts on.
pub(crate) async fn failed(conn: &mut Conn, error: ShareError) -> ApiError {
    match error {
        ShareError::NotFound => ApiError::not_found(),
        ShareError::Denied(refusal) => match refusals::denied(conn, *refusal).await {
            Ok(problem) | Err(problem) => problem,
        },
        ShareError::Invalid { field, message } => ApiError::invalid(&field, &message),
        ShareError::Unavailable { reason, thread } => {
            let named = match thread {
                Some(thread) => named_node(conn, thread).await.ok(),
                None => None,
            };
            ApiError::new(
                ErrorCode::ShareUnavailable,
                match reason {
                    "tools_off" => {
                        "the topic of that thread does not enable the tools of the shared tree"
                    }
                    "no_tools" => "the harness of that thread cannot be given Orch's tools",
                    _ => "nothing was named that an agent could read",
                },
            )
            .with("reason", reason)
            .with("thread", named)
        }
        ShareError::TooLarge {
            bytes,
            limit,
            scope,
        } => {
            let named = match scope {
                Some(scope) => named_node(conn, scope).await.ok(),
                None => None,
            };
            ApiError::new(
                ErrorCode::ShareTooLarge,
                "the copy is larger than sharing.snapshot_max_bytes allows where it lies",
            )
            .with("bytes", bytes)
            .with("limit", limit)
            .with("scope", named)
        }
        ShareError::Side { parent } => {
            let named = named_node(conn, parent).await.ok();
            ApiError::new(ErrorCode::ThreadSide, "a side thread takes no share")
                .with("parent", named)
        }
        ShareError::Archived { node } => {
            refusals::problem(conn, AuthError::Archived { node }).await
        }
        ShareError::Suspended { org } => {
            refusals::problem(conn, AuthError::Suspended { org }).await
        }
        ShareError::Page(error) => refusals::change_failed(conn, *error).await,
        ShareError::Unsupported(what) => ApiError::new(
            ErrorCode::CapabilityUnsupported,
            format!("a copy cannot be kept yet: {what}"),
        )
        .with("capability", "share.copy"),
        ShareError::Input(error) => ApiError::internal(error),
        ShareError::Internal(error) => ApiError::engine(error),
    }
}

/// A node as a reference, as the caller may know it.
async fn named_node(conn: &mut Conn, id: Id) -> ApiResult<NodeRef> {
    let rows = hierarchy::with_ancestors(conn, &[id]).await?;
    let row = rows
        .iter()
        .find(|row| row.id == id)
        .ok_or_else(ApiError::not_found)?;
    Ok(NodeRef {
        thread_display: None,
        id,
        kind: refusals::node_kind(&row.kind).unwrap_or(NodeKind::Org),
        name: Some(row.name.clone()),
        redacted: false,
    })
}

// ---------------------------------------------------------------- the types of the API

fn state_of(word: &str) -> AgentShareSnapshotState {
    match word {
        "delivered" => AgentShareSnapshotState::Delivered,
        "revoked" => AgentShareSnapshotState::Revoked,
        _ => AgentShareSnapshotState::Pending,
    }
}

fn chat_kind_of(word: &str) -> ChatKind {
    match word {
        "direct" => ChatKind::Direct,
        "group" => ChatKind::Group,
        _ => ChatKind::Node,
    }
}

fn redacted(id: Id) -> NodeRef {
    NodeRef {
        thread_display: None,
        id,
        kind: NodeKind::Org,
        name: None,
        redacted: true,
    }
}

/// Shares as a client reads them: what each is of, whom it is for, the entries that carry it or the copy that
/// was made, the comment that went with it, and who shared it.
#[allow(
    clippy::too_many_lines,
    reason = "one pass that looks up everything the shares mention, then builds them"
)]
async fn shares_view(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    records: &[Record],
) -> ApiResult<Vec<AgentShare>> {
    let mut node_ids: Vec<Id> = Vec::new();
    let mut user_ids: Vec<Id> = Vec::new();
    let mut chats: HashMap<Id, chat_rows::ChatRow> = HashMap::new();
    let mut documents: HashMap<Id, document_rows::DocumentRow> = HashMap::new();
    for record in records {
        node_ids.push(record.to);
        user_ids.extend(record.by);
        for grant in &record.grants {
            node_ids.push(grant.holder_id);
            user_ids.extend(grant.created_by);
        }
        match record.subject {
            Subject::Node(node) => node_ids.push(node),
            Subject::Chat(id) => {
                if let Some(row) = chat_rows::chat(conn, id).await? {
                    node_ids.extend(row.node_id);
                    chats.insert(id, row);
                }
            }
            Subject::Document(id) => {
                if let Some(row) = document_rows::get(conn, id).await? {
                    node_ids.push(row.node_id);
                    documents.insert(id, row);
                }
            }
        }
    }
    node_ids.sort_unstable();
    node_ids.dedup();
    user_ids.sort_unstable();
    user_ids.dedup();
    let refs = view::refs(conn, principal, &node_ids, Policy::default()).await?;
    let users = refusals::user_refs(conn, &user_ids).await?;
    let orgs: HashMap<Id, Id> = if node_ids.is_empty() {
        HashMap::new()
    } else {
        hierarchy::with_ancestors(conn, &node_ids)
            .await?
            .into_iter()
            .map(|row| (row.id, row.org_id))
            .collect()
    };
    let doc_ids: Vec<Id> = documents.keys().copied().collect();
    let listed = document_access::listed(conn, principal, &doc_ids, Policy::default())
        .await
        .map_err(ApiError::internal)?;
    let entries: Vec<_> = listed
        .into_iter()
        .filter_map(|each| {
            documents
                .get(&each.document.id)
                .map(|row| (row.clone(), each))
        })
        .collect();
    let now: DateTime<Utc> = ctx.now;
    let doc_refs: HashMap<Id, _> =
        document_views::refs(conn, principal, &entries, now, Policy::default())
            .await?
            .into_iter()
            .map(|each| (each.id, each))
            .collect();
    let node = |id: Id| refs.get(&id).cloned().unwrap_or_else(|| redacted(id));
    let mut out = Vec::new();
    for record in records {
        let (what, org) = match record.subject {
            Subject::Node(id) => (AgentShareWhat::Node(node(id)), orgs.get(&id).copied()),
            Subject::Chat(id) => {
                let Some(row) = chats.get(&id) else {
                    continue;
                };
                (
                    AgentShareWhat::Chat(ChatRef {
                        id,
                        kind: chat_kind_of(&row.kind),
                        org_id: row.org_id,
                        node: row.node_id.map(node),
                        name: row.name.clone(),
                    }),
                    Some(row.org_id),
                )
            }
            Subject::Document(id) => {
                let Some(reference) = doc_refs.get(&id) else {
                    continue;
                };
                (
                    AgentShareWhat::Document(reference.clone()),
                    documents.get(&id).map(|row| row.org_id),
                )
            }
        };
        let Some(by) = record.by.and_then(|by| users.get(&by).cloned()) else {
            continue;
        };
        let entries = record
            .grants
            .iter()
            .map(|grant| access_api::Entry {
                read_first: grant.read_first,
                id: grant.entry_id,
                holder: access_api::Holder::Agents {
                    node: node(grant.holder_id),
                },
                options: json!({}),
                outside: org
                    .is_some_and(|org| orgs.get(&grant.holder_id).is_some_and(|of| *of != org)),
                expires_at: grant.expires_at.map(Time::new),
                note: grant.note.clone(),
                source: serde_json::from_value(json!(grant.source))
                    .unwrap_or(access_api::EntrySource::Page),
                created_by: grant.created_by.and_then(|by| users.get(&by).cloned()),
                created_at: Time::new(grant.created_at),
                revision: grant.revision,
            })
            .collect();
        let snapshot = record.copy.as_ref().map(|copy| AgentShareSnapshot {
            path: copy.path.clone(),
            files: i64::from(copy.file_count),
            bytes: copy.total_bytes,
            as_of: Time::new(copy.as_of),
            state: state_of(&copy.state),
        });
        let input = match record.input {
            Some(id) => match threads::input(conn, id).await? {
                Some(row) => {
                    let people = People::load(conn, row.author_id.into_iter().chain(row.run_as))
                        .await
                        .map_err(ApiError::internal)?;
                    let people = people
                        .with_approvals(conn, row.approval_id)
                        .await
                        .map_err(ApiError::internal)?;
                    Some(input_view(&row, &people).map_err(ApiError::internal)?)
                }
                None => None,
            },
            None => None,
        };
        out.push(AgentShare {
            read_first: record.read_first,
            id: record.id,
            what,
            to: node(record.to),
            mode: record.mode,
            inside: record.inside,
            download: downloads(record),
            parts: record.parts,
            entries,
            snapshot,
            input,
            by,
            at: Time::new(record.at),
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------- whether the agents may take it away (the export of a place)

/// Whether the agents of a share may copy what is shared into their workspace. A copy is that already; a
/// live share names the agents in the Export row of each area it covers.
pub(crate) fn downloads(record: &Record) -> bool {
    if record.copy.is_some() {
        return true;
    }
    let named = |permission: Permission| {
        record
            .grants
            .iter()
            .any(|grant| grant.permission == permission.as_str())
    };
    let parts = &record.parts;
    (parts.timeline.is_some() || parts.chat.is_some() || parts.documents.is_some())
        && (parts.timeline.is_none() || named(Permission::ThreadExport))
        && (parts.chat.is_none() || named(Permission::ChatExport))
        && (parts.documents.is_none() || named(Permission::DocExport))
}

// ---------------------------------------------------------------- the requests

/// Shares a thing with agents. The share is made in the transaction that asked the caller's credential and
/// authority again once the body had come, with the caller and the time of that check.
async fn create(
    mut ctx: Ctx,
    (): (),
    body: ShareWithAgents,
) -> ApiResult<Answer<ShareAgentCreateReply>> {
    let (tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.into_principal();
    let made = ctx
        .app
        .engine
        .share_with_agents_guarded(tx, principal.clone(), &origin(&ctx), &body)
        .await;
    let mut conn = ctx.app.db.acquire().await?;
    let records = match made {
        Ok(records) => records,
        Err(error) => return Err(failed(&mut conn, error).await),
    };
    let shares = shares_view(&ctx, &mut conn, &principal, &records).await?;
    let answer = Answer::from(ShareAgentCreateReply { shares });
    Ok(if records.iter().any(|record| record.fresh) {
        answer
    } else {
        answer.status(StatusCode::OK)
    })
}

/// Lists the shares of a node, a document or a chat, or what was shared with the agents of a place.
async fn list(ctx: Ctx, query: AgentShareListQuery, (): ()) -> ApiResult<Vec<AgentShare>> {
    let id = ctx.id("id")?;
    let side = query.side.unwrap_or_default();
    let caller = authorization::caller_of(ctx.user()?)?;
    let engine = &ctx.app.engine;
    let made = match (ctx.param("object")?, side) {
        ("nodes", AgentShareSide::Of) => {
            engine
                .shares_of(&caller, &origin(&ctx), Subject::Node(id))
                .await
        }
        ("nodes", AgentShareSide::To) => engine.shares_to_place(&caller, &origin(&ctx), id).await,
        ("documents", AgentShareSide::Of) => {
            engine
                .shares_of(&caller, &origin(&ctx), Subject::Document(id))
                .await
        }
        ("chats", AgentShareSide::Of) => {
            engine
                .shares_of(&caller, &origin(&ctx), Subject::Chat(id))
                .await
        }
        ("documents" | "chats", AgentShareSide::To) => {
            return Err(ApiError::invalid("side", "is for a node"));
        }
        _ => return Err(ApiError::not_found()),
    };
    let mut conn = ctx.app.db.acquire().await?;
    let records = match made {
        Ok(records) => records,
        Err(error) => return Err(failed(&mut conn, error).await),
    };
    let principal = authorization::principal(&ctx, &mut conn).await?;
    shares_view(&ctx, &mut conn, &principal, &records).await
}

fn policy_word(policy: AgentReach) -> ThreadSharedPolicy {
    match policy {
        AgentReach::None => ThreadSharedPolicy::None,
        AgentReach::Topic => ThreadSharedPolicy::Topic,
        AgentReach::Project => ThreadSharedPolicy::Project,
    }
}

/// What a thread's agent may read beyond its own thread, and the copies it was given.
async fn thread_shared(ctx: Ctx, (): (), (): ()) -> ApiResult<ThreadSharedGetReply> {
    let thread = ctx.id("id")?;
    let caller = authorization::caller_of(ctx.user()?)?;
    let made = ctx
        .app
        .engine
        .shares_of_thread(&caller, &origin(&ctx), thread)
        .await;
    let mut conn = ctx.app.db.acquire().await?;
    let found = match made {
        Ok(found) => found,
        Err(error) => return Err(failed(&mut conn, error).await),
    };
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let shares = shares_view(&ctx, &mut conn, &principal, &found.shares).await?;
    let ids: Vec<Id> = found.material.paths.iter().map(|path| path.id).collect();
    let documents = document_rows::get_many(&mut conn, &ids).await?;
    let readable: HashSet<Id> = orch_authz::documents::listed(
        &mut conn,
        &principal,
        &documents.iter().map(|doc| doc.id).collect::<Vec<_>>(),
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?
    .into_iter()
    .filter(|doc| doc.holds(Permission::DocView) && !doc.document.deleted)
    .map(|doc| doc.document.id)
    .collect();
    let places = hierarchy::with_ancestors(
        &mut conn,
        &documents.iter().map(|doc| doc.node_id).collect::<Vec<_>>(),
    )
    .await?;
    let visible = view::refs(
        &mut conn,
        &principal,
        &places.iter().map(|node| node.id).collect::<Vec<_>>(),
        Policy::default(),
    )
    .await?;
    let documents: HashSet<Id> = documents
        .iter()
        .filter(|doc| {
            readable.contains(&doc.id)
                && places
                    .iter()
                    .find(|place| place.id == doc.node_id)
                    .is_some_and(|place| {
                        place
                            .path
                            .iter()
                            .all(|id| visible.get(id).is_some_and(|node| !node.redacted))
                    })
        })
        .map(|doc| doc.id)
        .collect();
    let documents = crate::routes::document_tree::metadata::named_document_paths(
        &mut conn,
        &principal,
        &documents.into_iter().collect::<Vec<_>>(),
    )
    .await?;
    let copies: HashSet<Id> = found
        .shares
        .iter()
        .filter(|share| {
            share
                .copy
                .as_ref()
                .is_some_and(|copy| copy.state == "delivered")
        })
        .map(|share| share.id)
        .collect();
    let read_first_paths = found
        .material
        .paths
        .into_iter()
        .filter(|path| documents.contains(&path.id) || copies.contains(&path.id))
        .collect();
    Ok(ThreadSharedGetReply {
        documents_policy: match found.material.policy {
            orch_authz::reach::DocumentPolicy::None => {
                orch_api_contract::share::ThreadDocumentPolicy::None
            }
            orch_authz::reach::DocumentPolicy::Read => {
                orch_api_contract::share::ThreadDocumentPolicy::Read
            }
            orch_authz::reach::DocumentPolicy::Write => {
                orch_api_contract::share::ThreadDocumentPolicy::Write
            }
        },
        read_first_paths,
        policy: policy_word(found.policy),
        shares,
    })
}

/// Changes the areas or the end of a live share, in the transaction that asked the caller again once the body
/// had come.
async fn update(mut ctx: Ctx, (): (), body: ShareAgentUpdateBody) -> ApiResult<AgentShare> {
    let share = ctx.id("share_id")?;
    let (tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.into_principal();
    let made = ctx
        .app
        .engine
        .update_share_guarded(tx, principal.clone(), &origin(&ctx), share, &body)
        .await;
    let mut conn = ctx.app.db.acquire().await?;
    let record = match made {
        Ok(record) => record,
        Err(error) => return Err(failed(&mut conn, error).await),
    };
    shares_view(&ctx, &mut conn, &principal, &[record])
        .await?
        .into_iter()
        .next()
        .ok_or_else(ApiError::not_found)
}

/// Takes a share back, in the transaction that asked the caller again when the request was served.
async fn delete(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let share = ctx.id("share_id")?;
    let (tx, guard) = transaction_guard::begin(&ctx).await?;
    let made = ctx
        .app
        .engine
        .revoke_share_guarded(tx, guard.into_principal(), &origin(&ctx), share)
        .await;
    match made {
        Ok(()) => Ok(()),
        Err(error) => {
            let mut conn = ctx.app.db.acquire().await?;
            Err(failed(&mut conn, error).await)
        }
    }
}

// ---------------------------------------------------------------- an agent's reach as a tree

/// The place a tree entry stands in or is: the node whose name its path ends in or passes through.
fn node_of(at: Where, tree: &Tree) -> Option<Id> {
    match at {
        Where::Place(node)
        | Where::Conversation(node)
        | Where::NodeChat(node)
        | Where::Docs(node) => Some(node),
        Where::Document(document)
        | Where::History(document)
        | Where::Version(document, _)
        | Where::DocumentFiles(document)
        | Where::DocumentFile { document, .. } => {
            tree.document_either(document).map(|each| each.row.node_id)
        }
        Where::DocumentFolder(id) => tree.directory_node(Directory::Container(id)),
        Where::ChatFiles(chat) | Where::ChatFile { chat, .. } => tree.chat_origin(chat).flatten(),
        Where::Root | Where::PeopleChat(_) | Where::Messages(_) | Where::PeopleChatDir(_) => None,
    }
}

fn document_of(at: Where) -> Option<Id> {
    match at {
        Where::DocumentFolder(id)
        | Where::Document(id)
        | Where::History(id)
        | Where::Version(id, _)
        | Where::DocumentFiles(id) => Some(id),
        Where::DocumentFile { document, .. } => Some(document),
        _ => None,
    }
}

fn chat_node(at: Where, tree: &Tree) -> Option<Id> {
    match at {
        Where::NodeChat(node) => Some(node),
        Where::ChatFiles(chat) | Where::ChatFile { chat, .. } => tree.chat_origin(chat).flatten(),
        _ => None,
    }
}

struct PersonTreeVisibility {
    places: HashSet<Id>,
    chats: HashSet<Id>,
    documents: HashSet<Id>,
}

impl PersonTreeVisibility {
    /// Agent standing does not authorize the person's view of any name in a path.
    fn named(&self, at: Where, tree: &Tree) -> bool {
        let visible = match at {
            Where::Root => true,
            Where::Place(node) | Where::Conversation(node) | Where::Docs(node) => {
                self.places.contains(&node)
            }
            Where::NodeChat(node) => self.chats.contains(&node),
            Where::DocumentFolder(id)
            | Where::Document(id)
            | Where::History(id)
            | Where::Version(id, _)
            | Where::DocumentFiles(id) => self.documents.contains(&id),
            Where::DocumentFile { document, .. } => self.documents.contains(&document),
            Where::ChatFiles(_) | Where::ChatFile { .. } => {
                chat_node(at, tree).is_some_and(|node| self.chats.contains(&node))
            }
            // Direct/group chat projection retains its existing separate redaction rule.
            Where::PeopleChat(_) | Where::Messages(_) | Where::PeopleChatDir(_) => false,
        };
        visible
            && document_of(at).is_none_or(|id| {
                tree.document_folder_ancestors(id)
                    .iter()
                    .all(|id| self.documents.contains(id))
            })
            && node_of(at, tree).is_none_or(|node| {
                tree.place(node).map_or_else(
                    || self.places.contains(&node),
                    |place| place.path.iter().all(|above| self.places.contains(above)),
                )
            })
    }
}

/// Batch the person's own standing for both the listed entries and the directory being listed.
async fn what_the_person_sees(
    conn: &mut Conn,
    principal: &Principal,
    entries: &[TreeEntry],
    tree: &Tree,
    at: Where,
) -> ApiResult<PersonTreeVisibility> {
    let targets: Vec<_> = entries
        .iter()
        .map(|entry| entry.at)
        .chain(std::iter::once(at))
        .collect();
    let mut places: Vec<Id> = Vec::new();
    for target in &targets {
        if let Some(node) = node_of(*target, tree) {
            if let Some(place) = tree.place(node) {
                places.extend(place.path.iter().copied());
            } else {
                places.push(node);
            }
        }
    }
    places.sort_unstable();
    places.dedup();
    let seen_places: HashSet<Id> = nodes::holding(
        conn,
        principal,
        &places,
        PermissionName::OfNode(NodePermission::View),
        Act::Use,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?
    .into_iter()
    .map(|listed| listed.id)
    .collect();
    let chats: Vec<Id> = targets
        .iter()
        .filter_map(|at| chat_node(*at, tree))
        .collect();
    let seen_chats: HashSet<Id> = nodes::holding(
        conn,
        principal,
        &chats,
        PermissionName::Exact(Permission::ChatView),
        Act::Use,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?
    .into_iter()
    .map(|listed| listed.id)
    .collect();
    let mut documents: Vec<Id> = targets.iter().filter_map(|at| document_of(*at)).collect();
    for id in documents.clone() {
        documents.extend(tree.document_folder_ancestors(id));
    }
    documents.sort_unstable();
    documents.dedup();
    // A document that was deleted is seen by whoever could bring it back, as it is by the agent, and by nobody who
    // only reads: to a reader it is gone.
    let seen_documents: HashSet<Id> = orch_db::queries::trace::read_scope(
        &documents,
        document_access::listed(conn, principal, &documents, Policy::default()),
    )
    .await
    .map_err(ApiError::internal)?
    .into_iter()
    .filter(|each| {
        if each.document.deleted {
            each.holds(Permission::DocManage)
        } else {
            each.holds(Permission::DocView)
        }
    })
    .map(|each| each.document.id)
    .collect();
    Ok(PersonTreeVisibility {
        places: seen_places,
        chats: seen_chats,
        documents: seen_documents,
    })
}

fn redacted_tree_path(at: Where) -> String {
    let id = match at {
        Where::Root => return SHARED_TREE_ROOT.to_owned(),
        Where::Place(id)
        | Where::Conversation(id)
        | Where::NodeChat(id)
        | Where::Docs(id)
        | Where::DocumentFolder(id)
        | Where::Document(id)
        | Where::History(id)
        | Where::Version(id, _)
        | Where::DocumentFiles(id)
        | Where::PeopleChat(id)
        | Where::Messages(id)
        | Where::PeopleChatDir(id)
        | Where::ChatFiles(id) => id,
        Where::DocumentFile { document, .. } => document,
        Where::ChatFile { chat, .. } => chat,
    };
    format!("id:{id}")
}

/// An agent's reach as the tree its own tools list.
async fn tree(
    ctx: Ctx,
    query: ThreadSharedTreeGetQuery,
    (): (),
) -> ApiResult<ThreadSharedTreeGetReply> {
    let thread = ctx.id("id")?;
    let depth = query.depth.unwrap_or(SHARED_TREE_DEPTH_MIN);
    if !(SHARED_TREE_DEPTH_MIN..=SHARED_TREE_DEPTH_MAX).contains(&depth) {
        return Err(ApiError::invalid(
            "depth",
            &format!("is from {SHARED_TREE_DEPTH_MIN} to {SHARED_TREE_DEPTH_MAX}"),
        ));
    }
    let asked = query.path.as_deref().unwrap_or(SHARED_TREE_ROOT);
    let Some((listing, tree, at)) = ctx
        .app
        .engine
        .agent_tree(thread, Some(asked), depth)
        .await
        .map_err(ApiError::internal)?
    else {
        return Err(ApiError::not_found());
    };
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let seen = what_the_person_sees(&mut conn, &principal, &listing.entries, &tree, at).await?;
    // A directory the person may not see is not one they can look into by guessing its name.
    if let Where::Place(place) = at
        && !seen.places.contains(&place)
    {
        let own = nodes::holding(
            &mut conn,
            &principal,
            &[place],
            PermissionName::OfNode(NodePermission::View),
            Act::Use,
            Policy::default(),
        )
        .await
        .map_err(ApiError::internal)?;
        if own.is_empty() {
            return Err(ApiError::not_found());
        }
    }
    let sharers: Vec<Id> = listing
        .entries
        .iter()
        .filter(|entry| seen.named(entry.at, &tree))
        .filter_map(|entry| entry.shared_by)
        .collect();
    let people = refusals::user_refs(&mut conn, &sharers).await?;
    let thread_ids: Vec<Id> = listing
        .entries
        .iter()
        .filter(|entry| seen.named(entry.at, &tree))
        .map(|entry| entry.id)
        .collect();
    let thread_refs =
        crate::routes::nodes::view::refs(&mut conn, &principal, &thread_ids, Policy::default())
            .await?;
    let entries: Vec<SharedEntry> = listing
        .entries
        .iter()
        .map(|entry| {
            let seen = seen.named(entry.at, &tree);
            SharedEntry {
                thread_display: thread_refs
                    .get(&entry.id)
                    .and_then(|node| node.thread_display.as_deref().cloned()),
                path: if seen {
                    entry.path.clone()
                } else {
                    format!("id:{}", entry.id)
                },
                name: seen.then(|| entry.name.clone()),
                id: entry.id,
                kind: entry.kind,
                description: if seen {
                    entry.description.clone()
                } else {
                    None
                },
                unit: entry.unit,
                parts: if seen { entry.parts } else { None },
                size: if seen { entry.size } else { None },
                updated_at: if seen { entry.updated_at } else { None },
                children: entry.children,
                readable: entry.readable,
                writable: entry.writable,
                manageable: entry.manageable,
                deleted: entry.deleted,
                via: entry.via,
                shared_by: if seen {
                    entry.shared_by.and_then(|by| people.get(&by).cloned())
                } else {
                    None
                },
                redacted: !seen,
            }
        })
        .collect();
    Ok(ThreadSharedTreeGetReply {
        path: if seen.named(at, &tree) {
            listing.path
        } else {
            redacted_tree_path(at)
        },
        entries,
        truncated: listing.truncated,
    })
}
