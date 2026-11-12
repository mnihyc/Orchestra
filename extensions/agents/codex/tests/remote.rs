//! A session whose tools are carried out by a Codex exec-server in a container of its own, while the harness
//! runs in another that cannot reach the workspace, on the real Codex in Docker.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::remote::{ExecServer, SplitProxy};
use common::{DockerLauncher, Rig, kinds, script};
use orch_agent_contract::adapter::AgentAdapter;
use orch_agent_contract::descriptor::ArrangementType;
use orch_agent_contract::events::{
    FileChangeKind, ItemStatus, SessionEvent, ToolCallPayload, ToolPayload, TurnOutcome,
};
use orch_agent_contract::session::SessionMode;
use orch_types::new_id;

#[tokio::test]
async fn the_tools_of_a_session_are_carried_out_by_an_exec_server_in_another_container() {
    let Some(mut rig) = Rig::start_with("remote", DockerLauncher::without_workspace).await else {
        return;
    };
    let workspace = rig.workspace_path().to_path_buf();
    let remote_workspace = std::path::PathBuf::from(format!("/orch-exec-workspace-{}", new_id()));
    assert!(
        !remote_workspace.exists(),
        "the native path must not exist on the adapter's filesystem"
    );
    let exec = ExecServer::start_at("remote", &workspace, &remote_workspace, &rig.token).await;
    let proxy = SplitProxy::start(rig.mock.addr(), exec.addr()).await;
    rig.loopback_port = Some(proxy.port());
    rig.boot().await;

    let thread = new_id();
    let mut spec = rig.spec(thread, SessionMode::New, None);
    spec.exec.executor = Some("codex-exec-server".to_owned());
    spec.settings.arrangement = ArrangementType::Separated;
    let directory = remote_workspace.to_string_lossy().into_owned();
    spec.settings.workspace_root.clone_from(&directory);
    spec.settings.directory.clone_from(&directory);
    spec.settings.cwd.clone_from(&directory);
    rig.adapter.open_session(spec).await.unwrap();

    // A command: it runs where the exec-server is, since the harness's own container has no such directory.
    let text = script(
        "write it",
        &[
            &format!(
                r#"tool exec_command {{"cmd":"echo from-the-executor > {0}/out.txt && echo printed-there && pwd"}}"#,
                remote_workspace.display()
            ),
            "then",
            "say written",
        ],
    );
    let events = rig.run_turn(thread, &text).await;
    let call = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::ItemCompleted(item) => {
                serde_json::from_value::<ToolCallPayload>(item.payload.clone()).ok()
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("a tool call completes: {:?}", kinds(&events)));
    let ToolPayload::Command {
        exit_code,
        output,
        cwd,
        ..
    } = &call.tool
    else {
        panic!("a command");
    };
    assert_eq!(*exit_code, Some(0), "{:?}", call.tool);
    let shown = output.inline.clone();
    assert!(shown.contains("printed-there"), "{shown:?}");
    assert!(shown.contains(&directory), "{shown:?}");
    assert_eq!(cwd, &directory);
    assert!(
        !remote_workspace.exists(),
        "native execution cannot create a same-spelling adapter workspace"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("out.txt")).unwrap(),
        "from-the-executor\n"
    );
    assert!(
        matches!(events.last(), Some(SessionEvent::TurnCompleted(ended)) if ended.status == TurnOutcome::Completed)
    );

    // A change of files, made by the harness's own tool for it, reaches the executor's disk too.
    let patch = "apply_patch <<'PATCH'\\n*** Begin Patch\\n*** Add File: made.txt\\n+made there\\n*** End Patch\\nPATCH";
    let text = script(
        "patch it",
        &[
            &format!(r#"tool exec_command {{"cmd":"{patch}"}}"#),
            "then",
            "say patched",
        ],
    );
    let events = rig.run_turn(thread, &text).await;
    let changed = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::ItemCompleted(item) if item.status == ItemStatus::Completed => {
                match serde_json::from_value::<ToolCallPayload>(item.payload.clone()) {
                    Ok(call) => match call.tool {
                        ToolPayload::FileChange { changes } => Some(changes),
                        _ => None,
                    },
                    Err(_) => None,
                }
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("a change of files completes: {:?}", kinds(&events)));
    assert_eq!(changed[0].change, FileChangeKind::Add);
    assert_eq!(
        std::fs::read_to_string(workspace.join("made.txt")).unwrap(),
        "made there\n"
    );
    rig.adapter.stop().await.unwrap();
}

const VIEW_RED: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==";
const VIEW_FADE: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
fn request_images(value: &serde_json::Value, images: &mut Vec<(String, Vec<u8>)>) {
    use base64::Engine as _;
    if value["type"] == "input_image"
        && let Some(url) = value["image_url"].as_str()
        && let Some((mime, data)) = url
            .strip_prefix("data:")
            .and_then(|url| url.split_once(";base64,"))
    {
        images.push((
            mime.into(),
            base64::engine::general_purpose::STANDARD
                .decode(data)
                .unwrap(),
        ));
    }
    match value {
        serde_json::Value::Array(values) => {
            for value in values {
                request_images(value, images);
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values() {
                request_images(value, images);
            }
        }
        _ => {}
    }
}
fn retained_view(events: &[SessionEvent]) -> (String, orch_types::Hash, String) {
    let images: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ItemCompleted(item) if item.payload["tool"]["kind"] == "image" => {
                Some(item)
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        images.len(),
        1,
        "one actual viewed image: {:?}",
        kinds(events)
    );
    let item = images[0];
    assert_eq!(item.status, ItemStatus::Completed);
    assert_eq!(item.payload["tool"]["name"], "view.png");
    assert!(
        item.payload["raw"]["output"]["blob"].is_string(),
        "the exact native source stays available"
    );
    (
        item.native_ref.clone(),
        serde_json::from_value(item.payload["tool"]["blob"].clone()).unwrap(),
        item.payload["tool"]["media_type"].as_str().unwrap().into(),
    )
}

#[tokio::test]
async fn viewed_images_come_from_the_remote_native_result_on_new_cold_resume_and_replay() {
    use base64::Engine as _;
    use orch_agent_contract::adapter::AdapterEvent;
    let Some(mut rig) = Rig::start_with("remote-images", DockerLauncher::without_workspace).await
    else {
        return;
    };
    let workspace = rig.workspace_path().to_owned();
    let remote = std::path::PathBuf::from(format!("/orch-image-workspace-{}", new_id()));
    assert!(
        !remote.exists(),
        "the adapter must have no same-spelling file source"
    );
    let exec = ExecServer::start_at("remote-images", &workspace, &remote, &rig.token).await;
    let proxy = SplitProxy::start(rig.mock.addr(), exec.addr()).await;
    rig.loopback_port = Some(proxy.port());
    rig.boot().await;
    let thread = new_id();
    let mut spec = rig.spec(thread, SessionMode::New, None);
    spec.exec.executor = Some("codex-exec-server".into());
    spec.settings.arrangement = ArrangementType::Separated;
    let directory = remote.to_string_lossy().into_owned();
    spec.settings.workspace_root.clone_from(&directory);
    spec.settings.directory.clone_from(&directory);
    spec.settings.cwd.clone_from(&directory);
    let opened = rig.adapter.open_session(spec).await.unwrap();
    let native = opened.native_session_ref;
    let mut live = std::collections::BTreeMap::new();
    for (ordinal, pixel) in [VIEW_RED, VIEW_FADE].into_iter().enumerate() {
        if ordinal == 1 {
            rig.adapter.stop().await.unwrap();
            rig.boot().await;
            let mut resumed = rig.spec(thread, SessionMode::Resume, Some(&native));
            resumed.exec.executor = Some("codex-exec-server".into());
            resumed.settings.arrangement = ArrangementType::Separated;
            resumed.settings.workspace_root.clone_from(&directory);
            resumed.settings.directory.clone_from(&directory);
            resumed.settings.cwd.clone_from(&directory);
            rig.adapter.open_session(resumed).await.unwrap();
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(pixel)
            .unwrap();
        std::fs::write(workspace.join("view.png"), &bytes).unwrap();
        let before = rig.model_requests().len();
        let ask = script(
            "open the environment image",
            &[
                &format!(
                    r#"tool view_image {{"path":"{}/view.png"}}"#,
                    remote.display()
                ),
                "then",
                "say image-read",
            ],
        );
        let events = rig.run_turn(thread, &ask).await;
        assert!(
            matches!(events.last(),Some(SessionEvent::TurnCompleted(turn)) if turn.status==TurnOutcome::Completed),
            "{:?}",
            kinds(&events)
        );
        let (reference, hash, mime) = retained_view(&events);
        let mut observed = Vec::new();
        for request in rig.model_requests().iter().skip(before) {
            request_images(&request.body, &mut observed);
        }
        let (model_mime, model_bytes) = observed
            .last()
            .expect("the real native tool's image reached the model");
        assert_eq!(&mime, model_mime);
        assert_eq!(hash, orch_types::Hash::of(model_bytes));
        let output = rig
            .adapter
            .output_directory()
            .expect("the existing retained output path")
            .join(hash.to_string());
        assert_eq!(std::fs::read(output).unwrap(), *model_bytes);
        assert!(
            !remote.join("view.png").exists(),
            "no adapter-local image can satisfy this proof"
        );
        assert_eq!(
            std::fs::read(workspace.join("view.png")).unwrap(),
            bytes,
            "viewing retains the environment's original file"
        );
        live.insert(reference, hash);
    }
    assert_eq!(
        live.len(),
        2,
        "the two actual native calls keep separate identities"
    );
    let (replayed, seen) = tokio::time::timeout(common::WAIT, async {
        let adapter = &mut rig.adapter;
        let stream = &mut rig.events;
        tokio::join!(adapter.resync_session(thread), async {
            let mut ended = 0;
            let mut images = std::collections::BTreeMap::new();
            while ended < 2 {
                match stream.recv().await.expect("the replay stream stays open") {
                    AdapterEvent::Session {
                        event: SessionEvent::ItemCompleted(item),
                        ..
                    } if item.payload["tool"]["kind"] == "image" => {
                        images.insert(
                            item.native_ref,
                            serde_json::from_value::<orch_types::Hash>(
                                item.payload["tool"]["blob"].clone(),
                            )
                            .unwrap(),
                        );
                    }
                    AdapterEvent::Session {
                        event: SessionEvent::TurnCompleted(_),
                        ..
                    } => ended += 1,
                    _ => {}
                }
            }
            images
        })
    })
    .await
    .expect("the existing native replay deadline");
    assert!(replayed.unwrap() > 0);
    assert_eq!(
        seen, live,
        "replay reuses exact retained image references and bytes"
    );
    let record = rig.adapter.read_session(&native, true).await.unwrap();
    let file = record.file.expect("adoption writes its ordinary record");
    let recorded: std::collections::BTreeMap<_, _> = std::fs::read_to_string(&file.path)
        .unwrap()
        .lines()
        .filter_map(|line| {
            use orch_agent_contract::discovery::RecordedEntry;
            use orch_agent_contract::events::ItemContent;
            match serde_json::from_str::<RecordedEntry>(line).unwrap() {
                RecordedEntry::Item(item) => match item.content {
                    ItemContent::ToolCall(call) => match call.tool {
                        ToolPayload::Image { blob, .. } => Some((item.native_ref, blob)),
                        _ => None,
                    },
                    _ => None,
                },
                _ => None,
            }
        })
        .collect();
    assert_eq!(
        recorded, live,
        "adoption retains the same exact image results"
    );
    for hash in recorded.values() {
        assert!(
            rig.adapter
                .output_directory()
                .unwrap()
                .join(hash.to_string())
                .is_file(),
            "adoption dependencies survive temporary record-cache cleanup"
        );
    }
    rig.adapter.stop().await.unwrap();
}
