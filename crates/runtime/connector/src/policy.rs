//! Local ceilings that hold for everyone, admins included, and come before every page: the arrangement types the machine allows and what its exec node may offer among them; the switches for file writes, delivered files, fetched builds and shells, and for listing and importing the tool servers found here; shown to the server, changed here, and from the server only for the keys the owner handed over, by whoever manages what each bounds.
//!
//! A machine that a person connected obeys the system only inside limits its owner set on it. This
//! module is those limits at work:
//!
//! * [`PolicyStore`] holds them. They are read from the connector's `config.toml`, changed on the
//!   machine by its owner whatever the key, changed from the system only for the keys the owner handed
//!   over (`policy.delegated`), and written back to the file on every change. A machine the system made
//!   has none: its limits are its template's.
//! * [`gate`] is the check every command passes before a role sees it, whoever asked. It is one
//!   function for the commands of all roles, so that a role added later is held to the same limits
//!   without having to remember them. A command the policy does not allow is refused with the policy's
//!   own error; the one thing it changes instead of refusing is a sandbox mode wider than the
//!   machine's ceiling, which is lowered to it and reported ([`Clamp`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use orch_config::ConnectorConfig;
use orch_link_contract::commands::{
    ApprovalMode, ArrangementType, Command, CommandError, ErrorCode, LeaseMode, MaterializePurpose,
    McpStart, SandboxMode, SessionOpenProvider,
};
use orch_link_contract::policy::{
    LocalPolicy, PolicyChange, PolicyError, PolicyHarness, PolicyKey,
};
use orch_types::Json;
use tokio::sync::watch;

/// Why the policy could not be read, changed or kept.
#[derive(Debug, thiserror::Error)]
pub enum PolicyFault {
    /// A locally stored configuration value is invalid; the current policy stays in force.
    #[error("{0}")]
    Config(String),
    /// The key or the value is not one of the policy.
    #[error("{0}")]
    Invalid(#[from] PolicyError),
    /// The file could not be written.
    #[error("cannot write {path}: {source}")]
    Write {
        /// The file.
        path: PathBuf,
        /// What the system said.
        source: std::io::Error,
    },
    /// The configuration could not be written as TOML.
    #[error("cannot write the configuration: {0}")]
    Toml(#[from] toml::ser::Error),
    /// This machine has no local policy: the system made it.
    #[error("a machine the system made has no local policy of its own")]
    Managed,
    /// The file could not be read as a configuration.
    #[error("cannot read {path}: {reason}")]
    Read {
        /// The file.
        path: PathBuf,
        /// Why not.
        reason: String,
    },
}

fn parse<T: serde::de::DeserializeOwned>(text: &str) -> Option<T> {
    serde_json::from_value(Json::String(text.to_owned())).ok()
}

/// Resolve an existing environment directory using the machine's one environment.roots policy.
/// Both personal connectors and host nodes call this on the machine owning the files.
/// Canonical roots and targets make a symlink out of an allowed tree fail the same check.
pub fn resolve_environment_directory(
    policy: &LocalPolicy,
    path: &Path,
) -> Result<PathBuf, CommandError> {
    resolve_environment_path(policy, path, true)
}

fn resolve_environment_path(
    policy: &LocalPolicy,
    path: &Path,
    directory: bool,
) -> Result<PathBuf, CommandError> {
    let refused = |why: String| {
        CommandError::new(
            ErrorCode::PolicyRootNotAllowed,
            format!("environment.roots: {why}"),
        )
    };
    if !path.is_absolute() {
        return Err(refused(
            "an existing directory must be an absolute path".into(),
        ));
    }
    let real = std::fs::canonicalize(path)
        .map_err(|error| refused(format!("cannot open {}: {error}", path.display())))?;
    if directory && !real.is_dir() {
        return Err(refused(format!("{} is not a directory", path.display())));
    }
    if !policy
        .environment_roots
        .iter()
        .any(|root| std::fs::canonicalize(root).is_ok_and(|root| real.starts_with(root)))
    {
        return Err(refused(format!(
            "{} is outside the directories this machine's owners allow",
            path.display()
        )));
    }
    Ok(real)
}

/// Pin the resolved directory beneath an allowed root while reading its identity. Linux openat2
/// refuses symlinks introduced after canonicalization, including in an ancestor of the selected path.
pub fn inspect_environment_directory(
    policy: &LocalPolicy,
    path: &Path,
) -> Result<orch_driver_contract::runtime::DirectoryBinding, CommandError> {
    hold_environment_directory(policy, path).map(|(binding, _)| binding)
}

/// Verifies and retains the same directory that attachment will use.
pub fn hold_environment_directory(
    policy: &LocalPolicy,
    path: &Path,
) -> Result<
    (
        orch_driver_contract::runtime::DirectoryBinding,
        orch_envhost::paths::HeldRoot,
    ),
    CommandError,
> {
    #[cfg(target_os = "linux")]
    {
        let roots: Vec<PathBuf> = policy.environment_roots.iter().map(PathBuf::from).collect();
        let held = orch_envhost::paths::HeldRoot::open(&roots, path).map_err(|error| {
            CommandError::new(
                ErrorCode::PolicyRootNotAllowed,
                format!("environment.roots: {error}"),
            )
        })?;
        let meta = held.metadata().map_err(|error| {
            CommandError::new(
                ErrorCode::PolicyRootNotAllowed,
                format!("environment.roots: {error}"),
            )
        })?;
        let binding = orch_driver_contract::runtime::DirectoryBinding {
            path: held.path().to_string_lossy().into_owned(),
            device: Some(meta.dev),
            inode: Some(meta.ino),
        };
        Ok((binding, held))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = resolve_environment_path(policy, path, true)?;
        Err(CommandError::new(
            ErrorCode::Unsupported,
            "this platform cannot verify an existing directory beneath environment.roots",
        ))
    }
}

/// The same roots policy also bounds file, socket and directory mount sources.
pub fn inspect_environment_path(
    policy: &LocalPolicy,
    path: &Path,
    directory: bool,
) -> Result<orch_driver_contract::runtime::DirectoryBinding, CommandError> {
    if directory {
        return inspect_environment_directory(policy, path);
    }
    let real = resolve_environment_path(policy, path, directory)?;
    #[cfg(target_os = "linux")]
    {
        use rustix::fs::{Mode, OFlags, ResolveFlags, fstat, openat2};
        let denied = |error: rustix::io::Errno| {
            CommandError::new(
                ErrorCode::PolicyRootNotAllowed,
                format!("environment.roots: the directory changed or cannot be opened: {error}"),
            )
        };
        for root in &policy.environment_roots {
            let Ok(root) = std::fs::canonicalize(root) else {
                continue;
            };
            let Ok(relative) = real.strip_prefix(&root) else {
                continue;
            };
            let base = openat2(
                rustix::fs::CWD,
                &root,
                OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
                ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
            )
            .map_err(denied)?;
            let relative = if relative.as_os_str().is_empty() {
                Path::new(".")
            } else {
                relative
            };
            let flags = OFlags::PATH
                | OFlags::CLOEXEC
                | if directory {
                    OFlags::DIRECTORY
                } else {
                    OFlags::empty()
                };
            let held = openat2(
                &base,
                relative,
                flags,
                Mode::empty(),
                ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
            )
            .map_err(denied)?;
            let stat = fstat(&held).map_err(denied)?;
            return Ok(orch_driver_contract::runtime::DirectoryBinding {
                path: real.to_string_lossy().into_owned(),
                device: Some(stat.st_dev),
                inode: Some(stat.st_ino),
            });
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = real;
    Err(CommandError::new(
        ErrorCode::Unsupported,
        "this platform cannot verify an existing directory beneath environment.roots",
    ))
}

/// Check access as the caller's actual identity, including supplementary groups and filesystem ACLs.
/// A connector calls this for itself; drivers check as the unit user before starting its connector.
pub fn writable_environment_directory(path: &Path) -> Result<(), CommandError> {
    #[cfg(unix)]
    {
        rustix::fs::accessat(rustix::fs::CWD, path, rustix::fs::Access::WRITE_OK | rustix::fs::Access::EXEC_OK, rustix::fs::AtFlags::EACCESS)
            .map_err(|_| CommandError::new(ErrorCode::PolicyRootNotAllowed, format!("this machine's owners must make {} writable and searchable for user id {} (Orch will not change its owner or mode)", path.display(), rustix::process::geteuid().as_raw())))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(CommandError::new(
            ErrorCode::Unsupported,
            "directory writability has not been verified on this platform",
        ))
    }
}

/// The local policy a configuration file holds.
pub fn policy_of(config: &ConnectorConfig) -> Result<LocalPolicy, PolicyError> {
    let agent_max_sandbox = parse(&config.agent.max_sandbox).ok_or(PolicyError::Value {
        key: PolicyKey::AgentMaxSandbox,
        expected: "read_only, workspace or open",
    })?;
    let provider_arrangements = config
        .provider
        .arrangements
        .iter()
        .map(|kind| {
            parse(kind).ok_or(PolicyError::Value {
                key: PolicyKey::ProviderArrangements,
                expected: "separated, key_separated, credential_shared or handed_over",
            })
        })
        .collect::<Result<Vec<ArrangementType>, _>>()?;
    let mut policy = LocalPolicy {
        drivers: config.drivers.clone(),
        storages: config.storage.clone(),
        resource_max: config.resources.max,
        roles: Some(
            config
                .connector
                .roles
                .iter()
                .filter_map(|role| parse(role))
                .collect(),
        ),
        local_values: BTreeMap::new(),
        hooks: config.hooks.clone(),
        environment_roots: config.environment.roots.clone(),
        environment_remote_write: config.environment.remote_write,
        environment_terminal: config.environment.terminal,
        environment_after_up: config.environment.after_up,
        environment_browser: config.environment.browser,
        environment_vnc: config.environment.vnc,
        ports: config.ports.clone(),
        mcp_import: config.mcp.import,
        mcp_add: config.mcp.add,
        exec_offers: config.exec.offers.clone(),
        environment_deliver_files: config.environment.deliver_files,
        agent_max_sandbox,
        provider_upstreams: config.provider.upstreams.clone(),
        provider_signin: config.provider.signin,
        provider_arrangements,
        agent_harnesses: config
            .agent
            .harnesses
            .iter()
            .map(|harness| PolicyHarness {
                kind: harness.kind.clone(),
                path: harness.path.clone(),
            })
            .collect(),
        tools_fetch: config.tools.fetch,
        agent_programs: config.agent.programs,
        provider_programs: config.provider.programs,
        machine_roles: config.machine.roles,
        update_auto: config.update.auto,
        policy_delegated: config.policy.delegated.clone(),
    };
    policy.set(
        PolicyKey::ProviderArrangements,
        &serde_json::json!(policy.provider_arrangements),
    )?;
    Ok(policy)
}

/// Puts a policy into a configuration, key by key, as the file keeps it.
pub fn put_policy(config: &mut ConnectorConfig, policy: &LocalPolicy) {
    config.drivers = policy.drivers.clone();
    config.storage.clone_from(&policy.storages);
    config.resources.max.clone_from(&policy.resource_max);
    config.hooks.clone_from(&policy.hooks);
    config
        .environment
        .roots
        .clone_from(&policy.environment_roots);
    config.environment.remote_write = policy.environment_remote_write;
    config.environment.terminal = policy.environment_terminal;
    config.environment.after_up = policy.environment_after_up;
    config.environment.browser = policy.environment_browser;
    config.environment.vnc = policy.environment_vnc;
    config.ports.clone_from(&policy.ports);
    config.environment.deliver_files = policy.environment_deliver_files;
    config.mcp.import = policy.mcp_import;
    config.mcp.add = policy.mcp_add;
    config.exec.offers.clone_from(&policy.exec_offers);
    config.agent.max_sandbox = policy.agent_max_sandbox.as_str().to_owned();
    config.agent.programs = policy.agent_programs;
    config.provider.programs = policy.provider_programs;
    config.machine.roles = policy.machine_roles;
    config.agent.harnesses = policy
        .agent_harnesses
        .iter()
        .map(|harness| orch_config::connector::AgentHarness {
            kind: harness.kind.clone(),
            path: harness.path.clone(),
        })
        .collect();
    config
        .provider
        .upstreams
        .clone_from(&policy.provider_upstreams);
    config.provider.arrangements = policy
        .provider_arrangements
        .iter()
        .map(|kind| kind.as_str().to_owned())
        .collect();
    config.provider.signin = policy.provider_signin;
    config.tools.fetch = policy.tools_fetch;
    config.update.auto = policy.update_auto;
    config.policy.delegated.clone_from(&policy.policy_delegated);
}

/// Explicit local constraints, preserving omitted defaults when enrollment rewrites configuration.
pub fn local_keys(config: &ConnectorConfig, _path: Option<&Path>) -> Vec<String> {
    config.policy.local_keys.clone().unwrap_or_default()
}

/// Import explicit keys once when reading an operator-authored file without provenance metadata.
pub fn explicit_local_keys(text: &str) -> Result<Vec<String>, toml::de::Error> {
    let written: toml::Value = toml::from_str(text)?;
    Ok(PolicyKey::ALL
        .iter()
        .filter(|key| **key != PolicyKey::PolicyDelegated)
        .filter(|key| match key.as_str().split_once('.') {
            Some((section, member)) => written.get(section).and_then(|v| v.get(member)).is_some(),
            None => {
                written.get(key.as_str()).is_some()
                    || (**key == PolicyKey::Storages && written.get("storage").is_some())
            }
        })
        .map(|key| key.as_str().into())
        .collect())
}

/// Write only explicit policy fields while retaining operational configuration and Orch-side values.
pub fn sparse_config(
    config: &ConnectorConfig,
    local: &[String],
) -> Result<String, toml::ser::Error> {
    // TOML has no null. Every policy value is typed/defaulted before it reaches here, so omitting
    // an optional object's null member preserves its explicit clearing when resolve reads it back.
    fn optional_members(value: &mut Json) {
        match value {
            Json::Object(members) => {
                members.retain(|_, value| !value.is_null());
                for value in members.values_mut() {
                    optional_members(value);
                }
            }
            Json::Array(values) => {
                for value in values {
                    optional_members(value);
                }
            }
            _ => {}
        }
    }
    let mut config = config.clone();
    for (key, value) in &mut config.policy.values {
        // Driver options are opaque JSON, encoded by the file codec without changing nulls.
        if key != PolicyKey::Drivers.as_str() {
            optional_members(value);
        }
    }
    let mut written = toml::Value::try_from(&config)?;
    for key in PolicyKey::ALL
        .iter()
        .filter(|key| **key != PolicyKey::PolicyDelegated)
    {
        if local.iter().any(|name| name == key.as_str()) {
            continue;
        }
        if let Some((section, member)) = key.as_str().split_once('.') {
            if let Some(table) = written.get_mut(section).and_then(toml::Value::as_table_mut) {
                table.remove(member);
            }
        } else if let Some(table) = written.as_table_mut() {
            table.remove(key.as_str());
        }
    }
    toml::to_string_pretty(&written)
}

fn with_installed_defaults(
    config: &ConnectorConfig,
    path: Option<&Path>,
) -> Result<LocalPolicy, PolicyError> {
    let configured = policy_of(config)?;
    let local = local_keys(config, path)
        .into_iter()
        .map(|name| {
            name.parse::<PolicyKey>()
                .map(|key| (name, configured.get(key)))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let mut policy = LocalPolicy::resolve(&config.policy.delegated, &local, &config.policy.values)?;
    policy.roles = Some(
        config
            .connector
            .roles
            .iter()
            .filter_map(|role| parse(role))
            .collect(),
    );
    Ok(policy)
}

// ---------------------------------------------------------------------------------------------------------
// The gate

/// A request that the machine's ceiling lowered instead of refusing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clamp {
    /// The key whose ceiling it was.
    pub key: &'static str,
    /// What was asked.
    pub asked: String,
    /// What it was lowered to.
    pub used: String,
}

/// A command that passed the gate: as it will be carried out, and what was lowered on the way.
#[derive(Debug, Clone, PartialEq)]
pub struct Gated {
    /// The command, with any sandbox mode lowered to the ceiling.
    pub command: Command,
    /// What was lowered.
    pub clamps: Vec<Clamp>,
}

fn refused(code: ErrorCode, key: &str, why: &str) -> CommandError {
    let sections = key
        .parse::<PolicyKey>()
        .map(|key| {
            key.sections()
                .iter()
                .map(|section| section.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_else(|_| "machine".into());
    let mut error = CommandError::new(
        code,
        format!(
            "{key} ({sections} role): {why}; the machine's operator decides this setting locally or hands it to the machine's managers"
        ),
    );
    error.details = Some(
        serde_json::json!({"policy_key":key,"role":sections,"decides":"machine_operator_or_managers"}),
    );
    error
}

/// Turns the code a check of the policy answers into the error of the command.
fn check(result: Result<(), ErrorCode>, key: &str, why: &str) -> Result<(), CommandError> {
    result.map_err(|code| refused(code, key, why))
}

fn mode_name(mode: SandboxMode) -> String {
    mode.as_str().to_owned()
}

/// Whether the harness of a session runs on this machine: it carries out its own tool calls, so the
/// directory it is given is a directory here. A harness that works through an executor runs with its
/// provider, and its directory is a path in another machine's environment.
fn harness_is_here(executor: Option<&String>) -> bool {
    executor.is_none()
}

fn refusal_authority(policy: &LocalPolicy, mut error: CommandError) -> CommandError {
    let key = error
        .details
        .as_ref()
        .and_then(|details| details.get("policy_key"))
        .and_then(Json::as_str)
        .and_then(|key| key.parse::<PolicyKey>().ok());
    if let Some(key) = key {
        let handed = policy.delegates(key);
        let decides = if handed {
            "machine_managers"
        } else {
            "machine_operator"
        };
        let program = if policy
            .roles
            .as_ref()
            .is_some_and(|roles| roles.contains(&orch_link_contract::session::Role::Node))
        {
            "orch-node"
        } else {
            "orch-connector"
        };
        let why = error
            .detail
            .split("; the machine's operator")
            .next()
            .unwrap_or(&error.detail);
        error.detail = if handed {
            format!("{why}; the machine's managers control this handed-over key in Orch")
        } else {
            format!(
                "{why}; the machine's operator keeps this key: `{program} policy set {} …`",
                key.as_str()
            )
        };
        if let Some(details) = error.details.as_mut().and_then(Json::as_object_mut) {
            details.insert("decides".into(), Json::String(decides.into()));
        }
    }
    error
}

/// Checks a command against the policy, whoever sent it. A command the policy forbids is refused with
/// the error the policy names; a sandbox mode wider than the ceiling is lowered to it.
///
/// What a key governs is stated beside the check. Commands no key concerns pass.
pub fn gate(policy: &LocalPolicy, command: Command) -> Result<Gated, CommandError> {
    gate_command(policy, command).map_err(|error| refusal_authority(policy, error))
}

#[allow(
    clippy::too_many_lines,
    reason = "one arm for each command a key concerns is the clearest form of the table"
)]
fn gate_command(policy: &LocalPolicy, command: Command) -> Result<Gated, CommandError> {
    // Nested preparation/start commands use precisely the ordinary policy supplier.
    // Rewrap the gated body, retaining every clamp rather than opening a second policy path.
    let nested = match command {
        Command::SessionOpenFenced(mut body) => {
            let gated = gate_command(policy, Command::SessionOpen(*body.session))?;
            let Command::SessionOpen(open) = gated.command else {
                return Err(CommandError::new(
                    ErrorCode::Internal,
                    "The ordinary open policy changed the command kind.",
                ));
            };
            body.session = Box::new(open);
            return Ok(Gated {
                command: Command::SessionOpenFenced(body),
                clamps: gated.clamps,
            });
        }
        Command::SessionPrefixOpen(mut body) => {
            let gated = gate_command(policy, Command::SessionOpen(*body.session))?;
            let Command::SessionOpen(open) = gated.command else {
                return Err(CommandError::new(
                    ErrorCode::Internal,
                    "The ordinary open policy changed the command kind.",
                ));
            };
            body.session = Box::new(open);
            return Ok(Gated {
                command: Command::SessionPrefixOpen(body),
                clamps: gated.clamps,
            });
        }
        Command::SessionContinuationResume(mut body) => {
            let gated = gate_command(policy, Command::SessionOpen(*body.session))?;
            let Command::SessionOpen(open) = gated.command else {
                return Err(CommandError::new(
                    ErrorCode::Internal,
                    "The ordinary open policy changed the command kind.",
                ));
            };
            body.session = Box::new(open);
            return Ok(Gated {
                command: Command::SessionContinuationResume(body),
                clamps: gated.clamps,
            });
        }
        Command::TurnStartFenced(mut body) => {
            let gated = gate_command(policy, Command::TurnStart(*body.turn))?;
            let Command::TurnStart(turn) = gated.command else {
                return Err(CommandError::new(
                    ErrorCode::Internal,
                    "The ordinary turn policy changed the command kind.",
                ));
            };
            body.turn = Box::new(turn);
            return Ok(Gated {
                command: Command::TurnStartFenced(body),
                clamps: gated.clamps,
            });
        }
        command => command,
    };
    let mut clamps = Vec::new();
    let mut command = nested;
    match &mut command {
        Command::AgentStart(start) => {
            if let Some(runtime) = &mut start.runtime {
                runtime.resources = policy
                    .bounded_resources_optional(runtime.resources)
                    .map_err(|reason| refused(ErrorCode::Unsupported, "resources.max", &reason))?;
            }
        }
        Command::PortPublish(body) => {
            if !policy
                .ports
                .permits(orch_api_contract::ports::PortOpeningKind::Host)
            {
                return Err(refused(
                    ErrorCode::Unsupported,
                    "ports",
                    "publishing on this machine's address is off",
                ));
            }
            crate::host_ports::permitted(&crate::host_ports::policy_listeners(&policy.ports), body)
                .map_err(|reason| refused(ErrorCode::Unsupported, "ports", &reason))?;
        }
        Command::UnitAfterUp(_) if !policy.environment_after_up => {
            return Err(refused(
                ErrorCode::Unsupported,
                "environment.after_up",
                "commands after start or attach are off",
            ));
        }
        Command::BrowserOpen(_) | Command::BrowserAdopt(_) | Command::BrowserProfile(_)
            if !policy.environment_browser =>
        {
            return Err(refused(
                ErrorCode::Unsupported,
                "environment.browser",
                "browser operations are off",
            ));
        }
        Command::DisplayOpen(_) | Command::DisplayAdopt(_) if !policy.environment_vnc => {
            return Err(refused(
                ErrorCode::Unsupported,
                "environment.vnc",
                "display operations are off",
            ));
        }
        Command::SessionOpen(open) => {
            let used = policy.sandbox(open.sandbox_mode);
            if used != open.sandbox_mode {
                clamps.push(Clamp {
                    key: "agent.max_sandbox",
                    asked: mode_name(open.sandbox_mode),
                    used: mode_name(used),
                });
                open.sandbox_mode = used;
            }
            if harness_is_here(open.exec.executor.as_ref()) {
                for directory in std::iter::once(&open.directory)
                    .chain(std::iter::once(&open.cwd))
                    .chain(open.mounts.iter().map(|mount| &mount.path))
                {
                    check(
                        policy.check_root(Path::new(directory)),
                        "environment.roots",
                        &format!("{directory} is outside the directories this machine offers"),
                    )?;
                }
            }
            if matches!(open.provider, SessionOpenProvider::Native { .. }) {
                check(
                    policy.check_arrangement(open.arrangement),
                    "provider.arrangements",
                    &format!(
                        "the login kept on this machine is not to be used in {} arrangements",
                        open.arrangement.as_str()
                    ),
                )?;
            }
        }
        Command::TurnStart(turn) => {
            let used = policy.sandbox(turn.sandbox_mode);
            if used != turn.sandbox_mode {
                clamps.push(Clamp {
                    key: "agent.max_sandbox",
                    asked: mode_name(turn.sandbox_mode),
                    used: mode_name(used),
                });
                turn.sandbox_mode = used;
            }
        }
        Command::EnvAttach(attach) => {
            attach.resources = policy
                .bounded_resources_optional(attach.resources)
                .map_err(|reason| refused(ErrorCode::Unsupported, "resources.max", &reason))?;
            if let Some(sources) = &attach.data_sources {
                for path in [sources.workspace.directory(), sources.home.directory()]
                    .into_iter()
                    .flatten()
                {
                    check(
                        policy.check_root(Path::new(path)),
                        "environment.roots",
                        &format!("{path} is outside the directories this machine offers"),
                    )?;
                }
            }
            let directories = attach
                .workspace_root
                .iter()
                .filter(|_| attach.data_sources.is_none())
                .chain(
                    attach
                        .mounts
                        .iter()
                        .flatten()
                        .filter(|mount| mount.volume.is_none())
                        .map(|mount| &mount.source),
                );
            for directory in directories {
                check(
                    policy.check_root(Path::new(directory)),
                    "environment.roots",
                    &format!("{directory} is outside the directories this machine offers"),
                )?;
            }
        }
        Command::FsWrite(_)
        | Command::FsMkdir(_)
        | Command::FsDataImport(_)
        | Command::FsRename(_)
        | Command::FsRemove(_)
        | Command::FsCopy(_)
        | Command::GitPrepare(_)
        | Command::GitPrepareCleanup(_)
        | Command::GitWorktree(_)
        | Command::GitWorktreeCleanup(_)
        | Command::GitWorktreeRepair(_)
        | Command::GitWorktreeStandalone(_)
        | Command::GitCommit(_)
        | Command::GitPush(_) => check(
            policy.check_remote_write(),
            "environment.remote_write",
            "this machine accepts no file written from outside",
        )?,
        Command::FsMaterialize(materialize) => match materialize.purpose {
            MaterializePurpose::Delivery => check(
                policy.check_deliver_files(),
                "environment.deliver_files",
                "this machine does not write the files that arrive with a message",
            )?,
            // An exact copy of files written into the environment from outside is a remote write like a move.
            MaterializePurpose::Move | MaterializePurpose::Exact => check(
                policy.check_remote_write(),
                "environment.remote_write",
                "this machine accepts no file written from outside",
            )?,
        },
        Command::PtyOpen(_) => {
            if !policy.environment_terminal {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    "environment.terminal: shells are switched off on this machine",
                ));
            }
        }
        Command::McpStart(McpStart::Given(_)) => check(
            policy.check_mcp_add(),
            "mcp.add",
            "this machine lets no tool server be written from outside",
        )?,
        Command::McpImport(_) => check(
            policy.check_mcp_import(),
            "mcp.import",
            "this machine lets no tool server be imported",
        )?,
        Command::ExecStart(exec) => {
            // An executor that has tool sets is offered when any of its sets is.
            check(
                policy.check_offer(&exec.executor),
                "exec.offers",
                &format!("this machine does not offer {}", exec.executor),
            )?;
        }
        Command::ProviderStart(start) => {
            for upstream in &start.upstreams {
                check(
                    policy.check_upstream(&upstream.base_url),
                    "provider.upstreams",
                    &format!(
                        "model calls are not to be forwarded to {}",
                        upstream.base_url
                    ),
                )?;
            }
        }
        Command::LeaseOpen(lease) => {
            let allowed = match lease.mode {
                LeaseMode::Forwarded => policy
                    .check_arrangement(ArrangementType::Separated)
                    .or_else(|_| policy.check_arrangement(ArrangementType::KeySeparated)),
                LeaseMode::Shared => policy.check_arrangement(ArrangementType::CredentialShared),
                LeaseMode::Handover => policy.check_arrangement(ArrangementType::HandedOver),
            };
            check(
                allowed,
                "provider.arrangements",
                "the credential kept on this machine is not to be used in that arrangement",
            )?;
        }
        _ => {}
    }
    Ok(Gated { command, clamps })
}

// ---------------------------------------------------------------------------------------------------------
// The store

struct State {
    config: ConnectorConfig,
    policy: Option<LocalPolicy>,
    active_roles: Option<Vec<orch_link_contract::session::Role>>,
}

/// The local policy of this connector: read from its file, kept in it, changed by its owner, and by the
/// system for the keys that were handed over.
pub struct PolicyStore {
    state: Mutex<State>,
    path: Option<PathBuf>,
    changed: watch::Sender<u64>,
}

impl std::fmt::Debug for PolicyStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PolicyStore")
            .field("managed", &self.is_managed())
            .finish_non_exhaustive()
    }
}

impl PolicyStore {
    /// The same role-filtered policy report for both local programs.
    #[must_use]
    pub fn describe(&self) -> Vec<String> {
        let Some(policy) = self.policy() else {
            return vec!["Policy: inherited from the hosting machine".into()];
        };
        let mut rows = vec!["Policy (* handed to Orch; otherwise kept on this machine)".into()];
        for key in PolicyKey::ALL.iter().filter(|key| {
            policy
                .roles
                .as_ref()
                .is_none_or(|roles| key.applies_to_roles(roles))
        }) {
            let definition = key.definition();
            rows.push(format!(
                "{}{} = {}\n  {}\n  managed: {}; cautious: {}",
                key.as_str(),
                if policy.delegates(*key) { " *" } else { "" },
                policy.get(*key),
                definition.meaning,
                definition.managed,
                definition.cautious
            ));
        }
        rows
    }

    /// Read the same JSON or comma-separated local value in connector and node commands.
    pub fn set_text(&self, key: PolicyKey, text: &str) -> Result<(), PolicyFault> {
        let current = self.policy().ok_or(PolicyFault::Managed)?.get(key);
        self.set_here(key, &value_from_text(text, &current))
    }
    /// The policy of a machine a person connected, whose file is `path` (`None` keeps it in memory
    /// only).
    pub fn connected(
        mut config: ConnectorConfig,
        path: Option<PathBuf>,
    ) -> Result<Self, PolicyFault> {
        config.policy.local_keys = Some(local_keys(&config, path.as_deref()));
        let policy = with_installed_defaults(&config, None)?;
        Ok(Self::build(config, Some(policy), path))
    }

    /// A node uses the same physical-machine policy; delegation is the installer's explicit choice.
    pub fn host(config: ConnectorConfig, path: Option<PathBuf>) -> Result<Self, PolicyFault> {
        Self::connected(config, path)
    }

    /// A machine the system made: it has no local policy, and nothing is held to one.
    #[must_use]
    pub fn managed(config: ConnectorConfig) -> Self {
        Self::build(config, None, None)
    }

    /// The active roles, as chosen by the hosting program rather than a machine-kind label.
    pub fn offered_roles(&self, roles: &[orch_link_contract::session::Role]) {
        use orch_link_contract::session::Role;
        let mut state = self.state();
        state.config.connector.roles = roles
            .iter()
            .map(|role| {
                match role {
                    Role::Agent => "agent",
                    Role::Environment => "environment",
                    Role::Provider => "provider",
                    Role::Node => "node",
                }
                .to_owned()
            })
            .collect();
        state.active_roles = Some(roles.to_vec());
        if let Some(policy) = &mut state.policy {
            policy.roles = Some(roles.to_vec());
        }
    }

    fn build(config: ConnectorConfig, policy: Option<LocalPolicy>, path: Option<PathBuf>) -> Self {
        Self {
            state: Mutex::new(State {
                config,
                policy,
                active_roles: None,
            }),
            path,
            changed: watch::channel(0).0,
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether the system made this machine, so that it has no local policy.
    #[must_use]
    pub fn is_managed(&self) -> bool {
        self.state().policy.is_none()
    }

    /// The policy now in force; `None` for a machine the system made.
    #[must_use]
    pub fn policy(&self) -> Option<LocalPolicy> {
        self.state().policy.clone()
    }

    /// Checked at the actual cache miss, so disabling downloads still permits verified installed builds.
    pub fn require_tools_fetch(&self) -> Result<(), CommandError> {
        let state = self.state();
        match &state.policy {
            Some(policy) if policy.tools_fetch => Ok(()),
            Some(policy) => Err(refusal_authority(
                policy,
                refused(
                    ErrorCode::Unsupported,
                    "tools.fetch",
                    "fetching builds is off",
                ),
            )),
            None => Err(refused(
                ErrorCode::Unsupported,
                "tools.fetch",
                "this runtime has not offered build fetching",
            )),
        }
    }

    /// The configuration as it stands, policy included.
    #[must_use]
    pub fn config(&self) -> ConnectorConfig {
        self.state().config.clone()
    }

    /// The one file owns both policy provenance and runtime program registrations; a policy save
    /// cannot overwrite a newer registry. The kept/delegated key is rechecked under this same lock.
    pub fn change_program(
        &self,
        kind: orch_api_contract::machine_programs::MachineProgramKind,
        name: &str,
        entry: Option<orch_config::connector::ConnectorPlugin>,
    ) -> Result<(), CommandError> {
        let key = crate::programs::policy_key(kind);
        let mut state = self.state();
        let Some(policy) = &state.policy else {
            return Err(refused(
                ErrorCode::Unsupported,
                key.as_str(),
                "this runtime has no registration policy of its own",
            ));
        };
        if !policy.delegates(key)
            || policy.get(key).as_bool() != Some(true)
            || policy
                .roles
                .as_ref()
                .is_none_or(|roles| !key.applies_to_roles(roles))
        {
            return Err(refusal_authority(
                policy,
                refused(
                    ErrorCode::Unsupported,
                    key.as_str(),
                    "runtime program registration is kept locally or switched off",
                ),
            ));
        }
        let word = match kind {
            orch_api_contract::machine_programs::MachineProgramKind::Agent => "adapter",
            orch_api_contract::machine_programs::MachineProgramKind::ProviderSource => "provider",
        };
        let existing = state
            .config
            .plugin
            .iter()
            .position(|program| program.kind == word && program.name == name);
        if entry.is_some() && existing.is_some() {
            return Err(CommandError::new(
                ErrorCode::Unsupported,
                "A program with this name is already registered; Test it or Remove it first.",
            ));
        }
        let mut next = State {
            config: state.config.clone(),
            policy: state.policy.clone(),
            active_roles: state.active_roles.clone(),
        };
        if let Some(entry) = entry {
            next.config.plugin.push(entry);
        } else {
            let index = existing.ok_or_else(|| {
                CommandError::new(ErrorCode::NotFound, "That program is no longer registered.")
            })?;
            if !next.config.plugin[index].registered_by_orch {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    "The operator keeps this program in the local configuration; remove its entry there and Reload.",
                ));
            }
            next.config.plugin.remove(index);
        }
        self.persist(&next).map_err(|error| {
            CommandError::new(
                ErrorCode::Internal,
                format!("The program registry could not be saved: {error}"),
            )
        })?;
        *state = next;
        self.changed.send_modify(|count| *count += 1);
        Ok(())
    }

    /// Atomically changes one harness selection under the machine's delegated policy.
    pub(crate) fn change_harness(
        &self,
        kind: &str,
        expected: Option<&str>,
        replacement: Option<&str>,
    ) -> Result<(), CommandError> {
        let key = PolicyKey::AgentHarnesses;
        let mut state = self.state();
        let Some(mut policy) = state.policy.clone() else {
            return Err(refused(
                ErrorCode::Unsupported,
                key.as_str(),
                "this runtime has no local harness policy",
            ));
        };
        if !policy.delegates(key) {
            return Err(refusal_authority(
                &policy,
                refused(
                    ErrorCode::Unsupported,
                    key.as_str(),
                    "the harness selection is kept locally",
                ),
            ));
        }
        if kind.is_empty() || replacement.is_some_and(str::is_empty) {
            return Err(CommandError::new(
                ErrorCode::Unsupported,
                "a harness kind and program path are required",
            ));
        }
        if policy.harness(kind).map(|entry| entry.path.as_str()) != expected {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "the harness selection changed; refresh the machine and try again",
            ));
        }
        policy.agent_harnesses.retain(|entry| entry.kind != kind);
        if let Some(path) = replacement {
            policy.agent_harnesses.push(PolicyHarness {
                kind: kind.to_owned(),
                path: path.to_owned(),
            });
        }
        let value = policy.get(key);
        let before = state.config.policy.values.clone();
        state
            .config
            .policy
            .values
            .insert(key.as_str().into(), value);
        if let Err(error) = self.commit(&mut state, policy) {
            state.config.policy.values = before;
            return Err(CommandError::new(
                ErrorCode::Internal,
                format!("the harness selection could not be saved: {error}"),
            ));
        }
        Ok(())
    }

    /// Save operational roles under the same lock/file as policy and programs. Active-role facts
    /// remain unchanged until the supervisor has rebuilt the actual role handlers.
    pub fn change_roles(
        &self,
        roles: &[orch_link_contract::session::Role],
    ) -> Result<bool, CommandError> {
        let mut state = self.state();
        let Some(policy) = &state.policy else {
            return Err(refused(
                ErrorCode::Unsupported,
                "machine.roles",
                "this runtime takes its roles from its template",
            ));
        };
        if !policy.delegates(PolicyKey::MachineRoles) || !policy.machine_roles {
            return Err(refusal_authority(
                policy,
                refused(
                    ErrorCode::Unsupported,
                    "machine.roles",
                    "changing the offered roles is kept locally or switched off",
                ),
            ));
        }
        let names = roles
            .iter()
            .map(crate::enrollment::role_name)
            .collect::<Vec<_>>();
        if state.config.connector.roles == names {
            return Ok(false);
        }
        let mut next = State {
            config: state.config.clone(),
            policy: state.policy.clone(),
            active_roles: state.active_roles.clone(),
        };
        next.config.connector.roles = names;
        self.persist(&next).map_err(|error| {
            CommandError::new(
                ErrorCode::Internal,
                format!("The roles could not be saved: {error}"),
            )
        })?;
        *state = next;
        Ok(true)
    }

    /// Told each time the policy changes.
    #[must_use]
    pub fn changes(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    /// Checks a command against the policy. A machine the system made lets everything pass: its limits
    /// are its template's.
    pub fn gate(&self, command: Command) -> Result<Gated, CommandError> {
        match self.policy() {
            Some(policy) => gate(&policy, command),
            None => Ok(Gated {
                command,
                clamps: Vec::new(),
            }),
        }
    }

    /// The program the policy gives for harness kind `kind`, if it names one.
    #[must_use]
    pub fn harness(&self, kind: &str) -> Option<PolicyHarness> {
        self.policy()?.harness(kind).cloned()
    }

    fn persist(&self, state: &State) -> Result<(), PolicyFault> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let local: Vec<String> = state
            .policy
            .as_ref()
            .map(|policy| policy.local_values.keys().cloned().collect())
            .unwrap_or_default();
        let text = sparse_config(&state.config, &local)?;
        write_atomically(path, text.as_bytes()).map_err(|source| PolicyFault::Write {
            path: path.clone(),
            source,
        })
    }

    fn commit(&self, state: &mut State, policy: LocalPolicy) -> Result<(), PolicyFault> {
        let mut config = state.config.clone();
        put_policy(&mut config, &policy);
        config.policy.local_keys = Some(policy.local_values.keys().cloned().collect());
        orch_config::loader::Validate::validate(&config)
            .map_err(|error| PolicyFault::Config(error.to_string()))?;
        let mut resolved = with_installed_defaults(&config, None)?;
        if let Some(roles) = &state.active_roles {
            resolved.roles = Some(roles.clone());
        }
        let next = State {
            policy: Some(resolved),
            config,
            active_roles: state.active_roles.clone(),
        };
        self.persist(&next)?;
        *state = next;
        self.changed.send_modify(|count| *count += 1);
        Ok(())
    }

    /// Changes a key as the machine's owner does on the machine: any key, whether handed over or not.
    pub fn set_here(&self, key: PolicyKey, value: &Json) -> Result<(), PolicyFault> {
        let mut state = self.state();
        let mut policy = state.policy.clone().ok_or(PolicyFault::Managed)?;
        policy.set(key, value)?;
        if key != PolicyKey::PolicyDelegated {
            policy
                .local_values
                .insert(key.as_str().into(), value.clone());
        }
        self.commit(&mut state, policy)
    }

    /// Hands keys over to the system (`*` hands all of them), as the owner does on the machine.
    pub fn delegate(&self, keys: &[String]) -> Result<(), PolicyFault> {
        let mut state = self.state();
        let mut policy = state.policy.clone().ok_or(PolicyFault::Managed)?;
        let mut handed: Vec<String> = policy.policy_delegated.clone();
        for key in keys {
            if !handed.contains(key) {
                handed.push(key.clone());
            }
        }
        policy.set(PolicyKey::PolicyDelegated, &Json::from(handed))?;
        self.commit(&mut state, policy)
    }

    /// Takes keys back from the system (`all` takes every one back), as the owner does on the machine.
    pub fn undelegate(&self, keys: &[String]) -> Result<(), PolicyFault> {
        let mut state = self.state();
        let mut policy = state.policy.clone().ok_or(PolicyFault::Managed)?;
        let handed: Vec<String> = if keys.iter().any(|key| key == "all") {
            Vec::new()
        } else {
            policy
                .policy_delegated
                .iter()
                .filter(|key| !keys.contains(key))
                .cloned()
                .collect()
        };
        policy.set(PolicyKey::PolicyDelegated, &Json::from(handed))?;
        self.commit(&mut state, policy)
    }

    /// Takes in what the machine's owner changed in the file while the connector runs (`orch-connector
    /// policy set` in another process, or an edit by hand): the policy the file holds is in force from
    /// now on. Says whether anything changed. A file that cannot be read as a configuration, or whose
    /// policy is not one, leaves the policy in force as it was.
    pub fn reload_from_file(&self) -> Result<bool, PolicyFault> {
        let Some(path) = &self.path else {
            return Ok(false);
        };
        let unreadable = |reason: String| PolicyFault::Read {
            path: path.clone(),
            reason,
        };
        let text = std::fs::read_to_string(path).map_err(|error| unreadable(error.to_string()))?;
        let mut read: ConnectorConfig =
            orch_config::load_text(&text, &path.display().to_string(), |name| {
                std::env::var(name).ok()
            })
            .map_err(|error| unreadable(error.to_string()))?;
        if read.policy.local_keys.is_none() {
            read.policy.local_keys =
                Some(explicit_local_keys(&text).map_err(|error| unreadable(error.to_string()))?);
        }
        let mut found = with_installed_defaults(&read, None)?;
        let mut state = self.state();
        if let Some(roles) = &state.active_roles {
            found.roles = Some(roles.clone());
        }
        if state.policy.is_none()
            || (state.policy.as_ref() == Some(&found)
                && state.config.connector.dedicated == read.connector.dedicated
                && state.config.plugin == read.plugin)
        {
            return Ok(false);
        }
        state.config = read;
        state.policy = Some(found);
        self.changed.send_modify(|count| *count += 1);
        Ok(true)
    }

    /// Applies a change that arrived from the system: only the keys that were handed over, and each
    /// that fits its key; the rest are refused and left as they were. What was applied is kept in the
    /// file.
    pub fn apply_from_system(
        &self,
        values: &BTreeMap<String, Json>,
    ) -> Result<(PolicyChange, Option<LocalPolicy>), PolicyFault> {
        let mut state = self.state();
        let Some(mut policy) = state.policy.clone() else {
            return Ok((
                PolicyChange {
                    applied: Vec::new(),
                    refused: values.keys().cloned().collect(),
                },
                None,
            ));
        };
        let change = policy.apply(values);
        if !change.applied.is_empty() {
            let before = state.config.policy.values.clone();
            for key in &change.applied {
                if let Some(value) = values.get(key) {
                    state
                        .config
                        .policy
                        .values
                        .insert(key.clone(), value.clone());
                }
            }
            if let Err(error) = self.commit(&mut state, policy.clone()) {
                state.config.policy.values = before;
                return Err(error);
            }
        }
        Ok((change, state.policy.clone()))
    }
}

/// How often a running connector looks at its file for a change its owner made there: soon enough that a
/// limit set at the terminal holds before the next thing is asked, and one file status a check.
pub const FILE_CHECK: std::time::Duration = std::time::Duration::from_secs(2);

/// A local command value: JSON, or comma-separated words for a list, or plain text.
#[must_use]
pub fn value_from_text(text: &str, current: &Json) -> Json {
    let parsed = serde_json::from_str::<Json>(text).ok();
    if let Some(array @ Json::Array(_)) = parsed {
        return array;
    }
    if current.is_array() {
        let words = match &parsed {
            Some(Json::String(inner)) => inner.as_str(),
            _ => text,
        };
        return Json::Array(
            words
                .split(',')
                .map(str::trim)
                .filter(|word| !word.is_empty())
                .map(Json::from)
                .collect(),
        );
    }
    parsed.unwrap_or_else(|| Json::from(text))
}

/// Takes in the owner's changes to the file until `stop`: its modification time and length are looked at
/// every [`FILE_CHECK`], and a change is read in. The connector's own writes read in as no change; a file that
/// cannot be read is said once, until it changes again.
pub async fn watch_file(
    store: std::sync::Arc<PolicyStore>,
    stop: tokio_util::sync::CancellationToken,
) {
    let Some(path) = store.path.clone() else {
        return;
    };
    let stamp = |path: &Path| {
        std::fs::metadata(path)
            .ok()
            .map(|meta| (meta.modified().ok(), meta.len()))
    };
    let mut seen = stamp(&path);
    let mut every = tokio::time::interval(FILE_CHECK);
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            () = stop.cancelled() => return,
            _ = every.tick() => {}
        }
        let now = stamp(&path);
        if now == seen {
            continue;
        }
        seen = now;
        match store.reload_from_file() {
            Ok(true) => {
                tracing::info!("the local policy was changed on the machine; it is in force now");
            }
            Ok(false) => {}
            Err(error) => tracing::warn!(
                %error,
                "the local policy in the file cannot be read; the policy in force stays as it was"
            ),
        }
    }
}

/// Writes a file so that it is either as it was or as it is now.
pub fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".writing");
    let temporary = PathBuf::from(temporary);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, path)
}

/// The approval mode a command asks for, for a log line.
#[must_use]
pub fn approval_name(mode: ApprovalMode) -> &'static str {
    match mode {
        ApprovalMode::Ask => "ask",
        ApprovalMode::OnRequest => "on_request",
        ApprovalMode::Auto => "auto",
        ApprovalMode::Bypass => "bypass",
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_kept_fetch_refusal_is_rechecked_before_the_node_can_download_any_build() {
        let mut config = orch_config::ConnectorConfig::default();
        config.policy.delegated = vec!["*".into()];
        let policy = super::PolicyStore::connected(config, None).unwrap();
        assert!(policy.require_tools_fetch().is_ok());
        policy
            .set_here(
                orch_link_contract::policy::PolicyKey::ToolsFetch,
                &serde_json::json!(false),
            )
            .unwrap();
        let error = policy.require_tools_fetch().unwrap_err();
        assert!(
            error.detail.contains("tools.fetch")
                && error.detail.contains("agent role")
                && error.detail.contains("operator")
        );
    }
    use super::*;

    #[test]
    fn individual_harness_confirmations_preserve_other_kinds_and_refuse_stale_changes() {
        let store = PolicyStore::connected(ConnectorConfig::default(), None).unwrap();
        store.offered_roles(&[orch_link_contract::session::Role::Environment]);
        assert!(store.change_harness("codex", None, Some("/one")).is_err());
        store.delegate(&["agent.harnesses".into()]).unwrap();
        store.change_harness("codex", None, Some("/one")).unwrap();
        store
            .change_harness("claude-code", None, Some("/two"))
            .unwrap();
        assert!(store.change_harness("codex", None, Some("/stale")).is_err());
        assert!(store.change_harness("codex", Some("/stale"), None).is_err());
        store.change_harness("codex", Some("/one"), None).unwrap();
        let current = store.policy().unwrap();
        assert!(current.harness("codex").is_none());
        assert_eq!(current.harness("claude-code").unwrap().path, "/two");
    }

    #[test]
    fn two_managers_confirming_different_harnesses_do_not_overwrite_each_other() {
        let store =
            std::sync::Arc::new(PolicyStore::connected(ConnectorConfig::default(), None).unwrap());
        store.offered_roles(&[orch_link_contract::session::Role::Environment]);
        store.delegate(&["agent.harnesses".into()]).unwrap();
        let ready = std::sync::Arc::new(std::sync::Barrier::new(2));
        std::thread::scope(|scope| {
            for (kind, path) in [("codex", "/one"), ("claude-code", "/two")] {
                let store = std::sync::Arc::clone(&store);
                let ready = std::sync::Arc::clone(&ready);
                scope.spawn(move || {
                    ready.wait();
                    store.change_harness(kind, None, Some(path)).unwrap();
                });
            }
        });
        let policy = store.policy().unwrap();
        assert_eq!(policy.harness("codex").unwrap().path, "/one");
        assert_eq!(policy.harness("claude-code").unwrap().path, "/two");
    }
    use orch_link_contract::commands::{
        EnvAttach, ExecStart, FsMaterialize, FsWrite, IndexMode, LeaseOpen, PtyOpen, SessionMode,
        SessionOpen, SessionOpenExec, SessionOpenMount, TurnStart,
    };
    use orch_types::new_id;
    use serde_json::json;

    fn open(directory: &str, executor: Option<&str>) -> SessionOpen {
        SessionOpen {
            agent_id: new_id(),
            thread_id: new_id(),
            segment_id: new_id(),
            mode: SessionMode::New,
            native_session_ref: None,
            archive: None,
            history: None,
            upto_turn_ordinal: None,
            workspace_root: directory.to_owned(),
            instruction_root_marker: false,
            directory: directory.to_owned(),
            cwd: directory.to_owned(),
            mounts: Vec::new(),
            model: "fake-large".to_owned(),
            effort: None,
            params: serde_json::Map::new(),
            approval_mode: ApprovalMode::OnRequest,
            sandbox_mode: SandboxMode::Workspace,
            instructions: String::new(),
            available_models: vec!["fake-large".to_owned()],
            subagent_limits: Default::default(),
            memory: None,
            arrangement: ArrangementType::Together,
            provider: SessionOpenProvider::Native {
                provider_id: new_id(),
            },
            exec: SessionOpenExec {
                executor: executor.map(str::to_owned),
                tool_set: None,
                environment_id: new_id(),
                generation: 1,
                ticket: None,
            },
            mcp: Vec::new(),
            skills: Vec::new(),
            orch_tools: Vec::new(),
            raw_events: false,
            raw_after: 0,
            side_reading: None,
            workspace_configuration: None,
            orch_tool_descriptions: std::collections::BTreeMap::new(),
            orch_tool_schemas: std::collections::BTreeMap::new(),
        }
    }

    fn code(error: CommandError) -> String {
        error.code
    }

    fn here() -> LocalPolicy {
        LocalPolicy {
            environment_roots: vec!["/srv/work".to_owned()],
            ..LocalPolicy::default()
        }
    }

    #[test]
    fn the_same_key_holds_on_hosts_and_directories_and_an_explicit_host_denial_survives_reload() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(
            &path,
            "[environment]\nterminal = false\n[policy]\ndelegated = ['*']\n",
        )
        .unwrap();
        let mut config = ConnectorConfig::default();
        config.policy.local_keys = Some(vec!["environment.terminal".into()]);
        config.policy.delegated = vec!["*".into()];
        let host = PolicyStore::host(config, Some(path)).unwrap();
        let personal = PolicyStore::connected(ConnectorConfig::default(), None).unwrap();
        assert!(!host.policy().unwrap().environment_terminal);
        assert!(!personal.policy().unwrap().environment_terminal);
        assert!(host.policy().unwrap().environment_browser);
        assert!(!personal.policy().unwrap().environment_browser);
        assert!(!host.reload_from_file().unwrap());
        let shell = Command::PtyOpen(PtyOpen {
            environment_id: new_id(),
            terminal_id: new_id(),
            cols: 80,
            rows: 24,
            cwd: "/srv/work".into(),
            record: false,
        });
        assert_eq!(code(host.gate(shell.clone()).unwrap_err()), "unsupported");
        assert_eq!(code(personal.gate(shell).unwrap_err()), "unsupported");
    }

    #[test]
    fn handing_over_a_sparse_policy_and_restarting_it_preserves_full_access_and_local_constraints()
    {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        let store = PolicyStore::connected(ConnectorConfig::default(), Some(path.clone())).unwrap();
        store.delegate(&["*".into()]).unwrap();
        assert!(store.policy().unwrap().environment_terminal);
        assert!(store.policy().unwrap().environment_after_up);
        store
            .set_here(PolicyKey::EnvironmentBrowser, &json!(false))
            .unwrap();
        let changed = store
            .apply_from_system(&BTreeMap::from([
                ("environment.browser".into(), json!(true)),
                ("environment.terminal".into(), json!(false)),
            ]))
            .unwrap();
        assert_eq!(changed.0.refused, ["environment.browser"]);
        assert_eq!(changed.0.applied, ["environment.terminal"]);
        let text = std::fs::read_to_string(&path).unwrap();
        let config: ConnectorConfig = toml::from_str(&text).unwrap();
        let restarted = PolicyStore::connected(config, Some(path)).unwrap();
        assert_eq!(restarted.policy(), store.policy());
        assert!(!restarted.policy().unwrap().environment_browser);
        assert!(!restarted.policy().unwrap().environment_terminal);
        assert!(restarted.policy().unwrap().environment_after_up);
    }

    #[test]
    fn a_program_only_file_edit_refreshes_the_registry_without_changing_its_policy() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        let store = PolicyStore::host(ConnectorConfig::default(), Some(path.clone())).unwrap();
        store.delegate(&["agent.programs".into()]).unwrap();
        assert!(!store.reload_from_file().unwrap());
        let policy = store.policy();
        let mut saved: ConnectorConfig =
            toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let program = orch_config::connector::ConnectorPlugin {
            kind: "adapter".into(),
            name: "local-agent".into(),
            command: vec!["local-agent".into()],
            ..Default::default()
        };
        saved.plugin.push(program.clone());
        let local = saved.policy.local_keys.as_ref().unwrap();
        std::fs::write(&path, sparse_config(&saved, local).unwrap()).unwrap();
        assert!(store.reload_from_file().unwrap());
        assert_eq!(store.config().plugin, [program]);
        assert_eq!(
            store.policy(),
            policy,
            "a registry edit grants no policy right"
        );
        assert!(!store.reload_from_file().unwrap());
    }

    #[test]
    fn keeping_a_shipped_value_is_explicit_and_a_bad_edit_leaves_full_access_in_force() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        let store = PolicyStore::host(ConnectorConfig::default(), Some(path.clone())).unwrap();
        store.delegate(&["environment.*".into()]).unwrap();
        store
            .set_here(PolicyKey::EnvironmentTerminal, &json!(true))
            .unwrap();
        let (change, _) = store
            .apply_from_system(&BTreeMap::from([(
                "environment.terminal".into(),
                json!(false),
            )]))
            .unwrap();
        assert_eq!(change.refused, ["environment.terminal"]);
        assert!(
            store
                .policy()
                .unwrap()
                .local_values
                .contains_key("environment.terminal")
        );
        assert!(!store.reload_from_file().unwrap());
        let valid = store.policy();
        let text = std::fs::read_to_string(&path).unwrap();
        let mut broken: ConnectorConfig = toml::from_str(&text).unwrap();
        broken
            .policy
            .values
            .insert("environment.typo".into(), json!(false));
        std::fs::write(&path, toml::to_string(&broken).unwrap()).unwrap();
        let error = store.reload_from_file().unwrap_err().to_string();
        assert!(error.contains("environment.typo"), "{error}");
        assert_eq!(store.policy(), valid);
        assert!(store.policy().unwrap().environment_browser);
        for (key, value) in [
            ("agent.max_sandbox", "unrecognized"),
            ("provider.arrangements", "unrecognized"),
        ] {
            let mut broken: ConnectorConfig = toml::from_str(&text).unwrap();
            if key == "agent.max_sandbox" {
                broken.agent.max_sandbox = value.into();
            } else {
                broken.provider.arrangements = vec![value.into()];
            }
            std::fs::write(&path, toml::to_string(&broken).unwrap()).unwrap();
            let error = store.reload_from_file().unwrap_err().to_string();
            assert!(error.contains(key), "{error}");
            assert_eq!(
                store.policy(),
                valid,
                "an invalid value never changes the live ceiling or defaults"
            );
        }
    }

    #[test]
    fn an_after_start_refusal_names_the_environment_key_and_its_current_decider() {
        let command = Command::UnitAfterUp(orch_link_contract::commands::UnitAfterUp {
            run_id: orch_types::new_id(),
            environment_id: Some(orch_types::new_id()),
            provider_id: None,
            agent_id: None,
            position: 0,
            name: "prepare".into(),
            run: orch_settings_contract::unit::AfterUpCommand {
                command: vec!["/bin/true".into()],
                timeout_secs: 30,
            },
        });
        let off = BTreeMap::from([("environment.after_up".into(), json!(false))]);
        let handed =
            LocalPolicy::resolve(&["environment.*".into()], &BTreeMap::new(), &off).unwrap();
        let error = gate(&handed, command.clone()).unwrap_err();
        assert!(
            error
                .detail
                .contains("environment.after_up (environment role)")
                && error.detail.contains("managers control")
        );
        assert_eq!(error.details.unwrap()["decides"], "machine_managers");
        let kept = LocalPolicy::resolve(&["*".into()], &off, &BTreeMap::new()).unwrap();
        let error = gate(&kept, command).unwrap_err();
        assert!(
            error.detail.contains("operator keeps")
                && error
                    .detail
                    .contains("orch-connector policy set environment.after_up")
        );
        assert_eq!(error.details.unwrap()["decides"], "machine_operator");
    }

    #[test]
    fn a_delegated_ports_value_with_cleared_optional_members_survives_save_and_restart() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        let store = PolicyStore::connected(ConnectorConfig::default(), Some(path.clone())).unwrap();
        store.delegate(&["environment.*".into()]).unwrap();
        let (changed, _) = store
            .apply_from_system(&BTreeMap::from([(
                "ports".into(),
                json!({"allowed":["place"],"publish":null,"limits":null}),
            )]))
            .unwrap();
        assert_eq!(changed.applied, ["ports"]);
        let data = crate::enrollment::DataDir::open(directory.path()).unwrap();
        let restarted = PolicyStore::connected(data.config(|_| None).unwrap(), Some(path)).unwrap();
        assert_eq!(restarted.policy(), store.policy());
        assert!(restarted.policy().unwrap().delegates(PolicyKey::Ports));
        assert!(
            restarted
                .policy()
                .unwrap()
                .permits_port(Some(orch_api_contract::ports::PortOpeningKind::Place))
        );
        assert!(
            !restarted
                .policy()
                .unwrap()
                .permits_port(Some(orch_api_contract::ports::PortOpeningKind::Link))
        );
    }

    // Written with Unix directories (`/srv/work`), which Windows reads as relative to the current drive.
    #[cfg(unix)]
    #[test]
    fn the_machine_sandbox_ceiling_holds_for_every_approval_mode() {
        let policy = here();
        let mut body = open("/srv/work/web", None);
        body.approval_mode = ApprovalMode::Bypass;
        body.sandbox_mode = SandboxMode::Open;
        let Command::SessionOpen(body) = gate(&policy, Command::SessionOpen(body)).unwrap().command
        else {
            panic!()
        };
        assert_eq!(body.sandbox_mode, SandboxMode::Workspace);
        let mut reviewed = open("/srv/work/web", None);
        reviewed.approval_mode = ApprovalMode::Auto;
        assert!(
            gate(&policy, Command::SessionOpen(reviewed)).is_ok(),
            "a harness's reviewer is not the mode that never asks"
        );
        let turn = TurnStart {
            plan: false,
            thread_id: new_id(),
            turn_id: new_id(),
            text: String::new(),
            images: Vec::new(),
            model: "m".into(),
            effort: None,
            params: serde_json::Map::new(),
            approval_mode: ApprovalMode::Bypass,
            sandbox_mode: SandboxMode::Workspace,
            native_action: None,
        };
        assert!(gate(&policy, Command::TurnStart(turn)).is_ok());
    }

    // Written with Unix directories (`/srv/work`), which Windows reads as relative to the current drive.
    #[cfg(unix)]
    #[test]
    fn a_sandbox_wider_than_the_ceiling_is_lowered_to_it_and_the_lowering_is_reported() {
        let policy = here();
        let mut body = open("/srv/work/web", None);
        body.sandbox_mode = SandboxMode::Open;
        let gated = gate(&policy, Command::SessionOpen(body)).unwrap();
        let Command::SessionOpen(lowered) = &gated.command else {
            panic!()
        };
        assert_eq!(lowered.sandbox_mode, SandboxMode::Workspace);
        assert_eq!(
            gated.clamps,
            [Clamp {
                key: "agent.max_sandbox",
                asked: "open".into(),
                used: "workspace".into()
            }]
        );
        let narrow = open("/srv/work/web", None);
        assert!(
            gate(&policy, Command::SessionOpen(narrow))
                .unwrap()
                .clamps
                .is_empty()
        );
    }

    // Written with Unix directories (`/srv/work`), which Windows reads as relative to the current drive.
    #[cfg(unix)]
    #[test]
    fn a_directory_outside_what_the_owner_offers_is_refused_for_a_harness_that_runs_here_only() {
        let policy = here();
        let outside = open("/etc", None);
        assert_eq!(
            code(gate(&policy, Command::SessionOpen(outside)).unwrap_err()),
            "policy.root_not_allowed"
        );
        let climbing = open("/srv/work/../../etc", None);
        assert_eq!(
            code(gate(&policy, Command::SessionOpen(climbing)).unwrap_err()),
            "policy.root_not_allowed",
            "a path that climbs out is outside"
        );
        let mut mounted = open("/srv/work/web", None);
        mounted.mounts.push(SessionOpenMount {
            name: "data".into(),
            path: "/var/data".into(),
            read_only: true,
        });
        assert_eq!(
            code(gate(&policy, Command::SessionOpen(mounted)).unwrap_err()),
            "policy.root_not_allowed"
        );
        let inside = open("/srv/work/web/api", None);
        assert!(gate(&policy, Command::SessionOpen(inside)).is_ok());
        let elsewhere = open("/workspace/web", Some("orch-exec"));
        assert!(
            gate(&policy, Command::SessionOpen(elsewhere)).is_ok(),
            "the directory of a harness that works through an executor is on another machine"
        );
    }

    // Written with Unix directories (`/srv/work`), which Windows reads as relative to the current drive.
    #[cfg(unix)]
    #[test]
    fn a_login_kept_here_is_used_only_in_the_arrangements_the_owner_allows() {
        let policy = here();
        let mut body = open("/srv/work/web", None);
        body.arrangement = ArrangementType::CredentialShared;
        assert_eq!(
            code(gate(&policy, Command::SessionOpen(body)).unwrap_err()),
            "policy.arrangement_not_allowed"
        );
    }

    #[test]
    fn nothing_is_written_from_outside_until_the_owner_allows_it() {
        let mut policy = here();
        let write = Command::FsWrite(FsWrite {
            work: None,
            environment_id: new_id(),
            path: "a.txt".into(),
            blob: orch_types::Hash::of(b"x"),
            mode: None,
            expected_hash: None,
        });
        assert_eq!(
            code(gate(&policy, write.clone()).unwrap_err()),
            "policy.remote_write_disabled"
        );
        policy.environment_remote_write = true;
        assert!(gate(&policy, write).is_ok());
        let delivery = Command::FsMaterialize(FsMaterialize {
            work: None,
            environment_id: new_id(),
            job_id: None,
            manifest_blob: orch_types::Hash::of(b"m"),
            target_root: "/srv/work/.orch".into(),
            purpose: MaterializePurpose::Delivery,
        });
        assert!(
            gate(&policy, delivery.clone()).is_ok(),
            "delivered files are on as installed"
        );
        policy.environment_deliver_files = false;
        assert_eq!(
            code(gate(&policy, delivery).unwrap_err()),
            "policy.deliver_files_disabled"
        );
        let moved = Command::FsMaterialize(FsMaterialize {
            work: None,
            environment_id: new_id(),
            job_id: None,
            manifest_blob: orch_types::Hash::of(b"m"),
            target_root: "/srv/work".into(),
            purpose: MaterializePurpose::Move,
        });
        assert!(
            gate(&policy, moved).is_ok(),
            "a move is a write, and writes are allowed now"
        );
    }

    // Written with Unix directories (`/srv/work`), which Windows reads as relative to the current drive.
    #[cfg(unix)]
    #[test]
    fn an_environment_may_be_attached_only_inside_the_offered_directories() {
        let policy = here();
        let attach = |root: &str| {
            Command::EnvAttach(EnvAttach {
                resources: None,
                data_generation: None,
                storage: None,
                data_sources: None,
                environment_id: new_id(),
                workspace_root: Some(root.to_owned()),
                ignore: Vec::new(),
                index: IndexMode::Full,
                mounts: None,
                variables: BTreeMap::new(),
                secrets: BTreeMap::new(),
            })
        };
        assert!(gate(&policy, attach("/srv/work/project")).is_ok());
        assert_eq!(
            code(gate(&policy, attach("/home")).unwrap_err()),
            "policy.root_not_allowed"
        );
    }

    #[test]
    fn a_shell_a_tool_server_and_an_executor_each_wait_for_their_own_switch() {
        let mut policy = here();
        let terminal = Command::PtyOpen(PtyOpen {
            environment_id: new_id(),
            terminal_id: new_id(),
            cols: 80,
            rows: 24,
            cwd: "/srv/work".into(),
            record: false,
        });
        assert_eq!(
            code(gate(&policy, terminal.clone()).unwrap_err()),
            "unsupported"
        );
        policy.environment_terminal = true;
        assert!(gate(&policy, terminal).is_ok());

        let given = Command::McpStart(McpStart::Given(
            serde_json::from_value(json!({
                "environment_id": new_id(), "name": "tmux",
                "spec": { "transport": "stdio", "command": ["tmux"] }, "secrets": {}
            }))
            .unwrap(),
        ));
        assert_eq!(
            code(gate(&policy, given.clone()).unwrap_err()),
            "policy.mcp_add_disabled"
        );
        policy.mcp_add = true;
        assert!(gate(&policy, given).is_ok());

        let exec = |executor: &str| {
            Command::ExecStart(ExecStart {
                environment_id: new_id(),
                executor: executor.into(),
            })
        };
        assert!(gate(&policy, exec("codex-exec-server")).is_ok());
        policy.exec_offers = vec!["plain".into()];
        assert_eq!(
            code(gate(&policy, exec("codex-exec-server")).unwrap_err()),
            "policy.exec_not_offered"
        );
        assert!(
            gate(&policy, exec("orch-exec")).is_ok(),
            "orch-exec is offered while one of its tool sets is"
        );
        policy.exec_offers = Vec::new();
        assert_eq!(
            code(gate(&policy, exec("orch-exec")).unwrap_err()),
            "policy.exec_not_offered"
        );
    }

    #[test]
    fn a_credential_is_leased_only_in_an_arrangement_the_owner_allows() {
        let policy = here();
        let lease = |mode: &str| {
            Command::LeaseOpen(
                serde_json::from_value::<LeaseOpen>(json!({
                    "lease_id": new_id(), "provider_id": new_id(), "thread_id": new_id(),
                    "mode": mode, "allowed_models": []
                }))
                .unwrap(),
            )
        };
        assert!(gate(&policy, lease("forwarded")).is_ok());
        assert_eq!(
            code(gate(&policy, lease("shared")).unwrap_err()),
            "policy.arrangement_not_allowed"
        );
        assert_eq!(
            code(gate(&policy, lease("handover")).unwrap_err()),
            "policy.arrangement_not_allowed"
        );
    }

    #[test]
    fn a_machine_the_system_made_is_held_to_nothing_here() {
        let store = PolicyStore::managed(ConnectorConfig::default());
        assert!(store.is_managed());
        let mut body = open("/etc", None);
        body.sandbox_mode = SandboxMode::Open;
        body.approval_mode = ApprovalMode::Auto;
        let gated = store.gate(Command::SessionOpen(body)).unwrap();
        assert!(gated.clamps.is_empty());
        let (change, policy) = store
            .apply_from_system(&BTreeMap::from([("tools.fetch".to_owned(), json!(true))]))
            .unwrap();
        assert_eq!(change.refused, ["tools.fetch"]);
        assert!(policy.is_none());
    }

    #[test]
    fn the_owner_changes_a_key_on_the_machine_and_it_is_kept_in_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let store = PolicyStore::connected(ConnectorConfig::default(), Some(path.clone())).unwrap();
        store
            .set_here(PolicyKey::EnvironmentRoots, &json!(["/srv/work"]))
            .unwrap();
        store.set_here(PolicyKey::McpAdd, &json!(true)).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let reread: ConnectorConfig =
            orch_config::load_text(&text, "config.toml", |_| None).unwrap();
        assert_eq!(reread.environment.roots, ["/srv/work"]);
        assert!(reread.mcp.add);
        assert!(
            store
                .set_here(PolicyKey::AgentMaxSandbox, &json!("anywhere"))
                .is_err()
        );
        assert_eq!(
            store.policy().unwrap().agent_max_sandbox,
            SandboxMode::Workspace,
            "a value that does not fit changes nothing"
        );
    }

    #[test]
    fn a_change_made_in_the_file_by_another_process_holds_in_the_running_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let running =
            PolicyStore::connected(ConnectorConfig::default(), Some(path.clone())).unwrap();
        running.set_here(PolicyKey::McpAdd, &json!(false)).unwrap();
        assert!(
            !running.reload_from_file().unwrap(),
            "its own write is no change"
        );
        // `orch-connector policy set` in another process, as the owner types it at the terminal.
        let text = std::fs::read_to_string(&path).unwrap();
        let config: ConnectorConfig =
            orch_config::load_text(&text, "config.toml", |_| None).unwrap();
        PolicyStore::connected(config, Some(path.clone()))
            .unwrap()
            .set_here(PolicyKey::EnvironmentTerminal, &json!(true))
            .unwrap();
        let told = running.changes();
        assert!(running.reload_from_file().unwrap());
        assert!(running.policy().unwrap().environment_terminal);
        assert!(
            told.has_changed().unwrap(),
            "whoever follows the policy is told"
        );
        // A file that is not a configuration leaves the policy in force as it was.
        std::fs::write(&path, "this is [not toml").unwrap();
        assert!(running.reload_from_file().is_err());
        assert!(running.policy().unwrap().environment_terminal);
    }

    #[test]
    fn the_system_changes_only_the_keys_the_owner_handed_over_and_never_the_list_of_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let store = PolicyStore::connected(ConnectorConfig::default(), Some(path.clone())).unwrap();
        store.delegate(&["environment.roots".to_owned()]).unwrap();
        let (change, policy) = store
            .apply_from_system(&BTreeMap::from([
                ("environment.roots".to_owned(), json!(["/srv/shared"])),
                ("environment.terminal".to_owned(), json!(true)),
                ("policy.delegated".to_owned(), json!(["*"])),
                ("nonsense".to_owned(), json!(1)),
            ]))
            .unwrap();
        assert_eq!(change.applied, ["environment.roots"]);
        assert_eq!(change.refused.len(), 3);
        let policy = policy.unwrap();
        assert_eq!(policy.environment_roots, ["/srv/shared"]);
        assert!(!policy.environment_terminal);
        assert_eq!(policy.policy_delegated, ["environment.roots"]);
        let kept: ConnectorConfig =
            orch_config::load_text(&std::fs::read_to_string(&path).unwrap(), "t", |_| None)
                .unwrap();
        assert_eq!(
            kept.policy.values.get("environment.roots"),
            Some(&json!(["/srv/shared"]))
        );
        assert!(
            kept.environment.roots.is_empty(),
            "Orch values never become local constraints on reload"
        );
        store.undelegate(&["all".to_owned()]).unwrap();
        let (again, _) = store
            .apply_from_system(&BTreeMap::from([(
                "environment.roots".to_owned(),
                json!(["/x"]),
            )]))
            .unwrap();
        assert!(again.applied.is_empty(), "nothing is handed over any more");
    }

    #[test]
    fn a_change_is_announced_to_whoever_watches_the_policy() {
        let store = PolicyStore::connected(ConnectorConfig::default(), None).unwrap();
        let mut watcher = store.changes();
        assert!(!watcher.has_changed().unwrap());
        store
            .set_here(PolicyKey::UpdateAuto, &json!(false))
            .unwrap();
        assert!(watcher.has_changed().unwrap());
        assert!(!watcher.borrow_and_update().eq(&0));
    }
}
