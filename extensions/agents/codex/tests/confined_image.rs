//! A pinned native program completes an image turn in the product's disposable Docker unit.
#![cfg(target_os = "linux")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "fail the case at the boundary that did not hold"
)]

mod common;
#[path = "../../../drivers/docker/tests/common/mod.rs"]
mod docker;
use async_trait::async_trait;
use axum::{
    Router,
    extract::State,
    http::{HeaderMap, StatusCode, Uri},
    response::IntoResponse,
};
use base64::Engine as _;
use orch_agent_contract::{
    adapter::{AdapterError, AgentAdapter},
    descriptor::{ApprovalMode, ArrangementType, SandboxMode},
    events::{SessionEvent, TurnOutcome},
    program_work::ProgramIo,
    session::{ProviderAccess, SessionMode},
};
use orch_driver_contract::{
    driver::EnvironmentDriver,
    network::{NetworkTransport, TransportPurpose},
    units::UnitRef,
    work::{WorkFile, WorkProcess, WorkProcessSpec, WorkUnitSpec},
};
use orch_types::{Hash, Time, new_id};
use serde_json::{Value, json};
use std::{
    net::SocketAddr,
    os::unix::fs::PermissionsExt,
    process::Command,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};

const PIXEL: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Wl6B9sAAAAASUVORK5CYII=";

#[tokio::test]
#[ignore = "invoked inside the product proxy sidecar"]
async fn capability_unit_proxy_fixture() {
    docker::serve_capability_proxy().await;
}

struct Model {
    requests: AtomicUsize,
    bearer: String,
}
async fn model(
    State(model): State<Arc<Model>>,
    uri: Uri,
    headers: HeaderMap,
) -> axum::response::Response {
    if !uri.path().ends_with("/responses") {
        return StatusCode::NOT_FOUND.into_response();
    }
    if headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some(model.bearer.as_str())
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let count = model.requests.fetch_add(1, Ordering::SeqCst);
    let item = if count == 0 {
        json!({"type":"image_generation_call","id":"ig_fixture","status":"completed","result":PIXEL,"revised_prompt":"One pixel."})
    } else {
        json!({"type":"message","id":"msg_fixture","role":"assistant","status":"completed","content":[{"type":"output_text","text":"Done.","annotations":[]}]})
    };
    let response = json!({"id":format!("resp_{count}"),"object":"response","model":common::MODEL,"status":"completed","output":[item],"usage":{"input_tokens":8,"output_tokens":3,"total_tokens":11}});
    let mut started = item.clone();
    started["status"] = json!("in_progress");
    if count == 0 {
        started["result"] = Value::Null;
    }
    let events = [
        json!({"type":"response.created","response":{"id":response["id"],"status":"in_progress","output":[]}}),
        json!({"type":"response.output_item.added","output_index":0,"item":started}),
        json!({"type":"response.output_item.done","output_index":0,"item":item}),
        json!({"type":"response.completed","response":response}),
    ];
    let mut sse = String::new();
    use std::fmt::Write;
    for (sequence, mut event) in events.into_iter().enumerate() {
        event["sequence_number"] = json!(sequence);
        writeln!(
            sse,
            "event: {}\ndata: {event}\n",
            event["type"].as_str().unwrap()
        )
        .unwrap();
    }
    ([("content-type", "text/event-stream")], sse).into_response()
}

// The test supplies the physical IO port directly. Only its synthetic provider address is substituted.
struct Unit {
    lab: Arc<docker::Lab>,
    spec: WorkUnitSpec,
    address: SocketAddr,
    unit: OnceLock<UnitRef>,
}
fn refused(error: impl std::fmt::Display) -> AdapterError {
    AdapterError::HarnessUnavailable(error.to_string())
}
#[async_trait]
impl ProgramIo for Unit {
    async fn prepare(&self, mut files: Vec<WorkFile>) -> Result<(), AdapterError> {
        let config = files
            .iter_mut()
            .find(|file| file.path == "home/config.toml")
            .unwrap();
        let mut table: toml::Table =
            toml::from_str(std::str::from_utf8(&config.bytes).unwrap()).unwrap();
        table.insert(
            "chatgpt_base_url".into(),
            toml::Value::String(format!("http://{}/backend-api", self.address)),
        );
        table.insert(
            "openai_base_url".into(),
            toml::Value::String(format!("http://{}/v1", self.address)),
        );
        config.bytes = toml::to_string(&table).unwrap().into_bytes();
        let mut spec = self.spec.clone();
        spec.files = files;
        let unit = self.lab.driver.create_work(&spec).await.map_err(refused)?;
        self.unit
            .set(unit)
            .map_err(|_| refused("the native program prepared a second unit"))
    }
    async fn launch(
        &self,
        mut spec: WorkProcessSpec,
    ) -> Result<Arc<dyn WorkProcess>, AdapterError> {
        assert_eq!(spec.program, "codex");
        spec.program = "/opt/orch/tools/codex/bin/codex".into();
        let process = self
            .lab
            .driver
            .prepare_work_process(self.unit.get().unwrap(), &spec)
            .await
            .map_err(refused)?;
        process.start().await.map_err(refused)?;
        Ok(process)
    }
    async fn read_file(&self, path: &str, max: u64) -> Result<Vec<u8>, AdapterError> {
        self.lab
            .driver
            .read_work_file(self.unit.get().unwrap(), path, max)
            .await
            .map_err(refused)
    }
    async fn retire(&self) -> Result<(), AdapterError> {
        self.lab
            .driver
            .retire_work(self.spec.work, &self.lab.system)
            .await
            .map_err(refused)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pinned_program_completes_an_image_without_workspace_credentials_or_other_network_access()
{
    let mut rig = common::Rig::start_with("confined-image", |launcher| launcher.without_network())
        .await
        .expect("the pinned native image is required");
    let auth = common::fake_auth_json("fixture@example.invalid", "pro", "fixture-account");
    let access: Value = serde_json::from_str(&auth).unwrap();
    std::fs::write(rig.home_path().join("auth.json"), auth.as_bytes()).unwrap();
    let boot = rig.boot_subscription().await;
    assert_eq!(boot.harness_version, common::expected_native_version());

    let lab = Arc::new(docker::capability_proxy_lab().await);
    let cache = lab.path(&format!("tools/codex/{}/bin", boot.harness_version));
    let binary = cache.join("codex");
    // Stream the single installed native executable from the pinned fixture image to the verified tool mount.
    let extracted=Command::new("docker").args(["run","--rm","--network","none","--entrypoint","sh",&common::image(),"-ec",
        "native=$(find /usr/local/lib/node_modules/@openai/codex -type f -name codex -perm /111); test -f \"$native\"; exec cat \"$native\""])
        .stdout(std::fs::File::create(&binary).unwrap()).status().unwrap();
    assert!(extracted.success());
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();

    let bridge = docker::bridge_address(&lab.docker).await.unwrap();
    let server = tokio::net::TcpListener::bind((bridge.as_str(), 0))
        .await
        .unwrap();
    let address = server.local_addr().unwrap();
    let other = docker::Gateway::listen(&bridge);
    let mock = Arc::new(Model {
        requests: AtomicUsize::new(0),
        bearer: format!(
            "Bearer {}",
            access["tokens"]["access_token"].as_str().unwrap()
        ),
    });
    let app = Router::new().fallback(model).with_state(mock.clone());
    let server = tokio::spawn(async move { axum::serve(server, app).await.unwrap() });
    let parent=lab.request("environment","image-source",|request| {
        request["tools"]=json!([{"name":"codex","version":boot.harness_version}]);
        request["env"]=json!({"LAB_RUN":"echo source-only > /workspace/source-only","ORCH_PRIVATE_TEST_KEY":"synthetic-parent-key"});
    });
    let provider = lab.run(&parent).await;
    assert_eq!(
        lab.read(&provider, "/workspace/source-only")
            .await
            .as_deref(),
        Some("source-only\n")
    );
    let work = new_id();
    lab.track_work(work);
    let port = Arc::new(Unit {
        lab: lab.clone(),
        unit: OnceLock::new(),
        address,
        spec: WorkUnitSpec {
            work,
            provider: provider.clone(),
            template: parent.spec.clone(),
            tools: parent.tools.clone(),
            endpoints: vec![NetworkTransport {
                authority: bridge.clone(),
                port: address.port(),
                protocol: "http".into(),
                purpose: TransportPurpose::Model,
            }],
            deadline: Time::new(chrono::Utc::now() + chrono::Duration::minutes(5)),
            files: vec![],
        },
    });
    let helper = new_id();
    let mut spec = rig.spec(helper, SessionMode::New, None);
    spec.provider = ProviderAccess::Native {
        provider_id: rig.provider_id,
    };
    spec.settings.arrangement = ArrangementType::Separated;
    spec.settings.approval_mode = ApprovalMode::Ask;
    spec.settings.sandbox_mode = SandboxMode::ReadOnly;
    spec.settings.workspace_root = "/work".into();
    spec.settings.directory = "/work".into();
    spec.settings.cwd = "/work".into();
    spec.settings.instructions.clear();
    spec.settings.image_generation = Some(true);
    rig.adapter
        .open_program_session(spec, port.clone())
        .await
        .unwrap();
    let unit = port.unit.get().unwrap();
    let probe = format!(
        r#"set -eu
test -r /work/home/auth.json
if cat /workspace/source-only >/dev/null || cat /run/orch/enroll >/dev/null; then exit 1; fi
test -z "${{ORCH_PRIVATE_TEST_KEY-}}${{ORCH_LLM_TOKEN-}}${{ORCH_MCP_TOKEN-}}"
proxy=${{HTTPS_PROXY#http://}}
exec 3<>/dev/tcp/${{proxy%:*}}/${{proxy##*:}}
printf 'CONNECT {bridge}:{} HTTP/1.1\r\n\r\n' >&3
read -r status <&3
[[ "$status" == *403* ]]
if timeout 5 bash -c 'exec 4<>/dev/tcp/{bridge}/{}'; then exit 1; fi
"#,
        other.port, other.port
    );
    let effects = lab
        .driver
        .prepare_work_process(
            unit,
            &WorkProcessSpec {
                program: "/bin/bash".into(),
                args: vec!["-c".into(), probe],
                env: Default::default(),
                cwd: None,
            },
        )
        .await
        .unwrap();
    effects.start().await.unwrap();
    assert_eq!(effects.wait().await.unwrap(), Some(0));
    assert_eq!(other.accepted.load(Ordering::SeqCst), 0);
    let mut turn = rig.turn(helper, "Generate one pixel and return the generated image.");
    turn.approval_mode = ApprovalMode::Ask;
    turn.sandbox_mode = SandboxMode::ReadOnly;
    rig.adapter.start_turn(turn).await.unwrap();
    let events = rig.until_turn_ends().await;
    assert!(events.iter().any(|event|matches!(event,SessionEvent::TurnCompleted(done) if done.status==TurnOutcome::Completed)));
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(PIXEL)
        .unwrap();
    let hash = Hash::of(&bytes);
    assert!(events.iter().any(|event|matches!(event,SessionEvent::ItemCompleted(item) if item.payload["tool"]["capability"]=="image_generation" && item.payload["tool"]["blob"]==json!(hash))));
    assert!(mock.requests.load(Ordering::SeqCst) > 0);
    rig.adapter.close_session(helper).await.unwrap();
    assert_eq!(
        std::fs::read(
            rig.adapter
                .output_directory()
                .unwrap()
                .join(hash.to_string())
        )
        .unwrap(),
        bytes
    );
    assert!(!lab.has_container(&unit.name).await);
    server.abort();
}
