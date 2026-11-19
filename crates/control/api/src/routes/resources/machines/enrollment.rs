//! Issuer-scoped enrollment progress, reusing the native resource projection and its current rights.

use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Answer, Ctx, Registry};
use http::header::CACHE_CONTROL;
use orch_api_contract::everything::EverythingResource;
use orch_api_contract::resources::{EnrollmentProgress, MachineEnrollmentStatusGet};
use orch_authz::Principal;
use orch_db::{Conn, DbError};
use orch_realtime_contract::feeds::ResourceKind;
use orch_types::Id;

/// Enrollment stores a 256-bit one-way fingerprint, encoded as hex in the receipt.
const RECEIPT_BYTES: usize = 32;

/// Binds the issuer-only recovery reader; live enrollment notices use the same projection.
pub(super) fn mount(registry: &mut Registry) {
    registry.on(MachineEnrollmentStatusGet, get);
}

async fn get(ctx: Ctx, (): (), (): ()) -> ApiResult<Answer<EnrollmentProgress>> {
    let receipt = ctx.param("receipt")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    Ok(
        Answer::from(show(&ctx, &mut conn, &principal, receipt).await?)
            .header(CACHE_CONTROL, crate::sign_in::NOT_KEPT),
    )
}

/// The same admitted projection supplies HTTP recovery and the issuer's live enrollment event.
pub(crate) async fn show(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    receipt: &str,
) -> ApiResult<EnrollmentProgress> {
    let fingerprint = hex::decode(receipt)
        .ok()
        .filter(|value| value.len() == RECEIPT_BYTES)
        .ok_or_else(|| ApiError::invalid("receipt", "must name an enrollment"))?;
    let (purpose, machine) =
        orch_db::queries::resources::enrollment_progress(conn, ctx.user()?.id(), &fingerprint)
            .await?
            .ok_or_else(ApiError::not_found)?;
    let mut progress = EnrollmentProgress {
        receipt: receipt.to_owned(),
        machine: None,
        host: None,
    };
    let Some(machine) = machine else {
        return Ok(progress);
    };
    let (kind, id) = if purpose == "host" {
        let host: Id = sqlx::query_scalar("SELECT id FROM hosts WHERE machine_id=$1")
            .bind(machine)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?
            .ok_or_else(ApiError::not_found)?;
        (ResourceKind::Host, host)
    } else {
        (ResourceKind::Machine, machine)
    };
    match crate::realtime::resource_rows::read(ctx, conn, principal, kind, id).await? {
        EverythingResource::Machine(row) => progress.machine = Some(row),
        EverythingResource::Host(row) => progress.host = Some(row),
        _ => {
            return Err(ApiError::internal(
                "an enrollment returned the wrong resource kind",
            ));
        }
    }
    Ok(progress)
}
