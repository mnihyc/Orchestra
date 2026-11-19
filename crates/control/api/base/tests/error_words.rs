//! Every public problem uses the shared catalogue; diagnostics never become its message.

use orch_api_contract::common::{ErrorCode, FieldFault, PROBLEM_JSON, Problem};
use orch_api_core::middleware::requests::ApiError;
use orch_db::{Db, DbError};
use orch_testkit::database::TestDb;
use std::time::Duration;

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

async fn deadline_pool(test: &TestDb, statement: u64, idle: u64) -> Db {
    let mut limits = orch_db::pool::Limits::REQUESTS;
    limits.statement = Some(Duration::from_millis(statement));
    limits.idle_in_transaction = Duration::from_millis(idle);
    Db::connect_with_limits(
        &test.url,
        1,
        Duration::from_secs(1),
        Duration::from_secs(1),
        limits,
    )
    .await
    .unwrap_or_else(|error| panic!("fixture pool: {error}"))
}

fn as_json(problem: &Problem) -> serde_json::Value {
    serde_json::to_value(problem).unwrap_or_else(|error| panic!("problem JSON: {error}"))
}

#[tokio::test]
async fn a_connection_wait_reports_the_deadline_of_the_pool_that_was_actually_full() {
    let test = TestDb::fresh().await;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_millis(120))
        .connect(&test.url)
        .await
        .unwrap_or_else(|error| panic!("fixture pool: {error}"));
    let held = pool
        .acquire()
        .await
        .unwrap_or_else(|error| panic!("fixture connection: {error}"));
    let db = Db::from_pool(pool);
    let problem = orch_db::pool::serving("a refused connection wait", async {
        let Err(error) = db.acquire().await else {
            panic!("the only connection is held");
        };
        body(ApiError::from(error)).await
    })
    .await;
    let problem = as_json(&problem);
    assert_eq!(problem["database_wait"]["phase"], "acquire");
    assert_eq!(problem["database_wait"]["applied_limit_ms"], 120);
    assert_eq!(problem["database_wait"]["occurrence_id"], "word-regression");
    drop(held);
    db.pool().close().await;
}

#[tokio::test]
async fn a_failed_statement_keeps_its_applied_deadline_when_later_work_uses_another_pool() {
    let test = TestDb::fresh().await;
    let earlier = deadline_pool(&test, 120, 10_000).await;
    let later = deadline_pool(&test, 9_000, 10_000).await;
    let problem = orch_db::pool::serving("a timed out statement", async {
        let mut conn = earlier
            .acquire()
            .await
            .unwrap_or_else(|error| panic!("fixture connection: {error}"));
        let Err(native) = sqlx::query("SELECT pg_sleep(2)").execute(&mut *conn).await else {
            panic!("the native statement deadline must expire");
        };
        let captured = DbError::from(native);
        drop(conn);
        let _later = later
            .acquire()
            .await
            .unwrap_or_else(|error| panic!("later connection: {error}"));
        body(ApiError::engine(orch_engine::EngineError::BeforeEffect(
            Box::new(orch_engine::EngineError::Db(captured)),
        )))
        .await
    })
    .await;
    let problem = as_json(&problem);
    assert_eq!(problem["database_wait"]["phase"], "statement");
    assert_eq!(problem["database_wait"]["applied_limit_ms"], 120);
    assert_eq!(problem["database_wait"]["service"], "api");
    assert!(!problem.to_string().contains("pg_sleep"));
    earlier.pool().close().await;
    later.pool().close().await;
}

#[tokio::test]
async fn an_idle_transaction_expiry_is_separate_from_a_statement_deadline() {
    let test = TestDb::fresh().await;
    let db = deadline_pool(&test, 9_000, 120).await;
    let problem = orch_db::pool::serving("an idle transaction", async {
        let mut conn = db
            .acquire()
            .await
            .unwrap_or_else(|error| panic!("fixture connection: {error}"));
        sqlx::query("BEGIN")
            .execute(&mut *conn)
            .await
            .unwrap_or_else(|error| panic!("fixture BEGIN: {error}"));
        tokio::time::sleep(Duration::from_millis(360)).await;
        let Err(native) = sqlx::query("SELECT 1").execute(&mut *conn).await else {
            panic!("PostgreSQL must end the idle transaction");
        };
        body(ApiError::from(DbError::from(native))).await
    })
    .await;
    let problem = as_json(&problem);
    assert_eq!(problem["database_wait"]["phase"], "idle_transaction");
    assert_eq!(problem["database_wait"]["applied_limit_ms"], 120);
    db.pool().close().await;
}

#[tokio::test]
async fn an_unreported_pool_profile_does_not_invent_a_statement_deadline() {
    let test = TestDb::fresh().await;
    let mut conn = test
        .pool
        .acquire()
        .await
        .unwrap_or_else(|error| panic!("fixture connection: {error}"));
    sqlx::query("SET statement_timeout='120ms'")
        .execute(&mut *conn)
        .await
        .unwrap_or_else(|error| panic!("fixture deadline: {error}"));
    let Err(native) = sqlx::query("SELECT pg_sleep(2)").execute(&mut *conn).await else {
        panic!("the native statement deadline must expire");
    };
    let problem = as_json(&body(ApiError::from(DbError::from(native))).await);
    assert_eq!(problem["database_wait"]["phase"], "statement");
    assert!(problem["database_wait"]["applied_limit_ms"].is_null());
}

#[tokio::test]
async fn a_native_cancellation_or_an_application_error_cannot_claim_a_statement_timeout() {
    let test = TestDb::fresh().await;
    for statement in [
        "SELECT pg_cancel_backend(pg_backend_pid()), pg_sleep(2)",
        "DO $fixture$ BEGIN RAISE EXCEPTION USING ERRCODE='57014', MESSAGE='canceling statement due to statement timeout'; END $fixture$",
    ] {
        let mut conn = test
            .pool
            .acquire()
            .await
            .unwrap_or_else(|error| panic!("fixture connection: {error}"));
        let Err(native) = sqlx::query(statement).execute(&mut *conn).await else {
            panic!("the native cancellation/error must be reported");
        };
        assert_eq!(
            native
                .as_database_error()
                .and_then(|error| error.code())
                .as_deref(),
            Some("57014")
        );
        let problem = as_json(&body(ApiError::from(DbError::from(native))).await);
        assert!(
            problem.get("database_wait").is_none(),
            "cancellation is not a known timeout: {problem}"
        );
    }
}

#[tokio::test]
async fn native_field_words_are_kept_only_for_the_unchanged_authorized_refusal() {
    let detail = "environment.roots: /srv/team_workspace/notes.v2 is outside the selected root";
    let native = body(ApiError::native_invalid("path", detail)).await;
    assert_eq!(native.code, ErrorCode::RequestInvalid);
    assert_eq!(
        native.extra["errors"],
        serde_json::json!([{"path":"path","message":detail}])
    );
    plain(&native.title);
    plain(&native.detail);

    let mut changed_code = ApiError::native_invalid("path", detail);
    changed_code.problem.code = ErrorCode::AuthzDenied;
    for error in [
        ApiError::native_invalid("path", detail).without_content(),
        ApiError::native_invalid("path", detail).with(
            "errors",
            [FieldFault {
                path: "another_field".into(),
                message: detail.into(),
            }],
        ),
        changed_code,
        // A wire problem carries no server-side authority to publish native content.
        ApiError::from(native),
    ] {
        let problem = body(error).await;
        let wire =
            serde_json::to_string(&problem).unwrap_or_else(|error| panic!("problem JSON: {error}"));
        assert!(!wire.contains("team_workspace"), "{wire}");
        assert!(!wire.contains("environment.roots"), "{wire}");
    }
}
