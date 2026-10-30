//! Isolated native build runner, using the adapter tests' shared cases and the scripted model.
//!
//! The program under test is looked up in the registry of programs, which says which cases a build has to pass and
//! which protocol adapter drives it. The adapter is made here, the composition root of the node, by that protocol:
//! the rest is the shared cases and what the adapter says through its hooks.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clap::Args as ClapArgs;
use orch_adapter_claude_code::{
    ClaudeCodeAdapter,
    process::{DirectLauncher as ClaudeDirectLauncher, LaunchSpec, Launcher as ClaudeLauncher},
};
use orch_adapter_codex::{
    CodexAdapter,
    process::{
        DirectLauncher as CodexDirectLauncher, HarnessProcess, LaunchError, LaunchRequest,
        Launcher as CodexLauncher,
    },
};
use orch_agent_contract::adapter::{AgentAdapter, AgentStart};
use orch_agent_contract::facts::AgentProfile;
use orch_agent_contract::loopback::Loopback;
use orch_agent_programs::{ProgramManifest, Programs};
use orch_conformance::native::{self, Fixture, ModelRequest};
use orch_link_contract::commands::NodeToolConformanceResult;
use orch_testkit::doubles::model::{MockConfig, MockLlm};
use orch_types::{SystemClock, new_id};
use serde_json::json;

/// Arguments accepted only by the node's internal isolated runner.
#[derive(Debug, Clone, ClapArgs)]
pub struct Args {
    /// Harness kind.
    #[arg(long)]
    pub kind: String,
    /// Exact build version.
    #[arg(long)]
    pub version: String,
    /// Executable from the read-only cached build mount.
    #[arg(long)]
    pub program: PathBuf,
    /// Case deadline supplied by the controller's setting.
    #[arg(long)]
    pub case_timeout_secs: u32,
    /// Fresh scratch directory, inside the disposable unit.
    #[arg(long)]
    pub scratch: PathBuf,
    /// Run only one informational probe, outside the background certification command.
    #[arg(long)]
    pub probe: Option<String>,
    /// Print controlled native and neutral frames to stderr for an explicit proof.
    #[arg(long)]
    pub evidence: bool,
}

#[derive(Debug)]
struct ClaudeLaunches(Arc<Mutex<Vec<BTreeMap<String, String>>>>);

struct StrictCodex;

#[async_trait::async_trait]
impl CodexLauncher for StrictCodex {
    async fn launch(&self, mut request: LaunchRequest) -> Result<HarnessProcess, LaunchError> {
        if request.args.first().map(String::as_str) == Some("app-server") {
            request.args.insert(1, "--strict-config".to_owned());
        }
        CodexDirectLauncher.launch(request).await
    }
}

impl ClaudeLauncher for ClaudeLaunches {
    fn command(&self, spec: &LaunchSpec) -> tokio::process::Command {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(spec.env.clone());
        ClaudeDirectLauncher.command(spec)
    }
}

/// The adapter that drives `program` for a check, by the protocol its manifest names; `launches` records the
/// environment of every launch of the program.
fn adapter_for(
    program: &ProgramManifest,
    launches: &Arc<Mutex<Vec<BTreeMap<String, String>>>>,
) -> Option<Box<dyn AgentAdapter>> {
    match program.protocol.as_str() {
        "codex_app_server" => Some(Box::new(CodexAdapter::with_launcher(Arc::new(StrictCodex)))),
        "claude_stream_json" => Some(Box::new(ClaudeCodeAdapter::with_launcher(
            Arc::new(SystemClock),
            Arc::new(ClaudeLaunches(launches.clone())),
        ))),
        _ => None,
    }
}

/// Runs every shared case and writes only the structured native result to standard output.
///
/// # Errors
/// The isolated scratch space or scripted service could not be created.
pub async fn run(args: &Args) -> anyhow::Result<()> {
    let program = Programs::builtin()
        .program(&args.kind)
        .ok_or_else(|| anyhow::anyhow!("no program {} is known to this node", args.kind))?;
    anyhow::ensure!(
        !native::cases(&program.checks).is_empty(),
        "no native cases for {}",
        args.kind
    );
    anyhow::ensure!(
        args.case_timeout_secs > 0,
        "the case deadline must be positive"
    );
    std::fs::create_dir_all(&args.scratch)?;
    let mut cases = Vec::new();
    let selected = if let Some(probe) = args.probe.as_deref() {
        anyhow::ensure!(
            native::probes(&program.checks)
                .iter()
                .any(|each| each == probe),
            "unknown informational probe"
        );
        vec![probe]
    } else {
        native::cases(&program.checks)
            .iter()
            .map(|(name, _)| *name)
            .collect()
    };
    for name in selected {
        let scratch = tempfile::Builder::new()
            .prefix("case-")
            .tempdir_in(&args.scratch)?;
        let home = scratch.path().join("home");
        let work = scratch.path().join("work");
        std::fs::create_dir(&home)?;
        std::fs::create_dir(&work)?;
        let token = format!("mock-{}", new_id().simple());
        let mut model = None;
        // Each controlled model lives only as long as its case.
        for port in 41000..=41999 {
            if let Ok(server) = MockLlm::start_on(
                ([127, 0, 0, 1], port).into(),
                MockConfig::default().with_credential(&token),
            )
            .await
            {
                model = Some(server);
                break;
            }
        }
        let model = model.ok_or_else(|| anyhow::anyhow!("no controlled model port is free"))?;
        let within = Duration::from_secs(u64::from(args.case_timeout_secs));
        let server = if matches!(name, "elicitation" | "tools.result_form") {
            Some(native::elicitation::Server::start(model.addr(), within).await?)
        } else {
            None
        };
        let port = server
            .as_ref()
            .map_or(model.addr().port(), |server| server.address.port());
        let launches = Arc::new(Mutex::new(Vec::new()));
        let mut adapter = adapter_for(program, &launches).ok_or_else(|| {
            anyhow::anyhow!(
                "this node has no adapter for the protocol {} of {}",
                program.protocol,
                program.kind
            )
        })?;
        let requests = || {
            model
                .requests()
                .into_iter()
                .map(|request| ModelRequest {
                    path: request.path,
                    body: request.body,
                })
                .collect()
        };
        let launched = || {
            launches
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        };
        let elicited = || {
            server
                .as_ref()
                .map_or_else(Vec::new, |server| server.seen())
        };
        let mut profile_env = BTreeMap::from([("HOME".to_owned(), home.display().to_string())]);
        profile_env.extend(adapter.native_check().profile_env);
        let observe = |event: &orch_agent_contract::events::SessionEvent| {
            eprintln!("{}", json!({"case":name,"event":event}));
        };
        let fixture = Fixture {
            start: AgentStart {
                provider_id: new_id(),
                agent_id: new_id(),
                harness_kind: args.kind.clone(),
                profile: AgentProfile {
                    config: json!({}),
                    env: profile_env,
                    ..AgentProfile::default()
                },
                home: Some(home.display().to_string()),
                provider_config: json!({}),
                loopback: Loopback::new(port, orch_plugin_secret(&token)),
                binary: Some(args.program.display().to_string()),
            },
            version: args.version.clone(),
            checks: &program.checks,
            workspace: work,
            within,
            requests: &requests,
            launches: &launched,
            elicitation: &elicited,
            observe: if args.evidence { Some(&observe) } else { None },
        };
        cases.push(if args.probe.is_some() {
            native::run_probe(adapter.as_mut(), &fixture, name).await
        } else {
            native::run_case(adapter.as_mut(), &fixture, name).await
        });
    }
    println!(
        "{}",
        serde_json::to_string(&NodeToolConformanceResult { cases })?
    );
    Ok(())
}

fn orch_plugin_secret(token: &str) -> orch_plugin_contract::Secret {
    orch_plugin_contract::Secret::new(token)
}
