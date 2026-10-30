//! A fixture-only content refusal for gateway worlds that exercise identity/backplane behavior.

use axum::{Router, routing::any};
use orch_api_core::{certificates::LinkFault, state::App};

/// A deterministic refusal; content-transfer tests supply their actual content router instead.
pub(crate) fn router() -> Router<App> {
    Router::new().route(
        "/internal/link/blobs/{*rest}",
        any(|| async {
            LinkFault {
                status: 501,
                code: "content.unavailable",
                detail: "Content is disabled in this test fixture.".to_owned(),
            }
        }),
    )
}
