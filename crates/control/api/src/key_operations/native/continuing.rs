//! The same native original-operation frame for API-owned workers, without a request context.

use orch_engine::operation_scope::continuing as shared;
pub(crate) use shared::{Error, Source};

/// Domain workers supply actual immutable native association and its existing source factory.
/// Known invalid original credentials remain errors and never fall back to ordinary work.
pub(crate) async fn prepare<F: Source>(
    conn: &mut orch_db::Conn,
    clock: &dyn orch_types::Clock,
    origin: &orch_authz::CapturedOperation,
    input: F::Input,
) -> Result<shared::Frame<F>, Error> {
    shared::prepare::<F>(conn, clock, origin, input).await
}
