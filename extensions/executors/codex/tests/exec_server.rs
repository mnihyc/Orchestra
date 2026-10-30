//! Codex's exec server, started the way an exec node starts it; the vendor's program runs only in Docker.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use base64::Engine as _;
use futures::{SinkExt as _, StreamExt as _};
use orch_agent_contract::descriptor::SandboxMode;
use orch_executor_codex::descriptor;
use orch_executor_codex::server::{CodexConfig, CodexExecServer};
use orch_executor_contract::context::EnvironmentContext;
use orch_executor_contract::executor::{Executor, ExecutorEndpoint};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::http::HeaderValue;

/// The image the vendor's programs run in.
const IMAGE: &str = "orch-dev-harness";

/// Container names start with this; nothing else is ever touched.
const CONTAINER_PREFIX: &str = "orch-test-exec-codex";

/// Options that let the vendor's sandbox start in a container: no filter on system calls and no profile of
/// the security module, which a stock container has and which refuse the namespaces the sandbox needs.
const SANDBOX_FRIENDLY: &str =
    "--security-opt seccomp=unconfined --security-opt apparmor=unconfined";

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// Whether the vendor's programs can be run here, and a message that says so when they cannot.
fn docker_ready() -> bool {
    let ready = StdCommand::new("docker")
        .args(["image", "inspect", IMAGE])
        .output()
        .is_ok_and(|output| output.status.success());
    if !ready {
        eprintln!(
            "SKIPPED: this test runs the vendor's program in Docker and needs the image {IMAGE}"
        );
    }
    ready
}

/// Removes the containers of a test, whatever the test did.
struct Containers(String);

impl Drop for Containers {
    fn drop(&mut self) {
        let listed = StdCommand::new("docker")
            .args(["ps", "-aq", "--filter", &format!("name=^{}-", self.0)])
            .output();
        if let Ok(listed) = listed {
            for id in String::from_utf8_lossy(&listed.stdout).split_whitespace() {
                let _ = StdCommand::new("docker").args(["rm", "-f", id]).output();
            }
        }
    }
}

/// How many containers of a test are there.
fn containers_of(prefix: &str) -> usize {
    let listed = StdCommand::new("docker")
        .args(["ps", "-q", "--filter", &format!("name=^{prefix}-")])
        .output()
        .unwrap();
    String::from_utf8_lossy(&listed.stdout)
        .split_whitespace()
        .count()
}

/// A place for one executor: its data directory, its tool cache and a workspace.
struct Rig {
    outer: tempfile::TempDir,
    env: EnvironmentContext,
    exec: CodexExecServer,
    prefix: String,
    _containers: Containers,
}

fn config() -> CodexConfig {
    CodexConfig {
        // The ports this machine lets a test listen on.
        ports: Some(41000..=41999),
        stop_grace: Duration::from_secs(5),
        ..CodexConfig::default()
    }
}

/// The program that stands for the vendor's: it runs the image's `codex` in a container.
const DOCKER_PROGRAM: &str = r#"#!/bin/sh
set -e
FORWARD=""
for name in CODEX_HOME $ORCH_TEST_FORWARD; do FORWARD="$FORWARD -e $name"; done
exec docker run --rm -i --init --name "$ORCH_TEST_CONTAINER-$$" --network host $ORCH_TEST_DOCKER_OPTS \
  -v "$ORCH_TEST_MOUNT:$ORCH_TEST_MOUNT" $FORWARD orch-dev-harness codex "$@"
"#;

fn rig_with(program: &str, docker_options: &str, variables: Value, config: CodexConfig) -> Rig {
    let outer = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let made = |name: &str| {
        let path = outer.path().join(name);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::canonicalize(path).unwrap()
    };
    let (data, tools, work) = (made("data"), made("tools"), made("work"));
    std::fs::create_dir_all(tools.join("codex")).unwrap();
    let path = tools.join("codex/codex");
    std::fs::write(&path, program).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let prefix = format!(
        "{CONTAINER_PREFIX}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    );
    let mut all = json!({
        "ORCH_TEST_CONTAINER": prefix,
        "ORCH_TEST_MOUNT": outer.path().canonicalize().unwrap(),
        "ORCH_TEST_DOCKER_OPTS": docker_options,
        "ORCH_TEST_FORWARD": "PROJECT_NAME",
    });
    for (name, value) in variables.as_object().unwrap() {
        all[name] = value.clone();
    }
    let env: EnvironmentContext = serde_json::from_value(json!({
        "environment_id": "0199abcd-1234-7000-8000-0000000000e1", "generation": 1,
        "data_dir": data, "workspace_root": work, "home": null, "tools_dir": tools,
        "tools": [{ "name": "codex", "version": "0.162.1" }], "mounts": [], "user": "agent",
        "shell": { "name": "sh", "path": "/bin/sh" }, "os": "linux", "arch": "x86_64",
        "path_style": "posix", "limits": {}, "variables": all, "mcp_servers_enabled": false
    }))
    .unwrap();
    Rig {
        outer,
        env,
        exec: CodexExecServer::with_config(config),
        _containers: Containers(prefix.clone()),
        prefix,
    }
}

fn docker_rig(docker_options: &str) -> Rig {
    rig_with(
        DOCKER_PROGRAM,
        docker_options,
        json!({ "PROJECT_NAME": "web" }),
        config(),
    )
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Connects to the server with the token, or without one.
async fn connect(endpoint: &ExecutorEndpoint, token: Option<&str>) -> Result<Socket, String> {
    let mut request = endpoint.address.as_str().into_client_request().unwrap();
    if let Some(token) = token {
        request.headers_mut().insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
    }
    tokio_tungstenite::connect_async(request)
        .await
        .map(|(socket, _)| socket)
        .map_err(|error| error.to_string())
}

fn token_of(endpoint: &ExecutorEndpoint) -> String {
    endpoint.token.as_ref().unwrap().expose().to_owned()
}

/// Sends one request and reads until its answer, keeping the notifications that came before it.
async fn ask(socket: &mut Socket, id: u64, method: &str, params: Value) -> (Value, Vec<Value>) {
    socket
        .send(Message::text(
            json!({ "id": id, "method": method, "params": params }).to_string(),
        ))
        .await
        .unwrap();
    let mut notes = Vec::new();
    loop {
        let message = tokio::time::timeout(Duration::from_secs(30), socket.next())
            .await
            .expect("the server answers")
            .expect("the connection stays")
            .expect("a message");
        let Message::Text(text) = message else {
            continue;
        };
        let value: Value = serde_json::from_str(text.as_str()).unwrap();
        if value["id"] == json!(id) {
            return (value, notes);
        }
        notes.push(value);
    }
}

/// The first thing a client does: initialize, which the server wants before any other request.
async fn initialize(socket: &mut Socket) -> Value {
    let (answer, _) = ask(socket, 1, "initialize", json!({ "clientName": "test" })).await;
    socket
        .send(Message::text(
            json!({ "method": "initialized", "params": {} }).to_string(),
        ))
        .await
        .unwrap();
    answer
}

/// Runs a command through the server and gives what it wrote to standard output.
async fn run_through(
    socket: &mut Socket,
    id: u64,
    script: &str,
    cwd: &Path,
    env: Value,
    metadata: Option<Value>,
) -> String {
    let mut params = json!({
        "processId": format!("p{id}"), "argv": ["sh", "-c", script], "cwd": format!("file://{}", cwd.display()),
        "env": env, "tty": false, "arg0": null,
        "envPolicy": { "inherit": "all", "ignoreDefaultExcludes": true, "exclude": [], "set": {}, "includeOnly": [] },
    });
    if let Some(metadata) = metadata {
        params["metadata"] = metadata;
    }
    let (started, mut notes) = ask(socket, id, "process/start", params).await;
    assert!(started.get("error").is_none(), "{started}");
    let mut out = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        for note in notes.drain(..) {
            if note["method"] == "process/output" && note["params"]["stream"] == "stdout" {
                let chunk = note["params"]["chunk"].as_str().unwrap();
                out.extend(
                    base64::engine::general_purpose::STANDARD
                        .decode(chunk)
                        .unwrap(),
                );
            }
            if note["method"] == "process/closed" {
                return String::from_utf8_lossy(&out).into_owned();
            }
        }
        assert!(Instant::now() < deadline, "the command did not end");
        let message = tokio::time::timeout(Duration::from_secs(30), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if let Message::Text(text) = message {
            notes.push(serde_json::from_str(text.as_str()).unwrap());
        }
    }
}

fn env_of(output: &str) -> std::collections::BTreeMap<String, String> {
    output
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect()
}

#[tokio::test]
async fn an_environment_without_a_build_of_the_tool_says_why_the_executor_cannot_start() {
    let mut rig = rig_with("#!/bin/sh\n", "", json!({}), config());
    let mut env = rig.env.clone();
    env.tools.clear();
    let report = rig.exec.available(&env).await;
    assert!(!report.available && report.sandbox_modes.is_empty());
    assert!(report.reason.unwrap().contains("holds no build of codex"));
    let refused = rig.exec.start(&env).await.unwrap_err();
    assert_eq!(refused.code(), "harness_unavailable");
    let mut no_program = rig.env.clone();
    std::fs::remove_file(rig.outer.path().join("tools/codex/codex")).unwrap();
    no_program.tools_dir = rig.env.tools_dir.clone();
    let report = rig.exec.available(&no_program).await;
    assert!(report.reason.unwrap().contains("has no program in it"));
}

/// A program that records how it was started and then does what `body` says.
fn recording_program(record: &Path, body: &str) -> String {
    format!(
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'codex-cli 9.9.9'; exit 0; fi\n\
         if [ \"$1\" = \"sandbox\" ]; then exit 1; fi\n\
         echo \"$@\" > '{rec}/args.txt'\nenv > '{rec}/env.txt'\n{body}\n",
        rec = record.display()
    )
}

#[tokio::test]
async fn the_server_is_started_on_the_loopback_address_with_a_token_and_a_clean_environment_of_its_own()
 {
    let record = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    // A server that announces an address and stays.
    let program = recording_program(record.path(), "echo ws://127.0.0.1:41999; sleep 30");
    let mut rig = rig_with(
        &program,
        "",
        json!({ "PROJECT_NAME": "web", "CODEX_HOME": "somewhere else" }),
        CodexConfig {
            start_timeout: Duration::from_secs(5),
            handshake_timeout: Duration::from_millis(500),
            ..config()
        },
    );
    let refused = rig.exec.start(&rig.env.clone()).await.unwrap_err();
    assert!(
        refused.to_string().contains("handshake"),
        "a fake server does not speak the protocol: {refused}"
    );
    let args = std::fs::read_to_string(record.path().join("args.txt")).unwrap();
    let words: Vec<&str> = args.split_whitespace().collect();
    assert_eq!(words[0], "exec-server");
    let listen = words.iter().position(|w| *w == "--listen").unwrap();
    let port: u16 = words[listen + 1]
        .strip_prefix("ws://127.0.0.1:")
        .expect("only the loopback address")
        .parse()
        .unwrap();
    assert!(
        (41000..=41999).contains(&port),
        "a port of the range given: {port}"
    );
    assert!(args.contains("--ws-auth capability-token"), "{args}");
    let digest = words[words
        .iter()
        .position(|w| *w == "--ws-token-sha256")
        .unwrap()
        + 1];
    assert!(
        digest.len() == 64 && digest.chars().all(|c| c.is_ascii_hexdigit()),
        "{digest}"
    );
    assert!(
        !words.contains(&"--exit-on-stdin-close"),
        "that flag belongs to the vendor's remote mode and the server refuses to start with it: {args}"
    );

    let env = std::fs::read_to_string(record.path().join("env.txt")).unwrap();
    let vars = env_of(&env);
    assert_eq!(vars.get("PROJECT_NAME").map(String::as_str), Some("web"));
    assert_eq!(
        vars.get("CODEX_HOME").map(PathBuf::from),
        Some(
            rig.outer
                .path()
                .canonicalize()
                .unwrap()
                .join("data/codex-home")
        ),
        "the program's home is the executor's own, whatever the environment's variables say"
    );
    assert!(vars.contains_key("PATH"));
    for name in vars.keys() {
        assert!(
            !name.starts_with("CARGO") && !name.starts_with("RUST") && name != "OLDPWD",
            "{name} comes from the executor's own environment and has no business in the server's"
        );
    }
}

#[tokio::test]
async fn a_server_that_never_says_where_it_listens_or_says_somewhere_else_is_given_up_on() {
    let records = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let no_ports = || CodexConfig {
        ports: None,
        start_timeout: Duration::from_millis(600),
        ..config()
    };
    let mut silent = rig_with(
        &recording_program(records.path(), "sleep 30"),
        "",
        json!({}),
        no_ports(),
    );
    let began = Instant::now();
    let refused = silent.exec.start(&silent.env.clone()).await.unwrap_err();
    assert!(
        refused.to_string().contains("announced no address"),
        "{refused}"
    );
    assert!(began.elapsed() < Duration::from_secs(10));

    let mut other = rig_with(
        &recording_program(records.path(), "echo ws://0.0.0.0:41999; sleep 30"),
        "",
        json!({}),
        no_ports(),
    );
    let refused = other.exec.start(&other.env.clone()).await.unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("not an address on the loopback"),
        "{refused}"
    );

    let mut dead = rig_with(
        &recording_program(
            records.path(),
            "echo 'Error: address already in use' >&2; exit 1",
        ),
        "",
        json!({}),
        no_ports(),
    );
    let refused = dead.exec.start(&dead.env.clone()).await.unwrap_err();
    assert!(
        refused.to_string().contains("address already in use"),
        "the server's own words: {refused}"
    );
}

/// A process that is started here, in a group of its own, with `words` after `sh -c 'sleep 300'`.
fn leftover(words: &[&str]) -> std::process::Child {
    use std::os::unix::process::CommandExt as _;
    StdCommand::new("sh")
        .args(["-c", "sleep 300"])
        .args(words)
        .process_group(0)
        .spawn()
        .unwrap()
}

async fn ended(child: &mut std::process::Child) -> bool {
    for _ in 0..200 {
        if child.try_wait().unwrap().is_some() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

#[tokio::test]
async fn a_server_left_behind_by_a_node_that_died_is_ended_at_the_next_start_and_nothing_else_is() {
    let records = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let digest = "ab".repeat(32);
    let mut rig = rig_with(
        &recording_program(records.path(), "echo ws://127.0.0.1:41999; sleep 30"),
        "",
        json!({}),
        CodexConfig {
            handshake_timeout: Duration::from_millis(300),
            ..config()
        },
    );
    let pid_file = rig.outer.path().join("data/exec-server.pid");
    let same_digest = ["exec-server", "--ws-token-sha256", digest.as_str()];

    // A process that is that server, by its command line: ended.
    let mut ours = leftover(&same_digest);
    std::fs::write(&pid_file, format!("{}\n{digest}\n", ours.id())).unwrap();
    let _ = rig.exec.start(&rig.env.clone()).await;
    assert!(
        ended(&mut ours).await,
        "the server of the node that died is still there"
    );

    // Processes the file names that are not that server are left alone: another digest, no `exec-server`,
    // a file that is no file of ours, and a process that is not there.
    let other_digest = "cd".repeat(32);
    let mut with_other_digest =
        leftover(&["exec-server", "--ws-token-sha256", other_digest.as_str()]);
    let mut unrelated = leftover(&[]);
    for (name, child) in [
        ("another token", &mut with_other_digest),
        ("another program", &mut unrelated),
    ] {
        std::fs::write(&pid_file, format!("{}\n{digest}\n", child.id())).unwrap();
        let _ = rig.exec.start(&rig.env.clone()).await;
        assert!(
            child.try_wait().unwrap().is_none(),
            "{name} was ended, and was not the server"
        );
        assert!(!pid_file.exists() || std::fs::read_to_string(&pid_file).is_ok());
    }
    for text in ["not a pid file", "", "99999999\nff\n", "-5\nab\n"] {
        std::fs::write(&pid_file, text).unwrap();
        let _ = rig.exec.start(&rig.env.clone()).await;
    }
    for mut child in [with_other_digest, unrelated] {
        child.kill().unwrap();
        child.wait().unwrap();
    }
}

#[tokio::test]
async fn the_vendors_server_is_started_in_docker_with_a_token_and_says_which_build_it_is() {
    if !docker_ready() {
        return;
    }
    let mut rig = docker_rig("");
    let env = rig.env.clone();
    let endpoint = rig.exec.start(&env).await.unwrap();
    let port = endpoint.port().unwrap();
    assert!((41000..=41999).contains(&port), "{port}");
    assert!(endpoint.address.starts_with("ws://127.0.0.1:"));
    assert!(endpoint.tool_sets.is_empty());
    let version = semver::Version::parse(&endpoint.version).unwrap();
    assert!(version.major == 0 && version.minor >= 160, "{version}");
    assert_eq!(rig.exec.endpoint(), Some(&endpoint));
    let token = token_of(&endpoint);

    let mut socket = connect(&endpoint, Some(&token)).await.unwrap();
    let (answer, _) = ask(
        &mut socket,
        1,
        "initialize",
        json!({ "clientName": "test" }),
    )
    .await;
    assert_eq!(
        answer["result"]["environmentInfo"]["executorVersion"],
        json!(endpoint.version)
    );
    drop(socket);
    let refused = connect(&endpoint, None).await.unwrap_err();
    assert!(refused.contains("401"), "no token: {refused}");
    let refused = connect(&endpoint, Some("not-the-token")).await.unwrap_err();
    assert!(refused.contains("401"), "another token: {refused}");

    let report = rig.exec.available(&env).await;
    assert!(report.available);
    assert_eq!(report.version.as_deref(), Some(endpoint.version.as_str()));
    assert_eq!(
        rig.exec.start(&env).await.unwrap(),
        endpoint,
        "a second start answers where it is"
    );
    rig.exec.stop().await.unwrap();
}

#[tokio::test]
async fn a_command_through_the_server_carries_the_environments_variables_and_the_clients_thread_id()
{
    if !docker_ready() {
        return;
    }
    let mut rig = docker_rig("");
    let env = rig.env.clone();
    let endpoint = rig.exec.start(&env).await.unwrap();
    let mut socket = connect(&endpoint, Some(&token_of(&endpoint)))
        .await
        .unwrap();
    initialize(&mut socket).await;
    let work = rig.outer.path().canonicalize().unwrap().join("work");
    // The client puts the thread's id in the environment it sends; the environment's variables are the server's.
    let thread = "01a12adc-c412-76a2-912d-e6331c0808e9";
    let out = run_through(
        &mut socket,
        2,
        "env",
        &work,
        json!({ "CODEX_THREAD_ID": thread }),
        None,
    )
    .await;
    let vars = env_of(&out);
    assert_eq!(
        vars.get("PROJECT_NAME").map(String::as_str),
        Some("web"),
        "{out}"
    );
    let attributed = descriptor::attribution().read(&vars);
    assert_eq!(attributed.thread_id.as_deref(), Some(thread));
    assert_eq!(attributed.tool_call_id, None);
    assert_eq!(
        vars.get("CODEX_HOME").map(String::as_str),
        Some(
            rig.outer
                .path()
                .canonicalize()
                .unwrap()
                .join("data/codex-home")
                .to_str()
                .unwrap()
        ),
        "the server's home is the executor's own"
    );

    // What the build checked here does with the ids a client names in `metadata`: nothing. If a newer build
    // starts to give `CODEX_TOOL_CALL_ID`, this is where to find out, and the attribution can use it.
    let metadata = json!({ "threadId": thread, "toolCallId": "call-1" });
    let out = run_through(&mut socket, 3, "env", &work, json!({}), Some(metadata)).await;
    let vars = env_of(&out);
    assert!(
        !vars.contains_key("CODEX_TOOL_CALL_ID") && !vars.contains_key("CODEX_THREAD_ID"),
        "this build gives no ids of its own from the metadata: {out}"
    );
    drop(socket);
    rig.exec.stop().await.unwrap();
}

#[tokio::test]
async fn stopping_the_executor_ends_the_server_and_what_it_started() {
    if !docker_ready() {
        return;
    }
    let mut rig = docker_rig("");
    let env = rig.env.clone();
    let endpoint = rig.exec.start(&env).await.unwrap();
    let mut socket = connect(&endpoint, Some(&token_of(&endpoint)))
        .await
        .unwrap();
    initialize(&mut socket).await;
    let work = rig.outer.path().canonicalize().unwrap().join("work");
    let (started, _) = ask(
        &mut socket,
        2,
        "process/start",
        json!({
            "processId": "long", "argv": ["sh", "-c", "sleep 300"], "cwd": format!("file://{}", work.display()),
            "env": {}, "tty": false, "arg0": null
        }),
    )
    .await;
    assert!(started.get("error").is_none(), "{started}");
    assert_eq!(containers_of(&rig.prefix), 1);
    let began = Instant::now();
    rig.exec.stop().await.unwrap();
    assert!(
        began.elapsed() < Duration::from_secs(15),
        "{:?}",
        began.elapsed()
    );
    let mut gone = false;
    for _ in 0..100 {
        if containers_of(&rig.prefix) == 0 {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(gone, "the container of the server is still there");
    assert!(
        connect(&endpoint, Some(&token_of(&endpoint)))
            .await
            .is_err(),
        "the server does not answer any more"
    );
    assert!(rig.exec.endpoint().is_none());
    assert!(
        rig.exec.stop().await.is_ok(),
        "stopping twice is not an error"
    );
}

#[tokio::test]
async fn two_servers_run_at_once_on_one_host_and_share_nothing() {
    if !docker_ready() {
        return;
    }
    let (mut first, mut second) = (docker_rig(""), docker_rig(""));
    let (env_one, env_two) = (first.env.clone(), second.env.clone());
    let (one, two) = tokio::join!(first.exec.start(&env_one), second.exec.start(&env_two));
    let (one, two) = (one.unwrap(), two.unwrap());
    assert_ne!(one.port().unwrap(), two.port().unwrap());
    assert_ne!(token_of(&one), token_of(&two));
    assert!(
        connect(&two, Some(&token_of(&one))).await.is_err(),
        "the token of one opens nothing of the other"
    );
    first.exec.stop().await.unwrap();
    let mut socket = connect(&two, Some(&token_of(&two))).await.unwrap();
    let (answer, _) = ask(
        &mut socket,
        1,
        "initialize",
        json!({ "clientName": "test" }),
    )
    .await;
    assert!(
        answer.get("result").is_some(),
        "stopping one leaves the other serving"
    );
    drop(socket);
    second.exec.stop().await.unwrap();
}

#[tokio::test]
async fn the_sandbox_modes_are_probed_in_a_container_with_and_without_user_namespaces() {
    if !docker_ready() {
        return;
    }
    // A stock container refuses the namespaces the vendor's sandbox needs: only `open` works there.
    let mut stock = docker_rig("");
    let env = stock.env.clone();
    let report = stock.exec.available(&env).await;
    assert!(report.available, "{report:?}");
    assert_eq!(
        report.sandbox_modes,
        [SandboxMode::Open],
        "a stock container"
    );
    let endpoint = stock.exec.start(&env).await.unwrap();
    assert_eq!(endpoint.sandbox_modes, [SandboxMode::Open]);
    stock.exec.stop().await.unwrap();

    // Without the filter on system calls and the profile, the sandbox starts, and keeps what it promises.
    let mut free = docker_rig(SANDBOX_FRIENDLY);
    let env = free.env.clone();
    let report = free.exec.available(&env).await;
    assert_eq!(
        report.sandbox_modes,
        [
            SandboxMode::ReadOnly,
            SandboxMode::Workspace,
            SandboxMode::Open
        ],
        "{report:?}"
    );
    let endpoint = free.exec.start(&env).await.unwrap();
    assert_eq!(endpoint.sandbox_modes, report.sandbox_modes);
    free.exec.stop().await.unwrap();
}
