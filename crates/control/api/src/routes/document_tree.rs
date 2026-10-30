//! Typed document-folder metadata, child lists and atomic replay-safe organization.

use crate::{
    middleware::{
        authorization::{self, refusals},
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

pub(crate) mod metadata;
/// Exact unit conversion for the deployment's retention policy.
const SECONDS_PER_DAY: u64 = 86_400;

/// Registers only typed document-domain endpoints; none accepts hierarchy folders as a container.
pub fn mount(registry: &mut Registry) {
    registry
        .on(DocumentTreeList, list)
        .on(DocumentContainerGet, get_container)
        .on(DocumentTreeEntryGet, get_entry)
        .on(DocumentContainerCreate, create)
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

async fn list(ctx: Ctx, query: DocumentTreeListQuery, (): ()) -> ApiResult<DocumentTreePage> {
    let node = ctx.id("id")?;
    let at = query.parent_container_id.map_or(
        DocumentDirectory::NodeDocs(node),
        DocumentDirectory::Container,
    );
    // Loading deployment key material may touch its file; do that before the read transaction.
    crate::routes::resources::sealing::master_key()?;
    let mut tx = orch_db::queries::feeds::begin_reading(&ctx.app.db).await?;
    let principal = authorization::principal(&ctx, &mut tx).await?;
    let read = metadata::page(&ctx, &mut tx, &principal, Some(node), at, &query).await?;
    tx.commit().await.map_err(DbError::from)?;
    finish_read(&ctx, &read).await?;
    Ok(read.value)
}

async fn read_one(ctx: Ctx, item: domain::Item) -> ApiResult<DocumentTreeRead> {
    let mut tx = orch_db::queries::feeds::begin_reading(&ctx.app.db).await?;
    let principal = authorization::principal(&ctx, &mut tx).await?;
    let read = metadata::entry(
        &ctx,
        &mut tx,
        &principal,
        convert(item)?,
        ctx.inspect_deleted,
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    finish_read(&ctx, &read).await?;
    Ok(read.value)
}

async fn finish_read<T>(ctx: &Ctx, read: &metadata::Projection<T>) -> ApiResult<()> {
    let (mut tx, guard) = transaction_guard::begin(ctx).await?;
    metadata::unchanged(&mut tx, guard.principal(), &read.cut, guard.now()).await?;
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
    Ok(())
}

async fn get_container(
    ctx: Ctx,
    _query: orch_api_contract::nodes::DeletedInspectionQuery,
    (): (),
) -> ApiResult<DocumentTreeRead> {
    let id = ctx.id("id")?;
    read_one(ctx, domain::Item::Container(id)).await
}

async fn get_entry(
    ctx: Ctx,
    query: DocumentTreeEntryGetQuery,
    (): (),
) -> ApiResult<DocumentTreeRead> {
    let id = ctx.id("id")?;
    let item = match query.kind {
        DocumentTreeKind::Document => domain::Item::Document(id),
        DocumentTreeKind::Container => domain::Item::Container(id),
    };
    read_one(ctx, item).await
}

async fn create(
    ctx: Ctx,
    (): (),
    body: DocumentContainerCreateBody,
) -> ApiResult<DocumentTreeReceipt> {
    orch_db::queries::transaction_guard::request_scope(
        orch_db::queries::transaction_guard::Scope::Creation,
        create_body(ctx, body),
    )
    .await
}

async fn create_body(
    mut ctx: Ctx,
    body: DocumentContainerCreateBody,
) -> ApiResult<DocumentTreeReceipt> {
    let node = ctx.id("id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.principal().clone();
    if let Some(id) = body.parent_container_id {
        orch_db::queries::documents::containers::lock_scopes(&mut tx, &[node]).await?;
        authorization::require_document(
            &mut tx,
            &principal,
            id,
            orch_access_contract::permissions::Permission::DocCreate,
            orch_authz::Policy::default(),
        )
        .await?;
        let folder = orch_db::queries::documents::containers::get(&mut tx, id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        if folder.node_id != node {
            return Err(ApiError::new(
                ErrorCode::NodeWrongParent,
                "that parent belongs to another document area",
            ));
        }
    }
    let request = domain::Request {
        command: domain::Command::CreateFolder,
        items: Vec::new(),
        to: Some(body.parent_container_id.map_or(
            domain::Directory::NodeDocs(node),
            domain::Directory::Container,
        )),
        name: Some(body.name),
        request_ref: body.client_id,
        version: None,
    };
    let done = domain::execute_in(
        &mut tx,
        domain::Actor::Person(&principal),
        &request,
        None,
        Time::new(ctx.now),
        options(&ctx),
    )
    .await;
    let receipt = result(&mut tx, done).await?;
    let receipt = metadata::receipt(&ctx, &mut tx, &principal, receipt).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(receipt)
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
