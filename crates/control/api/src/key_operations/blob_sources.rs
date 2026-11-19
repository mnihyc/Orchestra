//! HTTP assurance/error adapter for the shared stored-byte source planner.

use crate::middleware::authorization::refusals;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, OperationPlan};
use orch_api_contract::common::ErrorCode;
use orch_authz::Caller;
use orch_db::Conn;
use orch_engine::operation_scope::{BindingError, blob_sources as shared, targets::TargetError};
use orch_types::Hash;

pub(crate) use shared::{Binding, Prepared, SourceKind};

fn binding_error(error: BindingError) -> ApiError {
    match error {
        BindingError::Catalogue(error) => ApiError::internal(error),
        BindingError::Refused => ApiError::new(
            ErrorCode::AuthzDenied,
            "the key does not allow this operation",
        ),
    }
}
async fn failed(conn: &mut Conn, error: shared::Error) -> ApiError {
    match error {
        shared::Error::Binding(error) => binding_error(error),
        shared::Error::Missing | shared::Error::Target(TargetError::Missing) => {
            ApiError::not_found()
        }
        shared::Error::Target(TargetError::Db(error)) => error.into(),
        shared::Error::Target(TargetError::Authz(error)) => refusals::problem(conn, error).await,
        shared::Error::Source(orch_chat::Error::Db(error)) => error.into(),
        shared::Error::Source(orch_chat::Error::NotFound) => ApiError::not_found(),
        shared::Error::Source(orch_chat::Error::Invalid { path, message }) => {
            ApiError::invalid(&path, &message)
        }
        shared::Error::Source(error) => ApiError::internal(error),
        shared::Error::Declaration(error) => ApiError::internal(error),
    }
}

pub(crate) fn bind(plan: &mut OperationPlan, bindings: &[Binding]) -> ApiResult<()> {
    shared::bind(plan.shared_mut(), bindings).map_err(|error| match error {
        shared::Error::Binding(error) => binding_error(error),
        error => ApiError::internal(error),
    })
}

/// The API supplies its existing principal/assurance boundary; worker origins supply theirs to
/// the same shared helper. No source selection or scope policy is duplicated in this adapter.
pub(crate) async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    base: &OperationPlan,
    hashes: &[Hash],
    required: &[Binding],
) -> ApiResult<Prepared> {
    let principal = base.preparation_principal(ctx, conn, caller).await?;
    match shared::prepare(conn, &principal, base.shared(), hashes, required).await {
        Ok(prepared) => Ok(prepared),
        Err(error) => Err(failed(conn, error).await),
    }
}

/// The same planner restricted to the writer's originally selected source witnesses.
pub(crate) async fn prepare_selected(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    base: &OperationPlan,
    hashes: &[Hash],
    required: &[Binding],
    selected: &[(Hash, orch_chat::following::BlobOrigin)],
) -> ApiResult<Prepared> {
    let principal = base.preparation_principal(ctx, conn, caller).await?;
    match shared::prepare_selected(conn, &principal, base.shared(), hashes, required, selected)
        .await
    {
        Ok(prepared) => Ok(prepared),
        Err(error) => Err(failed(conn, error).await),
    }
}
