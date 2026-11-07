//! Typed document-folder metadata, child lists and atomic replay-safe organization.

use crate::{
    middleware::{
        authorization::refusals,
        requests::{ApiError, ApiResult},
        transaction_guard,
    },
    registry::{Ctx, Registry},
};
use orch_api_contract::{common::ErrorCode, document_tree::*};
use orch_authz::Principal;
use orch_db::{Conn, DbError};
use orch_engine::document_tree as domain;
use orch_types::Id;
use orch_types::Time;
use serde::{Serialize, de::DeserializeOwned};
use std::collections::HashMap;

mod creation;
mod list;
pub(crate) mod metadata;
mod read;
/// Exact unit conversion for the deployment's retention policy.
const SECONDS_PER_DAY: u64 = 86_400;

/// Registers only typed document-domain endpoints; none accepts hierarchy folders as a container.
pub fn mount(registry: &mut Registry) {
    registry
        .on_snapshot_read_with_optional_operation(DocumentTreeList, list::prepare, list::get)
        .on_snapshot_read_with_optional_operation(
            DocumentContainerGet,
            read::container,
            read::container_get,
        )
        .on_snapshot_read_with_optional_operation(
            DocumentTreeEntryGet,
            read::entry,
            read::entry_get,
        )
        .on_with_operation(
            DocumentContainerCreate,
            crate::key_operations::native::effects::prepare::<creation::Folders>,
            creation::create,
        )
        .on(DocumentTreeDeletePreview, preview)
        .on(DocumentTreeOperate, operate);
}

pub(crate) fn convert<T: Serialize, U: DeserializeOwned>(value: T) -> ApiResult<U> {
    serde_json::from_value(serde_json::to_value(value).map_err(ApiError::internal)?)
        .map_err(ApiError::internal)
}

/// Batched folder references for person-facing permission provenance, independently redacted.
pub(crate) async fn container_refs(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
) -> ApiResult<HashMap<Id, DocumentContainerRef>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let found = orch_authz::documents::listed(conn, principal, ids, orch_authz::Policy::default())
        .await
        .map_err(ApiError::internal)?;
    let mut out: HashMap<_, _> = ids
        .iter()
        .map(|id| {
            (
                *id,
                DocumentContainerRef {
                    id: *id,
                    kind: DocumentContainerMark::DocumentContainer,
                    name: None,
                    redacted: true,
                },
            )
        })
        .collect();
    for row in found {
        if row.document.kind == orch_access_contract::permissions::ObjectKind::DocumentContainer
            && row.holds(orch_access_contract::permissions::Permission::DocView)
        {
            out.insert(
                row.document.id,
                DocumentContainerRef {
                    id: row.document.id,
                    kind: DocumentContainerMark::DocumentContainer,
                    name: Some(row.document.title),
                    redacted: false,
                },
            );
        }
    }
    Ok(out)
}

pub(crate) fn options(ctx: &Ctx) -> domain::Options {
    domain::Options {
        retention: std::time::Duration::from_secs(
            ctx.app
                .config
                .retention
                .soft_delete_days
                .saturating_mul(SECONDS_PER_DAY),
        ),
        version_window_secs: i64::try_from(ctx.app.config.documents.version_window_secs)
            .unwrap_or(i64::MAX),
    }
}

pub(crate) async fn result<T>(conn: &mut Conn, value: Result<T, domain::Error>) -> ApiResult<T> {
    match value {
        Ok(value) => Ok(value),
        Err(domain::Error::Invalid(message)) => {
            Err(ApiError::new(ErrorCode::RequestInvalid, message))
        }
        Err(domain::Error::NotFound) => Err(ApiError::not_found()),
        Err(domain::Error::Denied) => Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "the complete document operation is not permitted",
        )),
        Err(domain::Error::Conflict) => Err(ApiError::new(
            ErrorCode::ConflictExists,
            "a document entry conflicts with this operation",
        )),
        Err(domain::Error::Stale) => Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the document tree changed; obtain a new deletion preview",
        )),
        Err(domain::Error::Limit) => Err(ApiError::new(
            ErrorCode::DocsTooLarge,
            "the complete document operation exceeds its limit",
        )),
        Err(domain::Error::Authorization(error)) => Err(refusals::problem(conn, error).await),
        Err(error) => Err(ApiError::internal(error)),
    }
}

fn request(body: &DocumentTreeOperationBody) -> ApiResult<domain::Request> {
    Ok(domain::Request {
        command: convert(body.command)?,
        items: convert(&body.items)?,
        to: convert(body.to)?,
        name: body.name.clone(),
        request_ref: body.client_id.clone(),
        version: body.version,
    })
}

enum ReadSource<'a> {
    Entry(&'a read::Ready),
    TreePage(&'a list::Ready),
}

async fn finish_read_scoped<T>(
    ctx: &Ctx,
    read: &metadata::Projection<T>,
    source: Option<&read::Ready>,
) -> ApiResult<()> {
    finish_read(ctx, read, source.map(ReadSource::Entry)).await
}

async fn finish_read<T>(
    ctx: &Ctx,
    read: &metadata::Projection<T>,
    source: Option<ReadSource<'_>>,
) -> ApiResult<()> {
    let (mut tx, guard) = transaction_guard::begin(ctx).await?;
    metadata::unchanged(&mut tx, guard.principal(), &read.cut, guard.now()).await?;
    match source {
        Some(ReadSource::Entry(source)) => {
            read::check_sources(ctx, &mut tx, guard.principal().caller(), source).await?;
        }
        Some(ReadSource::TreePage(source)) => {
            list::check_sources(ctx, &mut tx, guard.principal(), source).await?;
        }
        None => {}
    }
    guard.check_deadline(ctx)?;
    for info in &read.inspections {
        // Explicit entry inspection is audited by the ordinary registry finish path.
        if ctx
            .inspection
            .as_ref()
            .is_none_or(|selected| selected.id != info.id)
        {
            crate::middleware::inspection::record(
                ctx,
                &mut tx,
                guard.principal(),
                info,
                guard.now(),
            )
            .await?;
        }
    }
    tx.commit().await.map_err(DbError::from)?;
    guard.check_deadline(ctx)
}

async fn preview(
    mut ctx: Ctx,
    (): (),
    body: DocumentTreeOperationBody,
) -> ApiResult<DocumentTreePlan> {
    if body.command != DocumentTreeCommand::Delete || body.deletion_plan.is_some() {
        return Err(ApiError::invalid(
            "command",
            "only deletion has a fresh preview",
        ));
    }
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.principal().clone();
    let plan = domain::plan_delete_in(
        &mut tx,
        domain::Actor::Person(&principal),
        &request(&body)?,
        Time::new(ctx.now),
        options(&ctx),
    )
    .await;
    let plan = result(&mut tx, plan).await?;
    // No capability or mutation receipt is stored by a preview.
    tx.commit().await.map_err(DbError::from)?;
    convert(plan)
}

async fn operate(
    ctx: Ctx,
    (): (),
    body: DocumentTreeOperationBody,
) -> ApiResult<DocumentTreeReceipt> {
    use orch_db::queries::transaction_guard::{Scope, request_scope};
    let scope = match body.command {
        DocumentTreeCommand::CreateFolder | DocumentTreeCommand::Copy => Scope::Creation,
        DocumentTreeCommand::Rename | DocumentTreeCommand::RestoreVersion => Scope::Action,
        DocumentTreeCommand::Move | DocumentTreeCommand::Delete | DocumentTreeCommand::Restore => {
            Scope::Authority
        }
    };
    request_scope(scope, operate_body(ctx, body)).await
}

async fn operate_body(
    mut ctx: Ctx,
    body: DocumentTreeOperationBody,
) -> ApiResult<DocumentTreeReceipt> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.principal().clone();
    let deletion_plan: Option<domain::MembershipStamp> = convert(&body.deletion_plan)?;
    let done = domain::execute_in(
        &mut tx,
        domain::Actor::Person(&principal),
        &request(&body)?,
        deletion_plan.as_ref(),
        Time::new(ctx.now),
        options(&ctx),
    )
    .await;
    let done = result(&mut tx, done).await?;
    let done = metadata::receipt(&ctx, &mut tx, &principal, done).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(done)
}
