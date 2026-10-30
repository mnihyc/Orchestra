//! A pinned remote Codex executor applies a resumed cwd without changing native identity, replaying tools or rewriting files.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{
    Rig,
    remote::{ExecServer, SplitProxy},
    script,
};
use orch_agent_contract::{
    adapter::AgentAdapter,
    descriptor::{ApprovalMode, ArrangementType},
    events::{ItemContent, SessionEvent, TurnOutcome},
    session::SessionMode,
};
use orch_types::new_id;
use serde_json::{Value, json};
use std::{net::SocketAddr, path::Path, process::Stdio, time::Duration};
use tokio::{
    net::{TcpListener, TcpStream},
    process::{Child, Command},
};

type Proof<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
// The existing remote fixture's peer-start budget; no native turn's90s budget is enlarged.
const PEER_WAIT: Duration = Duration::from_secs(30);
const CORE: &str =
    "CONFIG-FIXED-CORE-BEGIN\nKeep these exact session instructions.\nCONFIG-FIXED-CORE-END";

struct LocalExecutor {
    child: Child,
    address: SocketAddr,
    _token: tempfile::TempDir,
}

impl LocalExecutor {
    async fn start(home: &Path, workspace: &Path, token: &str) -> Proof<Self> {
        assert!(
            orch_testkit::native_configuration::container_proof_mode(module_path!()),
            "direct peer execution is restricted to the owned proof container"
        );
        let directory = tempfile::tempdir()?;
        let file = directory.path().join("token");
        std::fs::write(&file, token)?;
        let mut address = None;
        // Reuse the remote fixture's loopback-only port range.
        for port in 41700..42000 {
            if let Ok(listener) = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port))).await
            {
                address = Some(listener.local_addr()?);
                break;
            }
        }
        let address = address.ok_or("no free loopback port for the owned native executor")?;
        let child = Command::new("codex")
            .args(["exec-server", "--listen"])
            .arg(format!("ws://{address}"))
            .args(["--ws-auth", "capability-token", "--ws-token-file"])
            .arg(file)
            .current_dir(workspace)
            .env_clear()
            .env("HOME", home)
            .env("CODEX_HOME", home)
            .env(
                "PATH",
                std::env::var_os("PATH").unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".into()),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let mut peer = Self {
            child,
            address,
            _token: directory,
        };
        tokio::time::timeout(PEER_WAIT, async {
            loop {
                if TcpStream::connect(address).await.is_ok() {
                    return Ok::<(), Box<dyn std::error::Error + Send + Sync>>(());
                }
                if let Some(status) = peer.child.try_wait()? {
                    return Err(format!(
                        "the owned native executor exited with {}",
                        status.code().unwrap_or(-1)
                    )
                    .into());
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await??;
        Ok(peer)
    }

    async fn stop(mut self) -> Proof {
        if self.child.try_wait()?.is_none() {
            self.child.start_kill()?;
            tokio::time::timeout(PEER_WAIT, self.child.wait()).await??;
        }
        Ok(())
    }
}

impl Drop for LocalExecutor {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

enum Executor {
    Container(ExecServer),
    Local(LocalExecutor),
}
impl Executor {
    fn address(&self) -> SocketAddr {
        match self {
            Self::Container(peer) => peer.addr(),
            Self::Local(peer) => peer.address,
        }
    }
    async fn stop(self) -> Proof {
        match self {
            Self::Container(peer) => {
                drop(peer);
                Ok(())
            }
            Self::Local(peer) => peer.stop().await,
        }
    }
}

fn fixed_core(value: &Value) -> Vec<Vec<u8>> {
    match value {
        Value::String(text) => {
            text.find("CONFIG-FIXED-CORE-BEGIN")
                .map_or_else(Vec::new, |start| {
                    assert_eq!(text.matches("CONFIG-FIXED-CORE-BEGIN").count(), 1);
                    let end = start
                        + text[start..]
                            .find("CONFIG-FIXED-CORE-END")
                            .expect("complete fixed core")
                        + "CONFIG-FIXED-CORE-END".len();
                    vec![text.as_bytes()[start..end].to_vec()]
                })
        }
        Value::Array(values) => values.iter().flat_map(fixed_core).collect(),
        Value::Object(values) => values.values().flat_map(fixed_core).collect(),
        _ => Vec::new(),
    }
}

fn actual_cwd(events: &[SessionEvent], cwd: &str) {
    assert!(
        matches!(events.last(),Some(SessionEvent::TurnCompleted(end)) if end.status==TurnOutcome::Completed)
    );
    assert!(events.iter().any(|event|matches!(event,SessionEvent::ItemCompleted(item)
        if item.payload["tool"]["kind"]=="command" && item.payload["tool"]["cwd"]==cwd
          && item.payload["tool"]["output"]["inline"].as_str().is_some_and(|text|text.lines().any(|line|line==cwd)))),
        "the remote executor must report and actually run the accepted cwd");
}

fn tool_turn(counter: &Path, marker: &str) -> String {
    script(
        marker,
        &[
            &format!(
                "tool exec_command {}",
                json!({"cmd":format!("echo call >> '{}' && pwd",counter.display())})
            ),
            "then",
            &format!("say {marker} complete"),
        ],
    )
}

#[tokio::test]
async fn a_remote_executor_uses_the_changed_cwd_on_the_same_native_conversation_and_keeps_the_following_turn_stable()
-> Proof {
    let mut rig = Rig::start("session-configuration")
        .await
        .expect("the pinned native image/container is required; this proof never silently skips");
    let peer = if orch_testkit::native_configuration::container_proof_mode(module_path!()) {
        Executor::Local(
            LocalExecutor::start(rig.home_path(), rig.workspace_path(), &rig.token).await?,
        )
    } else {
        Executor::Container(
            ExecServer::start("session-configuration", rig.workspace_path(), &rig.token).await,
        )
    };
    let proxy = SplitProxy::start(rig.mock.addr(), peer.address()).await;
    rig.loopback_port = Some(proxy.port());
    assert_eq!(rig.boot().await.harness_version, "0.165.0");
    let thread = new_id();
    let child = rig.workspace_path().join("child");
    std::fs::create_dir_all(&child)?;
    let counter = rig.workspace_path().join("tool-count");
    let fixed = rig.workspace_path().join("human-data.bin");
    std::fs::write(&fixed, b"unchanged remote workspace\0\xff")?;
    let mut spec = rig.spec(thread, SessionMode::New, None);
    spec.exec.executor = Some("codex-exec-server".into());
    spec.settings.arrangement = ArrangementType::Separated;
    spec.settings.instructions = CORE.into();
    spec.settings.raw_events = true;
    spec.settings.approval_mode = ApprovalMode::Bypass;
    let opened = rig.adapter.open_session(spec.clone()).await?;
    let events = rig
        .run_turn(thread, &tool_turn(&counter, "CONFIG-REMOTE-FIRST"))
        .await;
    actual_cwd(&events, &spec.settings.cwd);
    assert!(
        proxy.exec_connections() > 0,
        "the native harness really connected to the executor peer"
    );
    let first = rig
        .model_requests()
        .last()
        .ok_or("the first actual model input is absent")?
        .body
        .clone();
    let tools = serde_json::to_vec(first.get("tools").ok_or("actual native tools absent")?)?;
    let count = rig.model_requests().len();
    let launches = rig.launcher.launched.lock().unwrap().len();
    spec.settings.mode = SessionMode::Resume;
    spec.settings.native_session_ref = Some(opened.native_session_ref.clone());
    spec.settings.cwd = child.display().to_string();
    let resumed = rig.adapter.open_session(spec.clone()).await?;
    assert!(resumed.resumed);
    assert_eq!(resumed.native_session_ref, opened.native_session_ref);
    assert_eq!(
        rig.model_requests().len(),
        count,
        "reconfiguration submits no model input"
    );
    let events = rig
        .run_turn(thread, &tool_turn(&counter, "CONFIG-REMOTE-CHANGED"))
        .await;
    actual_cwd(&events, &spec.settings.cwd);
    assert!(
        !events.iter().any(|event| matches!(event,
            SessionEvent::ItemStarted(item) if matches!(item.content, ItemContent::Error(_))
        )),
        "a successful resumed native session must not show a fabricated error"
    );
    let body = rig
        .model_requests()
        .last()
        .ok_or("the resumed actual model input is absent")?
        .body
        .clone();
    assert!(body.to_string().contains("CONFIG-REMOTE-FIRST"));
    assert_eq!(fixed_core(&body), vec![CORE.as_bytes().to_vec()]);
    assert_eq!(
        serde_json::to_vec(body.get("tools").ok_or("resumed native tools absent")?)?,
        tools
    );
    let events = rig
        .run_turn(thread, &tool_turn(&counter, "CONFIG-REMOTE-UNCHANGED"))
        .await;
    actual_cwd(&events, &spec.settings.cwd);
    assert_eq!(
        rig.launcher.launched.lock().unwrap().len(),
        launches,
        "remote reconfiguration/unchanged turns do not start a replacement app server"
    );
    assert_eq!(
        std::fs::read_to_string(counter)?,
        "call\ncall\ncall\n",
        "old commands cannot be reexecuted during resume"
    );
    assert_eq!(std::fs::read(fixed)?, b"unchanged remote workspace\0\xff");
    assert!(
        !rig.workspace_path().join("AGENTS.md").exists()
            && !rig.workspace_path().join("CLAUDE.md").exists()
    );
    rig.adapter.stop().await?;
    peer.stop().await?;
    rig.prefix_proof_succeeded();
    Ok(())
}

/// Questions use the native person-input round trip in both supported execution placements.
#[tokio::test]
async fn native_questions_reach_the_person_and_return_content_in_each_admitted_mode_and_placement()
-> Proof {
    use orch_agent_contract::adapter::AdapterEvent;
    use orch_agent_contract::events::{HarnessRequestDetail, HarnessRequestKind};
    use orch_agent_contract::session::ApprovalDecision;
    use std::collections::BTreeMap;

    for separated in [false, true] {
        let mut rig = Rig::start("native-questions")
            .await
            .expect("the pinned native image/container is required");
        let peer = if separated {
            Some(
                if orch_testkit::native_configuration::container_proof_mode(module_path!()) {
                    Executor::Local(
                        LocalExecutor::start(rig.home_path(), rig.workspace_path(), &rig.token)
                            .await?,
                    )
                } else {
                    Executor::Container(
                        ExecServer::start("native-questions", rig.workspace_path(), &rig.token)
                            .await,
                    )
                },
            )
        } else {
            None
        };
        let proxy = if let Some(peer) = &peer {
            Some(SplitProxy::start(rig.mock.addr(), peer.address()).await)
        } else {
            None
        };
        if let Some(proxy) = &proxy {
            rig.loopback_port = Some(proxy.port());
        }
        let started = rig.boot().await;
        assert_eq!(started.harness_version, "0.165.0");
        for mode in [
            ApprovalMode::Ask,
            ApprovalMode::OnRequest,
            ApprovalMode::Bypass,
        ] {
            let thread = new_id();
            let mut spec = rig.spec(thread, SessionMode::New, None);
            spec.settings.approval_mode = mode;
            spec.settings.raw_events = true;
            if separated {
                spec.exec.executor = Some("codex-exec-server".into());
                spec.settings.arrangement = ArrangementType::Separated;
            }
            rig.adapter.open_session(spec).await?;
            let text = script(
                "A native question",
                &[
                    r#"tool request_user_input {"questions":[{"id":"choice","header":"Choice","question":"Which value?","options":[{"label":"First","description":"First value"},{"label":"Second","description":"Second value"}]}]}"#,
                    "then",
                    "say The person's answer was received",
                ],
            );
            let mut turn = rig.turn(thread, &text);
            turn.approval_mode = mode;
            let turn_id = turn.turn_id;
            let before = rig.model_requests().len();
            rig.adapter.start_turn(turn).await?;
            let seen = rig
                .until(|event| {
                    matches!(
                        event,
                        AdapterEvent::Session {
                            event: SessionEvent::ApprovalRequested(_)
                                | SessionEvent::TurnCompleted(_),
                            ..
                        }
                    )
                })
                .await;
            let mut events = common::session_events(&seen);
            let asked = events
                .iter()
                .find_map(|event| {
                    if let SessionEvent::ApprovalRequested(request) = event {
                        Some(request.clone())
                    } else {
                        None
                    }
                })
                .expect("the configured native default mode must ask a person");
            assert_eq!(asked.request.request_kind, HarnessRequestKind::UserInput);
            assert_eq!(asked.turn_id, turn_id);
            let HarnessRequestDetail::UserInput { questions, .. } = &asked.request.detail else {
                panic!("a native content question")
            };
            assert_eq!(questions.len(), 1);
            assert_eq!(questions[0].id, "choice");
            let source = asked
                .request
                .detail
                .native_item_ref()
                .expect("native question source correlation")
                .to_owned();
            rig.adapter
                .resolve_approval(
                    thread,
                    &asked.request_ref,
                    ApprovalDecision::Answers {
                        answers: BTreeMap::from([("choice".into(), vec!["Second".into()])]),
                        note: None,
                    },
                )
                .await?;
            events.extend(rig.until_turn_ends().await);
            assert!(
                matches!(events.last(), Some(SessionEvent::TurnCompleted(end)) if end.status == TurnOutcome::Completed && end.turn_id == turn_id)
            );
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(event, SessionEvent::ApprovalRequested(_)))
                    .count(),
                1
            );
            let source_present = events.iter().any(|event| matches!(event, SessionEvent::ItemStarted(item) if item.native_ref == source)
                || matches!(event, SessionEvent::ItemCompleted(item) if item.native_ref == source));
            if !source_present {
                // Retain protocol shape, never parameters, identifiers or credentials, on a failed canary.
                let inventory: Vec<_> = events.iter().map(|event| match event {
                    SessionEvent::RawFrame(frame) => json!({
                        "event": event.kind(),
                        "method": frame.payload.get("method"),
                        "item_type": frame.payload.pointer("/params/item/type"),
                        "source_matches": frame.payload.pointer("/params/item/id").and_then(Value::as_str) == Some(source.as_str()),
                    }),
                    SessionEvent::ItemStarted(item) => json!({"event":event.kind(),"source_matches":item.native_ref == source}),
                    SessionEvent::ItemCompleted(item) => json!({"event":event.kind(),"source_matches":item.native_ref == source}),
                    _ => json!({"event":event.kind()}),
                }).collect();
                eprintln!("native question event inventory: {}", json!(inventory));
            }
            assert!(
                source_present,
                "native source item must remain available for the question's work detail"
            );
            let received = rig
                .model_requests()
                .into_iter()
                .skip(before)
                .any(|request| {
                    request
                        .body
                        .get("input")
                        .and_then(Value::as_array)
                        .is_some_and(|items| {
                            items.iter().any(|item| {
                                item["type"] == "function_call_output"
                                    && item
                                        .get("output")
                                        .and_then(Value::as_str)
                                        .and_then(|value| serde_json::from_str::<Value>(value).ok())
                                        .is_some_and(|value| {
                                            value.pointer("/answers/choice/answers")
                                                == Some(&json!(["Second"]))
                                        })
                            })
                        })
                });
            assert!(
                received,
                "the actual native model continuation must contain the selected answer"
            );
            println!(
                "{}",
                json!({"native_question":true,"adapter":"codex","version":started.harness_version,"separated":separated,"mode":format!("{mode:?}"),"answer_returned":true,"source_correlated":true})
            );
        }
        if let Some(proxy) = &proxy {
            assert!(
                proxy.exec_connections() > 0,
                "the separated harness must actually connect to its executor"
            );
        }
        rig.adapter.stop().await?;
        if let Some(peer) = peer {
            peer.stop().await?;
        }
    }
    Ok(())
}
