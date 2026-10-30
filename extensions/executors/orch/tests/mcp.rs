//! Orch's own executor, started the way an exec node starts it, and spoken to by a real MCP client.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use axum::http::{HeaderName, HeaderValue};
use orch_agent_contract::descriptor::SandboxMode;
use orch_executor_contract::context::EnvironmentContext;
use orch_executor_contract::executor::{Executor, ExecutorEndpoint};
use orch_executor_orch::output::OutputLimits;
use orch_executor_orch::scope::{Mount, SESSION_HEADER, SessionScope};
use orch_executor_orch::server::{ExecConfig, OrchExec};
use orch_executor_orch::tools::ToolSet;
use rmcp::ServiceExt as _;
use rmcp::model::{CallToolRequestParams, CallToolResult, RequestMetaObject};
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;

/// A started executor, and the places a session of it works in.
struct Rig {
    outer: tempfile::TempDir,
    exec: OrchExec,
    endpoint: ExecutorEndpoint,
    work: PathBuf,
    elsewhere: PathBuf,
    mount: PathBuf,
}

/// A grace short enough for a test to wait for.
const GRACE: Duration = Duration::from_millis(600);

fn environment(data_dir: &Path, work: &Path, variables: Value, mcp: bool) -> EnvironmentContext {
    serde_json::from_value(json!({
        "environment_id": "0199abcd-1234-7000-8000-0000000000e1", "generation": 1,
        "data_dir": data_dir, "workspace_root": work, "home": null, "tools_dir": null, "tools": [],
        "mounts": [], "user": "agent", "shell": { "name": "sh", "path": "/bin/sh" },
        "os": "linux", "arch": "x86_64", "path_style": "posix", "limits": {},
        "variables": variables, "mcp_servers_enabled": mcp
    }))
    .unwrap()
}

fn config() -> ExecConfig {
    ExecConfig {
        stop_grace: GRACE,
        // The ports this machine lets a test listen on.
        ports: Some(41000..=41999),
        keep_alive: Duration::from_millis(100),
        ..ExecConfig::default()
    }
}

async fn rig_with(config: ExecConfig, variables: Value, mcp_servers: bool) -> Rig {
    let outer = tempfile::tempdir().unwrap();
    let make = |name: &str| {
        let path = outer.path().join(name);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::canonicalize(path).unwrap()
    };
    let (data, work, elsewhere, mount) =
        (make("data"), make("work"), make("elsewhere"), make("mount"));
    let mut exec = OrchExec::with_config(config);
    let endpoint = exec
        .start(&environment(&data, &work, variables, mcp_servers))
        .await
        .unwrap();
    Rig {
        outer,
        exec,
        endpoint,
        work,
        elsewhere,
        mount,
    }
}

async fn rig() -> Rig {
    rig_with(config(), json!({}), false).await
}

fn scope(rig: &Rig, set: ToolSet, mode: SandboxMode) -> SessionScope {
    SessionScope {
        thread_id: "thread-a".to_owned(),
        tool_set: set,
        directory: rig.work.display().to_string(),
        cwd: None,
        sandbox_mode: mode,
        mounts: vec![],
    }
}

type Client = RunningService<RoleClient, ()>;

async fn connect(rig: &Rig, scope: &SessionScope) -> Client {
    let mut headers = HashMap::new();
    headers.insert(
        HeaderName::from_static(SESSION_HEADER),
        HeaderValue::from_str(&scope.encode()).unwrap(),
    );
    let config = StreamableHttpClientTransportConfig::with_uri(rig.endpoint.address.clone())
        .auth_header(rig.endpoint.token.as_ref().unwrap().expose())
        .custom_headers(headers);
    ().serve(StreamableHttpClientTransport::from_config(config))
        .await
        .unwrap()
}

async fn call(client: &Client, tool: &str, arguments: Value) -> CallToolResult {
    call_with_meta(client, tool, arguments, None).await
}

async fn call_with_meta(
    client: &Client,
    tool: &str,
    arguments: Value,
    tool_use_id: Option<&str>,
) -> CallToolResult {
    let Value::Object(arguments) = arguments else {
        panic!("the arguments of a call are an object");
    };
    let mut params = CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments);
    if let Some(id) = tool_use_id {
        let mut meta = RequestMetaObject::new();
        meta.insert("claudecode/toolUseId".to_owned(), json!(id));
        params.meta = Some(meta);
    }
    client.call_tool(params).await.unwrap()
}

fn text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| block.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_error(result: &CallToolResult) -> bool {
    result.is_error == Some(true)
}

fn code_of(result: &CallToolResult) -> String {
    assert!(is_error(result), "an error was expected: {}", text(result));
    result.structured_content.as_ref().unwrap()["error"]["code"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn structured(result: &CallToolResult) -> &Value {
    assert!(!is_error(result), "{}", text(result));
    result.structured_content.as_ref().unwrap()
}

/// Waits until `check` is true, for at most ten seconds.
async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let started = Instant::now();
    while !check() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "gave up waiting for: {what}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[cfg(unix)]
fn process_exists(pid_file: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(pid_file) else {
        return false;
    };
    let Ok(pid) = text.trim().parse::<i32>() else {
        return false;
    };
    rustix::process::Pid::from_raw(pid)
        .is_some_and(|pid| rustix::process::test_kill_process(pid).is_ok())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_plain_set_lists_eight_tools_with_titles_and_hints_and_inputs_that_refuse_unknown_members()
 {
    let mut rig = rig().await;
    let client = connect(&rig, &scope(&rig, ToolSet::Plain, SandboxMode::Open)).await;
    let tools = client.list_all_tools().await.unwrap();
    let names: Vec<&str> = tools.iter().map(|tool| &*tool.name).collect();
    assert_eq!(
        names,
        [
            "shell",
            "shell_output",
            "shell_stop",
            "read",
            "write",
            "edit",
            "list",
            "search"
        ]
    );
    for tool in &tools {
        assert!(
            tool.title.is_some() && tool.description.is_some(),
            "{}",
            tool.name
        );
        assert_eq!(
            tool.input_schema["additionalProperties"],
            json!(false),
            "{}",
            tool.name
        );
        assert!(tool.annotations.is_some(), "{} has hints", tool.name);
    }
    let read_only: Vec<&str> = tools
        .iter()
        .filter(|tool| tool.annotations.as_ref().and_then(|a| a.read_only_hint) == Some(true))
        .map(|tool| &*tool.name)
        .collect();
    assert_eq!(read_only, ["shell_output", "read", "list", "search"]);
    let shell = tools.iter().find(|tool| tool.name == "shell").unwrap();
    assert_eq!(shell.input_schema["required"], json!(["command"]));
    for member in ["command", "cwd", "timeout_ms", "background"] {
        assert!(
            shell.input_schema["properties"].get(member).is_some(),
            "{member}"
        );
    }
    rig.exec.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_claude_code_set_lists_seven_tools_by_the_names_and_inputs_of_the_harness_and_custom_lists_none()
 {
    let mut rig = rig_with(config(), json!({}), true).await;
    let client = connect(&rig, &scope(&rig, ToolSet::ClaudeCode, SandboxMode::Open)).await;
    let tools = client.list_all_tools().await.unwrap();
    let names: Vec<&str> = tools.iter().map(|tool| &*tool.name).collect();
    assert_eq!(
        names,
        ["Bash", "Read", "Write", "Edit", "Glob", "Grep", "TaskStop"]
    );
    let members = |tool: &str| -> Vec<String> {
        let tool = tools.iter().find(|t| t.name == tool).unwrap();
        let mut members: Vec<String> = tool.input_schema["properties"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        members.sort();
        members
    };
    assert_eq!(
        members("Bash"),
        ["command", "description", "run_in_background", "timeout"]
    );
    assert_eq!(members("Read"), ["file_path", "limit", "offset"]);
    assert_eq!(members("Write"), ["content", "file_path"]);
    assert_eq!(
        members("Edit"),
        ["file_path", "new_string", "old_string", "replace_all"]
    );
    assert_eq!(members("Glob"), ["path", "pattern"]);
    assert_eq!(
        members("Grep"),
        [
            "-A",
            "-B",
            "-C",
            "-i",
            "-n",
            "glob",
            "head_limit",
            "multiline",
            "offset",
            "output_mode",
            "path",
            "pattern",
            "type"
        ]
    );
    assert_eq!(members("TaskStop"), ["task_id"]);

    let custom = connect(&rig, &scope(&rig, ToolSet::Custom, SandboxMode::Open)).await;
    assert!(custom.list_all_tools().await.unwrap().is_empty());
    let refused = call(&custom, "shell", json!({ "command": "echo no" })).await;
    assert_eq!(code_of(&refused), "not_found");
    assert_eq!(
        rig.endpoint.tool_sets,
        ["claude-code", "plain", "custom"],
        "custom is offered while an MCP server is enabled"
    );
    rig.exec.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_endpoint_says_where_the_executor_is_what_it_is_and_what_it_offers() {
    let mut rig = rig().await;
    let endpoint = rig.endpoint.clone();
    let port = endpoint.port().unwrap();
    assert!((41000..=41999).contains(&port), "{port}");
    assert!(endpoint.address.starts_with("http://127.0.0.1:"));
    assert!(endpoint.token.is_some());
    assert_eq!(endpoint.tool_sets, ["claude-code", "plain"]);
    assert_eq!(endpoint.sandbox_modes, [SandboxMode::Open]);
    assert_eq!(rig.exec.endpoint(), Some(&endpoint));
    assert!(!format!("{endpoint:?}").contains(endpoint.token.as_ref().unwrap().expose()));
    rig.exec.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_plain_tool_does_what_it_says() {
    let mut rig = rig().await;
    let client = connect(&rig, &scope(&rig, ToolSet::Plain, SandboxMode::Workspace)).await;

    let ran = call(
        &client,
        "shell",
        json!({ "command": "echo out; echo err >&2; exit 3" }),
    )
    .await;
    assert_eq!(
        structured(&ran),
        &json!({ "exit_code": 3, "stdout": "out\n", "stderr": "err\n", "truncated": false })
    );

    let written = call(
        &client,
        "write",
        json!({ "path": "notes/a.txt", "content": "alpha\nbeta\nalpha\n" }),
    )
    .await;
    assert_eq!(structured(&written), &json!({ "bytes": 17 }));
    let read = call(&client, "read", json!({ "path": "notes/a.txt" })).await;
    assert_eq!(
        text(&read),
        "     1\u{2192}alpha\n     2\u{2192}beta\n     3\u{2192}alpha\n"
    );
    let part = call(
        &client,
        "read",
        json!({ "path": "notes/a.txt", "offset": 2, "limit": 1 }),
    )
    .await;
    assert!(text(&part).starts_with("     2\u{2192}beta\n"));

    let ambiguous = call(
        &client,
        "edit",
        json!({ "path": "notes/a.txt", "old": "alpha", "new": "omega" }),
    )
    .await;
    assert_eq!(code_of(&ambiguous), "ambiguous");
    let edited = call(
        &client,
        "edit",
        json!({ "path": "notes/a.txt", "old": "alpha", "new": "omega", "all": true }),
    )
    .await;
    assert_eq!(structured(&edited), &json!({ "replaced": 2 }));
    assert_eq!(
        std::fs::read_to_string(rig.work.join("notes/a.txt")).unwrap(),
        "omega\nbeta\nomega\n"
    );

    let listed = call(&client, "list", json!({ "path": "." })).await;
    assert_eq!(
        structured(&listed)["entries"],
        json!([{ "path": "notes/", "kind": "dir" }])
    );
    let deep = call(&client, "list", json!({ "path": ".", "depth": 2 })).await;
    assert_eq!(
        structured(&deep)["entries"][1],
        json!({ "path": "notes/a.txt", "kind": "file", "size": 17 })
    );

    let found = call(
        &client,
        "search",
        json!({ "pattern": "omega", "path": "notes" }),
    )
    .await;
    assert_eq!(text(&found), "notes/a.txt:1: omega\nnotes/a.txt:3: omega\n");
    let none = call(&client, "search", json!({ "pattern": "zzz" })).await;
    assert_eq!(text(&none), "No lines match.");
    rig.exec.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_background_command_is_read_and_stopped_by_its_id() {
    let mut rig = rig().await;
    let client = connect(&rig, &scope(&rig, ToolSet::Plain, SandboxMode::Workspace)).await;
    let started = call(
        &client,
        "shell",
        json!({ "command": "echo first; sleep 0.3; echo second; sleep 60", "background": true }),
    )
    .await;
    let id = structured(&started)["process_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut seen = String::new();
    let began = Instant::now();
    while !seen.contains("second") {
        let progress = call(
            &client,
            "shell_output",
            json!({ "process_id": id, "wait_ms": 2000 }),
        )
        .await;
        let answer = structured(&progress);
        assert_eq!(answer["running"], json!(true));
        seen.push_str(answer["output"].as_str().unwrap());
        assert!(began.elapsed() < Duration::from_secs(10), "{seen}");
    }
    assert_eq!(seen, "first\nsecond\n");

    let stopped_at = Instant::now();
    let stopped = call(&client, "shell_stop", json!({ "process_id": id })).await;
    assert_eq!(
        structured(&stopped),
        &json!({ "exit_code": 143 }),
        "TERM ends a shell with 128+15"
    );
    assert!(stopped_at.elapsed() < Duration::from_secs(5));
    let after = call(&client, "shell_output", json!({ "process_id": id })).await;
    assert_eq!(structured(&after)["running"], json!(false));
    assert_eq!(structured(&after)["exit_code"], json!(143));
    let unknown = call(&client, "shell_stop", json!({ "process_id": "cmd_nope" })).await;
    assert_eq!(code_of(&unknown), "no_such_process");
    rig.exec.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_command_that_ignores_term_is_killed_when_the_grace_is_over() {
    let mut rig = rig().await;
    let client = connect(&rig, &scope(&rig, ToolSet::Plain, SandboxMode::Open)).await;
    let started = call(
        &client,
        "shell",
        json!({ "command": "trap '' TERM; while true; do sleep 0.1; done", "background": true }),
    )
    .await;
    let id = structured(&started)["process_id"]
        .as_str()
        .unwrap()
        .to_owned();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let began = Instant::now();
    let stopped = call(&client, "shell_stop", json!({ "process_id": id })).await;
    assert_eq!(
        structured(&stopped),
        &json!({ "exit_code": 137 }),
        "KILL is 128+9"
    );
    assert!(began.elapsed() >= GRACE, "{:?}", began.elapsed());
    assert!(began.elapsed() < GRACE + Duration::from_secs(5));
    rig.exec.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_foreground_command_is_ended_when_its_time_is_up() {
    let mut rig = rig().await;
    let client = connect(&rig, &scope(&rig, ToolSet::Plain, SandboxMode::Open)).await;
    let ran = call(
        &client,
        "shell",
        json!({ "command": "echo begun; sleep 60", "timeout_ms": 300 }),
    )
    .await;
    let answer = structured(&ran);
    assert_eq!(answer["exit_code"], json!(143));
    assert_eq!(answer["stdout"], json!("begun\n"));
    assert!(
        answer["note"].as_str().unwrap().contains("300 ms"),
        "{answer}"
    );
    let too_long = call(
        &client,
        "shell",
        json!({ "command": "true", "timeout_ms": 3_600_001 }),
    )
    .await;
    assert_eq!(code_of(&too_long), "invalid_input");
    let none = call(
        &client,
        "shell",
        json!({ "command": "true", "timeout_ms": 0 }),
    )
    .await;
    assert_eq!(code_of(&none), "invalid_input");
    rig.exec.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn output_over_the_bound_is_cut_in_the_middle_and_the_rest_is_kept_where_the_answer_says() {
    let config = ExecConfig {
        limits: OutputLimits {
            inline_bytes: 200,
            kept_bytes: 1024 * 1024,
            tail_bytes: 256 * 1024,
        },
        ..config()
    };
    let mut rig = rig_with(config, json!({}), false).await;
    let client = connect(&rig, &scope(&rig, ToolSet::Plain, SandboxMode::Workspace)).await;
    let ran = call(
        &client,
        "shell",
        json!({ "command": "i=0; while [ $i -lt 300 ]; do echo line-$i; i=$((i+1)); done" }),
    )
    .await;
    let answer = structured(&ran);
    assert_eq!(answer["truncated"], json!(true));
    let shown = answer["stdout"].as_str().unwrap();
    assert!(shown.starts_with("line-0\n") && shown.trim_end().ends_with("line-299"));
    assert!(
        shown.contains("bytes omitted") && shown.len() < 400,
        "{shown}"
    );
    let file = answer["output_file"].as_str().unwrap().to_owned();
    assert!(Path::new(&file).starts_with(rig.outer.path().join("data")));
    // The kept output is read with the tools the session has, though it lies outside its directory.
    let read = call(
        &client,
        "read",
        json!({ "path": file, "offset": 299, "limit": 5 }),
    )
    .await;
    assert!(
        text(&read).contains("line-298") && text(&read).contains("line-299"),
        "{}",
        text(&read)
    );
    let write = call(&client, "write", json!({ "path": file, "content": "mine" })).await;
    assert_eq!(
        code_of(&write),
        "read_only",
        "the kept output is never written"
    );
    rig.exec.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_path_outside_the_directory_is_refused_by_every_file_tool_in_workspace_and_read_only() {
    let mut rig = rig().await;
    let secret = rig.elsewhere.join("secret.txt");
    std::fs::write(&secret, "hidden\n").unwrap();
    std::fs::write(rig.work.join("inside.txt"), "visible\n").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&rig.elsewhere, rig.work.join("door")).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&rig.elsewhere, rig.work.join("door")).unwrap();
    let outside = secret.display().to_string();
    for mode in [SandboxMode::Workspace, SandboxMode::ReadOnly] {
        let plain = connect(&rig, &scope(&rig, ToolSet::Plain, mode)).await;
        let claude = connect(&rig, &scope(&rig, ToolSet::ClaudeCode, mode)).await;
        for (client, calls) in [
            (
                &plain,
                vec![
                    ("read", json!({ "path": outside })),
                    ("read", json!({ "path": "../elsewhere/secret.txt" })),
                    ("read", json!({ "path": "door/secret.txt" })),
                    ("write", json!({ "path": outside, "content": "x" })),
                    (
                        "edit",
                        json!({ "path": outside, "old": "hidden", "new": "x" }),
                    ),
                    (
                        "list",
                        json!({ "path": rig.elsewhere.display().to_string() }),
                    ),
                    ("list", json!({ "path": "door" })),
                    (
                        "search",
                        json!({ "pattern": "hidden", "path": rig.elsewhere.display().to_string() }),
                    ),
                ],
            ),
            (
                &claude,
                vec![
                    ("Read", json!({ "file_path": outside })),
                    ("Read", json!({ "file_path": "../elsewhere/secret.txt" })),
                    ("Write", json!({ "file_path": outside, "content": "x" })),
                    (
                        "Edit",
                        json!({ "file_path": outside, "old_string": "hidden", "new_string": "x" }),
                    ),
                    (
                        "Glob",
                        json!({ "pattern": "*", "path": rig.elsewhere.display().to_string() }),
                    ),
                    (
                        "Grep",
                        json!({ "pattern": "hidden", "path": rig.elsewhere.display().to_string() }),
                    ),
                    ("Grep", json!({ "pattern": "hidden", "path": "door" })),
                ],
            ),
        ] {
            for (tool, arguments) in calls {
                let result = call(client, tool, arguments.clone()).await;
                assert_eq!(
                    code_of(&result),
                    "outside_sandbox",
                    "{tool} {arguments} in {mode:?}: {}",
                    text(&result)
                );
            }
        }
        assert_eq!(
            std::fs::read_to_string(&secret).unwrap(),
            "hidden\n",
            "nothing was written"
        );
        if mode == SandboxMode::Workspace {
            // A command runs where it is told to only inside the places the session reaches.
            let elsewhere = rig.elsewhere.display().to_string();
            let shell = call(
                &plain,
                "shell",
                json!({ "command": "true", "cwd": elsewhere }),
            )
            .await;
            assert_eq!(code_of(&shell), "outside_sandbox");
        }
        assert!(
            text(&call(&plain, "read", json!({ "path": "inside.txt" })).await).contains("visible")
        );
    }
    // With open nothing is refused by the path.
    let open = connect(&rig, &scope(&rig, ToolSet::Plain, SandboxMode::Open)).await;
    let read = call(&open, "read", json!({ "path": outside })).await;
    assert!(text(&read).contains("hidden"));
    rig.exec.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_only_forbids_every_write_and_every_command_and_a_read_only_mount_forbids_the_writes_in_it()
 {
    let mut rig = rig().await;
    std::fs::write(rig.mount.join("lib.txt"), "library\n").unwrap();
    std::fs::write(rig.work.join("own.txt"), "mine\n").unwrap();
    let mut with_mount = scope(&rig, ToolSet::Plain, SandboxMode::Workspace);
    with_mount.mounts = vec![Mount {
        name: "lib".to_owned(),
        path: rig.mount.display().to_string(),
        read_only: true,
    }];
    let client = connect(&rig, &with_mount).await;
    let mount_file = rig.mount.join("lib.txt").display().to_string();
    assert!(text(&call(&client, "read", json!({ "path": mount_file })).await).contains("library"));
    let write = call(
        &client,
        "write",
        json!({ "path": mount_file, "content": "x" }),
    )
    .await;
    assert_eq!(code_of(&write), "read_only");
    assert_eq!(
        std::fs::read_to_string(rig.mount.join("lib.txt")).unwrap(),
        "library\n"
    );

    let mut read_only = scope(&rig, ToolSet::Plain, SandboxMode::ReadOnly);
    read_only.mounts = with_mount.mounts.clone();
    let client = connect(&rig, &read_only).await;
    assert!(text(&call(&client, "read", json!({ "path": "own.txt" })).await).contains("mine"));
    for (tool, arguments) in [
        ("write", json!({ "path": "new.txt", "content": "x" })),
        ("write", json!({ "path": "own.txt", "content": "x" })),
        (
            "edit",
            json!({ "path": "own.txt", "old": "mine", "new": "x" }),
        ),
        ("shell", json!({ "command": "touch made.txt" })),
        (
            "shell",
            json!({ "command": "touch made.txt", "background": true }),
        ),
    ] {
        let result = call(&client, tool, arguments.clone()).await;
        assert_eq!(code_of(&result), "read_only", "{tool} {arguments}");
    }
    assert!(!rig.work.join("new.txt").exists() && !rig.work.join("made.txt").exists());
    assert_eq!(
        std::fs::read_to_string(rig.work.join("own.txt")).unwrap(),
        "mine\n"
    );
    rig.exec.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_command_carries_the_thread_and_the_tool_call_it_belongs_to() {
    let mut rig = rig_with(
        config(),
        json!({ "PROJECT": "web", "ORCH_THREAD_ID": "forged" }),
        false,
    )
    .await;
    let plain = connect(&rig, &scope(&rig, ToolSet::Plain, SandboxMode::Open)).await;
    let claude = connect(&rig, &scope(&rig, ToolSet::ClaudeCode, SandboxMode::Open)).await;
    let named = call_with_meta(
        &claude,
        "Bash",
        json!({ "command": "echo $ORCH_THREAD_ID $ORCH_ITEM_ID $PROJECT" }),
        Some("toolu_01ABC"),
    )
    .await;
    assert_eq!(text(&named), "thread-a toolu_01ABC web\n");
    let unnamed = call(
        &plain,
        "shell",
        json!({ "command": "echo $ORCH_THREAD_ID $ORCH_ITEM_ID" }),
    )
    .await;
    let line = structured(&unnamed)["stdout"]
        .as_str()
        .unwrap()
        .trim_end()
        .to_owned();
    let (thread, item) = line.split_once(' ').unwrap();
    assert_eq!(thread, "thread-a");
    assert!(
        item.starts_with("rpc-"),
        "a harness that names no call gets the request's own id: {item}"
    );
    // A background command is attributed in the same way.
    let started = call_with_meta(
        &claude,
        "Bash",
        json!({ "command": "echo $ORCH_ITEM_ID > item.txt; sleep 60", "run_in_background": true }),
        Some("toolu_02XYZ"),
    )
    .await;
    assert!(text(&started).contains("cmd_"));
    eventually("the background command wrote its id", || {
        rig.work.join("item.txt").exists()
    })
    .await;
    eventually("the file has its content", || {
        std::fs::read_to_string(rig.work.join("item.txt")).is_ok_and(|t| t.trim() == "toolu_02XYZ")
    })
    .await;
    rig.exec.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_thread_cannot_read_or_stop_the_commands_of_another_thread() {
    let mut rig = rig().await;
    let starter = connect(&rig, &scope(&rig, ToolSet::Plain, SandboxMode::Open)).await;
    let mut other_scope = scope(&rig, ToolSet::Plain, SandboxMode::Open);
    other_scope.thread_id = "thread-b".to_owned();
    let intruder = connect(&rig, &other_scope).await;
    let started = call(
        &starter,
        "shell",
        json!({ "command": "sleep 60", "background": true }),
    )
    .await;
    let id = structured(&started)["process_id"]
        .as_str()
        .unwrap()
        .to_owned();
    for tool in ["shell_output", "shell_stop"] {
        let result = call(&intruder, tool, json!({ "process_id": id })).await;
        assert_eq!(code_of(&result), "no_such_process", "{tool}");
    }
    let still = call(&starter, "shell_output", json!({ "process_id": id })).await;
    assert_eq!(structured(&still)["running"], json!(true));
    rig.exec.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_claude_code_tool_answers_in_the_harnesss_form() {
    let mut rig = rig().await;
    let client = connect(
        &rig,
        &scope(&rig, ToolSet::ClaudeCode, SandboxMode::Workspace),
    )
    .await;
    let ok = call(
        &client,
        "Bash",
        json!({ "command": "echo hello", "description": "Say hello" }),
    )
    .await;
    assert_eq!(text(&ok), "hello\n");
    let failed = call(
        &client,
        "Bash",
        json!({ "command": "echo bad >&2; exit 2" }),
    )
    .await;
    assert_eq!(text(&failed), "Exit code 2\nbad\n");
    let silent = call(&client, "Bash", json!({ "command": "true" })).await;
    assert_eq!(text(&silent), "(no output)\n");

    let created = call(
        &client,
        "Write",
        json!({ "file_path": "src/a.rs", "content": "fn a() {}\nfn b() {}\nfn a2() {}\n" }),
    )
    .await;
    assert!(
        text(&created).starts_with("File created successfully at: "),
        "{}",
        text(&created)
    );
    let updated = call(
        &client,
        "Write",
        json!({ "file_path": "src/a.rs", "content": "fn a() {}\nfn b() {}\nfn a2() {}\n" }),
    )
    .await;
    assert!(text(&updated).contains("has been updated"));
    let read = call(
        &client,
        "Read",
        json!({ "file_path": "src/a.rs", "offset": 2, "limit": 1 }),
    )
    .await;
    assert!(
        text(&read).starts_with("     2\u{2192}fn b() {}\n"),
        "{}",
        text(&read)
    );
    let directory = call(&client, "Read", json!({ "file_path": "src" })).await;
    assert_eq!(code_of(&directory), "is_directory");
    assert!(
        text(&directory).contains("List it with a command"),
        "{}",
        text(&directory)
    );

    let ambiguous = call(
        &client,
        "Edit",
        json!({ "file_path": "src/a.rs", "old_string": "fn a", "new_string": "fn z" }),
    )
    .await;
    assert_eq!(code_of(&ambiguous), "ambiguous");
    let edited = call(&client, "Edit", json!({ "file_path": "src/a.rs", "old_string": "fn a", "new_string": "fn z", "replace_all": true })).await;
    assert!(
        text(&edited).contains("All 2 occurrences were replaced"),
        "{}",
        text(&edited)
    );

    let glob = call(&client, "Glob", json!({ "pattern": "**/*.rs" })).await;
    assert_eq!(text(&glob), "src/a.rs\n");
    let files = call(&client, "Grep", json!({ "pattern": "fn z" })).await;
    assert_eq!(text(&files), "Found 1 file\nsrc/a.rs\n");
    let content = call(
        &client,
        "Grep",
        json!({ "pattern": "fn z", "output_mode": "content", "-n": false, "-i": true }),
    )
    .await;
    assert_eq!(text(&content), "src/a.rs:fn z() {}\nsrc/a.rs:fn z2() {}\n");
    let count = call(
        &client,
        "Grep",
        json!({ "pattern": "fn", "output_mode": "count", "glob": "*.rs", "type": "rust" }),
    )
    .await;
    assert!(text(&count).starts_with("src/a.rs:3\n"), "{}", text(&count));
    rig.exec.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_member_that_the_table_does_not_list_is_refused_as_unknown() {
    let mut rig = rig().await;
    let plain = connect(&rig, &scope(&rig, ToolSet::Plain, SandboxMode::Open)).await;
    let claude = connect(&rig, &scope(&rig, ToolSet::ClaudeCode, SandboxMode::Open)).await;
    for (client, tool, arguments) in [
        (
            &plain,
            "shell",
            json!({ "command": "true", "shell": "bash" }),
        ),
        (&plain, "read", json!({ "path": "x", "pages": "1-2" })),
        (
            &claude,
            "Bash",
            json!({ "command": "true", "dangerouslyDisableSandbox": true }),
        ),
        (&claude, "Read", json!({ "file_path": "x", "pages": "1-2" })),
        (&claude, "Grep", json!({ "pattern": "x", "context": 2 })),
        (&claude, "Grep", json!({ "pattern": "x", "-o": true })),
        (
            &claude,
            "TaskStop",
            json!({ "task_id": "x", "shell_id": "y" }),
        ),
    ] {
        let result = call(client, tool, arguments.clone()).await;
        assert_eq!(code_of(&result), "invalid_input", "{tool} {arguments}");
        assert!(text(&result).contains("unknown field"), "{}", text(&result));
    }
    let missing = call(&plain, "write", json!({ "path": "x" })).await;
    assert_eq!(code_of(&missing), "invalid_input");
    let other_sets_tool = call(&claude, "shell", json!({ "command": "true" })).await;
    assert_eq!(
        code_of(&other_sets_tool),
        "not_found",
        "a set offers its own tools only"
    );
    rig.exec.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_directory_a_cd_leaves_is_kept_inside_the_thread_and_set_back_outside_it() {
    let mut rig = rig().await;
    std::fs::create_dir_all(rig.work.join("pkg/inner")).unwrap();
    let mut with_mount = scope(&rig, ToolSet::ClaudeCode, SandboxMode::Open);
    with_mount.mounts = vec![Mount {
        name: "lib".to_owned(),
        path: rig.mount.display().to_string(),
        read_only: false,
    }];
    let client = connect(&rig, &with_mount).await;
    let stay = call(&client, "Bash", json!({ "command": "cd pkg/inner && pwd" })).await;
    assert!(
        text(&stay).trim_end().ends_with("pkg/inner"),
        "{}",
        text(&stay)
    );
    let kept = call(&client, "Bash", json!({ "command": "pwd; export MARK=1" })).await;
    assert!(
        text(&kept).trim_end().ends_with("pkg/inner"),
        "the directory is kept: {}",
        text(&kept)
    );
    let variables = call(
        &client,
        "Bash",
        json!({ "command": "echo mark=${MARK:-none}" }),
    )
    .await;
    assert_eq!(text(&variables), "mark=none\n", "variables are not kept");
    // Relative paths of the file tools start from the session's directory too.
    call(
        &client,
        "Write",
        json!({ "file_path": "here.txt", "content": "x" }),
    )
    .await;
    assert!(rig.work.join("pkg/inner/here.txt").exists());
    let into_mount = call(
        &client,
        "Bash",
        json!({ "command": format!("cd {} && pwd", rig.mount.display()) }),
    )
    .await;
    assert!(text(&into_mount).contains("mount"), "{}", text(&into_mount));
    let still_there = call(&client, "Bash", json!({ "command": "pwd" })).await;
    assert!(
        text(&still_there).trim_end().ends_with("mount"),
        "an attached mount is a place to stay: {}",
        text(&still_there)
    );
    let out = call(
        &client,
        "Bash",
        json!({ "command": format!("cd {} && pwd", rig.elsewhere.display()) }),
    )
    .await;
    assert!(
        text(&out).contains("Shell cwd was reset to"),
        "{}",
        text(&out)
    );
    let back = call(&client, "Bash", json!({ "command": "pwd" })).await;
    assert!(
        text(&back).trim_end().ends_with("mount"),
        "set back to where it was: {}",
        text(&back)
    );
    rig.exec.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_background_bash_command_writes_its_output_to_a_file_that_read_reads_and_task_stop_stops_it()
 {
    let mut rig = rig().await;
    let client = connect(
        &rig,
        &scope(&rig, ToolSet::ClaudeCode, SandboxMode::Workspace),
    )
    .await;
    let started = call(
        &client,
        "Bash",
        json!({ "command": "echo working; sleep 60", "run_in_background": true }),
    )
    .await;
    let message = text(&started);
    let id = message
        .split("ID: ")
        .nth(1)
        .unwrap()
        .split('.')
        .next()
        .unwrap()
        .to_owned();
    let file = message
        .split("written to ")
        .nth(1)
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let mut shown = String::new();
    let began = Instant::now();
    while !shown.contains("working") {
        shown = text(&call(&client, "Read", json!({ "file_path": file })).await);
        assert!(began.elapsed() < Duration::from_secs(10), "{shown}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let stopped = call(&client, "TaskStop", json!({ "task_id": id })).await;
    assert!(
        text(&stopped).contains("exit code 143"),
        "{}",
        text(&stopped)
    );
    let unknown = call(&client, "TaskStop", json!({ "task_id": "cmd_nope" })).await;
    assert_eq!(code_of(&unknown), "no_such_process");
    rig.exec.stop().await.unwrap();
}

/// One HTTP exchange on the loopback address, written by hand: the status and the body, or nothing when no
/// server answered.
async fn exchange(
    rig: &Rig,
    headers: &[(&str, String)],
    host: Option<&str>,
) -> Option<(u16, String)> {
    let port = rig.endpoint.port().unwrap();
    let body = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "protocolVersion": "2025-06-18", "capabilities": {},
                    "clientInfo": { "name": "raw", "version": "0" } }
    })
    .to_string();
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.ok()?;
    let host = host.map_or_else(|| format!("127.0.0.1:{port}"), str::to_owned);
    let mut request = format!(
        "POST /mcp HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(&body);
    stream.write_all(request.as_bytes()).await.ok()?;
    let mut response = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut response)).await;
    let response = String::from_utf8_lossy(&response).into_owned();
    let status = response.split(' ').nth(1)?.parse().ok()?;
    Some((status, response))
}

async fn raw(rig: &Rig, headers: &[(&str, String)], host: Option<&str>) -> (u16, String) {
    exchange(rig, headers, host)
        .await
        .expect("a server answers")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_is_served_only_with_the_executors_token_and_a_session_it_can_read() {
    let mut rig = rig().await;
    let token = rig.endpoint.token.as_ref().unwrap().expose().to_owned();
    let session = scope(&rig, ToolSet::Plain, SandboxMode::Open).encode();
    let bearer = |token: &str| ("authorization", format!("Bearer {token}"));
    let with_session = ("x-orch-session", session.clone());

    let (status, _) = raw(&rig, std::slice::from_ref(&with_session), None).await;
    assert_eq!(status, 401, "no token");
    let (status, _) = raw(&rig, &[bearer("not-the-token"), with_session.clone()], None).await;
    assert_eq!(status, 401, "another token");
    let (status, _) = raw(&rig, &[("authorization", token.clone())], None).await;
    assert_eq!(
        status, 401,
        "a token that is not presented as a bearer token"
    );
    let (status, body) = raw(&rig, &[bearer(&token)], None).await;
    assert_eq!(status, 400, "no session: {body}");
    let (status, _) = raw(
        &rig,
        &[bearer(&token), ("x-orch-session", "!!".to_owned())],
        None,
    )
    .await;
    assert_eq!(status, 400, "a session that is not one");
    let (status, body) = raw(
        &rig,
        &[bearer(&token), with_session.clone()],
        Some("evil.example:80"),
    )
    .await;
    assert_eq!(status, 403, "a Host that is not the machine itself: {body}");
    let (status, body) = raw(&rig, &[bearer(&token), with_session], None).await;
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("orch-exec"), "{body}");
    rig.exec.stop().await.unwrap();
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn when_a_client_goes_away_the_command_of_its_call_is_ended() {
    let mut rig = rig().await;
    let pid_file = rig.work.join("pid.txt");
    let port = rig.endpoint.port().unwrap();
    let token = rig.endpoint.token.as_ref().unwrap().expose().to_owned();
    let session = scope(&rig, ToolSet::Plain, SandboxMode::Open).encode();
    let body = json!({
        "jsonrpc": "2.0", "id": 7, "method": "tools/call",
        "params": { "name": "shell", "arguments": { "command": format!("echo $$ > {}; sleep 60", pid_file.display()) } }
    })
    .to_string();
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nAuthorization: Bearer {token}\r\nx-orch-session: {session}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    eventually("the command runs", || process_exists(&pid_file)).await;
    drop(stream);
    eventually("the command of the vanished client is ended", || {
        !process_exists(&pid_file)
    })
    .await;
    rig.exec.stop().await.unwrap();
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stopping_the_executor_ends_what_it_started_and_closes_its_server() {
    let mut rig = rig().await;
    let client = connect(&rig, &scope(&rig, ToolSet::Plain, SandboxMode::Open)).await;
    let pid_file = rig.work.join("bg.pid");
    let started = call(
        &client,
        "shell",
        json!({ "command": format!("echo $$ > {}; sleep 60", pid_file.display()), "background": true }),
    )
    .await;
    assert!(structured(&started)["process_id"].is_string());
    eventually("the command runs", || process_exists(&pid_file)).await;
    let port = rig.endpoint.port().unwrap();
    let outputs = rig.outer.path().join("data/outputs");
    assert!(outputs.exists());
    rig.exec.stop().await.unwrap();
    assert!(
        !process_exists(&pid_file),
        "the command did not outlive the executor"
    );
    assert!(!outputs.exists(), "what the executor kept is removed");
    // The port is one of a range that other programs use, and one of them may have taken it already: what
    // has to be true is that this executor does not answer any more.
    let session = scope(&rig, ToolSet::Plain, SandboxMode::Open).encode();
    let token = rig.endpoint.token.as_ref().unwrap().expose().to_owned();
    let answer = exchange(
        &rig,
        &[
            ("authorization", format!("Bearer {token}")),
            ("x-orch-session", session),
        ],
        None,
    )
    .await;
    assert!(
        !answer.is_some_and(|(status, body)| status == 200 && body.contains("orch-exec")),
        "the executor still answers on port {port}"
    );
    assert!(
        rig.exec.stop().await.is_ok(),
        "stopping twice is not an error"
    );
    assert!(rig.exec.endpoint().is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_executors_run_side_by_side_and_share_nothing() {
    let mut first = rig().await;
    let mut second = rig().await;
    assert_ne!(
        first.endpoint.port().unwrap(),
        second.endpoint.port().unwrap()
    );
    assert_ne!(first.endpoint.token, second.endpoint.token);
    let one = connect(&first, &scope(&first, ToolSet::Plain, SandboxMode::Open)).await;
    let two = connect(&second, &scope(&second, ToolSet::Plain, SandboxMode::Open)).await;
    let started = call(
        &one,
        "shell",
        json!({ "command": "sleep 60", "background": true }),
    )
    .await;
    let id = structured(&started)["process_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let elsewhere = call(&two, "shell_output", json!({ "process_id": id })).await;
    assert_eq!(
        code_of(&elsewhere),
        "no_such_process",
        "the other executor has no such command"
    );
    // The token of one opens nothing of the other.
    let mut crossed = HashMap::new();
    crossed.insert(
        HeaderName::from_static(SESSION_HEADER),
        HeaderValue::from_str(&scope(&second, ToolSet::Plain, SandboxMode::Open).encode()).unwrap(),
    );
    let config = StreamableHttpClientTransportConfig::with_uri(second.endpoint.address.clone())
        .auth_header(first.endpoint.token.as_ref().unwrap().expose())
        .custom_headers(crossed);
    assert!(
        ().serve(StreamableHttpClientTransport::from_config(config))
            .await
            .is_err()
    );
    first.exec.stop().await.unwrap();
    // Stopping one leaves the other serving.
    let alive = call(&two, "shell", json!({ "command": "echo still" })).await;
    assert_eq!(structured(&alive)["stdout"], json!("still\n"));
    second.exec.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_environment_without_the_shell_says_why_the_executor_cannot_start() {
    let outer = tempfile::tempdir().unwrap();
    let mut env = environment(outer.path(), outer.path(), json!({}), false);
    env.shell.path = outer.path().join("no-such-shell").display().to_string();
    let mut exec = OrchExec::new();
    let availability = exec.available(&env).await;
    assert!(!availability.available);
    assert!(availability.reason.unwrap().contains("shell"));
    assert!(availability.sandbox_modes.is_empty());
    let refused = exec.start(&env).await.unwrap_err();
    assert_eq!(refused.code(), "harness_unavailable");
    env.shell.path = "/bin/sh".to_owned();
    let available = exec.available(&env).await;
    assert!(available.available);
    assert_eq!(available.tool_sets.as_deref().map(<[String]>::len), Some(2));
    let first = exec.start(&env).await.unwrap();
    let again = exec.start(&env).await.unwrap();
    assert_eq!(
        first, again,
        "a second start answers where the executor already is"
    );
    exec.stop().await.unwrap();
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_executor_that_is_dropped_without_being_stopped_ends_its_commands() {
    let rig = rig().await;
    let client = connect(&rig, &scope(&rig, ToolSet::Plain, SandboxMode::Open)).await;
    let pid_file = rig.work.join("dropped.pid");
    call(
        &client,
        "shell",
        json!({ "command": format!("echo $$ > {}; sleep 60", pid_file.display()), "background": true }),
    )
    .await;
    eventually("the command runs", || process_exists(&pid_file)).await;
    let Rig { exec, .. } = rig;
    drop(exec);
    eventually("the command is ended", || !process_exists(&pid_file)).await;
}
