//! The audit trail of the whole system, read a page at a time with its filters and written out as a streamed file for a span of at most a year.
//!
//! The trail is kept for good and is append-only; this is how an operator reads it and takes a copy.
//! The list and the file are made of the same entries, so that a file holds exactly what the list
//! holds for the same span and filters. Taking a copy is itself audited, and the line is written
//! before the first row is read, so that a copy cannot be taken without leaving it.

orch_api_base::api_family_callback! {
    bind __api_bind_944d774ef1, __api_install_944d774ef1, __api_callback_944d774ef1;
    slot __ApiCallback944d774ef1, __API_CALLBACK_944D774EF1, __api_set_944d774ef1;
    base orch_api_base::routes::admin::audit;

    async fn export_scope(arg0: orch_api_base::registry::Ctx, arg1: orch_api_contract::admin::AdminAuditExportQuery, arg2: bool) -> orch_api_base::middleware::requests::ApiResult<axum::response::Response>;
}
