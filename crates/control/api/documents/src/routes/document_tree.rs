//! Typed document-folder metadata, child lists and atomic replay-safe organization.

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::registry::Ctx;

use crate::registry::Registry;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::document_tree::*;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::DbError;

use orch_engine::document_tree as domain;

use orch_types::Id;

use std::collections::HashMap;

pub use orch_api_base::routes::document_tree::convert;

pub use orch_api_base::routes::document_tree::result;
/// Exact unit conversion for the deployment's retention policy.
const SECONDS_PER_DAY: u64 = 86_400;

/// Registers only typed document-domain endpoints; none accepts hierarchy folders as a container.
pub fn mount(registry: &mut Registry) {
    registry
        .creation(DocumentContainerCreate, creation::replay_binding())
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
        .on_with_operation(
            DocumentTreeDeletePreview,
            crate::key_operations::native::effects::prepare::<deletion::Reviews>,
            deletion::preview,
        )
        .on_with_operation(
            DocumentTreeOperate,
            crate::key_operations::native::effects::prepare::<operation::Operations>,
            operation::operate,
        );
}
orch_api_base::__api_bind_f1635129b9!();

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
orch_api_base::__api_bind_b705bab7c6!();

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
orch_api_base::__api_bind_d61ba637ac!();

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
    TreePage(&'a std::sync::Arc<list::Ready>),
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
    match source {
        Some(ReadSource::TreePage(source))
            if crate::key_operations::native::modern(guard.principal().caller()) =>
        {
            // The fresh guard replays preparation, whose partial discovery marks response
            // disclosure Pending. Revalidate this same frozen page through its Read boundary
            // before publication; ordinary source checks alone cannot complete disclosure.
            let cut = read.cut.clone();
            let source = std::sync::Arc::clone(source);
            let checking = ctx.clone();
            list::phase("bookkeeping response check");
            source
                .response_plan()
                .read_operation(
                    ctx,
                    &mut tx,
                    guard.principal().caller(),
                    move |conn, reader| {
                        Box::pin(async move {
                            metadata::unchanged(conn, reader, &cut, reader.now()).await?;
                            list::check_sources(&checking, conn, reader, &source).await
                        })
                    },
                )
                .await?
                .ok_or_else(|| {
                    ApiError::new(
                        ErrorCode::AuthzDenied,
                        "the frozen document page is no longer readable",
                    )
                })?;
            list::phase("bookkeeping response proven");
        }
        Some(ReadSource::Entry(source)) => {
            metadata::unchanged(&mut tx, guard.principal(), &read.cut, guard.now()).await?;
            read::check_sources(ctx, &mut tx, guard.principal().caller(), source).await?;
        }
        Some(ReadSource::TreePage(source)) => {
            metadata::unchanged(&mut tx, guard.principal(), &read.cut, guard.now()).await?;
            list::check_sources(ctx, &mut tx, guard.principal(), source).await?;
        }
        None => metadata::unchanged(&mut tx, guard.principal(), &read.cut, guard.now()).await?,
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

#[path = "document_tree/confirmation.rs"]
pub mod confirmation;

#[path = "document_tree/creation.rs"]
pub mod creation;

#[path = "document_tree/deletion.rs"]
pub mod deletion;

#[path = "document_tree/list.rs"]
pub mod list;

#[path = "document_tree/metadata.rs"]
pub mod metadata;

#[path = "document_tree/operation.rs"]
pub mod operation;

#[path = "document_tree/read.rs"]
pub mod read;
