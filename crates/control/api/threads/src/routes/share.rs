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

use crate::key_operations::native;

use crate::middleware::authorization;

use crate::middleware::authorization::refusals;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::registry::Answer;

use crate::registry::Ctx;

use crate::registry::Registry;

use crate::routes::documents::build as document_views;

use crate::routes::nodes::view;

use crate::sign_in::origin;

use chrono::DateTime;

use chrono::Utc;

use http::StatusCode;

use orch_access_contract::permissions::NodePermission;

use orch_access_contract::permissions::Permission;

use orch_access_contract::permissions::PermissionName;

use orch_api_contract::access as access_api;

use orch_api_contract::chats::ChatKind;

use orch_api_contract::chats::ChatRef;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::nodes::NodeKind;

use orch_api_contract::nodes::NodeRef;

use orch_api_contract::share::AgentShare;

use orch_api_contract::share::AgentShareList;

use orch_api_contract::share::AgentShareListQuery;

use orch_api_contract::share::AgentShareReceipt;

use orch_api_contract::share::AgentShareSide;

use orch_api_contract::share::AgentShareSnapshot;

use orch_api_contract::share::AgentShareSnapshotState;

use orch_api_contract::share::AgentShareWhat;

use orch_api_contract::share::SHARED_TREE_DEPTH_MAX;

use orch_api_contract::share::SHARED_TREE_DEPTH_MIN;

use orch_api_contract::share::SHARED_TREE_ROOT;

use orch_api_contract::share::ShareAgentCreate;

use orch_api_contract::share::ShareAgentCreateReceipt;

use orch_api_contract::share::ShareAgentCreateReply;

use orch_api_contract::share::ShareAgentCreateResponse;

use orch_api_contract::share::ShareAgentDelete;

use orch_api_contract::share::ShareAgentUpdate;

use orch_api_contract::share::ShareAgentUpdateBody;

use orch_api_contract::share::ShareWithAgents;

use orch_api_contract::share::SharedEntry;

use orch_api_contract::share::ThreadSharedGet;

use orch_api_contract::share::ThreadSharedGetReply;

use orch_api_contract::share::ThreadSharedTreeGet;

use orch_api_contract::share::ThreadSharedTreeGetQuery;

use orch_api_contract::share::ThreadSharedTreeGetReply;

use orch_authz::Error as AuthError;

use orch_authz::Act;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_authz::documents as document_access;

use orch_authz::nodes;

use orch_db::Conn;

use orch_db::queries::chats as chat_rows;

use orch_db::queries::documents as document_rows;

use orch_db::queries::hierarchy;

use orch_db::queries::threads;

use orch_engine::document_tree::Directory;

use orch_engine::sharing::Record;

use orch_engine::sharing::ShareError;

use orch_engine::sharing::Subject;

use orch_engine::tools::tree::Entry as TreeEntry;

use orch_engine::tools::tree::Tree;

use orch_engine::tools::tree::Where;

use orch_engine::views::People;

use orch_engine::views::input_view;

use orch_types::Id;

use orch_types::Time;

use serde_json::json;

use std::collections::HashMap;

use std::collections::HashSet;
/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .creation(ShareAgentCreate, operation::creation::binding())
        .on_with_operation(ShareAgentCreate, operation::resolve, create)
        .on_snapshot_read_with_optional_operation(
            AgentShareList,
            native::selected::<operation::records::List>,
            list,
        )
        .on(ThreadSharedGet, thread_shared)
        .on(ThreadSharedTreeGet, tree)
        .on_with_operation(
            ShareAgentUpdate,
            native::effects::prepare::<operation::records::Update>,
            operation::records::update,
        )
        .on_with_optional_operation(
            ShareAgentDelete,
            native::stored::<operation::records::Delete, (), ()>,
            operation::records::delete,
        );
}
orch_api_base::__api_bind_b57b73724e!();

// ---------------------------------------------------------------- refusals

/// What went wrong in sharing, as the problem a client acts on.
pub(crate) async fn failed(conn: &mut Conn, error: ShareError) -> ApiError {
    match error {
        ShareError::NotFound => ApiError::not_found(),
        ShareError::StalePrepared => ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the share changed during preparation",
        ),
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
orch_api_base::__api_bind_eb48af3518!();

/// A scoped failure retains its ordinary code without resolving additional protected names.
pub(crate) async fn failed_projected(
    conn: &mut Conn,
    error: ShareError,
    limited: bool,
) -> ApiError {
    if !limited {
        return failed(conn, error).await;
    }
    match error {
        ShareError::Unavailable { reason, .. } => ApiError::new(
            ErrorCode::ShareUnavailable,
            match reason {
                "tools_off" => {
                    "the topic of that thread does not enable the tools of the shared tree"
                }
                "no_tools" => "the harness of that thread cannot be given Orch's tools",
                _ => "nothing was named that an agent could read",
            },
        )
        .with("reason", reason),
        ShareError::TooLarge { bytes, limit, .. } => ApiError::new(
            ErrorCode::ShareTooLarge,
            "the copy is larger than sharing.snapshot_max_bytes allows where it lies",
        )
        .with("bytes", bytes)
        .with("limit", limit),
        ShareError::Side { .. } => {
            ApiError::new(ErrorCode::ThreadSide, "a side thread takes no share")
        }
        error => authorization::problem_without_content(failed(conn, error).await),
    }
}
orch_api_base::__api_bind_2f2b473303!();

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
    recipient: Option<Id>,
) -> ApiResult<Vec<AgentShare>> {
    shares_view_bounded(ctx, conn, principal, records, recipient, None).await
}

async fn shares_view_bounded(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    records: &[Record],
    recipient: Option<Id>,
    bounds: Option<&crate::routes::access::page::NameBounds>,
) -> ApiResult<Vec<AgentShare>> {
    Ok(
        project_shares(ctx, conn, principal, records, recipient, bounds)
            .await?
            .shares,
    )
}

#[derive(Default)]
pub(crate) struct ShareProjection {
    shares: Vec<AgentShare>,
    named: std::collections::BTreeSet<Id>,
    inputs: Vec<(Id, Id, Id)>,
}

async fn project_shares(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    records: &[Record],
    recipient: Option<Id>,
    bounds: Option<&crate::routes::access::page::NameBounds>,
) -> ApiResult<ShareProjection> {
    let mut projection = ShareProjection::default();
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
                    node_ids.push(row.home());
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
    let refs = match bounds {
        Some(bounds) => bounds.refs(conn, principal, &node_ids).await?,
        None => view::refs(conn, principal, &node_ids, Policy::default()).await?,
    };
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
    let doc_refs: HashMap<Id, _> = match bounds.filter(|bounds| bounds.limited) {
        Some(bounds) => {
            let names: Vec<_> = bounds
                .nodes
                .iter()
                .copied()
                .map(document_views::WrittenLabelSource::Node)
                .collect();
            let mut refs = HashMap::new();
            for (row, listed) in &entries {
                let shown = document_views::ProjectedDocumentRef::capture_allowed(
                    conn,
                    principal,
                    row,
                    listed,
                    now,
                    Policy::default(),
                    &names,
                )
                .await?;
                projection
                    .named
                    .extend(shown.sources().iter().filter_map(|source| match source {
                        document_views::WrittenLabelSource::Node(id) => Some(*id),
                        _ => None,
                    }));
                refs.insert(shown.document.id, shown.document);
            }
            refs
        }
        None => document_views::refs(conn, principal, &entries, now, Policy::default())
            .await?
            .into_iter()
            .map(|each| (each.id, each))
            .collect(),
    };
    let mut direct = HashSet::new();
    for record in records {
        direct.insert(record.to);
        direct.extend(record.grants.iter().map(|g| g.holder_id));
        match record.subject {
            Subject::Node(id) => {
                direct.insert(id);
            }
            Subject::Chat(id) => {
                if let Some(node) = chats.get(&id).and_then(|row| row.node_id) {
                    direct.insert(node);
                }
            }
            Subject::Document(_) => {}
        }
    }
    projection.named.extend(direct.into_iter().filter(|id| {
        refs.get(id)
            .is_some_and(|r| r.name.is_some() || r.thread_display.is_some())
    }));
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
                        visibility: orch_chat::views::visibility_of(row),
                        home: (row.kind == chat_rows::KIND_GROUP).then(|| node(row.home())),
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
        let by = record.by.and_then(|by| users.get(&by).cloned());
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
                    projection
                        .inputs
                        .push((row.id, row.thread_id, row.continuation_id));
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
        let viewer_entries = match bounds {
            Some(bounds) => viewer_entries_bounded(conn, principal, record, bounds).await?,
            None => viewer_entries(conn, principal, record).await?,
        };
        if !viewer_entries.is_empty()
            && let Some(bounds) = bounds.filter(|b| b.limited)
        {
            let graph = orch_authz::engine::Graph::load(conn, &[record.to])
                .await
                .map_err(ApiError::internal)?;
            let ids = graph
                .get(record.to)
                .map(|n| n.path.clone())
                .unwrap_or_default();
            let named = bounds.refs(conn, principal, &ids).await?;
            projection.named.extend(
                named
                    .into_iter()
                    .filter(|(_, r)| r.name.is_some() || r.thread_display.is_some())
                    .map(|(id, _)| id),
            );
        }
        out.push(AgentShare {
            viewer_entries,
            read_first: record.read_first,
            id: record.id,
            what,
            to: node(record.to),
            mode: record.mode,
            inside: record.inside,
            download: downloads(record),
            parts: record.parts,
            current: None,
            entries,
            snapshot,
            input,
            by,
            at: Time::new(record.at),
        });
    }
    // Keep source visibility and redaction ahead of assessment. Missing givers do not hide
    // otherwise visible history, but a hidden source never acquires a new projection here.
    let visible_ids: HashSet<_> = out.iter().map(|share| share.id).collect();
    let visible: Vec<_> = records
        .iter()
        .filter(|record| visible_ids.contains(&record.id))
        .cloned()
        .collect();
    let mut current =
        match orch_engine::sharing::current::project(conn, &visible, recipient, Time::new(now))
            .await
        {
            Ok(current) => current,
            Err(error) => return Err(failed(conn, error).await),
        };
    for share in &mut out {
        share.current = current.remove(&share.id);
    }
    projection.shares = out;
    Ok(projection)
}

// ---------------------------------------------------------------- whether the agents may take it away (the export of a place)

/// The normal human page entries, not an additional agent permission or a live-giver loan.
pub(crate) async fn viewer_entries(
    conn: &mut Conn,
    principal: &Principal,
    record: &Record,
) -> ApiResult<Vec<access_api::Entry>> {
    viewer_entries_in(conn, principal, record, None).await
}

pub(crate) async fn viewer_entries_bounded(
    conn: &mut Conn,
    principal: &Principal,
    record: &Record,
    bounds: &crate::routes::access::page::NameBounds,
) -> ApiResult<Vec<access_api::Entry>> {
    viewer_entries_in(conn, principal, record, Some(bounds)).await
}
orch_api_base::__api_bind_ffc7d657fe!();

async fn viewer_entries_in(
    conn: &mut Conn,
    principal: &Principal,
    record: &Record,
    bounds: Option<&crate::routes::access::page::NameBounds>,
) -> ApiResult<Vec<access_api::Entry>> {
    if record.mode != orch_api_contract::threads::ShareMode::Live
        || record.parts.documents.is_none()
    {
        return Ok(Vec::new());
    }
    if !hierarchy::with_ancestors(conn, &[record.to])
        .await?
        .iter()
        .any(|node| node.id == record.to && node.kind == "thread")
    {
        return Ok(Vec::new());
    }
    let mut pages = Vec::new();
    for grant in record.grants.iter().filter(|grant| {
        matches!(
            grant.permission.as_str(),
            "doc.view" | "doc.edit" | "doc.manage"
        )
    }) {
        let page = orch_db::queries::access::PageKey::new(
            &grant.object_kind,
            grant.object_id,
            &grant.page,
        );
        if !pages.contains(&page) {
            pages.push(page);
        }
    }
    let mut entries = Vec::new();
    for page in pages {
        if let Some(row) = orch_db::queries::access::find_entry(
            conn,
            &page,
            "any",
            "doc.view",
            "role",
            Some(record.to),
            Some("viewer"),
        )
        .await?
        {
            let entry = orch_authz::engine::entry_of(&row).map_err(ApiError::internal)?;
            if entry.counts(principal.now()) {
                entries.push(entry);
            }
        }
    }
    if entries.is_empty() {
        return Ok(Vec::new());
    }
    let (root, place) = match record.subject {
        Subject::Node(id) => {
            let nodes = hierarchy::with_ancestors(conn, &[id]).await?;
            let node = nodes
                .iter()
                .find(|node| node.id == id)
                .ok_or_else(ApiError::not_found)?;
            (Some(node.org_id), Some(id))
        }
        Subject::Document(id) => {
            let row = document_rows::get(conn, id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            (Some(row.org_id), Some(row.node_id))
        }
        Subject::Chat(_) => return Ok(Vec::new()),
    };
    match bounds {
        Some(bounds) => {
            crate::routes::access::page::Names::entries_bounded(
                conn, principal, root, place, &entries, bounds,
            )
            .await
        }
        None => {
            crate::routes::access::page::Names::entries(conn, principal, root, place, &entries)
                .await
        }
    }
}

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
orch_api_base::__api_bind_da018a6af3!();

// ---------------------------------------------------------------- the requests

/// Shares a thing with agents. The share is made in the transaction that asked the caller's credential and
/// authority again once the body had come, with the caller and the time of that check.
async fn create(
    mut ctx: Ctx,
    (): (),
    body: ShareWithAgents,
) -> ApiResult<Answer<ShareAgentCreateResponse>> {
    let (tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let prepared = guard.operation_data::<operation::PreparedShare>()?;
    let principal = guard.into_principal();
    let disclosure = &prepared.disclosure;
    let made = ctx
        .app
        .engine
        .pending_prepared_agent_share(tx, principal.clone(), &origin(&ctx), &body, &prepared.share)
        .await;
    let mut conn = ctx.app.db.acquire().await?;
    let mut pending = match made {
        Ok(pending) => pending,
        Err(error) => {
            return Err(failed_projected(
                &mut conn,
                error,
                ctx.key_receipt_required()
                    || principal
                        .caller()
                        .grant()
                        .is_some_and(|grant| grant.operation_scope().is_some()),
            )
            .await);
        }
    };
    if pending.is_fresh() {
        disclosure.require(&ctx, pending.connection()).await?;
    }
    let records = match pending.publish(&ctx.app.engine).await {
        Ok(records) => records,
        Err(error) => {
            return Err(failed_projected(&mut conn, error, ctx.key_receipt_required()).await);
        }
    };
    let receipt = orch_engine::sharing::creation::receipt(&records, &prepared.share.recipients)
        .map_err(ApiError::engine)?;
    drop(conn);
    let response =
        operation::creation::render(&ctx, &body.what, prepared.selection.clone(), receipt).await?;
    let answer = Answer::from(response);
    Ok(if records.iter().any(|record| record.fresh) {
        answer
    } else {
        answer.status(StatusCode::OK)
    })
}

/// Lists the shares of a node, a document or a chat, or what was shared with the agents of a place.
async fn list(ctx: Ctx, query: AgentShareListQuery, (): ()) -> ApiResult<Vec<AgentShare>> {
    native::selected_read::<operation::records::List>(ctx, query, ()).await
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
    let shares = shares_view(&ctx, &mut conn, &principal, &found.shares, Some(thread)).await?;
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
        read_first_paths,
        shares,
    })
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

#[path = "share/operation.rs"]
pub mod operation;
