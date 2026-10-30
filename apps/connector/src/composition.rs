//! Compose connector roles, adapters, executors, model protocols and environment host.
//!
//! Everything concrete is chosen here and nowhere else: the libraries below this program take what they
//! are given. The program carries the adapters of the harness kinds it can host (the scripted one, Codex and
//! Claude Code) and its Responses, Chat Completions and Messages relays, reads the data directory the
//! machine is enrolled in, starts the connector from it and keeps it
//! running until it is told to stop, renewing the certificate when it is due. Standard error is the log:
//! a service manager keeps it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use orch_adapter_acp::AcpAdapter;
use orch_adapter_acp::process::DirectLauncher;
use orch_adapter_claude_code::ClaudeCodeAdapter;
use orch_adapter_codex::CodexAdapter;
use orch_adapter_fake::FakeAdapter;
use orch_agent_contract::adapter::AgentAdapter;
use orch_agent_programs::{ProgramManifest, Programs};
use orch_build_info::BuildInfo;
use orch_connector_core::connector::{
    ConnectorBuilder, ConnectorParams, StartError, Supervision, run_until_with_updates,
};
use orch_connector_core::enrollment::{
    DataDir, EnrollInputs, HttpsEnroll, ManagedInputs, default_data_dir, enroll, role_name,
};
use orch_connector_core::facts::Host;
use orch_connector_core::forwarder::{ProtocolEntry, Protocols};
use orch_connector_core::harnesses::{AdapterRegistration, AdapterRegistry, HomeRule};
use orch_connector_core::roles::environment::EnvironmentParts;
use orch_connector_core::roles::provider::{ProviderParts, plug_in};
use orch_connector_core::session::{detect_platform, tls_dialer};
use orch_envhost::environment::ManagedPlaces;
use orch_executor_codex::server::CodexExecServer;
use orch_executor_contract::executor::Executor;
use orch_executor_orch::server::OrchExec;
use orch_plugin::registration::{BuiltIn, Plugins};
use orch_plugin_contract::registration::{PluginHost, PluginKind, PluginRegistration};
use orch_protocol_anthropic_messages::AnthropicMessages;
use orch_protocol_openai_responses::{OpenAiChat, OpenAiEmbeddings, OpenAiResponses};
use orch_telemetry::LogFormat;
use orch_types::{Clock, SystemClock};

/// The component name in the log.
const COMPONENT: &str = "connector";

/// The environment, as the libraries read it.
fn var(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// How the connector hosts the programs that a protocol adapter of its own drives: the version of that adapter's
/// declarations and how it is made. A program is hosted when its manifest names one of these protocols, so that a
/// program whose protocol is one of them needs a manifest and nothing else.
struct Protocol {
    adapter_version: u32,
    factory: fn(&ProgramManifest) -> Arc<orch_connector_core::harnesses::AdapterFactory>,
}

/// The protocol adapters this program carries, by the name a manifest gives its protocol.
fn protocol(name: &str) -> Option<Protocol> {
    match name {
        "fake" => Some(Protocol {
            adapter_version: orch_adapter_fake::descriptor::ADAPTER_VERSION,
            factory: |manifest| {
                let program = orch_adapter_fake::descriptor::Program::of(manifest);
                Arc::new(move |clock| Box::new(FakeAdapter::with_program(clock, program.clone())))
            },
        }),
        "codex_app_server" => Some(Protocol {
            adapter_version: orch_adapter_codex::descriptor::ADAPTER_VERSION,
            factory: |_| Arc::new(|clock| Box::new(CodexAdapter::new().with_clock(clock))),
        }),
        "claude_stream_json" => Some(Protocol {
            adapter_version: orch_adapter_claude_code::config::ADAPTER_VERSION,
            factory: |_| Arc::new(|clock| Box::new(ClaudeCodeAdapter::new(clock))),
        }),
        // Every program that speaks the Agent Client Protocol is data for the one generic adapter: its manifest, and
        // the launch data the adapter carries for it, if the program is one Orch ships.
        "acp" => Some(Protocol {
            adapter_version: orch_adapter_acp::descriptor::ADAPTER_VERSION,
            factory: |manifest| {
                #[expect(
                    clippy::expect_used,
                    reason = "adapters_for read the launch data of every program of this protocol before it made a factory"
                )]
                let program = orch_adapter_acp::programs::for_manifest(manifest).expect(
                    "the launch data of a program of the Agent Client Protocol was read before",
                );
                Arc::new(move |clock| {
                    Box::new(AcpAdapter::with_launcher(
                        program.clone(),
                        Arc::new(DirectLauncher),
                        clock,
                    ))
                })
            },
        }),
        _ => None,
    }
}

/// Where a program's processes get their homes, as its manifest says.
fn home_rule(manifest: &ProgramManifest) -> HomeRule {
    match manifest.placement.home {
        orch_agent_programs::manifest::HomeRule::Connector => HomeRule::Connector,
        orch_agent_programs::manifest::HomeRule::Program => HomeRule::Harness,
    }
}

/// The adapters of the harness kinds this program can host: every program of the registry whose protocol this
/// program carries an adapter for.
///
/// The scripted adapter needs no program and keeps its home in the data directory. The others need the harness's
/// program: a runtime the system made has it in its tools directory (or in its image), a machine a person
/// connected lists where its owner installed it. Their homes are named by the system for a runtime it made, and
/// are the harnesses' own on a person's machine, where the harness already has its sign-in; the connector's data
/// directory holds a home only when the system asks for one. Each program is named by its manifest: the names
/// its executable answers to, whether it needs a build, where its homes are.
pub fn adapters() -> anyhow::Result<AdapterRegistry> {
    adapters_for(Programs::builtin())
}

/// [`adapters`] for the programs of `programs`: the registry a test makes by adding a manifest to the shipped ones.
pub fn adapters_for(programs: &Programs) -> anyhow::Result<AdapterRegistry> {
    let mut registry = AdapterRegistry::new();
    for manifest in programs.programs() {
        if manifest.protocol == "acp" {
            orch_adapter_acp::programs::for_manifest(manifest)
                .with_context(|| format!("the launch data of {}", manifest.kind))?;
        }
        let carried = protocol(&manifest.protocol).with_context(|| {
            format!(
                "this program has no adapter for the protocol {} of {}",
                manifest.protocol, manifest.kind
            )
        })?;
        let kind = orch_agent_programs::intern(&manifest.kind);
        registry.register(AdapterRegistration {
            kind,
            adapter_version: carried.adapter_version,
            builtin: true,
            needs_binary: manifest.placement.needs_binary,
            home_rule: home_rule(manifest),
            factory: (carried.factory)(manifest),
        })?;
        registry.program_names(kind, &manifest.program_names());
    }
    Ok(registry)
}

/// What the environment role is given: the executors an environment can run (Orch's own, and the exec server of
/// the Codex build the machine or its tool cache holds), and, for a runtime the system made, the places its
/// driver put the workspace, the home, the mounts and the tools in.
pub fn environment_parts(launch: Option<&ManagedInputs>) -> EnvironmentParts {
    let places = launch.and_then(|launch| {
        Some(ManagedPlaces {
            workspace_root: launch.workspace_dir.clone()?,
            home: launch.home_dir.clone(),
            mounts_file: launch.mounts_file.clone(),
            tools_dir: Some(launch.tools_dir.clone()),
            // Report the local kernel's enforcement, including an in-place driver's honest false.
            mounts_enforced: orch_envhost::mounts::driver_mounts_enforced(
                launch.mounts_file.as_deref(),
            ),
        })
    });
    EnvironmentParts {
        executors: vec![
            Arc::new(|| Box::new(OrchExec::new())),
            Arc::new(|| Box::new(CodexExecServer::new())),
        ],
        places,
        ..EnvironmentParts::default()
    }
}

/// The programs from local configuration and the template, checked together before any is started.
fn plugin_registrations(
    config: &orch_config::ConnectorConfig,
    template: Option<&str>,
) -> anyhow::Result<Vec<PluginRegistration>> {
    let mut entries = config
        .plugin
        .iter()
        .map(|plugin| {
            orch_plugin::registration::registration_of(
                &plugin.kind,
                &plugin.name,
                &plugin.command,
                Some(&plugin.connect),
                &plugin.env,
                &plugin.restart,
            )
        })
        .collect::<Result<Vec<_>, _>>()
        .context("reading registered connector programs")?;
    if let Some(template) = template {
        let declared: Vec<PluginRegistration> = serde_json::from_str(template).map_err(|_| {
            anyhow::anyhow!("ORCH_PLUGINS must be a JSON list of plugin registrations")
        })?;
        entries.extend(declared);
    }
    anyhow::ensure!(
        entries
            .iter()
            .all(|entry| entry.kind.host() == PluginHost::Connector),
        "a connector hosts only providers, translators, adapters and executors"
    );
    orch_plugin::registration::hosted_by(PluginHost::Connector, &entries, &plugin_builtin())?;
    Ok(entries)
}

fn plugin_builtin() -> BuiltIn {
    let programs = Programs::builtin();
    BuiltIn::new()
        .with(PluginKind::Adapter, programs.kinds())
        .with(
            PluginKind::Executor,
            programs.executors().map(|executor| executor.kind.as_str()),
        )
        // A program may take neither a built-in source's name nor the word for no source.
        .with(
            PluginKind::Provider,
            orch_provider_contract::choice::builtin::ALL
                .into_iter()
                .chain(["none"]),
        )
}

fn adapters_with_plugins(plugins: &Plugins) -> anyhow::Result<AdapterRegistry> {
    let mut registry = adapters()?;
    for (_, factory) in plugins.adapters() {
        let sample = factory.adapter();
        let descriptor = sample.declared();
        // A program that is not Orch's is a peer of those that are: its manifest says whether it needs a build of
        // its program, which the tool cache gives it like any other's, and where its homes are. One that sends no
        // manifest is made as a program that needs nothing and keeps its homes in the connector's data directory.
        let manifest = sample.manifest();
        let (needs_binary, home) = manifest
            .as_ref()
            .map_or((false, HomeRule::Connector), |manifest| {
                (manifest.placement.needs_binary, home_rule(manifest))
            });
        registry.register(AdapterRegistration {
            kind: sample.kind(),
            adapter_version: descriptor.adapter_version,
            builtin: false,
            needs_binary,
            home_rule: home,
            factory: Arc::new(move |_clock| Box::new(factory.adapter())),
        })?;
        if let Some(manifest) = manifest {
            registry.program_names(sample.kind(), &manifest.program_names());
            registry.set_manifest(sample.kind(), manifest);
        }
    }
    Ok(registry)
}

fn environment_with_plugins(
    launch: Option<&ManagedInputs>,
    plugins: &Plugins,
) -> anyhow::Result<EnvironmentParts> {
    let mut parts = environment_parts(launch);
    let mut kinds = parts
        .executors
        .iter()
        .map(|factory| factory().kind())
        .collect::<std::collections::BTreeSet<_>>();
    for (_, factory) in plugins.executors() {
        let kind = factory.executor().kind();
        anyhow::ensure!(
            kinds.insert(kind),
            "an executor of kind {kind:?} is registered already"
        );
        parts
            .executors
            .push(Arc::new(move || Box::new(factory.executor())));
    }
    Ok(parts)
}

/// The built-in sources of a key's facts this program carries, as the API's program registers them too: the
/// connector makes the vendor reader a provider's choice names, or its default, from this table alone.
fn builtin_sources() -> anyhow::Result<Arc<orch_provider_contract::builtins::BuiltinSources>> {
    Ok(Arc::new(
        orch_provider_contract::builtins::BuiltinSources::new([
            orch_provider_openrouter::builtin(),
            orch_provider_deepseek::builtin(),
        ])?,
    ))
}

#[cfg(test)]
fn provider_sources(
    plugins: &Plugins,
) -> anyhow::Result<Arc<orch_connector_core::sources::Programs>> {
    let mut programs = orch_connector_core::sources::Programs::default();
    for (name, program) in plugins.providers() {
        programs
            .register(&name, program)
            .map_err(anyhow::Error::msg)?;
    }
    let supervised = plugins.clone();
    let programs = programs.with_lookup(Arc::new(move |name| {
        let plugin = supervised.get(PluginKind::Provider, name)?;
        let bridge =
            Arc::new(orch_plugin::bridges::provider::ProviderBridge::new(plugin.clone()).ok()?);
        // Reuse the registry's complete descriptor and namespace checks for a late first declaration.
        let mut checked = orch_connector_core::sources::Programs::default();
        checked.register(name, bridge).ok()?;
        checked.get(name)
    }));
    Ok(Arc::new(programs))
}

#[cfg(test)]
fn plugin_reports(plugins: &Plugins) -> Vec<orch_link_contract::session::MachineFactsPlugin> {
    plugins
        .all()
        .iter()
        .map(|plugin| {
            let report = plugin.report();
            orch_link_contract::session::MachineFactsPlugin {
                kind: report.kind.as_str().to_owned(),
                name: report.name,
                version: report.version,
                state: match report.state {
                    orch_plugin_contract::PluginState::Running => "running",
                    orch_plugin_contract::PluginState::Restarting => "restarting",
                    orch_plugin_contract::PluginState::Failed => "failed",
                }
                .to_owned(),
                descriptor: if plugin.kind() == PluginKind::Provider {
                    plugin.negotiated().and_then(|negotiated| {
                        serde_json::from_value(negotiated.descriptor.clone()).ok()
                    })
                } else {
                    None
                },
                executor: if plugin.kind() == PluginKind::Executor {
                    plugin.negotiated().and_then(|description| {
                        serde_json::from_value(description.descriptor.clone()).ok()
                    })
                } else {
                    None
                },
            }
        })
        .collect()
}

/// The wire protocols the forwarder speaks. Responses, Chat Completions and Embeddings use a base address that
/// ends in `/v1`; the Messages format's paths carry their `/v1` themselves, and its
/// harness is given the address of the session as it is.
pub fn protocols() -> Protocols {
    // What a program adds to its requests that must not leave the machine: the rules of every program of the
    // Agent Client Protocol that Orch ships, which the forwarder holds each call to whichever program makes it.
    let mut strip_headers = Vec::new();
    let mut refuse_members = Vec::new();
    for manifest in Programs::builtin()
        .programs()
        .filter(|manifest| manifest.protocol == "acp")
    {
        match orch_adapter_acp::programs::for_manifest(manifest) {
            Ok(program) => {
                strip_headers.extend(program.privacy.strip_headers);
                refuse_members.extend(program.privacy.refuse_members);
            }
            Err(error) => {
                tracing::error!(%error, kind = %manifest.kind, "the launch data of a program is not readable; its requests are not held to its privacy rules");
            }
        }
    }
    let privacy = orch_connector_core::forwarder::Privacy::new(&strip_headers, &refuse_members);
    Protocols::new()
        .with_privacy(privacy)
        .with(ProtocolEntry::new(OpenAiResponses, "/v1", |names| {
            OpenAiResponses.models_response(names)
        }))
        .with(ProtocolEntry::new(OpenAiChat, "/v1", |names| {
            OpenAiChat.models_response(names)
        }))
        .with(ProtocolEntry::new(OpenAiEmbeddings, "/v1", |names| {
            OpenAiEmbeddings.models_response(names)
        }))
        .with(ProtocolEntry::new(AnthropicMessages, "", |names| {
            AnthropicMessages.models_response(names)
        }))
}

/// The kinds of harness this program can host, in order.
pub fn adapter_kinds() -> anyhow::Result<Vec<String>> {
    Ok(adapters()?
        .facts()
        .into_iter()
        .map(|adapter| adapter.kind)
        .collect())
}

/// Where the data directory is: the one named, or the one a managed runtime was given, or the
/// platform's place for one user's data.
pub fn data_dir_of(named: Option<PathBuf>) -> PathBuf {
    named.unwrap_or_else(|| default_data_dir(var))
}

/// `enroll`: the machine gives the system a key to certify, with the token, and keeps the answer.
pub async fn enroll_machine(
    dir: &Path,
    options: &crate::arguments::Enroll,
    build: BuildInfo,
) -> anyhow::Result<String> {
    orch_telemetry::init_logging(COMPONENT, LogFormat::Text);
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let data = DataDir::open(dir).with_context(|| format!("opening {}", dir.display()))?;
    let config = data.config(var).context("reading config.toml")?;
    let choices = crate::enrollment_options::selected(options, &config)?;
    let name = options
        .name
        .clone()
        .unwrap_or_else(|| Host::detect().hostname);
    let inputs = EnrollInputs {
        address: options.url.clone(),
        token: options.token.clone(),
        name,
        roles: choices.roles.clone(),
        platform: detect_platform(),
        connector_version: build.version.to_owned(),
        managed: false,
    };
    let enrolled = orch_connector_core::enrollment::enroll_with_choices(
        &inputs,
        &data,
        &HttpsEnroll::new(Arc::clone(&clock), None),
        clock.as_ref(),
        &choices,
    )
    .await
    .context("enrolling")?;
    Ok(format!(
        "Enrolled {} ({}) in {}.\nThe machine plays the roles {}; `orch-connector run` holds the session.",
        enrolled.record.name,
        enrolled.record.machine_id,
        enrolled.record.system.name,
        inputs
            .roles
            .iter()
            .map(role_name)
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// `run`: starts the connector and keeps it running until the process is asked to stop.
pub async fn run(dir: Option<PathBuf>, managed: bool, build: BuildInfo) -> anyhow::Result<()> {
    let named = dir.is_some();
    let launch = managed
        .then(|| ManagedInputs::from_variables(var))
        .transpose()
        .context("reading the launch variables of this runtime")?;
    let dir = dir.or_else(|| launch.as_ref().map(|launch| launch.data_dir.clone()));
    let dir = data_dir_of(dir);
    let _instance = (!managed)
        .then(|| orch_connector_core::uninstall::InstanceLock::running(&dir))
        .transpose()?;
    let data = DataDir::open(&dir).with_context(|| format!("opening {}", dir.display()))?;
    let Some(_runner) = data
        .try_run_lock()
        .context("opening this machine's runner lease")?
    else {
        println!("This data directory is already in use by its runner.");
        return Ok(());
    };
    if !managed {
        orch_connector_core::uninstall::register_running(
            data.root(),
            &std::env::current_exe()?,
            named,
        )?;
    }
    // The connector's own log goes to standard error and to a file a day under `logs/`, kept the
    // days `[log] keep_days` says; a configuration that cannot be read is reported below, where
    // it is read again for the plugins.
    let keep_days = data.config(var).unwrap_or_default().log.keep_days;
    let logging = if var(orch_telemetry::logging::FILES_ONLY_VARIABLE).as_deref() == Some("1") {
        orch_telemetry::logging::init_files_only
    } else {
        orch_telemetry::logging::init_with_files
    };
    logging(COMPONENT, LogFormat::Text, &dir.join("logs"), keep_days);
    tracing::info!(%build, "starting");
    // What an update on Windows left of the program it replaced.
    if let Ok(program) = std::env::current_exe() {
        orch_connector_core::update::remove_old(&program);
    }
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let shutdown = stopped().context("registering process stop signals")?;
    let transport = HttpsEnroll::new(Arc::clone(&clock), None);
    if !data.is_enrolled() {
        let Some(launch) = &launch else {
            anyhow::bail!(
                "{} holds no identity: enroll first, with `orch-connector enroll --url <address> --token <token>`",
                dir.display()
            );
        };
        let name = Host::detect().hostname;
        let inputs = launch
            .enrollment(&name, detect_platform(), build.version)
            .context("preparing the enrollment of this runtime")?;
        enroll(&inputs, &data, &transport, clock.as_ref())
            .await
            .context("enrolling this runtime")?;
        tracing::info!("enrolled with the token this runtime was given");
    }

    let tools_dir = launch.as_ref().map(|launch| launch.tools_dir.clone());
    let plugin_config = data.config(var).context("reading connector plugins")?;
    crate::enrollment_options::connector_roles(&plugin_config.connector.roles.join(","))?;
    let registrations = plugin_registrations(&plugin_config, var("ORCH_PLUGINS").as_deref())?;
    let plugins = orch_plugin::registration::Plugins::start(
        orch_plugin_contract::registration::PluginHost::Connector,
        &registrations,
        &plugin_builtin(),
        &orch_plugin::supervisor::Options::new(build.version),
    )
    .await
    .context("starting registered connector programs")?;
    // Plugin startup must finish within the same budget as starting a provider runtime.
    let startup = std::time::Duration::from_secs(u64::from(
        orch_link_contract::commands::CommandKind::ProviderStart.deadline_secs(),
    ));
    if tokio::time::timeout(startup, plugins.settled())
        .await
        .is_err()
    {
        plugins.shutdown().await;
        anyhow::bail!("registered programs did not finish starting in time");
    }
    let composition = adapters_with_plugins(&plugins).and_then(|registry| {
        environment_with_plugins(launch.as_ref(), &plugins)
            .map(|environment| (registry, environment))
    });
    let (registry, environment) = match composition {
        Ok(parts) => parts,
        Err(error) => {
            plugins.shutdown().await;
            return Err(error);
        }
    };
    let template = var("ORCH_PLUGINS")
        .map(|text| serde_json::from_str::<Vec<PluginRegistration>>(&text))
        .transpose()
        .context("reading template program registrations")?
        .unwrap_or_default();
    let programs = match crate::programs::Programs::new(
        &plugin_config,
        template,
        plugins.clone(),
        registry.clone(),
        data.config_path(),
        orch_plugin::supervisor::Options::new(build.version),
        plugin_builtin(),
    ) {
        Ok(programs) => Arc::new(programs),
        Err(error) => {
            plugins.shutdown().await;
            anyhow::bail!(error.detail);
        }
    };
    let lookup = Arc::clone(&programs);
    let sources = Arc::new(
        orch_connector_core::sources::Programs::default()
            .with_lookup(Arc::new(move |name| lookup.source(name))),
    );
    let vendors = match builtin_sources() {
        Ok(vendors) => vendors,
        Err(error) => {
            plugins.shutdown().await;
            return Err(error);
        }
    };
    let configured = Arc::clone(&programs);
    let previews = tokio::spawn(async move {
        configured.preview_configured().await;
    });
    let reported = Arc::clone(&programs);
    let plugin_facts = Arc::new(move || reported.reports());
    let make =
        |identity: orch_pki::enrollment::MachineIdentity| -> Result<ConnectorBuilder, StartError> {
            let config = data.config(var)?;
            let dialer = Arc::new(tls_dialer(&identity, Arc::clone(&clock))?);
            let roles =
                crate::enrollment_options::connector_roles(&config.connector.roles.join(","))
                    .map_err(|error| {
                        orch_connector_core::enrollment::EnrollError::Config(error.to_string())
                    })?;
            let provider = ProviderParts::new(
                protocols().with_translators(
                    plugins
                        .translators()
                        .into_iter()
                        .map(|(_, translator)| translator)
                        .collect(),
                ),
                &dir,
                &identity.key,
                Arc::clone(&clock),
            )?;
            let builder = ConnectorBuilder::new(ConnectorParams {
                follow_system_endpoints: true,
                hosted_endpoints: None,
                version: build.version.to_owned(),
                data_dir: dir.clone(),
                dialer,
                endpoints: data.endpoints(&config)?,
                roles: roles.clone(),
                config,
                identity,
                managed: launch.is_some(),
                clock: Arc::clone(&clock),
                platform: detect_platform(),
                adapters: registry.clone(),
                tools_dir: tools_dir.clone(),
                blobs: None,
            })
            .environment(environment.clone())
            .provider_sources(Arc::clone(&sources))
            .builtin_sources(Arc::clone(&vendors))
            .programs(programs.clone())
            .installation(orch_connector_core::installation::connector(&data))
            .plugin_facts(plugin_facts.clone());
            Ok(plug_in(builder, &roles, provider))
        };
    let automatic = launch
        .is_none()
        .then(|| crate::update::Automatic::new(dir.clone(), build))
        .transpose()?;
    let result = run_until_with_updates(
        &data,
        &make,
        &transport,
        Arc::clone(&clock),
        orch_connector_core::connector::RunOptions {
            local_endpoints: None,
            supervision: Supervision::default(),
            updates: automatic
                .as_ref()
                .map(|updates| updates as &dyn orch_connector_core::automatic::Updates),
        },
        &var,
        shutdown,
    )
    .await;
    previews.abort();
    programs.shutdown().await;
    plugins.shutdown().await;
    if matches!(&result, Err(StartError::Updated)) {
        return automatic
            .as_ref()
            .context("a managed runtime cannot self-update")?
            .restart();
    }
    result.context("running the connector")?;
    tracing::info!("stopped");
    Ok(())
}

/// Resolves when the process is asked to stop: an interrupt, or on Unix a termination.
fn stopped() -> std::io::Result<impl std::future::Future<Output = ()>> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut interrupt = signal(SignalKind::interrupt())?;
        let mut terminate = signal(SignalKind::terminate())?;
        Ok(async move {
            tokio::select! { _ = interrupt.recv() => {}, _ = terminate.recv() => {} }
        })
    }
    #[cfg(not(unix))]
    {
        Ok(async {
            let _ = tokio::signal::ctrl_c().await;
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::Method;
    use serde_json::{Value, json};

    /// Thirty seconds allows a loaded machine to start the disposable scripted program without hanging a test.
    const PLUGIN_START_WITHIN: std::time::Duration = std::time::Duration::from_secs(30);

    async fn configured_program(
        kind: PluginKind,
        descriptor: Value,
        mut responses: Value,
        from_template: bool,
    ) -> (tempfile::TempDir, Plugins) {
        let dir = tempfile::tempdir().unwrap();
        responses["describe"] = json!({"result": descriptor.clone()});
        let mut methods = orch_plugin_contract::negotiation::methods(kind)
            .iter()
            .filter(|method| method.required)
            .map(|method| method.name.to_owned())
            .collect::<Vec<_>>();
        methods.extend(responses.as_object().unwrap().keys().cloned());
        methods.sort();
        methods.dedup();
        let script = json!({"kind":kind.as_str(),"name":"test-program","version":"1.0.0",
            "methods":methods,"descriptor":descriptor,"responses":responses});
        let script_path = dir.path().join("script.json");
        std::fs::write(&script_path, script.to_string()).unwrap();
        let command = vec![
            "python3".to_owned(),
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../crates/platform/plugin/tests/support/scripted_plugin.py")
                .display()
                .to_string(),
            "--script".to_owned(),
            script_path.display().to_string(),
        ];
        let entry = PluginRegistration {
            kind,
            name: "test-program".into(),
            command: Some(command.clone()),
            connect: None,
            env: Default::default(),
            restart: Some(orch_plugin_contract::registration::Restart::Never),
        };
        let local = format!(
            "[[plugin]]\nkind = {:?}\nname = {:?}\ncommand = {}\nrestart = 'never'\n",
            kind.as_str(),
            entry.name,
            serde_json::to_string(&command).unwrap()
        );
        let config: orch_config::ConnectorConfig = orch_config::load_text(
            if from_template { "" } else { &local },
            "config.toml",
            |_| None,
        )
        .unwrap();
        let template = serde_json::to_string(&vec![entry]).unwrap();
        let entries =
            plugin_registrations(&config, from_template.then_some(template.as_str())).unwrap();
        let plugins = Plugins::start(
            PluginHost::Connector,
            &entries,
            &plugin_builtin(),
            &orch_plugin::Options::new("1.0.0"),
        )
        .await
        .unwrap();
        // Process startup is bounded even if a broken fixture never says hello.
        tokio::time::timeout(PLUGIN_START_WITHIN, plugins.settled())
            .await
            .unwrap();
        assert_eq!(
            plugins.reports()[0].state,
            orch_plugin_contract::PluginState::Running,
            "{:?}",
            plugins.reports()[0]
        );
        (dir, plugins)
    }

    fn adapter_descriptor(kind: &str) -> Value {
        json!({"kind":kind,"adapter_version":1,
            "harness":{"name":"Tiny","version":"1.0.0","supported_range":">=1.0.0","status":"supported"},
            "arrangements":[{"type":"together"}],"approval_modes":["auto"],"sandbox_modes":["open"]})
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_registered_adapter_is_offered_and_starts_a_harness_from_config_or_template() {
        for from_template in [false, true] {
            let descriptor = adapter_descriptor("tiny");
            let (_dir, plugins) = configured_program(
                PluginKind::Adapter,
                descriptor.clone(),
                json!({"agent.start":{"result":{"harness_version":"1.0.0","descriptor":descriptor}},
                    "agent.stop":{"result":{}}}),
                from_template,
            )
            .await;
            let registry = adapters_with_plugins(&plugins).unwrap();
            assert!(
                registry
                    .facts()
                    .iter()
                    .any(|item| item.kind == "tiny" && !item.builtin)
            );
            let registration = registry.get("tiny").unwrap();
            let mut adapter = (registration.factory)(Arc::new(SystemClock));
            let request = serde_json::from_value(json!({"provider_id":orch_types::new_id(),
                "agent_id":orch_types::new_id(),"harness_kind":"tiny",
                "profile":{"harness_version":null,"config":{},"env":{}},"home":null,
                "provider_config":{},"loopback":{"base_url":"http://127.0.0.1:41000","token":"test-only"},
                "binary":null})).unwrap();
            let (sink, _events) = orch_agent_contract::adapter::EventSink::channel();
            assert_eq!(
                adapter.start(request, sink).await.unwrap().descriptor.kind,
                "tiny"
            );
            adapter.stop().await.unwrap();
            plugins.shutdown().await;
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_registered_executor_is_offered_and_starts_in_an_environment_from_config_or_template()
    {
        for from_template in [false, true] {
            let (_dir,plugins) = configured_program(PluginKind::Executor,
                json!({"kind":"tiny-exec","title":"Tiny executor","protocol":"harness","sandbox_modes":["open"]}),
                json!({"available":{"result":{"available":true,"version":"1.0.0","reason":null,"sandbox_modes":["open"]}},
                    "start":{"result":{"address":"127.0.0.1:41231","version":"1.0.0","tool_sets":[],"sandbox_modes":["open"]}},
                    "stop":{"result":{}}}),from_template).await;
            let parts = environment_with_plugins(None, &plugins).unwrap();
            let mut executor = parts
                .executors
                .iter()
                .map(|factory| factory())
                .find(|executor| executor.kind() == "tiny-exec")
                .unwrap();
            let context = serde_json::from_value(json!({"environment_id":orch_types::new_id(),
                "generation":1,"data_dir":"/test/exec","workspace_root":"/test/work","home":null,
                "tools_dir":null,"tools":[],"mounts":[],"user":"agent",
                "shell":{"name":"sh","path":"/bin/sh"},"os":"linux","arch":"x86_64","path_style":"posix",
                "limits":{},"variables":{},"mcp_servers_enabled":false})).unwrap();
            assert!(executor.available(&context).await.available);
            assert_eq!(
                executor.start(&context).await.unwrap().address,
                "127.0.0.1:41231"
            );
            executor.stop().await.unwrap();
            plugins.shutdown().await;
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_registered_translator_changes_the_forwarded_request_from_config_or_template() {
        for from_template in [false, true] {
            let (_dir,plugins) = configured_program(PluginKind::Translator,
                json!({"from":"alpha","to":"beta","title":"Alpha to Beta","summary":"Converts a test request",
                    "losses":{"preserved":["messages"],"approximated":[],"dropped":[]}}),
                json!({"translate_request":{"result":{"method":"POST","path":"/translated","body":"converted","authorized":false}}}),
                from_template).await;
            let translators = plugins.translators();
            assert_eq!(translators.len(), 1);
            let translator = &translators[0].1;
            let answer = translator
                .translate_request(
                    "/input",
                    bytes::Bytes::from_static(b"original"),
                    &orch_model_contract::translator::UpstreamSide {
                        base_url: "https://test.invalid".into(),
                        options: json!({}),
                        credential: None,
                    },
                )
                .unwrap();
            assert_eq!(answer.path, "/translated");
            assert_eq!(answer.body, bytes::Bytes::from_static(b"converted"));
            plugins.shutdown().await;
        }
    }

    #[test]
    fn template_programs_cannot_replace_local_or_builtin_names_and_invalid_json_is_not_printed() {
        let local: orch_config::ConnectorConfig = orch_config::load_text(
            "[[plugin]]\nkind='adapter'\nname='custom'\ncommand=['program']\n",
            "config.toml",
            |_| None,
        )
        .unwrap();
        let duplicate = r#"[{"kind":"adapter","name":"custom","command":["other"]}]"#;
        assert!(
            plugin_registrations(&local, Some(duplicate))
                .unwrap_err()
                .to_string()
                .contains("two plugins")
        );
        assert!(
            plugin_registrations(
                &Default::default(),
                Some(r#"[{"kind":"executor","name":"orch-exec","command":["other"]}]"#)
            )
            .is_err()
        );
        assert!(
            plugin_registrations(
                &Default::default(),
                Some(r#"[{"kind":"driver","name":"other","command":["other"]}]"#)
            )
            .is_err()
        );
        let error = plugin_registrations(&Default::default(), Some("secret-fixture-invalid-json"))
            .unwrap_err();
        assert!(!error.to_string().contains("secret-fixture"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_registered_provider_tells_its_plan_from_config_or_template() {
        use orch_provider_contract::source::FactsSource;
        for from_template in [false, true] {
            let (_dir,plugins) = configured_program(PluginKind::Provider,
                json!({"name":"test-program","title":"A custom plan","summary":"Reports a custom plan",
                    "contract":1,"facts":{"reports":["plan","windows"],"credential":"none"},
                    "models":null,"presents":null,"fields":[]}),
                json!({"facts.read":{"result":{"plan":"custom","windows":[]}}}),from_template).await;
            let sources = provider_sources(&plugins).unwrap();
            let program = sources.get("test-program").unwrap();
            let ask = serde_json::from_value(
                json!({"provider":{"id":orch_types::new_id(),"name":"Custom"},
                "endpoints":[],"config":{},"secrets":{}}),
            )
            .unwrap();
            let facts = FactsSource::read_facts(program.as_ref(), ask)
                .await
                .unwrap();
            assert_eq!(facts.0["plan"], "custom");
            assert_eq!(facts.0["windows"], json!([]));
            let reports = plugin_reports(&plugins);
            assert_eq!(
                (reports[0].kind.as_str(), reports[0].state.as_str()),
                ("provider", "running")
            );
            assert_eq!(
                reports[0].descriptor.as_ref().unwrap().title,
                "A custom plan"
            );
            plugins.shutdown().await;
            assert_eq!(plugin_reports(&plugins)[0].state, "failed");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_configured_provider_can_recover_its_first_failed_start_without_restarting_the_connector()
     {
        use orch_provider_contract::source::FactsSource;
        /// A clock change is noticed promptly; the normal supervisor's restart delay is retained.
        const TICK: std::time::Duration = std::time::Duration::from_millis(5);
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("initially-missing.json");
        let command = vec![
            "python3".into(),
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../crates/platform/plugin/tests/support/scripted_plugin.py")
                .display()
                .to_string(),
            "--script".into(),
            script.display().to_string(),
        ];
        let entry = PluginRegistration {
            kind: PluginKind::Provider,
            name: "test-program".into(),
            command: Some(command),
            connect: None,
            env: Default::default(),
            restart: None,
        };
        let clock = orch_types::ManualClock::new("2026-10-15T10:00:00Z".parse().unwrap());
        let plugins = Plugins::start(
            PluginHost::Connector,
            &[entry],
            &plugin_builtin(),
            &orch_plugin::Options::new("1.0.0")
                .with_clock(Arc::new(clock.clone()))
                .with_timing(orch_plugin::Timing { tick: TICK }),
        )
        .await
        .unwrap();
        tokio::time::timeout(PLUGIN_START_WITHIN, plugins.settled())
            .await
            .unwrap();
        assert_eq!(
            plugins.reports()[0].state,
            orch_plugin_contract::PluginState::Restarting
        );
        let sources = provider_sources(&plugins).unwrap();
        assert!(sources.get("test-program").is_none());
        let descriptor = json!({"name":"test-program","title":"A repaired plan","summary":"Reports a repaired plan",
            "contract":1,"facts":{"reports":["plan"],"credential":"none"},"models":null,"presents":null});
        std::fs::write(
            &script,
            json!({"kind":"provider","name":"test-program","version":"1.0.0",
            "methods":["describe","facts.read"],"descriptor":descriptor,
            "responses":{"facts.read":{"result":{"plan":"repaired"}}}})
            .to_string(),
        )
        .unwrap();
        clock.advance(
            chrono::Duration::from_std(orch_plugin_contract::registration::BACKOFF_FIRST).unwrap(),
        );
        let source = tokio::time::timeout(PLUGIN_START_WITHIN, async {
            loop {
                if plugins.reports()[0].state == orch_plugin_contract::PluginState::Running
                    && let Some(source) = sources.get("test-program")
                {
                    break source;
                }
                tokio::time::sleep(TICK).await;
            }
        })
        .await
        .unwrap();
        let ask = serde_json::from_value(
            json!({"provider":{"id":orch_types::new_id(),"name":"Repaired"},
            "endpoints":[],"config":{},"secrets":{}}),
        )
        .unwrap();
        assert_eq!(
            FactsSource::read_facts(source.as_ref(), ask)
                .await
                .unwrap()
                .0["plan"],
            "repaired"
        );
        plugins.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_adapter_cannot_use_a_custom_registration_name_to_replace_a_builtin_harness() {
        let (_dir, plugins) = configured_program(
            PluginKind::Adapter,
            adapter_descriptor("codex"),
            json!({}),
            false,
        )
        .await;
        assert!(adapters_with_plugins(&plugins).is_err());
        plugins.shutdown().await;
    }

    #[test]
    fn the_program_hosts_the_scripted_harness_and_the_two_real_ones_and_knows_what_claude_code_runs()
     {
        let registry = adapters().unwrap();
        assert_eq!(
            adapter_kinds().unwrap(),
            ["claude-code", "codex", "dsh", "fake"],
            "the kinds a machine says it can host"
        );
        assert_eq!(
            registry.programs_of("claude-code"),
            ["claude", "claude-code"],
            "the program a person installed, and the one a tool cache saves under the name of its tool"
        );
        assert_eq!(registry.programs_of("codex"), ["codex"]);
        assert_eq!(
            registry.programs_of("dsh"),
            [
                "deepseek_harness_runtime/runtime/deepseek-harness-sdk-runtime-linux-x64",
                "dsh"
            ],
            "a program that PyPI carries is found below the directory of its wheel"
        );
        assert!(!registry.get("fake").unwrap().needs_binary);
        assert!(registry.get("codex").unwrap().needs_binary);
        assert!(registry.get("claude-code").unwrap().needs_binary);
    }

    #[test]
    fn the_forwarder_speaks_each_format_and_reads_the_address_each_harness_is_given() {
        let protocols = protocols();
        assert_eq!(
            protocols.names(),
            [
                "openai-responses",
                "openai-chat",
                "openai-embeddings",
                "anthropic-messages"
            ]
        );
        let responses = protocols.get("openai-responses").unwrap();
        // The base address Codex is given ends in /v1; what follows is the format's own path.
        assert_eq!(
            responses.resolve(&Method::POST, "/v1/responses").as_deref(),
            Some("/responses")
        );
        assert_eq!(
            responses.resolve(&Method::POST, "/v1/chat/completions"),
            None
        );
        let chat = protocols.get("openai-chat").unwrap();
        assert_eq!(
            chat.resolve(&Method::POST, "/v1/chat/completions")
                .as_deref(),
            Some("/chat/completions")
        );
        assert_eq!(
            chat.resolve(&Method::DELETE, "/v1/chat/completions/saved"),
            None
        );
        assert_eq!(chat.resolve(&Method::POST, "/v1/responses"), None);
        let messages = protocols.get("anthropic-messages").unwrap();
        assert_eq!(
            messages.resolve(&Method::POST, "/v1/messages").as_deref(),
            Some("/v1/messages")
        );
        assert_eq!(messages.resolve(&Method::POST, "/v1/complete"), None);
    }

    #[test]
    fn every_program_orch_ships_is_hosted_and_a_third_one_needs_a_manifest_and_no_code() {
        let registry = adapters().unwrap();
        for manifest in Programs::builtin().programs() {
            let hosted = registry.get(&manifest.kind).unwrap();
            assert_eq!(hosted.kind, manifest.kind);
            assert_eq!(hosted.needs_binary, manifest.placement.needs_binary);
            assert!(hosted.builtin);
        }
        assert_eq!(
            registry.programs_of("claude-code"),
            ["claude", "claude-code"]
        );
        assert_eq!(registry.programs_of("codex"), ["codex"]);

        let third = orch_testkit::programs::third_manifest();
        let programs = Programs::builtin().with_program(third).unwrap();
        let registry = adapters_for(&programs).unwrap();
        let hosted = registry.get(orch_testkit::programs::THIRD_KIND).unwrap();
        assert!(!hosted.needs_binary);
        let adapter = (hosted.factory)(Arc::new(SystemClock));
        assert_eq!(adapter.kind(), orch_testkit::programs::THIRD_KIND);
        assert_eq!(
            adapter.kind(),
            "fake-third",
            "the scripted adapter is the program its manifest says"
        );
        assert_eq!(registry.programs_of("fake-third"), ["fake-third"]);
    }

    #[test]
    fn the_program_registers_each_vendor_reader_and_a_key_at_its_address_reads_it_by_default() {
        use orch_provider_contract::choice::{Keeper, SourceElement, builtin, default_source};
        let table = builtin_sources().unwrap();
        assert_eq!(
            table.iter().map(|entry| entry.name).collect::<Vec<_>>(),
            builtin::KEY_FACTS
        );
        let openrouter = [url::Url::parse("https://openrouter.ai/api/v1").unwrap()];
        assert_eq!(
            default_source(SourceElement::Facts, Keeper::Key, &openrouter, &table),
            Some(builtin::OPENROUTER)
        );
        let deepseek = [url::Url::parse("https://api.deepseek.com").unwrap()];
        assert_eq!(
            default_source(SourceElement::Facts, Keeper::Key, &deepseek, &table),
            Some(builtin::DEEPSEEK)
        );
    }
}
