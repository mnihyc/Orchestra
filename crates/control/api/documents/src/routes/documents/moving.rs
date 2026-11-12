//! One native document movement consumes its locked packet, then publishes only current Read.

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::effects;

use crate::key_operations::native::resource_reply;

use crate::routes::access::operations::NameRoles;

use crate::routes::resources::projection::single;

use futures::future::BoxFuture;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::common::ReceiptProjection;

use orch_api_contract::document_sources as sources;

use orch_api_contract::document_sources::MoveSource as Source;

use orch_api_contract::documents::DocumentMoveReceipt;

use orch_api_contract::documents::DocumentMoveResponse;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::resources::Resource;

use orch_authz::Caller;

use std::sync::Arc;

use std::sync::Mutex;
pub(super) struct Moves;

pub(super) struct Proposal {
    source: DocumentRow,
    to: Id,
    changes: bool,
    replay: bool,
    movement: Mutex<Option<Arc<document_tree_domain::DocumentMovement>>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Moved {
    document: Id,
    node: Id,
}

pub(super) struct Subject {
    moved: Moved,
    row: DocumentRow,
    directory: document_tree_domain::Directory,
}

const ROLES: single::Roles = single::Roles {
    names: NameRoles {
        groups: None,
        nodes: [
            (ObjectKind::Org, sources::MOVE_NODES[0]),
            (ObjectKind::Project, sources::MOVE_NODES[1]),
            (ObjectKind::Topic, sources::MOVE_NODES[2]),
            (ObjectKind::Thread, sources::MOVE_NODES[3]),
        ],
        containers: Some(sources::MOVE_CONTAINERS),
    },
    sources: sources::MOVE_REFERENCES,
};

impl effects::Effect for Moves {
    type Source = Source;
    type Query = ();
    type Body = DocumentMoveBody;
    type Value = Proposal;
    type Output = Moved;
    fn declaration(_value: &Proposal) -> &'static Declaration<Source> {
        &sources::MOVE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: DocumentMoveBody,
    ) -> BoxFuture<'a, ApiResult<(Proposal, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let ready =
                move_operands::prepare(ctx, conn, caller, body.node_id, &sources::MOVE).await?;
            let movement = if ready.changes {
                let request = document_tree_domain::Request {
                    command: document_tree_domain::Command::Move,
                    items: vec![document_tree_domain::Item::Document(ready.source.id)],
                    to: Some(document_tree_domain::Directory::NodeDocs(body.node_id)),
                    name: None,
                    request_ref: orch_types::new_id().to_string(),
                    version: None,
                };
                let prepared = document_tree_domain::prepare_document_movement_in(
                    conn,
                    document_tree_domain::Actor::Person(&ready.principal),
                    &request,
                    ready.principal.now(),
                )
                .await;
                Some(Arc::new(
                    crate::routes::document_tree::result(conn, prepared).await?,
                ))
            } else {
                None
            };
            let replay = movement.as_ref().is_some_and(|value| value.is_replay());
            let facts = move_operands::facts(conn, &ready.source, body.node_id).await?;
            Ok((
                Proposal {
                    source: ready.source,
                    to: body.node_id,
                    changes: ready.changes,
                    replay,
                    movement: Mutex::new(movement),
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
            may_move(conn, principal, Policy::default(), &value.source, value.to).await?;
            Ok(())
        })
    }
}

impl effects::ApplyOwned for Moves {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        mut tx: orch_db::Tx,
        principal: &'a Principal,
        value: &'a Proposal,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Moved>>> {
        Box::pin(async move {
            if value.changes {
                let packet = value
                    .movement
                    .lock()
                    .map_err(|_| ApiError::internal("the native movement packet is poisoned"))?
                    .take()
                    .ok_or_else(|| {
                        ApiError::internal("the native movement packet was already consumed")
                    })?;
                let packet = Arc::try_unwrap(packet).map_err(|_| {
                    ApiError::internal("the native movement packet is still in use")
                })?;
                let result = document_tree_domain::apply_document_movement_in(
                    &mut tx,
                    document_tree_domain::Actor::Person(principal),
                    packet,
                    principal.now(),
                    crate::routes::document_tree::options(ctx),
                    move || before.check(),
                )
                .await?;
                crate::routes::document_tree::result(&mut tx, result).await?;
            }
            let row = live(&mut tx, value.source.id).await?;
            if stored_directory(&mut tx, &row).await?
                != document_tree_domain::Directory::NodeDocs(value.to)
            {
                return Err(ApiError::internal(
                    "the native movement did not reach its declared directory",
                ));
            }
            let output = Moved {
                document: row.id,
                node: row.node_id,
            };
            let identity = transaction_guard::current_http_identity(
                ctx,
                &mut tx,
                transaction_guard::HttpIdentityMode::Read,
            )
            .await?;
            identity.check_deadline(&ctx.app)?;
            tx.commit().await.map_err(DbError::from)?;
            Ok(if value.changes && !value.replay {
                effects::Outcome::Applied(output)
            } else {
                effects::Outcome::Unchanged(output)
            })
        })
    }
}

impl resource_reply::ResourceProjection for Moves {
    type Source = Source;
    type Value = Subject;
    type Input = Moved;
    const READ_SOURCE: Source = Source::Result;
    const ROLES: &'static single::Roles = &ROLES;
    fn declaration(_ctx: &Ctx, _input: &Moved) -> ApiResult<&'static Declaration<Source>> {
        Ok(&sources::MOVE)
    }
    fn load<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Moved,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = live(conn, input.document).await?;
            let directory = stored_directory(conn, &row).await?;
            let mut facts = move_operands::facts(conn, &row, input.node).await?;
            let mut result = facts
                .iter()
                .find(|fact| fact.source == Source::Document)
                .cloned()
                .ok_or_else(|| ApiError::internal("the native moved subject is missing"))?;
            result.source = Source::Result;
            facts.push(result);
            Ok((
                Subject {
                    moved: *input,
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
            id: value.moved.document,
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
            Ok(opened.moved == fresh.moved
                && opened.row.node_id == fresh.row.node_id
                && opened.directory == fresh.directory)
        })
    }
}

fn receipt(value: &Moved) -> DocumentMoveResponse {
    DocumentMoveResponse::Receipt(DocumentMoveReceipt {
        projection: ReceiptProjection::Receipt,
        document_id: value.document,
        node_id: value.node,
    })
}

pub(super) async fn move_to(
    ctx: Ctx,
    (): (),
    _body: DocumentMoveBody,
) -> ApiResult<DocumentMoveResponse> {
    let (tx, _initial) = transaction_guard::begin(&ctx).await?;
    let completed = effects::apply_owned::<Moves>(&ctx, tx).await?;
    let safe = receipt(&completed.output);
    crate::key_operations::publication::receipt(&completed.ctx, &safe)?;
    Ok(
        match resource_reply::with_input::<Moves>(
            &completed.ctx,
            &completed.accepted,
            completed.output,
        )
        .await?
        {
            Some(Resource::Document(document)) => DocumentMoveResponse::Full(Box::new(document)),
            None => safe,
            Some(_) => {
                return Err(ApiError::internal(
                    "the native moved-document projection has another kind",
                ));
            }
        },
    )
}
