//! Roles components.
//!
//! A **role** is something the dispatcher hands commands to: the agent role hosts harnesses, the
//! provider role keeps credentials and forwards model calls, the environment role is the exec node of a
//! workspace. A role says which kinds of command it takes ([`RoleHandler::takes`]) and answers them
//! ([`RoleHandler::handle`]); the dispatcher has already held the command to the machine's local policy
//! and bounds it by its deadline, so a role sees only commands it may carry out. A role may add to what
//! the machine reports of itself ([`RoleHandler::contribute`]), take up what an earlier run left
//! ([`RoleHandler::recover`]) and clean up when the connector stops ([`RoleHandler::stop`]).
//!
//! To add a role: implement [`RoleHandler`], and give it to the connector builder with
//! [`crate::connector::ConnectorBuilder::role`]. What a role needs of the connector around it is in
//! [`Services`]. A role whose commands the catalogue has and which no handler takes is answered
//! `unsupported`, naming the command no registered role accepts.

pub mod agent;
pub mod environment;
pub mod ports;
pub mod provider;

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use orch_config::ConnectorConfig;
use orch_link::commands::{CommandContext, CommandExecution};
use orch_link_contract::commands::{Command, CommandError, CommandKind, CommandResult};
use orch_link_contract::session::Role;
use orch_pki::certificates::LeafKey;
use orch_types::{Clock, Id};

use crate::content::Content;
use crate::events::Publisher;
use crate::facts::FactsDraft;
use crate::harnesses::Hosting;
use crate::loopback::Loopback;
use crate::policy::{Clamp, PolicyStore};
use crate::tunnels::Tunnels;

/// The key a machine opens the values sealed to it with, and seals values for another machine with.
pub struct Keys {
    key: LeafKey,
}

impl std::fmt::Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keys").finish_non_exhaustive()
    }
}

impl Keys {
    /// Keys over the machine's private key.
    #[must_use]
    pub const fn new(key: LeafKey) -> Self {
        Self { key }
    }

    /// Opens a value that was sealed to this machine for the command `command`.
    pub fn open(&self, command: Id, sealed: &str) -> Result<String, CommandError> {
        self.open_with(&orch_pki::sealing::info_for_command(command), sealed)
    }

    /// Seals a value to another machine, for the command `command`.
    pub fn seal_to(
        &self,
        command: Id,
        public_key: &str,
        value: &str,
    ) -> Result<String, CommandError> {
        self.seal_with(
            public_key,
            &orch_pki::sealing::info_for_command(command),
            value,
        )
    }

    /// Seals a value to another machine, bound to `info`.
    pub fn seal_with(
        &self,
        public_key: &str,
        info: &[u8],
        value: &str,
    ) -> Result<String, CommandError> {
        use orch_pki::sealing::{RecipientKey, seal};
        let recipient = RecipientKey::parse(public_key)
            .map_err(|e| seal_failure(format!("the key to seal to is not one: {e}")))?;
        seal(&recipient, info, value.as_bytes())
            .map_err(|e| seal_failure(format!("a value could not be sealed: {e}")))
    }

    /// Seals a value to this machine's own key, bound to `info`: what the sealed store keeps.
    pub fn seal_for_self(&self, info: &[u8], value: &str) -> Result<String, CommandError> {
        use orch_pki::sealing::{RecipientKey, seal};
        let recipient = RecipientKey::from_point(&self.key.public_point())
            .map_err(|e| seal_failure(format!("this machine's key cannot be sealed to: {e}")))?;
        seal(&recipient, info, value.as_bytes())
            .map_err(|e| seal_failure(format!("a value could not be sealed: {e}")))
    }

    /// Opens a value that was sealed to this machine's key and bound to `info`.
    pub fn open_with(&self, info: &[u8], sealed: &str) -> Result<String, CommandError> {
        let bytes = orch_pki::sealing::open(&self.key, info, sealed).map_err(|error| {
            seal_failure(format!("a sealed value could not be opened: {error}"))
        })?;
        String::from_utf8(bytes).map_err(|_| seal_failure("a sealed value is not text".to_owned()))
    }
}

fn seal_failure(what: String) -> CommandError {
    CommandError::new(orch_link_contract::commands::ErrorCode::Internal, what)
}

/// Where the harness processes of the agent role are, for a role that needs to reach one: the provider role
/// asks the harness that holds a provider's sign-in to issue a credential from it. Empty on a machine that
/// does not play the agent role.
#[derive(Debug, Default)]
pub struct AgentSlot {
    hosting: OnceLock<Arc<Hosting>>,
}

impl AgentSlot {
    /// The harness processes, if the machine plays the agent role.
    #[must_use]
    pub fn get(&self) -> Option<&Arc<Hosting>> {
        self.hosting.get()
    }

    /// Puts the hosting in the slot, once.
    pub fn fill(&self, hosting: Arc<Hosting>) {
        let _ = self.hosting.set(hosting);
    }
}

/// What a role gets from the connector around it.
#[derive(Clone)]
pub struct Services {
    /// The actual connection and its current preference list.
    pub link: orch_link::machine::MachineLink,
    /// Resource controllers proved under the account running this physical connector.
    pub process_controls: orch_driver_contract::process_limits::ProcessControls,
    /// Where the time comes from.
    pub clock: Arc<dyn Clock>,
    /// The connector's data directory.
    pub data_dir: PathBuf,
    /// Where events are put.
    pub publisher: Arc<dyn Publisher>,
    /// The machine's local policy.
    pub policy: Arc<PolicyStore>,
    /// Provider programs registered beside this machine's credentials.
    pub provider_sources: Arc<crate::sources::Programs>,
    /// The built-in sources of a key's facts the program registered: the vendors' readers, by name.
    pub builtin_sources: Arc<orch_provider_contract::builtins::BuiltinSources>,
    /// Descriptions and opened address bindings live only on this keeping connector.
    pub(crate) source_runtime: Arc<crate::sources::dispatch::Runtime>,
    /// The listener that serves harnesses beside the connector.
    pub loopback: Arc<Loopback>,
    /// Moving content to and from the system.
    pub content: Arc<Content>,
    /// The machine's key.
    pub keys: Arc<Keys>,
    /// The configuration as the connector started with it.
    pub config: ConnectorConfig,
    /// The read-only directory of tools a managed runtime is given, if it was.
    pub tools_dir: Option<PathBuf>,
    /// The harnesses that directory held when the connector started: the builds the system put in this
    /// runtime, which it reports among the harnesses it has and its environment lists among its tools.
    pub installed: Vec<crate::builds::Installed>,
    /// The tunnels: those other machines open to this one are handed to the role that serves them, and a role
    /// opens its own here.
    pub tunnels: Arc<Tunnels>,
    /// Public listeners owned by this physical machine, shared by all of its roles.
    pub port_publisher: Arc<crate::host_publish::Publisher>,
    /// The harness processes of the agent role.
    pub agents: Arc<AgentSlot>,
}

impl std::fmt::Debug for Services {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Services").finish_non_exhaustive()
    }
}

impl Services {
    /// The harnesses the system put in this runtime, as the machine's facts list the harnesses found on it:
    /// whatever role the machine plays, since an executor of an environment runs a harness's build as well as
    /// a harness does.
    pub fn installed_harnesses(
        &self,
    ) -> impl Iterator<Item = orch_link_contract::session::MachineFactsHarness> + '_ {
        self.installed
            .iter()
            .map(|found| orch_link_contract::session::MachineFactsHarness {
                kind: found.kind.clone(),
                path: found.program.display().to_string(),
                version: found.version.clone(),
                daemon: false,
            })
    }
}

/// A command as a role is handed it.
#[derive(Debug, Clone)]
pub struct Call {
    /// What the link knows of it: its identifier, its key, its deadline, and the token that fires when
    /// the system asks for it not to be carried out.
    pub context: CommandContext,
    /// What the machine's ceilings lowered in it before it got here.
    pub clamps: Vec<Clamp>,
}

/// Something that carries out commands of one role.
#[async_trait]
pub trait RoleHandler: Send + Sync {
    /// Reconciles external work from an interrupted journal entry before acknowledging that it stopped.
    async fn interrupted(&self, _id: Id, _command: &Command) -> Result<(), CommandError> {
        Ok(())
    }
    /// Ordinary commands complete under their serial key; recording may return a captured publication.
    async fn begin(&self, call: Call, command: Command) -> CommandExecution {
        CommandExecution::ready(self.handle(call, command).await)
    }
    /// The role it is.
    fn role(&self) -> Role;

    /// The kinds of command it takes.
    fn takes(&self) -> &'static [CommandKind];

    /// Whether this role answers `command`. A kind that is addressed to either of two roles (a provider's
    /// models, asked of the machine that holds its sign-in or of the one that holds its key) can be taken by
    /// both; the one that holds what the command names answers it. A role that takes a kind alone answers
    /// all of it, which is what the default says.
    fn claims(&self, _command: &Command) -> bool {
        true
    }

    /// Carries out a command of one of those kinds. The result has to be that of the command's kind.
    async fn handle(&self, call: Call, command: Command) -> Result<CommandResult, CommandError>;

    /// Adds to what the machine reports of itself.
    fn contribute(&self, _facts: &mut FactsDraft) {}

    /// Takes up what an earlier run of the connector left, once the link is there to report on it.
    async fn recover(&self) {}

    /// Looks again at local discoveries when a new authenticated link session stands.
    async fn connected(&self, _session: Id) {}

    /// Closes any role-owned outward gate when the authenticated link is lost.
    async fn disconnected(&self) {}

    /// Stops what the role runs.
    async fn stop(&self) {}

    /// Ends controller-side background tasks when simulating an abrupt process exit. It must not
    /// change retained records or stop external units; the next process recovers those.
    fn abandon(&self) {}
}
