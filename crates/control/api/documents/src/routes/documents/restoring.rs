//! Retained source, actual recorded creation directory and one checked native restoration.

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::effects;

use crate::key_operations::native::resource_reply;

use crate::routes::access::operations::NameRoles;

use crate::routes::resources::projection::single;

use futures::future::BoxFuture;

use orch_access_contract::decisions::Decision;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::common::ReceiptProjection;

use orch_api_contract::document_sources as sources;

use orch_api_contract::document_sources::RestoreSource as Source;

use orch_api_contract::documents::DocumentRestoreBody;

use orch_api_contract::documents::DocumentRestoreReceipt;

use orch_api_contract::documents::DocumentRestoreResponse;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::resources::Resource;

use orch_authz::Caller;

use orch_authz::documents::State;

use std::sync::Arc;

use std::sync::Mutex;
const DOCUMENT: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Document);

pub(super) struct Restores;

pub(super) struct Proposal {
    source: DocumentRow,
    directory: document_tree_domain::Directory,
    replay: bool,
    until: Option<DateTime<Utc>>,
    native: Mutex<Option<Arc<document_tree_domain::DocumentRestoration>>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Restored {
    document: Id,
    directory: document_tree_domain::Directory,
}

pub(super) struct Subject {
    restored: Restored,
    row: DocumentRow,
    directory: document_tree_domain::Directory,
}

const ROLES: single::Roles = single::Roles {
    names: NameRoles {
        groups: None,
        nodes: [
            (ObjectKind::Org, sources::RESTORE_NODES[0]),
            (ObjectKind::Project, sources::RESTORE_NODES[1]),
            (ObjectKind::Topic, sources::RESTORE_NODES[2]),
            (ObjectKind::Thread, sources::RESTORE_NODES[3]),
        ],
        containers: Some(sources::RESTORE_CONTAINERS),
    },
    sources: sources::RESTORE_REFERENCES,
};

async fn destination(
    conn: &mut Conn,
    at: document_tree_domain::Directory,
) -> ApiResult<KeyResolvedTarget> {
    match at {
        document_tree_domain::Directory::NodeDocs(id) => {
            crate::key_operations::targets::node_identity(conn, id).await
        }
        document_tree_domain::Directory::Container(id) => read_operation::identities(conn, &[id])
            .await?
            .remove(&id)
            .filter(|target| {
                target.object.kind == KeyTargetKind::Object(ObjectKind::DocumentContainer)
            })
            .ok_or_else(ApiError::not_found),
    }
}

async fn facts(
    conn: &mut Conn,
    row: &DocumentRow,
    at: document_tree_domain::Directory,
) -> ApiResult<Vec<native::Facts<Source>>> {
    Ok(vec![
        native::Facts::one(
            Source::Document,
            DOCUMENT,
            read_operation::identity(conn, row.id).await?,
        ),
        native::Facts::one(Source::Destination, DOCUMENT, destination(conn, at).await?),
    ])
}

async fn require_retained(
    conn: &mut Conn,
    principal: &Principal,
    row: &DocumentRow,
) -> ApiResult<()> {
    match orch_db::queries::trace::read_scope(
        &[row.id, row.node_id],
        documents::check_all(
            conn,
            principal,
            row.id,
            &[Permission::DocManage],
            Act::Delete,
            State::AsItWas,
            Policy::default(),
        ),
    )
    .await
    {
        Ok(Decision::Allowed { .. }) => Ok(()),
        Ok(Decision::Refused(refusal)) => Err(refusals::denied(conn, refusal).await?),
        Err(error) => Err(refusals::problem(conn, error).await),
    }
}

async fn parents(conn: &mut Conn, principal: &Principal, row: &DocumentRow) -> ApiResult<()> {
    if orch_db::queries::hierarchy::deleted_node(conn, row.node_id)
        .await?
        .is_some()
    {
        let refs =
            crate::routes::nodes::view::refs(conn, principal, &[row.node_id], Policy::default())
                .await?;
        return Err(ApiError::new(
            ErrorCode::NodeRestoreParentFirst,
            "bring back the document's place under its own restoration authority first",
        )
        .with("parent", refs.get(&row.node_id)));
    }
    if let Some(parent) = rows::containers::ancestors_many(conn, &[row.id])
        .await?
        .into_iter()
        .find(|parent| parent.deleted_at.is_some())
    {
        let refs =
            crate::routes::document_tree::container_refs(conn, principal, &[parent.id]).await?;
        return Err(ApiError::new(
            ErrorCode::NodeRestoreParentFirst,
            "bring back the document's containing folder first",
        )
        .with("parent", refs.get(&parent.id)));
    }
    Ok(())
}

impl effects::Effect for Restores {
    type Source = Source;
    type Query = ();
    type Body = DocumentRestoreBody;
    type Value = Proposal;
    type Output = Restored;
    fn declaration(_value: &Proposal) -> &'static Declaration<Source> {
        &sources::RESTORE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: DocumentRestoreBody,
    ) -> BoxFuture<'a, ApiResult<(Proposal, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let result: ApiResult<_> = async {
                let stored = rows::get(conn, ctx.id("id")?)
                    .await?
                    .ok_or_else(ApiError::not_found)?;
                let initial_directory = stored_directory(conn, &stored).await?;
                let initial = facts(conn, &stored, initial_directory).await?;
                let known = native::bind_required(ctx, &sources::RESTORE, &initial)?;
                let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
                require_retained(conn, &principal, &stored).await?;
                rows::containers::lock_document_scope(conn, stored.id).await?;
                let source = rows::lock(conn, stored.id)
                    .await?
                    .filter(|row| row.deleted_at.is_some())
                    .ok_or_else(ApiError::not_found)?;
                let life = orch_db::queries::trace::life(conn, "document", source.id).await?;
                let current = life.as_ref().and_then(|life| life.deletion_id);
                if body.expected_deletion_id.is_some() && body.expected_deletion_id != current {
                    return Err(ApiError::new(
                        ErrorCode::ConflictStaleRevision,
                        "the selected deletion is no longer current",
                    )
                    .with("current_deletion_id", current));
                }
                let at = stored_directory(conn, &source).await?;
                let bound = facts(conn, &source, at).await?;
                let known = native::bind_required(ctx, &sources::RESTORE, &bound)?;
                let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
                require_retained(conn, &principal, &source).await?;
                parents(conn, &principal, &source).await?;
                let request = document_tree_domain::Request {
                    command: document_tree_domain::Command::Restore,
                    items: vec![document_tree_domain::Item::Document(source.id)],
                    to: None,
                    name: None,
                    request_ref: orch_types::new_id().to_string(),
                    version: None,
                };
                let prepared = document_tree_domain::prepare_document_restoration_in(
                    conn,
                    document_tree_domain::Actor::Person(&principal),
                    &request,
                    principal.now(),
                    crate::routes::document_tree::options(ctx),
                )
                .await;
                let prepared = crate::routes::document_tree::result(conn, prepared).await?;
                let replay = prepared.is_replay();
                let until = prepared.until();
                let bound = facts(conn, &source, at).await?;
                Ok((
                    Proposal {
                        source,
                        directory: at,
                        replay,
                        until,
                        native: Mutex::new(Some(Arc::new(prepared))),
                    },
                    bound,
                ))
            }
            .await;
            result.map_err(|error| {
                if native::modern(caller) {
                    error.without_content()
                } else {
                    error
                }
            })
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Proposal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            require_retained(conn, principal, &value.source).await?;
            parents(conn, principal, &value.source).await?;
            let creation = document_tree_domain::can_create_in(
                conn,
                document_tree_domain::Actor::Person(principal),
                value.directory,
                principal.now(),
            )
            .await;
            if !crate::routes::document_tree::result(conn, creation).await? {
                return Err(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "the restoration directory is no longer writable",
                ));
            }
            Ok(())
        })
    }
}

impl effects::ApplyOwned for Restores {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        mut tx: orch_db::Tx,
        principal: &'a Principal,
        value: &'a Proposal,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Restored>>> {
        Box::pin(async move {
            let packet = value
                .native
                .lock()
                .map_err(|_| ApiError::internal("the native restoration packet is poisoned"))?
                .take()
                .ok_or_else(|| {
                    ApiError::internal("the native restoration packet was already consumed")
                })?;
            let packet = Arc::try_unwrap(packet)
                .map_err(|_| ApiError::internal("the native restoration packet is still in use"))?;
            let result = document_tree_domain::apply_document_restoration_in(
                &mut tx,
                document_tree_domain::Actor::Person(principal),
                packet,
                principal.now(),
                crate::routes::document_tree::options(ctx),
                move || {
                    before.check()?;
                    if value
                        .until
                        .is_some_and(|until| until <= ctx.app.clock.now())
                    {
                        return Err(ApiError::new(
                            ErrorCode::ConflictExists,
                            "the retained restoration promise has ended",
                        ));
                    }
                    Ok(())
                },
            )
            .await?;
            crate::routes::document_tree::result(&mut tx, result).await?;
            let row = live(&mut tx, value.source.id).await?;
            let output = Restored {
                document: row.id,
                directory: value.directory,
            };
            let identity = transaction_guard::current_http_identity(
                ctx,
                &mut tx,
                transaction_guard::HttpIdentityMode::Read,
            )
            .await?;
            identity.check_deadline(&ctx.app)?;
            tx.commit().await.map_err(DbError::from)?;
            Ok(if value.replay {
                effects::Outcome::Unchanged(output)
            } else {
                effects::Outcome::Applied(output)
            })
        })
    }
}

impl resource_reply::ResourceProjection for Restores {
    type Source = Source;
    type Value = Subject;
    type Input = Restored;
    const READ_SOURCE: Source = Source::Result;
    const ROLES: &'static single::Roles = &ROLES;
    fn declaration(_ctx: &Ctx, _input: &Restored) -> ApiResult<&'static Declaration<Source>> {
        Ok(&sources::RESTORE)
    }
    fn load<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Restored,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = live(conn, input.document).await?;
            let directory = stored_directory(conn, &row).await?;
            let mut facts = facts(conn, &row, input.directory).await?;
            let mut result = facts
                .iter()
                .find(|fact| fact.source == Source::Document)
                .cloned()
                .ok_or_else(|| ApiError::internal("the native restored subject is missing"))?;
            result.source = Source::Result;
            facts.push(result);
            Ok((
                Subject {
                    restored: *input,
                    row,
                    directory,
                },
                facts,
            ))
        })
    }
    fn object(value: &Subject) -> ObjectRef {
        ObjectRef {
            kind: ObjectKind::Document,
            id: value.restored.document,
        }
    }
    fn require_read<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(authorization::require_document(
            conn,
            principal,
            value.row.id,
            Permission::DocView,
            Policy::default(),
        ))
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        opened: &'a Subject,
        fresh: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.restored == fresh.restored
                && opened.row.node_id == fresh.row.node_id
                && opened.directory == fresh.directory)
        })
    }
}

fn receipt(value: &Restored) -> DocumentRestoreResponse {
    DocumentRestoreResponse::Receipt(DocumentRestoreReceipt {
        projection: ReceiptProjection::Receipt,
        document_id: value.document,
    })
}

pub(super) async fn restore(
    ctx: Ctx,
    (): (),
    _body: DocumentRestoreBody,
) -> ApiResult<DocumentRestoreResponse> {
    let (tx, _initial) = transaction_guard::begin(&ctx).await?;
    let completed = effects::apply_owned::<Restores>(&ctx, tx).await?;
    let safe = receipt(&completed.output);
    crate::key_operations::publication::receipt(&completed.ctx, &safe)?;
    Ok(
        match resource_reply::with_input::<Restores>(
            &completed.ctx,
            &completed.accepted,
            completed.output,
        )
        .await?
        {
            Some(Resource::Document(document)) => DocumentRestoreResponse::Full(Box::new(document)),
            None => safe,
            Some(_) => {
                return Err(ApiError::internal(
                    "the native restored-document projection has another kind",
                ));
            }
        },
    )
}
