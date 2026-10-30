//! Start or attach configuration and supported extension kinds.
//!
//! An external implementation is registered in the configuration of the
//! component that hosts its kind, as one `[[plugin]]` entry
//! ([`PluginRegistration`]). The host either starts the program and talks to
//! it over its standard input and output, or attaches to one that is already
//! running, over a local socket. The messages are the same either way.
//!
//! The transport carries no authentication of its own, so an address to
//! attach to is on the machine itself and nowhere else
//! ([`ConnectAddress`]). A program that goes away is started or attached to
//! again after a pause that doubles ([`backoff`]).

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The extension points an external program can implement.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum PluginKind {
    /// Makes and runs the units that environments and provider runtimes are.
    Driver,
    /// Drives one kind of harness.
    Adapter,
    /// Carries out a harness's tool calls inside an environment.
    Executor,
    /// Converts requests of one model API into another.
    Translator,
    /// Writes titles, descriptions and tags from activity.
    Summarizer,
    /// Delivers notifications outside the application.
    Notifier,
    /// Reads a provider's facts and models, spends its resets, or presents its key, beside its credential.
    Provider,
}

/// The component that hosts the implementations of a kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PluginHost {
    /// The environment host.
    Node,
    /// The API server.
    Api,
    /// The worker.
    Worker,
    /// A connector, from that connector's own configuration.
    Connector,
}

impl PluginKind {
    /// Every kind.
    pub const ALL: [Self; 7] = [
        Self::Driver,
        Self::Adapter,
        Self::Executor,
        Self::Translator,
        Self::Summarizer,
        Self::Notifier,
        Self::Provider,
    ];

    /// The kind as it is written.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Driver => "driver",
            Self::Adapter => "adapter",
            Self::Executor => "executor",
            Self::Translator => "translator",
            Self::Summarizer => "summarizer",
            Self::Notifier => "notifier",
            Self::Provider => "provider",
        }
    }

    /// The component whose configuration registers implementations of this
    /// kind, and which starts them.
    #[must_use]
    pub const fn host(self) -> PluginHost {
        match self {
            Self::Driver => PluginHost::Node,
            Self::Summarizer => PluginHost::Api,
            Self::Notifier => PluginHost::Worker,
            Self::Adapter | Self::Executor | Self::Translator | Self::Provider => {
                PluginHost::Connector
            }
        }
    }
}

impl fmt::Display for PluginKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a host does when a program it started is gone.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Restart {
    /// Leave it down.
    Never,
    /// Start it again unless it ended by itself without an error.
    #[default]
    OnFailure,
    /// Start it again whatever it ended with.
    Always,
}

/// How a program that the host started came to be gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// It exited by itself and reported success.
    Clean,
    /// It exited with an error, was killed, or stopped answering.
    Failed,
}

impl Restart {
    /// Whether a program that ended as `ended` is started again.
    #[must_use]
    pub const fn restarts(self, ended: Ended) -> bool {
        match (self, ended) {
            (Self::Never, _) | (Self::OnFailure, Ended::Clean) => false,
            (Self::OnFailure, Ended::Failed) | (Self::Always, _) => true,
        }
    }
}

/// The pause before the first new start or attachment.
pub const BACKOFF_FIRST: Duration = Duration::from_secs(1);
/// The longest pause between two tries: long enough that a program that
/// cannot start costs nothing, short enough that one that was repaired is
/// back within a minute.
pub const BACKOFF_MOST: Duration = Duration::from_secs(60);

/// The pause before try number `attempt` (the first is 0) of starting or
/// attaching to a program that is gone: it doubles from [`BACKOFF_FIRST`]
/// and stays at [`BACKOFF_MOST`].
#[must_use]
pub fn backoff(attempt: u32) -> Duration {
    let doubled = 1u64
        .checked_shl(attempt)
        .and_then(|factor| BACKOFF_FIRST.as_secs().checked_mul(factor));
    match doubled {
        Some(secs) if secs < BACKOFF_MOST.as_secs() => Duration::from_secs(secs),
        _ => BACKOFF_MOST,
    }
}

/// One `[[plugin]]` entry of a host component's configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PluginRegistration {
    /// The extension point it implements.
    pub kind: PluginKind,
    /// Its name. No built-in implementation of that kind may have it.
    pub name: String,
    /// The program and its arguments, for one that the host starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    /// Where one that is already running is attached to: `unix:<path>` or
    /// `tcp://127.0.0.1:<port>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect: Option<String>,
    /// Variables the started program finds in its environment.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// What the host does when the started program is gone. Left out, it is
    /// started again unless it ended without an error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart: Option<Restart>,
}

/// Where a program that is already running is attached to. Both forms are
/// on the machine itself.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ConnectAddress {
    /// A socket in the file system.
    Unix(String),
    /// A port on the loopback address.
    Tcp(u16),
}

/// An address that cannot be attached to.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConnectAddressError {
    /// It is in neither form.
    #[error("an address to attach to is unix:<path> or tcp://127.0.0.1:<port>")]
    Form,
    /// It names another host than the machine itself.
    #[error(
        "a plugin is attached to on 127.0.0.1 only: the transport has no authentication of its own"
    )]
    NotLoopback,
    /// Its port is no port.
    #[error("{0:?} is no port")]
    Port(String),
}

impl FromStr for ConnectAddress {
    type Err = ConnectAddressError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if let Some(path) = text.strip_prefix("unix:") {
            return if path.is_empty() {
                Err(ConnectAddressError::Form)
            } else {
                Ok(Self::Unix(path.to_owned()))
            };
        }
        let Some(rest) = text.strip_prefix("tcp://") else {
            return Err(ConnectAddressError::Form);
        };
        let Some((host, port)) = rest.rsplit_once(':') else {
            return Err(ConnectAddressError::Form);
        };
        if host != "127.0.0.1" {
            return Err(ConnectAddressError::NotLoopback);
        }
        match port.parse::<u16>() {
            Ok(port) if port != 0 => Ok(Self::Tcp(port)),
            _ => Err(ConnectAddressError::Port(port.to_owned())),
        }
    }
}

impl fmt::Display for ConnectAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unix(path) => write!(f, "unix:{path}"),
            Self::Tcp(port) => write!(f, "tcp://127.0.0.1:{port}"),
        }
    }
}

/// How a host reaches a registered implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginStart {
    /// It starts the program and talks over its standard input and output;
    /// what the program writes to standard error is its log.
    Command {
        /// The program and its arguments.
        argv: Vec<String>,
        /// Variables added to its environment.
        env: BTreeMap<String, String>,
        /// What happens when it is gone.
        restart: Restart,
    },
    /// It attaches to one that is already running.
    Attach(ConnectAddress),
}

/// Why an entry cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistrationError {
    /// It has no name.
    #[error("a plugin needs a name")]
    NoName,
    /// It says both how to start the program and where to attach to it, or
    /// neither.
    #[error("plugin {0:?}: exactly one of command and connect is given")]
    CommandOrConnect(String),
    /// Its command names no program.
    #[error("plugin {0:?}: command names no program")]
    EmptyCommand(String),
    /// Its address cannot be attached to.
    #[error("plugin {name:?}: {source}")]
    Connect {
        /// The plugin's name.
        name: String,
        /// What is wrong with the address.
        source: ConnectAddressError,
    },
    /// A built-in implementation of the same kind has its name.
    #[error("plugin {name:?}: a built-in {kind} has that name")]
    NameTaken {
        /// The extension point.
        kind: PluginKind,
        /// The name.
        name: String,
    },
}

impl PluginRegistration {
    /// How the host reaches this implementation, when the entry is one that
    /// can be used: it has a name, and exactly one of `command` and
    /// `connect`. `env` and `restart` say nothing for a program that is
    /// attached to: it has its own environment, and is always attached to
    /// again.
    pub fn start(&self) -> Result<PluginStart, RegistrationError> {
        if self.name.is_empty() {
            return Err(RegistrationError::NoName);
        }
        match (&self.command, &self.connect) {
            (Some(argv), None) => {
                if argv.first().is_none_or(String::is_empty) {
                    return Err(RegistrationError::EmptyCommand(self.name.clone()));
                }
                Ok(PluginStart::Command {
                    argv: argv.clone(),
                    env: self.env.clone(),
                    restart: self.restart.unwrap_or_default(),
                })
            }
            (None, Some(address)) => address.parse().map(PluginStart::Attach).map_err(|source| {
                RegistrationError::Connect {
                    name: self.name.clone(),
                    source,
                }
            }),
            _ => Err(RegistrationError::CommandOrConnect(self.name.clone())),
        }
    }

    /// Refuses a name that one of `builtin`, the built-in implementations
    /// of the same kind on this host, already has.
    pub fn check_name(&self, builtin: &[&str]) -> Result<(), RegistrationError> {
        if builtin.contains(&self.name.as_str()) {
            Err(RegistrationError::NameTaken {
                kind: self.kind,
                name: self.name.clone(),
            })
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry() -> PluginRegistration {
        serde_json::from_value(json!({
            "kind": "driver",
            "name": "firecracker",
            "command": ["/opt/orch-plugins/orch-driver-firecracker"],
            "env": { "FC_KERNEL": "/var/lib/fc/vmlinux" },
            "restart": "on-failure"
        }))
        .unwrap()
    }

    #[test]
    fn an_entry_is_read_as_the_configuration_writes_it() {
        let entry = entry();
        assert_eq!(entry.kind, PluginKind::Driver);
        assert_eq!(entry.restart, Some(Restart::OnFailure));
        assert_eq!(
            serde_json::to_value(&entry).unwrap(),
            json!({
                "kind": "driver",
                "name": "firecracker",
                "command": ["/opt/orch-plugins/orch-driver-firecracker"],
                "env": { "FC_KERNEL": "/var/lib/fc/vmlinux" },
                "restart": "on-failure"
            })
        );
        assert!(
            serde_json::from_value::<PluginRegistration>(
                json!({ "kind": "driver", "name": "x", "comand": ["x"] })
            )
            .is_err()
        );
    }

    #[test]
    fn a_started_program_is_restarted_after_a_failure_unless_the_entry_says_otherwise() {
        let mut entry = entry();
        entry.restart = None;
        let PluginStart::Command { restart, env, .. } = entry.start().unwrap() else {
            panic!("a command is started");
        };
        assert_eq!(restart, Restart::OnFailure);
        assert_eq!(env["FC_KERNEL"], "/var/lib/fc/vmlinux");
        assert!(restart.restarts(Ended::Failed));
        assert!(!restart.restarts(Ended::Clean));
        assert!(Restart::Always.restarts(Ended::Clean));
        assert!(!Restart::Never.restarts(Ended::Failed));
    }

    #[test]
    fn exactly_one_of_command_and_connect_is_given() {
        let mut both = entry();
        both.connect = Some("unix:/run/orch-plugins/firecracker.sock".into());
        assert_eq!(
            both.start(),
            Err(RegistrationError::CommandOrConnect("firecracker".into()))
        );
        let mut neither = entry();
        neither.command = None;
        assert_eq!(
            neither.start(),
            Err(RegistrationError::CommandOrConnect("firecracker".into()))
        );
        let mut empty = entry();
        empty.command = Some(vec![]);
        assert_eq!(
            empty.start(),
            Err(RegistrationError::EmptyCommand("firecracker".into()))
        );
    }

    #[test]
    fn a_plugin_is_attached_to_on_the_machine_itself_only() {
        let mut attached = entry();
        attached.command = None;
        attached.connect = Some("unix:/run/orch-plugins/firecracker.sock".into());
        assert_eq!(
            attached.start(),
            Ok(PluginStart::Attach(ConnectAddress::Unix(
                "/run/orch-plugins/firecracker.sock".into()
            )))
        );
        assert_eq!(
            "tcp://127.0.0.1:7411".parse(),
            Ok(ConnectAddress::Tcp(7411))
        );
        assert_eq!(
            ConnectAddress::Tcp(7411).to_string(),
            "tcp://127.0.0.1:7411"
        );
        for elsewhere in [
            "tcp://10.0.0.5:7411",
            "tcp://0.0.0.0:7411",
            "tcp://localhost:7411",
        ] {
            assert_eq!(
                elsewhere.parse::<ConnectAddress>(),
                Err(ConnectAddressError::NotLoopback)
            );
        }
        assert_eq!(
            "tcp://127.0.0.1:0".parse::<ConnectAddress>(),
            Err(ConnectAddressError::Port("0".into()))
        );
        assert_eq!(
            "http://127.0.0.1:7411".parse::<ConnectAddress>(),
            Err(ConnectAddressError::Form)
        );
    }

    #[test]
    fn a_plugin_may_not_take_the_name_of_a_built_in_implementation() {
        let mut entry = entry();
        assert_eq!(entry.check_name(&["docker", "host"]), Ok(()));
        entry.name = "docker".into();
        assert_eq!(
            entry.check_name(&["docker", "host"]),
            Err(RegistrationError::NameTaken {
                kind: PluginKind::Driver,
                name: "docker".into()
            })
        );
    }

    #[test]
    fn the_pause_before_a_new_start_doubles_from_a_second_to_a_minute() {
        let pauses: Vec<u64> = (0..9).map(|attempt| backoff(attempt).as_secs()).collect();
        assert_eq!(pauses, [1, 2, 4, 8, 16, 32, 60, 60, 60]);
        assert_eq!(backoff(u32::MAX), BACKOFF_MOST);
    }

    #[test]
    fn each_kind_is_hosted_by_one_component() {
        assert_eq!(PluginKind::Driver.host(), PluginHost::Node);
        assert_eq!(PluginKind::Summarizer.host(), PluginHost::Api);
        assert_eq!(PluginKind::Notifier.host(), PluginHost::Worker);
        for kind in [
            PluginKind::Adapter,
            PluginKind::Executor,
            PluginKind::Translator,
            PluginKind::Provider,
        ] {
            assert_eq!(kind.host(), PluginHost::Connector);
        }
        assert_eq!(
            serde_json::to_value(PluginKind::ALL).unwrap(),
            json!([
                "driver",
                "adapter",
                "executor",
                "translator",
                "summarizer",
                "notifier",
                "provider"
            ])
        );
        for kind in PluginKind::ALL {
            assert_eq!(serde_json::to_value(kind).unwrap(), json!(kind.as_str()));
        }
    }
}
