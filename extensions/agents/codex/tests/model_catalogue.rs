//! The native catalogue is tested before using it to carry an admitted provider's model details.
//!
//! Two catalogues disagree about the same namespaced model. The startup catalogue supplies model
//! details, while the image-view tool is independently controlled by its feature gate. A thread-file
//! override must not be mistaken for effective process metadata. A new process validates the second file.
//! Only the existing loopback model double and a newly generated home are used.

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use common::{Rig, WAIT};
use orch_adapter_codex::process::{self, LaunchRequest, Launcher, ProcessControl, StderrTail};
use orch_adapter_codex::protocol::{self, Incoming, Rpc};
use orch_agent_contract::flow;
use serde_json::json;

type ProbeResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

// A bundled suffix would otherwise supply vision and Lite metadata. The explicit catalogue must win.
const MODEL: &str = "openai/gpt-6-sol";
const PROVIDER: &str = "catalogue_probe";
const TOKEN_VARIABLE: &str = "ORCH_CATALOGUE_PROBE_TOKEN";
// Distinct, small test windows identify the actual supplier without triggering automatic compaction.
const TEXT_WINDOW: i64 = 40_000;
const IMAGE_WINDOW: i64 = 80_000;

fn probe_config(catalogue: &Path, base_url: &str) -> toml::Table {
    let options = process::home_options(&json!({}), true);
    process::effective_config(
        &options,
        &json!({
            "model_catalog_json": catalogue,
            "model_provider": PROVIDER,
            "model_providers": {(PROVIDER): {
                "name": "Catalogue probe",
                "base_url": base_url,
                "wire_api": "responses",
                "env_key": TOKEN_VARIABLE,
                "requires_openai_auth": false,
                "supports_websockets": false
            }},
            "web_search": "disabled"
        }),
    )
}

fn probe_launch(home: &Path, workspace: &Path, token: &str) -> LaunchRequest {
    LaunchRequest {
        program: process::DEFAULT_PROGRAM.to_owned(),
        args: process::app_server_args(&[]),
        env: BTreeMap::from([
            (
                process::HOME_VARIABLE.to_owned(),
                home.to_string_lossy().into_owned(),
            ),
            (TOKEN_VARIABLE.to_owned(), token.to_owned()),
        ]),
        cwd: Some(workspace.to_owned()),
    }
}

#[test]
fn the_catalogue_probe_uses_its_declared_fixture_credential_and_http_only() -> ProbeResult<()> {
    let config = probe_config(
        Path::new("fixture/models.json"),
        "http://127.0.0.1:41700/v1",
    );
    let request = probe_launch(
        Path::new("fixture/home"),
        Path::new("fixture/workspace"),
        "synthetic-catalogue-token",
    );
    assert_eq!(
        config.get("model_provider").and_then(toml::Value::as_str),
        Some(PROVIDER)
    );
    let provider = config
        .get("model_providers")
        .and_then(toml::Value::as_table)
        .and_then(|providers| providers.get(PROVIDER))
        .and_then(toml::Value::as_table)
        .ok_or("the probe's selected provider entry")?;
    let env_key = provider
        .get("env_key")
        .and_then(toml::Value::as_str)
        .ok_or("the provider's env_key")?;
    assert_eq!(
        request.env.get(env_key).map(String::as_str),
        Some("synthetic-catalogue-token")
    );
    assert_eq!(
        provider.get("base_url").and_then(toml::Value::as_str),
        Some("http://127.0.0.1:41700/v1")
    );
    assert_eq!(
        provider.get("wire_api").and_then(toml::Value::as_str),
        Some("responses")
    );
    assert_eq!(
        provider
            .get("requires_openai_auth")
            .and_then(toml::Value::as_bool),
        Some(false)
    );
    assert_eq!(
        provider
            .get("supports_websockets")
            .and_then(toml::Value::as_bool),
        Some(false)
    );
    assert!(!toml::to_string(&config)?.contains("synthetic-catalogue-token"));
    assert!(
        !request
            .args
            .iter()
            .any(|arg| arg.contains("synthetic-catalogue-token"))
    );
    assert!(!request.env.contains_key("OPENAI_API_KEY"));
    assert!(!request.env.contains_key("OPENAI_BASE_URL"));
    Ok(())
}

fn write_catalogue(path: &Path, image: bool, window: i64) -> ProbeResult<()> {
    let modalities = if image {
        vec!["text", "image"]
    } else {
        vec!["text"]
    };
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&json!({ "models": [{
            "slug": MODEL,
            "display_name": "Catalogue fixture",
            "description": null,
            "supported_reasoning_levels": [],
            "shell_type": "disabled",
            "visibility": "list",
            "supported_in_api": true,
            "priority": 0,
            "additional_speed_tiers": ["fast"],
            "service_tiers": [{"id":"priority", "name":"Fast", "description":"Fixture tier"}],
            "availability_nux": null,
            "upgrade": null,
            "model_messages": {"instructions_template":"Answer the supplied request without invoking tools unless explicitly asked."},
            "include_apps_usage_instructions": false,
            "support_verbosity": false,
            "default_verbosity": null,
            "apply_patch_tool_type": null,
            "truncation_policy": {"mode":"bytes", "limit":10_000},
            "context_window": window,
            "max_context_window": window,
            "effective_context_window_percent": 100,
            "experimental_supported_tools": [],
            "input_modalities": modalities,
            "use_responses_lite": false
        }] }))?,
    )?;
    Ok(())
}

struct NativeProbe {
    rpc: Rpc,
    incoming: flow::Receiver<Incoming>,
    control: Box<dyn ProcessControl>,
    diagnostics: StderrTail,
    stderr: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for NativeProbe {
    fn drop(&mut self) {
        self.rpc.finish();
        self.control.kill();
        if let Some(stderr) = self.stderr.take() {
            stderr.abort();
        }
        let diagnostics = self.diagnostics.text();
        if !diagnostics.is_empty() {
            eprintln!("catalogue probe native diagnostics: {diagnostics}");
        }
    }
}

impl NativeProbe {
    async fn start(rig: &Rig, catalogue: &Path) -> ProbeResult<Self> {
        let base_url = format!("http://{}/v1", rig.mock.addr());
        let config = probe_config(catalogue, &base_url);
        process::prepare_home(rig.home.path())?;
        process::write_home_config(rig.home.path(), &config)?;
        let launched = rig
            .launcher
            .launch(probe_launch(
                rig.home.path(),
                rig.workspace.path(),
                &rig.token,
            ))
            .await?;
        let (rpc, incoming) = protocol::connect(launched.stdout, launched.stdin, None);
        let diagnostics = StderrTail::default();
        let stderr = launched
            .stderr
            .map(|stderr| tokio::spawn(diagnostics.clone().follow(stderr)));
        let probe = Self {
            rpc,
            incoming,
            control: launched.control,
            diagnostics,
            stderr,
        };
        tokio::time::timeout(
            WAIT,
            probe.rpc.call(
                "initialize",
                json!({
                    "clientInfo": {"name":"orch_catalogue_probe", "title":"Catalogue probe", "version":"1.0.0"},
                    "capabilities": {"experimentalApi":true}
                }),
            ),
        )
        .await??;
        probe.rpc.notify("initialized", None).await?;
        Ok(probe)
    }

    async fn turn(
        &mut self,
        rig: &Rig,
        catalogue_override: Option<&Path>,
        marker: &str,
        view_image: Option<bool>,
    ) -> ProbeResult<i64> {
        tokio::time::timeout(WAIT, async {
            let mut config = json!({"web_search":"disabled"});
            if let Some(enabled) = view_image {
                config["features.view_image"] = json!(enabled);
            }
            if let Some(path) = catalogue_override {
                config["model_catalog_json"] = json!(path);
            }
            let opened = self
                .rpc
                .call(
                    "thread/start",
                    json!({
                        "model":MODEL,
                        "modelProvider":PROVIDER,
                        "cwd":rig.workspace.path(),
                        "approvalPolicy":"never",
                        "sandbox":"danger-full-access",
                        "config":config
                    }),
                )
                .await?;
            let thread = opened["thread"]["id"]
                .as_str()
                .ok_or("thread/start did not name a thread")?;
            let started = self
                .rpc
                .call(
                    "turn/start",
                    json!({
                        "threadId":thread,
                        "input":[{"type":"text", "text":marker, "text_elements":[]}],
                        "model":MODEL,
                        "serviceTierForTurn":"priority"
                    }),
                )
                .await?;
            let turn = started["turn"]["id"]
                .as_str()
                .ok_or("turn/start did not name a turn")?;
            let mut window = None;
            loop {
                match self.incoming.recv().await {
                    Some(Incoming::Notification { method, params })
                        if params["threadId"] == thread =>
                    {
                        if method == "thread/tokenUsage/updated" && params["turnId"] == turn {
                            window = params["tokenUsage"]["modelContextWindow"].as_i64();
                        } else if method == "turn/completed" && params["turn"]["id"] == turn {
                            assert_eq!(params["turn"]["status"], "completed", "{params}");
                            return window
                                .ok_or_else(|| "native usage did not report a window".into());
                        }
                    }
                    Some(Incoming::Request { id, method, .. }) => {
                        self.rpc
                            .respond_error(&id, -32601, "Unexpected request in catalogue probe")
                            .await?;
                        return Err(format!("unexpected native request: {method}").into());
                    }
                    Some(Incoming::Closed { reason }) => return Err(reason.into()),
                    None => return Err("native notification stream ended".into()),
                    _ => {}
                }
            }
        })
        .await?
    }
}

fn assert_request(rig: &Rig, marker: &str, image: bool) -> ProbeResult<()> {
    let requests = rig.model_requests();
    let request = requests
        .iter()
        .find(|request| request.body.to_string().contains(marker))
        .ok_or("the native turn did not reach the scripted Responses endpoint")?;
    assert_eq!(request.body["model"], MODEL);
    assert_eq!(
        request.credential.verdict,
        orch_testkit::doubles::model::Verdict::Accepted,
        "the probe must authenticate before its catalogue metadata can be observed"
    );
    assert_eq!(request.body["service_tier"], "priority");
    let tools = request.body["tools"]
        .as_array()
        .ok_or("the native request did not carry a tool list")?;
    assert_eq!(
        tools.iter().any(|tool| tool["name"] == "view_image"),
        image,
        "view_image is independently offered by the native feature gate, not input_modalities"
    );
    let input = request.body["input"]
        .as_array()
        .ok_or("the native request did not carry input items")?;
    assert!(
        input.iter().any(|item| item["type"] == "message")
            && !input.iter().any(|item| item["type"] == "additional_tools"),
        "the explicit catalogue must not inherit the bundled suffix's Lite request shape"
    );
    Ok(())
}

#[tokio::test]
async fn catalogue_details_stay_process_scoped_and_the_image_tool_has_its_own_gate()
-> ProbeResult<()> {
    let Some(rig) = Rig::start("catalogue-scope").await else {
        return Ok(());
    };
    let text = rig.home.path().join("text-models.json");
    let image = rig.home.path().join("image-models.json");
    write_catalogue(&text, false, TEXT_WINDOW)?;
    write_catalogue(&image, true, IMAGE_WINDOW)?;

    let mut native = NativeProbe::start(&rig, &text).await?;
    let marker = "catalogue_startup_text";
    assert_eq!(native.turn(&rig, None, marker, None).await?, TEXT_WINDOW);
    assert_request(&rig, marker, true)?;

    let marker = "catalogue_thread_override";
    assert_eq!(
        native.turn(&rig, Some(&image), marker, None).await?,
        TEXT_WINDOW,
        "accepting a thread config must not be confused with using its catalogue"
    );
    assert_request(&rig, marker, true)?;
    let marker = "catalogue_text_with_explicit_image_gate";
    assert_eq!(
        native.turn(&rig, None, marker, Some(false)).await?,
        TEXT_WINDOW
    );
    assert_request(&rig, marker, false)?;
    drop(native);

    let mut native = NativeProbe::start(&rig, &image).await?;
    let marker = "catalogue_restart_image";
    assert_eq!(native.turn(&rig, None, marker, None).await?, IMAGE_WINDOW);
    assert_request(&rig, marker, true)?;
    Ok(())
}

#[tokio::test]
async fn admitted_model_catalogues_keep_concurrent_native_windows_tiers_and_image_gates()
-> ProbeResult<()> {
    use orch_agent_contract::adapter::AgentAdapter;
    use orch_agent_contract::events::SessionEvent;
    use orch_agent_contract::session::{NativeModelContext, SessionMode};
    use orch_provider_contract::models::{ModelParameters, ServiceTier};
    use orch_types::new_id;
    let Some(mut rig) = Rig::start("admitted-model-catalogues").await else {
        return Ok(());
    };
    rig.boot().await;
    let first = new_id();
    let second = new_id();
    let mut specs = Vec::new();
    for (thread, image, window) in [(first, false, TEXT_WINDOW), (second, true, IMAGE_WINDOW)] {
        let mut spec = rig.spec(thread, SessionMode::New, None);
        spec.settings.model = MODEL.into();
        spec.settings.available_models = vec![MODEL.into()];
        spec.settings.params = json!({"service_tier":"priority","web_search":"disabled"});
        spec.settings.native_model = Some(Box::new(NativeModelContext {
            model: MODEL.into(),
            parameters: ModelParameters {
                input_images: Some(image),
                context_max: Some(window),
                service_tiers: Some(vec![ServiceTier {
                    id: "priority".into(),
                    label: "Fast".into(),
                }]),
                ..Default::default()
            },
            capabilities: json!([
                {"capability":"generation","native_config":{"model.use_responses_lite":false}},
                {"capability":"image_input","native_config":{"features.view_image":image}}
            ]),
            extensions: BTreeMap::new(),
        }));
        rig.adapter.open_session(spec.clone()).await?;
        specs.push(spec);
    }
    for (index, marker) in [
        (0, "admitted_text_first"),
        (1, "admitted_image"),
        (0, "admitted_text_after_other_model"),
    ] {
        let spec = &specs[index];
        let mut turn = rig.turn(spec.settings.thread_id, marker);
        turn.model = MODEL.into();
        turn.params = spec.settings.params.clone();
        turn.native_model = spec.settings.native_model.clone();
        rig.adapter.start_turn(turn).await?;
        let events = rig.until_turn_ends().await;
        let windows: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::UsageUpdated(update) => update.context_window,
                _ => None,
            })
            .collect();
        let expected = if index == 0 {
            TEXT_WINDOW
        } else {
            IMAGE_WINDOW
        };
        assert!(
            windows.contains(&expected),
            "the native usage must name its admitted {expected} window: {events:?}"
        );
        assert_request(&rig, marker, index == 1)?;
    }
    rig.adapter.stop().await?;
    Ok(())
}
