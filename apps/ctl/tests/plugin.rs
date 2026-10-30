//! Provider programs are checked and registered through the real CLI, with failures leaving local configuration intact.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_types::Json;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

/// A complete transport suite starts several disposable programs; a busy build host still finishes promptly.
const PATIENCE: Duration = Duration::from_secs(120);

fn fixture(dir: &Path, broken: bool) -> (PathBuf, PathBuf) {
    let declaration = json!({"name":"test-plan","title":"Test plan","summary":"Reports a disposable plan",
        "contract":1,"facts":{"reports":["windows"],"credential":"none"},
        "models":null,"presents":null,"fields":[]});
    let window = json!({"windows":[{"id":"five-hour","label":"5 hours","duration_minutes":300,
        "used_percent":if broken {json!("wrong-type")} else {json!(25)},"resets_at":null}]});
    let examples = [
        json!({}),
        json!({"windows":null}),
        json!({"windows":[]}),
        window,
    ];
    let script = json!({"kind":"provider","name":"test-plan","version":"1.0.0",
        "methods":["describe","facts.read"],"descriptor":declaration,
        "responses":{"describe":{"result":declaration},"facts.read":examples.iter()
            .map(|value| json!({"result":value})).collect::<Vec<_>>()}});
    let script_file = dir.join("program.json");
    std::fs::write(&script_file, script.to_string()).unwrap();
    let ask = json!({"provider":{"id":"0199abcd-1234-7000-8000-0000000000a1","name":"Disposable"},
        "endpoints":[],"config":{},"secrets":{}});
    let cases = examples
        .iter()
        .map(|value| json!({"ask":ask,"expected":value}))
        .collect::<Vec<_>>();
    let fixture_file = dir.join("fixture.json");
    std::fs::write(
        &fixture_file,
        json!({"facts":cases,"models":[],"reset":null,"present":null,
        "refusal":null,"now":"2026-10-15T10:00:00.000Z","secrets":[]})
        .to_string(),
    )
    .unwrap();
    (script_file, fixture_file)
}

fn program() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/platform/plugin/tests/support/scripted_plugin.py")
}

async fn run(mut command: Command) -> std::process::Output {
    command.kill_on_drop(true);
    tokio::time::timeout(PATIENCE, command.output())
        .await
        .unwrap()
        .unwrap()
}

fn says(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[tokio::test]
async fn plugin_check_discovers_a_provider_and_runs_the_member_state_suite() {
    let dir = tempfile::tempdir().unwrap();
    let (script, fixture) = fixture(dir.path(), false);
    let mut command = Command::new(env!("CARGO_BIN_EXE_orchctl"));
    command
        .args(["plugin", "check", "--fixture"])
        .arg(fixture)
        .arg("python3")
        .arg(program())
        .arg("--script")
        .arg(script);
    let output = run(command).await;
    assert!(output.status.success(), "{}", says(&output));
    let text = says(&output);
    for rule in [
        "transport.hello",
        "provider.descriptor",
        "provider.facts.shape",
        "provider.facts.states",
    ] {
        assert!(
            text.lines()
                .any(|line| line.contains("passed") && line.contains(rule)),
            "{text}"
        );
    }
    assert!(
        text.lines()
            .any(|line| line.contains("skipped") && line.contains("provider.resets.idempotent")),
        "checking does not spend an undisclosed reset: {text}"
    );
}

#[tokio::test]
async fn plugin_add_checks_then_preserves_existing_limits_and_registers_the_provider_name() {
    let dir = tempfile::tempdir().unwrap();
    let (script, fixture) = fixture(dir.path(), false);
    let config = dir.path().join("config.toml");
    let original = "# the owner's limits\n[environment]\nroots=['/work']\nremote_write=false\n";
    std::fs::write(&config, original).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_orchctl"));
    command
        .args(["plugin", "add", "--name", "custom-plan", "--config-file"])
        .arg(&config)
        .args(["--kind", "provider", "--fixture"])
        .arg(fixture)
        .arg("python3")
        .arg(program())
        .arg("--script")
        .arg(script);
    let output = run(command).await;
    assert!(output.status.success(), "{}", says(&output));
    let written = std::fs::read_to_string(&config).unwrap();
    assert!(written.starts_with(original));
    let config: orch_config::ConnectorConfig =
        orch_config::load_text(&written, "config.toml", |_| None).unwrap();
    assert!(!config.environment.remote_write);
    assert_eq!(config.environment.roots, ["/work"]);
    assert_eq!(config.plugin[0].kind, "provider");
    assert_eq!(config.plugin[0].name, "custom-plan");
    assert!(says(&output).contains("restart the connector"));
}

#[tokio::test]
async fn a_provider_that_reports_windows_in_another_shape_is_not_added_and_the_file_is_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let (script, fixture) = fixture(dir.path(), true);
    let config = dir.path().join("config.toml");
    let original = "# keep these limits\n[environment]\nremote_write=false\n";
    std::fs::write(&config, original).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_orchctl"));
    command
        .args(["plugin", "add", "--name", "broken-plan", "--config-file"])
        .arg(&config)
        .args(["--kind", "provider", "--fixture"])
        .arg(fixture)
        .arg("python3")
        .arg(program())
        .arg("--script")
        .arg(script);
    let output = run(command).await;
    assert!(!output.status.success());
    assert!(
        says(&output)
            .lines()
            .any(|line| line.contains("FAILED") && line.contains("provider.facts.shape")),
        "{}",
        says(&output)
    );
    assert_eq!(std::fs::read_to_string(&config).unwrap(), original);
}

#[tokio::test]
async fn a_malformed_provider_fixture_names_no_secret_in_the_cli_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let (script, fixture) = fixture(dir.path(), false);
    let mut value: Json = serde_json::from_slice(&std::fs::read(&fixture).unwrap()).unwrap();
    value["facts"][0]["ask"]["provider"]["id"] = json!("private-fixture-value");
    std::fs::write(&fixture, value.to_string()).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_orchctl"));
    command
        .args(["plugin", "check", "--kind", "provider", "--fixture"])
        .arg(fixture)
        .arg("python3")
        .arg(program())
        .arg("--script")
        .arg(script);
    let output = run(command).await;
    assert!(!output.status.success());
    assert!(!says(&output).contains("private-fixture-value"));
    assert!(says(&output).contains("fixture has another shape"));
}

#[tokio::test]
async fn an_address_uses_the_same_suite_with_a_file_token_and_plain_http_only_when_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let token = dir.path().join("address.token");
    let value = "cli-address-token-private";
    std::fs::write(&token, value).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let ready = dir.path().join("ready.json");
    let mut server = Command::new("python3")
        .arg(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/plugins/provider/address.py"),
        )
        .arg("--state-dir")
        .arg(dir.path().join("account"))
        .arg("--token-file")
        .arg(&token)
        .arg("--ready-file")
        .arg(&ready)
        .args(["--port", "0"])
        .args(["--at", "2026-10-15T00:00:00Z"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut output = BufReader::new(server.stdout.take().unwrap()).lines();
    let signal = tokio::time::timeout(PATIENCE, output.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let published: Json = serde_json::from_str(&signal).unwrap();
    let url = published["url"].as_str().unwrap();
    let fixture = dir.path().join("address-fixture.json");
    std::fs::write(
        &fixture,
        json!({"facts":[{"ask":{
        "provider":{"id":"0199abcd-1234-7000-8000-0000000000a1","name":"Disposable"},
        "endpoints":[],"config":{"mode":"absent"},"secrets":{}},"expected":{}}],
        "models":[],"reset":null,"present":null,"refusal":null,
        "now":"2026-10-15T00:00:00.000Z","secrets":[]})
        .to_string(),
    )
    .unwrap();
    let make = |token: &Path, allow: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_orchctl"));
        command
            .args(["plugin", "check", "--address", url, "--token-file"])
            .arg(token)
            .arg("--fixture")
            .arg(&fixture);
        if allow {
            command.arg("--allow-http");
        }
        command
    };
    let accepted = run(make(&token, true)).await;
    assert!(accepted.status.success(), "{}", says(&accepted));
    assert!(
        says(&accepted)
            .lines()
            .any(|line| line.contains("passed") && line.contains("provider.facts.shape"))
    );
    assert!(
        says(&accepted)
            .lines()
            .any(|line| line.contains("skipped") && line.contains("provider.credential.present"))
    );
    assert!(!says(&accepted).contains(value));
    let denied = run(make(&token, false)).await;
    assert!(!denied.status.success());
    assert!(says(&denied).contains("scheme is not allowed"));
    assert!(!says(&denied).contains(value));
    let wrong = dir.path().join("wrong.token");
    std::fs::write(&wrong, "wrong-cli-address-token").unwrap();
    let unauthorized = run(make(&wrong, true)).await;
    assert!(!unauthorized.status.success());
    assert!(says(&unauthorized).contains("unauthorized"));
    assert!(!says(&unauthorized).contains("wrong-cli-address-token"));
    server.kill().await.unwrap();
    server.wait().await.unwrap();
}
