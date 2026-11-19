//! Every public problem uses the shared catalogue; diagnostics never become its message.

use orch_api_contract::common::{ErrorCode, FieldFault, PROBLEM_JSON, Problem};
use orch_api_core::middleware::requests::ApiError;
use orch_db::DbError;

async fn body(error: ApiError) -> Problem {
    let response = error.into_response_for("word-regression");
    assert_eq!(response.headers()["content-type"], PROBLEM_JSON);
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap_or_else(|error| panic!("problem body: {error}"));
    serde_json::from_slice(&bytes).unwrap_or_else(|error| panic!("problem JSON: {error}"))
}

fn plain(message: &str) {
    for token in [
        "model_inputs_segment_id_fkey",
        "threads_starting_step_check",
        "store_unavailable",
        "harness_unavailable",
        "agent.harnesses",
        "home_node_id",
    ] {
        assert!(!message.contains(token), "diagnostic in message: {message}");
    }
}

#[tokio::test]
async fn every_api_error_answers_with_its_catalogue_sentence_and_retains_machine_fields() {
    for code in ErrorCode::ALL {
        let problem = body(
            ApiError::new(
                *code,
                "harness_unavailable: agent.harnesses: threads_starting_step_check",
            )
            .with("detail", "harness_unavailable: agent.harnesses")
            .with("message", "threads_starting_step_check")
            .with("required", "files.view")
            .with(
                "errors",
                [FieldFault {
                    path: "home_node_id".into(),
                    message: "model_inputs_segment_id_fkey is missing".into(),
                }],
            ),
        )
        .await;
        assert_eq!(problem.code, *code);
        assert_eq!(problem.status, code.status());
        assert_eq!(problem.request_id, "word-regression");
        assert_eq!(
            problem.extra["required"], "files.view",
            "structured authority is not display copy"
        );
        assert_eq!(
            problem.extra["errors"][0]["path"], "home_node_id",
            "the form can still focus the field"
        );
        plain(&problem.title);
        plain(&problem.detail);
        plain(
            problem.extra["message"]
                .as_str()
                .unwrap_or_else(|| panic!("missing public message")),
        );
        plain(
            problem.extra["errors"][0]["message"]
                .as_str()
                .unwrap_or_else(|| panic!("missing field explanation")),
        );
        assert!(
            problem.detail.contains('.'),
            "the response needs a sentence and recovery: {code}"
        );
    }
}

#[tokio::test]
async fn database_names_stay_out_of_conflict_and_value_refusal_messages() {
    for error in [
        DbError::Unique {
            constraint: "model_inputs_segment_id_fkey".into(),
        },
        DbError::Check {
            table: "threads".into(),
            constraint: "threads_starting_step_check".into(),
        },
    ] {
        let problem = body(ApiError::from(error)).await;
        plain(&problem.title);
        plain(&problem.detail);
        assert!(problem.status == 409 || problem.status == 422);
    }
}
