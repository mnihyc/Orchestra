//! Connector configuration and operational tunables.
//!
//! A connector's `config.toml`, on the machine it runs on. A runtime the
//! system creates takes these defaults unless its template brings a file.
//!
//! The file holds three kinds of thing. The operational tunables (`[spool]`,
//! `[journal]`, `[envhost]`, `[adapter]`, `[provider]`, `[log]`) are numbers
//! an operator may move. What the connector is (`data_dir`, `[connector]`,
//! `endpoints`, `[[plugin]]`) is written when it is enrolled or registered.
//! And the local policy, the limits its owner sets on a machine that a
//! person connected: each key of it is a key of a table, so that
//! `environment.roots` is `roots` in `[environment]`. The server can change
//! only the keys that `[policy] delegated` hands over.

use crate::loader::{ConfigError, Validate};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

mod driver_options;

fn s(text: &str) -> String {
    text.to_owned()
}

macro_rules! section {
    ($(#[$meta:meta])* $name:ident { $( $(#[$fmeta:meta])* $field:ident : $ty:ty = $default:expr ),* $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[serde(default, deny_unknown_fields)]
        pub struct $name {
            $( $(#[$fmeta])* pub $field: $ty, )*
        }

        impl Default for $name {
            fn default() -> Self {
                Self { $( $field: $default, )* }
            }
        }
    };
}

section! {
    /// The whole of a connector's `config.toml`.
    ConnectorConfig {
        /// Where the connector keeps its identity, spool, journal and the
        /// homes of its harnesses; empty takes the default of the platform
        /// (`ORCH_DATA_DIR` names it for a runtime the system made).
        data_dir: String = String::new(),
        /// What this connector is and what it does.
        connector: Machine = Machine::default(),
        /// The addresses of the gateway, in the order to try them: what
        /// enrollment gave.
        endpoints: Vec<ConnectorEndpoint> = Vec::new(),
        /// External implementations registered on this machine by its owner.
        plugin: Vec<ConnectorPlugin> = Vec::new(),
        /// Named directories for owned data, using the same shape as node storages.
        /// Empty uses the allowed environment roots in their configured order.
        #[serde(rename="storages",alias="storage")]
        storage: Vec<crate::server::NodeStorage> = Vec::new(),
        /// The node role's unit resource ceilings.
        resources:orch_link_contract::policy::MachineResources=orch_link_contract::policy::MachineResources::default(),
        /// Driver offers and ceilings shared by nodes and the machine policy page.
        #[serde(serialize_with="driver_options::serialize_drivers",deserialize_with="driver_options::deserialize_drivers")]
        drivers:orch_link_contract::policy::MachineDrivers=orch_link_contract::policy::MachineDrivers::default(),
        /// The directories environments may be in, and what may be written
        /// from outside.
        environment: EnvironmentPolicy = EnvironmentPolicy::default(),
        /// The tool servers found here, and those written from the system.
        mcp: McpPolicy = McpPolicy::default(),
        /// What the exec node may offer.
        exec: ExecPolicy = ExecPolicy::default(),
        /// What the system may ask of a harness here.
        agent: AgentPolicy = AgentPolicy::default(),
        /// Whether builds may be fetched.
        tools: ToolsPolicy = ToolsPolicy::default(),
        /// Who may change which roles this physical connector offers.
        machine: MachinePolicy = MachinePolicy::default(),
        /// Whether the connector updates itself.
        update: UpdatePolicy = UpdatePolicy::default(),
        /// What the system may change of this policy.
        policy: PolicyHandover = PolicyHandover::default(),
        /// The machine policy's lifecycle programs, run by the hosting program.
        hooks: orch_link_contract::policy::MachineHooks = orch_link_contract::policy::MachineHooks::default(),
        /// Opening audiences and the reachable listener declaration of this machine.
        ports: orch_link_contract::policy::MachinePorts = orch_link_contract::policy::MachinePorts::default(),
        /// The local spool of events the system has not acknowledged yet.
        spool: Spool = Spool::default(),
        /// The journal of command results.
        journal: Journal = Journal::default(),
        /// The environment role.
        envhost: Envhost = Envhost::default(),
        /// Harness adapters.
        adapter: Adapter = Adapter::default(),
        /// The provider role.
        provider: Provider = Provider::default(),
        /// The connector's own log files.
        log: Log = Log::default(),
    }
}

section! {
    /// `[connector]`
    Machine {
        /// What this machine is called in the system; empty takes the name of the host.
        name: String = String::new(),
        /// Local declaration that this machine is dedicated to Orch; never a delegated policy key.
        dedicated:bool = false,
        /// Whether legacy orch.toml node policy values were imported into this file once.
        node_policy_imported:bool = false,
        /// Version of that import; older imports did not include lifecycle hooks.
        node_policy_import_version:u32 = 0,
        /// The roles it plays: `agent`, `environment`, `provider`, and `node` for the one machine of a
        /// host that makes managed runtimes.
        roles: Vec<String> = vec![s("agent"), s("environment")],
    }
}

section! {
    /// One address of the gateway.
    ConnectorEndpoint {
        /// The address.
        url: String = String::new(),
        /// How it is verified: `pinned` (the system's authority and a client
        /// certificate) or `public` (public roots and a signed handshake).
        trust: String = s("pinned"),
    }
}

section! {
    /// `[[plugin]]`: an external implementation of an extension point, run
    /// as the connector's user. Exactly one of `command` and `connect` is given.
    ConnectorPlugin {
        /// This entry was registered through Orch and may be removed there while its policy permits it.
        registered_by_orch:bool = false,
        /// `provider`, `adapter`, `executor` or `translator`.
        kind: String = String::new(),
        /// Its name.
        name: String = String::new(),
        /// The program and its arguments; the connector starts it.
        command: Vec<String> = Vec::new(),
        /// Or an address of one that already runs: `unix:<path>` or `tcp://127.0.0.1:<port>`.
        connect: String = String::new(),
        /// Its environment.
        env: BTreeMap<String, String> = BTreeMap::new(),
        /// When it is started again: `never`, `on-failure` or `always`.
        restart: String = s("on-failure"),
    }
}

section! {
    /// `[environment]`: policy keys `environment.*`
    EnvironmentPolicy {
        /// Directories inside which an environment may have its root and a mount its location.
        roots: Vec<String> = Vec::new(),
        /// Whether file writes arriving through the system are accepted.
        remote_write: bool = false,
        /// Whether interactive shells may be opened through the system.
        terminal: bool = false,
        /// Whether named commands run after an environment or provider becomes available.
        after_up: bool = false,
        /// Whether browsers may be shown, started and driven through the system.
        browser: bool = false,
        /// Policy key `environment.vnc`: permits virtual screens and announced local VNC servers, including a served personal desktop.
        vnc: bool = false,
        /// Whether files that arrive with a message to an agent are written into the workspace.
        deliver_files: bool = true,
    }
}

section! {
    /// `[mcp]`: policy keys `mcp.*`
    McpPolicy {
        /// Whether the tool servers the harnesses here configured may be listed and imported.
        import: bool = true,
        /// Whether a tool server may be written from the system for an environment here.
        add: bool = false,
    }
}

section! {
    /// `[exec]`: policy key `exec.offers`
    ExecPolicy {
        /// What the exec node may offer a harness that runs elsewhere: executor kinds and tool sets.
        offers: Vec<String> = orch_agent_programs::Programs::builtin().every_offer(),
    }
}

section! {
    /// A harness kind this machine may run, with where its program is.
    AgentHarness {
        /// The harness kind.
        kind: String = String::new(),
        /// Its program, or the daemon to attach to.
        path: String = String::new(),
    }
}

section! {
    /// `[agent]`: policy keys `agent.*`
    AgentPolicy {
        /// The widest sandbox mode the system may ask for: `read_only`, `workspace` or `open`.
        max_sandbox: String = s("workspace"),
        /// The harness kinds this machine may run.
        harnesses: Vec<AgentHarness> = Vec::new(),
        /// Permit handed-over registration of agent adapter programs at run time.
        programs:bool = false,
    }
}

section! {
    /// `[tools]`: policy key `tools.fetch`
    ToolsPolicy {
        /// Whether the connector may download the builds the system names.
        fetch: bool = false,
    }
}

section! {
    /// `[machine]`: policy key `machine.roles`; active roles stay in `[connector]`.
    MachinePolicy {
        /// Permit delegated role changes from Orch. Locally kept values still win.
        roles:bool = false,
    }
}

section! {
    /// `[update]`: policy key `update.auto`
    UpdatePolicy {
        /// Whether the connector updates itself.
        auto: bool = true,
    }
}

section! {
    /// `[policy]`: policy key `policy.delegated`
    PolicyHandover {
        /// The keys that may be changed from the system; `["*"]` hands over all of them.
        delegated: Vec<String> = Vec::new(),
        /// Sparse values accepted from Orch, separate from the operator's local constraints.
        #[serde(serialize_with="driver_options::serialize_values",deserialize_with="driver_options::deserialize_values")]
        values: orch_link_contract::policy::PolicyValues = BTreeMap::new(),
        /// Local key provenance when configuration was saved by Orch; absent means read explicit file keys.
        local_keys: Option<Vec<String>> = None,
    }
}

section! {
    /// `[spool]`
    Spool {
        /// The most bytes it holds.
        max_bytes: u64 = 268_435_456,
        /// The oldest event it holds.
        max_age_days: u64 = 7,
        /// The size of one of its files.
        segment_bytes: u64 = 8_388_608,
    }
}

section! {
    /// `[journal]`
    Journal {
        /// How long a command's result is kept, to answer the same command again.
        keep_hours: u64 = 24,
    }
}

section! {
    /// `[envhost]`
    Envhost {
        /// A full rescan of the workspace, for where watching falls short.
        rescan_secs: u64 = 60,
        /// How often a listing is uploaded while files keep changing.
        manifest_interval_secs: u64 = 300,
        /// Above this many entries a workspace is listed live only.
        index_max_entries: u64 = 250_000,
        /// The most seconds a full scan gets before the workspace is listed live.
        index_scan_max_secs: u64 = 10,
        /// Restarts of a crashing tool server before it is reported failed…
        mcp_restarts: u32 = 3,
        /// …within this time.
        mcp_restart_window_secs: u64 = 600,
    }
}

section! {
    /// `[adapter]`
    Adapter {
        /// Command output kept inline with its item.
        output_inline_bytes: u64 = 16_384,
        /// The most output kept for one command.
        output_max_bytes: u64 = 67_108_864,
        /// How many times a harness that died is started again…
        harness_restarts: u32 = 3,
        /// …within this time, before it is reported failed. The figures of the tool servers.
        harness_restart_window_secs: u64 = 600,
    }
}

section! {
    /// `[provider]`
    Provider {
        /// The largest model request that is forwarded.
        max_request_bytes: u64 = 33_554_432,
        /// Policy key `provider.signin`: permits Orch's sign-in and renewal in the person's harness home.
        signin: bool = false,
        /// Permit handed-over registration of provider source programs at run time.
        programs:bool = false,
        /// Policy key `provider.upstreams`: the base URLs model calls may be forwarded to.
        upstreams: Vec<String> = Vec::new(),
        /// Policy key `provider.arrangements`: the types in which a credential kept here may be used at
        /// the system's request. As installed, the two in which nothing of it leaves the machine.
        arrangements: Vec<String> = vec![s("separated"), s("key_separated")],
    }
}

section! {
    /// `[log]`
    Log {
        /// How long the connector keeps its own log files. They are never sent anywhere.
        keep_days: u64 = 14,
    }
}

/// The roles a connector may play. `node` is the host's one machine (a node is a connector with that role
/// and no other), and enrolling writes the roles it enrolled with into this file.
const ROLES: [&str; 4] = ["agent", "environment", "provider", "node"];
/// The sandbox modes, from the strictest.
const SANDBOX_MODES: [&str; 3] = ["read_only", "workspace", "open"];
/// The arrangement types in which a credential may be used at the system's request; `together` needs
/// nobody's consent, so it is not one of them.
const CONSENTED_ARRANGEMENTS: [&str; 4] = [
    "separated",
    "key_separated",
    "credential_shared",
    "handed_over",
];

impl Validate for ConnectorConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        let invalid = |key: &str, message: &str| ConfigError::Invalid {
            key: key.to_owned(),
            message: message.to_owned(),
        };
        let mut storage_names = std::collections::BTreeSet::new();
        for storage in &self.storage {
            if storage.name.trim().is_empty()
                || !storage_names.insert(&storage.name)
                || !std::path::Path::new(&storage.path).is_absolute()
            {
                return Err(invalid(
                    "storage",
                    "each storage needs a unique nonempty name and an absolute path",
                ));
            }
        }
        if !self.storage.is_empty()
            && self
                .storage
                .iter()
                .filter(|storage| storage.default)
                .count()
                != 1
        {
            return Err(invalid(
                "storage",
                "exactly one declared storage must be the default",
            ));
        }
        if !self.drivers.valid() {
            return Err(invalid(
                "drivers",
                "driver offers need valid allow-lists and absolute host launch programs",
            ));
        }
        // Connection values belong to the selected driver and its declared schema.
        // Validation happens when that implementation opens, including external drivers.
        if !self.hooks.valid() {
            return Err(invalid(
                "hooks",
                "hook executables must be absolute paths and timeout_secs must be nonzero",
            ));
        }
        if !self.ports.valid() {
            return Err(invalid(
                "ports",
                "a publish declaration needs a reachable address, listen address and valid ranges",
            ));
        }
        if let Some(key) = self.policy.local_keys.as_ref().and_then(|keys| {
            keys.iter().find(|key| {
                key.parse::<orch_link_contract::policy::PolicyKey>()
                    .is_err()
            })
        }) {
            return Err(invalid(
                key,
                "this name in policy.local_keys is not a machine policy key",
            ));
        }
        // A name that is not a policy key is said as the delegation's fault before the values are resolved with it.
        if self
            .policy
            .delegated
            .iter()
            .any(|key| !orch_link_contract::policy::PolicyKey::delegation_name(key))
        {
            return Err(invalid(
                "policy.delegated",
                "must name keys of the local policy other than itself, or `*`",
            ));
        }
        if let Err(error) = orch_link_contract::policy::LocalPolicy::resolve(
            &self.policy.delegated,
            &BTreeMap::new(),
            &self.policy.values,
        ) {
            return Err(invalid("policy.values", &error.to_string()));
        }
        if self.spool.segment_bytes == 0 || self.spool.segment_bytes > self.spool.max_bytes {
            return Err(invalid(
                "spool.segment_bytes",
                "must be more than 0 and at most spool.max_bytes",
            ));
        }
        if self.adapter.output_inline_bytes > self.adapter.output_max_bytes {
            return Err(invalid(
                "adapter.output_inline_bytes",
                "must be at most adapter.output_max_bytes",
            ));
        }
        if self.adapter.harness_restart_window_secs == 0 {
            return Err(invalid(
                "adapter.harness_restart_window_secs",
                "must be more than 0",
            ));
        }
        if self.connector.roles.is_empty()
            || self
                .connector
                .roles
                .iter()
                .any(|role| !ROLES.contains(&role.as_str()))
        {
            return Err(invalid(
                "connector.roles",
                "must name at least one of agent, environment, provider and node",
            ));
        }
        for endpoint in &self.endpoints {
            if endpoint.url.is_empty() || !matches!(endpoint.trust.as_str(), "pinned" | "public") {
                return Err(invalid(
                    "endpoints",
                    "each needs an address and a trust of pinned or public",
                ));
            }
        }
        if !SANDBOX_MODES.contains(&self.agent.max_sandbox.as_str()) {
            return Err(invalid(
                "agent.max_sandbox",
                "must be read_only, workspace or open",
            ));
        }
        if self
            .agent
            .harnesses
            .iter()
            .any(|harness| harness.kind.is_empty() || harness.path.is_empty())
        {
            return Err(invalid(
                "agent.harnesses",
                "each needs a kind and the path of its program",
            ));
        }
        if self
            .provider
            .arrangements
            .iter()
            .any(|kind| !CONSENTED_ARRANGEMENTS.contains(&kind.as_str()))
        {
            return Err(invalid(
                "provider.arrangements",
                "must be separated, key_separated, credential_shared or handed_over",
            ));
        }
        for plugin in &self.plugin {
            if !orch_plugin_contract::registration::PluginKind::ALL
                .iter()
                .any(|kind| {
                    kind.host() == orch_plugin_contract::registration::PluginHost::Connector
                        && kind.as_str() == plugin.kind
                })
            {
                return Err(invalid(
                    "plugin.kind",
                    "a connector runs providers, adapters, executors and translators",
                ));
            }
            if plugin.name.is_empty() || (plugin.command.is_empty() == plugin.connect.is_empty()) {
                return Err(invalid(
                    "plugin",
                    "needs a name and exactly one of command and connect",
                ));
            }
            if !matches!(plugin.restart.as_str(), "never" | "on-failure" | "always") {
                return Err(invalid(
                    "plugin.restart",
                    "must be never, on-failure or always",
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::load_text;

    #[test]
    fn named_storage_configuration_has_one_default_and_never_treats_a_path_as_a_template_value() {
        let config: ConnectorConfig = load_text(
            "[[storage]]\nname='fast'\npath='/srv/work'\ndefault=true\n",
            "test",
            none,
        )
        .unwrap();
        assert_eq!(config.storage[0].name, "fast");
        assert_eq!(config.storage[0].path, "/srv/work");
        for text in [
            "[[storage]]\nname='fast'\npath='relative'\ndefault=true\n",
            "[[storage]]\nname='fast'\npath='/srv/work'\n",
        ] {
            assert!(load_text::<ConnectorConfig>(text, "test", none).is_err());
        }
    }

    fn none(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn an_empty_file_is_a_connector_as_installed() {
        let config: ConnectorConfig = load_text("", "test", none).unwrap();
        assert_eq!(config, ConnectorConfig::default());
        assert!(!config.environment.remote_write);
        assert!(!config.environment.terminal);
        assert!(config.environment.deliver_files);
        assert!(config.mcp.import && !config.mcp.add);
        assert_eq!(config.agent.max_sandbox, "workspace");
        assert_eq!(config.provider.arrangements, ["separated", "key_separated"]);
        assert!(config.policy.delegated.is_empty());
        assert_eq!(config.connector.roles, ["agent", "environment"]);
    }

    #[test]
    fn the_policy_keys_are_written_as_the_tables_they_are_named_by() {
        let text = r#"
            [environment]
            roots = ["/srv/work"]
            remote_write = true
            [agent]
            max_sandbox = "read_only"
            harnesses = [{ kind = "codex", path = "/usr/local/bin/codex" }]
            [policy]
            delegated = ["environment.roots", "agent.max_sandbox"]
            [[endpoints]]
            url = "wss://gw.example.com:9443"
            trust = "pinned"
        "#;
        let config: ConnectorConfig = load_text(text, "test", none).unwrap();
        assert_eq!(config.environment.roots, ["/srv/work"]);
        assert_eq!(config.agent.harnesses[0].kind, "codex");
        assert_eq!(config.endpoints[0].trust, "pinned");
    }

    #[test]
    fn a_key_the_policy_does_not_have_stops_the_start() {
        let error = load_text::<ConnectorConfig>("[mcp]\nimporting = true\n", "config.toml", none)
            .unwrap_err();
        assert!(error.to_string().contains("importing"), "{error}");
    }

    #[test]
    fn the_launch_variable_of_a_managed_runtime_names_the_data_directory() {
        let variable = |name: &str| (name == "ORCH_DATA_DIR").then(|| "/var/lib/orch".to_owned());
        let config: ConnectorConfig = load_text("", "test", variable).unwrap();
        assert_eq!(config.data_dir, "/var/lib/orch");
    }

    #[test]
    fn a_list_in_the_environment_replaces_a_list_of_the_file() {
        let variable =
            |name: &str| (name == "ORCH_ENVIRONMENT_ROOTS").then(|| "[\"/a\", \"/b\"]".to_owned());
        let config: ConnectorConfig =
            load_text("[environment]\nroots = [\"/c\"]\n", "test", variable).unwrap();
        assert_eq!(config.environment.roots, ["/a", "/b"]);
    }

    #[test]
    fn values_that_cannot_hold_are_refused_with_their_key() {
        for (text, key) in [
            ("[agent]\nmax_sandbox = \"anything\"\n", "agent.max_sandbox"),
            (
                "[provider]\narrangements = [\"together\"]\n",
                "provider.arrangements",
            ),
            (
                "[policy]\ndelegated = [\"policy.delegated\"]\n",
                "policy.delegated",
            ),
            (
                "[policy]\ndelegated = [\"mcp.nonsense\"]\n",
                "policy.delegated",
            ),
            ("[connector]\nroles = []\n", "connector.roles"),
            (
                "[[endpoints]]\nurl = \"wss://x\"\ntrust = \"maybe\"\n",
                "endpoints",
            ),
            ("[[plugin]]\nkind = \"adapter\"\nname = \"x\"\n", "plugin"),
        ] {
            let error = load_text::<ConnectorConfig>(text, "test", none).unwrap_err();
            assert!(error.to_string().starts_with(key), "{text}: {error}");
        }
    }

    #[test]
    fn the_file_enrolling_writes_for_a_node_reads_again() {
        let config: ConnectorConfig =
            load_text("[connector]\nroles = [\"node\"]\n", "test", none).unwrap();
        assert_eq!(config.connector.roles, ["node"]);
        let error =
            load_text::<ConnectorConfig>("[connector]\nroles = [\"janitor\"]\n", "test", none)
                .unwrap_err();
        assert!(error.to_string().starts_with("connector.roles"), "{error}");
    }

    #[test]
    fn a_plugin_is_started_by_command_or_reached_by_address_and_never_both() {
        let started =
            "[[plugin]]\nkind = \"adapter\"\nname = \"acme\"\ncommand = [\"/opt/acme\"]\n";
        let reached =
            "[[plugin]]\nkind = \"adapter\"\nname = \"acme\"\nconnect = \"unix:/run/acme.sock\"\n";
        let both = "[[plugin]]\nkind = \"adapter\"\nname = \"acme\"\ncommand = [\"/opt/acme\"]\nconnect = \"unix:/run/acme.sock\"\n";
        assert!(load_text::<ConnectorConfig>(started, "test", none).is_ok());
        assert!(load_text::<ConnectorConfig>(reached, "test", none).is_ok());
        assert!(load_text::<ConnectorConfig>(both, "test", none).is_err());
    }
}
