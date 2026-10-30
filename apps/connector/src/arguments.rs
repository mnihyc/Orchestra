//! The command line of the connector: what a person types to enroll a machine, run it, look at it and set the limits of its owner.

use std::path::PathBuf;

use clap::{Args as ClapArgs, Parser, Subcommand};

/// The Orch connector: it connects this machine to a system, and lets the system use it as the owner allows.
#[derive(Debug, Parser)]
#[command(name = "orch-connector", version = env!("ORCH_RELEASE_VERSION"))]
pub struct Args {
    /// The connector's data directory: its identity, configuration, spool and caches. Any number of
    /// connectors run on one host, each with a directory of its own.
    #[arg(long, global = true, env = "ORCH_DATA_DIR", value_name = "DIR")]
    pub data_dir: Option<PathBuf>,
    /// What to do. Without one the connector runs, as `run` does.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// What the connector can be told to do.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Keep the local-only declaration that this machine is dedicated to Orch (true or false).
    Dedicated {
        /// Whether the whole machine is the isolation boundary for its directories.
        #[arg(action=clap::ArgAction::Set)]
        value: bool,
    },
    /// Enroll this machine into a system with a token from it.
    Enroll(Enroll),
    /// Hold the session to the system and do what it asks within the owner's limits, until stopped.
    Run(Run),
    /// Show who this machine is to the system, its roles, its limits and what it can host.
    Status,
    /// Show or change the limits the owner of this machine sets.
    #[command(subcommand)]
    Policy(Policy),
    /// Show or change the roles offered by this program; changes remain local to this machine.
    #[command(subcommand)]
    Roles(Roles),
    /// Show the version of this build.
    Version,
    /// Open the console: the screens of this machine's enrollment, environments, tools, limits, providers and
    /// activity, over the local socket of the connector that runs here.
    Console,
    /// Install and control the per-user service that keeps the connector running.
    #[command(subcommand)]
    Service(Service),
    /// Choose the directories environments may be made in.
    #[command(subcommand)]
    Env(Env),
    /// List the tool servers the harnesses here have configured, and forget a copy kept for Orch.
    #[command(subcommand)]
    Mcp(Mcp),
    /// Give a provider's key to this machine.
    #[command(subcommand)]
    Provider(Provider),
    /// Carry a port of an environment to this machine's loopback.
    #[command(subcommand)]
    Port(Port),
    /// Read this environment's ports and identify an exact peer from inside it.
    #[command(subcommand)]
    Ports(Ports),
    /// Update the connector now to the newer build its system offers.
    Update,
    /// Show a plan, then remove this instance's service and the last instance's program.
    Uninstall(Uninstall),
    /// Record the exact files and service placed by the verified installer.
    #[command(name = "__record-installation", hide = true)]
    RecordInstallation(RecordInstallation),
    /// Check connectivity, the clock, the certificate and the harnesses.
    Doctor,
    /// Be the proxy of one restricted unit, in the container a host's node makes for it beside the unit.
    #[cfg(unix)]
    #[command(hide = true)]
    Proxy(Proxy),
}

/// The local uninstall's explicit data choice and single confirmation.
#[derive(Debug, ClapArgs)]
pub struct Uninstall {
    /// Also remove this instance's data and directories it made; borrowed folders are kept.
    #[arg(long)]
    pub purge: bool,
    /// Show the complete plan and leave everything in place.
    #[arg(long, conflicts_with = "yes")]
    pub plan: bool,
    /// Confirm the displayed plan without prompting for the enrolled name.
    #[arg(long)]
    pub yes: bool,
}

/// Installer facts are recorded by the executable that will later remove them.
#[derive(Debug, ClapArgs)]
pub struct RecordInstallation {
    /// A service is installed for this instance.
    #[arg(long)]
    pub service: bool,
    /// The service uses the named data-directory suffix.
    #[arg(long)]
    pub named: bool,
}

/// `proxy`.
#[cfg(unix)]
#[derive(Debug, ClapArgs)]
pub struct Proxy {
    /// Refuse all traffic unless the host-owned address observer has a fresh inventory.
    #[arg(long)]
    pub host_inventory: bool,
    /// The directory of its orders, its heartbeat and its state, which the node keeps.
    #[arg(long, value_name = "DIR")]
    pub dir: PathBuf,
    /// Its address on the unit's private network, the only one it listens on.
    #[arg(long, value_name = "ADDRESS")]
    pub listen: std::net::IpAddr,
}

/// `enroll`.
#[derive(Debug, ClapArgs)]
pub struct Enroll {
    /// The roles to offer: environment, agent, provider, separated by commas.
    #[arg(long, value_name = "ROLES")]
    pub roles: Option<String>,
    /// Hand over selected role sections, separated by commas.
    #[arg(long, value_name = "SECTIONS", conflicts_with = "delegate_policy")]
    pub delegate: Option<String>,
    /// Hand over every policy key to Orch; explicit locally kept values still win.
    #[arg(long, conflicts_with = "delegate")]
    pub delegate_policy: bool,
    /// Declare this VM or machine dedicated to Orch. Its directories use the machine as isolation.
    #[arg(long)]
    pub dedicated: bool,
    /// The system's address, as the installer was given it.
    #[arg(long, value_name = "URL")]
    pub url: String,
    /// The enrollment token the system showed: it begins `oenr_` and carries the pin of the system's
    /// authority, which this machine trusts the answer by.
    #[arg(long, value_name = "TOKEN")]
    pub token: String,
    /// What this machine is called in the system. The name of the host when left out.
    #[arg(long, value_name = "NAME")]
    pub name: Option<String>,
}

/// `run`.
#[derive(Debug, ClapArgs)]
pub struct Run {
    /// The system made this machine: enroll from the launch variables a driver sets when the data
    /// directory holds no identity, and take the limits from the system alone.
    #[arg(long)]
    pub managed: bool,
}

/// `policy`.
#[derive(Debug, Subcommand)]
pub enum Policy {
    /// Every key with its value, and which the owner handed to the system.
    Show,
    /// Set a key. A value is JSON (`true`, `["/work"]`); a word that is none is a text, and for
    /// a list of texts the words may be separated by commas.
    Set {
        /// The key, such as `environment.roots`.
        key: String,
        /// The value.
        value: String,
    },
    /// Let the system change a key (`all` for every one).
    Delegate {
        /// The key, or `all`.
        key: String,
    },
    /// Take a key back from the system (`all` for every one).
    Undelegate {
        /// The key, or `all`.
        key: String,
    },
}

/// `service`.
#[derive(Debug, Subcommand)]
pub enum Service {
    /// Install the service. It reaches out through the proxy this shell names (`HTTPS_PROXY`, `HTTP_PROXY`,
    /// `ALL_PROXY`, `NO_PROXY`), and on macOS or Windows where none is named, through the system's proxy settings.
    Install(ServiceInstall),
    /// Remove the service.
    Uninstall,
    /// Start the service.
    Start,
    /// Stop the service.
    Stop,
    /// Say whether the service runs.
    Status,
}

/// `service install`.
#[derive(Debug, ClapArgs)]
pub struct ServiceInstall {
    /// The proxy the service reaches out through instead of this shell's: `http://host:port`,
    /// `socks5://host:port` or `socks5h://host:port` (the proxy resolves names), with `user:password@` where it
    /// asks for them; or `none`, to reach every host directly.
    #[arg(long, value_name = "ADDRESS")]
    pub proxy: Option<String>,
    /// The hosts reached directly, comma-separated (names, with the names below them, addresses and networks),
    /// instead of this shell's `NO_PROXY`.
    #[arg(long, value_name = "HOSTS")]
    pub no_proxy: Option<String>,
}

/// `env`.
#[derive(Debug, Subcommand)]
pub enum Env {
    /// Allow environments in a directory.
    AddRoot {
        /// The directory.
        dir: PathBuf,
    },
    /// Stop allowing environments in a directory.
    RemoveRoot {
        /// The directory.
        dir: PathBuf,
    },
    /// List the directories.
    ListRoots,
}

/// `mcp`.
#[derive(Debug, Subcommand)]
pub enum Mcp {
    /// List the servers the harnesses have configured.
    List,
    /// Keep a copy of one of them for Orch.
    Import {
        /// The server's name.
        name: String,
        /// The harness whose configuration names it.
        #[arg(long, value_name = "HARNESS")]
        from: Option<String>,
    },
    /// Forget a copy.
    Forget {
        /// The server's name.
        name: String,
    },
}

/// `provider`.
#[derive(Debug, Subcommand)]
pub enum Provider {
    /// Read a key from standard input and keep it, sealed to this machine, for a provider. The key
    /// is never sent to the system: the provider is started here without it.
    SetKey {
        /// The provider, by the identifier the system shows for it.
        #[arg(long, value_name = "ID")]
        provider: String,
    },
}

/// The local role commands; the system cannot execute these as policy updates.
#[derive(Debug, Subcommand)]
pub enum Roles {
    /// Show the configured connector roles.
    Show,
    /// Set connector roles; restart the connector to activate the new set.
    Set {
        /// One or more of environment, agent, provider, separated by spaces or commas.
        #[arg(required=true,num_args=1..,value_delimiter=',')]
        roles: Vec<String>,
    },
}

/// `port`.
#[derive(Debug, Subcommand)]
pub enum Port {
    /// Listen on this machine's loopback and carry each connection, or each UDP sender, to a port
    /// of an environment. The person's token, or the whole link, is read from standard input.
    Forward(PortForward),
}

/// Read-only queries of the environment named by ORCH_PORTS_URL.
#[derive(Debug, Subcommand)]
pub enum Ports {
    /// Show listeners and openings; --look asks for a fresh observation.
    List {
        /// Ask the environment for a fresh observation.
        #[arg(long)]
        look: bool,
        /// Restrict the list to one listener number.
        #[arg(long, value_parser=clap::value_parser!(u16).range(1..))]
        port: Option<u16>,
        /// Maximum whole rows in the page, from 1 to 200.
        #[arg(long, value_parser=clap::value_parser!(u32).range(1..=200))]
        limit: Option<u32>,
        /// Continue with the exact cursor from the preceding page.
        #[arg(long)]
        cursor: Option<String>,
    },
    /// Show the stable address of a listener, including one not listening yet.
    Address {
        /// The listener number.
        #[arg(value_parser=clap::value_parser!(u16).range(1..))]
        port: u16,
    },
    /// Identify the exact native peer of an active connection or UDP sender.
    Whois {
        /// The literal peer IP and port returned by the program's socket.
        #[arg(long)]
        peer: std::net::SocketAddr,
        /// The program's own listener number.
        #[arg(long, value_parser=clap::value_parser!(u16).range(1..))]
        listener_port: u16,
        /// The actual socket transport.
        #[arg(long, value_enum)]
        transport: ForwardTransport,
    },
}

/// The transport `port forward` carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ForwardTransport {
    /// TCP connections.
    Tcp,
    /// UDP datagrams.
    Udp,
}

/// `port forward`.
#[derive(Debug, ClapArgs)]
#[command(group(
    clap::ArgGroup::new("credential")
        .required(true)
        .args(["token_stdin", "link_stdin"])
))]
pub struct PortForward {
    /// The environment, for a person's own or named-place use.
    #[arg(long, value_name = "ID", requires_all = ["port", "token_stdin"])]
    pub environment: Option<orch_types::Id>,
    /// Its port.
    #[arg(long, value_name = "PORT", requires = "environment")]
    pub port: Option<u16>,
    /// TCP or UDP.
    #[arg(long, value_enum)]
    pub transport: ForwardTransport,
    /// Read the person's token from standard input.
    #[arg(long, requires_all = ["environment", "url"])]
    pub token_stdin: bool,
    /// Read the whole link from standard input; it names the installation itself.
    #[arg(long)]
    pub link_stdin: bool,
    /// The installation's address, for a person's token.
    #[arg(long, value_name = "URL")]
    pub url: Option<String>,
    /// The loopback address to listen on; a free port of 127.0.0.1 when left out.
    #[arg(long, value_name = "ADDRESS")]
    pub listen: Option<std::net::SocketAddr>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn port_forward_takes_its_credential_from_standard_input_and_one_kind_of_it() {
        let Some(Command::Port(Port::Forward(link))) =
            parse(&["port", "forward", "--link-stdin", "--transport", "udp"]).command
        else {
            panic!("not port forward");
        };
        assert!(link.link_stdin && !link.token_stdin);
        assert_eq!(link.transport, ForwardTransport::Udp);
        let environment = orch_types::new_id().to_string();
        let Some(Command::Port(Port::Forward(person))) = parse(&[
            "port",
            "forward",
            "--environment",
            &environment,
            "--port",
            "8080",
            "--transport",
            "tcp",
            "--token-stdin",
            "--url",
            "https://o.example",
            "--listen",
            "127.0.0.1:9000",
        ])
        .command
        else {
            panic!("not port forward");
        };
        assert_eq!(person.port, Some(8080));
        assert!(
            Args::try_parse_from([
                "orch-connector",
                "port",
                "forward",
                "--transport",
                "tcp",
                "--link-stdin",
                "--token-stdin",
            ])
            .is_err()
        );
        assert!(
            Args::try_parse_from(["orch-connector", "port", "forward", "--transport", "tcp"])
                .is_err()
        );
    }

    fn parse(words: &[&str]) -> Args {
        let mut line = vec!["orch-connector"];
        line.extend_from_slice(words);
        Args::try_parse_from(line).unwrap()
    }

    #[test]
    fn the_command_line_is_consistent() {
        Args::command().debug_assert();
    }

    #[test]
    fn no_command_means_run_and_the_data_directory_may_come_before_or_after() {
        assert!(parse(&[]).command.is_none());
        let first = parse(&["--data-dir", "/d", "run", "--managed"]);
        let last = parse(&["run", "--managed", "--data-dir", "/d"]);
        assert_eq!(first.data_dir, Some(PathBuf::from("/d")));
        assert_eq!(last.data_dir, Some(PathBuf::from("/d")));
        assert!(matches!(
            first.command,
            Some(Command::Run(Run { managed: true }))
        ));
    }

    #[test]
    fn enroll_takes_the_address_and_the_token_and_may_be_given_a_name() {
        let args = parse(&[
            "enroll",
            "--url",
            "https://o.example",
            "--token",
            "oenr_x",
            "--name",
            "laptop",
        ]);
        let Some(Command::Enroll(enroll)) = args.command else {
            panic!("not enroll");
        };
        assert_eq!(enroll.url, "https://o.example");
        assert_eq!(enroll.token, "oenr_x");
        assert_eq!(enroll.name.as_deref(), Some("laptop"));
        assert!(Args::try_parse_from(["orch-connector", "enroll", "--url", "u"]).is_err());
    }

    #[test]
    fn the_policy_commands_of_the_manual_are_there() {
        for words in [
            &["policy", "show"][..],
            &["policy", "set", "update.auto", "false"],
            &["policy", "delegate", "all"],
            &["policy", "undelegate", "environment.roots"],
        ] {
            assert!(
                matches!(parse(words).command, Some(Command::Policy(_))),
                "{words:?}"
            );
        }
    }

    #[test]
    fn every_command_parses() {
        for words in [
            &["console"][..],
            &["run"],
            &["status"],
            &["version"],
            &["policy", "show"],
            &["env", "list-roots"],
            &["env", "add-root", "/srv/work"],
            &["service", "install"],
            &["service", "status"],
            &["doctor"],
            &["update"],
            &["mcp", "list"],
            &["mcp", "forget", "files"],
            &["provider", "set-key", "--provider", "p"],
        ] {
            assert!(parse(words).command.is_some(), "{words:?}");
        }
    }
}
