//! An administrative opening is anchored to one completed custody row, not an arbitrary hash.
use crate::key_operations::{native, targets};
use crate::middleware::{
    admin_openings,
    requests::{ApiError, ApiResult},
    transaction_guard,
};
use crate::registry::{Ctx, Registry};
use axum::{body::Body, response::Response};
use futures::future::BoxFuture;
use orch_access_contract::key_scope::{KeyIdentityKind, KeyTargetKind};
use orch_api_contract::{
    operation_sources::Declaration,
    threads::{AdminUploadContentGet, UploadBlobGetQuery},
    upload_sources::{self as sources, Source},
};
use orch_authz::{Caller, Principal};
use orch_db::{Conn, queries::uploads::inventory};
use std::sync::Arc;

pub(super) fn mount(registry: &mut Registry) {
    registry.on_raw_read_with_optional_operation(
        AdminUploadContentGet,
        native::selected::<Content>,
        get,
    );
}
struct Content;
#[derive(Clone, PartialEq, Eq)]
struct Selected {
    row: inventory::Row,
    name: String,
}
impl native::Selected for Content {
    type Source = Source;
    type Query = UploadBlobGetQuery;
    type Value = Arc<Selected>;
    type Reply = Arc<Selected>;
    const DECLARATION: &'static Declaration<Source> = &sources::ADMIN_OPEN;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let root = native::Facts::one(
                Source::AdminBlob,
                KeyTargetKind::Identity(KeyIdentityKind::System),
                targets::system(),
            );
            let known = native::bind_known(ctx, Self::DECLARATION, std::slice::from_ref(&root))?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            super::native::require(ctx, &principal)?;
            let row = inventory::completed(conn, ctx.id("id")?)
                .await?
                .ok_or_else(ApiError::not_found)?;
            if let Some(opened) = ctx.optional_retained_source::<inventory::Row>()?
                && *opened != row
            {
                return Err(ApiError::not_found());
            }
            if row.blob()?.is_none() {
                return Err(ApiError::not_found());
            }
            let name = crate::uploads::clean_name("name", &query.name)?;
            Ok((Arc::new(Selected { row, name }), vec![root]))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { super::native::require(ctx, principal) })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>> {
        Box::pin(async move { Ok(Arc::clone(value)) })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a Self::Query,
        opened: &'a Self::Value,
        fresh: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(opened == fresh) })
    }
}

async fn get(ctx: Ctx, query: UploadBlobGetQuery, _body: Body) -> ApiResult<Response> {
    let selected = native::selected_read::<Content>(ctx.clone(), query.clone(), ()).await?;
    ctx.retain_source(selected.row.clone())?;
    let hash = selected.row.blob()?.ok_or_else(ApiError::not_found)?;
    let response =
        crate::routes::threads::uploads::serve_completed(&ctx, hash, selected.name.clone(), query)
            .await?;
    if !response.status().is_success() && response.status() != http::StatusCode::NOT_MODIFIED {
        return Ok(response);
    }
    // The same current native source is rechecked after the actual opener/preview/archive waits.
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    super::native::require(&ctx, guard.principal())?;
    let id = ctx.id("id")?;
    if inventory::completed(&mut tx, id).await?.as_ref() != Some(&selected.row) {
        return Err(ApiError::not_found());
    }
    guard.check_deadline(&ctx)?;
    admin_openings::note(
        &ctx,
        &mut tx,
        "upload",
        id,
        serde_json::json!("system"),
        admin_openings::nonce(&ctx)?,
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(response)
}
