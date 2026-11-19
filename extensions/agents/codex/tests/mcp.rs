//! The MCP servers of a harness's own configuration, found, read and kept out of a session, and the servers a
//! session is given, reached by the real Codex in Docker.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::path::Path;
use std::time::Duration;

use common::Rig;
use orch_agent_contract::adapter::AgentAdapter;
use orch_agent_contract::discovery::{McpImportRequest, McpScope, McpSearch, McpTransport};
use orch_agent_contract::events::OrchTool;
use orch_agent_contract::facts::{Attach, AttachMode};
use orch_agent_contract::session::{ConfigTrust, SessionMode, WorkspaceConfiguration};
use orch_testkit::doubles::model::Verdict;
use orch_types::new_id;

/// The text of a server that is a program: when the harness starts it, it leaves a file in the workspace.
fn marking_server(name: &str, workspace: &Path) -> String {
    format!(
        "[mcp_servers.{name}]\ncommand = \"/bin/sh\"\nargs = [\"-c\", \"touch {0}/started-{name}; sleep 120\"]\n",
        workspace.display()
    )
}

/// Waits until the scripted server has been reached at the address of a server of the session: proof that the
/// harness started the servers of that session.
async fn until_reached(rig: &Rig, key: &str, name: &str) {
    let path = format!("/mcp/{key}/{name}");
    for _ in 0..300 {
        if rig
            .mock
            .requests()
            .iter()
            .any(|request| request.path == path)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the harness did not come to {path}");
}

#[tokio::test]
async fn the_servers_of_a_persons_own_configuration_are_found_read_and_kept_out_of_a_session() {
    let Some(mut rig) = Rig::start("mcp-own").await else {
        return;
    };
    let workspace = rig.workspace_path().to_path_buf();
    let home = rig.home_path().to_path_buf();
    // The person's own home and project: two programs that mark their start, one with a variable that is a
    // secret and one that is not, an address, a project they trust with a server of its own, and a directory
    // inside it with another (which the harness reads on rules of its own, and whose server it started in the
    // same way when it was asked about it directly).
    let nested = workspace.join("nested");
    let mut text = marking_server("marker", &workspace);
    text.push_str(
        "[mcp_servers.marker.env]\nPLAIN_SETTING = \"visible\"\nGITHUB_TOKEN = \"gh-value-1\"\n",
    );
    text.push_str(&marking_server("other", &workspace));
    text.push_str("[mcp_servers.remote]\nurl = \"http://127.0.0.1:9/mcp\"\n");
    text.push_str(&format!(
        "[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
        workspace.display()
    ));
    std::fs::write(home.join("config.toml"), &text).unwrap();
    std::fs::create_dir_all(workspace.join(".codex")).unwrap();
    std::fs::write(
        workspace.join(".codex").join("config.toml"),
        marking_server("project_server", &workspace),
    )
    .unwrap();
    std::fs::create_dir_all(nested.join(".codex")).unwrap();
    std::fs::write(
        nested.join(".codex").join("config.toml"),
        marking_server("nested_server", &workspace),
    )
    .unwrap();

    let mut start = rig.agent_start();
    start.home = None;
    start.profile.attach = Some(Attach {
        mode: AttachMode::Spawn,
        binary: None,
        home: Some(
            orch_agent_contract::facts::EnvironmentDataSource::ExistingDirectory {
                path: home.to_string_lossy().into_owned(),
            },
        ),
    });
    rig.adapter.start(start, rig.sink.clone()).await.unwrap();
    // A person's own home is not written to: what Orch settles is passed as arguments of the launch.
    let launched = rig.launcher.launched.lock().unwrap().clone();
    assert!(
        launched[0].args.iter().any(|arg| arg == "-c"),
        "{:?}",
        launched[0].args
    );
    assert_eq!(
        std::fs::read_to_string(home.join("config.toml")).unwrap(),
        text,
        "the configuration of the person is as it was"
    );

    let directories = vec![
        workspace.to_string_lossy().into_owned(),
        nested.to_string_lossy().into_owned(),
    ];
    let found = rig
        .adapter
        .discover_mcp(McpSearch {
            directories: directories.clone(),
        })
        .await
        .unwrap();
    let find = |name: &str| found.iter().find(|server| server.name == name).unwrap();
    assert_eq!(found.len(), 5, "{found:?}");
    assert_eq!(find("marker").scope, McpScope::User);
    assert_eq!(find("marker").transport, McpTransport::Stdio);
    assert_eq!(find("remote").transport, McpTransport::Http);
    assert_eq!(find("project_server").scope, McpScope::Directory);
    assert_eq!(
        find("project_server").directory.as_deref(),
        Some(&*workspace.to_string_lossy())
    );
    assert_ne!(find("marker").digest, find("other").digest);
    assert!(
        found
            .iter()
            .all(|server| server.source == "codex" && !server.digest.is_empty())
    );

    // A definition read out for an import keeps the program with its arguments, the variables apart from the
    // ones that are secrets, and the secrets by themselves.
    let read = rig
        .adapter
        .read_mcp(McpImportRequest {
            server_id: new_id(),
            source: "codex".to_owned(),
            scope: McpScope::User,
            name: "marker".to_owned(),
            directory: None,
        })
        .await
        .unwrap();
    assert_eq!(read.spec.transport, McpTransport::Stdio);
    let command = read.spec.command.clone().unwrap();
    assert_eq!(&command[..2], ["/bin/sh", "-c"]);
    assert!(command[2].contains("started-marker"), "{command:?}");
    assert_eq!(
        read.spec.env.get("PLAIN_SETTING").map(String::as_str),
        Some("visible")
    );
    assert_eq!(read.spec.secret_env, ["GITHUB_TOKEN"]);
    assert_eq!(read.secrets["GITHUB_TOKEN"].expose(), "gh-value-1");
    assert!(!read.spec.env.contains_key("GITHUB_TOKEN"));
    let from_project = rig
        .adapter
        .read_mcp(McpImportRequest {
            server_id: new_id(),
            source: "codex".to_owned(),
            scope: McpScope::Directory,
            name: "project_server".to_owned(),
            directory: Some(directories[0].clone()),
        })
        .await
        .unwrap();
    assert_eq!(from_project.spec.transport, McpTransport::Stdio);
    let missing = rig
        .adapter
        .read_mcp(McpImportRequest {
            server_id: new_id(),
            source: "codex".to_owned(),
            scope: McpScope::User,
            name: "nothing".to_owned(),
            directory: None,
        })
        .await
        .unwrap_err();
    assert_eq!(missing.code(), "not_found");

    // The project's directories hold a configuration of Codex's own: while nobody who manages the environment has
    // answered for it, a session there is refused before the harness reads any of it. Kept out, a session is held
    // to the servers it is given: those of the person's configuration and those of the configuration of the
    // project's directories are switched off, so the harness starts none of them, while it does start the one
    // the session is given.
    for directory in [&workspace, &nested] {
        let thread = new_id();
        let mut spec = rig.spec(thread, SessionMode::New, None);
        spec.settings.directory = directory.to_string_lossy().into_owned();
        spec.settings.cwd = spec.settings.directory.clone();
        spec.mcp = vec!["notes".to_owned()];
        let key = spec.session_key.clone();
        let unanswered = rig.adapter.open_session(spec.clone()).await.unwrap_err();
        assert_eq!(unanswered.code(), "workspace_configuration", "{unanswered}");
        spec.settings.workspace_configuration = Some(WorkspaceConfiguration {
            answer: ConfigTrust::KeepOut,
            files: orch_agent_contract::adapter::AdapterError::configuration_files(
                &unanswered.detail(),
            ),
        });
        rig.adapter.open_session(spec).await.unwrap();
        until_reached(&rig, &key, "notes").await;
        tokio::time::sleep(Duration::from_secs(2)).await;
        let started: Vec<_> = std::fs::read_dir(&workspace)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("started-"))
            .collect();
        assert!(
            started.is_empty(),
            "the harness started what the session is held from: {started:?}"
        );
    }
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn the_servers_a_session_is_given_are_reached_at_the_loopback_with_the_token_from_the_environment()
 {
    let Some(mut rig) = Rig::start("mcp-given").await else {
        return;
    };
    rig.boot().await;
    let thread = new_id();
    let mut spec = rig.spec(thread, SessionMode::New, None);
    spec.settings.orch_tools = vec![OrchTool::ListShared, OrchTool::ReadShared];
    spec.mcp = vec!["notes".to_owned()];
    let key = spec.session_key.clone();
    let error = rig.adapter.open_session(spec).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("required MCP servers failed to initialize"),
        "the required Orch server's refusal stops the session: {error}"
    );

    // Nothing answers there but the scripted model server, which refuses: it is enough that the harness comes
    // to the address of each server of the session, with the token the launch put in its environment.
    let mut reached = Vec::new();
    for _ in 0..300 {
        reached = rig
            .mock
            .requests()
            .into_iter()
            .filter(|request| request.path.starts_with("/mcp/"))
            .collect();
        let paths: Vec<&str> = reached
            .iter()
            .map(|request| request.path.as_str())
            .collect();
        if paths.contains(&format!("/mcp/{key}/orch").as_str())
            && paths.contains(&format!("/mcp/{key}/notes").as_str())
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    for name in ["orch", "notes"] {
        let path = format!("/mcp/{key}/{name}");
        let first = reached
            .iter()
            .find(|request| request.path == path)
            .unwrap_or_else(|| panic!("the harness did not come to {path}: {reached:?}"));
        assert_eq!(first.method, "POST", "{path}");
        assert_eq!(
            first.credential.verdict,
            Verdict::Accepted,
            "the harness presents the token of the launch to {path}"
        );
    }
    let launched = rig.launcher.launched.lock().unwrap().clone();
    assert_eq!(
        common::env_of(&launched[0]).get("ORCH_MCP_TOKEN"),
        Some(&rig.token)
    );
    assert!(
        launched[0].args.iter().all(|arg| !arg.contains(&rig.token)),
        "the token is in no argument"
    );
    rig.adapter.stop().await.unwrap();
}
