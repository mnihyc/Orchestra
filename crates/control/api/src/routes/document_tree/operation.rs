//! Compound tree intents consume complete native sources and publish only their own completed Read.

use super::{confirmation, creation, metadata, options, request, result};
use crate::key_operations::{
    native::{self, effects, reply},
    publication,
};
use crate::middleware::{
    authorization,
    requests::{ApiError, ApiResult},
    transaction_guard::{self, HttpIdentityMode},
};
use crate::registry::Ctx;
use crate::routes::{access::page::NameBounds, documents::read_operation};
use futures::future::BoxFuture;
use orch_access_contract::{
    key_scope::{KeyResolvedTarget, KeyTargetKind},
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::{
    common::{ErrorCode, ReceiptProjection},
    document_sources::{self as sources, OperationSource as Source},
    document_tree::*,
    operation_sources::Declaration,
    operations::FacetUsage,
};
use orch_authz::{Caller, Policy, Principal, engine::Graph};
use orch_db::{Conn, DbError, queries::documents::containers};
use orch_engine::{EngineError, document_tree as domain};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

const FOLDER: KeyTargetKind = KeyTargetKind::Object(ObjectKind::DocumentContainer);
pub(super) struct Operations;
pub(super) struct Proposal {
    command: domain::Command,
    native: Mutex<Option<Arc<domain::Operation>>>,
    facts: Vec<native::Facts<Source>>,
    replay: bool,
}
pub(super) struct Completed {
    command: domain::Command,
    receipt: domain::Receipt,
    facts: Vec<native::Facts<Source>>,
    contexts: Vec<domain::Item>,
}
pub(super) struct Subject {
    rows: Vec<containers::EntryRow>,
}
pub(super) struct View {
    names: NameBounds,
    cuts: Vec<metadata::Cut>,
}

fn declaration(command: domain::Command) -> &'static Declaration<Source> {
    match command {
        domain::Command::CreateFolder => &sources::OPERATION_CREATE_FOLDER,
        domain::Command::Copy => &sources::OPERATION_COPY,
        domain::Command::MoveOut => &sources::OPERATION_MOVE_OUT,
        domain::Command::Move => &sources::OPERATION_MOVE,
        domain::Command::Rename => &sources::OPERATION_RENAME,
        domain::Command::Delete => &sources::OPERATION_DELETE,
        domain::Command::Restore => &sources::OPERATION_RESTORE,
        domain::Command::RestoreVersion => &sources::OPERATION_RESTORE_VERSION,
    }
}
fn family(row: &containers::EntryRow) -> ApiResult<KeyTargetKind> {
    Ok(KeyTargetKind::Object(match row.kind.as_str() {
        "document" => ObjectKind::Document,
        "container" => ObjectKind::DocumentContainer,
        _ => {
            return Err(ApiError::internal(
                "the native tree selected another entry kind",
            ));
        }
    }))
}
fn item(row: &containers::EntryRow) -> ApiResult<domain::Item> {
    Ok(match family(row)? {
        KeyTargetKind::Object(ObjectKind::Document) => domain::Item::Document(row.id),
        _ => domain::Item::Container(row.id),
    })
}
async fn role(
    conn: &mut Conn,
    rows: &[containers::EntryRow],
    source: Source,
) -> ApiResult<native::Facts<Source>> {
    let ids = rows.iter().map(|row| row.id).collect::<Vec<_>>();
    let actual = read_operation::identities(conn, &ids).await?;
    let mut groups: BTreeMap<KeyTargetKind, Vec<KeyResolvedTarget>> = BTreeMap::new();
    for row in rows {
        let kind = family(row)?;
        let target = actual
            .get(&row.id)
            .filter(|target| target.object.kind == kind)
            .cloned()
            .ok_or_else(ApiError::not_found)?;
        groups.entry(kind).or_default().push(target);
    }
    Ok(native::Facts::collection(
        source,
        groups.into_iter().collect(),
    ))
}
async fn selected_rows(
    conn: &mut Conn,
    items: &[domain::Item],
) -> ApiResult<Vec<containers::EntryRow>> {
    let mut rows = Vec::new();
    for item in items {
        let kind = match item {
            domain::Item::Document(_) => "document",
            domain::Item::Container(_) => "container",
        };
        rows.push(
            containers::entry(conn, kind, item.id())
                .await?
                .ok_or_else(ApiError::not_found)?,
        );
    }
    Ok(rows)
}
async fn input_facts(
    conn: &mut Conn,
    request: &domain::Request,
    selected: &domain::OperationSources,
) -> ApiResult<Vec<native::Facts<Source>>> {
    let mut facts = Vec::new();
    let (roots, affected) = match (selected.roots(), selected.entries()) {
        (Some(roots), Some(entries)) => (roots.to_vec(), entries.to_vec()),
        (None, None) if selected.is_replay() => {
            if matches!(
                request.command,
                domain::Command::Copy | domain::Command::MoveOut
            ) || request.command == domain::Command::CreateFolder
            {
                (Vec::new(), Vec::new())
            } else {
                (
                    selected_rows(conn, &request.items).await?,
                    selected_rows(
                        conn,
                        selected.replay_items().ok_or_else(|| {
                            ApiError::internal("native replay lost its identities")
                        })?,
                    )
                    .await?,
                )
            }
        }
        _ => {
            return Err(ApiError::internal(
                "the native source stage has an incomplete shape",
            ));
        }
    };
    if request.command != domain::Command::CreateFolder {
        facts.push(role(conn, &roots, Source::Selected).await?);
        facts.push(role(conn, &affected, Source::Affected).await?);
    }
    let needs_destination = matches!(
        request.command,
        domain::Command::CreateFolder
            | domain::Command::Copy
            | domain::Command::MoveOut
            | domain::Command::Move
            | domain::Command::Restore
    );
    if needs_destination {
        let mut pairs: BTreeSet<(KeyTargetKind, domain::Directory)> = BTreeSet::new();
        if request.command == domain::Command::CreateFolder {
            let directory = selected
                .destination()
                .map(|value| value.0)
                .or(request.to)
                .ok_or_else(ApiError::not_found)?;
            pairs.insert((FOLDER, directory));
        } else if request.command == domain::Command::Restore && request.to.is_none() {
            for row in &roots {
                pairs.insert((
                    family(row)?,
                    row.parent_container_id.map_or(
                        domain::Directory::NodeDocs(row.node_id),
                        domain::Directory::Container,
                    ),
                ));
            }
        } else {
            let directory = selected
                .destination()
                .map(|value| value.0)
                .or(request.to)
                .ok_or_else(ApiError::not_found)?;
            let kinds = if selected.is_replay() {
                selected
                    .replay_items()
                    .ok_or_else(|| ApiError::internal("native replay lost its result kinds"))?
                    .iter()
                    .map(|item| match item {
                        domain::Item::Document(_) => KeyTargetKind::Object(ObjectKind::Document),
                        domain::Item::Container(_) => FOLDER,
                    })
                    .collect::<BTreeSet<_>>()
            } else {
                affected
                    .iter()
                    .map(family)
                    .collect::<ApiResult<BTreeSet<_>>>()?
            };
            for kind in kinds {
                pairs.insert((kind, directory));
            }
        }
        let mut groups: BTreeMap<KeyTargetKind, Vec<KeyResolvedTarget>> = BTreeMap::new();
        for (kind, directory) in pairs {
            groups
                .entry(kind)
                .or_default()
                .push(creation::directory_target(conn, directory, selected.is_replay()).await?);
        }
        facts.push(native::Facts::collection(
            Source::Destinations,
            groups.into_iter().collect(),
        ));
    }
    Ok(facts)
}
fn prepared(value: &Proposal) -> ApiResult<Arc<domain::Operation>> {
    value
        .native
        .lock()
        .map_err(|_| ApiError::internal("the native operation is poisoned"))?
        .as_ref()
        .cloned()
        .ok_or_else(|| ApiError::internal("the native operation was consumed"))
}
impl effects::Effect for Operations {
    type Source = Source;
    type Query = ();
    type Body = DocumentTreeOperationBody;
    type Value = Proposal;
    type Output = Arc<Completed>;
    fn declaration(value: &Proposal) -> &'static Declaration<Source> {
        declaration(value.command)
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Proposal, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let request = request(&body)?;
            if matches!(
                request.command,
                domain::Command::Delete | domain::Command::MoveOut
            ) != body.deletion_plan.is_some()
            {
                return Err(ApiError::invalid(
                    "deletion_plan",
                    "deletion and Move to another org require their complete native deletion plan",
                ));
            }
            let decoded = body
                .deletion_plan
                .as_ref()
                .map(confirmation::decode)
                .transpose()?;
            let row = declaration(request.command);
            // The actual caller supplies identity for factual header discovery only.
            // No admission or private material is produced until every required role is known.
            let actor =
                authorization::load_principal(conn, caller.clone(), ctx.app.clock.time()).await?;
            let selected =
                domain::prepare_operation_sources_in(conn, domain::Actor::Person(&actor), &request)
                    .await;
            let selected = result(conn, selected).await?;
            let facts = input_facts(conn, &request, &selected).await?;
            let all = native::bind_required(ctx, row, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &all).await?;
            let checked = domain::require_operation_sources_in(
                conn,
                domain::Actor::Person(&principal),
                &selected,
                principal.now(),
            )
            .await;
            result(conn, checked).await?;
            // The complete typed closure is admitted before namespace/audience/history/file material.
            let native = domain::complete_operation_in(
                conn,
                domain::Actor::Person(&principal),
                selected,
                decoded.as_ref().map(confirmation::Decoded::as_confirmation),
                principal.now(),
                options(ctx),
            )
            .await;
            let native = result(conn, native).await?;
            let replay = native.is_replay();
            Ok((
                Proposal {
                    command: native.command(),
                    native: Mutex::new(Some(Arc::new(native))),
                    facts: facts.clone(),
                    replay,
                },
                facts,
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
            let native = prepared(value)?;
            let checked = domain::require_operation_in(
                conn,
                domain::Actor::Person(principal),
                &native,
                principal.now(),
            )
            .await;
            result(conn, checked).await
        })
    }
}
impl effects::ApplyOwned for Operations {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        mut tx: orch_db::Tx,
        principal: &'a Principal,
        value: &'a Proposal,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Arc<Completed>>>> {
        Box::pin(async move {
            let native = value
                .native
                .lock()
                .map_err(|_| ApiError::internal("the native operation is poisoned"))?
                .take()
                .ok_or_else(|| ApiError::internal("the native operation was consumed"))?;
            let native = Arc::try_unwrap(native)
                .map_err(|_| ApiError::internal("the native operation is still in use"))?;
            let contexts = native.context_items();
            let deadline = before.deadline();
            let written = domain::apply_operation_in(
                &mut tx,
                domain::Actor::Person(principal),
                native,
                principal.now(),
                options(ctx),
                move || {
                    before
                        .check()
                        .map_err(|error| EngineError::BeforeEffect(Box::new(error)))
                },
            )
            .await;
            let written = match written {
                Err(domain::Error::Engine(EngineError::BeforeEffect(error))) => {
                    return Err(error
                        .downcast::<ApiError>()
                        .map(|error| *error)
                        .unwrap_or_else(ApiError::internal));
                }
                other => result(&mut tx, other).await?,
            };
            deadline.check()?;
            let current =
                transaction_guard::current_http_identity(ctx, &mut tx, HttpIdentityMode::Read)
                    .await?;
            current.check_deadline(&ctx.app)?;
            deadline.check()?;
            tx.commit().await.map_err(DbError::from)?;
            let output = Arc::new(Completed {
                command: value.command,
                receipt: written,
                facts: value.facts.clone(),
                contexts,
            });
            Ok(if value.replay {
                effects::Outcome::Unchanged(output)
            } else {
                effects::Outcome::Applied(output)
            })
        })
    }
}
fn closed() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "the original native tree result is no longer readable",
    )
}
fn matches_entry(row: &containers::EntryRow, old: &domain::Entry) -> ApiResult<bool> {
    Ok(item(row)? == old.item
        && row.node_id == old.node
        && row.org_id == old.root
        && row.parent_container_id.map_or(
            domain::Directory::NodeDocs(row.node_id),
            domain::Directory::Container,
        ) == old.parent
        && row.basename == old.basename
        && row.title == old.title
        && row.revision == old.revision
        && row.owner_id == old.owner
        && row.deleted_at == old.deleted_at
        && row.deleted_in == old.deletion
        && row.files == old.files
        && row.versions == old.versions)
}
impl reply::Projection for Operations {
    type Source = Source;
    type Input = Arc<Completed>;
    type Subject = Subject;
    type Value = View;
    type Reply = DocumentTreeReceipt;
    const READ_SOURCE: Source = Source::Results;
    fn declaration(input: &Self::Input) -> &'static Declaration<Source> {
        declaration(input.command)
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Self::Input,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let items = input
                .receipt
                .entries
                .iter()
                .map(|entry| entry.item)
                .collect::<Vec<_>>();
            let rows = selected_rows(conn, &items).await?;
            for (row, old) in rows.iter().zip(&input.receipt.entries) {
                if !matches_entry(row, old)? {
                    return Err(closed());
                }
            }
            let mut facts = input.facts.clone();
            facts.push(role(conn, &rows, Source::Results).await?);
            Ok((Subject { rows }, facts))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Self::Input,
        subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            {
                let retained = subject
                    .rows
                    .iter()
                    .filter(|row| row.deleted_at.is_some())
                    .map(|row| row.id)
                    .collect::<Vec<_>>();
                orch_db::queries::trace::read_scope(&retained, async {
                    for row in &subject.rows {
                        authorization::require_document(
                            conn,
                            principal,
                            row.id,
                            Permission::DocView,
                            Policy::default(),
                        )
                        .await?;
                        if row.deleted_at.is_some() {
                            authorization::require_document(
                                conn,
                                principal,
                                row.id,
                                Permission::DocManage,
                                Policy::default(),
                            )
                            .await?;
                        }
                    }
                    Ok::<(), ApiError>(())
                })
                .await?;
            }
            Ok(())
        })
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Self::Input,
        subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<(View, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let contexts = selected_rows(conn, &input.contexts).await?;
            {
                let retained = contexts
                    .iter()
                    .filter(|row| row.deleted_at.is_some())
                    .map(|row| row.id)
                    .collect::<Vec<_>>();
                orch_db::queries::trace::read_scope(&retained, async {
                    for row in &contexts {
                        authorization::require_document(
                            conn,
                            principal,
                            row.id,
                            Permission::DocView,
                            Policy::default(),
                        )
                        .await?;
                        if row.deleted_at.is_some() {
                            authorization::require_document(
                                conn,
                                principal,
                                row.id,
                                Permission::DocManage,
                                Policy::default(),
                            )
                            .await?;
                        }
                    }
                    Ok::<(), ApiError>(())
                })
                .await?;
            }
            let context_fact = role(conn, &contexts, Source::Contexts).await?;
            let tree = domain::project_entries_in(
                conn,
                domain::Actor::Person(principal),
                subject.rows.clone(),
                principal.now(),
                true,
            )
            .await;
            let tree = result(conn, tree).await?;
            if metadata::available_entries(conn, &tree.entries, options(ctx))
                .await?
                .len()
                != subject.rows.len()
            {
                return Err(closed());
            }
            let nodes = subject
                .rows
                .iter()
                .map(|row| row.node_id)
                .collect::<BTreeSet<_>>();
            let graph = Graph::load(conn, &nodes.iter().copied().collect::<Vec<_>>())
                .await
                .map_err(ApiError::internal)?;
            let mut candidates = BTreeSet::new();
            for node in &nodes {
                candidates.extend(
                    graph
                        .get(*node)
                        .ok_or_else(ApiError::not_found)?
                        .path
                        .iter()
                        .copied(),
                );
            }
            let refs = crate::routes::nodes::view::refs(
                conn,
                principal,
                &candidates.iter().copied().collect::<Vec<_>>(),
                Policy::default(),
            )
            .await?;
            candidates.retain(|id| {
                refs.get(id)
                    .is_some_and(|row| row.name.is_some() || row.thread_display.is_some())
            });
            let declared = declaration(input.command);
            let plan = native::bind_known(ctx, declared, &[])?;
            let (mut facts, allowed) = native::node_name_facts(
                &plan,
                declared,
                principal.caller(),
                &graph,
                &candidates,
                &[
                    (ObjectKind::Org, Source::Orgs),
                    (ObjectKind::Project, Source::Projects),
                    (ObjectKind::Topic, Source::Topics),
                    (ObjectKind::Thread, Source::Threads),
                ],
            )?;
            facts.push(context_fact);
            let mut names = NameBounds {
                limited: native::modern(principal.caller()),
                nodes: allowed,
                ..Default::default()
            };
            let result_ids = subject
                .rows
                .iter()
                .map(|row| row.id)
                .collect::<BTreeSet<_>>();
            let ancestor_ids = tree
                .ancestors
                .iter()
                .filter(|row| row.title.is_some() || row.basename.is_some())
                .map(|row| row.id)
                .filter(|id| !result_ids.contains(id))
                .collect::<BTreeSet<_>>();
            let actual =
                read_operation::identities(conn, &ancestor_ids.iter().copied().collect::<Vec<_>>())
                    .await?;
            let mut selected = Vec::new();
            for id in ancestor_ids {
                let target = actual
                    .get(&id)
                    .filter(|target| target.object.kind == FOLDER)
                    .cloned()
                    .ok_or_else(ApiError::not_found)?;
                if names.limited
                    && !native::permits_usage(
                        &plan,
                        declared,
                        principal.caller(),
                        &Source::Containers,
                        FacetUsage::ResponseContent,
                        FOLDER,
                        &target,
                    )?
                {
                    continue;
                }
                names.containers.insert(id);
                selected.push(target);
            }
            // Ancestors also returned as results are already admitted by their own Result Read.
            names.containers.extend(
                tree.ancestors
                    .iter()
                    .map(|row| row.id)
                    .filter(|id| result_ids.contains(id)),
            );
            facts.push(native::Facts::collection(
                Source::Containers,
                if selected.is_empty() {
                    Vec::new()
                } else {
                    vec![(FOLDER, selected)]
                },
            ));
            let mut cuts = Vec::new();
            for node in nodes {
                cuts.push(metadata::cut(conn, principal, node, principal.now()).await?);
            }
            Ok((View { names, cuts }, facts))
        })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Self::Input,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<DocumentTreeReceipt>> {
        Box::pin(metadata::receipt_bounded(
            ctx,
            conn,
            principal,
            input.receipt.clone(),
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
        _input: &'a Self::Input,
        opened: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            if opened.subject.rows.len() != fresh.subject.rows.len()
                || !opened.value.names.nodes.is_subset(&fresh.value.names.nodes)
                || !opened
                    .value
                    .names
                    .containers
                    .is_subset(&fresh.value.names.containers)
            {
                return Ok(false);
            }
            for cut in &opened.value.cuts {
                match metadata::unchanged(conn, principal, cut, principal.now()).await {
                    Ok(()) => {}
                    Err(error) if error.problem.code == ErrorCode::ConflictStaleRevision => {
                        return Ok(false);
                    }
                    Err(error) => return Err(error),
                }
            }
            Ok(true)
        })
    }
}
fn receipt(input: &Completed) -> ApiResult<DocumentTreeOperationResponse> {
    Ok(DocumentTreeOperationResponse::Receipt(
        DocumentTreeOperationReceipt {
            projection: ReceiptProjection::Receipt,
            operation: input.receipt.operation,
            entries: super::convert(
                input
                    .receipt
                    .entries
                    .iter()
                    .map(|entry| entry.item)
                    .collect::<Vec<_>>(),
            )?,
        },
    ))
}
pub(super) async fn operate(
    ctx: Ctx,
    (): (),
    body: DocumentTreeOperationBody,
) -> ApiResult<DocumentTreeOperationResponse> {
    use orch_db::queries::transaction_guard::{Scope, request_scope};
    let scope = match body.command {
        DocumentTreeCommand::CreateFolder | DocumentTreeCommand::Copy => Scope::Creation,
        DocumentTreeCommand::Rename | DocumentTreeCommand::RestoreVersion => Scope::Action,
        DocumentTreeCommand::Move
        | DocumentTreeCommand::MoveOut
        | DocumentTreeCommand::Delete
        | DocumentTreeCommand::Restore => Scope::Authority,
    };
    request_scope(scope, async {
        let (tx, _guard) = transaction_guard::begin(&ctx).await?;
        let completed = effects::apply_owned::<Operations>(&ctx, tx).await?;
        let safe = receipt(&completed.output)?;
        publication::receipt(&completed.ctx, &safe)?;
        Ok(
            match reply::render::<Operations>(&completed.ctx, &completed.accepted, completed.output)
                .await?
            {
                Some(full) => DocumentTreeOperationResponse::full(full),
                None => safe,
            },
        )
    })
    .await
}
