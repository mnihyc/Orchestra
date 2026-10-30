//! Provider health projected through the same bounded, authorized reader as the status API.

use super::server::Connection;
use super::{Fail, Inner};
use orch_types::{Id, Json};
use std::sync::Arc;

/// No transaction or pooled connection survives this projection; it contains no model-request rows.
pub(crate) async fn project(
    inner: &Arc<Inner>,
    connection: &Connection,
    provider: Id,
) -> Result<Option<Json>, Fail> {
    let mut conn = inner.db.acquire().await?;
    let Some(principal) =
        super::subscriptions::principal_for_connection(inner, &mut conn, connection).await?
    else {
        return Ok(None);
    };
    match crate::routes::usage::figures::health_for(
        &mut conn,
        &principal,
        provider,
        inner.clock.now(),
    )
    .await
    {
        Ok(health) => Ok(Some(serde_json::json!({"what":"health","health":health}))),
        Err(error) if error.code() == orch_api_contract::common::ErrorCode::Internal => {
            Err(Fail::other(error))
        }
        Err(_) => Ok(None),
    }
}

/// Advancing the window alone is not a visible change; its measured results and flags are.
pub(crate) fn fingerprint(data: &Json) -> orch_types::Hash {
    let mut measured = data.clone();
    if let Some(health) = measured.get_mut("health").and_then(Json::as_object_mut) {
        health.remove("since");
    }
    orch_types::Hash::of(measured.to_string().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_measures_send_no_new_status_just_because_time_passed() {
        let first = serde_json::json!({"what":"health","health":{"since":"2026-10-15T16:00:00Z","errors":false,"first_output":false,"recent":{"requests":12}}});
        let mut later = first.clone();
        later["health"]["since"] = serde_json::json!("2026-10-15T16:00:10Z");
        assert_eq!(fingerprint(&first), fingerprint(&later));
        later["health"]["errors"] = serde_json::json!(true);
        assert_ne!(fingerprint(&first), fingerprint(&later));
        later["health"]["errors"] = serde_json::json!(false);
        later["health"]["recent"]["requests"] = serde_json::json!(13);
        assert_ne!(fingerprint(&first), fingerprint(&later));
    }
}
