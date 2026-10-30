//! The environment role on the in-memory hub with the scripted controller, over real temporary directories: what the system asks of the files of an environment, and what the machine's owner holds it to.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod rig_environment;

use std::collections::BTreeMap;

use orch_link_contract::commands::{
    EnvAttach, ExecStart, FsCopy, FsFetch, FsList, FsMaterialize, FsMkdir, FsRemove, FsRename,
    FsSnapshot, FsStat, FsWrite, IndexMode, MaterializePurpose, McpImport, McpStart, McpStartGiven,
    Selection,
};
use orch_types::new_id;
use rig_environment::EnvRig;
use serde_json::json;

#[tokio::test]
async fn a_restart_reconstructs_attachment_and_executor_without_new_start_commands_and_detach_stays_detached()
 {
    use orch_connector_core::roles::RoleHandler;
    use orch_link_contract::commands::EnvDetach;
    let mut rig = EnvRig::new();
    // Policy offers name Orch's tool sets; the start command names their executor.
    rig.config.exec.offers = vec!["plain".to_owned()];
    rig.parts.executors = vec![rig_environment::stand_in_executors::header_echo()];
    let workspace = rig.directory("recovered-work");
    std::fs::write(workspace.join("kept.txt"), "kept workspace").unwrap();
    rig.start().await;
    rig.attach(&workspace).await;
    rig.succeed(&ExecStart {
        environment_id: rig.environment,
        executor: "orch-exec".to_owned(),
    })
    .await;
    rig.stop().await;
    rig.start().await;
    let role = rig.connector.as_ref().unwrap().environment_role().unwrap();
    assert!(role.host().environment(rig.environment).is_ok());
    let before = role
        .host()
        .executor_endpoint(rig.environment, "orch-exec")
        .await
        .unwrap();
    role.recover().await;
    let after = role
        .host()
        .executor_endpoint(rig.environment, "orch-exec")
        .await
        .unwrap();
    assert_eq!(before.address, after.address);
    assert_eq!(role.host().running_executors().await.len(), 1);
    let fetched = rig
        .succeed(&FsFetch {
            environment_id: rig.environment,
            mount: None,
            path: "kept.txt".to_owned(),
            expect: None,
            max_bytes: None,
        })
        .await;
    let hash = serde_json::from_value(fetched["hash"].clone()).unwrap();
    assert_eq!(
        rig.blobs.content(&hash).unwrap().as_ref(),
        b"kept workspace"
    );
    rig.succeed(&EnvDetach {
        environment_id: rig.environment,
    })
    .await;
    rig.stop().await;
    rig.start().await;
    assert!(
        rig.connector
            .as_ref()
            .unwrap()
            .environment_role()
            .unwrap()
            .host()
            .environment(rig.environment)
            .is_err()
    );
    rig.stop().await;
}

#[tokio::test]
async fn saved_attachment_cannot_outlive_the_machine_owners_current_allowed_roots() {
    let mut rig = EnvRig::new();
    let workspace = rig.directory("kept-policy");
    rig.start().await;
    rig.attach(&workspace).await;
    rig.stop().await;
    rig.config.environment.roots = vec![rig.directory("elsewhere").display().to_string()];
    rig.start().await;
    assert!(
        rig.connector
            .as_ref()
            .unwrap()
            .environment_role()
            .unwrap()
            .host()
            .environment(rig.environment)
            .is_err()
    );
    rig.stop().await;
}

#[tokio::test]
async fn a_directory_is_listed_and_a_file_is_written_through_a_content_transfer_and_read_back() {
    let mut rig = EnvRig::new();
    let workspace = rig.directory("work");
    std::fs::write(workspace.join("hello.txt"), "hello").unwrap();
    std::fs::create_dir(workspace.join("src")).unwrap();
    rig.start().await;
    let facts = rig.attach(&workspace).await;
    assert_eq!(
        facts["workspace_root"],
        json!(workspace.display().to_string())
    );

    // A directory is listed with what is in it.
    let listing = rig
        .succeed(&FsList {
            environment_id: rig.environment,
            mount: None,
            path: String::new(),
            watch_id: None,
            watch_instance: None,
            page: None,
        })
        .await;
    let names: Vec<&str> = listing["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"hello.txt") && names.contains(&"src"),
        "{names:?}"
    );
    assert_eq!(listing["truncated"], json!(false));

    // A file is written from content the system holds, a transfer that names the command.
    let content = b"written through a content transfer";
    let blob = rig.stored(content);
    let written = rig
        .succeed(&FsWrite {
            work: None,
            environment_id: rig.environment,
            path: "src/new.txt".to_owned(),
            blob,
            mode: None,
            expected_hash: None,
        })
        .await;
    assert_eq!(written["size"], json!(content.len()));
    assert_eq!(
        std::fs::read(workspace.join("src/new.txt")).unwrap(),
        content
    );

    // And a file is read back: its content goes up to the system, which holds it afterwards.
    let fetched = rig
        .succeed(&FsFetch {
            environment_id: rig.environment,
            mount: None,
            path: "hello.txt".to_owned(),
            expect: None,
            max_bytes: None,
        })
        .await;
    let hash: orch_types::Hash = serde_json::from_value(fetched["hash"].clone()).unwrap();
    assert_eq!(rig.blobs.content(&hash).unwrap().as_ref(), b"hello");
    let again = rig
        .succeed(&FsFetch {
            environment_id: rig.environment,
            mount: None,
            path: "src/new.txt".to_owned(),
            expect: None,
            max_bytes: None,
        })
        .await;
    assert_eq!(again["hash"], json!(blob));
    rig.stop().await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_directory_reports_its_actual_controllers_and_applies_the_declared_memory_to_started_work()
 {
    use orch_envhost::processes::RootKind;
    let mut rig = EnvRig::new();
    rig.start().await;
    let controls = rig.connector.as_ref().unwrap().services().process_controls;
    let facts = rig
        .succeed(&orch_link_contract::commands::MachineProbe {})
        .await;
    assert_eq!(
        facts["environment_descriptor"]["resource_limits"]["memory"],
        if controls.memory {
            "enforced"
        } else {
            "reported"
        }
    );
    assert_eq!(
        facts["environment_descriptor"]["resource_limits"]["disk"],
        "reported"
    );
    let directory = rig.directory("resource-ceiling");
    let mut body = attach_body(rig.environment, &directory);
    body.resources = Some(orch_driver_contract::runtime::Resources {
        memory_mb: Some(64),
        ..Default::default()
    });
    rig.succeed(&body).await;
    let environment = rig
        .connector
        .as_ref()
        .unwrap()
        .environment_role()
        .unwrap()
        .host()
        .environment(rig.environment)
        .unwrap();
    assert_eq!(environment.context().limits.memory_mb, Some(64));
    if controls.memory {
        assert!(!environment.context().process_launcher.is_empty());
        let mut command = environment.processes().command("python3");
        command
            .args([
                "-I",
                "-B",
                "-c",
                "value=bytearray(128*1024*1024); print('escaped')",
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        let child = environment
            .processes()
            .spawn_tracked(&mut command, RootKind::Executor)
            .unwrap();
        let output = tokio::time::timeout(
            rig_environment::PATIENCE,
            tokio::task::spawn_blocking(move || child.wait_with_output()),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        assert!(!output.status.success());
        assert!(!String::from_utf8_lossy(&output.stdout).contains("escaped"));
    } else {
        assert!(
            environment.context().process_launcher.is_empty(),
            "reported support never invents enforcement"
        );
    }
    rig.stop().await;
}

fn attach_body(environment_id: orch_types::Id, root: &std::path::Path) -> EnvAttach {
    EnvAttach {
        resources: None,
        data_generation: None,
        storage: None,
        data_sources: None,
        environment_id,
        workspace_root: Some(root.display().to_string()),
        ignore: Vec::new(),
        index: IndexMode::Full,
        mounts: Some(Vec::new()),
        variables: BTreeMap::new(),
        secrets: BTreeMap::new(),
    }
}

#[tokio::test]
async fn a_path_outside_the_workspace_is_refused_whatever_the_command() {
    let mut rig = EnvRig::new();
    let workspace = rig.directory("work");
    let outside = rig.directory("outside");
    std::fs::write(outside.join("secret.txt"), "not yours").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.join("secret.txt"), workspace.join("leak")).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(outside.join("secret.txt"), workspace.join("leak")).unwrap();
    rig.start().await;
    rig.attach(&workspace).await;
    let environment_id = rig.environment;
    let blob = rig.stored(b"content");

    for path in [
        "../outside/secret.txt",
        "src/../../outside/secret.txt",
        "/etc/hostname",
    ] {
        let named = path.to_owned();
        assert_eq!(
            rig.fail(&FsStat {
                environment_id,
                mount: None,
                path: named.clone()
            })
            .await,
            "not_found",
            "{path}"
        );
        assert_eq!(
            rig.fail(&FsFetch {
                environment_id,
                mount: None,
                path: named.clone(),
                expect: None,
                max_bytes: None,
            })
            .await,
            "not_found",
            "{path}"
        );
        assert_eq!(
            rig.fail(&FsWrite {
                work: None,
                environment_id,
                path: named.clone(),
                blob,
                mode: None,
                expected_hash: None
            })
            .await,
            "not_found",
            "{path}"
        );
        assert_eq!(
            rig.fail(&FsMkdir {
                environment_id,
                path: named.clone()
            })
            .await,
            "not_found",
            "{path}"
        );
        assert_eq!(
            rig.fail(&FsList {
                environment_id,
                mount: None,
                path: named,
                watch_id: None,
                watch_instance: None,
                page: None,
            })
            .await,
            "not_found",
            "{path}"
        );
    }
    // A link inside the workspace that points out of it leads nowhere either.
    assert_eq!(
        rig.fail(&FsFetch {
            environment_id,
            mount: None,
            path: "leak".to_owned(),
            expect: None,
            max_bytes: None,
        })
        .await,
        "not_found"
    );
    assert_eq!(
        std::fs::read(outside.join("secret.txt")).unwrap(),
        b"not yours"
    );
    assert!(!outside.join("src").exists());
    rig.stop().await;
}

#[tokio::test]
async fn a_write_the_machines_owner_forbids_is_refused_with_the_policys_code_and_changes_nothing() {
    let mut rig = EnvRig::new();
    rig.config.environment.remote_write = false;
    rig.config.environment.deliver_files = false;
    let workspace = rig.directory("work");
    std::fs::write(workspace.join("keep.txt"), "as it was").unwrap();
    rig.start().await;
    rig.attach(&workspace).await;
    let environment_id = rig.environment;
    let blob = rig.stored(b"what somebody sent");

    let write = FsWrite {
        work: None,
        environment_id,
        path: "keep.txt".to_owned(),
        blob,
        mode: None,
        expected_hash: None,
    };
    assert_eq!(rig.fail(&write).await, "policy.remote_write_disabled");
    assert_eq!(
        rig.fail(&FsMkdir {
            environment_id,
            path: "made".to_owned()
        })
        .await,
        "policy.remote_write_disabled"
    );
    assert_eq!(
        rig.fail(&FsRename {
            environment_id,
            from: "keep.txt".to_owned(),
            to: "kept.txt".to_owned()
        })
        .await,
        "policy.remote_write_disabled"
    );
    assert_eq!(
        rig.fail(&FsCopy {
            environment_id,
            from: "keep.txt".to_owned(),
            to: "copy.txt".to_owned()
        })
        .await,
        "policy.remote_write_disabled"
    );
    assert_eq!(
        rig.fail(&FsRemove {
            environment_id,
            path: "keep.txt".to_owned(),
            recursive: false
        })
        .await,
        "policy.remote_write_disabled"
    );
    // Files that arrive with a message have a switch of their own, and a move is a write.
    let delivery = FsMaterialize {
        work: None,
        environment_id,
        job_id: None,
        manifest_blob: blob,
        target_root: String::new(),
        purpose: MaterializePurpose::Delivery,
    };
    assert_eq!(rig.fail(&delivery).await, "policy.deliver_files_disabled");
    let moved = FsMaterialize {
        work: None,
        purpose: MaterializePurpose::Move,
        ..delivery
    };
    assert_eq!(rig.fail(&moved).await, "policy.remote_write_disabled");

    assert_eq!(
        std::fs::read(workspace.join("keep.txt")).unwrap(),
        b"as it was"
    );
    let names: Vec<String> = std::fs::read_dir(&workspace)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["keep.txt"], "nothing was made");
    rig.stop().await;
}

#[tokio::test]
async fn an_environment_outside_what_the_owner_offers_is_refused_and_so_is_what_the_owner_did_not_allow()
 {
    let mut rig = EnvRig::new();
    rig.config.exec.offers = vec!["plain".to_owned()];
    rig.config.mcp.import = false;
    let workspace = rig.directory("work");
    rig.start().await;

    // A root the owner does not offer.
    let elsewhere = tempfile::tempdir().unwrap();
    let refused = rig.fail(&attach_body(new_id(), elsewhere.path())).await;
    assert_eq!(refused, "policy.root_not_allowed");
    rig.attach(&workspace).await;
    let environment_id = rig.environment;

    // An executor the owner does not let the exec node offer.
    assert_eq!(
        rig.fail(&ExecStart {
            environment_id,
            executor: "codex-exec-server".to_owned()
        })
        .await,
        "policy.exec_not_offered"
    );
    // A tool server written from Orch, while the owner allows none; and one imported, while none may be.
    let spec = json!({ "transport": "stdio", "command": ["true"] });
    assert_eq!(
        rig.fail(&McpStart::Given(Box::new(McpStartGiven {
            server_id: None,
            environment_id,
            name: "mine".to_owned(),
            spec: serde_json::from_value(spec).unwrap(),
            secrets: BTreeMap::new(),
        })))
        .await,
        "policy.mcp_add_disabled"
    );
    assert_eq!(
        rig.fail(&McpImport {
            expected_fingerprint: None,
            environment_id,
            server_id: new_id(),
            source: "codex".to_owned(),
            scope: "user".to_owned(),
            name: "github".to_owned(),
            directory: None,
        })
        .await,
        "policy.mcp_import_disabled"
    );
    rig.stop().await;
}

#[tokio::test]
async fn a_tree_is_snapshotted_and_materialized_in_another_environment_with_its_progress_reported()
{
    let mut rig = EnvRig::new();
    let source = rig.directory("source");
    std::fs::create_dir_all(source.join("src/deep")).unwrap();
    std::fs::write(source.join("a.txt"), "first").unwrap();
    std::fs::write(source.join("src/deep/b.txt"), "second ".repeat(1000)).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("a.txt", source.join("link")).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file("a.txt", source.join("link")).unwrap();
    let target = rig.directory("target");
    rig.start().await;
    rig.attach(&source).await;
    let second = new_id();
    rig.succeed(&attach_body(second, &target)).await;

    // The whole tree goes up: the list, and every content the system lacked.
    let job = new_id();
    let snapshot = rig
        .succeed(&FsSnapshot {
            work: None,
            environment_id: rig.environment,
            job_id: job,
            selection: Selection {
                include: vec![String::new()],
                exclude: Vec::new(),
            },
        })
        .await;
    assert_eq!(
        snapshot["file_count"],
        json!(2),
        "two files; a link is no file"
    );
    assert_eq!(snapshot["missing"], json!(0));
    let manifest: orch_types::Hash =
        serde_json::from_value(snapshot["manifest_blob"].clone()).unwrap();
    assert!(rig.blobs.content(&manifest).is_some());
    // The progress of the job went out on the environment's stream, in order, ending done.
    rig.wait_for("job.progress", 1).await;
    let reports: Vec<_> = rig
        .applied()
        .into_iter()
        .filter(|e| e.kind == "job.progress")
        .collect();
    assert!(reports.iter().all(|e| e.body["job_id"] == json!(job)));

    // And is written in the other environment, which then has the same files.
    let moved = rig
        .succeed(&FsMaterialize {
            work: None,
            environment_id: second,
            job_id: Some(new_id()),
            manifest_blob: manifest,
            target_root: String::new(),
            purpose: MaterializePurpose::Move,
        })
        .await;
    assert_eq!(moved["files"], json!(2));
    assert_eq!(
        std::fs::read_to_string(target.join("a.txt")).unwrap(),
        "first"
    );
    assert_eq!(
        std::fs::read_to_string(target.join("src/deep/b.txt")).unwrap(),
        "second ".repeat(1000)
    );
    assert_eq!(
        std::fs::read_link(target.join("link")).unwrap().to_str(),
        Some("a.txt")
    );
    // Written again, nothing is written twice.
    let again = rig
        .succeed(&FsMaterialize {
            work: None,
            environment_id: second,
            job_id: None,
            manifest_blob: manifest,
            target_root: String::new(),
            purpose: MaterializePurpose::Move,
        })
        .await;
    assert_eq!(again["files"], json!(0));
    assert!(again["skipped"].as_i64().unwrap() >= 2);
    rig.stop().await;
}

#[tokio::test]
async fn what_the_environment_says_on_its_own_account_goes_on_its_stream() {
    let mut rig = EnvRig::new();
    let workspace = rig.directory("work");
    std::fs::write(workspace.join("a.txt"), "a").unwrap();
    rig.start().await;
    rig.attach(&workspace).await;
    // Attached, it is running; and the facts the system stores come with the answer.
    rig.wait_for("env.status", 1).await;
    let status = rig
        .applied()
        .into_iter()
        .find(|e| e.kind == "env.status")
        .unwrap();
    assert_eq!(status.body["status"], "running");
    // The whole tree's list is uploaded and announced when it is asked for.
    let manifest = rig
        .succeed(&orch_link_contract::commands::FsManifest {
            environment_id: rig.environment,
            selection: Selection {
                include: vec![String::new()],
                exclude: Vec::new(),
            },
        })
        .await;
    rig.wait_for("env.manifest", 1).await;
    let announced = rig
        .applied()
        .into_iter()
        .find(|e| e.kind == "env.manifest")
        .unwrap();
    assert_eq!(announced.body["manifest_blob"], manifest["manifest_blob"]);
    // Detached, it is offline.
    rig.succeed(&orch_link_contract::commands::EnvDetach {
        environment_id: rig.environment,
    })
    .await;
    rig.wait_for("env.status", 2).await;
    let kinds: Vec<_> = rig
        .applied()
        .into_iter()
        .filter(|e| e.kind == "env.status")
        .map(|e| e.body["status"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(kinds, ["running", "offline"]);
    rig.stop().await;
}

/// A tools directory as the node leaves it for a build of Codex: the package's program below `bin`, and the
/// version the directory says.
#[cfg(unix)]
fn tools_with_codex() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let tools = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tools.path().join("codex/bin")).unwrap();
    let program = tools.path().join("codex/bin/codex");
    std::fs::write(&program, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(tools.path().join("codex/.version"), "0.165.0\n").unwrap();
    tools
}

/// A rig for a runtime the system made, whose connector can host Codex and whose environment offers an executor
/// that runs only where the build of Codex is.
#[cfg(unix)]
fn rig_of_a_runtime(tools: &std::path::Path) -> EnvRig {
    use orch_adapter_fake::FakeAdapter;
    use orch_connector_core::harnesses::{AdapterRegistration, HomeRule};
    let mut rig = EnvRig::new();
    rig.adapters
        .register(AdapterRegistration {
            kind: "codex",
            adapter_version: 1,
            builtin: true,
            needs_binary: true,
            home_rule: HomeRule::Harness,
            factory: std::sync::Arc::new(|clock| Box::new(FakeAdapter::new(clock))),
        })
        .unwrap();
    rig.parts.executors = vec![rig_environment::stand_in_executors::needs_a_build()];
    rig.tools_dir = Some(tools.to_owned());
    rig
}

#[cfg(unix)]
#[tokio::test]
async fn an_executor_that_runs_a_harnesss_build_is_available_where_the_system_put_the_build_and_the_machine_says_it_has_it()
 {
    let tools = tools_with_codex();
    let mut rig = rig_of_a_runtime(tools.path());
    let workspace = rig.directory("work");
    rig.start().await;
    let facts = rig.attach(&workspace).await;
    let executor = facts["executors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|each| each["kind"] == "needs-a-build")
        .unwrap();
    assert_eq!(executor["available"], true, "{executor}");
    assert_eq!(
        executor["version"], "0.165.0",
        "the tool is listed with the version its directory says"
    );
    let harnesses = rig.connector.as_ref().unwrap().facts().harnesses;
    assert_eq!(
        harnesses
            .iter()
            .map(|each| (
                each.kind.as_str(),
                each.version.as_str(),
                each.path.as_str()
            ))
            .collect::<Vec<_>>(),
        [(
            "codex",
            "0.165.0",
            tools.path().join("codex/bin/codex").to_str().unwrap()
        )]
    );
    rig.stop().await;
}

#[cfg(unix)]
#[tokio::test]
async fn an_executor_that_runs_a_harnesss_build_is_not_available_in_a_runtime_that_was_not_given_the_build()
 {
    let empty = tempfile::tempdir().unwrap();
    let mut rig = rig_of_a_runtime(empty.path());
    let workspace = rig.directory("work");
    rig.start().await;
    let facts = rig.attach(&workspace).await;
    let executor = facts["executors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|each| each["kind"] == "needs-a-build")
        .unwrap();
    assert_eq!(executor["available"], false, "{executor}");
    assert!(
        rig.connector.as_ref().unwrap().facts().harnesses.is_empty(),
        "it has no harness to report"
    );
    rig.stop().await;
}
