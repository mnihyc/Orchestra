//! The scripted model server as a program: where it listens, what credential it requires, and how it stops.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use std::process::Stdio;
use std::time::Duration;

use orch_testkit::doubles::model::{ClientRequest, ModelClient};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

/// How long the program may take to announce where it listens, or to stop.
const PATIENCE: Duration = Duration::from_secs(20);

/// Starts the program on a port of the loopback that the system gives, and reads where it listens.
async fn start(credential: Option<&str>, extra: &[&str]) -> (Child, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_orch-mock-llm"));
    command
        .args(["--listen", "127.0.0.1:0"])
        .args(extra)
        .env_remove("ORCH_MOCK_LLM_CREDENTIAL")
        .stdout(Stdio::piped())
        .kill_on_drop(true);
    if let Some(credential) = credential {
        command.env("ORCH_MOCK_LLM_CREDENTIAL", credential);
    }
    let mut child = command.spawn().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut lines = BufReader::new(stdout).lines();
    let first = tokio::time::timeout(PATIENCE, lines.next_line())
        .await
        .expect("the program announces itself")
        .unwrap()
        .expect("the program prints a line");
    let url = first
        .strip_prefix("listening on ")
        .expect("the first line says where")
        .to_owned();
    (child, url)
}

#[tokio::test]
async fn the_program_requires_the_credential_of_its_environment_and_answers_with_it() {
    let (mut child, url) = start(Some("sk-program-test"), &[]).await;
    let client = ModelClient::new(&url);
    let health = client
        .send(ClientRequest::get("/_mock/health"))
        .await
        .unwrap();
    assert_eq!(
        health.status, 200,
        "the control endpoint needs no credential"
    );
    let body = json!({ "model": "mock-large", "input": "hello" });
    let refused = client
        .send(ClientRequest::post_json("/v1/responses", &body))
        .await
        .unwrap();
    assert_eq!(refused.status, 401);
    let answered = client
        .send(ClientRequest::post_json("/v1/responses", &body).bearer("sk-program-test"))
        .await
        .unwrap();
    assert_eq!(answered.status, 200);
    assert_eq!(
        answered.json()["output"][0]["content"][0]["text"],
        "You said: hello"
    );
    child.kill().await.unwrap();
}

#[tokio::test]
async fn a_program_without_a_credential_lets_everyone_in_and_lists_the_models_it_was_given() {
    let (mut child, url) = start(None, &["--model", "only-model"]).await;
    let client = ModelClient::new(&url);
    let list = client
        .send(ClientRequest::get("/v1/models"))
        .await
        .unwrap()
        .json();
    assert_eq!(list["data"][0]["id"], "only-model");
    assert_eq!(list["data"].as_array().unwrap().len(), 1);
    child.kill().await.unwrap();
}

#[tokio::test]
async fn the_program_stops_when_it_is_told_to_terminate() {
    let (mut child, _url) = start(None, &[]).await;
    let pid = child.id().expect("a running child has an id").to_string();
    let sent = Command::new("kill")
        .args(["-TERM", &pid])
        .status()
        .await
        .unwrap();
    assert!(sent.success());
    let status = tokio::time::timeout(PATIENCE, child.wait())
        .await
        .expect("the program stops")
        .unwrap();
    assert!(
        status.success(),
        "a termination signal is a stop and no failure: {status}"
    );
}
