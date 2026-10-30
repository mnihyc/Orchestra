//! External plugin behavioral conformance checks, by the suites of orch-conformance.
//!
//! `orchctl plugin check <command…>` starts the program with that command line, and `--connect <address>`
//! attaches to one that runs (`unix:<path>` or `tcp://127.0.0.1:<port>`); either is then checked against what
//! Orch asks of an implementation of its kind: first the plugin transport's own rules, then its kind's suite,
//! the very cases that the built-in implementations' tests run. Its kind is what it answers `hello` with, unless
//! `--kind` names it. One line is printed for each rule (passed, FAILED or skipped, with why), and the status is
//! 0 when no rule failed and 1 otherwise; a program that cannot be started or reached is a failure too.
//!
//! `plugin add` runs the same checks before appending a program to a connector's configuration. A failed
//! check leaves the file untouched. An address is a provider's source, chosen through its Sources page,
//! rather than an executable registration in local configuration.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Subcommand};
use orch_conformance::report::Report;
use orch_conformance::{
    agent, driver, executor, notifier, provider, summarizer, translator, transport,
};
use orch_driver_contract::driver::EnvironmentDriver;
use orch_driver_contract::runtime::RuntimeSpec;
use orch_plugin::bridges::agent::AgentBridge;
use orch_plugin::bridges::driver::DriverBridge;
use orch_plugin::bridges::executor::ExecutorBridge;
use orch_plugin::bridges::notifier::NotifierBridge;
use orch_plugin::bridges::provider::ProviderBridge;
use orch_plugin::bridges::summarizer::SummarizerBridge;
use orch_plugin::bridges::translator::TranslatorBridge;
use orch_plugin::{Options, Plugin};
use orch_plugin_contract::registration::{PluginKind, PluginRegistration, Restart};
use orch_provider_contract::source::{CredentialPresenter, FactsSource, ModelsSource};
use orch_types::Json;

use super::console::{refuse, say};

#[path = "plugin_address.rs"]
mod address;

/// The name a checked program is registered under while it is checked: the check installs nothing, so any name
/// serves, and this one says what it is in the program's own log.
const CHECK_NAME: &str = "check";

/// External implementations of Orch's extension points, checked before they are registered.
#[derive(Debug, Args)]
pub struct PluginArgs {
    #[command(subcommand)]
    command: PluginCommand,
}

#[derive(Debug, Subcommand)]
enum PluginCommand {
    /// Check a program against the suite of the kind it implements: the
    /// transport's own rules, then its kind's. One line a rule; the status is 1
    /// when one failed.
    Check(CheckArgs),
    /// Check a connector program, then register it in a named local config.toml.
    Add(AddArgs),
}

#[derive(Debug, Args)]
struct AddArgs {
    /// Its registered name, unique among programs of its kind.
    #[arg(long)]
    name: String,
    /// The connector's config.toml; existing settings and comments are preserved.
    #[arg(long, value_name = "FILE")]
    config_file: PathBuf,
    #[command(flatten)]
    check: CheckArgs,
}

#[derive(Debug, Args)]
struct CheckArgs {
    /// Attach to a program that is already running instead of starting one:
    /// `unix:<path>` or `tcp://127.0.0.1:<port>`.
    #[arg(long, value_name = "ADDRESS", conflicts_with_all = ["command", "address"])]
    connect: Option<String>,
    /// Check a provider source at an HTTP(S) address through the connector's address binding.
    #[arg(long, value_name = "URL", conflicts_with_all = ["command", "connect"])]
    address: Option<String>,
    /// Read this address's authorization token from a file; its contents are never printed.
    #[arg(long, value_name = "FILE", requires = "address")]
    token_file: Option<PathBuf>,
    /// Send the file's token as this header instead of a bearer token.
    #[arg(long, value_name = "NAME", requires = "token_file")]
    auth_header: Option<String>,
    /// Permit plain HTTP for this operator-requested address check.
    #[arg(long, requires = "address")]
    allow_http: bool,
    /// Explicitly allow this address to receive the disposable fixture's provider key.
    #[arg(long, requires = "address")]
    send_credential: bool,
    /// The kind it implements, where its answer to hello is not to be relied
    /// on: provider, driver, adapter, executor, translator, summarizer or notifier.
    #[arg(long, value_name = "KIND")]
    kind: Option<String>,
    /// For a summarizer: the rest of its `auto.summarizer` setting, as JSON.
    #[arg(long, value_name = "JSON", default_value = "{}")]
    config: String,
    /// For a driver: a JSON file containing its template spec. The check
    /// creates, runs and removes a disposable unit and its three volumes.
    #[arg(long, value_name = "FILE")]
    spec: Option<PathBuf>,
    /// For a provider, adapter, executor, notifier or translator: a JSON fixture
    /// naming disposable contexts and controlled counterparts.
    #[arg(long, value_name = "FILE")]
    fixture: Option<PathBuf>,
    /// Check only transport behavior, explicitly leaving the kind's behavior unchecked.
    #[arg(long)]
    transport_only: bool,
    /// The program to start, and its arguments.
    #[arg(
        value_name = "COMMAND",
        trailing_var_arg = true,
        allow_hyphen_values = true,
        required_unless_present_any = ["connect", "address"]
    )]
    command: Vec<String>,
}

/// Does what the command asks and says what came of it.
#[must_use]
pub fn run(args: &PluginArgs) -> ExitCode {
    match &args.command {
        PluginCommand::Check(check_args) => check(check_args),
        PluginCommand::Add(add_args) => add(add_args),
    }
}

fn add(args: &AddArgs) -> ExitCode {
    if args.check.address.is_some() {
        return refuse(
            "an address is selected in a provider's Sources; plugin add registers local programs",
        );
    }
    if args.check.transport_only {
        return refuse("plugin add requires the kind's full suite; remove --transport-only");
    }
    if args.name.trim().is_empty() {
        return refuse("a registered program needs a name");
    }
    let named = match args.check.kind.as_deref().map(kind_of).transpose() {
        Ok(named) => named,
        Err(message) => return refuse(message),
    };
    let config = match serde_json::from_str(&args.check.config) {
        Ok(config) => config,
        Err(_) => return refuse("--config must be a JSON object"),
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => return refuse(format!("the check could not start its runtime: {error}")),
    };
    let outcome = runtime.block_on(async {
        let kind = match named {
            Some(kind) => kind,
            None => {
                let start = registration(&args.check, PluginKind::Driver).start()
                    .map_err(|error| error.to_string())?;
                transport::discover(&start, env!("ORCH_RELEASE_VERSION")).await
                    .map_err(|error| format!("the program did not say what it is: {error}"))?.kind
            }
        };
        if kind.host() != orch_plugin_contract::registration::PluginHost::Connector {
            return Err("plugin add writes connector registrations: provider, translator, adapter or executor".to_owned());
        }
        let mut entry = registration(&args.check, kind);
        entry.name.clone_from(&args.name);
        entry.restart = Some(Restart::OnFailure);
        // Refuse a collision before starting a potentially expensive check.
        registration_text(&read_registration_file(&args.config_file)?, &entry)?;
        let report = checked(&args.check, Some(kind), &config).await?;
        let _ = say(&report);
        if !report.passed() {
            return Err("the program failed its checks; no registration was written".to_owned());
        }
        // Read again after the check to retain any settings edited while it ran.
        let current = read_registration_file(&args.config_file)?;
        let updated = registration_text(&current, &entry)?;
        write_registration_file(&args.config_file, updated.as_bytes())?;
        Ok(())
    });
    match outcome {
        Ok(()) => {
            let _ = say(format!(
                "Registered {:?}; restart the connector to load it.",
                args.name
            ));
            ExitCode::SUCCESS
        }
        Err(message) => refuse(message),
    }
}

fn read_registration_file(path: &Path) -> Result<String, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(format!("cannot read the connector configuration: {error}")),
    }
}

fn registration_text(text: &str, entry: &PluginRegistration) -> Result<String, String> {
    let config: orch_config::ConnectorConfig =
        orch_config::load_text(text, "connector config", |_| None)
            .map_err(|error| error.to_string())?;
    entry.start().map_err(|error| error.to_string())?;
    let programs = orch_agent_programs::Programs::builtin();
    let builtin: Vec<&str> = match entry.kind.as_str() {
        "adapter" => programs.kinds().collect(),
        "executor" => programs
            .executors()
            .map(|executor| executor.kind.as_str())
            .collect(),
        "provider" => orch_provider_contract::choice::builtin::ALL
            .into_iter()
            .chain(["none"])
            .collect(),
        _ => Vec::new(),
    };
    entry
        .check_name(&builtin)
        .map_err(|error| error.to_string())?;
    if config
        .plugin
        .iter()
        .any(|plugin| plugin.kind == entry.kind.as_str() && plugin.name == entry.name)
    {
        return Err(format!(
            "a {} program named {:?} is registered already",
            entry.kind, entry.name
        ));
    }
    #[derive(serde::Serialize)]
    struct Added<'a> {
        plugin: [&'a PluginRegistration; 1],
    }
    let added = toml::to_string(&Added { plugin: [entry] })
        .map_err(|_| "the plugin registration cannot be written".to_owned())?;
    let updated = format!("{text}\n{added}");
    let _: orch_config::ConnectorConfig =
        orch_config::load_text(&updated, "connector config", |_| None)
            .map_err(|error| error.to_string())?;
    Ok(updated)
}

fn write_registration_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temporary = parent.join(format!(".orch-plugin-{}.writing", orch_types::new_id()));
    let original = std::fs::metadata(path)
        .ok()
        .map(|metadata| metadata.permissions());
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let written = (|| -> std::io::Result<()> {
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        if let Some(permissions) = original {
            file.set_permissions(permissions)?;
        }
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)
    })();
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("cannot save the connector configuration: {error}"));
    }
    Ok(())
}

fn check(args: &CheckArgs) -> ExitCode {
    let config: Json = match serde_json::from_str(&args.config) {
        Ok(config) => config,
        Err(error) => return refuse(format!("--config is not JSON: {error}")),
    };
    let named = match args.kind.as_deref().map(kind_of).transpose() {
        Ok(named) => named,
        Err(message) => return refuse(message),
    };
    // A plugin behind a bridge answers on tasks of their own, and a bridge of a contract whose trait is not async
    // waits on them: a runtime of several threads serves both.
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => return refuse(format!("the check could not start its runtime: {error}")),
    };
    match runtime.block_on(checked(args, named, &config)) {
        Ok(report) => {
            // A closed standard output has nobody to read the report; the status still says how it went.
            let _ = say(&report);
            if report.passed() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(message) => refuse(message),
    }
}

/// The kind named by a word, as a configuration file writes it.
fn kind_of(word: &str) -> Result<PluginKind, String> {
    serde_json::from_value(Json::String(word.to_owned())).map_err(|_| {
        format!(
            "{word:?} is no kind of extension: provider, driver, adapter, executor, translator, summarizer or notifier"
        )
    })
}

/// The program or the address, as the person gave it.
fn target(args: &CheckArgs) -> String {
    match &args.connect {
        Some(address) => address.clone(),
        None => args.command.join(" "),
    }
}

/// The registration the check starts the program by: never restarted, since a program that dies while it is
/// checked has failed the check.
fn registration(args: &CheckArgs, kind: PluginKind) -> PluginRegistration {
    PluginRegistration {
        kind,
        name: CHECK_NAME.to_owned(),
        command: (!args.command.is_empty()).then(|| args.command.clone()),
        connect: args.connect.clone(),
        env: BTreeMap::new(),
        restart: Some(Restart::Never),
    }
}

async fn checked(
    args: &CheckArgs,
    named: Option<PluginKind>,
    config: &Json,
) -> Result<Report, String> {
    if args.address.is_some() {
        return address::checked(args, named).await;
    }
    let version = env!("ORCH_RELEASE_VERSION");
    let start = registration(args, named.unwrap_or(PluginKind::Driver))
        .start()
        .map_err(|error| error.to_string())?;
    let (kind, subject) = match named {
        Some(kind) => (kind, format!("{kind} {}", target(args))),
        None => {
            let hello = transport::discover(&start, version)
                .await
                .map_err(|error| format!("{} did not say what it is: {error}", target(args)))?;
            (
                hello.kind,
                format!("{} {:?} {}", hello.kind, hello.name, hello.version),
            )
        }
    };
    let mut report = Report::new(subject);
    if transport::run(&start, kind, version, &mut report)
        .await
        .is_none()
    {
        return Ok(report);
    }
    if args.transport_only {
        return Ok(report);
    }
    match kind {
        PluginKind::Provider => {
            let fixture = provider_fixture(args)?;
            let plugin = Plugin::start(&registration(args, kind), Options::new(version))
                .await
                .map_err(|error| error.to_string())?;
            plugin.settled().await;
            match ProviderBridge::new(plugin.clone()) {
                Ok(bridge) => {
                    let descriptor = bridge.declared();
                    provider::run(
                        provider::Sources {
                            descriptor: &descriptor,
                            facts: descriptor
                                .facts
                                .is_some()
                                .then_some(&bridge as &dyn FactsSource),
                            models: descriptor
                                .models
                                .is_some()
                                .then_some(&bridge as &dyn ModelsSource),
                            presenter: descriptor
                                .presents
                                .is_some()
                                .then_some(&bridge as &dyn CredentialPresenter),
                        },
                        Some(&fixture),
                        &mut report,
                    )
                    .await;
                }
                Err(_) => report.checked(
                    "provider.bridge",
                    "Its host can take it as a provider source",
                    Err("the program's provider declaration is unavailable or invalid".into()),
                ),
            }
            plugin.shutdown().await;
        }
        PluginKind::Summarizer => {
            let plugin = Plugin::start(&registration(args, kind), Options::new(version))
                .await
                .map_err(|error| error.to_string())?;
            plugin.settled().await;
            match SummarizerBridge::new(plugin.clone()) {
                Ok(bridge) => summarizer::run(&bridge, config, &mut report).await,
                Err(error) => report.checked(
                    "summarizer.bridge",
                    "Its host can take it as a summarizer",
                    Err(error.to_string().into()),
                ),
            }
            plugin.shutdown().await;
        }
        PluginKind::Driver => {
            let spec = args
                .spec
                .as_deref()
                .ok_or("a driver check needs --spec FILE for a disposable unit")?;
            let spec = read_json(spec)?;
            let plugin = Plugin::start(&registration(args, kind), Options::new(version))
                .await
                .map_err(|error| error.to_string())?;
            plugin.settled().await;
            match DriverBridge::new(plugin.clone()) {
                Ok(bridge) => {
                    let base: RuntimeSpec = serde_json::from_value(serde_json::json!({
                        "runtime_id": orch_types::new_id(), "kind": "environment", "name": "base",
                        "driver": bridge.describe().name, "spec": spec, "resources": {},
                        "roles": ["environment"], "gateway_url": "wss://127.0.0.1:41000",
                        "enroll_token": "conformance-only", "ca_cert": "", "tools": [],
                        "volumes": {"workspace":{"name":"base-workspace","storage":null},
                            "home":{"name":"base-home","storage":null}, "state":{"name":"base-state","storage":null}},
                        "mounts": [], "env": {}, "labels": {}
                    })).map_err(|error| error.to_string())?;
                    driver::run(&bridge, &base, "orch-conformance", &mut report).await;
                }
                Err(error) => report.checked(
                    "driver.bridge",
                    "Its host can take it as a driver",
                    Err(error.to_string().into()),
                ),
            }
            plugin.shutdown().await;
        }
        PluginKind::Adapter => {
            let fixture: agent::Fixture = read_fixture(args)?;
            let plugin = Plugin::start(&registration(args, kind), Options::new(version))
                .await
                .map_err(|error| error.to_string())?;
            plugin.settled().await;
            match AgentBridge::new(plugin.clone()) {
                Ok(mut bridge) => agent::run(&mut bridge, Some(&fixture), &mut report).await,
                Err(error) => report.checked(
                    "agent.bridge",
                    "Its host can take it as an adapter",
                    Err(error.to_string().into()),
                ),
            }
            plugin.shutdown().await;
        }
        PluginKind::Executor => {
            let fixture: executor::Fixture = read_fixture(args)?;
            if args.connect.is_some() {
                return Err("an executor isolation check needs two started programs; use a command instead of --connect".into());
            }
            let first = Plugin::start(&registration(args, kind), Options::new(version))
                .await
                .map_err(|error| error.to_string())?;
            first.settled().await;
            let second = match Plugin::start(&registration(args, kind), Options::new(version)).await
            {
                Ok(plugin) => plugin,
                Err(error) => {
                    first.shutdown().await;
                    return Err(error.to_string());
                }
            };
            second.settled().await;
            match (
                ExecutorBridge::new(first.clone()),
                ExecutorBridge::new(second.clone()),
            ) {
                (Ok(mut a), Ok(mut b)) => {
                    executor::run(&mut a, &mut b, &fixture, &mut report).await
                }
                (Err(error), _) | (_, Err(error)) => report.checked(
                    "executor.bridge",
                    "Its host can take it as an executor",
                    Err(error.to_string().into()),
                ),
            }
            first.shutdown().await;
            second.shutdown().await;
        }
        PluginKind::Notifier => {
            let fixture: notifier::Fixture = read_fixture(args)?;
            let plugin = Plugin::start(&registration(args, kind), Options::new(version))
                .await
                .map_err(|error| error.to_string())?;
            plugin.settled().await;
            match NotifierBridge::new(plugin.clone()) {
                Ok(bridge) => notifier::run(&bridge, Some(&fixture), &mut report).await,
                Err(error) => report.checked(
                    "notifier.bridge",
                    "Its host can take it as a notifier",
                    Err(error.to_string().into()),
                ),
            }
            plugin.shutdown().await;
        }
        PluginKind::Translator => {
            let fixture: translator::Fixture = read_fixture(args)?;
            let plugin = Plugin::start(&registration(args, kind), Options::new(version))
                .await
                .map_err(|error| error.to_string())?;
            plugin.settled().await;
            match TranslatorBridge::new(plugin.clone()) {
                Ok(bridge) => translator::run(&bridge, Some(&fixture), &mut report),
                Err(error) => report.checked(
                    "translator.bridge",
                    "Its host can take it as a translator",
                    Err(error.to_string().into()),
                ),
            }
            plugin.shutdown().await;
        }
    }
    Ok(report)
}

fn provider_fixture(args: &CheckArgs) -> Result<provider::Fixture, String> {
    let path = args
        .fixture
        .as_deref()
        .ok_or("a provider check needs --fixture FILE with a disposable account")?;
    serde_json::from_value(read_json(path)?).map_err(|_| {
        "the provider fixture has another shape; secret values are not shown".to_owned()
    })
}

fn read_json(path: &Path) -> Result<Json, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("cannot read the fixture: {error}"))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("the fixture is not JSON: {error}"))
}

fn read_fixture<T: serde::de::DeserializeOwned>(args: &CheckArgs) -> Result<T, String> {
    let path = args
        .fixture
        .as_deref()
        .ok_or("this kind needs --fixture FILE with disposable contexts")?;
    serde_json::from_value(read_json(path)?)
        .map_err(|error| format!("the fixture has the wrong shape: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: &str, name: &str) -> PluginRegistration {
        PluginRegistration {
            kind: kind_of(kind).unwrap(),
            name: name.into(),
            command: Some(vec!["test-program".into()]),
            connect: None,
            env: BTreeMap::new(),
            restart: Some(Restart::OnFailure),
        }
    }

    #[test]
    fn registering_a_checked_program_preserves_comments_and_the_machines_existing_limits() {
        let before = "# the machine's owner wrote this\n[environment]\nremote_write = false\nroots = ['/work']\n";
        let after = registration_text(before, &entry("adapter", "tiny")).unwrap();
        assert!(after.starts_with(before));
        let saved: orch_config::ConnectorConfig =
            orch_config::load_text(&after, "config.toml", |_| None).unwrap();
        assert!(!saved.environment.remote_write);
        assert_eq!(saved.environment.roots, ["/work"]);
        assert_eq!(saved.plugin[0].name, "tiny");
        assert_eq!(saved.plugin[0].command, ["test-program"]);
        assert_eq!(saved.plugin[0].restart, "on-failure");
    }

    #[test]
    fn an_existing_or_builtin_registration_is_refused_without_a_changed_configuration() {
        let before = registration_text("", &entry("executor", "tiny")).unwrap();
        assert!(
            registration_text(&before, &entry("executor", "tiny"))
                .unwrap_err()
                .contains("registered already")
        );
        assert!(registration_text(&before, &entry("adapter", "codex")).is_err());
        let saved: orch_config::ConnectorConfig =
            orch_config::load_text(&before, "config.toml", |_| None).unwrap();
        assert_eq!(saved.plugin.len(), 1);
        assert_eq!(saved.plugin[0].name, "tiny");
    }

    #[test]
    fn a_saved_registration_is_complete_and_replaces_the_file_only_when_writing_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "# retain this\n").unwrap();
        let added = registration_text(
            &read_registration_file(&path).unwrap(),
            &entry("translator", "custom"),
        )
        .unwrap();
        write_registration_file(&path, added.as_bytes()).unwrap();
        let saved: orch_config::ConnectorConfig = orch_config::load_text(
            &std::fs::read_to_string(&path).unwrap(),
            "config.toml",
            |_| None,
        )
        .unwrap();
        assert_eq!(saved.plugin[0].kind, "translator");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        assert!(
            write_registration_file(&dir.path().join("missing/config.toml"), b"invalid").is_err()
        );
        assert_eq!(read_registration_file(&path).unwrap(), added);
    }
}
