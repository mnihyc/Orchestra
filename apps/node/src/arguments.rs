//! The command line of the node: what an operator types to enroll a host, run it and look at it.

use std::path::PathBuf;

use clap::{Args as ClapArgs, Parser, Subcommand};

/// The Orch node: it lets the system make and keep the units of managed runtimes on this host.
#[derive(Debug, Parser)]
#[command(name = "orch-node", version = env!("ORCH_RELEASE_VERSION"))]
pub struct Args {
    /// The deployment's configuration file: the `[node]` table is the node's.
    #[arg(
        long,
        global = true,
        env = "ORCH_CONFIG",
        default_value = "/etc/orch/orch.toml"
    )]
    pub config: PathBuf,
    /// Where the node keeps its identity, its spool, its records and its tool cache; the `[node] data_dir` of
    /// the configuration when left out.
    #[arg(long, global = true, env = "ORCH_NODE_DATA_DIR", value_name = "DIR")]
    pub data_dir: Option<PathBuf>,
    /// What to do. Without one the node runs, as `run` does.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// What the node can be told to do.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Enroll this host into a system with a host token from it.
    Enroll(Enrollment),
    /// Load the drivers and hold the session to the system until stopped. A host that is not enrolled yet is
    /// enrolled first, when a token is given.
    Run(Enrollment),
    /// Show who this host is to the system and what it was told to load.
    Status,
    /// Verify and install the system's recommended node build, then restart its installed service.
    Update,
    /// Show a plan, then remove this node's service, installer configuration and program.
    Uninstall(Uninstall),
    /// Record files placed by the verified installer.
    #[command(name = "__record-installation", hide = true)]
    RecordInstallation(RecordInstallation),
    /// Show, set, hand over or take back settings in this machine's single policy file.
    #[command(subcommand)]
    Policy(Policy),
    /// Show or set this host's own ceiling for host ports, which no page of the host can widen.
    #[command(subcommand)]
    Ports(Ports),
    /// Show the version of this build.
    Version,
    /// Internal one-shot namespace operation; it opens no node session or configuration.
    #[command(name = "__runtime-helper", hide = true)]
    RuntimeHelper(crate::runtime_helper::Args),
    /// Internal native build runner; started only in its disposable unit.
    #[command(name = "__harness-conformance", hide = true)]
    HarnessConformance(crate::conformance::Args),
}

/// A deliberate local uninstall and its data choice.
#[derive(Debug, ClapArgs)]
pub struct Uninstall {
    /// Also remove this system's runtime objects and the node's data directory.
    #[arg(long)]
    pub purge: bool,
    /// Show the plan without stopping or changing anything.
    #[arg(long, conflicts_with = "yes")]
    pub plan: bool,
    /// Confirm the displayed plan without typing the host's name.
    #[arg(long)]
    pub yes: bool,
    /// The bundled updater owns its files; remove only the node's runtime objects.
    #[arg(long, hide = true)]
    pub runtime_only: bool,
}

/// Exact installer ownership used by the local uninstall.
#[derive(Debug, ClapArgs)]
pub struct RecordInstallation {
    /// The installer wrote the node configuration; an existing supplied file is kept.
    #[arg(long)]
    pub configuration: bool,
    /// The installer registered orch-node.service.
    #[arg(long)]
    pub service: bool,
}

/// The same policy commands offered by orch-connector, over the same machine file.
#[derive(Debug, Clone, Subcommand)]
pub enum Policy {
    /// Show each offered key, meaning, value, both shipped values and local authority.
    Show,
    /// Explicitly keep a value on this machine (JSON, or plain words).
    Set { key: String, value: String },
    /// Hand a key, role section or all to Orch; explicit local values stay in force.
    Delegate { key: String },
    /// Keep a key, role section or all on this machine.
    Undelegate { key: String },
}

/// `ports`: a compatibility command for the ports member of the same policy file.
#[derive(Debug, Clone, Subcommand)]
pub enum Ports {
    /// The ceiling, and what the node would propose for the host's page now (which opens nothing).
    Show,
    /// States the whole ceiling: a member left out sets no ceiling; `--clear` takes the table away.
    Set(PortsSet),
}

/// What `ports set` states.
#[derive(Debug, Clone, ClapArgs)]
pub struct PortsSet {
    /// The incoming address callers are given, fixed: a name or an address.
    #[arg(long, value_name = "NAME-OR-ADDRESS")]
    pub incoming_address: Option<String>,
    /// `all` (any address of either family, or a narrower one), or the one local IP permitted.
    #[arg(long, value_name = "all|IP")]
    pub listen: Option<String>,
    /// A range a declaration's ranges must lie in, `FROM-TO` or one number; again for more.
    #[arg(long = "range", value_name = "FROM-TO")]
    pub ranges: Vec<String>,
    /// Permit no host opening at all (`ranges = []`).
    #[arg(long, conflicts_with = "ranges")]
    pub no_ranges: bool,
    /// Clear the public listener declaration and its optional range constraints.
    #[arg(long, conflicts_with_all = ["incoming_address", "listen", "ranges", "no_ranges"])]
    pub clear: bool,
}

/// What enrolling takes.
#[derive(Debug, Clone, ClapArgs)]
pub struct Enrollment {
    /// Hand the node's policy over locally; an enrollment token cannot choose it silently.
    #[arg(long)]
    pub delegate_policy: bool,
    /// The system's address; the `[node] gateway_url` of the configuration when left out.
    #[arg(long, value_name = "URL")]
    pub url: Option<String>,
    /// The host token the system showed: it begins `oenr_` and carries the pin of the system's authority,
    /// which this host trusts the answer by.
    #[arg(
        long,
        env = "ORCH_NODE_TOKEN",
        value_name = "TOKEN",
        conflicts_with = "token_file"
    )]
    pub token: Option<String>,
    /// A file that holds the token, as the bundled deployment mounts it (`node_enrollment_token`).
    #[arg(long, value_name = "FILE")]
    pub token_file: Option<PathBuf>,
    /// What this host is called in the system; the name of the machine when left out.
    #[arg(long, value_name = "NAME")]
    pub name: Option<String>,
}
