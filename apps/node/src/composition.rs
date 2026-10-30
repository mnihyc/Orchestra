//! Compose the host runtime service with configured environment drivers.
//!
//! Everything concrete is chosen here and in [`crate::docker`], and nowhere else: the library below this
//! program (`orch-node-core`) takes what it is given. The program knows two drivers by name, `docker`, which it
//! carries, and `host`, whose package is not built yet and which it reports as not available when the
//! configuration asks for it. It reads the operator's `orch.toml` (the `[node]` table), the data directory the
//! node is enrolled in, enrolls the host with a token the first time (as the service, it waits in a stated
//! state while the system does not accept the token yet, as a new system does until its first administrator
//! is set up), and then holds the session until it is told to stop. Standard error is its log, which a
//! service manager keeps.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use orch_build_info::BuildInfo;
use orch_config::ServerConfig;
use orch_connector_core::enrollment::{
    DataDir, EnrollError, EnrollTransport, Enrolled, HttpsEnroll,
};
use orch_connector_core::facts::Host;
use orch_link_contract::enrollment::EnrollRefusal;
use orch_node_core::drivers::{DriverHost, Registrations};
use orch_node_core::enrollment::{
    HostEnrollment, Wiring, enroll_host, run_with_updates as run_node,
};
use orch_node_core::images::BlobDesk;
use orch_node_core::role::Prepared;
use orch_node_core::settings::NodeSettings;
use orch_telemetry::LogFormat;
use orch_types::{Clock, SystemClock};

use crate::docker;

/// The component name in the log.
const COMPONENT: &str = "node";

/// The environment, as the libraries read it.
fn var(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// A built-in connection object's field types, with its typed decoder checking nested values.
pub(crate) fn configuration_schema(defaults: &orch_types::Json) -> orch_types::Json {
    use orch_types::Json;
    let properties: serde_json::Map<String, Json> = defaults
        .as_object()
        .into_iter()
        .flatten()
        .map(|(name, value)| {
            let schema = match value {
                Json::Null => serde_json::json!({"type":["string","null"]}),
                Json::Bool(_) => serde_json::json!({"type":"boolean"}),
                Json::Number(_) => serde_json::json!({"type":"integer","minimum":0}),
                Json::String(_) => serde_json::json!({"type":"string"}),
                Json::Array(_) => serde_json::json!({"type":"array"}),
                Json::Object(_) => serde_json::json!({"type":"object"}),
            };
            (name.clone(), schema)
        })
        .collect();
    serde_json::json!({"type":"object","properties":properties,"additionalProperties":false})
}

/// The drivers this program carries, by the name the configuration gives them.
#[must_use]
pub fn registrations() -> Registrations {
    Registrations::new()
        .register(docker::NAME, docker::registration(|_| {}))
        .register(
            "host",
            std::sync::Arc::new(|context| {
                Box::pin(async move {
                    let mut settings = context.settings.clone();
                    if let Some(value) = &context.configuration {
                        settings.host = serde_json::from_value(value.clone()).map_err(|error| {
                            orch_driver_contract::driver::DriverError::InvalidSpec(
                                error.to_string(),
                            )
                        })?;
                    }
                    let config = orch_driver_host::HostConfig {
                        base: settings
                            .host
                            .base
                            .clone()
                            .unwrap_or_else(|| context.settings.data_dir.join("host")),
                        connector: context.settings.connector_dir().join("orch-connector"),
                        tools: context.settings.tools_dir(),
                        prefer_scopes: settings.host.scopes,
                        storages: context
                            .settings
                            .storages
                            .iter()
                            .map(|storage| orch_driver_host::HostStorage {
                                name: storage.name.clone(),
                                path: storage.path.clone(),
                                default: storage.default,
                            })
                            .collect(),
                    };
                    let driver = orch_driver_host::HostDriver::open(
                        config,
                        std::sync::Arc::new(orch_types::SystemClock),
                    )
                    .await?;
                    let schema =
                        configuration_schema(&orch_node_core::machine_policy::host_options(
                            &orch_config::server::NodeHost::default(),
                        ));
                    let checked_schema = schema.clone();
                    Ok(
                        orch_node_core::drivers::HostDriver::new(std::sync::Arc::new(driver))
                            .with_configuration(
                                schema,
                                std::sync::Arc::new(move |value| {
                                    orch_plugin::schema::validate(&checked_schema, value).map_err(
                                        |error| {
                                            orch_driver_contract::driver::DriverError::InvalidSpec(
                                                error.to_string(),
                                            )
                                        },
                                    )?;
                                    serde_json::from_value::<orch_config::server::NodeHost>(
                                        value.clone(),
                                    )
                                    .map_err(|error| {
                                        orch_driver_contract::driver::DriverError::InvalidSpec(
                                            error.to_string(),
                                        )
                                    })?;
                                    Ok(())
                                }),
                            ),
                    )
                })
            }),
        )
}

/// Composes configured external drivers through the same supervised plugin contract as the built-ins.
pub fn driver_registrations(config: &ServerConfig, version: &str) -> anyhow::Result<Registrations> {
    let mut registrations = registrations();
    #[cfg(all(target_os = "linux", feature = "kubernetes"))]
    {
        registrations = crate::kubernetes::register(registrations)?;
    }
    #[cfg(all(not(target_os = "linux"), feature = "kubernetes"))]
    {
        registrations = registrations.not_built(
            "kubernetes",
            "the Kubernetes driver runs on Linux nodes only",
        );
    }
    #[cfg(not(feature = "kubernetes"))]
    {
        registrations = registrations.not_built(
            "kubernetes",
            "the Kubernetes driver is not part of this release",
        );
    }
    let mut names = std::collections::BTreeSet::from([
        "docker".to_owned(),
        "host".to_owned(),
        "kubernetes".to_owned(),
    ]);
    for entry in config.plugin.iter().filter(|entry| entry.kind == "driver") {
        if !names.insert(entry.name.clone()) {
            anyhow::bail!("a driver name is registered twice or belongs to a built-in driver");
        }
        let registration = orch_plugin::registration::registration_of(
            &entry.kind,
            &entry.name,
            &entry.command,
            Some(&entry.connect),
            &entry.env,
            &entry.restart,
        )?;
        registration.start()?;
        let name = entry.name.clone();
        let version = version.to_owned();
        registrations = registrations.register(
            &name,
            std::sync::Arc::new(move |context| {
                let registration = registration.clone();
                let version = version.clone();
                Box::pin(async move {
                    let plugin = orch_plugin::supervisor::Plugin::start(
                        &registration,
                        orch_plugin::supervisor::Options::new(version),
                    )
                    .await
                    .map_err(|_| {
                        orch_driver_contract::driver::DriverError::Unavailable(
                            "the configured external driver could not be started".into(),
                        )
                    })?;
                    // A hello has its own bounded contract deadline; this also bounds a supervisor that never settles.
                    const LOAD_WITHIN: std::time::Duration = std::time::Duration::from_secs(30);
                    tokio::time::timeout(LOAD_WITHIN, plugin.settled())
                        .await
                        .map_err(|_| orch_driver_contract::driver::DriverError::Timeout)?;
                    let bridge = orch_plugin::bridges::driver::DriverBridge::new(plugin.clone())
                        .map_err(|error| {
                            orch_driver_contract::driver::DriverError::Unavailable(
                                error.to_string(),
                            )
                        })?;
                    let driver_name =
                        orch_driver_contract::driver::EnvironmentDriver::describe(&bridge).name;
                    if driver_name != registration.name {
                        plugin.shutdown().await;
                        return Err(orch_driver_contract::driver::DriverError::InvalidSpec(
                            "the registered driver describes a different name".into(),
                        ));
                    }
                    if let Some(configuration) = &context.configuration
                        && let Err(error) = bridge.configure(configuration).await
                    {
                        plugin.shutdown().await;
                        return Err(error);
                    }
                    Ok(orch_node_core::drivers::HostDriver::new(
                        std::sync::Arc::new(bridge),
                    ))
                })
            }),
        );
    }
    Ok(registrations)
}

/// What the operator configured: the `[node]` table of `orch.toml`, with the data directory the command line
/// names in its place when it names one.
pub fn settings_of(
    config_path: &Path,
    data_dir: Option<PathBuf>,
) -> anyhow::Result<(ServerConfig, NodeSettings)> {
    let config: ServerConfig = orch_config::load(config_path)
        .with_context(|| format!("reading {}", config_path.display()))?;
    let mut settings = NodeSettings::from_config(&config);
    if let Some(dir) = data_dir {
        settings.data_dir = dir;
    }
    settings.config_file = Some(config_path.to_owned());
    orch_node_core::machine_policy::configure(&mut settings)?;
    Ok((config, settings))
}

/// The token a person gave: as text, or as the file the bundled deployment mounts it in.
pub fn token_of(token: Option<&str>, file: Option<&Path>) -> anyhow::Result<Option<String>> {
    match (token, file) {
        (Some(token), _) => Ok(Some(token.trim().to_owned())),
        (None, Some(file)) => Ok(Some(
            std::fs::read_to_string(file)
                .with_context(|| format!("reading the token file {}", file.display()))?
                .trim()
                .to_owned(),
        )),
        (None, None) => Ok(None),
    }
}

/// What the node enrolls with.
pub struct Enrollment {
    /// Explicit local consent to hand the node's policy to Orch.
    pub delegate_policy: bool,
    /// The system's address; the configuration's gateway when none is given.
    pub url: Option<String>,
    /// The host token.
    pub token: Option<String>,
    /// What the host is called; the host's name when none is given.
    pub name: Option<String>,
}

/// What the host enrolls with, when it is not enrolled yet; `None` when it is.
fn host_of(
    settings: &NodeSettings,
    data: &DataDir,
    enrollment: &Enrollment,
    again: bool,
) -> anyhow::Result<Option<HostEnrollment>> {
    if data.is_enrolled() && !again {
        return Ok(None);
    }
    let token = enrollment.token.clone().with_context(|| {
        format!(
            "{} holds no identity: enroll first, with `orch-node enroll --url <address> --token <token>`",
            data.root().display()
        )
    })?;
    Ok(Some(HostEnrollment {
        delegate_policy: enrollment.delegate_policy,
        address: enrollment
            .url
            .clone()
            .unwrap_or_else(|| settings.gateway_url.clone()),
        token,
        name: enrollment
            .name
            .clone()
            .unwrap_or_else(|| Host::detect().hostname),
    }))
}

/// The line that says what enrolling did.
fn enrolled_line(enrolled: &Enrolled) -> String {
    format!(
        "Enrolled {} ({}) in {}.",
        enrolled.record.name, enrolled.record.machine_id, enrolled.record.system.name
    )
}

/// Enrolls the host if it is not enrolled yet, with one try. Answers what to say about it, if anything was
/// done.
pub async fn enroll_if_needed(
    settings: &NodeSettings,
    data: &DataDir,
    enrollment: &Enrollment,
    build: BuildInfo,
) -> anyhow::Result<Option<String>> {
    let Some(host) = host_of(settings, data, enrollment, false)? else {
        return Ok(None);
    };
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let enrolled = enroll_host(
        data,
        &host,
        build.version,
        &HttpsEnroll::new(Arc::clone(&clock), None),
        clock.as_ref(),
    )
    .await
    .context("enrolling")?;
    Ok(Some(enrolled_line(&enrolled)))
}

/// How the service waits to enroll while the system does not accept its token yet, or cannot be reached.
#[derive(Debug, Clone, Copy)]
pub struct Patience {
    /// Between two tries.
    pub retry: Duration,
    /// Between two warnings that the node is still waiting.
    pub reminder: Duration,
}

impl Patience {
    /// The service's. A try every ten seconds: the API tries to register the bundled node's token at that
    /// cadence until the first administrator exists, so the node enrolls within about one period of setup,
    /// and a try costs one key and one request. A warning every ten minutes: a node that waits for what
    /// will not come (a token used or run out, a wrong address) says so in the log without filling it.
    pub const SERVICE: Self = Self {
        retry: Duration::from_secs(10),
        reminder: Duration::from_secs(10 * 60),
    };
}

/// Why a try that failed may succeed later with nobody touching the node; `None` when waiting does not
/// change it (a token that is not one, a request the system calls invalid, no protocol in common, an
/// answer not to be trusted, the node's own files).
fn waits_for(error: &EnrollError) -> Option<&'static str> {
    match error {
        // A new system registers the bundled node's token once its first administrator exists; until then
        // it answers the token as it answers a used one.
        EnrollError::Refused(EnrollRefusal::Gone) => Some(
            "the system does not accept this host token yet (a new system accepts the bundled node's token once its first administrator is set up)",
        ),
        EnrollError::Unreachable(_) => Some("the system cannot be reached yet"),
        _ => None,
    }
}

/// Enrolls the host the first time, as the service does: while the system does not accept the token yet,
/// or cannot be reached, the node says so once, tries again every `patience.retry` and warns every
/// `patience.reminder`, rather than ending for its manager to start it again. Answers what was enrolled, or
/// `None` when `stop` came first. Any other failure ends it at once, since waiting does not change it. The
/// token is never written to the log.
pub async fn enroll_waiting(
    data: &DataDir,
    host: &HostEnrollment,
    version: &str,
    transport: &dyn EnrollTransport,
    clock: &dyn Clock,
    patience: Patience,
    stop: impl Future<Output = ()>,
) -> anyhow::Result<Option<Enrolled>> {
    let mut stop = std::pin::pin!(stop);
    let mut warned: Option<tokio::time::Instant> = None;
    loop {
        let error = match enroll_host(data, host, version, transport, clock).await {
            Ok(enrolled) => return Ok(Some(enrolled)),
            Err(error) => error,
        };
        let Some(why) = waits_for(&error) else {
            return Err(anyhow::Error::new(error).context("enrolling"));
        };
        let now = tokio::time::Instant::now();
        match warned {
            None => {
                tracing::info!(
                    reason = %error,
                    retry_seconds = patience.retry.as_secs(),
                    "waiting to enroll: {why}"
                );
                warned = Some(now);
            }
            Some(at) if now.duration_since(at) >= patience.reminder => {
                tracing::warn!(
                    reason = %error,
                    "still waiting to enroll: {why}; if the system was set up already, this token was used or ran out (make a host token and run `orch-node enroll`), or the address is not the system's"
                );
                warned = Some(now);
            }
            Some(_) => tracing::debug!(reason = %error, "still waiting to enroll"),
        }
        tokio::select! {
            () = stop.as_mut() => return Ok(None),
            () = tokio::time::sleep(patience.retry) => {}
        }
    }
}

/// `enroll`: the host gives the system a key to certify, with the token, and keeps the answer.
pub async fn enroll(
    config_path: &Path,
    data_dir: Option<PathBuf>,
    enrollment: &Enrollment,
    build: BuildInfo,
) -> anyhow::Result<String> {
    orch_telemetry::init_logging(COMPONENT, LogFormat::Text);
    let (_, settings) = settings_of(config_path, data_dir)?;
    let data = DataDir::open(&settings.data_dir)
        .with_context(|| format!("opening {}", settings.data_dir.display()))?;
    let host = host_of(&settings, &data, enrollment, true)?.context("no enrollment command")?;
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let enrolled = enroll_host(
        &data,
        &host,
        build.version,
        &HttpsEnroll::new(Arc::clone(&clock), None),
        clock.as_ref(),
    )
    .await
    .context("enrolling")?;
    Ok(enrolled_line(&enrolled))
}

/// `status`: who this host is to the system, and what it was told to load.
pub fn status(config_path: &Path, data_dir: Option<PathBuf>) -> anyhow::Result<String> {
    let (_, settings) = settings_of(config_path, data_dir)?;
    if !DataDir::enrolled_at(&settings.data_dir) {
        return Ok(format!(
            "{} holds no identity: this host is not enrolled.\ndrivers to load: {}",
            settings.data_dir.display(),
            settings.drivers.join(", ")
        ));
    }
    let data = DataDir::open(&settings.data_dir)?;
    let record = data.record()?;
    Ok(format!(
        "{} ({}) in {}, enrolled {}\ndata directory: {}\ndrivers to load: {}",
        record.name,
        record.machine_id,
        record.system.name,
        record.enrolled_at,
        settings.data_dir.display(),
        settings.drivers.join(", ")
    ))
}

/// Local machine policy commands share the connector's store, parser, catalogue and file.
pub fn policy(
    config_path: &Path,
    data_dir: Option<PathBuf>,
    command: &crate::arguments::Policy,
) -> anyhow::Result<String> {
    use crate::arguments::Policy;
    let (_, settings) = settings_of(config_path, data_dir)?;
    let store = orch_node_core::machine_policy::open(&settings)?;
    match command {
        Policy::Show => {}
        Policy::Set { key, value } => store.set_text(key.parse()?, value)?,
        Policy::Delegate { key } => store.delegate(&[if key == "all" {
            "*".into()
        } else {
            key.clone()
        }])?,
        Policy::Undelegate { key } => store.undelegate(&[if key == "all" {
            "*".into()
        } else {
            key.clone()
        }])?,
    }
    Ok(store.describe().join("\n"))
}

/// Ranges, each as its first and last number, as a person reads them.
fn ranges_text(ranges: impl IntoIterator<Item = (u16, u16)>) -> String {
    let written: Vec<String> = ranges
        .into_iter()
        .map(|(from, to)| {
            if from == to {
                from.to_string()
            } else {
                format!("{from}-{to}")
            }
        })
        .collect();
    if written.is_empty() {
        "none".to_owned()
    } else {
        written.join(", ")
    }
}

/// `ports show`: the ceiling of this host's `[ports]` table, and what the node would propose for the host's
/// page now, which binds, reserves and opens nothing.
pub fn ports_show(config_path: &Path) -> anyhow::Result<String> {
    use orch_node_core::host_ports;
    let (_, settings) = settings_of(config_path, None)?;
    let limits = host_ports::limits(&settings.ports);
    let proposal = host_ports::propose(
        &limits,
        &host_ports::taken_now(),
        orch_types::Clock::time(&SystemClock),
    );
    let ceiling_ranges = limits.ranges.as_deref().map_or_else(
        || "no ceiling".to_owned(),
        |ranges| {
            if ranges.is_empty() {
                "none: no host opening is permitted".to_owned()
            } else {
                ranges_text(ranges.iter().map(|range| (range.from, range.to)))
            }
        },
    );
    let mut lines = vec![
        format!(
            "ports (environment role), from {}:",
            settings.data_dir.join("config.toml").display()
        ),
        format!(
            "  incoming address: {}",
            limits.incoming_address.as_deref().unwrap_or("not fixed")
        ),
        format!(
            "  listen: {}",
            limits.listen.as_deref().unwrap_or("no ceiling")
        ),
        format!("  ranges: {ceiling_ranges}"),
        format!("  revision: {}", limits.revision),
        "proposal for the host's page now (it opens nothing):".to_owned(),
        format!(
            "  incoming address: {} ({})",
            proposal
                .values
                .incoming_address
                .as_deref()
                .unwrap_or("the system's"),
            proposal.sources.incoming_address
        ),
        format!(
            "  listen: {} ({})",
            proposal.values.listen, proposal.sources.listen
        ),
        format!(
            "  ranges: {} ({})",
            ranges_text(
                proposal
                    .values
                    .ranges
                    .iter()
                    .map(|range| (range.from, range.to))
            ),
            proposal.sources.ranges
        ),
    ];
    if let Some(reason) = proposal.reason {
        lines.push(format!("  {reason}"));
    }
    Ok(lines.join("\n"))
}

/// One range of `ports set`: `FROM-TO`, or one number.
fn parse_range(text: &str) -> anyhow::Result<(u16, u16)> {
    let number = |part: &str| -> anyhow::Result<u16> {
        part.trim()
            .parse::<u16>()
            .ok()
            .filter(|number| *number > 0)
            .with_context(|| format!("{part} is not a port number (1-65535)"))
    };
    let (from, to) = match text.split_once('-') {
        Some((from, to)) => (number(from)?, number(to)?),
        None => {
            let only = number(text)?;
            (only, only)
        }
    };
    anyhow::ensure!(from <= to, "{text} runs backwards");
    Ok((from, to))
}

/// `text` as a TOML basic string.
#[cfg(test)]
fn toml_string(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for character in text.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            control if control.is_control() => {
                quoted.push_str(&format!("\\u{:04X}", u32::from(control)));
            }
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

/// `text` with its `[ports]` table replaced by `table`, taken away with none, or `table` added at the end
/// where there was none; everything else, comments included, stays as it was.
#[cfg(test)]
fn with_ports_table(text: &str, table: Option<&str>) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 4);
    match lines.iter().position(|line| line.trim() == "[ports]") {
        Some(start) => {
            let end = lines[start + 1..]
                .iter()
                .position(|line| line.trim_start().starts_with('['))
                .map_or(lines.len(), |offset| start + 1 + offset);
            out.extend(lines[..start].iter().map(|line| (*line).to_owned()));
            if let Some(table) = table {
                out.extend(table.lines().map(str::to_owned));
                out.push(String::new());
            }
            out.extend(lines[end..].iter().map(|line| (*line).to_owned()));
        }
        None => {
            out.extend(lines.iter().map(|line| (*line).to_owned()));
            if let Some(table) = table {
                if out.last().is_some_and(|line| !line.trim().is_empty()) {
                    out.push(String::new());
                }
                out.extend(table.lines().map(str::to_owned));
            }
        }
    }
    while out.last().is_some_and(|line| line.trim().is_empty()) {
        out.pop();
    }
    let mut joined = out.join("\n");
    joined.push('\n');
    joined
}

/// `ports set`: keeps the same ports object that policy set uses, validated and watched by the running node.
pub fn ports_set(config_path: &Path, set: &crate::arguments::PortsSet) -> anyhow::Result<String> {
    use orch_link_contract::policy::{LocalPolicy, PolicyDefaults, PolicyKey};
    let (_, settings) = settings_of(config_path, None)?;
    let store = orch_node_core::machine_policy::open(&settings)?;
    let mut ports = LocalPolicy::installed(PolicyDefaults::Managed).ports;
    if !set.clear {
        let ranges = if set.no_ranges {
            Some(Vec::new())
        } else if set.ranges.is_empty() {
            None
        } else {
            Some(
                set.ranges
                    .iter()
                    .map(|range| {
                        parse_range(range)
                            .map(|(from, to)| orch_api_contract::ports::PortRange { from, to })
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?,
            )
        };
        ports.limits = Some(orch_api_contract::ports::HostPortsLimits {
            incoming_address: set
                .incoming_address
                .as_ref()
                .map(|value| value.trim().to_owned()),
            listen: set.listen.as_ref().map(|value| value.trim().to_owned()),
            ranges: ranges.clone(),
            revision: 0,
        });
        if let (Some(address), Some(ranges)) = (&set.incoming_address, ranges)
            && !ranges.is_empty()
        {
            ports.publish = Some(orch_api_contract::ports::HostPorts {
                incoming_address: address.trim().into(),
                listen: set.listen.clone().unwrap_or_else(|| "all".into()),
                ranges,
            });
        }
    }
    store.set_here(PolicyKey::Ports, &serde_json::json!(ports))?;
    Ok(format!(
        "Saved ports in the machine policy.\n{}",
        ports_show(config_path)?
    ))
}

/// `run`: enrolls the host if it is not yet (waiting while the system does not accept the token yet), loads
/// the drivers, and keeps the session until the process is asked to stop.
pub async fn run(
    config_path: &Path,
    data_dir: Option<PathBuf>,
    enrollment: &Enrollment,
    build: BuildInfo,
) -> anyhow::Result<()> {
    let (config, settings) = settings_of(config_path, data_dir)?;
    let _instance = orch_connector_core::uninstall::InstanceLock::running(&settings.data_dir)?;
    orch_telemetry::init_logging(COMPONENT, LogFormat::named(&config.log.format));
    tracing::info!(%build, "starting");
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let data = DataDir::open(&settings.data_dir)
        .with_context(|| format!("opening {}", settings.data_dir.display()))?;
    let Some(_runner) = data
        .try_run_lock()
        .context("opening this node's runner lease")?
    else {
        println!("This data directory is already in use by its runner.");
        return Ok(());
    };
    orch_connector_core::uninstall::register_running(
        data.root(),
        &std::env::current_exe()?,
        false,
    )?;
    let mut stop = std::pin::pin!(stopped().context("registering process stop signals")?);
    if let Some(host) = host_of(&settings, &data, enrollment, false)? {
        let transport = HttpsEnroll::new(Arc::clone(&clock), None);
        match enroll_waiting(
            &data,
            &host,
            build.version,
            &transport,
            clock.as_ref(),
            Patience::SERVICE,
            stop.as_mut(),
        )
        .await?
        {
            Some(enrolled) => tracing::info!("{}", enrolled_line(&enrolled)),
            None => {
                tracing::info!("stopped before enrolling");
                return Ok(());
            }
        }
    }
    // Enrollment may have handed keys over; resolve the same file again before opening drivers.
    let (_, settings) = settings_of(config_path, Some(settings.data_dir.clone()))?;
    let prepared = Prepared::open(&settings).context("opening the files of the node")?;
    let desk = Arc::new(BlobDesk::new());
    let drivers = DriverHost::load(
        &settings,
        driver_registrations(&config, build.version)?,
        Arc::clone(&desk),
    )
    .await;
    if drivers.names().is_empty() {
        tracing::warn!("no driver is loaded: nothing can be scheduled on this host until one is");
    }
    let wiring = Wiring::new(build.version, Arc::clone(&clock), prepared, drivers, desk);
    let updater = orch_connector_core::update::ProgramUpdater::new(
        settings.data_dir.clone(),
        build.version,
        orch_connector_core::update::Program::Node,
    )?;
    orch_connector_core::update::remove_old(&updater.program_path);
    run_node(
        &wiring,
        &data,
        &HttpsEnroll::new(Arc::clone(&clock), None),
        &var,
        Some(&updater),
        stop,
    )
    .await
    .context("running the node")?;
    tracing::info!("stopped");
    Ok(())
}

/// Resolves when the process is asked to stop: an interrupt, or on Unix a termination.
fn stopped() -> std::io::Result<impl std::future::Future<Output = ()>> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        // Registration happens before driver/session startup, even before this future is polled.
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
    use async_trait::async_trait;
    use chrono::TimeZone;
    use orch_link_contract::enrollment::{
        Endpoint, EnrollReply, EnrollReplySystem, EnrollRequest, EnrollmentToken, RenewReply,
        RenewRequest, Trust,
    };
    use orch_pki::ca::CertificateAuthority;
    use orch_pki::enrollment::{
        IssuedToken, MachineIdentity, MemoryLedger, TokenGrant, TokenPurpose, issue_token,
    };
    use orch_types::{ManualClock, new_id};
    use std::sync::Mutex;

    /// A system in memory that knows the host token only from its `adopt_at`-th try on, as a new system
    /// knows the bundled node's token only once its first administrator exists; or that refuses every try
    /// with `refusal`.
    struct System {
        authority: CertificateAuthority,
        ledger: MemoryLedger,
        issued: IssuedToken,
        clock: ManualClock,
        adopt_at: usize,
        refusal: Option<EnrollRefusal>,
        tries: Mutex<usize>,
    }

    impl System {
        fn new(adopt_at: usize, refusal: Option<EnrollRefusal>) -> Self {
            let clock = ManualClock::new(
                chrono::Utc
                    .with_ymd_and_hms(2026, 10, 14, 10, 0, 0)
                    .unwrap(),
            );
            let authority = CertificateAuthority::create("Test", 3650, &clock).unwrap();
            let issued = issue_token(&authority, Duration::from_secs(900), &clock);
            Self {
                authority,
                ledger: MemoryLedger::new(),
                issued,
                clock,
                adopt_at,
                refusal,
                tries: Mutex::new(0),
            }
        }

        fn host(&self) -> HostEnrollment {
            HostEnrollment {
                delegate_policy: false,
                address: "https://orch.test".into(),
                token: self.issued.token.to_string(),
                name: "host-a".into(),
            }
        }

        fn tries(&self) -> usize {
            *self.tries.lock().unwrap()
        }
    }

    #[async_trait]
    impl EnrollTransport for System {
        async fn enroll(
            &self,
            _address: &str,
            _token: &EnrollmentToken,
            request: &EnrollRequest,
        ) -> Result<EnrollReply, EnrollError> {
            let tries = {
                let mut tries = self.tries.lock().unwrap();
                *tries += 1;
                *tries
            };
            if let Some(refusal) = self.refusal {
                return Err(EnrollError::Refused(refusal));
            }
            if tries == self.adopt_at {
                self.ledger.record(
                    &self.issued,
                    TokenGrant {
                        purpose: TokenPurpose::Host,
                        created_by: new_id(),
                    },
                );
            }
            let enrolled = orch_pki::enrollment::enroll(
                &self.authority,
                &self.ledger,
                request,
                365,
                &self.clock,
            )
            .map_err(|error| match error {
                orch_pki::enrollment::EnrollError::Refused(refusal) => {
                    EnrollError::Refused(refusal)
                }
                other => EnrollError::Unreachable(other.to_string()),
            })?;
            Ok(EnrollReply {
                machine_id: enrolled.machine_id,
                certificate_pem: enrolled.certificate.pem(),
                ca_pem: self.authority.certificate_pem(),
                endpoints: vec![Endpoint {
                    url: "wss://gateway.orch.test:9443".into(),
                    trust: Trust::Pinned,
                }],
                system: EnrollReplySystem {
                    name: "Orch".into(),
                    public_url: "https://orch.test".into(),
                },
                protocol_version: enrolled.protocol_version,
            })
        }

        async fn renew(
            &self,
            _endpoint: &Endpoint,
            _identity: &MachineIdentity,
            _request: &RenewRequest,
        ) -> Result<RenewReply, EnrollError> {
            Err(EnrollError::Unreachable(
                "this system renews nothing".into(),
            ))
        }
    }

    /// Short waits, so that a test's tries follow each other quickly; nothing is held to them.
    const QUICK: Patience = Patience {
        retry: Duration::from_millis(5),
        reminder: Duration::from_millis(20),
    };

    #[tokio::test]
    async fn a_current_node_token_keeps_its_policy_local_until_the_enrollment_flag_hands_it_over() {
        use orch_link_contract::{enrollment_choices::EnrollmentChoices, session::Role};
        for handover in [false, true] {
            let mut system = System::new(1, None);
            let choices = EnrollmentChoices::new(
                vec![Role::Node],
                if handover {
                    vec!["*".into()]
                } else {
                    Vec::new()
                },
                false,
            )
            .unwrap();
            system.issued.token = system.issued.token.with_choices(&choices).unwrap();
            system.issued.digest = orch_pki::enrollment::token_digest(system.issued.token.secret());
            let mut host = system.host();
            host.delegate_policy = handover;
            let directory = tempfile::tempdir().unwrap();
            let data = DataDir::open(directory.path()).unwrap();
            enroll_host(&data, &host, "0.1.0", &system, &system.clock)
                .await
                .unwrap();
            let saved = data.config(|_| None).unwrap();
            assert_eq!(saved.connector.roles, ["node"]);
            assert_eq!(
                saved.policy.delegated, choices.delegate,
                "a new token does not imply the older automatic full handover"
            );
        }
    }

    #[tokio::test]
    async fn the_service_waits_while_the_system_does_not_accept_its_token_and_enrolls_once_it_does()
    {
        let system = System::new(3, None);
        let dir = tempfile::tempdir().unwrap();
        let data = DataDir::open(dir.path()).unwrap();
        let enrolled = enroll_waiting(
            &data,
            &system.host(),
            "0.1.0",
            &system,
            &system.clock,
            QUICK,
            std::future::pending(),
        )
        .await
        .unwrap()
        .expect("the host is enrolled once the system accepts its token");
        assert_eq!(system.tries(), 3, "two refusals waited for, then enrolled");
        assert!(data.is_enrolled());
        assert_eq!(enrolled.record.name, "host-a");
    }

    #[tokio::test]
    async fn a_refusal_that_waiting_does_not_change_ends_the_service_at_once() {
        let system = System::new(1, Some(EnrollRefusal::Invalid));
        let dir = tempfile::tempdir().unwrap();
        let data = DataDir::open(dir.path()).unwrap();
        let error = enroll_waiting(
            &data,
            &system.host(),
            "0.1.0",
            &system,
            &system.clock,
            QUICK,
            std::future::pending(),
        )
        .await
        .unwrap_err();
        assert!(format!("{error:#}").starts_with("enrolling"), "{error:#}");
        assert_eq!(system.tries(), 1);
        assert!(!data.is_enrolled());
    }

    #[tokio::test]
    async fn stopping_while_waiting_ends_the_service_without_enrolling() {
        let system = System::new(usize::MAX, None);
        let dir = tempfile::tempdir().unwrap();
        let data = DataDir::open(dir.path()).unwrap();
        let (tell, told) = tokio::sync::oneshot::channel::<()>();
        let host = system.host();
        let waiting = enroll_waiting(
            &data,
            &host,
            "0.1.0",
            &system,
            &system.clock,
            QUICK,
            async {
                let _ = told.await;
            },
        );
        let stopper = async {
            // Stops once the node has been refused at least twice, whenever that is.
            while system.tries() < 2 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            let _ = tell.send(());
        };
        let (answer, ()) = tokio::join!(waiting, stopper);
        assert!(answer.unwrap().is_none());
        assert!(system.tries() >= 2);
        assert!(!data.is_enrolled());
    }

    #[test]
    fn only_an_unknown_token_or_an_unreachable_system_is_waited_for() {
        assert!(waits_for(&EnrollError::Refused(EnrollRefusal::Gone)).is_some());
        assert!(waits_for(&EnrollError::Unreachable("connection refused".into())).is_some());
        assert!(waits_for(&EnrollError::Refused(EnrollRefusal::Invalid)).is_none());
        assert!(waits_for(&EnrollError::Refused(EnrollRefusal::ProtocolUnsupported)).is_none());
        assert!(waits_for(&EnrollError::BadToken).is_none());
    }

    #[test]
    fn the_ports_table_is_replaced_added_or_taken_away_and_nothing_else_moves() {
        let config = "# the operator's note\nversion = 1\n\n[ports]\nlisten = \"all\"\n# about the ranges\nranges = []\n\n[node]\ndata_dir = \"/var/lib/orch-node\"\n";
        let replaced = with_ports_table(config, Some("[ports]\nlisten = \"192.0.2.10\""));
        assert_eq!(
            replaced,
            "# the operator's note\nversion = 1\n\n[ports]\nlisten = \"192.0.2.10\"\n\n[node]\ndata_dir = \"/var/lib/orch-node\"\n"
        );
        let gone = with_ports_table(config, None);
        assert_eq!(
            gone,
            "# the operator's note\nversion = 1\n\n[node]\ndata_dir = \"/var/lib/orch-node\"\n"
        );
        let added = with_ports_table("version = 1\n", Some("[ports]\nranges = []"));
        assert_eq!(added, "version = 1\n\n[ports]\nranges = []\n");
    }

    #[test]
    fn a_range_is_two_numbers_or_one_and_never_backwards() {
        assert_eq!(parse_range("49152-49167").unwrap(), (49_152, 49_167));
        assert_eq!(parse_range("50000").unwrap(), (50_000, 50_000));
        assert!(parse_range("0-10").is_err());
        assert!(parse_range("20-10").is_err());
        assert!(parse_range("a-b").is_err());
        assert_eq!(toml_string("a \"b\" \\c"), "\"a \\\"b\\\" \\\\c\"");
        assert_eq!(ranges_text([(1, 1), (10, 20)]), "1, 10-20");
        assert_eq!(ranges_text(std::iter::empty()), "none");
    }

    #[test]
    fn a_ceiling_set_by_the_command_is_written_read_back_and_shown() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("orch.toml");
        std::fs::write(
            &path,
            format!(
                "# kept\nversion = 1\n[node]\ndata_dir = {:?}\n",
                dir.path().join("machine").to_string_lossy()
            ),
        )
        .unwrap();
        let set = crate::arguments::PortsSet {
            incoming_address: Some("node.example.net".into()),
            listen: Some("all".into()),
            ranges: vec!["50000-50015".into()],
            no_ranges: false,
            clear: false,
        };
        let said = ports_set(&path, &set).unwrap();
        assert!(said.contains("node.example.net"), "{said}");
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.starts_with("# kept\nversion = 1\n"), "{written}");
        let data = DataDir::open(&dir.path().join("machine")).unwrap();
        let config = data.config(|_| None).unwrap();
        let kept = orch_connector_core::policy::PolicyStore::host(config, Some(data.config_path()))
            .unwrap()
            .policy()
            .unwrap();
        assert_eq!(
            kept.ports.publish.unwrap().incoming_address,
            "node.example.net"
        );
        assert!(kept.local_values.contains_key("ports"));
        let wrong = crate::arguments::PortsSet {
            listen: Some("everywhere".into()),
            ..set
        };
        assert!(
            ports_set(&path, &wrong).is_err(),
            "a ceiling that is not valid is not written"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), written);
    }

    #[test]
    fn node_policy_commands_use_the_connector_file_and_preserve_explicit_kept_values() {
        use crate::arguments::Policy;
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("orch.toml");
        std::fs::write(
            &config,
            format!(
                "version=1\n[node]\ndata_dir={:?}\n",
                dir.path().join("machine").to_string_lossy()
            ),
        )
        .unwrap();
        policy(
            &config,
            None,
            &Policy::Delegate {
                key: "environment.*".into(),
            },
        )
        .unwrap();
        let changed = policy(
            &config,
            None,
            &Policy::Set {
                key: "environment.terminal".into(),
                value: "true".into(),
            },
        )
        .unwrap();
        assert!(changed.contains("environment.terminal = true"), "{changed}");
        let data = DataDir::open(&dir.path().join("machine")).unwrap();
        let store = orch_connector_core::policy::PolicyStore::connected(
            data.config(|_| None).unwrap(),
            Some(data.config_path()),
        )
        .unwrap();
        assert!(store.policy().unwrap().environment_terminal);
        assert!(
            !store
                .policy()
                .unwrap()
                .delegates(orch_link_contract::policy::PolicyKey::EnvironmentTerminal)
        );
    }
}
