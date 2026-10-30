//! A requested content-check failure retains its machine reason beside the standard API problem.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use axum::http::StatusCode;
use orch_api_contract::common::{ErrorCode, Problem};
use orch_api_core::middleware::requests::ApiError;

#[tokio::test]
async fn a_requested_mount_check_retains_its_read_failure_beside_the_catalogue_problem() {
    let reason = "The data host cannot read this location";
    let response = ApiError::engine(orch_engine::EngineError::MountFingerprint(reason.into()))
        .into_response_for("content-check");
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let problem: Problem = serde_json::from_slice(&body).unwrap();
    assert_eq!(problem.code, ErrorCode::PlanBlocked);
    assert_eq!(problem.extra["blockers"][0]["message"], reason);
    assert_eq!(
        problem.extra["blockers"][0]["code"],
        "mount_fingerprint_unavailable"
    );
    assert_eq!(problem.request_id, "content-check");
}
