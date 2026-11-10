//! Folder creation consumes the native locked plan and projects its real durable outcome.

use super::metadata;

use super::options;

use super::result;

use crate::key_operations::native;

use crate::key_operations::native::effects;

use crate::key_operations::native::reply;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::middleware::transaction_guard::HttpIdentityMode;

use crate::registry::Ctx;

use crate::routes::access::page::NameBounds;

use crate::routes::documents::read_operation;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::common::ReceiptProjection;

use orch_api_contract::document_sources as sources;

use orch_api_contract::document_sources::EntrySource as Source;

use orch_api_contract::document_tree::DocumentContainerCreateBody;

use orch_api_contract::document_tree::DocumentContainerCreateReceipt;

use orch_api_contract::document_tree::DocumentContainerCreateResponse;

use orch_api_contract::document_tree::DocumentTreeReceipt;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::operations::FacetUsage;

use orch_authz::Caller;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_authz::engine::Graph;

use orch_db::queries::documents::containers;

use orch_db::Conn;

use orch_db::DbError;

use orch_engine::document_tree as domain;

use orch_engine::operation_scope::targets;

use orch_engine::operation_scope::targets::TargetError;

use orch_types::Id;

use std::collections::BTreeSet;

use std::sync::Arc;

use std::sync::Mutex;
const CONTAINER: KeyTargetKind = KeyTargetKind::Object(ObjectKind::DocumentContainer);

pub(super) struct Folders;

pub(super) struct Proposal {
    destination: domain::Directory,
    native: Mutex<Option<Arc<domain::FolderCreation>>>,
    replay: bool,
    replayed: Option<Id>,
}

pub(super) struct Created {
    receipt: domain::Receipt,
    destination: domain::Directory,
    id: Id,
    replay: bool,
    header_replay: bool,
}

pub(super) struct Subject {
    row: containers::EntryRow,
}

pub(super) struct View {
    entry: domain::Entry,
    names: NameBounds,
    cut: metadata::Cut,
}

pub(super) fn replay_binding() -> crate::middleware::idempotency::Creation<
    (),
    DocumentContainerCreateBody,
    DocumentContainerCreateResponse,
> {
    crate::middleware::idempotency::Creation {
        kind: "document_folder_operation",
        replay: replay_creation,
        identity: |value| {
            let operation = match value {
                DocumentContainerCreateResponse::Full(value) => value.operation,
                DocumentContainerCreateResponse::Receipt(value) => value.operation,
            };
            Ok(orch_db::idempotency::creation::Created {
                kind: "document_folder_operation".into(),
                id: operation,
                data: serde_json::Value::Null,
            })
        },
    }
}

fn replay_creation(
    ctx: Ctx,
    (): (),
    _: DocumentContainerCreateBody,
    created: orch_db::idempotency::creation::Created,
) -> BoxFuture<'static, ApiResult<DocumentContainerCreateResponse>> {
    Box::pin(async move {
        let operation = created.id;
        let folder: Id =
            serde_json::from_value(created.data["folder"].clone()).map_err(ApiError::internal)?;
        let destination = serde_json::from_value(created.data["destination"].clone())
            .map_err(ApiError::internal)?;
        let native = domain::Receipt {
            operation,
            entries: Vec::new(),
            copied: serde_json::from_value(created.data["copied"].clone())
                .map_err(ApiError::internal)?,
            gaining: serde_json::from_value(created.data["gaining"].clone())
                .map_err(ApiError::internal)?,
            losing: serde_json::from_value(created.data["losing"].clone())
                .map_err(ApiError::internal)?,
            restore_until: serde_json::from_value(created.data["restore_until"].clone())
                .map_err(ApiError::internal)?,
        };
        let input = Created {
            receipt: native,
            destination,
            id: folder,
            replay: true,
            header_replay: true,
        };
        let safe = receipt(&input);
        crate::key_operations::publication::receipt(&ctx, &safe)?;
        match reply::render_completed_replay::<Folders>(&ctx, ctx.user()?.id(), input).await {
            Ok(Some(value)) => Ok(DocumentContainerCreateResponse::Full(Box::new(value))),
            Ok(None) => Ok(safe),
            Err(error) if matches!(error.code(), ErrorCode::NotFound | ErrorCode::AuthzDenied) => {
                Ok(safe)
            }
            Err(error) => Err(error),
        }
    })
}

fn refused() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "the native folder result is no longer readable",
    )
}

fn target_error(error: TargetError) -> ApiError {
    match error {
        TargetError::Missing => ApiError::not_found(),
        TargetError::Db(error) => error.into(),
        TargetError::Authz(error) => ApiError::internal(error),
    }
}

pub(super) async fn directory_target(
    conn: &mut Conn,
    directory: domain::Directory,
    retained: bool,
) -> ApiResult<KeyResolvedTarget> {
    match directory {
        domain::Directory::NodeDocs(id) => {
            if retained {
                targets::node_identity(conn, id).await.map_err(target_error)
            } else {
                targets::node(conn, id).await.map_err(target_error)
            }
        }
        domain::Directory::Container(id) => read_operation::identities(conn, &[id])
            .await?
            .remove(&id)
            .filter(|target| target.object.kind == CONTAINER)
            .ok_or_else(ApiError::not_found),
    }
}

async fn check_area(ctx: &Ctx, conn: &mut Conn, directory: domain::Directory) -> ApiResult<()> {
    if let domain::Directory::Container(id) = directory {
        let parent = containers::get(conn, id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        if parent.node_id != ctx.id("id")? {
            return Err(ApiError::new(
                ErrorCode::NodeWrongParent,
                "that parent belongs to another document area",
            ));
        }
    }
    Ok(())
}

fn plan(value: &Proposal) -> ApiResult<Arc<domain::FolderCreation>> {
    value
        .native
        .lock()
        .map_err(|_| ApiError::internal("the native folder plan is poisoned"))?
        .as_ref()
        .cloned()
        .ok_or_else(|| ApiError::internal("the native folder plan was already consumed"))
}

impl effects::Effect for Folders {
    type Source = Source;
    type Query = ();
    type Body = DocumentContainerCreateBody;
    type Value = Proposal;
    type Output = Created;
    fn declaration(_value: &Proposal) -> &'static Declaration<Source> {
        &sources::FOLDER_CREATE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Proposal, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let destination = body.parent_container_id.map_or(
                domain::Directory::NodeDocs(ctx.id("id")?),
                domain::Directory::Container,
            );
            check_area(ctx, conn, destination).await?;
            let target = directory_target(conn, destination, false).await?;
            let known = native::bind_known(
                ctx,
                &sources::FOLDER_CREATE,
                &[native::Facts::one(Source::Destination, CONTAINER, target)],
            )?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            let request = domain::Request {
                command: domain::Command::CreateFolder,
                items: Vec::new(),
                to: Some(destination),
                name: Some(body.name),
                request_ref: body.client_id,
                version: None,
            };
            let prepared = domain::prepare_folder_creation_in(
                conn,
                domain::Actor::Person(&principal),
                &request,
                principal.now(),
            )
            .await;
            let prepared = result(conn, prepared).await?;
            check_area(ctx, conn, destination).await?;
            let target = directory_target(conn, destination, false).await?;
            let replay = prepared.is_replay();
            let replayed = match prepared.replay_items() {
                [domain::Item::Container(id)] => Some(*id),
                [] => None,
                _ => {
                    return Err(ApiError::internal(
                        "the native folder replay has another result shape",
                    ));
                }
            };
            Ok((
                Proposal {
                    destination,
                    native: Mutex::new(Some(Arc::new(prepared))),
                    replay,
                    replayed,
                },
                vec![native::Facts::one(Source::Destination, CONTAINER, target)],
            ))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Proposal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let prepared = plan(value)?;
            let checked = domain::require_folder_creation_in(
                conn,
                domain::Actor::Person(principal),
                &prepared,
                principal.now(),
            )
            .await;
            result(conn, checked).await
        })
    }
}

impl effects::ApplyOwned for Folders {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        mut tx: orch_db::Tx,
        principal: &'a Principal,
        value: &'a Proposal,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Created>>> {
        Box::pin(async move {
            let prepared = value
                .native
                .lock()
                .map_err(|_| ApiError::internal("the native folder plan is poisoned"))?
                .take()
                .ok_or_else(|| ApiError::internal("the native folder plan was already consumed"))?;
            let prepared = Arc::try_unwrap(prepared)
                .map_err(|_| ApiError::internal("the native folder plan is still in use"))?;
            let applied = domain::apply_folder_creation_in(
                &mut tx,
                domain::Actor::Person(principal),
                prepared,
                principal.now(),
                options(ctx),
                move || before.check(),
            )
            .await?;
            let receipt = result(&mut tx, applied).await?;
            let id = match receipt.entries.as_slice() {
                [entry] => match entry.item {
                    domain::Item::Container(id) => id,
                    _ => {
                        return Err(ApiError::internal(
                            "the native folder writer returned another entry kind",
                        ));
                    }
                },
                [] => value.replayed.ok_or_else(|| {
                    ApiError::internal("the native folder outcome lost its identity")
                })?,
                _ => {
                    return Err(ApiError::internal(
                        "the native folder writer returned multiple roots",
                    ));
                }
            };
            orch_db::idempotency::creation::observe_with(&mut tx,Some(principal.user()),"document_folder_operation",receipt.operation,serde_json::json!({"folder":id,"destination":value.destination,"copied":receipt.copied,"gaining":receipt.gaining,"losing":receipt.losing,"restore_until":receipt.restore_until})).await?;
            let identity =
                transaction_guard::current_http_identity(ctx, &mut tx, HttpIdentityMode::Read)
                    .await?;
            identity.check_deadline(&ctx.app)?;
            tx.commit().await.map_err(DbError::from)?;
            let created = Created {
                receipt,
                destination: value.destination,
                id,
                replay: value.replay,
                header_replay: false,
            };
            Ok(if value.replay {
                effects::Outcome::Unchanged(created)
            } else {
                effects::Outcome::Applied(created)
            })
        })
    }
}

impl reply::Projection for Folders {
    type Source = Source;
    type Input = Created;
    type Subject = Subject;
    type Value = View;
    type Reply = DocumentTreeReceipt;
    const READ_SOURCE: Source = Source::Entry;
    fn declaration(_input: &Created) -> &'static Declaration<Source> {
        &sources::FOLDER_CREATE
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Created,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = containers::entry(conn, "container", input.id)
                .await?
                .filter(|row| row.deleted_at.is_none())
                .ok_or_else(ApiError::not_found)?;
            let target = read_operation::identities(conn, &[input.id])
                .await?
                .remove(&input.id)
                .ok_or_else(ApiError::not_found)?;
            let mut facts = vec![native::Facts::one(Source::Entry, CONTAINER, target)];
            if !input.header_replay {
                let destination = directory_target(conn, input.destination, true).await?;
                facts.push(native::Facts::one(
                    Source::Destination,
                    CONTAINER,
                    destination,
                ));
            }
            Ok((Subject { row }, facts))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Created,
        subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if native::modern(principal.caller()) || input.replay {
                let held = orch_authz::documents::listed(
                    conn,
                    principal,
                    &[subject.row.id],
                    Policy::default(),
                )
                .await
                .map_err(ApiError::internal)?;
                if !held.iter().any(|held| {
                    held.document.id == subject.row.id && held.holds(Permission::DocView)
                }) {
                    return Err(refused());
                }
            }
            Ok(())
        })
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Created,
        subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<(View, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let tree = domain::project_entries_in(
                conn,
                domain::Actor::Person(principal),
                vec![subject.row.clone()],
                principal.now(),
                true,
            )
            .await;
            let tree = result(conn, tree).await?;
            let entry = tree
                .entries
                .first()
                .map(|listed| listed.entry.clone())
                .ok_or_else(ApiError::not_found)?;
            let graph = Graph::load(conn, &[subject.row.node_id])
                .await
                .map_err(ApiError::internal)?;
            let candidates: BTreeSet<_> = if tree.entries.is_empty() {
                BTreeSet::new()
            } else {
                graph
                    .get(subject.row.node_id)
                    .ok_or_else(ApiError::not_found)?
                    .path
                    .iter()
                    .copied()
                    .collect()
            };
            let refs = crate::routes::nodes::view::refs(
                conn,
                principal,
                &candidates.iter().copied().collect::<Vec<_>>(),
                Policy::default(),
            )
            .await?;
            let nodes = candidates
                .into_iter()
                .filter(|id| {
                    refs.get(id)
                        .is_some_and(|node| node.name.is_some() || node.thread_display.is_some())
                })
                .collect();
            let selection = native::bind_known(ctx, &sources::FOLDER_CREATE, &[])?;
            let (mut facts, allowed) = native::node_name_facts(
                &selection,
                &sources::FOLDER_CREATE,
                principal.caller(),
                &graph,
                &nodes,
                &[
                    (ObjectKind::Org, Source::Orgs),
                    (ObjectKind::Project, Source::Projects),
                    (ObjectKind::Topic, Source::Topics),
                    (ObjectKind::Thread, Source::Threads),
                ],
            )?;
            let mut names = NameBounds {
                limited: native::modern(principal.caller()),
                nodes: allowed,
                ..Default::default()
            };
            let ids: Vec<_> = tree
                .ancestors
                .iter()
                .filter(|ancestor| ancestor.title.is_some() || ancestor.basename.is_some())
                .map(|ancestor| ancestor.id)
                .filter(|id| *id != subject.row.id)
                .collect();
            let mut targets = read_operation::identities(conn, &ids).await?;
            let mut selected = Vec::new();
            for id in ids {
                let target = targets.remove(&id).ok_or_else(ApiError::not_found)?;
                if names.limited
                    && !native::permits_usage(
                        &selection,
                        &sources::FOLDER_CREATE,
                        principal.caller(),
                        &Source::Containers,
                        FacetUsage::ResponseContent,
                        CONTAINER,
                        &target,
                    )?
                {
                    continue;
                }
                names.containers.insert(id);
                selected.push(target);
            }
            facts.push(native::Facts::collection(
                Source::Containers,
                if selected.is_empty() {
                    Vec::new()
                } else {
                    vec![(CONTAINER, selected)]
                },
            ));
            let cut = metadata::cut(conn, principal, subject.row.node_id, principal.now()).await?;
            Ok((View { entry, names, cut }, facts))
        })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Created,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<DocumentTreeReceipt>> {
        let mut receipt = input.receipt.clone();
        if input.header_replay {
            // Current protected metadata is materialized only after this operation's real Read.
            receipt.entries = vec![prepared.value.entry.clone()];
        }
        Box::pin(metadata::receipt_bounded(
            ctx,
            conn,
            principal,
            receipt,
            prepared
                .value
                .names
                .limited
                .then_some(&prepared.value.names),
        ))
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Created,
        opened: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            if opened.subject.row.id != fresh.subject.row.id
                || opened.subject.row.node_id != fresh.subject.row.node_id
                || opened.subject.row.parent_container_id != fresh.subject.row.parent_container_id
            {
                return Ok(false);
            }
            if !opened.value.names.nodes.is_subset(&fresh.value.names.nodes)
                || !opened
                    .value
                    .names
                    .containers
                    .is_subset(&fresh.value.names.containers)
            {
                return Ok(false);
            }
            match metadata::unchanged(conn, principal, &opened.value.cut, principal.now()).await {
                Ok(()) => Ok(true),
                Err(error) if error.problem.code == ErrorCode::ConflictStaleRevision => Ok(false),
                Err(error) => Err(error),
            }
        })
    }
}

fn receipt(input: &Created) -> DocumentContainerCreateResponse {
    DocumentContainerCreateResponse::Receipt(DocumentContainerCreateReceipt {
        projection: ReceiptProjection::Receipt,
        operation: input.receipt.operation,
        folder_id: input.id,
    })
}

pub(super) async fn create(
    ctx: Ctx,
    (): (),
    _body: DocumentContainerCreateBody,
) -> ApiResult<DocumentContainerCreateResponse> {
    orch_db::queries::transaction_guard::request_scope(
        orch_db::queries::transaction_guard::Scope::Creation,
        create_body(ctx),
    )
    .await
}

async fn create_body(ctx: Ctx) -> ApiResult<DocumentContainerCreateResponse> {
    let (tx, _guard) = transaction_guard::begin(&ctx).await?;
    let completed = effects::apply_owned::<Folders>(&ctx, tx).await?;
    let safe = receipt(&completed.output);
    crate::key_operations::publication::receipt(&completed.ctx, &safe)?;
    Ok(
        match reply::render::<Folders>(&completed.ctx, &completed.accepted, completed.output)
            .await?
        {
            Some(view) => DocumentContainerCreateResponse::Full(Box::new(view)),
            None => safe,
        },
    )
}
