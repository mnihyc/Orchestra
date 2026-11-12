//! Thin native error adapter over the common original raw identity evaluator.
use super::continuing::Error;
pub use orch_authz::http_origin::Credential;
use orch_authz::{Caller, CapturedOperation};
use orch_db::{Conn, queries::transaction_guard};
use orch_types::{Clock, Time};

pub(crate) async fn current(
    conn: &mut Conn,
    clock: &dyn Clock,
    origin: &CapturedOperation,
    credential: &Credential,
    original_until: Option<Time>,
    operation: &str,
) -> Result<(Caller, Option<Time>), Error> {
    let (caller, until, _) =
        current_identity(conn, clock, origin, credential, original_until, operation).await?;
    Ok((caller, until))
}
pub(crate) async fn current_identity(
    conn: &mut Conn,
    clock: &dyn Clock,
    origin: &CapturedOperation,
    credential: &Credential,
    original_until: Option<Time>,
    operation: &str,
) -> Result<(Caller, Option<Time>, Option<transaction_guard::Identity>), Error> {
    orch_authz::http_origin::current_identity(
        conn,
        clock,
        origin,
        credential,
        original_until,
        operation,
    )
    .await
    .map_err(super::http_origin::mapped)
}
